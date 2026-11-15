//! `sources sanitize`: find the items of one source already carrying a declared private term,
//! and fix what replacement can.
//!
//! The terms are the policy's ([`WritePolicy::declared_terms`]) and nothing here matches one:
//! a search per term narrows which items are read, and every item read is put to the policy's
//! own sanitisation before anything is rewritten. A term, and any query built from one, is held
//! in memory for the sweep alone — never written to a report, a log, a cache key or standard
//! error.
//!
//! **Coverage is reported, never assumed.** A sweep that searches cannot establish three
//! classes of record: a hit only in stored metadata, which no search of this contract reaches;
//! a hit only in a comment, which no search of this contract reaches either; and a spelling the
//! policy matches but a backend's tokenizer does not. A search sweep names each under
//! `coverage.uncovered`. `--complete` reads the source whole — every item, page by page, with
//! its comments — and so covers all three; a source whose requests spend no quota, a folder of
//! Markdown, is always read whole.
//!
//! [`WritePolicy::declared_terms`]: super::boundary::WritePolicy::declared_terms

use onetaskgraph_plugin_api::{
    Classification, NativeId, PageRequest, SourceError, SourceName, Task, TaskQuery, TextFields,
    TextQuery,
};
use schemars::JsonSchema;
use serde::Serialize;

use super::boundary::{
    ChangedField, Exposure, Locator, LocatorKind, Rewrite, SanitizeSurface, SanitizeVerdict,
    StoredTask, TermsRequest, WriteVerdict, every_comment, refusal, sanitised_task, screened_field,
    screened_locator, stored_field,
};
use super::{Engine, EngineError};
use crate::resolve::ResolvedSource;

/// A class of record a search cannot establish.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Uncovered {
    /// A hit only in stored metadata, which no search of the plugin contract reaches.
    Metadata,
    /// A hit only in a comment, which no search of the plugin contract reaches.
    Comments,
    /// A spelling the policy matches and a backend's index does not.
    NormalizedSpellings,
}

/// The classes of record a search-only sweep cannot establish.
const UNCOVERED: [Uncovered; 3] = [
    Uncovered::Metadata,
    Uncovered::Comments,
    Uncovered::NormalizedSpellings,
];

/// What kind of record a sweep entry is about: a task, the one kind a sweep reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SweepKind {
    /// A task.
    Task,
}

/// What a sweep did with one item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ItemOutcome {
    /// Every field it carried a term in was rewritten.
    Rewritten,
    /// A dry run: every field it carries a term in would be rewritten.
    WouldRewrite,
    /// Nothing in it needed rewriting.
    Unchanged,
    /// Something in it replacement cannot fix.
    Unfixable,
    /// It moved twice under the sweep, and nothing was overwritten.
    Conflict,
    /// A search returned it, and the policy finds no term in it.
    NotAMatch,
}

/// What a sweep did with one field: `{"outcome": …}`, and `why` beside an unfixable one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum FieldOutcome {
    /// It was rewritten.
    Rewritten,
    /// A dry run: it would be rewritten.
    WouldRewrite,
    /// Replacement cannot fix it.
    Unfixable {
        /// Why, in neutral words.
        why: String,
    },
}

/// What one `sources sanitize` did, naming every record by its screened locator and never by
/// what it said.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct SweepReport {
    /// The source swept, as screened.
    pub source: Locator,
    /// Whether nothing was written.
    pub dry_run: bool,
    /// Whether the source was read whole rather than searched.
    // llmlint: ignore[invalid_states_unrepresentable] The task's stated report contract carries this member beside `coverage.complete` — `{"source", "dry_run", "complete", "requests", "coverage": {"complete", "uncovered"}, "items"}` — and both SDKs are generated from it; `sanitize_tasks`, the one constructor, sets both from the one decision to read whole.
    pub complete: bool,
    /// What the sweep cost, from the source's own request accounting.
    pub requests: SweepRequests,
    /// Which classes of record the sweep could not establish.
    pub coverage: SweepCoverage,
    /// One entry per item a search returned, and per item a whole read found carrying a term.
    pub items: Vec<SweepItem>,
}

/// Source-counted requests and engine operations, not a partition of those requests.
/// Only `total` counts every request; one operation may send several, and guarded reads or
/// individual comment writes may be internal to an operation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct SweepRequests {
    /// Every request in the source counter delta, or `0` without a usable counter.
    pub total: u64,
    /// Search pages — or, for a sweep reading its source whole, listing pages — the sweep asked
    /// its source for: each one call of the plugin contract, which a source may answer with
    /// more than one request.
    pub page_operations: u64,
    /// Batched reads of items with their first comment page, each one call, and each walk of
    /// an item's further comment pages, counted once however many pages it took.
    pub detail_operations: u64,
    /// Items the sweep wrote back, each with every comment of it that changed.
    pub rewrite_operations: u64,
}

