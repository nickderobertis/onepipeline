//! Private record addresses are removed before a public copy is rendered.

use std::collections::BTreeSet;

use super::{
    Engine, EngineError, Level, Planned, Running, content_of, fits, located, location_of,
    metadata_of, metadata_ref, refused, set_content, stops_a_location, stored,
};
use crate::config::SourceVisibility;
use crate::section_links::SectionForm;
use onetaskgraph_plugin_api::{
    DocumentQuery, PageRequest, ProjectQuery, SourceError, Support, TaskQuery,
};

impl Engine {
    /// Write `item`'s body and carried comments with each reference-style link inline when its
    /// destination would keep such a link only as text, before anything reads them for what they
    /// name: an inlined link is then rewritten, screened and counted as any inline link is. A
    /// rendering's provenance moves with its body, as a rewrite of its references moves it.
    fn inline_reference_links(&self, item: &mut Planned) -> Result<(), EngineError> {
        if !SectionForm::for_kind(self.writable(&item.to)?.kind())
            .is_some_and(SectionForm::inlines_reference_links)
        {
            return Ok(());
        }
        if let Some(content) = content_of(&item.item).map(str::to_owned) {
            let inlined = crate::section_links::inline_reference_links(&content);
            if inlined != content {
                stored::restamp(metadata_of(&mut item.item), &[&content], &inlined);
                set_content(&mut item.item, inlined);
            }
        }
        for comment in item.carried.iter_mut().flatten() {
            comment.body = crate::section_links::inline_reference_links(&comment.body);
        }
        Ok(())
    }

    /// Remove private addresses before the boundary check sees a public body's content.
    pub(super) async fn prepare_public_body(
        &self,
        item: &mut Planned,
        running: &mut Running,
    ) -> Result<(), EngineError> {
        self.inline_reference_links(item)?;
        if self.declared(&item.to) == SourceVisibility::Private {
            return Ok(());
        }
        if !content_of(&item.item).is_some_and(could_address)
            && !item
                .carried
                .iter()
                .flatten()
                .any(|comment| could_address(&comment.body))
        {
            return Ok(());
        }
        if running.private_locations.is_none() {
            running.private_locations = Some(self.private_locations().await?);
        }
        // Read links while their private source spellings still identify their referents.
        // The renderer screens every public result and uses opaque pending links until a
        // counterpart has landed.
        if !running.private_locations.as_ref().is_none_or(Vec::is_empty) {
            self.rewrite_references(
                std::slice::from_mut(item),
                &running.resolvable,
                running.carries_work,
                &mut running.references,
                running.private_locations.as_deref(),
            )
            .await?;
            return Ok(());
        }
        let source = self.readable(&item.source.source)?;
        let form = SectionForm::for_kind(source.kind());
        let own = located(location_of(&item.item));
        let stored = source
            .source()
            .stored_headings(item.item.level().record(), &item.source.native);
        if let Some(content) = content_of(&item.item) {
            let authored = form.map_or_else(
                || content.to_owned(),
                |form| form.read_same_record(content, own.as_deref(), stored.as_deref()),
            );
            let clean = without_private_links(
                &authored,
                running.private_locations.as_deref().unwrap_or_default(),
                (form == Some(SectionForm::LocalMd))
                    .then_some(own.as_deref())
                    .flatten(),
            );
            if clean != content
                && let Some(provenance) =
                    super::stored::restamped(metadata_ref(&item.item), &[content], &clean)
            {
                metadata_of(&mut item.item).insert(
                    crate::template::TemplateProvenance::KEY.to_owned(),
                    provenance.to_value(),
                );
            }
            set_content(&mut item.item, clean);
        }
        if let Some(comments) = &mut item.carried {
            for comment in comments {
                comment.body = without_private_links(
                    &comment.body,
                    running.private_locations.as_deref().unwrap_or_default(),
                    (form == Some(SectionForm::LocalMd))
                        .then_some(own.as_deref())
                        .flatten(),
                );
            }
        }
        Ok(())
    }

