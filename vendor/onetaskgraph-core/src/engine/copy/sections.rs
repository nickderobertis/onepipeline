//! A copied body's references and section links, written in the destination's form.
//!
//! [`BodyLinks`] is what a copy learns about one body before it writes it: the body as its
//! source holds it, the records of its project it names, and each one's headings. It is
//! rendered by [`BodyLinks::render`] against what the copy knows at that moment — each
//! referent's counterpart at the destination, where the body's own record lands, and the
//! heading ids a destination stored — and rendered again whenever the copy knows more,
//! because a counterpart created in this very copy, or a Linear record's heading ids, are
//! only known once it has been written. A link the copy cannot address yet is written as a
//! **placeholder**, the link exactly as the source spelled it, which
//! [`BodyLinks::finished`] replaces once nothing more will be learned.
//!
//! How a link is spelled on each kind is not decided here: every spelling, every parse back
//! and every markup is [`SectionForm`]'s.

use onetaskgraph_plugin_api::{NativeId, StoredHeading};

use super::{Counted, FRAGMENT, Level, stops_a_location};
use crate::GlobalId;
use crate::section_links::{
    Heading, RecordHeadings, SectionForm, Written, relative_path, resolve_relative,
};

/// The section-link figures of one body's rendering, which the copy reports per record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct SectionCounts {
    /// Fragments naming no heading of the record they point at, carried verbatim.
    pub(super) unresolved: u64,
    /// Links the native form cannot address, using the record URL across records or the
    /// authored fragment within one record.
    pub(super) unaddressable: u64,
}

impl SectionCounts {
    pub(super) fn add(&mut self, other: Self) {
        self.unresolved += other.unresolved;
        self.unaddressable += other.unaddressable;
    }
}

/// Where one referent's counterpart is, as the copy knows it.
#[derive(Debug, Clone)]
pub(super) enum Counterpart {
    /// At the destination, under `id`, which reports `location` for it.
    At {
        /// Its id there.
        id: NativeId,
        /// The location string the destination reports for it.
        location: String,
    },
    /// Written by this copy, so known only once it has landed: asked of [`Knows`].
    InCopy,
    /// No counterpart: the reference is left as it is, and counted unresolved.
    None,
    /// More than one, or two referents spelled alike: left as it is, and counted ambiguous.
    Ambiguous,
}

/// One record of the body's project that the body names.
#[derive(Debug, Clone)]
pub(super) struct Linked {
    /// Which interface it is read through.
    pub(super) level: Level,
    /// Its qualified id at the source.
    pub(super) id: GlobalId,
    /// The location string its source reports for it.
    pub(super) location: String,
    /// Every spelling of that location in the source kind's own form.
    spellings: Vec<String>,
    /// Its headings at the source.
    headings: RecordHeadings,
    /// The ids its source stored those headings under, where it has any.
    stored: Option<Vec<StoredHeading>>,
    /// Its counterpart.
    pub(super) counterpart: Counterpart,
}

impl Linked {
    /// A record whose source reports `location`, holding `content` and, where the source
    /// stores heading ids, `stored`.
    pub(super) fn new(
        level: Level,
        id: GlobalId,
        location: String,
        from: Option<SectionForm>,
        content: Option<&str>,
        stored: Option<Vec<StoredHeading>>,
    ) -> Self {
        let spellings =
            from.map_or_else(|| vec![location.clone()], |form| form.spellings(&location));
        Self {
            level,
            id,
            location,
            spellings,
            headings: RecordHeadings::of(content.unwrap_or_default()),
            stored,
            counterpart: Counterpart::None,
        }
    }

    /// Every spelling of its location in the source kind's own form.
    pub(super) fn spellings(&self) -> &[String] {
        &self.spellings
    }
}

/// What a rendering asks of the copy it is part of.
pub(super) trait Knows {
    /// Where a referent written by this copy landed, once it has: its id and the location
    /// the destination reports for it.
    fn landed(&self, referent: &Linked) -> Option<(NativeId, String)>;
    /// The heading ids the destination stored the record `id`, read through `level`, under.
    fn stored(&self, level: Level, id: &NativeId) -> Option<Vec<StoredHeading>>;
    /// Where the body's own record is at the destination, once it is known.
    fn own(&self) -> Option<(NativeId, String)>;
}

