//! Whether each release adoption a plan states **can ever complete**, as the linked
//! `onevcs` answers it.
//!
//! A lifecycle node waiting on a dependency in another repository waits on that
//! repository's release, and some of those waits never end: a `published` node behind
//! a producer that names no target to wait for is held for ever, and a `fast` node
//! whose publication opens no change request fails at its last step. Which ones is
//! **onevcs's** to say — every fact the rules read is that library's configuration —
//! so this module states none of them. It asks [`onevcs::adoption_check`] once per
//! lifecycle node and out-of-repository dependency, and carries the verdict's rule id
//! and reason verbatim: a refusal here is onevcs's sentence, with the node and the
//! dependency it is about put in front of it.
//!
//! **A question onevcs could not answer refuses nothing.** An `unanswered` verdict —
//! a repository this host has not registered, a declaration that does not read — is
//! a notice naming its reason, and the plan loads as it always did: refusing there
//! would refuse a plan that runs today.
//!
//! Three places ask. The loader (`plan check` and `start`) and every live edit that
//! adds or changes a node, its adoption, its `consumes` or an edge refuse on a
//! `never-completes`; `adopt`, which takes over a run already under way, refuses
//! nothing and raises each as a non-blocking surface instead.

use std::collections::BTreeMap;

use onevcs::{AdoptionQuery, AdoptionRule, AdoptionState};

use crate::plan::{Node, Plan};

/// The `source` a refusal made here carries, beside the loader's `engine` and a
/// registered check's path: the rule and its reason are onevcs's.
pub(crate) const SOURCE: &str = "onevcs";

/// One rule onevcs says stops one node's adoption of one dependency completing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refused {
    /// The consuming node.
    pub(crate) node: String,
    /// The dependency it waits on the release of.
    pub(crate) dependency: String,
    /// onevcs's rule.
    pub(crate) rule: AdoptionRule,
    /// onevcs's reason, verbatim.
    pub(crate) reason: String,
}

impl Refused {
    /// The plan field a planner changes to answer it: the rung for the two rules about
    /// the rung, and `consumes` for the two about a named target.
    pub(crate) fn field(&self) -> &'static str {
        match self.rule {
            AdoptionRule::PublishedNoTarget | AdoptionRule::FastNoChangeRequest => "adoption",
            AdoptionRule::ConsumesWithoutChangeRequest | AdoptionRule::ConsumesUndeclaredTarget => {
                "consumes"
            }
        }
    }

    /// The dependency, the rule and onevcs's reason — what `plan check` reports as a
    /// refusal's `reason`, its node and field standing beside it.
    pub(crate) fn detail(&self) -> String {
        format!(
            "dependency '{}': {}: {}",
            self.dependency, self.rule, self.reason
        )
    }

    /// The whole line, as `plan check` prints it and `start` and a refused edit say it.
    pub(crate) fn line(&self) -> String {
        format!(
            "{SOURCE}: node '{}': `{}`: {}",
            self.node,
            self.field(),
            self.detail()
        )
    }
}

/// One adoption onevcs could not answer about, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Unanswered {
    pub(crate) node: String,
    pub(crate) dependency: String,
    pub(crate) reason: String,
}

impl Unanswered {
    /// The notice it is reported as, on stderr.
    pub(crate) fn notice(&self) -> String {
        format!(
            "onepipeline: node '{}': dependency '{}': {SOURCE} could not say whether this \
             release adoption can complete, so it was not checked: {}",
            self.node, self.dependency, self.reason
        )
    }
}

/// Everything one ask found.
#[derive(Debug, Default)]
pub(crate) struct Asked {
    pub(crate) refused: Vec<Refused>,
    pub(crate) unanswered: Vec<Unanswered>,
}

impl Asked {
    /// Say every unanswered adoption on stderr.
    pub(crate) fn report_unanswered(&self) {
        for unanswered in &self.unanswered {
            eprintln!("{}", unanswered.notice());
        }
    }

