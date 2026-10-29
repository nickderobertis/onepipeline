//! Journal assertions shared by pooled-slot and Unix hand-back journeys.

use crate::harness::World;

use super::pool_support::why;

/// The `workspace` entry holding one node, off the newest `node-held` naming it.
pub(crate) fn workspace_hold_of(world: &World, run: &str, node: &str) -> Option<serde_json::Value> {
    world
        .events_of(run, "node-held")
        .into_iter()
        .rfind(|event| event["labels"]["node"] == node)?["payload"]["reasons"]
        .as_array()?
        .iter()
        .find(|reason| reason["kind"] == "workspace")
        .cloned()
}

/// The stream position of the first event of `kind` labelled `node`.
pub(crate) fn first_seq(world: &World, run: &str, kind: &str, node: &str) -> u64 {
    world
        .events_of(run, kind)
        .iter()
        .find(|event| event["labels"]["node"] == node)
        .and_then(|event| event["seq"].as_u64())
        .unwrap_or_else(|| panic!("no {kind} for {node}\n{}", why(world, run)))
}