/// Which classes of record a sweep could not establish.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
// llmlint: ignore[invalid_states_unrepresentable] The report's JSON is the task's stated contract, `{"complete": bool, "uncovered": [...]}`, and both SDKs are generated from it; the one constructor, `sanitize_tasks`, sets `complete` exactly when `uncovered` is empty.
pub struct SweepCoverage {
    /// Whether it covered every class.
    pub complete: bool,
    /// Each class it did not cover.
    pub uncovered: Vec<Uncovered>,
}

/// What a sweep did with one item.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct SweepItem {
    /// The item's qualified id, as screened.
    pub id: Locator,
    /// What kind of record it is.
    pub kind: SweepKind,
    /// What the sweep did with it.
    pub outcome: ItemOutcome,
    /// What it did with each field it changed or could not.
    pub fields: Vec<SweepField>,
}

/// What a sweep did with one field of an item.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct SweepField {
    /// `content`, `title`, `label`, `repositories`, `metadata:<key>` or `comment:<id>`, its
    /// key or id as screened.
    pub field: Locator,
    /// What the sweep did with it.
    #[serde(flatten)]
    pub outcome: FieldOutcome,
}

impl SweepReport {
    /// The exit status the command line answers with: `1` when anything was left unfixable or
    /// in conflict, `3` when the sweep could not cover every class of record and nothing else
    /// failed, and `0` otherwise. An unavailable policy is a refusal, exiting `2`, before any
    /// report.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        // A source whose own name carries a term is a record nothing can name either.
        if matches!(self.source, Locator::Redacted { .. })
            || self
                .items
                .iter()
                .any(|item| matches!(item.outcome, ItemOutcome::Unfixable | ItemOutcome::Conflict))
        {
            1
        } else if !self.coverage.complete {
            3
        } else {
            0
        }
    }

    /// Number every redacted locator from 1, in the order the report emits them.
    fn renumber(&mut self) {
        let mut next = 1;
        let mut number = |locator: &mut Locator| {
            if let Locator::Redacted { ordinal, .. } = locator {
                *ordinal = next;
                next += 1;
            }
        };
        number(&mut self.source);
        for item in &mut self.items {
            number(&mut item.id);
            for field in &mut item.fields {
                number(&mut field.field);
            }
        }
    }
}

/// Why an item whose locator carries a term is unfixable.
const IDENTIFIER: &str = privateterms::LOCATOR_UNFIXABLE_REASON;

impl Engine {
    /// Sweep the tasks of `source`: find those already carrying a declared private term, and
    /// rewrite every field and comment of theirs replacement can fix, unless `dry_run`. Its
    /// projects and documents are not read, and `complete` is coverage of its tasks.
    ///
    /// # Errors
    ///
    /// [`EngineError::UnknownSource`] for a source nothing configures;
    /// [`EngineError::TermsUnlisted`] when the policy cannot list its terms — no
    /// `terms_command`; [`EngineError::BoundaryUnavailable`] when the policy could not decide,
    /// every locator redacted; and [`EngineError::SourceRefused`] when the source refuses a
    /// read or a write.
    pub async fn sanitize_tasks(
        &self,
        source: &SourceName,
        dry_run: bool,
        complete: bool,
    ) -> Result<SweepReport, EngineError> {
        let destination = self.built(source)?;
        match self
            .sweep_tasks(destination, source, dry_run, complete)
            .await
        {
            Err(EngineError::SourceRefused { error, .. }) => {
                // A failed read may precede the first item's screening. Screen its source
                // on this failure path only; no extra policy call is added to a passing sweep.
                let exposure = Exposure::default().locate(LocatorKind::Source, source.as_str());
                let answer = self.screen(&exposure).await;
                if matches!(answer.verdict, WriteVerdict::Unavailable { .. }) {
                    return Err(refusal(destination, "the source", &exposure, &answer));
                }
                Err(EngineError::SourceRefused {
                    name: screened_locator(
                        &exposure,
                        &answer.locators,
                        LocatorKind::Source,
                        source.as_str(),
                    )
                    .to_string(),
                    error,
                })
            }
            result => result,
        }
    }