/// Which rendering of a body this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Pass {
    /// One while the copy may still learn more: a link not addressable yet is left pending.
    Provisional,
    /// The rendering that lands once nothing more will be learned: nothing is left pending,
    /// using the record URL across records or the authored fragment within one record
    /// when the native heading still cannot be addressed.
    Final,
}

/// Where a link was spelled in a body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spelled {
    /// As a whole Markdown link destination, `](…)`.
    Destination,
    /// Anywhere else, as a literal location.
    Literal,
}

/// A link written as its source spelled it, because the destination's form cannot address it
/// yet: rendered again once the copy knows more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Pending {
    /// The placeholder sent to the destination; opaque when the body is public.
    pub(super) text: String,
    /// The source spelling, held only in memory while a public placeholder is written.
    original: String,
    /// Where it was spelled, which is the context it is read again in.
    spelled: Spelled,
}

impl Pending {
    /// The source spelling this link is rendered from once it is addressable.
    pub(super) fn source_spelling(&self) -> &str {
        &self.original
    }

    /// The same link with its source spelling as a sanitisation passed it.
    pub(super) fn reword(&mut self, reworded: impl FnOnce(&str) -> String) {
        self.original = reworded(&self.original);
    }
}

/// One rendering of a body.
#[derive(Debug, Clone, Default)]
pub(super) struct Rendered {
    /// The body as the destination is sent it.
    pub(super) content: String,
    /// The reference figures the invocation reports.
    pub(super) counts: Counted,
    /// The section-link figures the record reports.
    pub(super) sections: SectionCounts,
    /// The placeholders written.
    pub(super) pending: Vec<Pending>,
    /// Links resolved to verified destination counterparts during this rendering.
    destination_links: Vec<(std::ops::Range<usize>, String)>,
    /// Fragments naming no heading of their referent, carried byte for byte.
    carried_fragments: Vec<String>,
}

/// One body's references and section links, as the copy learned them before writing it.
#[derive(Debug, Clone)]
pub(super) struct BodyLinks {
    /// The body as its source holds it.
    pub(super) original: String,
    /// The source kind's form, by which its links are read; `None` reads every fragment as
    /// opaque.
    from: Option<SectionForm>,
    /// The destination kind's form, in which they are written; `None` carries every
    /// fragment byte for byte.
    to: Option<SectionForm>,
    /// Which interface the body's own record is written through.
    pub(super) level: Level,
    /// Where its source says the body's own record is.
    own_location: Option<String>,
    /// The body's own headings.
    own_headings: RecordHeadings,
    /// The ids its source stored the body's own headings under.
    own_stored: Option<Vec<StoredHeading>>,
    /// The records of its project it names, longest spelling first.
    pub(super) referents: Vec<Linked>,
    /// How a rendering treats private addresses.
    screening: Screening,
    /// Whether a folder of Markdown is sent every link to a record as a relative path, so that
    /// no spelling of a public view names where that folder sits on this machine.
    always_relative: bool,
}

/// What a body's text is read for when asking which records it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reading {
    /// Whole references alone.
    WholeReferences,
    /// A body bound for a destination declared private, or screened for a public one: a relative
    /// path written alone as well.
    BarePathsToo,
}

/// How a body's rendering treats private addresses.
#[derive(Debug, Clone, Default)]
enum Screening {
    /// No boundary reads this body: only a whole reference is read.
    #[default]
    None,
    /// Bound for a destination declared private: a relative path written alone, a reference-style
    /// definition's among them, is read as an inline link's is, and nothing is screened.
    Private,
    /// Bound for a public destination: a relative path written alone, a reference-style
    /// definition's among them, is read as a reference too, and these private addresses are
    /// screened from every rendering.
    Public(Vec<String>),
    /// Read as such a body is, and not screened in this rendering: a placeholder rendered again
    /// alone, screened once inserted back into it, or a body asked what fragments it carries.
    PublicPlaceholder,
}

impl Screening {
    /// The private addresses this rendering itself screens.
    fn private_locations(&self) -> Option<&[String]> {
        match self {
            Self::Public(locations) => Some(locations),
            Self::None | Self::Private | Self::PublicPlaceholder => None,
        }
    }

    /// Whether a relative path written alone is read as a reference.
    fn reads_bare_paths(&self) -> bool {
        !matches!(self, Self::None)
    }

