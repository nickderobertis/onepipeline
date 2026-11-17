//! The process environment, captured once.
//!
//! Everything downstream — the environment configuration layer, document discovery,
//! and secret resolution — reads this snapshot rather than calling
//! [`std::env::var`] itself. Two reasons, and only the second is about tests. A
//! command that read the live environment twice could answer one question two ways
//! if something changed between the reads. And a snapshot is a value, so the pure
//! parsing and merging below it take one as an argument instead of reaching out of
//! process.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt;

/// A snapshot of the environment a process was started with.
///
/// # Debug redacts
///
/// This holds whatever the process was given, which on a configured host includes
/// `LINEAR_API_KEY` and `GH_PROJECTS_TOKEN`. Deriving `Debug` would put both in
/// any log line, panic message or `{:?}` that ever touched one, so the
/// implementation below prints the *names* and never a value.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Environment {
    variables: BTreeMap<String, String>,
    unusable: BTreeSet<String>,
}

impl Environment {
    /// Capture the environment this process was started with.
    ///
    /// Read as OS strings rather than through [`std::env::vars`], which panics on an
    /// entry that is not valid Unicode — and a process is handed its environment by
    /// whoever spawned it, so that is an input from outside rather than an invariant.
    ///
    /// A *name* that is not Unicode is dropped: no configuration document, no
    /// `ONETASKGRAPH_` variable and no `api_key_env:` can spell one, so nothing in this
    /// product could ever ask for it. A *value* that is not Unicode keeps its name in
    /// [`unusable`](Self::unusable), where the environment layer turns a
    /// `ONETASKGRAPH_`-prefixed one into a refusal naming the variable — because a
    /// setting this product cannot read is exactly the thing it may not quietly ignore.
    #[must_use]
    pub fn from_process() -> Self {
        Self::from_os_pairs(std::env::vars_os())
    }

    /// Build a snapshot from OS strings, sorting out what this product can read.
    ///
    /// Separate from [`from_process`](Self::from_process) so that the sorting is a
    /// function of its argument: a hand-built pair is the only way to drive the two
    /// not-valid-Unicode cases without one process spawning another.
    #[must_use]
    pub fn from_os_pairs(pairs: impl IntoIterator<Item = (OsString, OsString)>) -> Self {
        let mut variables = BTreeMap::new();
        let mut unusable = BTreeSet::new();
        for (name, value) in pairs {
            let Ok(name) = name.into_string() else {
                continue;
            };
            match value.into_string() {
                Ok(value) => {
                    variables.insert(name, value);
                }
                Err(_) => {
                    unusable.insert(name);
                }
            }
        }
        Self {
            variables,
            unusable,
        }
    }

    /// Build a snapshot from explicit pairs.
    #[must_use]
    pub fn from_pairs<K, V>(pairs: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<String>,
        V: Into<String>,
    {
        Self {
            variables: pairs
                .into_iter()
                .map(|(name, value)| (name.into(), value.into()))
                .collect(),
            unusable: BTreeSet::new(),
        }
    }

    /// Every name this process was given whose value is not valid Unicode.
    pub fn unusable(&self) -> impl Iterator<Item = &str> {
        self.unusable.iter().map(String::as_str)
    }

    /// The value of `name`, or `None` when the snapshot does not define it.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.variables.get(name).map(String::as_str)
    }

    /// The value of `name` when it is set to something other than the empty string.
    ///
    /// An exported-but-empty variable is how a shell says "unset this for the child"
    /// in practice, and treating `XDG_CONFIG_HOME=` as a path to the filesystem root
    /// would send discovery somewhere nobody asked for.
    #[must_use]
    pub fn non_empty(&self, name: &str) -> Option<&str> {
        self.get(name).filter(|value| !value.is_empty())
    }

    /// Every variable, name and value, in name order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.variables
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }

    /// Every variable name, in order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.variables.keys().map(String::as_str)
    }
}

impl fmt::Debug for Environment {
    /// Names only. See the type's own note: a value here may be a live credential.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Environment")
            .field("variables", &self.names().collect::<Vec<_>>())
            .field("unusable", &self.unusable().collect::<Vec<_>>())
            .field("values", &"<redacted>")
            .finish()
    }
}
