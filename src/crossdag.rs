//! Cross-DAG edges: `run:<run_id>#<node_id>`.
//!
//! A dependency that names another run is resolved by **reading that run's
//! ledger**, not this graph. Every unknown resolves toward *blocked* rather than
//! failed — an unknown run, a node that has not settled, and a node that settled
//! badly are all upstreams that may still arrive, and failing the consumer would
//! throw away a graph that is merely early.
//!
//! Once an upstream does arrive, the consumer records **how far that run had
//! got** when it did. If the upstream moves past that point afterwards, the
//! consumer reports it and does not re-run: the work it did was correct when it
//! was done, and whether it should be done again is the planner's judgement, not
//! this crate's.
//!
//! Reading is unlocked, exactly as every other reader of a journal is. A round
//! that observes an upstream mid-write sees a prefix of it, which resolves
//! toward blocked and is re-read on the next pass.

// llmlint: ignore-file[invalid_states_unrepresentable] a dependency string, a run id, and
// a node id are the identifiers the plan schema spells and the journal payload carries;
// they are the same plain strings everywhere else in this crate, for the reason
// `src/plan.rs` records.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::error::Result;
use crate::graph::{Graph, NodeStatus};
use crate::journal::{self, Journal};
use crate::ledger::{self, RunPaths};

/// The prefix a cross-DAG reference starts with.
pub const PREFIX: &str = "run:";

/// The shape a reference must have, for the refusal to say so.
pub const SYNTAX: &str = "run:<run_id>#<node_id>";

/// One parsed `run:<run_id>#<node_id>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    /// The run whose ledger answers this edge.
    pub run: String,
    /// The node within it.
    pub node: String,
}

/// Parse a reference, or `None` if this dependency does not name another run.
///
/// Both halves must be non-empty: `run:#build` and `run:other#` name nothing
/// that could ever resolve, so they are malformed rather than merely pending.
pub fn parse(dependency: &str) -> Option<Reference> {
    let rest = dependency.strip_prefix(PREFIX)?;
    let (run, node) = rest.split_once('#')?;
    if run.is_empty() || node.is_empty() {
        return None;
    }
    Some(Reference {
        run: run.to_string(),
        node: node.to_string(),
    })
}

/// Whether a dependency is a well-formed cross-DAG reference.
pub fn is_reference(dependency: &str) -> bool {
    parse(dependency).is_some()
}

/// Whether a dependency was *meant* to be one and is malformed.
///
/// Anything starting `run:` names no node of this graph, so reporting it as a
/// missing dependency would send a planner looking for a node they never wrote.
pub fn is_malformed(dependency: &str) -> bool {
    dependency.starts_with(PREFIX) && parse(dependency).is_none()
}

/// How far a run's ledger has got.
///
/// The count of records in its merged store. The journal is append-only, so this
/// only ever rises, and it rises whenever the upstream does anything at all —
/// which is exactly the question a watch asks. It is deliberately not a per-
/// stream `seq`: a run is written by more than one process, so no single stream's
/// sequence describes the run.
pub fn extent(root: &Path, run: &str) -> Option<u64> {
    let paths = RunPaths::under(root, run);
    if !paths.exists() {
        return None;
    }
    Some(ledger::read_lines(&paths.journal()).len() as u64)
}

/// How a node of another run last settled, as that run's ledger records it.
///
/// The *last* settlement wins: a node that failed in one round and succeeded in
/// a later one is done, which is the whole point of a planner retrying it.
/// A record this build cannot read is skipped, the same way every other reader
/// of a journal skips one.
fn settled_status(root: &Path, reference: &Reference) -> Option<NodeStatus> {
    let paths = RunPaths::under(root, &reference.run);
    if !paths.exists() {
        return None;
    }
    ledger::read_lines(&paths.journal())
        .iter()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| event.get("kind").and_then(Value::as_str) == Some(journal::NODE_SETTLED))
        .filter(|event| {
            event
                .get("labels")
                .and_then(|l| l.get("node"))
                .and_then(Value::as_str)
                == Some(reference.node.as_str())
        })
        .filter_map(|event| {
            event
                .get("payload")
                .and_then(|p| p.get("status"))
                .and_then(Value::as_str)
                .and_then(NodeStatus::parse)
        })
        .next_back()
}

/// Every well-formed cross-DAG reference a graph names, with the nodes naming it.
pub fn edges(graph: &Graph) -> BTreeMap<String, Vec<String>> {
    let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for node in graph.iter() {
        for dep in &node.deps {
            if is_reference(dep) {
                edges.entry(dep.clone()).or_default().push(node.id.clone());
            }
        }
    }
    edges
}

/// Resolves this run's cross-DAG edges and remembers what it has already said.
///
/// The memory is the run's **own journal**, not this process: a round is one
/// process and a watch outlives many, so a baseline held only in memory would be
/// re-captured every round and a report would be re-sent every round.
#[derive(Debug)]
pub struct Observer {
    root: PathBuf,
    /// Where each resolved upstream had got when it was first resolved.
    baselines: BTreeMap<String, u64>,
    /// The `(dependency, consumer)` pairs already reported as moved.
    reported: BTreeSet<(String, String)>,
}

