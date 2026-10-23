//! A run's **member liveness**, kept beside its journal rather than in it.
//!
//! Every member of a dispatch heartbeats — `oneagentgraph` sends a
//! `member-heartbeat` every fifteen seconds while a member's turn is open — and
//! the only question any reader asks of those beats is the latest one: how long
//! ago a member was last heard from. Journalled, each beat was a record of about
//! 430 bytes every reader, pass and watch then carried for the rest of the run:
//! about 100 KB per member-hour, and a third of a long run's journal.
//!
//! So a driver keeps **one entry per member of a dispatch in flight** in
//! `<run>/liveness.json` — keyed by the node, the `oneagentgraph` run and the
//! member — overwrites it whole and atomically when a beat advances an entry, and
//! drops a node's entries when its dispatch settles. Nothing else changes about a
//! member's records: everything a member emits but its heartbeats is still
//! journalled, so a transcript and the read API's evidence read what they read
//! before.
//!
//! # What reads it
//!
//! A reader takes a member's last heartbeat as **the later of the journal's and
//! this file's**, so a run recorded before this file existed — its heartbeats in
//! its journal — reads exactly as it did. The file is derived state: absent, or
//! missing an entry, it says nothing beyond what the journal holds, an older
//! engine ignores it, and deleting it loses only the beats since the journal last
//! carried one.
//!
//! An observer's members heartbeat too, and carry no node: they are not members
//! of a dispatch, so their beats are neither journalled nor kept here. The
//! observer's health has words of its own, read off the launch record and
//! `oneagentgraph`'s own history.

// llmlint: ignore-file[invalid_states_unrepresentable] every identifier and timestamp here is
// a `String` for the reason `src/ledger.rs`'s own file-level suppression states: a node id, a
// graph run id, a member name and an instant are serialized fields a later build reads back,
// so a reader has to accept what is there. The instant is checked where it is written — only
// a stamp `projection::millis_of` reads is ever recorded — and where it is read.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::event::Envelope;
use crate::ledger::{self, RunPaths};
use crate::projection::{self, RunState};

/// The schema version of `liveness.json`, refused where it is not this one: a
/// document a reader does not understand is read as no document, which is what an
/// absent one says.
pub(crate) const LIVENESS_SCHEMA_VERSION: u32 = 1;

fn this_version<'de, D: serde::Deserializer<'de>>(reader: D) -> Result<u32, D::Error> {
    let found = u32::deserialize(reader)?;
    if found != LIVENESS_SCHEMA_VERSION {
        return Err(serde::de::Error::custom(format!(
            "liveness schema_version {found}, and this build reads {LIVENESS_SCHEMA_VERSION}"
        )));
    }
    Ok(found)
}

/// `<run>/liveness.json`: the one type its writer and every reader share.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Liveness {
    #[serde(deserialize_with = "this_version")]
    schema_version: u32,
    /// One entry per member of a dispatch still in flight.
    pub(crate) members: Vec<MemberBeat>,
}

impl Default for Liveness {
    fn default() -> Self {
        Self {
            schema_version: LIVENESS_SCHEMA_VERSION,
            members: Vec::new(),
        }
    }
}

/// One member's latest heartbeat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MemberBeat {
    /// The node whose dispatch the member belongs to.
    pub(crate) node: String,
    /// The `oneagentgraph` run the member is part of.
    pub(crate) graph_run: String,
    /// The member's name in that graph.
    pub(crate) member: String,
    /// When it last heartbeat: RFC 3339, UTC, to the millisecond.
    pub(crate) last_heartbeat_at: String,
}

impl Liveness {
    /// The run's liveness as its file holds it: nothing, where there is no file
    /// or one this build cannot read.
    pub(crate) fn read(paths: &RunPaths) -> Self {
        ledger::read_json_opt(&paths.liveness()).unwrap_or_default()
    }

    /// The latest heartbeat any member sent, in milliseconds.
    pub(crate) fn latest(&self) -> Option<u64> {
        self.members
            .iter()
            .filter_map(|beat| projection::millis_of(&beat.last_heartbeat_at))
            .max()
    }
}

/// Every change to a run's file in this process goes through this, so the relay
/// recording a beat and the loop dropping a settled node's entries never write
/// over each other's read.
static WRITING: Mutex<()> = Mutex::new(());

/// Whether `event` is a member's heartbeat.
pub(crate) fn is_heartbeat(event: &Envelope) -> bool {
    event.source == crate::event::Source::Agentgraph
        && event.kind.0 == oneagentgraph::event::EventKind::MemberHeartbeat.as_str()
}