    async fn sweep_tasks(
        &self,
        destination: &ResolvedSource,
        source: &SourceName,
        dry_run: bool,
        complete: bool,
    ) -> Result<SweepReport, EngineError> {
        let terms = self
            .boundary_policy()
            .declared_terms(&TermsRequest {
                scope: self.term_scope(),
                replacements: self.replacements().to_vec(),
            })
            .await;
        let terms = match terms {
            Ok(Some(terms)) => terms,
            failure => {
                let exposure = Exposure::default().locate(LocatorKind::Source, source.as_str());
                let answer = self.screen(&exposure).await;
                let name = screened_locator(
                    &exposure,
                    &answer.locators,
                    LocatorKind::Source,
                    source.as_str(),
                )
                .to_string();
                return Err(match (failure, answer.verdict) {
                    (_, WriteVerdict::Unavailable { reason }) => {
                        EngineError::TermsUndecided { name, why: reason }
                    }
                    (Err(error), _) => EngineError::TermsUndecided {
                        name,
                        why: error.message,
                    },
                    _ => EngineError::TermsUnlisted {
                        name,
                        why: "no write_policy.terms_command is configured".to_owned(),
                    },
                });
            }
        };
        // Read whole only a source that says it counts no requests: one whose count could not
        // be read may be spending a shared allowance, so it is searched as a counting one is.
        let metering = destination.source().metering().await;
        let whole = complete || matches!(metering, Ok(None));
        let before = metering.ok().flatten();
        let mut sweep = Sweep {
            requests: SweepRequests::default(),
            items: Vec::new(),
            source: None,
        };
        if whole {
            let mut cursor = None;
            loop {
                let listing = PageRequest {
                    cursor: cursor.take(),
                    limit: 100,
                };
                let first_comments = PageRequest {
                    cursor: None,
                    // Spend the listing's existing comment connection on up to one full
                    // page, avoiding follow-up reads for moderately active issues.
                    limit: 100,
                };
                let comments = destination
                    .source()
                    .capabilities()
                    .comments
                    .is_native()
                    .then_some(&first_comments);
                // The commented listing carries the first comment page without a second
                // detail read. Older plugins may refuse it and retain the ordinary path.
                let detailed = destination
                    .source()
                    .query_task_details(&TaskQuery::default(), &listing, comments)
                    .await;
                let page = match detailed {
                    Ok(page) => page,
                    Err(SourceError::Refused { .. }) if comments.is_some() => {
                        let page = destination
                            .source()
                            .query_tasks(&TaskQuery::default(), &listing)
                            .await
                            .map_err(|error| refused(destination, error))?;
                        sweep.requests.page_operations += 1;
                        let ids = page.items.into_iter().map(|task| task.id).collect();
                        self.sweep_batch(destination, ids, false, dry_run, &mut sweep)
                            .await?;
                        match page.next {
                            Some(next) => {
                                cursor = Some(next);
                                continue;
                            }
                            None => break,
                        }
                    }
                    Err(error) => return Err(refused(destination, error)),
                };
                sweep.requests.page_operations += 1;
                self.sweep_details(destination, page.items, false, dry_run, &mut sweep)
                    .await?;
                match page.next {
                    Some(next) => cursor = Some(next),
                    None => break,
                }
            }
        } else {
            let mut seen = std::collections::BTreeSet::new();
            for term in &terms {
                let mut cursor = None;
                loop {
                    let page = destination
                        .source()
                        .query_tasks(
                            &TaskQuery {
                                text: Some(TextQuery {
                                    terms: term.rule.term.clone(),
                                    fields: TextFields::TitleOrContent,
                                }),
                                ..TaskQuery::default()
                            },
                            &PageRequest {
                                cursor: cursor.take(),
                                limit: 100,
                            },
                        )
                        .await
                        .map_err(|error| refused(destination, error))?;
                    sweep.requests.page_operations += 1;
                    let ids = page
                        .items
                        .into_iter()
                        .map(|task| task.id)
                        .filter(|id| seen.insert(id.clone()))
                        .collect();
                    self.sweep_batch(destination, ids, true, dry_run, &mut sweep)
                        .await?;
                    match page.next {
                        Some(next) => cursor = Some(next),
                        None => break,
                    }
                }
            }
        }
        let shown_source = match sweep.source.take() {
            Some(shown) => shown,
            None => {
                // No item's call screened the source's name, so one call on the report's path.
                let exposure = Exposure::default().locate(LocatorKind::Source, source.as_str());
                let answer = self.screen(&exposure).await;
                if let WriteVerdict::Unavailable { .. } = answer.verdict {
                    return Err(refusal(destination, "", &exposure, &answer));
                }
                screened_locator(
                    &exposure,
                    &answer.locators,
                    LocatorKind::Source,
                    source.as_str(),
                )
            }
        };
        let after = destination.source().metering().await.ok().flatten();
        sweep.requests.total = match (before, after) {
            (Some(before), Some(after)) => after.requests.saturating_sub(before.requests),
            _ => 0,
        };
        let mut report = SweepReport {
            source: shown_source,
            dry_run,
            complete: whole,
            requests: sweep.requests,
            coverage: SweepCoverage {
                complete: whole,
                uncovered: if whole {
                    Vec::new()
                } else {
                    UNCOVERED.to_vec()
                },
            },
            items: sweep.items,
        };
        if report.items.iter().any(|item| {
            matches!(item.id, Locator::Redacted { .. })
                || item
                    .fields
                    .iter()
                    .any(|field| matches!(field.field, Locator::Redacted { .. }))
        }) || matches!(report.source, Locator::Redacted { .. })
        {
            report.renumber();
        }
        Ok(report)
    }

