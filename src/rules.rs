//! The executor-rules schema.
//!
//! One YAML document: the executors that exist, then ordered predicates over
//! their capacity and a node's labels. The first rule whose `when` holds decides
//! where the node dispatches; a rule with no `when` is the fallback.
//!
//! The grammar is what makes a dispatch-server or Kubernetes executor a config
//! change rather than a code change: [`ExecutorRules::select`] evaluates the same
//! ordered predicates whatever executors are declared.

// llmlint: ignore-file[invalid_states_unrepresentable] `min_free_mem` stays the `2GiB`
// string the contract's example spells — the wire syntax is the contract's, and
// [`bytes_of`] is where it becomes a byte count. `Predicate`'s two families are both
// optional fields rather than an enum because the contract makes `when` a *mapping* whose
// conditions conjoin; "neither is set" is refused at load instead. Divergences 4 and 5 in
// docs/contract-divergences.md record both rulings.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::event::Labels;
use crate::executor::{CapacityReport, Executor, LocalExecutor};

/// The executor-rules file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutorRules {
    /// The executors that exist, by name.
    pub executors: Vec<ExecutorEntry>,
    /// Ordered: the first rule whose `when` holds decides.
    pub rules: Vec<Rule>,
}

/// One executor an [`ExecutorRules`] file declares.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutorEntry {
    /// The name a [`Rule`] selects it by.
    pub name: String,
    /// Which executor implementation it is.
    #[serde(rename = "type")]
    pub kind: ExecutorKind,
    /// Refuse a dispatch once the one-minute load average is above this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_load1: Option<f64>,
    /// Refuse a dispatch once free memory is below this, written the way the
    /// contract's example writes it (`2GiB`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_free_mem: Option<String>,
}

impl ExecutorEntry {
    /// Whether this executor is within the limits it declares.
    ///
    /// An unreadable limit resolves toward "has capacity", the same way the
    /// capacity probe's own unknowns do: a rules file nobody can evaluate must
    /// not stall a healthy host.
    pub fn has_capacity(&self, report: &CapacityReport) -> bool {
        if report.slots_free == 0 {
            return false;
        }
        if let Some(max) = self.max_load1 {
            if report.load1 > max {
                return false;
            }
        }
        if let Some(min) = self.min_free_mem.as_deref().and_then(bytes_of) {
            if report.mem_free_bytes < min {
                return false;
            }
        }
        true
    }
}

/// Read a binary-prefixed size as a byte count, as the contract's `2GiB` writes
/// it.
///
/// The wire syntax is the contract's, so this reads exactly what it spells and
/// answers `None` for anything else rather than guessing at a unit.
pub fn bytes_of(text: &str) -> Option<u64> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let number: f64 = number.parse().ok()?;
    if !number.is_finite() || number < 0.0 {
        return None;
    }
    let scale: u64 = match unit.trim() {
        "" | "B" => 1,
        "KiB" => 1 << 10,
        "MiB" => 1 << 20,
        "GiB" => 1 << 30,
        "TiB" => 1 << 40,
        _ => return None,
    };
    let bytes = number * scale as f64;
    (bytes.is_finite() && bytes >= 0.0).then_some(bytes as u64)
}

/// Which executor implementation an [`ExecutorEntry`] names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ExecutorKind {
    /// [`crate::executor::LocalExecutor`] — the only kind v1
    /// ships.
    Local,
}

/// One ordered rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// The predicate. Omitted, the rule always holds — the fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<Predicate>,
    /// The executor to dispatch on when it holds.
    #[serde(rename = "use")]
    pub use_executor: String,
}