/// Record a heartbeat a dispatch's member sent, where it advances that member's
/// entry. A beat naming no node — an observer's — is not a dispatch member's, and
/// is not kept.
pub(crate) fn beat(paths: &RunPaths, event: &Envelope) {
    let Some(node) = event.labels.node.as_deref() else {
        return;
    };
    let Some(at) = projection::millis_of(&event.ts) else {
        return;
    };
    let graph_run = event
        .labels
        .run_id
        .clone()
        .unwrap_or_else(|| event.stream.clone());
    let member = event.labels.member.clone().unwrap_or_default();
    let _writing = WRITING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut liveness = Liveness::read(paths);
    let held = liveness
        .members
        .iter_mut()
        .find(|beat| beat.node == node && beat.graph_run == graph_run && beat.member == member);
    match held {
        Some(beat)
            if projection::millis_of(&beat.last_heartbeat_at).is_some_and(|was| was >= at) =>
        {
            return;
        }
        Some(beat) => beat.last_heartbeat_at = event.ts.clone(),
        None => liveness.members.push(MemberBeat {
            node: node.to_string(),
            graph_run,
            member,
            last_heartbeat_at: event.ts.clone(),
        }),
    }
    write(paths, &liveness);
}

/// Drop every entry of `node`'s members: its dispatch has settled.
pub(crate) fn settled(paths: &RunPaths, node: &str) {
    let _writing = WRITING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut liveness = Liveness::read(paths);
    let before = liveness.members.len();
    liveness.members.retain(|beat| beat.node != node);
    if liveness.members.len() != before {
        write(paths, &liveness);
    }
}

/// Drop every entry: a driver taking the run over has ended every dispatch the
/// driver before it left in flight.
pub(crate) fn clear(paths: &RunPaths) {
    let _writing = WRITING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _ = std::fs::remove_file(paths.liveness());
}

/// Written whole and atomically, best effort: a file that could not be written
/// costs a reader the beats since the journal last carried one, and nothing else.
// llmlint: ignore[changed_behavior_has_e2e] a liveness file that cannot be written needs a run directory the driver itself writes and may not, which is host sabotage rather than a journey; what a reader does with no usable file — read the journal's beats — is driven by `journal_scaling::journalled_heartbeats_with_no_liveness_file_read_as_they_did`.
fn write(paths: &RunPaths, liveness: &Liveness) {
    let _ = ledger::write_json(&paths.liveness(), liveness);
}

/// Take what the file says onto a folded state: each node's last heartbeat is the
/// later of the fold's and the file's, and so is the run's last write — a live
/// member's heartbeat is activity a quiet run is judged by, as a journalled one
/// was.
pub(crate) fn apply(paths: &RunPaths, state: &mut RunState) {
    let liveness = Liveness::read(paths);
    for beat in &liveness.members {
        let Some(at) = projection::millis_of(&beat.last_heartbeat_at) else {
            continue;
        };
        let activity = state.activity.entry(beat.node.clone()).or_default();
        activity.last_heartbeat_at = Some(activity.last_heartbeat_at.map_or(at, |was| was.max(at)));
    }
    if let Some(latest) = liveness.latest() {
        state.last_write_at = Some(state.last_write_at.map_or(latest, |was| was.max(latest)));
    }
}

/// The run's last write, with its members' heartbeats counted as writes.
pub(crate) fn last_write_with_beats(paths: &RunPaths, last_write_at: Option<u64>) -> Option<u64> {
    match Liveness::read(paths).latest() {
        Some(latest) => Some(last_write_at.map_or(latest, |was| was.max(latest))),
        None => last_write_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The document entry 141 of `docs/contract-divergences.md` shows is one this
    /// type reads, and writes back as itself: the one place the shape is stated
    /// outside the type is held to it.
    #[test]
    fn the_shape_the_divergence_states_is_the_shape_the_type_reads() {
        let entry = include_str!("../docs/contract-divergences.md")
            .split("## 141.")
            .nth(1)
            .expect("entry 141");
        let block = entry
            .split("```json")
            .nth(1)
            .and_then(|rest| rest.split("```").next())
            .expect("a json block in entry 141");
        let read: Liveness = serde_json::from_str(block).expect("the stated shape reads");
        assert_eq!(read.members.len(), 1);
        assert_eq!(read.members[0].member, "worker");
        let written: serde_json::Value = serde_json::to_value(&read).expect("it serialises");
        let stated: serde_json::Value = serde_json::from_str(block).expect("it parses");
        assert_eq!(written, stated);
        let mut later = stated;
        later["schema_version"] = serde_json::json!(LIVENESS_SCHEMA_VERSION + 1);
        assert!(serde_json::from_value::<Liveness>(later).is_err());
    }
}
