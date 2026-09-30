//! The repository-identity launch interlock, delegated to `onevcs`.
//!
//! Reached by calling that library, like every other operation this crate
//! performs against it: [`onevcs::session_holders`] is the enumeration the
//! `session holders` verb prints, so the launcher reads the sibling's own typed
//! records rather than re-parsing the JSON a process printed for a person.
//!
//! What the enumeration answers is who holds an identity; whether that holder
//! is *concurrent* with the plan being launched is this module's question. A
//! live holder whose run and node every one of the plan's nodes on its identity
//! waits for, through a cross-DAG `run:<run>#<node>` edge, is not: the work that
//! would race it is scheduled after it. [`classify`] is that rule, and the
//! launcher refuses on what it leaves as a conflict.

use std::collections::{BTreeMap, BTreeSet};

use crate::error::{Error, Result};
use crate::executor::{SESSION_NODE_LABEL, SESSION_RUN_LABEL};
use crate::plan::{Node, Plan};

/// Whether a holder's owner is still there — the sibling's own verdict.
///
/// A pid alone cannot answer it, which is why the sibling reports it rather
/// than leaving a caller to derive one from [`Holder::owner_pid`]; re-deriving
/// it here would be a second liveness rule for the same question.
pub(crate) use onevcs::Liveness;

/// Where a holder's session is in its life. Named `State` here because that is
/// what this crate's interlock calls the distinction it makes.
pub(crate) use onevcs::Lifecycle as State;

/// One session holding a repository's workspace.
pub(crate) use onevcs::SessionHolder as Holder;

/// The one sentence that says which way forward fits, in the refusal's close.
/// `--acknowledge-concurrent`'s own `--help` says the same.
///
/// Held to both by `the_judgment_is_the_one_the_help_and_the_contract_state`.
pub const JUDGMENT: &str = "Depend on the holding node (a cross-DAG `run:<run>#<node>` dep) \
when this work needs that work first; acknowledge when this work outranks the concurrent run or \
no conflict between the two runs' changes is expected.";

// llmlint: ignore-block[invalid_states_unrepresentable] a repository spelling, an identity key,
// a run id, a node id and a `run:<run>#<node>` dependency are the plain strings the plan
// schema spells and `onevcs::SessionHolder` carries (its `identity` and `labels`), exactly as
// `src/crossdag.rs` records for the same identifiers; these are borrowed views of those
// values, compared for equality and rendered, and never parsed back into anything.
/// Every holder of the plan's repositories, and which identity each `repo` the
/// plan names resolved to.
#[derive(Debug, Default)]
pub struct Holdings {
    /// Each distinct holder, once.
    pub holders: Vec<Holder>,
    /// A plan `repo` spelling to the identities its holders were found on —
    /// the sibling's own resolution, read off the records it answered with, so
    /// a node and a holder are compared as one value. A repository nobody holds
    /// maps to nothing, which is all this interlock needs of it.
    pub identities: BTreeMap<String, BTreeSet<String>>,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// Ask `onevcs` about every distinct repository named by the plan.
pub fn holders(plan: &Plan) -> Result<Holdings> {
    let repos: BTreeSet<_> = plan
        .tasks
        .iter()
        .filter_map(|node| node.repo.as_deref())
        .collect();
    let mut by_identity_and_token = BTreeMap::new();
    let mut identities = BTreeMap::new();
    for repo in repos {
        let found = onevcs::session_holders(repo).map_err(|error| {
            sibling(format!(
                "cannot read the session holders of {repo}: {error}"
            ))
        })?;
        let on: &mut BTreeSet<String> = identities.entry(repo.to_string()).or_default();
        for holder in found {
            on.insert(holder.identity.clone());
            by_identity_and_token.insert((holder.identity.clone(), holder.token.clone()), holder);
        }
    }
    Ok(Holdings {
        holders: by_identity_and_token.into_values().collect(),
        identities,
    })
}

/// The run and node a holder's session was opened for, read off the labels
/// this engine stamps on every node session. `None` for a session missing
/// either — one opened by hand, or by an engine that predates the labels.
pub fn attribution(holder: &Holder) -> Option<(&str, &str)> {
    let run = holder.labels.get(SESSION_RUN_LABEL)?;
    let node = holder.labels.get(SESSION_NODE_LABEL)?;
    (!run.is_empty() && !node.is_empty()).then_some((run.as_str(), node.as_str()))
}

// llmlint: ignore-block[invalid_states_unrepresentable] the same identifiers as `Holdings`
// above, for the reason given there.
/// A live holder the plan's own dependencies acknowledge.
#[derive(Debug)]
pub struct Deferred<'a> {
    pub holder: &'a Holder,
    /// The run and node its session labels attribute it to.
    pub run: &'a str,
    pub node: &'a str,
    /// The reference every dependent reaches, `run:<run>#<node>`.
    pub dependency: String,
    /// The plan's nodes on the holder's identity, each of which reaches it.
    pub dependents: Vec<String>,
}