    /// Reported locations only, held for this copy invocation. A failed inventory refuses
    /// the copy before it can publish a body whose links have not been checked.
    pub(super) async fn private_locations(&self) -> Result<Vec<String>, EngineError> {
        let mut locations = Vec::new();
        for configured in self
            .sources
            .iter()
            .filter(|source| self.declared(source.name()) == SourceVisibility::Private)
        {
            let source = self.readable(configured.name())?;
            for level in [Level::Task, Level::Project, Level::Document] {
                if (level == Level::Document
                    && source.source().capabilities().documents == Support::Unsupported)
                    || (level == Level::Project
                        && source.source().capabilities().projects == Support::Unsupported)
                {
                    continue;
                }
                let mut page = PageRequest {
                    cursor: None,
                    limit: source.source().capabilities().max_page_size.clamp(1, 100),
                };
                let mut visited = BTreeSet::new();
                loop {
                    let (held, next) = match level {
                        Level::Task => {
                            let read = source
                                .source()
                                .query_tasks(&TaskQuery::default(), &page)
                                .await
                                .map_err(|error| refused(source, error))?;
                            fits(read.items.len(), page.limit)
                                .map_err(|error| refused(source, error))?;
                            (
                                read.items
                                    .into_iter()
                                    .flat_map(|item| {
                                        item.location.into_iter().chain(
                                            item.url.map(onetaskgraph_plugin_api::Location::Url),
                                        )
                                    })
                                    .collect::<Vec<_>>(),
                                read.next,
                            )
                        }
                        Level::Project => {
                            let read = source
                                .source()
                                .query_projects(&ProjectQuery::default(), &page)
                                .await
                                .map_err(|error| refused(source, error))?;
                            fits(read.items.len(), page.limit)
                                .map_err(|error| refused(source, error))?;
                            (
                                read.items
                                    .into_iter()
                                    .flat_map(|item| {
                                        item.location.into_iter().chain(
                                            item.url.map(onetaskgraph_plugin_api::Location::Url),
                                        )
                                    })
                                    .collect(),
                                read.next,
                            )
                        }
                        Level::Document => {
                            let read = source
                                .source()
                                .query_documents(&DocumentQuery::default(), &page)
                                .await
                                .map_err(|error| refused(source, error))?;
                            fits(read.items.len(), page.limit)
                                .map_err(|error| refused(source, error))?;
                            (
                                read.items
                                    .into_iter()
                                    .flat_map(|item| {
                                        item.location.into_iter().chain(
                                            item.url.map(onetaskgraph_plugin_api::Location::Url),
                                        )
                                    })
                                    .collect(),
                                read.next,
                            )
                        }
                    };
                    for location in held {
                        if let Some(location) = located(Some(&location)) {
                            locations.extend(SectionForm::for_kind(source.kind()).map_or_else(
                                || vec![location.clone()],
                                |form| form.spellings(&location),
                            ));
                        }
                    }
                    let Some(next) = next else { break };
                    if !visited.insert(next.0.clone()) {
                        return Err(refused(
                            source,
                            SourceError::Malformed {
                                message: "private location inventory repeated a cursor".to_owned(),
                            },
                        ));
                    }
                    page.cursor = Some(next);
                }
            }
        }
        locations.sort();
        locations.dedup();
        Ok(locations)
    }
}

/// Reported record locations are URLs or absolute paths. A slash inside ordinary prose
/// (including a repository name) is not evidence that an unrelated private store must be read;
/// a word naming a Markdown file is, because a folder of Markdown's records are those files and
/// a body may name one by a relative path alone.
pub(super) fn could_address(content: &str) -> bool {
    content.contains("](")
        || content.contains("]:")
        || content.contains("://")
        || super::sections::bare_paths(content)
            .any(|(_, word)| word.split('#').next().unwrap_or_default().ends_with(".md"))
        || content.char_indices().any(|(at, character)| {
            (character == '/' || character == '\\')
                && super::stops_a_location(content[..at].chars().next_back())
                || character == ':' && content[at + 1..].starts_with(['/', '\\'])
        })
}

/// Keep link text, without the Markdown destination, for any private record address.
/// Literal addresses are removed too; fragments must never leave a private id behind.
pub(super) fn without_private_links(
    content: &str,
    locations: &[String],
    own: Option<&str>,
) -> String {
    without_private_links_keeping(content, locations, own, &[])
}

