//! What a landed retry superseded, told to `onevcs` when it lands.
//!
//! Each landed lineage's earlier attempts on other branches go to
//! [`onevcs::record_supersession`] and are journalled as `branches-superseded`;
//! nothing here changes a settlement. `onepipeline supersessions` backfills the
//! same record from a run's journal. Divergence entry 91 states the proposal.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::edits::StatedLanding;
use crate::event::{Envelope, PipelineKind, Source};
use crate::graph::Landing;
use crate::journal::{self, Journal};
use crate::ledger::RunPaths;
use crate::payload::{BranchesSuperseded, SupersededAttempt, UnrecordedAttempt};
use crate::projection::RunState;
use crate::Result;

// The labels every supersession carries, spelled as entry 91's
// `supersession_labels` states them; a unit test holds the two together.
const RUN_LABEL: &str = "run";
const NODE_LABEL: &str = "node";
const BY_LABEL: &str = "superseded_by_node";

/// One earlier attempt of a landed lineage.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) struct Attempt {
    /// The node id the retry edge names, which is the `node` label `onevcs` is
    /// given.
    pub(crate) node: String,
    /// Its settlement's branch, else its `session-opened`'s: what `onevcs` is told
    /// was superseded.
    pub(crate) branch: String,
}

/// One earlier attempt `onevcs` would not record, and what it answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Unrecorded {
    #[serde(flatten)]
    pub(crate) attempt: Attempt,
    /// What `onevcs` answered.
    pub(crate) error: String,
}

/// A lineage whose head landed, and the earlier attempts on other branches it
/// superseded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Lineage {
    /// The node that landed: the lineage's head.
    pub(crate) node: String,
    /// The branch it landed from.
    pub(crate) branch: String,
    /// The repository its work lands in, as the node names it.
    pub(crate) repo: String,
    /// Where it landed: the commit, or the change request's URL where no commit
    /// is recorded.
    pub(crate) landing: StatedLanding,
    /// Every earlier attempt, root first, whose branch is not the one that landed.
    /// An attempt on the landed branch is `onevcs`'s own to chain and is not here.
    pub(crate) superseded: Vec<Attempt>,
}

/// What one landed lineage's recording did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Recorded {
    /// The attempts recorded as superseded.
    pub(crate) recorded: Vec<Attempt>,
    /// The attempts `onevcs` refused, and why.
    pub(crate) failed: Vec<Unrecorded>,
}

/// Every landed lineage that has earlier attempts, in head order.
///
/// A lineage whose head's branch or landing the run never recorded is left out:
/// `onevcs` is asked for a supersession by the branch that superseded and where it
/// landed, and there is no answer to give it.
pub(crate) fn lineages(state: &RunState) -> Vec<Lineage> {
    let predecessor: BTreeMap<&str, &str> = state
        .superseded
        .iter()
        .map(|(retried, replacement)| (replacement.as_str(), retried.as_str()))
        .collect();
    let heads: BTreeSet<&String> = state
        .landings
        .keys()
        .chain(state.landing_commits.keys())
        .filter(|node| predecessor.contains_key(node.as_str()))
        .collect();
    heads
        .into_iter()
        .filter(|node| landed(state, node))
        .filter_map(|node| {
            let branch = branch_of(state, node)?;
            let landing = landing_of(state, node)?;
            let repo = state.graph.get(node).and_then(|n| n.repo.clone())?;
            let mut chain: Vec<&str> = Vec::new();
            let mut at = node.as_str();
            while let Some(earlier) = predecessor.get(at) {
                // A journal a person edited can hold a cycle; a lineage is walked
                // once whatever it holds.
                if chain.contains(earlier) || *earlier == node.as_str() {
                    break;
                }
                chain.push(earlier);
                at = earlier;
            }
            chain.reverse();
            let superseded = chain
                .into_iter()
                .filter_map(|attempt| {
                    let left = branch_of(state, attempt)?;
                    (left != branch).then(|| Attempt {
                        node: attempt.to_owned(),
                        branch: left,
                    })
                })
                .collect();
            Some(Lineage {
                node: node.clone(),
                branch,
                repo,
                landing,
                superseded,
            })
        })
        .collect()
}

