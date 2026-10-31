//! Shared lifecycle journal assertions and pool setup.

use crate::harness::World;

/// The identity key `onevcs` resolves this world's `service` repository to.
///
/// Spelled once: the hold, the surface and the status line all name it, and a
/// journey that read it back off the record it was asserting would prove only
/// that the record agreed with itself.
pub(crate) const SERVICE_IDENTITY: &str = "github.com/owner/service";

/// Every `onevcs`-produced event one run recorded, by kind.
pub(crate) fn vcs_kinds(world: &World, run: &str) -> Vec<String> {
    world
        .journal(run)
        .iter()
        .filter(|event| event["source"] == "vcs")
        .filter_map(|event| event["kind"].as_str().map(str::to_string))
        .collect()
}

/// Why a run settled the way it did, as the sibling itself said it.
pub(crate) fn why(world: &World, run: &str) -> String {
    let settled: Vec<String> = world
        .events_of(run, "node-settled")
        .iter()
        .map(|event| {
            format!(
                "{} {} {}: {}",
                event["labels"]["node"],
                event["payload"]["status"],
                event["payload"]["outcome"],
                event["payload"]["detail"]
            )
        })
        .collect();
    format!(
        "what the nodes settled on:\n  {}\n  the sibling recorded: {:?}",
        settled.join("\n  "),
        vcs_kinds(world, run)
    )
}

/// A re-dispatch is another `node-dispatched`, so counting these in order is how
/// a journey tells one attempt from several.
pub(crate) fn dispatches_of(world: &World, run: &str, node: &str) -> Vec<serde_json::Value> {
    world
        .events_of(run, "node-dispatched")
        .into_iter()
        .filter(|event| event["labels"]["node"] == node)
        .collect()
}

/// Size this host's pools: one warm slot and no overflow for the `service`
/// identity, and the shipped default — no pool, unlimited overflow — for every
/// other.
///
/// A rule rather than the default, so a second identity in the same world is
/// **unconfigured**: `onevcs` places a session of an unconfigured identity
/// without surveying its pool, which is what lets a journey make the capacity
/// read fail for one identity while its open still succeeds.
pub(crate) fn pool_one_slot_no_overflow(world: &World) {
    std::fs::write(
        world.onevcs_home().join("workspaces.yml"),
        "version: 1\nrules:\n  - match: {host: github.com, owner: owner, name: service}\n    \
         pool: 1\n    overflow: 0\n",
    )
    .expect("the workspaces file is written");
}

/// The `workspace` reason on the **first** `node-held` naming `node` — the hold
/// the pass both nodes were ready in decided — if that hold carries one.
///
/// Not the newest: the record is written on every change of reason or reading,
/// so a later pass can replace that hold a moment after it is written — with the
/// host's room once the host fills, or with a reading taken after a session
/// opened — and a wait that reads only the newest misses the pass it is about.
pub(crate) fn first_workspace_hold_of(
    world: &World,
    run: &str,
    node: &str,
) -> Option<serde_json::Value> {
    world
        .events_of(run, "node-held")
        .into_iter()
        .find(|event| event["labels"]["node"] == node)?["payload"]["reasons"]
        .as_array()?
        .iter()
        .find(|reason| reason["kind"] == "workspace")
        .cloned()
}
