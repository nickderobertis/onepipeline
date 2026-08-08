//! Session timing and usage, aggregated from the merged event store.
//!
//! The one property that makes this view usable is that **the buckets sum
//! exactly to WALL**. A breakdown whose parts do not add up to the whole cannot
//! answer "where did the time go?", which is the only question it is for — so
//! the residue is a bucket of its own rather than a rounding error hidden in the
//! others.
//!
//! Time is attributed over the run's *wall clock*, not by adding up
//! per-dispatch durations: nodes overlap, so a sum of durations exceeds the
//! elapsed time and the answer stops meaning anything.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::event::{Envelope, Source};
use crate::graph::NodeStatus;
use crate::journal;
use crate::projection;

/// The schema version of the telemetry document.
pub const TELEMETRY_SCHEMA_VERSION: u32 = 1;

/// One run's timing, with a breakdown that sums exactly to its wall clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunTelemetry {
    /// The schema version.
    pub schema_version: u32,
    /// The run.
    pub run_id: String,
    /// The whole elapsed time, in milliseconds.
    pub wall_ms: u64,
    /// The buckets, which sum exactly to [`wall_ms`](Self::wall_ms).
    pub buckets: Vec<Bucket>,
    /// How many dispatches the run started.
    pub dispatches: u64,
    /// How many of them settled `done`.
    pub settled_done: u64,
    /// How many nodes settled without a dispatch because they expected no diff.
    pub no_diff: u64,
    /// How many surfaces were sent, and how many a planner read.
    pub surfaces_queued: u64,
    /// Surfaces a planner actually consumed.
    pub surfaces_read: u64,
}

/// One span of the run's wall clock, named by what the run was doing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bucket {
    /// What the run was doing.
    pub name: String,
    /// For how long, in milliseconds.
    pub ms: u64,
}

/// The bucket for wall time with at least one dispatch in flight.
pub const DISPATCHING: &str = "dispatching";
/// The bucket for wall time waiting on a planner decision.
pub const AWAITING_PLANNER: &str = "awaiting-planner";
/// The bucket for wall time waiting on a person.
pub const AWAITING_HUMAN: &str = "awaiting-human";
/// The bucket for everything else the run's own clock covers — scheduling,
/// round transitions, and the gaps between them.
pub const ORCHESTRATING: &str = "orchestrating";

/// Aggregate one run's telemetry from its merged event store.
pub fn of_run(run: &str, events: &[Envelope]) -> RunTelemetry {
    let state = projection::fold(events);
    let stamps: Vec<(u64, &Envelope)> = events
        .iter()
        .filter_map(|event| projection::millis_of(&event.ts).map(|ms| (ms, event)))
        .collect();

    let first = stamps.first().map_or(0, |(ms, _)| *ms);
    let last = stamps.last().map_or(first, |(ms, _)| *ms);
    let wall_ms = last.saturating_sub(first);

    // Walk the timeline once, attributing each span between consecutive events
    // to whatever the run was doing across it. Every millisecond of the wall
    // clock lands in exactly one bucket, which is what makes the sum exact.
    let mut totals: BTreeMap<&str, u64> = BTreeMap::new();
    let mut in_flight: u64 = 0;
    let mut awaiting_planner = false;
    let mut awaiting_human: u64 = 0;
    let mut previous = first;

    for (ms, event) in &stamps {
        let span = ms.saturating_sub(previous);
        if span > 0 {
            let bucket = if in_flight > 0 {
                DISPATCHING
            } else if awaiting_planner {
                AWAITING_PLANNER
            } else if awaiting_human > 0 {
                AWAITING_HUMAN
            } else {
                ORCHESTRATING
            };
            *totals.entry(bucket).or_insert(0) += span;
        }
        previous = *ms;

        if event.source != Source::Pipeline {
            continue;
        }
        match event.kind.0.as_str() {
            journal::NODE_DISPATCHED => in_flight += 1,
            journal::NODE_SETTLED => {
                let status = event
                    .payload
                    .get("status")
                    .and_then(|v| v.as_str())
                    .and_then(NodeStatus::parse);
                if state
                    .dispatched_at
                    .contains_key(event.labels.node.as_deref().unwrap_or_default())
                {
                    in_flight = in_flight.saturating_sub(1);
                }
                if status == Some(NodeStatus::Waiting) {
                    awaiting_human += 1;
                }
            }
            journal::HUMAN_ATTESTED => awaiting_human = awaiting_human.saturating_sub(1),
            journal::PLANNER_SURFACED => {
                awaiting_planner = event
                    .payload
                    .get("blocking")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
            }
            journal::PLANNER_REPLIED => awaiting_planner = false,
            _ => {}
        }
    }

    let mut buckets: Vec<Bucket> = [DISPATCHING, AWAITING_PLANNER, AWAITING_HUMAN, ORCHESTRATING]
        .into_iter()
        .map(|name| Bucket {
            name: name.to_string(),
            ms: totals.get(name).copied().unwrap_or(0),
        })
        .collect();

    // The invariant, enforced rather than asserted: any residue from a clock
    // that moved backwards between two events lands in `orchestrating` so the
    // parts still add up to the whole.
    let counted: u64 = buckets.iter().map(|bucket| bucket.ms).sum();
    if let Some(residue) = wall_ms.checked_sub(counted) {
        if let Some(bucket) = buckets.iter_mut().find(|b| b.name == ORCHESTRATING) {
            bucket.ms += residue;
        }
    } else if let Some(bucket) = buckets.iter_mut().find(|b| b.name == ORCHESTRATING) {
        bucket.ms = bucket.ms.saturating_sub(counted - wall_ms);
    }

    RunTelemetry {
        schema_version: TELEMETRY_SCHEMA_VERSION,
        run_id: run.to_string(),
        wall_ms,
        buckets,
        dispatches: state.dispatched_at.len() as u64,
        settled_done: state
            .recorded
            .values()
            .filter(|status| **status == NodeStatus::Done)
            .count() as u64,
        no_diff: state
            .outcomes
            .values()
            .filter(|outcome| *outcome == "no-changes")
            .count() as u64,
        surfaces_queued: state.surfaces_queued,
        surfaces_read: state.surfaces_read,
    }
}