/// Whether the run recorded a node's work reaching its base.
fn landed(state: &RunState, node: &str) -> bool {
    state.landings.get(node) == Some(&Landing::Landed) || state.landing_commits.contains_key(node)
}

/// The branch a node's attempt left: its settlement's, else its session's.
fn branch_of(state: &RunState, node: &str) -> Option<String> {
    state
        .branches
        .get(node)
        .filter(|branch| !branch.is_empty())
        .cloned()
        .or_else(|| {
            state
                .sessions
                .get(node)
                .or_else(|| state.abandoned.get(node))
                .map(|session| session.branch().as_str().to_owned())
        })
}

/// Where a node's work landed, as `onevcs` takes a landing: a commit, else a
/// change request's URL. A recorded value that is neither is no landing to tell it.
fn landing_of(state: &RunState, node: &str) -> Option<StatedLanding> {
    let stated = state.stated_landings.get(node);
    if let Some(commit @ StatedLanding::Commit(_)) = stated {
        return Some(commit.clone());
    }
    state
        .landing_commits
        .get(node)
        .and_then(|commit| StatedLanding::parse(commit))
        .or_else(|| stated.cloned())
        .or_else(|| {
            state
                .known_change_url(node)
                .and_then(|url| StatedLanding::parse(&url))
        })
}

/// One attempt a landed node's `branches-superseded` record says `onevcs`
/// recorded: that node, the landing it was recorded at, and the attempt.
///
/// Keyed by the node that landed and its landing, so a record counts only for the
/// lineage and the landing it was written for: an attempt a record names under
/// another head, or at a landing the lineage no longer states, is asked again.
pub(crate) type RecordedPair = (String, String, Attempt);

/// Every attempt a `branches-superseded` record says `onevcs` recorded, under the
/// landed node that record was written for.
///
/// A record that does not read as one is said so on stderr and counts for
/// nothing, so its pairs are asked of `onevcs` again — which records a
/// supersession once however often it is told.
// llmlint: ignore-block[changed_behavior_has_e2e] no entry point of this build writes an
// unreadable record — every `branches-superseded` is serialized by `journal` from the typed
// payload — so a journey reaches the arm only by editing the journal by hand, which is the
// state it stands for: a record a person edited or another build wrote.
// `tests::an_unreadable_record_counts_for_nothing_and_a_readable_one_for_its_head` drives
// this reader over both.
pub(crate) fn recorded_in(events: &[Envelope]) -> BTreeSet<RecordedPair> {
    events
        .iter()
        .filter(|event| {
            event.source == Source::Pipeline
                && PipelineKind::from_wire(&event.kind) == Some(PipelineKind::BranchesSuperseded)
        })
        .filter_map(|event| {
            serde_json::from_value::<BranchesSuperseded>(serde_json::Value::Object(
                event.payload.clone(),
            ))
            .map_err(|error| {
                eprintln!(
                    "onepipeline: a branches-superseded record in this run's journal could not \
                     be read ({error}); what it names is asked of onevcs again"
                );
            })
            .ok()
        })
        .flat_map(|record| {
            let (head, landing) = (record.node, record.landing);
            record
                .superseded
                .into_iter()
                // A pair naming no node or no branch is none this build asked
                // `onevcs` to record, so it stands for nothing recorded.
                .filter(|attempt| !attempt.node.is_empty() && !attempt.branch.is_empty())
                .map(move |attempt| {
                    (
                        head.clone(),
                        landing.clone(),
                        Attempt {
                            node: attempt.node,
                            branch: attempt.branch,
                        },
                    )
                })
        })
        .collect()
}
// llmlint: ignore-end[changed_behavior_has_e2e]