/// A live holder nothing but `--acknowledge-concurrent` passes.
#[derive(Debug)]
pub struct Conflict<'a> {
    pub holder: &'a Holder,
    /// Its run and node, `None` when its session carries neither label.
    pub attribution: Option<(&'a str, &'a str)>,
    /// The plan's nodes on the holder's identity that do not reach its
    /// `run:<run>#<node>` — every one of them for an unattributed holder.
    pub undeclared: Vec<String>,
}

/// The live holders, split by whether a dependency acknowledges each.
#[derive(Debug, Default)]
pub struct Classified<'a> {
    /// Those every plan node on their identity waits for.
    pub deferred: Vec<Deferred<'a>>,
    /// Everything else.
    pub conflicts: Vec<Conflict<'a>>,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

impl Classified<'_> {
    /// The dependency covering `holder`, if one does.
    pub fn dependency_of(&self, holder: &Holder) -> Option<&str> {
        self.deferred
            .iter()
            .find(|deferred| deferred.holder.token == holder.token)
            .map(|deferred| deferred.dependency.as_str())
    }
}

/// Split the live holders by the acknowledgement rule.
///
/// A holder on identity I labelled run R and node N is deferred to iff the plan
/// has a node on I and **every** node on I reaches the exact reference
/// `run:R#N` — as its own dep, or through the plan's own in-plan edges. A dep
/// on some other node of R is not counted, even one downstream of N: R is
/// live-editable, so that node settling need not imply N has. Neither is one
/// such edge while another node on I lacks it, since that other node still
/// races the holder.
pub fn classify<'a>(
    plan: &Plan,
    identities: &BTreeMap<String, BTreeSet<String>>,
    live: &[&'a Holder],
) -> Classified<'a> {
    let by_id: BTreeMap<&str, &Node> = plan
        .tasks
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let mut classified = Classified::default();
    for &holder in live {
        let on: Vec<&Node> = plan
            .tasks
            .iter()
            .filter(|node| {
                node.repo
                    .as_deref()
                    .and_then(|repo| identities.get(repo))
                    .is_some_and(|found| found.contains(&holder.identity))
            })
            .collect();
        let Some((run, node)) = attribution(holder) else {
            classified.conflicts.push(Conflict {
                holder,
                attribution: None,
                undeclared: on.iter().map(|node| node.id.clone()).collect(),
            });
            continue;
        };
        let dependency = format!("{}{run}#{node}", crate::crossdag::PREFIX);
        let undeclared: Vec<String> = on
            .iter()
            .filter(|candidate| !reaches(candidate, &dependency, &by_id, &mut BTreeSet::new()))
            .map(|candidate| candidate.id.clone())
            .collect();
        if on.is_empty() || !undeclared.is_empty() {
            classified.conflicts.push(Conflict {
                holder,
                attribution: Some((run, node)),
                undeclared,
            });
        } else {
            classified.deferred.push(Deferred {
                holder,
                run,
                node,
                dependency,
                dependents: on.iter().map(|node| node.id.clone()).collect(),
            });
        }
    }
    classified
}