impl Observer {
    /// An observer seeded from what this run has already recorded.
    pub fn new(
        root: &Path,
        baselines: BTreeMap<String, u64>,
        reported: BTreeSet<(String, String)>,
    ) -> Self {
        Self {
            root: root.to_path_buf(),
            baselines,
            reported,
        }
    }

    /// The observer a run's own folded state describes.
    pub fn of_run(paths: &RunPaths, state: &crate::projection::RunState) -> Self {
        let root = paths
            .dir
            .parent()
            .map_or_else(ledger::runs_root, Path::to_path_buf);
        Self::new(
            &root,
            state.cross_dag_baselines.clone(),
            state.cross_dag_reported.clone(),
        )
    }

    /// Resolve every edge the graph names, recording what it learns.
    ///
    /// Returns the status each *dependency* resolved to, which is what the
    /// scheduler asks about a reference. Emits at most one `cross-dag-satisfied`
    /// per edge and one `upstream-modified` per edge and consumer, ever.
    pub fn resolve(
        &mut self,
        graph: &Graph,
        paths: &RunPaths,
        round: u64,
        journal: &mut Journal,
    ) -> Result<BTreeMap<String, NodeStatus>> {
        let mut resolved = BTreeMap::new();
        for (dependency, consumers) in edges(graph) {
            let Some(reference) = parse(&dependency) else {
                continue;
            };
            let status = settled_status(&self.root, &reference);
            // Only `done` satisfies. Everything else — an unknown run, a node
            // that has not settled, one that failed or was skipped — leaves the
            // consumer waiting on an upstream that may still arrive.
            if status != Some(NodeStatus::Done) {
                resolved.insert(dependency, NodeStatus::Blocked);
                continue;
            }
            resolved.insert(dependency.clone(), NodeStatus::Done);

            let Some(extent) = extent(&self.root, &reference.run) else {
                continue;
            };
            let baseline = match self.baselines.get(&dependency) {
                Some(baseline) => *baseline,
                None => {
                    self.baselines.insert(dependency.clone(), extent);
                    // Recorded against the first consumer, so the baseline has a
                    // node to belong to in a stream every view reads by node.
                    if let Some(first) = consumers.first() {
                        journal.emit(
                            journal::CROSS_DAG_SATISFIED,
                            journal::labels(&paths.run, Some(round), Some(first)),
                            journal::payload(&[
                                ("dependency", json!(dependency)),
                                ("last_seq", json!(extent)),
                            ]),
                        )?;
                    }
                    extent
                }
            };
            if extent <= baseline {
                continue;
            }
            for consumer in consumers {
                let pair = (dependency.clone(), consumer.clone());
                if !self.reported.insert(pair) {
                    continue;
                }
                // Reported, never acted on: the consumer's work was correct when
                // it was done, and whether it should be done again is the
                // planner's call.
                journal.emit(
                    journal::UPSTREAM_MODIFIED,
                    journal::labels(&paths.run, Some(round), Some(&consumer)),
                    journal::payload(&[
                        ("dependency", json!(dependency)),
                        ("captured_last_seq", json!(baseline)),
                        ("observed_last_seq", json!(extent)),
                    ]),
                )?;
            }
        }
        Ok(resolved)
    }
}

/// Resolve a graph's edges without recording anything.
///
/// For the readers that must not write: a view, and the transition's own check
/// for whether the next round has anything startable in it. Both need the same
/// answer the round would get; neither may append to the journal.
pub fn resolve_quietly(root: &Path, graph: &Graph) -> BTreeMap<String, NodeStatus> {
    edges(graph)
        .into_keys()
        .filter_map(|dependency| {
            let reference = parse(&dependency)?;
            let status = match settled_status(root, &reference) {
                Some(NodeStatus::Done) => NodeStatus::Done,
                _ => NodeStatus::Blocked,
            };
            Some((dependency, status))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reference_needs_both_halves() {
        assert_eq!(
            parse("run:other#build"),
            Some(Reference {
                run: "other".into(),
                node: "build".into()
            })
        );
        // A node id may itself address a step within its node.
        assert_eq!(
            parse("run:other#ship/verify").map(|r| r.node),
            Some("ship/verify".to_string())
        );
        for malformed in ["run:other", "run:#build", "run:other#", "run:", "run:#"] {
            assert_eq!(parse(malformed), None, "{malformed} parsed");
            assert!(is_malformed(malformed), "{malformed} is not reported wrong");
        }
        // Not a reference at all, so not this module's business either way.
        assert_eq!(parse("build"), None);
        assert!(!is_malformed("build"));
    }

    #[test]
    fn an_unknown_run_has_no_extent_and_no_status() {
        let root =
            std::env::temp_dir().join(format!("onepipeline-crossdag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a scratch root");
        assert_eq!(extent(&root, "nobody"), None);
        assert_eq!(
            settled_status(
                &root,
                &Reference {
                    run: "nobody".into(),
                    node: "build".into()
                }
            ),
            None
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