/// Record each of `attempts` as superseded by `lineage`'s landing, and say what
/// `onevcs` answered for each.
pub(crate) fn record(run: &str, lineage: &Lineage, attempts: &[Attempt]) -> Recorded {
    let mut recorded = Vec::new();
    let mut failed = Vec::new();
    for attempt in attempts {
        let supersession = onevcs::Supersession {
            repo: lineage.repo.clone(),
            branch: attempt.branch.clone(),
            superseded_by: lineage.branch.clone(),
            landing: lineage.landing.reference().to_owned(),
            labels: BTreeMap::from([
                (RUN_LABEL.to_owned(), run.to_owned()),
                (NODE_LABEL.to_owned(), attempt.node.clone()),
                (BY_LABEL.to_owned(), lineage.node.clone()),
            ]),
        };
        match onevcs::record_supersession(&supersession) {
            Ok(()) => recorded.push(attempt.clone()),
            Err(error) => failed.push(Unrecorded {
                attempt: attempt.clone(),
                error: crate::engine::bounded(&error.to_string()),
            }),
        }
    }
    Recorded { recorded, failed }
}

/// Journal what one lineage's recording did, as `branches-superseded`.
pub(crate) fn journal(
    paths: &RunPaths,
    journal: &mut Journal,
    lineage: &Lineage,
    recorded: &Recorded,
) -> Result<()> {
    let payload = BranchesSuperseded {
        node: lineage.node.clone(),
        landing: lineage.landing.reference().to_owned(),
        superseded: recorded
            .recorded
            .iter()
            .map(|attempt| SupersededAttempt {
                node: attempt.node.clone(),
                branch: attempt.branch.clone(),
            })
            .collect(),
        failed: recorded
            .failed
            .iter()
            .map(|unrecorded| UnrecordedAttempt {
                node: unrecorded.attempt.node.clone(),
                branch: unrecorded.attempt.branch.clone(),
                error: unrecorded.error.clone(),
            })
            .collect(),
    };
    let serde_json::Value::Object(payload) =
        serde_json::to_value(payload).expect("a payload struct serializes to an object")
    else {
        unreachable!("a payload struct serializes to an object")
    };
    journal.emit(
        PipelineKind::BranchesSuperseded,
        journal::labels(&paths.run, None),
        payload,
    )
}

/// The driver's half: each lineage recorded once, when its head lands.
///
/// Seeded from the journal, so a fresh driver — an `adopt`, a relaunch — neither
/// records a pair its predecessor already did nor journals it twice. A lineage is
/// answered once per driver whether or not `onevcs` accepted every attempt: a
/// refusal is on the record, and `onepipeline supersessions --record` is what asks
/// again.
#[derive(Debug, Default)]
pub(crate) struct Watch {
    recorded: BTreeSet<RecordedPair>,
    answered: BTreeSet<String>,
}

impl Watch {
    /// The pairs this run's journal already records.
    pub(crate) fn of_run(paths: &RunPaths) -> Self {
        Self {
            recorded: recorded_in(&journal::read(&paths.journal())),
            answered: BTreeSet::new(),
        }
    }

    /// Record every lineage that landed since this driver last looked.
    ///
    /// # Errors
    ///
    /// The reason the run's journal could not be written.
    pub(crate) fn pass(
        &mut self,
        paths: &RunPaths,
        journal: &mut Journal,
        state: &RunState,
    ) -> Result<()> {
        for lineage in lineages(state) {
            // llmlint: ignore[changed_behavior_has_e2e] once per lineage per driver by design,
            // so a landing that later gains a commit is not asked again: `onevcs` credits the
            // change-request record by the base commit naming that change, so a second record
            // at the commit changes no class, and `supersessions --record` keys by landing.
            if !self.answered.insert(lineage.node.clone()) {
                continue;
            }
            let pending = unrecorded(&lineage, &self.recorded);
            if pending.is_empty() {
                continue;
            }
            let recorded = record(&paths.run, &lineage, &pending);
            self::journal(paths, journal, &lineage, &recorded)?;
            self.recorded
                .extend(recorded.recorded.into_iter().map(|attempt| {
                    (
                        lineage.node.clone(),
                        lineage.landing.reference().to_owned(),
                        attempt,
                    )
                }));
        }
        Ok(())
    }
}