    /// The screening of a placeholder of a body screened like this.
    fn of_placeholder(&self) -> Self {
        match self {
            Self::None => Self::None,
            Self::Private => Self::Private,
            Self::Public(_) | Self::PublicPlaceholder => Self::PublicPlaceholder,
        }
    }
}

/// One recognised occurrence: what it names, and where its text ends.
struct Occurrence<'a> {
    /// The referent, or `None` for the body's own record.
    referent: Option<&'a Linked>,
    /// Where the occurrence's text ends, its fragment included.
    end: usize,
    /// Its fragment, without the `#`.
    fragment: Option<&'a str>,
    /// Whether it was spelled as a path relative to the body's own file.
    relative: bool,
    /// Where it was spelled.
    spelled: Spelled,
}

impl BodyLinks {
    /// What the copy knows of one body before writing it.
    pub(super) fn new(
        original: String,
        from: Option<SectionForm>,
        to: Option<SectionForm>,
        level: Level,
        own_location: Option<String>,
        own_stored: Option<Vec<StoredHeading>>,
        mut referents: Vec<Linked>,
    ) -> Self {
        referents.sort_by_key(|referent| {
            std::cmp::Reverse(
                referent
                    .spellings
                    .iter()
                    .map(String::len)
                    .max()
                    .unwrap_or(0),
            )
        });
        Self {
            screening: Screening::None,
            always_relative: false,
            own_headings: RecordHeadings::of(&original),
            original,
            from,
            to,
            level,
            own_location,
            own_stored,
            referents,
        }
    }

    /// Screen public renderings after resolving counterparts, retaining private addresses
    /// only in the invocation's link plan.
    pub(super) fn screen_public(mut self, locations: Option<&[String]>) -> Self {
        self.screening = locations.map_or(Screening::None, |locations| {
            Screening::Public(locations.to_vec())
        });
        self
    }

    /// Read a relative path written alone as an inline link is read, for a destination declared
    /// private. A body already screened for a public destination reads one anyway.
    pub(super) fn for_private_destination(mut self) -> Self {
        if matches!(self.screening, Screening::None) {
            self.screening = Screening::Private;
        }
        self
    }

    /// Write every link to a record as a relative path in a folder of Markdown, as a public view
    /// holds them, however the source spelled it.
    pub(super) fn always_relative(mut self) -> Self {
        self.always_relative = true;
        self
    }

    /// The same links read in `content` — a comment of the record — whose fragments of the
    /// record's own headings are the record's, as `self`'s are.
    pub(super) fn for_comment(&self, content: String) -> Self {
        Self {
            original: content,
            ..self.clone()
        }
    }

    /// Whether `content` names a record whose source reports `location`, in any spelling of
    /// the source kind's form, as a whole reference — or, read from `own` in a folder of
    /// Markdown, as a Markdown link to a path relative to it. In a [`Reading::BarePathsToo`], also
    /// as such a path alone.
    pub(super) fn names(
        from: Option<SectionForm>,
        content: &str,
        own: Option<&str>,
        location: &str,
        reading: Reading,
    ) -> bool {
        let spellings = from.map_or_else(
            || vec![location.to_owned()],
            |form| form.spellings(location),
        );
        if spellings
            .iter()
            .any(|spelling| (0..content.len()).any(|at| delimited(content, at, spelling)))
        {
            return true;
        }
        from == Some(SectionForm::LocalMd)
            && own.is_some_and(|own| {
                let names_it = |target: &str| {
                    let path = target.split(FRAGMENT).next().unwrap_or_default();
                    resolve_relative(own, path).as_deref() == Some(location)
                };
                destinations(content).any(|(_, target)| names_it(target))
                    || (reading == Reading::BarePathsToo
                        && bare_paths(content).any(|(_, path)| names_it(path)))
            })
    }

    /// Every spelling of the body's own record's location in the source kind's own form.
    fn own_spellings(&self) -> Vec<String> {
        self.own_location.as_ref().map_or_else(Vec::new, |own| {
            self.from
                .map_or_else(|| vec![own.clone()], |form| form.spellings(own))
        })
    }