/// What a [`Rule`] tests.
///
/// A mapping, as the contract's `when: {executor_has_capacity: local}` writes
/// it, holding the contract's two predicate families. Several conditions in one
/// mapping **conjoin**: all of them hold or the rule does not fire. A mapping
/// naming neither family is refused when the file loads — read as an always-true
/// rule it would shadow every rule after it, which is never what someone who
/// wrote a `when` at all meant.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Predicate {
    /// The named executor is within the limits its [`ExecutorEntry`] declares.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor_has_capacity: Option<String>,
    /// The node's own labels carry each of these, by exact string equality.
    ///
    /// Exact rather than glob: a rules file decides where work runs, and a
    /// pattern language is a second thing to get wrong at the one boundary that
    /// must not silently match the wrong host.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub node_label: BTreeMap<String, String>,
}

impl Predicate {
    /// Whether this predicate names nothing at all.
    fn is_empty(&self) -> bool {
        self.executor_has_capacity.is_none() && self.node_label.is_empty()
    }

    /// Whether the node's labels carry every pair this predicate names.
    fn labels_match(&self, labels: &Labels) -> bool {
        self.node_label
            .iter()
            .all(|(key, value)| label_of(labels, key).is_some_and(|actual| &actual == value))
    }
}

/// One reserved label, as a string a rules file can be written against.
fn label_of(labels: &Labels, key: &str) -> Option<String> {
    match key {
        "run_id" => labels.run_id.clone(),
        "round" => labels.round.map(|round| round.to_string()),
        "node" => labels.node.clone(),
        "persona" => labels.persona.clone(),
        _ => None,
    }
}

/// The reserved label keys a `node_label` predicate may name.
///
/// `step` is deliberately absent: an executor is chosen once per node, before
/// any of its steps run, so a rule testing `step` could never hold. Refusing it
/// at load says that; accepting it would leave a rule that silently never fires.
/// A free-form extra is absent for the same reason it is free-form — no plan
/// schema declares it, so nothing could be validated against.
pub const SELECTABLE_LABELS: &[&str] = &["run_id", "round", "node", "persona"];

impl ExecutorRules {
    /// The rules a run uses when it is pointed at no file.
    ///
    /// v1 ships one executor, so the default is the contract's own example: try
    /// the local executor while it has capacity, and fall back to it anyway —
    /// there is nowhere else for the work to go, and refusing to dispatch would
    /// be worse than dispatching onto a busy host.
    pub fn shipped_default() -> Self {
        Self {
            executors: vec![ExecutorEntry {
                name: "local".into(),
                kind: ExecutorKind::Local,
                max_load1: None,
                min_free_mem: None,
            }],
            rules: vec![
                Rule {
                    when: Some(Predicate {
                        executor_has_capacity: Some("local".into()),
                        ..Predicate::default()
                    }),
                    use_executor: "local".into(),
                },
                Rule {
                    when: None,
                    use_executor: "local".into(),
                },
            ],
        }
    }

