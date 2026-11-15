//! Qualifying a source's own id into one a user can type.
//!
//! A plugin never sees a [`GlobalId`]; that is why this type lives in the engine
//! and not in the contract.

use std::fmt;
use std::str::FromStr;

use onetaskgraph_plugin_api::{NativeId, SourceError, SourceName};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One item, qualified by the source it came from.
///
/// Rendered `<source>:<native>` and parsed by splitting on the **first** colon,
/// so a native id may contain colons freely.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
pub struct GlobalId {
    /// The configured source the item came from.
    pub source: SourceName,
    /// The source's own opaque id for it.
    pub native: NativeId,
}

impl GlobalId {
    /// The reserved metadata key a copied item records the id it was copied from under.
    ///
    /// The correspondence between an item and the one it came from lives on the item, in
    /// the plugin that owns it, and nowhere else: nothing anywhere holds a mapping, so
    /// the invariant that the engine keeps no state between calls is untouched.
    ///
    /// Spelled here rather than in the plugin contract because the value is a *qualified*
    /// id, which no plugin ever constructs or interprets — a source stores this key's
    /// value as it stores any other caller-defined one.
    pub const ORIGIN_KEY: &'static str = "onetaskgraph.origin";

    /// The reserved metadata key a copy records the first item of a chain of copies under —
    /// the qualified id of the item copied from nowhere — when that is not already the
    /// [`ORIGIN_KEY`](Self::ORIGIN_KEY) it records.
    ///
    /// [`ORIGIN_KEY`](Self::ORIGIN_KEY) names one hop and every hop overwrites it, so an item
    /// two copies from where it was authored could only be led back there through the copy
    /// between — and not at all once that copy was deleted. This key is carried from hop to
    /// hop instead, so an item's root is one read of the item itself: the key, else its
    /// origin, else its own id. A copy whose origin is withheld records none, and one naming
    /// a private source is never recorded where a write is not declared private. Spelled
    /// beside [`ORIGIN_KEY`](Self::ORIGIN_KEY), for its reason.
    pub const LINEAGE_KEY: &'static str = "onetaskgraph.lineage";

    /// Qualify `native` as belonging to `source`.
    #[must_use]
    pub fn new(source: SourceName, native: NativeId) -> Self {
        Self { source, native }
    }
}

impl fmt::Display for GlobalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.source, self.native)
    }
}

impl FromStr for GlobalId {
    type Err = SourceError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let Some((source, native)) = value.split_once(':') else {
            return Err(SourceError::Config {
                message: format!(
                    "{value:?} is not a qualified id; write it as <source>:<id>, for example \
                     work:ENG-1"
                ),
            });
        };
        if native.is_empty() {
            return Err(SourceError::Config {
                message: format!("{value:?} names a source but no id; write it as <source>:<id>"),
            });
        }
        Ok(Self {
            source: SourceName::new(source)?,
            native: NativeId::from(native),
        })
    }
}

impl TryFrom<String> for GlobalId {
    type Error = SourceError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<GlobalId> for String {
    fn from(value: GlobalId) -> Self {
        value.to_string()
    }
}