/// A lineage's earlier attempts that `recorded` does not hold under its head at
/// its landing.
fn unrecorded(lineage: &Lineage, recorded: &BTreeSet<RecordedPair>) -> Vec<Attempt> {
    let landing = lineage.landing.reference();
    lineage
        .superseded
        .iter()
        .filter(|attempt| {
            !recorded.contains(&(lineage.node.clone(), landing.to_owned(), (*attempt).clone()))
        })
        .cloned()
        .collect()
}

/// Record what a run's journal says landed, from a process that is not driving
/// it — the reply that settled a node with nothing driving the run.
///
/// # Errors
///
/// The reason the run's journal could not be written.
pub(crate) fn record_landed(paths: &RunPaths, journal: &mut Journal) -> Result<()> {
    let events = journal::read(&paths.journal());
    let state = crate::projection::fold(&events);
    Watch {
        recorded: recorded_in(&events),
        answered: BTreeSet::new(),
    }
    .pass(paths, journal, &state)
}

/// What `onepipeline supersessions` is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Mode {
    /// Record nothing.
    Answer,
    /// Record each pair the journal does not already.
    Record,
}

/// `onepipeline supersessions RUN [--record]`: which earlier attempts of the run's
/// landed retries were on a branch other than the one that landed, and what
/// became of recording them with `onevcs`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct Supersessions {
    /// The run id the verb was given, as it resolved.
    pub(crate) run: String,
    pub(crate) mode: Mode,
    /// Every landed lineage with earlier attempts, in head order.
    pub(crate) lineages: Vec<SupersededLineage>,
}

/// One landed lineage of [`Supersessions`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct SupersededLineage {
    /// The head that landed, and every earlier attempt on another branch.
    #[serde(flatten)]
    pub(crate) lineage: Lineage,
    /// The attempts the run's journal did not yet record: what this call
    /// records, or would.
    pub(crate) to_record: Vec<Attempt>,
    /// The attempts this call recorded.
    pub(crate) recorded: Vec<Attempt>,
    /// The attempts `onevcs` refused this call, and why.
    pub(crate) failed: Vec<Unrecorded>,
}

impl Supersessions {
    /// The status the binary exits with: [`EXIT_SUCCESS`](crate::error::EXIT_SUCCESS) when it answered or
    /// recorded, nothing to record included, and `1` when `onevcs` refused a
    /// record — the contract's code for work left unfinished.
    pub(crate) fn exit_code(&self) -> i32 {
        if self
            .lineages
            .iter()
            .any(|lineage| !lineage.failed.is_empty())
        {
            crate::error::EXIT_QUEUED
        } else {
            crate::error::EXIT_SUCCESS
        }
    }
}

/// `onepipeline supersessions RUN [--record]`.
///
/// Reads the run's journal and nothing else — no launch record, checkpoint or
/// result — so it answers for a settled run with no driver, whatever else of its
/// store survives.
/// With `record`, each pair the journal does not already record is handed to
/// `onevcs::record_supersession` — the function a driver calls when the retry
/// lands — and one `branches-superseded` per lineage that had any is appended to
/// the run's journal, so a second call records nothing new.
///
/// # Errors
///
/// A run with no directory under the runs root, one whose journal cannot be read
/// or holds no record this build reads, or one whose journal cannot be written.
pub(crate) fn supersessions(paths: &RunPaths, mode: Mode) -> Result<Supersessions> {
    if !paths.exists() {
        return Err(crate::Error::NoSuchRun {
            run: paths.run.clone(),
            root: paths
                .dir
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .to_path_buf(),
        });
    }
    let mut events = journal_of(paths)?;
    journal::merge_order(&mut events);
    let state = crate::projection::fold(&events);
    let already = recorded_in(&events);
    let mut journal = (mode == Mode::Record).then(|| Journal::open(paths));
    let mut lineages = Vec::new();
    for lineage in self::lineages(&state) {
        let to_record = unrecorded(&lineage, &already);
        let (recorded, failed) = match &mut journal {
            Some(journal) if !to_record.is_empty() => {
                let done = self::record(&paths.run, &lineage, &to_record);
                self::journal(paths, journal, &lineage, &done)?;
                (done.recorded, done.failed)
            }
            _ => (Vec::new(), Vec::new()),
        };
        lineages.push(SupersededLineage {
            lineage,
            to_record,
            recorded,
            failed,
        });
    }
    Ok(Supersessions {
        run: paths.run.clone(),
        mode,
        lineages,
    })
}

