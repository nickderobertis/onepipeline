//! The executor-rules schema.
//!
//! One YAML document: the executors that exist, then ordered predicates over
//! their capacity and a node's labels. The first rule whose `when` holds decides
//! where the node dispatches; a rule with no `when` is the fallback.
//!
//! The grammar is what makes a dispatch-server or Kubernetes executor a config
//! change rather than a code change: [`select`] evaluates the same ordered
//! predicates whatever executors are declared.

// llmlint: ignore-file[invalid_states_unrepresentable] `min_free_mem` stays the `2GiB`
// string the contract's example spells — the wire syntax is the contract's, and
// [`bytes_of`] is where it becomes a byte count — and `Predicate` names only
// `executor_has_capacity`, the one predicate the contract spells, because inventing the
// label predicates it alludes to would be interface drift. Both are recorded in
// docs/contract-divergences.md.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
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
    /// [`LocalExecutor`](crate::executor::LocalExecutor) — the only kind v1
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
/// it. The contract spells exactly one condition; the label predicates it
/// alludes to would join this mapping as further fields, and are not invented
/// here.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Predicate {
    /// The named executor is within the limits its [`ExecutorEntry`] declares.
    pub executor_has_capacity: String,
}

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
                        executor_has_capacity: "local".into(),
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
        for rule in &self.rules {
            if !self.executors.iter().any(|e| e.name == rule.use_executor) {
                return Err(Error::Invalid(format!(
                    "rule uses executor '{}', which is not declared",
                    rule.use_executor
                )));
            }
            if let Some(when) = &rule.when {
                if !self
                    .executors
                    .iter()
                    .any(|e| e.name == when.executor_has_capacity)
                {
                    return Err(Error::Invalid(format!(
                        "rule tests executor '{}', which is not declared",
                        when.executor_has_capacity
                    )));
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
    pub fn select(
        &self,
        pinned: Option<&str>,
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
                Some(when) => self
                    .executors
                    .iter()
                    .find(|e| e.name == when.executor_has_capacity)
                    .is_some_and(|entry| entry.has_capacity(&report(&entry.name))),
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

        // An unreadable limit resolves toward having capacity.
        let vague = ExecutorEntry {
            min_free_mem: Some("some".into()),
            ..entry
        };
        assert!(vague.has_capacity(&report(4, 2.0, 1)));
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
                        executor_has_capacity: "fast".into(),
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
        assert_eq!(rules.select(None, &idle).expect("a rule matched"), "fast");

        let busy = |_: &str| report(4, 9.0, u64::MAX);
        assert_eq!(rules.select(None, &busy).expect("the fallback"), "slow");
    }

    #[test]
    fn a_node_that_pins_an_executor_gets_it_or_is_refused_by_name() {
        let rules = ExecutorRules::shipped_default();
        let idle = |_: &str| report(4, 0.0, u64::MAX);
        assert_eq!(rules.select(Some("local"), &idle).expect("pinned"), "local");
        let message = rules
            .select(Some("kubernetes"), &idle)
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
                    executor_has_capacity: "elsewhere".into(),
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
                    executor_has_capacity: "local".into(),
                }),
                use_executor: "local".into(),
            }],
        };
        let busy = |_: &str| report(4, 5.0, u64::MAX);
        let message = rules.select(None, &busy).unwrap_err().to_string();
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
        assert_eq!(rules.select(None, &idle).expect("a rule matched"), "local");
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
