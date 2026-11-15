//! How two of a task's change tokens stand to each other.
//!
//! A [`Task::version`](onetaskgraph_plugin_api::Task::version) is opaque to every caller, and
//! the one reader that looks inside is the source that spelled it, through
//! [`TaskSource::order_versions`](onetaskgraph_plugin_api::TaskSource::order_versions), so a
//! token's format and its order have one source each. [`Engine::compare_versions`] asks it.
//!
//! The answer is about **two versions of one item from one source**: versions of different
//! items, or of different sources, are never compared.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::{Engine, GlobalId};

/// How one version of an item stands to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VersionOrder {
    /// Nothing the token covers changed between them.
    Equal,
    /// The first is the later of the two.
    Later,
    /// The first is the earlier of the two.
    Earlier,
    /// The two cannot be put in order: different items, a source this engine has not built,
    /// a value the source cannot parse, or two versions its token says differ without saying
    /// which came first.
    Incomparable,
}

/// One version of one item: the item's qualified id and the token its source gave.
#[derive(Debug, Clone, Copy)]
pub struct Versioned<'a> {
    /// The item.
    pub item: &'a GlobalId,
    /// Its source's token for it.
    pub version: &'a str,
}

impl Engine {
    /// How `a` stands to `b`, by the ordering of the source both name.
    #[must_use]
    pub fn compare_versions(&self, a: Versioned<'_>, b: Versioned<'_>) -> VersionOrder {
        if a.item != b.item {
            return VersionOrder::Incomparable;
        }
        let Some(source) = self.ready_source(&a.item.source) else {
            return VersionOrder::Incomparable;
        };
        match source.order_versions(a.version, b.version) {
            Some(Ordering::Equal) => VersionOrder::Equal,
            Some(Ordering::Greater) => VersionOrder::Later,
            Some(Ordering::Less) => VersionOrder::Earlier,
            None => VersionOrder::Incomparable,
        }
    }
}