/// Every record of a run's journal, refusing a journal that cannot be opened or
/// holds none this build reads: every run's journal opens with its launch, so an
/// empty read is a store this verb cannot answer for rather than a run with
/// nothing to record.
fn journal_of(paths: &RunPaths) -> Result<Vec<Envelope>> {
    let path = paths.journal();
    std::fs::File::open(&path).map_err(|source| crate::Error::Ledger {
        path: path.clone(),
        source,
    })?;
    let events = journal::read(&path);
    if events.is_empty() {
        return Err(crate::Error::Invalid(format!(
            "{}: the journal holds no record this build can read, so there is no run to \
             answer for",
            path.display()
        )));
    }
    Ok(events)
}

/// The lines `onepipeline supersessions RUN` prints on stdout. What `onevcs`
/// answered for a record it refused is [`render_refusals`]'s, for stderr.
pub(crate) fn render(answered: &Supersessions) -> String {
    if answered.lineages.is_empty() {
        return format!(
            "run {}: no retry that landed has an earlier attempt, so there is nothing to \
             record\n",
            answered.run
        );
    }
    let mut out = String::new();
    for lineage in &answered.lineages {
        let head = &lineage.lineage;
        out.push_str(&format!(
            "{} landed {} at {}\n",
            head.node,
            head.branch,
            head.landing.reference()
        ));
        if head.superseded.is_empty() {
            out.push_str(
                "  every earlier attempt was on the branch that landed, which onevcs chains \
                 itself\n",
            );
        }
        for attempt in &head.superseded {
            let state = if lineage
                .failed
                .iter()
                .any(|failed| &failed.attempt == attempt)
            {
                "not recorded"
            } else if lineage.recorded.contains(attempt) {
                "recorded"
            } else if lineage.to_record.contains(attempt) {
                "to record: run again with --record"
            } else {
                "already recorded"
            };
            out.push_str(&format!(
                "  superseded {} on {}: {state}\n",
                attempt.node, attempt.branch
            ));
        }
    }
    out
}

/// What `onepipeline supersessions RUN --record` prints on stderr when `onevcs`
/// refused a record: each refused branch with its answer, and what to run once
/// that is fixed. Nothing when every record was taken.
pub(crate) fn render_refusals(answered: &Supersessions) -> String {
    let failed: Vec<&Unrecorded> = answered
        .lineages
        .iter()
        .flat_map(|lineage| &lineage.failed)
        .collect();
    if failed.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for refused in &failed {
        out.push_str(&format!(
            "onepipeline: onevcs refused to record {} ({}) as superseded: {}\n",
            refused.attempt.branch, refused.attempt.node, refused.error
        ));
    }
    out.push_str(&format!(
        "onepipeline: run `onepipeline supersessions {} --record` again once that is fixed\n",
        answered.run
    ));
    out
}

/// The one JSON object `onepipeline supersessions RUN --json` prints.
pub(crate) fn render_json(answered: &Supersessions) -> String {
    serde_json::to_string(answered).expect("the answer serializes")
}