    /// Whether this body holds anything a rendering could change: a referent, or a link to
    /// its own record a destination form writes otherwise.
    pub(super) fn matters(&self) -> bool {
        !self.referents.is_empty()
            || (self.from.is_some()
                && self.to.is_some()
                && destinations(&self.original).any(|(_, target)| target.starts_with(FRAGMENT)))
            || self.own_location.as_deref().is_some_and(|own| {
                self.own_spellings().iter().any(|spelling| {
                    (0..self.original.len()).any(|at| delimited(&self.original, at, spelling))
                }) || (self.from == Some(SectionForm::LocalMd) && {
                    let names_itself = |target: &str| {
                        let path = target.split(FRAGMENT).next().unwrap_or_default();
                        resolve_relative(own, path).as_deref() == Some(own)
                    };
                    destinations(&self.original).any(|(_, target)| names_itself(target))
                        || (self.screening.reads_bare_paths()
                            && bare_paths(&self.original).any(|(_, path)| names_itself(path)))
                })
            })
    }

    /// Every fragment this body's links to its referents would carry byte for byte, because
    /// it names no heading: text no heading vouches for, which a boundary check has to read.
    pub(super) fn carried_fragments(&self) -> Vec<String> {
        /// Knows every referent as landed, so each occurrence reaches its fragment.
        struct Landed;
        impl Knows for Landed {
            fn landed(&self, _referent: &Linked) -> Option<(NativeId, String)> {
                Some((NativeId(String::new()), String::new()))
            }
            fn stored(&self, _level: Level, _id: &NativeId) -> Option<Vec<StoredHeading>> {
                None
            }
            fn own(&self) -> Option<(NativeId, String)> {
                None
            }
        }
        // Read as the public body it is, or a fragment on a path written alone would reach the
        // destination without this answer naming it.
        Self {
            screening: Screening::PublicPlaceholder,
            ..self.clone()
        }
        .render(&Landed, Pass::Final)
        .carried_fragments
    }

    /// The body rendered against what `knows` says now, on the terms `pass` names.
    pub(super) fn render(&self, knows: &dyn Knows, pass: Pass) -> Rendered {
        let content = self.original.as_str();
        let mut rendered = Rendered {
            content: String::with_capacity(content.len()),
            ..Rendered::default()
        };
        let mut at = 0;
        while at < content.len() {
            if content[at..].starts_with("](") {
                // A destination may be written in angle brackets, as Linear stores every one.
                let opening = if content[at..].starts_with("](<") {
                    "](<"
                } else {
                    "]("
                };
                rendered.content.push_str(opening);
                at += opening.len();
                if let Some(occurrence) = self.in_destination(content, at, opening.len() == 3) {
                    at = self.write(&occurrence, content, at, knows, pass, &mut rendered);
                }
                continue;
            }
            if let Some(occurrence) = self.spelled(content, at) {
                at = self.write(&occurrence, content, at, knows, pass, &mut rendered);
                continue;
            }
            if self.screening.reads_bare_paths()
                && let Some(occurrence) = self.bare(content, at)
            {
                at = self.write(&occurrence, content, at, knows, pass, &mut rendered);
                continue;
            }
            let character = content[at..]
                .chars()
                .next()
                .expect("a character at a boundary this walk only ever lands on");
            rendered.content.push(character);
            at += character.len_utf8();
        }
        if let Some(locations) = self.screening.private_locations() {
            rendered.content = super::privacy::without_private_links_keeping(
                &rendered.content,
                locations,
                (self.from == Some(SectionForm::LocalMd))
                    .then_some(self.own_location.as_deref())
                    .flatten(),
                &rendered.destination_links,
            );
        }
        rendered
    }