/// Resolved destination links may have the same relative spelling as private source links.
/// Only the renderer can authorize them, after verifying their destination counterparts.
pub(super) fn without_private_links_keeping(
    content: &str,
    locations: &[String],
    own: Option<&str>,
    destination_links: &[(std::ops::Range<usize>, String)],
) -> String {
    // Markdown unescapes backslashes before punctuation, including the `\.` in a
    // Windows temporary directory. Compare destinations in the parser's own spelling too.
    let parsed_locations = locations
        .iter()
        .filter_map(|location| {
            let link = format!("[record](<{location}>)");
            pulldown_cmark::Parser::new(&link).find_map(|event| match event {
                pulldown_cmark::Event::Start(pulldown_cmark::Tag::Link { dest_url, .. }) => {
                    Some(dest_url.into_string())
                }
                _ => None,
            })
        })
        .collect::<Vec<_>>();
    let known = locations
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let private = |target: &str, range: &std::ops::Range<usize>| {
        if destination_links.iter().any(|(written, link)| {
            link == target && range.start <= written.start && written.end <= range.end
        }) {
            return false;
        }
        let path = target.split('#').next().unwrap_or_default();
        let resolved = own.and_then(|own| crate::section_links::resolve_relative(own, path));
        locations.iter().chain(&parsed_locations).any(|location| {
            target == location
                || resolved.as_ref() == Some(location)
                || target
                    .strip_prefix(location)
                    .is_some_and(|rest| rest.starts_with('#'))
        })
    };
    let bare_private = |path: &str| {
        let path = path.split('#').next().unwrap_or_default();
        own.and_then(|own| crate::section_links::resolve_relative(own, path))
            .is_some_and(|resolved| known.contains(resolved.as_str()))
    };
    let parser = pulldown_cmark::Parser::new(content);
    let mut replacements = parser
        .reference_definitions()
        .iter()
        .filter(|(_, definition)| private(&definition.dest, &definition.span))
        .map(|(_, definition)| (definition.span.clone(), String::new()))
        .collect::<Vec<_>>();
    for (event, range) in parser.into_offset_iter() {
        if let pulldown_cmark::Event::Start(pulldown_cmark::Tag::Link { dest_url, .. }) = event
            && private(&dest_url, &range)
        {
            let link = &content[range.clone()];
            replacements.push((range, link_label(link).to_owned()));
        }
    }
    // A path alone, as a table cell names a record. A word inside a link or definition just
    // replaced goes with it, and one inside a verified link is the counterpart's own spelling.
    for (at, path) in super::sections::bare_paths(content) {
        let range = at..at + path.len();
        let overlaps =
            |held: &std::ops::Range<usize>| range.start < held.end && held.start < range.end;
        if bare_private(path)
            && !replacements.iter().any(|(held, _)| overlaps(held))
            && !destination_links
                .iter()
                .any(|(written, _)| overlaps(written))
        {
            replacements.push((range, String::new()));
        }
    }
    replacements.sort_by_key(|(range, _)| range.start);
    let mut out = content.to_owned();
    for (range, text) in replacements.into_iter().rev() {
        out.replace_range(range, &text);
    }
    for location in locations {
        while let Some(at) = out
            .match_indices(location)
            .map(|(at, _)| at)
            .find(|at| super::delimited_at(&out, *at, location))
        {
            let end = out[at + location.len()..]
                .find(|c: char| stops_a_location(Some(c)))
                .map_or(out.len(), |end| at + location.len() + end);
            out.replace_range(at..end, "");
        }
    }
    out
}

/// The authored label, including inline formatting and escaped or nested brackets. An
/// autolink's label is its private address itself, so none of that address is retained.
fn link_label(link: &str) -> &str {
    let Some(rest) = link.strip_prefix('[') else {
        return "";
    };
    let mut nested = 0;
    let mut escaped = false;
    for (at, character) in rest.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '[' => nested += 1,
            ']' if nested == 0 => return &rest[..at],
            ']' => nested -= 1,
            _ => {}
        }
    }
    ""
}