    /// Read a rules file, refusing anything the grammar does not accept.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| Error::Ledger {
            path: path.to_path_buf(),
            source: e,
        })?;
        let rules: Self = serde_norway::from_str(&text)
            .map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))?;
        rules.validate()?;
        Ok(rules)
    }

    /// Check that every rule names a declared executor.
    ///
    /// A rule pointing at an executor nobody declared would silently never fire,
    /// which reads as a scheduling bug rather than the typo it is.
    pub fn validate(&self) -> Result<()> {
        if self.executors.is_empty() {
            return Err(Error::Invalid(
                "a rules file needs at least one executor".into(),
            ));
        }
        for entry in &self.executors {
            // A limit nobody can read resolves toward "has capacity", so a
            // `2GB` typo would silently mean *no limit at all* — the executor
            // it was written to protect would take every dispatch on a host
            // that has run out of memory. A rules file is external input, so
            // it fails here, by name, rather than at the first dispatch.
            if let Some(limit) = &entry.min_free_mem {
                if bytes_of(limit).is_none() {
                    return Err(Error::Invalid(format!(
                        "executor '{}' sets min_free_mem '{limit}', which is not a size: \
                         write a byte count or one of B, KiB, MiB, GiB, TiB",
                        entry.name
                    )));
                }
            }
        }
        for rule in &self.rules {
            if !self.executors.iter().any(|e| e.name == rule.use_executor) {
                return Err(Error::Invalid(format!(
                    "rule uses executor '{}', which is not declared",
                    rule.use_executor
                )));
            }
            if let Some(when) = &rule.when {
                if when.is_empty() {
                    return Err(Error::Invalid(
                        "a rule's `when` names no condition: write \
                         `executor_has_capacity`, `node_label`, or no `when` at all"
                            .into(),
                    ));
                }
                if let Some(tested) = &when.executor_has_capacity {
                    if !self.executors.iter().any(|e| &e.name == tested) {
                        return Err(Error::Invalid(format!(
                            "rule tests executor '{tested}', which is not declared"
                        )));
                    }
                }
                // A label nothing can ever carry is a typo, not a rule that
                // happens never to fire: it would silently shadow the fallback's
                // job of explaining where the work went.
                for key in when.node_label.keys() {
                    if !SELECTABLE_LABELS.contains(&key.as_str()) {
                        return Err(Error::Invalid(format!(
                            "rule tests node label '{key}', which is not one of {}",
                            SELECTABLE_LABELS.join(", ")
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    /// The executor a node dispatches on.
    ///
    /// A node's own `executor` wins outright: naming one is the planner deciding
    /// where the work runs. Otherwise the rules are ordered — the first whose
    /// `when` holds decides, and a rule with no `when` is the fallback.
    /// `labels` are the node's own, which is the granularity the choice is made
    /// at: one executor per node, before any of its steps run.
    pub fn select(
        &self,
        pinned: Option<&str>,
        labels: &Labels,
        report: &dyn Fn(&str) -> CapacityReport,
    ) -> Result<String> {
        if let Some(pinned) = pinned {
            if !self.executors.iter().any(|e| e.name == pinned) {
                return Err(Error::Invalid(format!(
                    "node pins executor '{pinned}', which the rules do not declare"
                )));
            }
            return Ok(pinned.to_string());
        }
        for rule in &self.rules {
            let holds = match &rule.when {
                None => true,
                Some(when) => {
                    let capacity = when.executor_has_capacity.as_ref().is_none_or(|tested| {
                        self.executors
                            .iter()
                            .find(|e| &e.name == tested)
                            .is_some_and(|entry| entry.has_capacity(&report(&entry.name)))
                    });
                    capacity && when.labels_match(labels)
                }
            };
            if holds {
                return Ok(rule.use_executor.clone());
            }
        }
        Err(Error::Invalid(
            "no rule matched and none is a fallback: nothing can dispatch".into(),
        ))
    }
}

/// The executor implementation a declared entry names.
pub fn executor_for(entry: &ExecutorEntry) -> Box<dyn Executor> {
    match entry.kind {
        ExecutorKind::Local => Box::new(LocalExecutor),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(slots_free: u32, load1: f64, mem_free_bytes: u64) -> CapacityReport {
        CapacityReport {
            slots_free,
            load1,
            mem_free_bytes,
        }
    }

    #[test]
    fn the_contracts_own_size_syntax_reads_as_bytes() {
        assert_eq!(bytes_of("2GiB"), Some(2 * (1 << 30)));
        assert_eq!(bytes_of("512MiB"), Some(512 * (1 << 20)));
        assert_eq!(bytes_of("1KiB"), Some(1024));
        assert_eq!(bytes_of("1TiB"), Some(1u64 << 40));
        assert_eq!(bytes_of("4096"), Some(4096));
        assert_eq!(bytes_of("4096B"), Some(4096));
        assert_eq!(bytes_of(" 1.5GiB "), Some(1_610_612_736));
        // Anything the contract does not spell is refused rather than guessed.
        assert_eq!(bytes_of("2GB"), None);
        assert_eq!(bytes_of("lots"), None);
        assert_eq!(bytes_of("-1GiB"), None);
        assert_eq!(bytes_of(""), None);
    }

    #[test]
    fn an_executor_is_out_of_capacity_when_any_limit_it_declares_is_exceeded() {
        let entry = ExecutorEntry {
            name: "local".into(),
            kind: ExecutorKind::Local,
            max_load1: Some(8.0),
            min_free_mem: Some("2GiB".into()),
        };
        assert!(entry.has_capacity(&report(4, 2.0, 8 << 30)));
        assert!(!entry.has_capacity(&report(0, 2.0, 8 << 30)), "no slots");
        assert!(
            !entry.has_capacity(&report(4, 9.0, 8 << 30)),
            "over max_load1"
        );
        assert!(
            !entry.has_capacity(&report(4, 2.0, 1 << 30)),
            "under min_free_mem"
        );

        // Defence in depth: `validate` refuses such a file at load, so this is
        // only reachable for an entry built in process. It still resolves
        // toward having capacity rather than stalling a healthy host.
        let vague = ExecutorEntry {
            min_free_mem: Some("some".into()),
            ..entry
        };
        assert!(vague.has_capacity(&report(4, 2.0, 1)));
    }

    /// The limit exists to keep dispatches off a host that has run out of
    /// memory. A unit this crate cannot read used to mean *no limit at all*, so
    /// the one file written to enforce the bound was the one that removed it.
    #[test]
    fn a_memory_limit_in_a_unit_this_build_cannot_read_is_refused_by_name() {
        let unreadable = ExecutorRules {
            executors: vec![ExecutorEntry {
                name: "local".into(),
                kind: ExecutorKind::Local,
                max_load1: None,
                // Decimal `GB`, not binary `GiB` — the plausible typo.
                min_free_mem: Some("2GB".into()),
            }],
            rules: vec![Rule {
                when: None,
                use_executor: "local".into(),
            }],
        };
        let refused = unreadable.validate().expect_err("an unreadable unit");
        let said = refused.to_string();
        assert!(said.contains("min_free_mem"), "{said}");
        assert!(said.contains("2GB"), "{said}");
        assert!(
            said.contains("GiB"),
            "the refusal did not say what to write: {said}"
        );

        // Every unit the contract spells still loads.
        for good in ["2GiB", "512MiB", "1024", "2048B", "1KiB", "1TiB"] {
            let entry = ExecutorEntry {
                min_free_mem: Some(good.to_string()),
                ..unreadable.executors[0].clone()
            };
            ExecutorRules {
                executors: vec![entry],
                ..unreadable.clone()
            }
            .validate()
            .unwrap_or_else(|e| panic!("{good} was refused: {e}"));
        }
    }

    #[test]
    fn the_first_rule_whose_predicate_holds_decides() {
        let rules = ExecutorRules {
            executors: vec![
                ExecutorEntry {
                    name: "fast".into(),
                    kind: ExecutorKind::Local,
                    max_load1: Some(1.0),
                    min_free_mem: None,
                },
                ExecutorEntry {
                    name: "slow".into(),
                    kind: ExecutorKind::Local,
                    max_load1: None,
                    min_free_mem: None,
                },
            ],
            rules: vec![
                Rule {
                    when: Some(Predicate {
                        executor_has_capacity: Some("fast".into()),
                        ..Predicate::default()
                    }),
                    use_executor: "fast".into(),
                },
                Rule {
                    when: None,
                    use_executor: "slow".into(),
                },
            ],
        };
        rules
            .validate()
            .expect("both rules name declared executors");

        let idle = |_: &str| report(4, 0.5, u64::MAX);
        assert_eq!(
            rules
                .select(None, &Labels::default(), &idle)
                .expect("a rule matched"),
            "fast"
        );

        let busy = |_: &str| report(4, 9.0, u64::MAX);
        assert_eq!(
            rules
                .select(None, &Labels::default(), &busy)
                .expect("the fallback"),
            "slow"
        );
    }

    /// The rules a label-routing file writes: reviewers to one executor,
    /// everything else to the other.
    fn by_persona() -> ExecutorRules {
        ExecutorRules {
            executors: vec![
                ExecutorEntry {
                    name: "review-pool".into(),
                    kind: ExecutorKind::Local,
                    max_load1: None,
                    min_free_mem: None,
                },
                ExecutorEntry {
                    name: "local".into(),
                    kind: ExecutorKind::Local,
                    max_load1: None,
                    min_free_mem: None,
                },
            ],
            rules: vec![
                Rule {
                    when: Some(Predicate {
                        executor_has_capacity: Some("review-pool".into()),
                        node_label: BTreeMap::from([("persona".into(), "reviewer".into())]),
                    }),
                    use_executor: "review-pool".into(),
                },
                Rule {
                    when: None,
                    use_executor: "local".into(),
                },
            ],
        }
    }

    fn labelled(node: &str, persona: &str) -> Labels {
        Labels {
            run_id: Some("demo".into()),
            round: Some(2),
            node: Some(node.into()),
            persona: Some(persona.into()),
            ..Labels::default()
        }
    }

    #[test]
    fn a_node_label_predicate_matches_the_nodes_own_labels_exactly() {
        let rules = by_persona();
        rules.validate().expect("both families are legal");
        let idle = |_: &str| report(4, 0.0, u64::MAX);

        assert_eq!(
            rules
                .select(None, &labelled("audit", "reviewer"), &idle)
                .expect("the label rule holds"),
            "review-pool"
        );
        // Exact, not a prefix or a glob: `reviewer-2` is a different persona.
        assert_eq!(
            rules
                .select(None, &labelled("audit", "reviewer-2"), &idle)
                .expect("the fallback"),
            "local"
        );
        // A label the node does not carry at all cannot match.
        assert_eq!(
            rules
                .select(None, &Labels::default(), &idle)
                .expect("the fallback"),
            "local"
        );
    }

    #[test]
    fn the_conditions_in_one_when_conjoin() {
        let mut rules = by_persona();
        // The label holds; the capacity half does not, so the rule does not fire.
        let exhausted = |_: &str| report(0, 0.0, u64::MAX);
        assert_eq!(
            rules
                .select(None, &labelled("audit", "reviewer"), &exhausted)
                .expect("the fallback"),
            "local"
        );

        // And the other way round: capacity holds, the label does not.
        rules.rules[0].when = Some(Predicate {
            executor_has_capacity: Some("review-pool".into()),
            node_label: BTreeMap::from([("node".into(), "audit".into())]),
        });
        let idle = |_: &str| report(4, 0.0, u64::MAX);
        assert_eq!(
            rules
                .select(None, &labelled("build", "reviewer"), &idle)
                .expect("the fallback"),
            "local"
        );
    }

    #[test]
    fn a_when_that_names_nothing_or_names_a_label_that_is_not_one_is_refused() {
        let mut rules = by_persona();
        rules.rules[0].when = Some(Predicate::default());
        let message = rules.validate().unwrap_err().to_string();
        assert!(message.contains("names no condition"), "{message}");

        let mut rules = by_persona();
        rules.rules[0].when = Some(Predicate {
            executor_has_capacity: None,
            node_label: BTreeMap::from([("presona".into(), "reviewer".into())]),
        });
        let message = rules.validate().unwrap_err().to_string();
        assert!(message.contains("presona"), "{message}");
    }

    #[test]
    fn a_label_predicate_round_trips_through_the_rules_file_syntax() {
        let yaml = "executors:\n  - {name: local, type: local}\nrules:\n  \
                    - when: {node_label: {persona: reviewer}}\n    use: local\n  \
                    - use: local\n";
        let rules: ExecutorRules = serde_norway::from_str(yaml).expect("it parses");
        rules.validate().expect("it validates");
        assert_eq!(
            rules.rules[0]
                .when
                .as_ref()
                .expect("a predicate")
                .node_label,
            BTreeMap::from([("persona".to_string(), "reviewer".to_string())])
        );
        let again: ExecutorRules =
            serde_norway::from_str(&serde_norway::to_string(&rules).expect("serializes"))
                .expect("re-parses");
        assert_eq!(again, rules);
    }

    #[test]
    fn a_node_that_pins_an_executor_gets_it_or_is_refused_by_name() {
        let rules = ExecutorRules::shipped_default();
        let idle = |_: &str| report(4, 0.0, u64::MAX);
        assert_eq!(
            rules
                .select(Some("local"), &Labels::default(), &idle)
                .expect("pinned"),
            "local"
        );
        let message = rules
            .select(Some("kubernetes"), &Labels::default(), &idle)
            .unwrap_err()
            .to_string();
        assert!(message.contains("kubernetes"), "{message}");
    }

    #[test]
    fn a_rules_file_naming_an_undeclared_executor_is_refused() {
        let undeclared_use = ExecutorRules {
            executors: vec![ExecutorEntry {
                name: "local".into(),
                kind: ExecutorKind::Local,
                max_load1: None,
                min_free_mem: None,
            }],
            rules: vec![Rule {
                when: None,
                use_executor: "elsewhere".into(),
            }],
        };
        assert!(undeclared_use
            .validate()
            .unwrap_err()
            .to_string()
            .contains("not declared"));

        let undeclared_test = ExecutorRules {
            executors: vec![ExecutorEntry {
                name: "local".into(),
                kind: ExecutorKind::Local,
                max_load1: None,
                min_free_mem: None,
            }],
            rules: vec![Rule {
                when: Some(Predicate {
                    executor_has_capacity: Some("elsewhere".into()),
                    ..Predicate::default()
                }),
                use_executor: "local".into(),
            }],
        };
        assert!(undeclared_test
            .validate()
            .unwrap_err()
            .to_string()
            .contains("tests executor"));

        let empty = ExecutorRules {
            executors: vec![],
            rules: vec![],
        };
        assert!(empty
            .validate()
            .unwrap_err()
            .to_string()
            .contains("at least one"));
    }

    #[test]
    fn a_rules_file_with_no_matching_rule_and_no_fallback_says_so() {
        let rules = ExecutorRules {
            executors: vec![ExecutorEntry {
                name: "local".into(),
                kind: ExecutorKind::Local,
                max_load1: Some(0.0),
                min_free_mem: None,
            }],
            rules: vec![Rule {
                when: Some(Predicate {
                    executor_has_capacity: Some("local".into()),
                    ..Predicate::default()
                }),
                use_executor: "local".into(),
            }],
        };
        let busy = |_: &str| report(4, 5.0, u64::MAX);
        let message = rules
            .select(None, &Labels::default(), &busy)
            .unwrap_err()
            .to_string();
        assert!(message.contains("nothing can dispatch"), "{message}");
    }

    #[test]
    fn the_shipped_example_file_loads_and_validates() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/executors.yaml");
        let rules = ExecutorRules::load(&path).expect("the shipped example is legal");
        assert_eq!(rules.executors[0].name, "local");
        assert_eq!(rules.executors[0].min_free_mem.as_deref(), Some("2GiB"));
        assert_eq!(rules.rules.len(), 2);

        let idle = |_: &str| report(4, 0.0, u64::MAX);
        assert_eq!(
            rules
                .select(None, &Labels::default(), &idle)
                .expect("a rule matched"),
            "local"
        );
    }

    #[test]
    fn a_missing_or_malformed_rules_file_is_refused_at_its_boundary() {
        let missing = std::path::Path::new("no/such/executors.yaml");
        assert!(ExecutorRules::load(missing).is_err());

        let dir = std::env::temp_dir().join(format!("onepipeline-rules-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let path = dir.join("executors.yaml");
        std::fs::write(
            &path,
            "executors: [{name: local, type: local, typo: 1}]\nrules: []\n",
        )
        .expect("written");
        let message = ExecutorRules::load(&path).unwrap_err().to_string();
        assert!(message.contains("typo"), "{message}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_declared_local_entry_resolves_to_the_local_executor() {
        let entry = ExecutorEntry {
            name: "local".into(),
            kind: ExecutorKind::Local,
            max_load1: None,
            min_free_mem: None,
        };
        assert_eq!(executor_for(&entry).name(), "local");
    }
}