/// Entry 91's block in `docs/contract-divergences.md`: the proposal this module
/// and `maintenance`'s retirement pass are held to.
#[cfg(test)]
pub(crate) fn proposal() -> serde_json::Value {
    let record = include_str!("../docs/contract-divergences.md");
    let entry = record
        .split("\n## ")
        .find(|entry| entry.starts_with("91."))
        .expect("the divergence record carries entry 91");
    let block = entry
        .split("```json")
        .nth(1)
        .and_then(|rest| rest.split("```").next())
        .expect("entry 91 carries its json block");
    serde_json::from_str(block).expect("entry 91's block is JSON")
}

/// The words a list of `proposal()` names.
#[cfg(test)]
pub(crate) fn proposed(key: &str) -> BTreeSet<String> {
    serde_json::from_value(proposal()[key].clone())
        .unwrap_or_else(|error| panic!("entry 91's {key} is a list of words: {error}"))
}

/// The top-level properties the payload document generated from `T` declares.
#[cfg(test)]
pub(crate) fn declared<T: schemars::JsonSchema>() -> BTreeSet<String> {
    schemars::schema_for!(T).as_value()["properties"]
        .as_object()
        .expect("a payload document declares its properties")
        .keys()
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> RunState {
        crate::projection::fold(&[])
    }

    fn retried(state: &mut RunState, from: &str, to: &str) {
        state.superseded.insert(from.to_owned(), to.to_owned());
    }

    fn lifecycle(state: &mut RunState, id: &str) {
        let node: crate::plan::Node = serde_json::from_value(serde_json::json!({
            "id": id, "repo": "service", "task": "## What\nShip."
        }))
        .expect("a node parses");
        state.graph.insert(node);
    }

    /// The walk is root first through every retry edge, drops the attempts on the
    /// landed branch, and takes a commit before a change request's URL.
    #[test]
    fn a_landed_lineage_names_every_earlier_attempt_on_another_branch_root_first() {
        let mut state = state();
        lifecycle(&mut state, "r3");
        retried(&mut state, "r1", "r2");
        retried(&mut state, "r2", "r3");
        for (node, branch) in [("r1", "a"), ("r2", "c"), ("r3", "c")] {
            state.branches.insert(node.to_owned(), branch.to_owned());
        }
        state.landings.insert("r3".to_owned(), Landing::Landed);
        state
            .change_urls
            .insert("r3".to_owned(), "https://github.com/o/s/pull/7".to_owned());
        let found = lineages(&state);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(
            found[0].landing.reference(),
            "https://github.com/o/s/pull/7"
        );
        assert_eq!(
            found[0].superseded,
            vec![Attempt {
                node: "r1".to_owned(),
                branch: "a".to_owned()
            }]
        );

        state
            .landing_commits
            .insert("r3".to_owned(), "f".repeat(40));
        assert_eq!(lineages(&state)[0].landing.reference(), "f".repeat(40));
    }

    /// A head that did not land, or whose branch nothing recorded, is no lineage
    /// `onevcs` can be told about; a node nothing retried is not a lineage at all.
    #[test]
    fn an_unlanded_head_an_unrecorded_branch_and_an_unretried_node_are_not_lineages() {
        let mut state = state();
        lifecycle(&mut state, "b");
        lifecycle(&mut state, "alone");
        retried(&mut state, "a", "b");
        state.branches.insert("a".to_owned(), "x".to_owned());
        state.landings.insert("b".to_owned(), Landing::Unlanded);
        state
            .landing_commits
            .insert("alone".to_owned(), "e".repeat(40));
        state.branches.insert("alone".to_owned(), "y".to_owned());
        assert!(lineages(&state).is_empty());

        state.landings.insert("b".to_owned(), Landing::Landed);
        state.landing_commits.insert("b".to_owned(), "e".repeat(40));
        assert!(lineages(&state).is_empty(), "b's own branch is unrecorded");
    }

    /// One `branches-superseded` envelope carrying `payload`.
    fn superseded_record(payload: serde_json::Value) -> Envelope {
        serde_json::from_value(serde_json::json!({
            "v": 2, "ts": "2026-09-26T00:00:00.000Z", "stream": "s", "seq": 0,
            "source": "pipeline", "kind": "branches-superseded",
            "labels": {"run_id": "r"}, "payload": payload, "artifacts": [],
        }))
        .expect("an envelope reads")
    }

    /// A record that reads names its pairs under its own head and landing; one
    /// that does not names nothing, and neither does a pair naming no node or no
    /// branch, so what they held is asked again.
    #[test]
    fn an_unreadable_record_counts_for_nothing_and_a_readable_one_for_its_head() {
        let events = [
            superseded_record(serde_json::json!({
                "node": "svc-2", "landing": "f".repeat(40),
                "superseded": [
                    {"node": "svc", "branch": "a"},
                    {"node": "", "branch": "c"},
                    {"node": "svc-1", "branch": ""},
                ],
                "failed": [],
            })),
            superseded_record(serde_json::json!({
                "node": "svc-3", "superseded": "not a list",
            })),
        ];
        assert_eq!(
            recorded_in(&events),
            BTreeSet::from([(
                "svc-2".to_owned(),
                "f".repeat(40),
                Attempt {
                    node: "svc".to_owned(),
                    branch: "a".to_owned(),
                }
            )])
        );
    }

    /// A pair recorded under one landed node, or at another landing, does not
    /// count for this lineage.
    #[test]
    fn a_pair_counts_as_recorded_only_under_the_head_and_landing_it_was_recorded_for() {
        let attempt = Attempt {
            node: "svc".to_owned(),
            branch: "a".to_owned(),
        };
        let lineage = Lineage {
            node: "svc-2".to_owned(),
            branch: "b".to_owned(),
            repo: "service".to_owned(),
            landing: StatedLanding::Commit("f".repeat(40)),
            superseded: vec![attempt.clone()],
        };
        let landing = "f".repeat(40);
        for elsewhere in [
            ("other".to_owned(), landing.clone(), attempt.clone()),
            (
                "svc-2".to_owned(),
                "https://github.com/o/s/pull/7".to_owned(),
                attempt.clone(),
            ),
        ] {
            assert_eq!(
                unrecorded(&lineage, &BTreeSet::from([elsewhere])),
                std::slice::from_ref(&attempt)
            );
        }
        let here = BTreeSet::from([("svc-2".to_owned(), landing, attempt)]);
        assert!(unrecorded(&lineage, &here).is_empty());
    }

    /// Entry 91 tells a planner the fields `branches-superseded` carries and the
    /// shape `--json` prints; these are the types that write both.
    #[test]
    fn entry_91_names_the_fields_this_build_writes() {
        assert_eq!(
            proposed("branches_superseded_fields"),
            declared::<BranchesSuperseded>()
        );
        let answered = Supersessions {
            run: "r".to_owned(),
            mode: Mode::Record,
            lineages: vec![SupersededLineage {
                lineage: Lineage {
                    node: "b".to_owned(),
                    branch: "y".to_owned(),
                    repo: "service".to_owned(),
                    landing: StatedLanding::Commit("f".repeat(40)),
                    superseded: Vec::new(),
                },
                to_record: Vec::new(),
                recorded: Vec::new(),
                failed: Vec::new(),
            }],
        };
        let printed = serde_json::to_value(&answered).expect("the answer serializes");
        let keys = |value: &serde_json::Value| -> BTreeSet<String> {
            value
                .as_object()
                .expect("an object")
                .keys()
                .cloned()
                .collect()
        };
        assert_eq!(proposed("verb_json_fields"), keys(&printed));
        assert_eq!(
            proposed("verb_json_lineage_fields"),
            keys(&printed["lineages"][0])
        );
        assert_eq!(printed["mode"], "record");
    }

    /// The label keys entry 91 tells a planner are the ones `onevcs` is given.
    #[test]
    fn entry_91_names_the_labels_each_supersession_carries() {
        assert_eq!(
            proposed("supersession_labels"),
            BTreeSet::from([RUN_LABEL, NODE_LABEL, BY_LABEL].map(String::from))
        );
    }
}