/// Whether `node` waits for `dependency`, directly or through in-plan edges.
fn reaches<'a>(
    node: &'a Node,
    dependency: &str,
    by_id: &BTreeMap<&str, &'a Node>,
    seen: &mut BTreeSet<&'a str>,
) -> bool {
    if !seen.insert(node.id.as_str()) {
        return false;
    }
    node.deps.iter().any(|dep| {
        dep == dependency
            || by_id
                .get(dep.as_str())
                .is_some_and(|upstream| reaches(upstream, dependency, by_id, seen))
    })
}

/// The refusal for a launch whose conflicts were not acknowledged: every
/// conflicting holder, what would acknowledge it, and both ways forward.
pub fn refusal(run: &str, conflicts: &[Conflict<'_>]) -> String {
    let shared = conflicts
        .iter()
        .map(|conflict| {
            let holder = conflict.holder;
            let held = format!(
                "identity '{}' held by session '{}' (owner_pid {})",
                holder.identity, holder.token.0, holder.owner_pid
            );
            match conflict.attribution {
                Some((holding_run, holding_node)) => format!(
                    "{held} for run '{holding_run}' node '{holding_node}'; plan node(s) {} do \
                     not depend on it: declare `{}{holding_run}#{holding_node}` under \
                     `onepipeline.deps` on them",
                    quoted(&conflict.undeclared),
                    crate::crossdag::PREFIX,
                ),
                None => format!(
                    "{held}, which is not attributable to a run's node (its session carries no \
                     `{SESSION_RUN_LABEL}`/`{SESSION_NODE_LABEL}` labels), so only \
                     --acknowledge-concurrent passes it"
                ),
            }
        })
        .collect::<Vec<_>>()
        .join("; ");
    let forward = if conflicts
        .iter()
        .any(|conflict| conflict.attribution.is_some())
    {
        "declare each dependency named above, or pass --acknowledge-concurrent to proceed \
         deliberately"
    } else {
        "pass --acknowledge-concurrent to proceed deliberately"
    };
    format!("concurrent project work refused for run '{run}': {shared}. To launch, {forward}. {JUDGMENT}")
}

fn quoted(ids: &[String]) -> String {
    ids.iter()
        .map(|id| format!("'{id}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn sibling(message: String) -> Error {
    Error::Sibling {
        tool: "onevcs",
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holder(token: &str, identity: &str, labels: &[(&str, &str)]) -> Holder {
        Holder {
            token: onevcs::SessionToken(token.into()),
            identity: identity.into(),
            branch: "b".into(),
            worktree: "/w".into(),
            owner_pid: 7,
            state: State::Open,
            liveness: Liveness::Live,
            labels: labels
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        }
    }

    fn node(id: &str, repo: Option<&str>, deps: &[&str]) -> Node {
        Node {
            id: id.into(),
            repo: repo.map(Into::into),
            deps: deps.iter().map(|dep| (*dep).to_string()).collect(),
            ..Node::default()
        }
    }

    fn plan(nodes: Vec<Node>) -> Plan {
        Plan {
            schema_version: crate::plan::PLAN_SCHEMA_VERSION,
            goal: None,
            name: None,
            concurrency: 4,
            tasks: nodes,
        }
    }

    fn identities() -> BTreeMap<String, BTreeSet<String>> {
        BTreeMap::from([
            ("service".into(), BTreeSet::from(["id/service".into()])),
            ("other".into(), BTreeSet::from(["id/other".into()])),
        ])
    }

    #[test]
    fn a_holder_every_node_on_its_identity_reaches_is_deferred_and_others_conflict() {
        let held = holder("s1", "id/service", &[("run", "r"), ("node", "n")]);
        let unlabelled = holder("s2", "id/service", &[("run", "r")]);
        let blank = holder("s3", "id/service", &[("run", ""), ("node", "n")]);
        let live = [&held, &unlabelled, &blank];
        // Directly and through an in-plan edge from a node on no identity.
        let covered = plan(vec![
            node("a", Some("service"), &["run:r#n"]),
            node("prep", None, &["run:r#n"]),
            node("b", Some("service"), &["prep"]),
            node("c", Some("other"), &[]),
        ]);
        let classified = classify(&covered, &identities(), &live);
        assert_eq!(classified.deferred.len(), 1);
        assert_eq!(classified.deferred[0].dependency, "run:r#n");
        assert_eq!(classified.deferred[0].dependents, ["a", "b"]);
        assert_eq!(classified.dependency_of(&held), Some("run:r#n"));
        assert_eq!(classified.dependency_of(&unlabelled), None);
        assert_eq!(classified.conflicts.len(), 2);
        assert!(classified
            .conflicts
            .iter()
            .all(|conflict| conflict.attribution.is_none() && conflict.undeclared == ["a", "b"]));
    }

    #[test]
    fn a_partial_or_a_different_node_or_no_node_on_the_identity_is_a_conflict() {
        let held = holder("s1", "id/service", &[("run", "r"), ("node", "n")]);
        for (nodes, undeclared) in [
            (
                vec![
                    node("a", Some("service"), &["run:r#n"]),
                    node("b", Some("service"), &[]),
                ],
                vec!["b"],
            ),
            (
                vec![node("a", Some("service"), &["run:r#later"])],
                vec!["a"],
            ),
            (vec![node("a", Some("other"), &["run:r#n"])], vec![]),
        ] {
            let classified = classify(&plan(nodes), &identities(), &[&held]);
            assert!(classified.deferred.is_empty());
            assert_eq!(classified.conflicts[0].attribution, Some(("r", "n")));
            assert_eq!(classified.conflicts[0].undeclared, undeclared);
        }
    }

    #[test]
    fn an_in_plan_cycle_does_not_recurse_forever() {
        let held = holder("s1", "id/service", &[("run", "r"), ("node", "n")]);
        let cyclic = plan(vec![
            node("a", Some("service"), &["b"]),
            node("b", None, &["a"]),
        ]);
        let classified = classify(&cyclic, &identities(), &[&held]);
        assert_eq!(classified.conflicts[0].undeclared, ["a"]);
    }

    #[test]
    fn the_judgment_is_the_one_the_help_and_the_contract_state() {
        let flat = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
        let command = <crate::cli::Cli as clap::CommandFactory>::command();
        let start = command
            .find_subcommand("start")
            .expect("`start` is a subcommand");
        let flag = start
            .get_arguments()
            .find(|arg| arg.get_id() == "acknowledge_concurrent")
            .expect("`start` takes --acknowledge-concurrent");
        let help = flag
            .get_long_help()
            .or_else(|| flag.get_help())
            .expect("the flag documents itself")
            .to_string();
        // clap drops a doc comment's closing period.
        assert!(
            flat(&help).contains(JUDGMENT.trim_end_matches('.')),
            "{help}"
        );
        let contract = include_str!("../docs/contract.md");
        assert!(flat(contract).contains(JUDGMENT));
    }

    #[test]
    fn the_refusal_names_each_conflict_its_remedy_and_both_ways_forward() {
        let held = holder("s1", "id/service", &[("run", "r"), ("node", "n")]);
        let unlabelled = holder("s2", "id/service", &[]);
        let said = refusal(
            "second",
            &[
                Conflict {
                    holder: &held,
                    attribution: Some(("r", "n")),
                    undeclared: vec!["a".into(), "b".into()],
                },
                Conflict {
                    holder: &unlabelled,
                    attribution: None,
                    undeclared: vec!["a".into()],
                },
            ],
        );
        for part in [
            "concurrent project work refused for run 'second':",
            "identity 'id/service' held by session 's1' (owner_pid 7) for run 'r' node 'n'",
            "plan node(s) 'a', 'b' do not depend on it",
            "declare `run:r#n` under `onepipeline.deps`",
            "identity 'id/service' held by session 's2' (owner_pid 7), which is not attributable",
            "declare each dependency named above, or pass --acknowledge-concurrent",
            JUDGMENT,
        ] {
            assert!(said.contains(part), "{part:?} is missing from: {said}");
        }
        let alone = refusal(
            "second",
            &[Conflict {
                holder: &unlabelled,
                attribution: None,
                undeclared: vec![],
            }],
        );
        assert!(
            alone.contains(". To launch, pass --acknowledge-concurrent to proceed deliberately.")
        );
        assert!(!alone.contains("declare each dependency"));
    }
}