    /// `written` — a rendering whose placeholders were `pending`, as it was sent or as the
    /// destination stored it — with each placeholder rendered again against what `knows` says
    /// now, answering it and the placeholders still pending. Every other byte is left as it is.
    pub(super) fn finished(
        &self,
        written: &str,
        pending: &[Pending],
        knows: &dyn Knows,
        pass: Pass,
    ) -> (String, Vec<Pending>) {
        let mut replacements: Vec<(Pending, Option<String>)> = Vec::new();
        let mut withheld = Vec::new();
        for placeholder in pending {
            if replacements.iter().any(|(held, _)| held == placeholder) {
                continue;
            }
            let original = if placeholder.spelled == Spelled::Destination {
                format!("]({})", placeholder.original)
            } else {
                placeholder.original.clone()
            };
            let lone = Self {
                original,
                // This is a destination fragment, not a complete Markdown link. Screen
                // only after inserting it into the complete body below, preserving labels.
                screening: self.screening.of_placeholder(),
                ..self.clone()
            }
            .render(knows, pass);
            let mut text = if !lone.pending.is_empty() {
                None
            } else if placeholder.spelled == Spelled::Destination {
                lone.content
                    .strip_prefix("](")
                    .and_then(|rest| rest.strip_suffix(')'))
                    .map(str::to_owned)
            } else {
                Some(lone.content)
            };
            if pass == Pass::Final
                && text.is_some()
                && lone.destination_links.is_empty()
                && let Some(locations) = self.screening.private_locations()
            {
                let original = format!("[record](<{}>)", placeholder.original);
                let clean = super::privacy::without_private_links(
                    &original,
                    locations,
                    (self.from == Some(SectionForm::LocalMd))
                        .then_some(self.own_location.as_deref())
                        .flatten(),
                );
                if clean != original {
                    // Keep the existing opaque placeholder until the complete Markdown is
                    // screened below: removing its destination alone would leave an empty link.
                    withheld.push(placeholder.text.clone());
                    text = Some(placeholder.text.clone());
                }
            }
            replacements.push((placeholder.clone(), text));
        }
        replacements.sort_by_key(|(placeholder, _)| std::cmp::Reverse(placeholder.text.len()));
        let mut still = Vec::new();
        let mut out = String::with_capacity(written.len());
        let mut at = 0;
        while at < written.len() {
            // A placeholder the destination autolinked when it was first written, which names
            // it twice and only as a whole is a link to anything: replaced whole.
            if let Some((length, text)) = replacements.iter().find_map(|(placeholder, text)| {
                let length = autolinked(written, at, &placeholder.text)?;
                (placeholder.spelled == Spelled::Literal)
                    .then_some(text.as_ref())
                    .flatten()
                    .map(|text| (length, text))
            }) {
                out.push_str(text);
                at += length;
                continue;
            }
            let found = replacements.iter().find(|(placeholder, _)| {
                if placeholder.spelled == Spelled::Destination {
                    // As it was sent, or as a destination that writes every link destination in
                    // angle brackets stores it.
                    written[at..].starts_with(&placeholder.text) && {
                        let next = written[at + placeholder.text.len()..].chars().next();
                        (written[..at].ends_with("](")
                            && next.is_none_or(|c| c == ')' || c.is_whitespace()))
                            || (written[..at].ends_with("](<") && next == Some('>'))
                    }
                } else {
                    placeholder_at(written, at, &placeholder.text)
                }
            });
            if let Some((placeholder, text)) = found {
                match text {
                    Some(text) => out.push_str(text),
                    None => {
                        out.push_str(&placeholder.text);
                        if !still.contains(placeholder) {
                            still.push(placeholder.clone());
                        }
                    }
                }
                at += placeholder.text.len();
                continue;
            }
            let character = written[at..].chars().next().expect("a character boundary");
            out.push(character);
            at += character.len_utf8();
        }
        if let Some(locations) = self.screening.private_locations() {
            let mut locations = locations.to_vec();
            locations.extend(withheld);
            // Existing relative links were screened in their source context by `render`.
            // Only unresolved placeholders and absolute private addresses need removing now;
            // reading a resolved destination path against the source would misidentify it.
            out = super::privacy::without_private_links(&out, &locations, None);
        }
        (out, still)
    }