/// Render the operator's breakdown.
pub fn render_breakdown(telemetry: &RunTelemetry) -> String {
    let mut out = format!(
        "{}  WALL {}\n",
        telemetry.run_id,
        duration(telemetry.wall_ms)
    );
    for bucket in &telemetry.buckets {
        let share = (bucket.ms * 100)
            .checked_div(telemetry.wall_ms)
            .unwrap_or(0);
        out.push_str(&format!(
            "  {:<18} {:>10}  {share:>3}%\n",
            bucket.name,
            duration(bucket.ms)
        ));
    }
    out.push_str(&format!(
        "  {} dispatch(es), {} done, {} no-diff; {} surface(s) sent, {} read\n",
        telemetry.dispatches,
        telemetry.settled_done,
        telemetry.no_diff,
        telemetry.surfaces_queued,
        telemetry.surfaces_read
    ));
    out
}

/// A duration in milliseconds, rendered for a person.
pub fn duration(ms: u64) -> String {
    let seconds = ms / 1_000;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    if seconds < 3_600 {
        return format!("{}m{:02}s", seconds / 60, seconds % 60);
    }
    format!("{}h{:02}m", seconds / 3_600, (seconds % 3_600) / 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventKind, Labels, ENVELOPE_VERSION};
    use crate::plan::{Node, Plan, PLAN_SCHEMA_VERSION};
    use serde_json::json;

    fn at(
        seconds: u64,
        kind: &str,
        node: Option<&str>,
        fields: &[(&str, serde_json::Value)],
    ) -> Envelope {
        Envelope {
            v: ENVELOPE_VERSION,
            ts: crate::sys::rfc3339_from_millis(1_786_000_000_000 + seconds * 1_000),
            stream: "s".into(),
            seq: seconds,
            source: Source::Pipeline,
            kind: EventKind(kind.into()),
            labels: Labels {
                run_id: Some("demo".into()),
                round: Some(1),
                node: node.map(str::to_string),
                ..Labels::default()
            },
            payload: journal::payload(fields),
            artifacts: Vec::new(),
        }
    }

    fn plan() -> Plan {
        Plan {
            schema_version: PLAN_SCHEMA_VERSION,
            goal: None,
            name: Some("demo".into()),
            concurrency: 4,
            tasks: vec![Node {
                id: "build".into(),
                persona: Some("engineer".into()),
                task: Some("## What\ndo it".into()),
                ..Node::default()
            }],
        }
    }

    #[test]
    fn the_buckets_sum_exactly_to_the_wall_clock() {
        let events = vec![
            at(0, journal::RUN_STARTED, None, &[("plan", json!(plan()))]),
            at(10, journal::NODE_DISPATCHED, Some("build"), &[]),
            at(
                70,
                journal::NODE_SETTLED,
                Some("build"),
                &[("status", json!("done"))],
            ),
            at(100, journal::ROUND_FINISHED, None, &[]),
        ];
        let telemetry = of_run("demo", &events);
        assert_eq!(telemetry.wall_ms, 100_000);
        let summed: u64 = telemetry.buckets.iter().map(|b| b.ms).sum();
        assert_eq!(summed, telemetry.wall_ms, "{:?}", telemetry.buckets);

        let bucket = |name: &str| {
            telemetry
                .buckets
                .iter()
                .find(|b| b.name == name)
                .expect(name)
                .ms
        };
        assert_eq!(bucket(DISPATCHING), 60_000);
        assert_eq!(bucket(ORCHESTRATING), 40_000);
        assert_eq!(telemetry.dispatches, 1);
        assert_eq!(telemetry.settled_done, 1);
    }

    #[test]
    fn waiting_on_a_person_and_on_the_planner_are_different_buckets() {
        let events = vec![
            at(0, journal::RUN_STARTED, None, &[("plan", json!(plan()))]),
            at(
                10,
                journal::NODE_SETTLED,
                Some("approve"),
                &[("status", json!("waiting"))],
            ),
            at(
                40,
                journal::HUMAN_ATTESTED,
                None,
                &[("ref", json!("approve"))],
            ),
            at(
                50,
                journal::PLANNER_SURFACED,
                None,
                &[("blocking", json!(true))],
            ),
            at(90, journal::PLANNER_REPLIED, None, &[]),
        ];
        let telemetry = of_run("demo", &events);
        let bucket = |name: &str| {
            telemetry
                .buckets
                .iter()
                .find(|b| b.name == name)
                .expect(name)
                .ms
        };
        assert_eq!(bucket(AWAITING_HUMAN), 30_000);
        assert_eq!(bucket(AWAITING_PLANNER), 40_000);
        assert_eq!(
            telemetry.buckets.iter().map(|b| b.ms).sum::<u64>(),
            telemetry.wall_ms
        );
    }

    #[test]
    fn a_non_blocking_surface_does_not_park_the_run_on_the_planner() {
        let events = vec![
            at(0, journal::RUN_STARTED, None, &[("plan", json!(plan()))]),
            at(
                10,
                journal::PLANNER_SURFACED,
                None,
                &[("blocking", json!(false))],
            ),
            at(50, journal::ROUND_FINISHED, None, &[]),
        ];
        let telemetry = of_run("demo", &events);
        let awaiting = telemetry
            .buckets
            .iter()
            .find(|b| b.name == AWAITING_PLANNER)
            .expect("the bucket")
            .ms;
        assert_eq!(awaiting, 0, "a heartbeat parked the run");
    }

    #[test]
    fn an_empty_run_has_a_zero_wall_clock_and_still_balances() {
        let telemetry = of_run("demo", &[]);
        assert_eq!(telemetry.wall_ms, 0);
        assert_eq!(telemetry.buckets.iter().map(|b| b.ms).sum::<u64>(), 0);
        assert!(render_breakdown(&telemetry).contains("WALL 0s"));
    }

    #[test]
    fn a_clock_that_moved_backwards_still_leaves_the_buckets_summing_to_wall() {
        let mut events = vec![
            at(0, journal::RUN_STARTED, None, &[("plan", json!(plan()))]),
            at(60, journal::NODE_DISPATCHED, Some("build"), &[]),
            at(30, journal::ROUND_FINISHED, None, &[]),
        ];
        // Deliberately out of order: the wall clock is first-to-last as read.
        events.reverse();
        let telemetry = of_run("demo", &events);
        assert_eq!(
            telemetry.buckets.iter().map(|b| b.ms).sum::<u64>(),
            telemetry.wall_ms,
            "{:?}",
            telemetry.buckets
        );
    }

    #[test]
    fn the_breakdown_names_every_bucket_and_its_share() {
        let events = vec![
            at(0, journal::RUN_STARTED, None, &[("plan", json!(plan()))]),
            at(10, journal::NODE_DISPATCHED, Some("build"), &[]),
            at(
                20,
                journal::NODE_SETTLED,
                Some("build"),
                &[("status", json!("done"))],
            ),
        ];
        let rendered = render_breakdown(&of_run("demo", &events));
        for name in [DISPATCHING, AWAITING_PLANNER, AWAITING_HUMAN, ORCHESTRATING] {
            assert!(rendered.contains(name), "{rendered} omits {name}");
        }
        assert!(rendered.contains('%'), "{rendered}");
    }

    #[test]
    fn a_duration_reads_in_the_units_its_size_calls_for() {
        assert_eq!(duration(0), "0s");
        assert_eq!(duration(45_000), "45s");
        assert_eq!(duration(125_000), "2m05s");
        assert_eq!(duration(7_500_000), "2h05m");
    }
}
