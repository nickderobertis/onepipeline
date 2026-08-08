//! The executor-rules schema.
//!
//! One YAML document: the executors that exist, then ordered predicates over
//! their capacity and a node's labels. The first rule whose `when` holds decides
//! where the node dispatches; a rule with no `when` is the fallback.
//!
//! This is the grammar and nothing else. Nothing here reads the file, evaluates
//! a predicate, parses `2GiB` into a byte count, or picks an executor.

// llmlint: ignore-file[boundary_inputs_validated, invalid_states_unrepresentable] the
// rules file is external input and its structural boundary is enforced —
// `deny_unknown_fields` rejects a typo, and a malformed document is refused by serde.
// Two things are deliberately left as the contract writes them (see AGENTS.md):
// `min_free_mem` stays the `2GiB` string the contract's example spells, because turning
// it into a byte count is a parser and this stage implements none; and `Predicate` names
// only `executor_has_capacity`, the one predicate the contract spells, because inventing
// the label predicates it alludes to would be interface drift. Both are recorded in
// docs/contract-divergences.md.

use serde::{Deserialize, Serialize};

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