    /// The occurrence a Markdown link destination starting at `at` holds, when it is a
    /// fragment of the body's own record or a path relative to the body's own file. Every
    /// other destination is left to [`Self::spelled`].
    fn in_destination<'a>(
        &'a self,
        content: &'a str,
        at: usize,
        angled: bool,
    ) -> Option<Occurrence<'a>> {
        let end = content[at..]
            .find(|c: char| {
                if angled {
                    c == '>'
                } else {
                    c == ')' || c.is_whitespace()
                }
            })
            .map_or(content.len(), |found| at + found);
        let target = &content[at..end];
        if let Some(fragment) = target.strip_prefix(FRAGMENT) {
            return Some(Occurrence {
                referent: None,
                end,
                fragment: Some(fragment),
                relative: false,
                spelled: Spelled::Destination,
            });
        }
        self.relative(target, end, Spelled::Destination)
    }

    /// The occurrence `target`, ending at `end`, holds when it is a path relative to the body's
    /// own file naming a referent or that file itself.
    fn relative<'a>(
        &'a self,
        target: &'a str,
        end: usize,
        spelled: Spelled,
    ) -> Option<Occurrence<'a>> {
        if self.from != Some(SectionForm::LocalMd) {
            return None;
        }
        let (path, fragment) = match target.split_once(FRAGMENT) {
            Some((path, fragment)) => (path, Some(fragment)),
            None => (target, None),
        };
        let resolved = resolve_relative(self.own_location.as_deref()?, path)?;
        let referent = self
            .referents
            .iter()
            .find(|referent| referent.location == resolved);
        if referent.is_none() && self.own_location.as_deref() != Some(resolved.as_str()) {
            return None;
        }
        Some(Occurrence {
            referent,
            end,
            fragment,
            relative: true,
            spelled,
        })
    }

    /// The relative path spelled alone at `at`, as a table cell or a reference-style definition
    /// names a record, when one is.
    fn bare<'a>(&'a self, content: &'a str, at: usize) -> Option<Occurrence<'a>> {
        let path = bare_path_at(content, at)?;
        self.relative(path, at + path.len(), Spelled::Literal)
    }

    /// The whole reference spelled at `at`, with its fragment, when one is: a referent's, or
    /// the body's own record's.
    fn spelled<'a>(&'a self, content: &'a str, at: usize) -> Option<Occurrence<'a>> {
        let own = self.own_spellings();
        let (referent, spelling) = self
            .referents
            .iter()
            .flat_map(|referent| {
                referent
                    .spellings
                    .iter()
                    .map(move |spelling| (Some(referent), spelling.as_str()))
            })
            .chain(own.iter().map(|spelling| (None, spelling.as_str())))
            .find(|(_, spelling)| delimited(content, at, spelling))?;
        let after = at + spelling.len();
        let (end, fragment) = if content[after..].starts_with(FRAGMENT) {
            let start = after + FRAGMENT.len_utf8();
            let end = content[start..]
                .find(|c: char| stops_a_location(Some(c)))
                .map_or(content.len(), |found| start + found);
            (end, Some(&content[start..end]))
        } else {
            (after, None)
        };
        Some(Occurrence {
            referent,
            end,
            fragment,
            relative: false,
            spelled: Spelled::Literal,
        })
    }

    /// Write one occurrence starting at `at`, answering where the walk goes on from.
    fn write(
        &self,
        occurrence: &Occurrence<'_>,
        content: &str,
        at: usize,
        knows: &dyn Knows,
        pass: Pass,
        rendered: &mut Rendered,
    ) -> usize {
        let verbatim = &content[at..occurrence.end];
        let written = match occurrence.referent {
            None => self.own_link(occurrence, verbatim, knows, pass, rendered),
            Some(referent) => {
                self.cross_link(referent, occurrence, verbatim, knows, pass, rendered)
            }
        };
        match written {
            Some(written) => {
                let resolved = match occurrence.referent {
                    Some(referent) => match referent.counterpart {
                        Counterpart::At { .. } => true,
                        Counterpart::InCopy => knows.landed(referent).is_some(),
                        Counterpart::None | Counterpart::Ambiguous => false,
                    },
                    None => {
                        knows.own().is_some()
                            && (self.from.is_none()
                                || self.to.is_none()
                                || occurrence.fragment.is_none()
                                || self.own_heading(occurrence, verbatim).is_some())
                    }
                };
                if resolved {
                    let start = rendered.content.len();
                    rendered
                        .destination_links
                        .push((start..start + written.len(), written.clone()));
                }
                rendered.content.push_str(&written);
            }
            None => {
                let text = if self.screening.private_locations().is_some() {
                    // A provisional write must not name the private source. Retain that
                    // spelling only in memory, behind a token absent from the authored body.
                    let mut token = format!("onetaskgraph-pending-{}-end", rendered.pending.len());
                    while content.contains(&token) {
                        token.push('x');
                    }
                    token
                } else {
                    verbatim.to_owned()
                };
                rendered.content.push_str(&text);
                rendered.pending.push(Pending {
                    text,
                    original: verbatim.to_owned(),
                    spelled: occurrence.spelled,
                });
            }
        }
        occurrence.end
    }

    /// The source heading an own-record occurrence actually names. An unknown fragment
    /// is carried verbatim, so it cannot authorize a private relative source address.
    fn own_heading(&self, occurrence: &Occurrence<'_>, verbatim: &str) -> Option<Heading> {
        let from = self.from?;
        let fragment = occurrence.fragment?;
        if verbatim.starts_with(FRAGMENT) {
            from.same_record(fragment, &self.own_headings)
        } else {
            from.heading_of(fragment, &self.own_headings, self.own_stored.as_deref())
        }
    }

    /// A link to the body's own record: a fragment of it, or its own location. `None` is a
    /// link not addressable yet.
    fn own_link(
        &self,
        occurrence: &Occurrence<'_>,
        verbatim: &str,
        knows: &dyn Knows,
        pass: Pass,
        rendered: &mut Rendered,
    ) -> Option<String> {
        let bare = verbatim.starts_with(FRAGMENT);
        let (Some(_), Some(to), Some(_)) = (self.from, self.to, occurrence.fragment) else {
            // A plain link to itself, or a source or a destination with no form: its location
            // rewritten as any reference's is, its fragment carried.
            if bare {
                return Some(verbatim.to_owned());
            }
            return match knows.own() {
                Some((_, location)) => {
                    rendered.counts.rewritten += 1;
                    self.plain(&location, occurrence.relative, knows)
                        .map(|link| with_fragment(link, occurrence.fragment))
                        .or_else(|| (pass == Pass::Final).then(|| location.clone()))
                }
                None if pass == Pass::Final => Some(verbatim.to_owned()),
                None => None,
            };
        };
        let heading = self.own_heading(occurrence, verbatim);
        let Some(heading) = heading else {
            rendered.sections.unresolved += 1;
            return Some(verbatim.to_owned());
        };
        let own = knows.own();
        let stored = own
            .as_ref()
            .and_then(|(id, _)| knows.stored(self.level, id));
        let written = to.same(
            own.as_ref().map(|(_, location)| location.as_str()),
            &heading,
            &self.own_headings,
            stored.as_deref(),
        );
        let fallback = match SectionForm::LocalMd.same(None, &heading, &self.own_headings, None) {
            Written::Link(authored) => authored,
            _ => verbatim.to_owned(),
        };
        settle(written, fallback, pass, rendered)
    }

    /// A link to another record of the body's project. `None` is a link not addressable yet.
    fn cross_link(
        &self,
        referent: &Linked,
        occurrence: &Occurrence<'_>,
        verbatim: &str,
        knows: &dyn Knows,
        pass: Pass,
        rendered: &mut Rendered,
    ) -> Option<String> {
        let (id, location) = match &referent.counterpart {
            Counterpart::At { id, location } => (id.clone(), location.clone()),
            Counterpart::InCopy => match knows.landed(referent) {
                Some(landed) => landed,
                None if pass == Pass::Final => {
                    rendered.counts.unresolved += 1;
                    return Some(verbatim.to_owned());
                }
                None => return None,
            },
            Counterpart::None => {
                rendered.counts.unresolved += 1;
                return Some(verbatim.to_owned());
            }
            Counterpart::Ambiguous => {
                rendered.counts.unresolved += 1;
                rendered.counts.ambiguous += 1;
                return Some(verbatim.to_owned());
            }
        };
        rendered.counts.rewritten += 1;
        let Some(fragment) = occurrence.fragment else {
            return self
                .plain(&location, occurrence.relative, knows)
                .or_else(|| (pass == Pass::Final).then(|| location.clone()));
        };
        let heading = self.from.and_then(|from| {
            from.heading_of(fragment, &referent.headings, referent.stored.as_deref())
        });
        let (Some(to), Some(heading)) = (self.to, heading) else {
            if self.to.is_some() && self.from.is_some() {
                rendered.sections.unresolved += 1;
            }
            rendered.carried_fragments.push(fragment.to_owned());
            // A fragment naming no heading, or a destination with no form: the record's link,
            // its fragment carried byte for byte — into a folder of Markdown as the path relative
            // to the linking file a section link there is.
            // llmlint: ignore[changed_behavior_has_e2e] Held by `a_reference_carrying_a_fragment_is_rewritten_and_keeps_the_fragment` and `a_fragment_reference_is_left_and_counted_as_any_other_reference_without_a_counterpart` in `crates/onetaskgraph-core/tests/copy.rs`, which drive the real engine's copy into an in-memory destination that reports a location for every record and holds no section form, and by `a_reference_carrying_a_fragment_lands_on_the_copied_tasks_destination_location` and the section-link journeys in `crates/onetaskgraph-e2e` for every kind with one.
            let relative = occurrence.relative || self.to == Some(SectionForm::LocalMd);
            return self
                .plain(&location, relative, knows)
                .or_else(|| (pass == Pass::Final).then(|| location.clone()))
                .map(|link| format!("{link}#{fragment}"));
        };
        let own = knows.own().map(|(_, location)| location);
        let stored = knows.stored(referent.level, &id);
        let written = to.cross(
            &location,
            own.as_deref(),
            &heading,
            &referent.headings,
            stored.as_deref(),
        );
        settle(written, location, pass, rendered)
    }

    /// A plain reference to `location`: relative to the body's own file when it was spelled
    /// so and the destination is a folder of Markdown, and the location itself otherwise.
    /// `None` while the body's own file is not known yet.
    fn plain(&self, location: &str, relative: bool, knows: &dyn Knows) -> Option<String> {
        if (relative || self.always_relative) && self.to == Some(SectionForm::LocalMd) {
            return knows.own().map(|(_, own)| relative_path(&own, location));
        }
        Some(location.to_owned())
    }
}