    /// Every refusal, one line each, or `None` where there is none.
    pub(crate) fn refusal(&self) -> Option<String> {
        if self.refused.is_empty() {
            return None;
        }
        Some(
            self.refused
                .iter()
                .map(Refused::line)
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }
}

/// Ask onevcs about every lifecycle node of `plan` that `asked` selects, and each of
/// its dependencies that lands in another repository.
///
/// A dependency that is not one of this plan's nodes — a cross-DAG reference, whose
/// repository is another run's to say — lands in no repository, lands in the node's
/// own, or reaches it over an export edge is not a release wait, and is not asked about.
pub(crate) fn ask(plan: &Plan, asked: impl Fn(&Node) -> bool) -> Asked {
    let by_id: BTreeMap<&str, &Node> = plan
        .tasks
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    // Keyed by the whole query, so nodes sharing a repository pair and a statement ask
    // onevcs once between them.
    let mut answers: BTreeMap<String, Result<onevcs::AdoptionVerdict, String>> = BTreeMap::new();
    let mut found = Asked::default();
    for node in plan.tasks.iter().filter(|node| asked(node)) {
        let Some(consumer) = node.repo.as_deref() else {
            continue;
        };
        for dep in &node.deps {
            if plan
                .export_edges
                .iter()
                .any(|edge| edge.from == *dep && edge.to == node.id)
            {
                continue;
            }
            let Some(producer) = by_id.get(dep.as_str()).and_then(|up| up.repo.as_deref()) else {
                continue;
            };
            if producer == consumer {
                continue;
            }
            let query = AdoptionQuery {
                consumer: consumer.to_owned(),
                producer: producer.to_owned(),
                adoption: node.adoption,
                consumes: node.consumes.get(dep).cloned(),
                consumer_policy: node.merge_policy,
            };
            let verdict = answers
                .entry(format!("{query:?}"))
                .or_insert_with(|| onevcs::adoption_check(&query).map_err(|e| e.to_string()));
            let verdict = match verdict {
                Ok(verdict) => verdict,
                Err(failure) => {
                    found.unanswered.push(Unanswered {
                        node: node.id.clone(),
                        dependency: dep.clone(),
                        reason: failure.clone(),
                    });
                    continue;
                }
            };
            // The same repository under two spellings: its work reaches the node
            // through that repository's branches, and there is no release to wait on.
            if verdict.state != AdoptionState::Unanswered && verdict.consumer == verdict.producer
            {
                continue;
            }
            match verdict.state {
                AdoptionState::Completes => {}
                AdoptionState::NeverCompletes => {
                    found
                        .refused
                        .extend(verdict.refusals.iter().map(|refusal| Refused {
                            node: node.id.clone(),
                            dependency: dep.clone(),
                            rule: refusal.rule,
                            reason: refusal.reason.clone(),
                        }));
                }
                AdoptionState::Unanswered => found.unanswered.push(Unanswered {
                    node: node.id.clone(),
                    dependency: dep.clone(),
                    reason: verdict
                        .unanswered
                        .clone()
                        .unwrap_or_else(|| "onevcs gave no reason".to_owned()),
                }),
            }
        }
    }
    found
}

/// The loader's ask: every lifecycle node, refused as one invalid plan where onevcs
/// says any adoption can never complete. Unanswered ones are said on stderr.
///
/// # Errors
///
/// [`Error::Invalid`](crate::error::Error::Invalid) naming each refusal, one per line.
pub(crate) fn load(plan: &Plan) -> crate::error::Result<()> {
    let asked = ask(plan, |_| true);
    asked.report_unanswered();
    match asked.refusal() {
        Some(lines) => Err(crate::error::Error::Invalid(format!(
            "onevcs says a release adoption this plan states can never complete:\n{lines}"
        ))),
        None => Ok(()),
    }
}
