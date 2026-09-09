//! Replaying a journal this build did not write.
//!
//! A run's state is never stored, only derived: every view re-folds the whole
//! journal on each read, so what an operator is shown is a replay. The records
//! folded there arrive from **outside** — a runs root outlives the build that
//! made it, and every build that ever ran on this host writes into the same one
//! — so a record whose clock is spelled another way is a record this build
//! refuses to place in time rather than one it reads.
//!
//! # Why the case is here rather than in `tests/e2e`
//!
//! The same reason `onevcs_seam.rs` gives for its own placement. An e2e here
//! reaches `onepipeline` as a spawned process, and no invocation of that process
//! can write a record it cannot read back: it stamps everything with its own
//! clock, in the one spelling it accepts. Reaching the case through the CLI
//! would mean editing a live run's store underneath it, which is not a replay of
//! anything — it is the suite standing in for a producer.
//!
//! So the journal is treated as what it is, the **input**, and the entry point
//! is the one the commands themselves use: [`Survey::of`] reads a runs root
//! exactly as `onepipeline status` does, and the strings asserted below are the
//! ones `status` and `results` print.

use onepipeline::event::{Envelope, EventKind, Labels, PipelineKind, Source, ENVELOPE_VERSION};
use onepipeline::plan::{Node, Plan, PLAN_SCHEMA_VERSION};
use onepipeline::views::{results, status, Survey};
use serde_json::json;
use std::path::{Path, PathBuf};

/// When the run this file replays was recorded. Fixed, because a replayed
/// journal is a document rather than something happening now.
const RECORDED_AT: &str = "2026-08-18T04:00:00.000Z";

/// When its planner asked the running node to stop, in the spelling every build
/// of this crate writes.
const PARKED_AT: &str = "2026-08-18T04:01:00.000Z";

/// The same instant as another build spells it: RFC 3339 with a numeric UTC
/// offset rather than `Z`. A real timestamp, and one this build refuses —
/// the envelope fixes a single spelling so a stranger's clock cannot become a
/// run's timing evidence.
const PARKED_AT_FOREIGN: &str = "2026-08-18T04:01:00.000+00:00";