/// What one form's answer is written as: `fallback` when the form still cannot address the
/// heading on the last rendering, and `None` — not addressable yet — before it.
fn settle(
    written: Written,
    fallback: String,
    pass: Pass,
    rendered: &mut Rendered,
) -> Option<String> {
    match written {
        Written::Link(link) => Some(link),
        Written::Unaddressable(link) => {
            rendered.sections.unaddressable += 1;
            Some(link)
        }
        Written::Pending if pass == Pass::Final => {
            rendered.sections.unaddressable += 1;
            Some(fallback)
        }
        Written::Pending => None,
    }
}

/// `link` with `#fragment` after it, when there is one.
fn with_fragment(link: String, fragment: Option<&str>) -> String {
    match fragment {
        Some(fragment) => format!("{link}#{fragment}"),
        None => link,
    }
}

/// Whether `location` occurs at `at` as a whole reference, as `delimited_at` reads one.
fn delimited(content: &str, at: usize, location: &str) -> bool {
    super::delimited_at(content, at, location)
}

/// The length of the autolink of `text` at `at` — `[text](<http://text>)` or its `https://`
/// spelling, as Linear stores a word it reads as an address — when one is there.
fn autolinked(written: &str, at: usize, text: &str) -> Option<usize> {
    ["http", "https"].iter().find_map(|scheme| {
        let link = format!("[{text}](<{scheme}://{text}>)");
        written[at..].starts_with(&link).then_some(link.len())
    })
}