    /// Read `ids` with the first page of their comments in the source's own batched read, and
    /// decide each in turn — the rest of an item's comments read only where it has more.
    async fn sweep_batch(
        &self,
        destination: &ResolvedSource,
        ids: Vec<NativeId>,
        searched: bool,
        dry_run: bool,
        sweep: &mut Sweep,
    ) -> Result<(), EngineError> {
        if ids.is_empty() {
            return Ok(());
        }
        let commented = destination.source().capabilities().comments.is_native();
        let first = PageRequest {
            cursor: None,
            limit: 20,
        };
        // Like a copy, sanitization reads the forward edges next. The existing detail
        // method lets a hosted source carry them in the same request as the comments.
        let details = if commented {
            destination
                .source()
                .get_task_details_for_copy(&ids, &first)
                .await
        } else {
            destination.source().get_task_details(&ids, None).await
        };
        sweep.requests.detail_operations += 1;
        let problem = if details.len() != ids.len() {
            Some(format!(
                "source answered {} task details for the {} ids",
                details.len(),
                ids.len()
            ))
        } else if ids
            .iter()
            .zip(&details)
            .any(|(id, detail)| matches!(detail, Ok(Some(read)) if &read.task.id != id))
        {
            Some("source answered a task detail for a different id".to_owned())
        } else {
            None
        };
        if let Some(message) = problem {
            let exposure =
                Exposure::default().locate(LocatorKind::Source, destination.name().as_str());
            let answer = self.screen(&exposure).await;
            if matches!(answer.verdict, WriteVerdict::Unavailable { .. }) {
                return Err(refusal(destination, "the source", &exposure, &answer));
            }
            return Err(EngineError::SourceRefused {
                name: screened_locator(
                    &exposure,
                    &answer.locators,
                    LocatorKind::Source,
                    destination.name().as_str(),
                )
                .to_string(),
                error: SourceError::Malformed { message },
            });
        }
        let details = details
            .into_iter()
            .filter_map(|detail| match detail {
                Ok(Some(read)) => Some(Ok(read)),
                Ok(None) => None,
                Err(error) => Some(Err(refused(destination, error))),
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.sweep_details(destination, details, searched, dry_run, sweep)
            .await
    }

    /// Decide one listing page, retaining further comments for only the current item.
    async fn sweep_details(
        &self,
        destination: &ResolvedSource,
        details: Vec<onetaskgraph_plugin_api::TaskDetailRead>,
        searched: bool,
        dry_run: bool,
        sweep: &mut Sweep,
    ) -> Result<(), EngineError> {
        for read in details {
            let id = read.task.id.clone();
            let mut comments = Vec::new();
            match read.comments {
                Some(Err(error)) => return Err(refused(destination, error)),
                Some(Ok(Some(page))) => {
                    comments.extend(page.items);
                    if page.next.is_some() {
                        // Every page of one revision, read again from the start rather than
                        // resumed, so no comment is counted twice.
                        comments = every_comment(destination, &id).await?;
                        sweep.requests.detail_operations += 1;
                    }
                }
                Some(Ok(None)) | None => {}
            }
            self.sweep_item(
                destination,
                id,
                read.task,
                comments,
                searched,
                dry_run,
                sweep,
            )
            .await?;
        }
        Ok(())
    }

    /// Decide, and unless `dry_run` write, one item read with its comments: put it to the
    /// policy's sanitisation once, and rewrite what that answer rewrote.
    #[expect(
        clippy::too_many_arguments,
        reason = "one item as read, where it lives, and how the sweep was asked to treat it"
    )]
    async fn sweep_item(
        &self,
        destination: &ResolvedSource,
        id: NativeId,
        task: Task,
        comments: Vec<onetaskgraph_plugin_api::Comment>,
        searched: bool,
        dry_run: bool,
        sweep: &mut Sweep,
    ) -> Result<(), EngineError> {
        let item = format!("{}:{id}", destination.name());
        let stored = StoredTask::read(destination, &item, &task, &comments).await?;
        let answer: SanitizeVerdict = self.screen(&stored.exposure).await;
        if let WriteVerdict::Unavailable { .. } = answer.verdict {
            return Err(refusal(destination, &item, &stored.exposure, &answer));
        }
        let shown = |kind: LocatorKind, value: &str| {
            screened_locator(&stored.exposure, &answer.locators, kind, value)
        };
        if sweep.source.is_none() {
            sweep.source = Some(shown(LocatorKind::Source, destination.name().as_str()));
        }
        let shown_item = shown(LocatorKind::Item, &item);
        let rewrite = Rewrite::of(&stored.exposure, &answer);
        let mut fields = Vec::new();
        let mut unfixable = false;
        // A private repository among its own: the item is private by classification, which no
        // replacement changes.
        if self
            .classify(Classification::Public, &task.repositories)
            .await
            == Classification::Private
        {
            unfixable = true;
            fields.push(SweepField {
                field: Locator::Plain("repositories".to_owned()),
                outcome: FieldOutcome::Unfixable {
                    why: "names a private repository".to_owned(),
                },
            });
        }
        if let WriteVerdict::Refuse { .. } = answer.verdict {
            unfixable = true;
            if answer
                .unfixed
                .iter()
                .any(|unfixed| unfixed.surface == SanitizeSurface::Locators)
                || matches!(shown_item, Locator::Redacted { .. })
            {
                // Each identifier the policy names, by its redacted form — the item's own id as
                // `id`, since the entry already names it — and the item's id alone when the
                // policy named none.
                let mut named: Vec<SweepField> = answer
                    .unfixed
                    .iter()
                    .filter(|unfixed| unfixed.surface == SanitizeSurface::Locators)
                    .filter_map(|unfixed| usize::try_from(unfixed.index).ok())
                    .filter_map(|index| {
                        let locator = stored.exposure.locators().get(index)?;
                        let field = match locator.kind {
                            LocatorKind::Item | LocatorKind::Source => {
                                Locator::Plain("id".to_owned())
                            }
                            _ => answer.locators.get(index)?.clone(),
                        };
                        Some(SweepField {
                            field,
                            outcome: FieldOutcome::Unfixable {
                                why: IDENTIFIER.to_owned(),
                            },
                        })
                    })
                    .collect();
                named.dedup();
                if named.is_empty() {
                    named.push(SweepField {
                        field: Locator::Plain("id".to_owned()),
                        outcome: FieldOutcome::Unfixable {
                            why: IDENTIFIER.to_owned(),
                        },
                    });
                }
                sweep.items.push(SweepItem {
                    id: shown_item,
                    kind: SweepKind::Task,
                    outcome: ItemOutcome::Unfixable,
                    fields: named,
                });
                return Ok(());
            }
            // Name fixed and rewritable entries from the layout sent to the policy.
            let mut named: Vec<ChangedField> = answer
                .unfixed
                .iter()
                .filter_map(|unfixed| {
                    if unfixed.fixed {
                        return Some(if unfixed.surface == SanitizeSurface::Metadata {
                            Exposure::task_fixed_field(&task, usize::try_from(unfixed.index).ok()?)
                        } else {
                            ChangedField::Id
                        });
                    }
                    stored_field(
                        &task,
                        &comments,
                        unfixed.surface,
                        usize::try_from(unfixed.index).ok()?,
                    )
                })
                .collect();
            named.dedup();
            if named.is_empty() && fields.is_empty() {
                named.push(ChangedField::Content);
            }
            for field in named {
                let field = screened_field(&field, &stored.exposure, &answer.locators);
                if fields.iter().any(|existing| existing.field == field) {
                    continue;
                }
                fields.push(SweepField {
                    field,
                    outcome: FieldOutcome::Unfixable {
                        why: "carries a term no replacement is declared for".to_owned(),
                    },
                });
            }
        }
        let Some(sanitised) = sanitised_task(&task, &comments, &rewrite, &stored.depends_on) else {
            if unfixable {
                sweep.items.push(SweepItem {
                    id: shown_item,
                    kind: SweepKind::Task,
                    outcome: ItemOutcome::Unfixable,
                    fields,
                });
            } else if searched {
                // A hit the policy finds nothing in: one an earlier sanitisation already
                // rewrote — it carries a declared replacement — is unchanged, and any other is
                // a hit the policy does not confirm.
                let replaced = self.replacements().iter().any(|entry| match entry {
                    super::boundary::Replacement::Replace { with, .. } => {
                        !with.is_empty() && carries(&task, &comments, with)
                    }
                    super::boundary::Replacement::Reject { .. } => false,
                });
                sweep.items.push(SweepItem {
                    id: shown_item,
                    kind: SweepKind::Task,
                    outcome: if replaced {
                        ItemOutcome::Unchanged
                    } else {
                        ItemOutcome::NotAMatch
                    },
                    fields: Vec::new(),
                });
            }
            return Ok(());
        };
        let written = match self
            .sanitise_written(destination, &item, &id, &task, sanitised, dry_run)
            .await
        {
            Ok(sanitised) => sanitised,
            Err(EngineError::SanitizeConflict { .. }) => {
                sweep.items.push(SweepItem {
                    id: shown_item,
                    kind: SweepKind::Task,
                    outcome: ItemOutcome::Conflict,
                    fields,
                });
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        if written.was_written() {
            sweep.requests.rewrite_operations += 1;
        }
        let (word, whole) = if dry_run {
            (FieldOutcome::WouldRewrite, ItemOutcome::WouldRewrite)
        } else {
            (FieldOutcome::Rewritten, ItemOutcome::Rewritten)
        };
        let outcome = if !written.has_changes() {
            ItemOutcome::Unchanged
        } else if unfixable {
            ItemOutcome::Unfixable
        } else {
            whole
        };
        // A record read again is named by the screening of what was read.
        let (exposure, answered) = written.screening().map_or(
            (&stored.exposure, answer.locators.as_slice()),
            |(exposure, answered)| (exposure, answered.as_slice()),
        );
        for field in written.fields() {
            fields.push(SweepField {
                field: screened_field(field, exposure, answered),
                outcome: word.clone(),
            });
        }
        sweep.items.push(SweepItem {
            id: shown_item,
            kind: SweepKind::Task,
            outcome,
            fields,
        });
        Ok(())
    }
}

struct Sweep {
    requests: SweepRequests,
    items: Vec<SweepItem>,
    /// The source's name as the first item's call screened it.
    source: Option<Locator>,
}

fn refused(
    destination: &ResolvedSource,
    error: onetaskgraph_plugin_api::SourceError,
) -> EngineError {
    EngineError::SourceRefused {
        name: destination.name().to_string(),
        error,
    }
}

/// Whether `task` or one of `comments` carries `text` anywhere a sanitisation rewrites: its
/// title, content, label names, the string values of its metadata, and its comments' bodies.
fn carries(task: &Task, comments: &[onetaskgraph_plugin_api::Comment], text: &str) -> bool {
    fn in_value(value: &serde_json::Value, text: &str) -> bool {
        match value {
            serde_json::Value::String(value) => value.contains(text),
            serde_json::Value::Array(values) => values.iter().any(|value| in_value(value, text)),
            serde_json::Value::Object(fields) => fields.values().any(|value| in_value(value, text)),
            _ => false,
        }
    }
    task.title.contains(text)
        || task
            .content
            .as_deref()
            .is_some_and(|content| content.contains(text))
        || task.labels.iter().any(|label| label.name.contains(text))
        || task.metadata.values().any(|value| in_value(value, text))
        || comments.iter().any(|comment| comment.body.contains(text))
}

/// Each vocabulary's word, as the report spells it.
macro_rules! spelled {
    ($kind:ty) => {
        impl std::fmt::Display for $kind {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match serde_json::to_value(self) {
                    Ok(serde_json::Value::String(word)) => formatter.write_str(&word),
                    _ => Err(std::fmt::Error),
                }
            }
        }
    };
}

spelled!(Uncovered);
spelled!(ItemOutcome);

/// The sweep domain's published response roots.
pub(crate) fn schemas() -> [(&'static str, schemars::Schema); 1] {
    [("SweepReport", schemars::schema_for!(SweepReport))]
}