/// A runs root of this test's own, emptied first so a rerun reads only what it
/// wrote.
fn scratch(name: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("onepipeline-replay-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("a runs root");
    root
}

/// The one-node plan the replayed run was launched with.
fn plan() -> Plan {
    Plan {
        schema_version: PLAN_SCHEMA_VERSION,
        goal: None,
        name: Some("replay".into()),
        concurrency: 4,
        tasks: vec![Node {
            id: "slow".into(),
            persona: Some("engineer".into()),
            task: Some("## What\nhold the workspace open".into()),
            ..Node::default()
        }],
    }
}

/// One record of this crate's own, as it appears in a journal.
fn recorded(
    kind: PipelineKind,
    at: &str,
    node: Option<&str>,
    payload: serde_json::Value,
) -> Envelope {
    Envelope {
        v: ENVELOPE_VERSION,
        ts: at.to_string(),
        stream: "replayed".into(),
        seq: 0,
        source: Source::Pipeline,
        kind: EventKind(kind.as_str().into()),
        phase: None,
        labels: Labels {
            node: node.map(str::to_string),
            ..Labels::default()
        },
        payload: payload
            .as_object()
            .cloned()
            .expect("a payload is an object"),
        artifacts: Vec::new(),
    }
}

/// A run store left behind under `root`: the launch record a view identifies it
/// by, and the journal it replays.
///
/// The three records are the whole of what a park means — the run's plan, the
/// node's dispatch, and the commit that parked it while it ran — and `parked_at`
/// is the only thing any caller here varies.
fn left_behind(root: &Path, run: &str, parked_at: &str) {
    let dir = root.join(run);
    std::fs::create_dir_all(&dir).expect("a run directory");
    std::fs::write(
        dir.join("launch.json"),
        json!({
            "run_id": run,
            "plan": "plan.json",
            "launcher": "claude-code",
            "session": "session-replay",
            "pid": std::process::id(),
            "host": "replay",
            "started_at": RECORDED_AT,
            "heartbeat_interval": 1_800,
        })
        .to_string(),
    )
    .expect("a launch record");

    let journal = [
        recorded(
            PipelineKind::RunStarted,
            RECORDED_AT,
            None,
            json!({"plan": plan()}),
        ),
        recorded(
            PipelineKind::NodeDispatched,
            RECORDED_AT,
            Some("slow"),
            json!({}),
        ),
        recorded(
            PipelineKind::EditCommitted,
            parked_at,
            None,
            json!({"operations": [{"kind": "node-parked", "node": "slow"}]}),
        ),
    ]
    .iter()
    .map(|event| serde_json::to_string(event).expect("an event serialises"))
    .collect::<Vec<_>>()
    .join("\n");
    std::fs::write(dir.join("events.jsonl"), format!("{journal}\n")).expect("a journal");
}

/// What the two views say about the run replayed out of `root`.
fn views_of(root: &Path) -> (String, String) {
    let survey = Survey::of(root);
    assert!(
        survey.skipped.is_empty(),
        "the replayed run was refused rather than read: {:?}",
        survey.skipped
    );
    let view = survey
        .views
        .first()
        .expect("the replayed run is the one under the root");
    (status(&survey), results(view))
}

/// The control: the same journal with the park stamped the way this build
/// stamps one. Without it the case below would pass on a fixture that never
/// reached the behaviour at all.
#[test]
fn a_park_this_build_can_place_reads_as_a_cancellation_still_converging() {
    let root = scratch("placeable");
    left_behind(&root, "placeable", PARKED_AT);

    let (status, results) = views_of(&root);
    assert!(
        status.contains("slow: cancelling") && status.contains("asked to stop"),
        "a park of a running node was not reported as a stop still converging:\n{status}"
    );
    assert!(
        results.contains("cancelling, asked to stop"),
        "the same fact is missing from the view a planner reads an outcome from:\n{results}"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// And the case: a park nothing can place in time keeps the word every reader
/// already has, rather than a stop pending since an invented moment.
///
/// The wait is rendered as an age — "asked to stop 40s ago" — and the only
/// moment there is to measure it from is the one that commit was written at. A
/// journal whose stamps this build refuses carries no such moment, so there is
/// nothing to age the wait by, and a duration nothing measured is exactly the
/// invented age this readout exists to stop reporting.
#[test]
fn a_park_this_build_cannot_place_reads_as_a_park_rather_than_a_pending_stop() {
    let root = scratch("foreign");
    left_behind(&root, "foreign", PARKED_AT_FOREIGN);

    let (status, results) = views_of(&root);
    assert!(
        !status.contains("cancelling"),
        "a park nothing can place in time is reported as a stop pending since an \
         invented moment:\n{status}"
    );
    // The park itself was never in doubt: what the reader loses is the wait, not
    // the node.
    assert!(
        results.contains("parked"),
        "the node the commit parked is not reported parked:\n{results}"
    );
    assert!(
        !results.contains("cancelling"),
        "the same invented wait reaches the view a planner reads an outcome \
         from:\n{results}"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// When the surface below was handed to a planner, and when it had been queued.
///
/// Two different moments, which is the whole reason the second is recorded: the
/// text was written at the first and read at the second.
const SURFACED_AT: &str = "2026-08-18T04:02:00.000Z";
const SURFACE_QUEUED_AT: u64 = 1_755_489_600_000;

/// A run store whose journal ends with one surface handed to the planner,
/// carrying exactly `payload`.
///
/// The payload is the caller's whole subject here — what a build wrote on that
/// record — so it is stated rather than assembled.
fn a_surface_was_read(root: &Path, run: &str, payload: serde_json::Value) {
    let dir = root.join(run);
    std::fs::create_dir_all(&dir).expect("a run directory");
    std::fs::write(
        dir.join("launch.json"),
        json!({
            "run_id": run,
            "plan": "plan.json",
            "launcher": "claude-code",
            "session": "session-replay",
            "pid": std::process::id(),
            "host": "replay",
            "started_at": RECORDED_AT,
            "heartbeat_interval": 1_800,
        })
        .to_string(),
    )
    .expect("a launch record");

    let journal = [
        recorded(
            PipelineKind::RunStarted,
            RECORDED_AT,
            None,
            json!({"plan": plan()}),
        ),
        recorded(
            PipelineKind::NodeDispatched,
            RECORDED_AT,
            Some("slow"),
            json!({}),
        ),
        recorded(PipelineKind::PlannerSurfaced, SURFACED_AT, None, payload),
    ]
    .iter()
    .map(|event| serde_json::to_string(event).expect("an event serialises"))
    .collect::<Vec<_>>()
    .join("\n");
    std::fs::write(dir.join("events.jsonl"), format!("{journal}\n")).expect("a journal");
}

/// A hand-out recorded before the queued instant existed folds exactly as one
/// recorded after it does.
///
/// The added field is additive and this is what that claim means: the same run,
/// the same records, the same stamps, and the only difference the key a build
/// before the change never wrote. Both journals are replayed through the reader
/// every view is derived from — a record that fell out of the fold, or one
/// placed differently in time, is a different state and shows up here. The
/// comparison is on that state rather than on a rendering because a rendering
/// carries this host's clock and its provider probe, which differ between two
/// reads a moment apart for reasons that have nothing to do with the record.
#[test]
fn a_delivered_surface_recorded_before_the_queued_instant_existed_still_reads() {
    let root = scratch("surfaced");
    let four = json!({
        "kind": "check-in",
        "message": "the workspace is still held",
        "source": "check-in",
        "blocking": false,
    });
    let mut five = four.clone();
    five["queued_at"] = json!(SURFACE_QUEUED_AT);

    // What a reader takes from a hand-out: that one was read, and when it was
    // read — the two the fold derives from this record and nothing else.
    let folded = |root: &Path| -> (u64, Option<u64>) {
        let survey = Survey::of(root);
        assert!(
            survey.skipped.is_empty(),
            "the replayed run was refused rather than read: {:?}",
            survey.skipped
        );
        let view = survey
            .views
            .first()
            .expect("the replayed run is the one under the root");
        (view.state.surfaces_read, view.state.last_surface_at)
    };

    a_surface_was_read(&root, "surfaced", four);
    let before = folded(&root);
    a_surface_was_read(&root, "surfaced", five);
    let after = folded(&root);

    assert_eq!(
        before.0, 1,
        "the hand-out a build before the field wrote was not folded as one read"
    );
    assert!(
        before.1.is_some(),
        "the hand-out a build before the field wrote was not placed in time"
    );
    assert_eq!(
        before, after,
        "a hand-out recorded before the queued instant existed folds differently from \
         one recorded after it"
    );
    std::fs::remove_dir_all(&root).ok();
}