/// Whether the placeholder `text` of a link spelled literally occurs at `at`: as a whole
/// reference, or with the sentence punctuation [`bare_path_at`] leaves out of a path alone.
fn placeholder_at(written: &str, at: usize, text: &str) -> bool {
    delimited(written, at, text)
        || (written[at..].starts_with(text) && bare_path_at(written, at) == Some(text))
}

/// Every Markdown link destination `content` holds, with where it starts — one written in
/// angle brackets without them, as [`BodyLinks::render`] reads it.
fn destinations(content: &str) -> impl Iterator<Item = (usize, &str)> {
    content.match_indices("](").map(move |(at, _)| {
        let angled = content[at + 2..].starts_with('<');
        let start = at + 2 + usize::from(angled);
        let end = content[start..]
            .find(|c: char| {
                if angled {
                    c == '>'
                } else {
                    c == ')' || c.is_whitespace()
                }
            })
            .map_or(content.len(), |found| start + found);
        (start, &content[start..end])
    })
}

/// The word starting at `at`, its fragment included and without the sentence punctuation
/// after it, when `at` starts one: what may be a relative path spelled alone.
pub(super) fn bare_path_at(content: &str, at: usize) -> Option<&str> {
    if !content.is_char_boundary(at)
        || !stops_a_location(content[..at].chars().next_back())
        || stops_a_location(content[at..].chars().next())
    {
        return None;
    }
    let word = content[at..]
        .split(|c: char| stops_a_location(Some(c)))
        .next()?;
    Some(word.trim_end_matches(['.', ':', '!', '?']))
}

/// Every word of `content`, with where it starts: see [`bare_path_at`].
pub(super) fn bare_paths(content: &str) -> impl Iterator<Item = (usize, &str)> {
    content
        .char_indices()
        .filter_map(|(at, _)| bare_path_at(content, at).map(|path| (at, path)))
}
