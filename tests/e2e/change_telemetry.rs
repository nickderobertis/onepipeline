//! `telemetry RUN --changes`: each change a run made, read back off the records
//! the run itself kept.
//!
//! Every journey here records its run through the engine's own driver, against a
//! real repository on disk that the linked `onevcs` publishes into — so the
//! `gate-run` and landing records the view aggregates are the ones that library
//! really emits, from a real `pre-push` hook git really ran. Only the agent turn
//! is the double, scripted to take a known time.
//!
//! What each journey asserts the view against is the **store**, read
//! independently: the view is an aggregate, so the only honest oracle is the
//! records it aggregates.
// llmlint: ignore-file[e2e_not_mocked] the sibling under test is *not* substituted here:
// `onevcs` is the library this crate links, driving real git against a real origin and
// a real hook. `oneagentgraph` is still the double, because a real agent turn is a paid
// one and what these journeys measure is the change around it.

use std::path::Path;

use serde_json::{json, Value};

use crate::draft_lifecycle::{rules, settled_run, team_repository, GREEN};
use crate::harness::{agent, lifecycle, plan_of, World};

/// How long each scripted agent turn of the landed journey takes, in ms: apart
/// from the gate's one and two seconds, so the two cannot be mistaken.
const TURN_MS: u64 = 1_500;

/// A lifecycle node into the registered checkout, naming its own title so no
/// drafting dispatch is spent.
fn node(id: &str, repo: &Path) -> Value {
    json!({
        "id": id,
        "repo": repo.to_string_lossy(),
        "persona": "engineer",
        "title": "feat: land the change the worker made",
        "task": "## What\nShip the service.\n\n## Why\nUsers need it.\n\n## Acceptance criteria\n- It is published.",
    })
}

/// A `pre-push` hook that refuses its first push after `refuse` seconds and lets
/// every later one through after `pass` seconds — a gate of two known,
/// distinguishable durations, one failing and one passing.
fn refuses_once(world: &World, refuse: u32, pass: u32) -> Vec<String> {
    let marker = world.root.join("pre-push.refused-once");
    let marker = marker.to_string_lossy().replace('\\', "/");
    vec![
        "sh".into(),
        "-c".into(),
        format!(
            "if [ -f '{marker}' ]; then sleep {pass}; exit 0; fi; touch '{marker}'; \
             sleep {refuse}; exit 1"
        ),
    ]
}

/// Milliseconds since the epoch of one `YYYY-MM-DDThh:mm:ss.sssZ` stamp, the
/// one shape every producer here writes.
fn ms(stamp: &str) -> i64 {
    assert_eq!(stamp.len(), 24, "not a millisecond UTC stamp: {stamp:?}");
    let field = |from: usize, to: usize| -> i64 { stamp[from..to].parse().expect("a digit field") };
    let (year, month, day) = (field(0, 4), field(5, 7), field(8, 10));
    // Howard Hinnant's days-from-civil.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    ((days * 86_400 + field(11, 13) * 3_600 + field(14, 16) * 60 + field(17, 19)) * 1_000)
        + field(20, 23)
}

/// Seconds in the document as whole milliseconds, which is what they are.
fn millis(seconds: &Value) -> i64 {
    let seconds = seconds
        .as_f64()
        .unwrap_or_else(|| panic!("not seconds: {seconds}"));
    (seconds * 1_000.0).round() as i64
}

/// The eight segments, in milliseconds.
fn segments(change: &Value) -> Vec<(&'static str, i64)> {
    [
        "agent",
        "scheduling",
        "gate",
        "publication",
        "review_wait",
        "merge_queue",
        "release_wait",
        "other",
    ]
    .into_iter()
    .map(|name| (name, millis(&change["segments"][name])))
    .collect()
}

fn segment(change: &Value, name: &str) -> i64 {
    millis(&change["segments"][name])
}

fn total(change: &Value) -> i64 {
    segments(change).iter().map(|(_, ms)| ms).sum()
}

/// The events one run recorded under any of `nodes`.
fn under<'a>(journal: &'a [Value], nodes: &[&str]) -> Vec<&'a Value> {
    journal
        .iter()
        .filter(|event| {
            event["labels"]["node"]
                .as_str()
                .is_some_and(|node| nodes.contains(&node))
        })
        .collect()
}

/// Each dispatch's agent interval, read off the store: from a `node-dispatched`
/// to the first `member-settled` of the same node after it.
fn agent_intervals(journal: &[Value], nodes: &[&str]) -> Vec<i64> {
    let records = under(journal, nodes);
    records
        .iter()
        .enumerate()
        .filter(|(_, event)| event["kind"] == "node-dispatched")
        .map(|(at, dispatched)| {
            let settled = records[at..]
                .iter()
                .find(|event| {
                    event["kind"] == "member-settled"
                        && event["labels"]["node"] == dispatched["labels"]["node"]
                })
                .expect("every dispatch's agent settled");
            ms(settled["ts"].as_str().expect("a stamp"))
                - ms(dispatched["ts"].as_str().expect("a stamp"))
        })
        .collect()
}

/// The run's `--changes --json` document, through the binary.
fn changes(world: &World, run: &str) -> Value {
    world
        .run(&["telemetry", run, "--changes", "--json"])
        .exited(0)
        .json()
}

/// A node whose first id failed, retried under a second that a gate refused
/// once and then let land: one change, measured from the first id's dispatch to
/// the landing `onevcs` recorded, every millisecond of it named.
#[test]
fn a_retried_change_is_one_lineage_measured_from_its_first_dispatch_to_its_landing() {
    let world = World::new("changes-landed");
    let hook = refuses_once(&world, 1, 2);
    let hook: Vec<&str> = hook.iter().map(String::as_str).collect();
    let repo = world.repository("local-direct", &hook);
    world.script("service.fail", "1");
    world.script("service-2.work-anew", "the worker wrote this\n");
    world.script("service-2.takes", &TURN_MS.to_string());

    let run = "landed";
    let path = world.plan(run, &plan_of(run, vec![node("service", &repo.checkout)]));
    world.run(&["start", &path, "--attach"]).settled();
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [
                {"op": "retry", "id": "service", "node": node("service-2", &repo.checkout)}
            ]})
            .to_string(),
        )
        .exited(0);
    world
        .run(&["adopt", run])
        .exited(0)
        .out_has("\"settlement\":\"complete\"");

    let document = changes(&world, run);
    assert_eq!(document["schema_version"], 1, "{document}");
    assert_eq!(document["run_id"], run, "{document}");
    let entries = document["changes"].as_array().expect("a list of changes");
    assert_eq!(entries.len(), 1, "a retry chain is one change: {document}");
    let change = &entries[0];
    assert_eq!(change["node"], "service-2", "{change}");
    assert_eq!(
        change["lineage"],
        json!(["service", "service-2"]),
        "{change}"
    );
    assert_eq!(change["repository"], "github.com/owner/service", "{change}");
    assert_eq!(change["outcome"], "merged", "{change}");

    let journal = world.journal(run);
    let lineage = ["service", "service-2"];
    let records = under(&journal, &lineage);

    // The clock starts at the first id's dispatch, not the retry's.
    let dispatched: Vec<&&Value> = records
        .iter()
        .filter(|event| event["kind"] == "node-dispatched")
        .collect();
    assert_eq!(dispatched[0]["labels"]["node"], "service");
    assert_eq!(change["dispatched_at"], dispatched[0]["ts"], "{change}");
    // Three dispatches ran: the first id's, and the retry's two — the second a
    // re-dispatch after the gate refused its push and preserved the branch.
    assert_eq!(dispatched.len(), 3, "{dispatched:?}");
    assert_eq!(change["dispatches"], 3, "{change}");
    // Two publications ran, one per push the retry's dispatches made.
    let pushes = records
        .iter()
        .filter(|event| event["kind"] == "push")
        .count();
    assert_eq!(pushes, 2);
    assert_eq!(change["publication_attempts"], 2, "{change}");

    // The landing is the one `onevcs` recorded, and it is what the base carries.
    let landed: Vec<&&Value> = records
        .iter()
        .filter(|event| event["kind"] == "merge-completed")
        .collect();
    assert_eq!(landed.len(), 1, "{landed:?}");
    assert_eq!(
        change["landed_at"], landed[0]["payload"]["landed_at"],
        "{change}"
    );
    assert_eq!(
        change["landing"], landed[0]["payload"]["landing"],
        "{change}"
    );
    assert!(
        change["landed_at"].is_string() && change["landing"].is_string(),
        "{change}"
    );

    // The gate runs are the `gate-run` records, one failed and one passed.
    let recorded: Vec<Value> = records
        .iter()
        .filter(|event| event["kind"] == "gate-run")
        .map(|event| {
            let payload = &event["payload"];
            json!({
                "gate": payload["gate"], "attempt": payload["attempt"],
                "started_at": payload["started_at"], "ended_at": payload["ended_at"],
                "seconds": payload["seconds"], "verdict": payload["verdict"],
            })
        })
        .collect();
    assert_eq!(change["gate_runs"], json!(recorded), "{change}");
    let verdicts: Vec<&Value> = recorded.iter().map(|run| &run["verdict"]).collect();
    assert_eq!(verdicts, [&json!("failed"), &json!("passed")]);
    let gate_ms: i64 = recorded.iter().map(|run| millis(&run["seconds"])).sum();
    assert_eq!(millis(&change["gate_seconds"]), gate_ms, "{change}");

    // The cycle is the landing minus the first dispatch, and the segments divide
    // it exactly.
    let cycle = ms(change["landed_at"].as_str().expect("landed"))
        - ms(change["dispatched_at"].as_str().expect("dispatched"));
    assert_eq!(millis(&change["cycle_seconds"]), cycle, "{change}");
    assert_eq!(total(change), cycle, "{:?}", segments(change));

    // Each measured interval is in its own segment: the gate reads the gate
    // runs and nothing else, the agent reads its own dispatches' turns.
    assert_eq!(segment(change, "gate"), gate_ms, "{change}");
    assert!(
        gate_ms >= 3_000,
        "the hook slept 1s and then 2s: {gate_ms}ms"
    );
    let agent: i64 = agent_intervals(&journal, &lineage).iter().sum();
    assert_eq!(segment(change, "agent"), agent, "{change}");
    assert!(
        agent >= 2 * TURN_MS as i64,
        "the retry's two turns took {TURN_MS}ms each: {agent}ms"
    );
    assert_ne!(agent, gate_ms, "the two durations were scripted apart");
    // The planner's retry is the wait between the first id settling and the
    // retry's dispatch, and the publication around the gates is its own.
    assert!(segment(change, "scheduling") > 0, "{change}");
    assert!(segment(change, "publication") > 0, "{change}");
    // Every record here decides what the change was doing, so nothing is left
    // over and nothing is unmeasured.
    assert_eq!(segment(change, "other"), 0, "{change}");
    assert_eq!(change["not_measured"], json!([]), "{change}");

    // The text form is one line for the change, saying the same.
    world
        .run(&["telemetry", run, "--changes"])
        .exited(0)
        .out_has("service-2  github.com/owner/service  merged  cycle ")
        .out_has("dispatches 3  publications 2  gates 2 (");
}

/// A change its gate refused at every attempt never landed: no landing, no
/// cycle, and its segments divide the time from its dispatch to the run's last
/// record — with the agent's and the gate's time each in its own.
#[test]
fn a_change_that_never_landed_is_measured_to_the_runs_last_record() {
    let world = World::new("changes-unlanded");
    let repo = world.repository("local-direct", &["sh", "-c", "sleep 1; exit 1"]);
    world.script("service.work-anew", "the worker wrote this\n");
    world.script("service.takes", "500");

    let run = "refused";
    let path = world.plan(run, &plan_of(run, vec![node("service", &repo.checkout)]));
    world.run(&["start", &path, "--attach"]).settled();
    let result = world.run_json(run, "result.json");
    assert_eq!(result["nodes"][0]["outcome"], "push-rejected", "{result}");

    let document = changes(&world, run);
    let change = &document["changes"][0];
    assert_eq!(change["node"], "service", "{change}");
    assert_eq!(change["outcome"], "push-rejected", "{change}");
    assert_eq!(change["landed_at"], Value::Null, "{change}");
    assert_eq!(change["landing"], Value::Null, "{change}");
    assert_eq!(change["cycle_seconds"], Value::Null, "{change}");

    let journal = world.journal(run);
    let last = journal
        .iter()
        .filter_map(|event| event["ts"].as_str().map(ms))
        .max()
        .expect("the run recorded something");
    let open = last - ms(change["dispatched_at"].as_str().expect("dispatched"));
    assert!(open > 0, "the journey's interval is empty");
    assert_eq!(total(change), open, "{:?}", segments(change));

    let gate_ms: i64 = change["gate_runs"]
        .as_array()
        .expect("gate runs")
        .iter()
        .map(|run| millis(&run["seconds"]))
        .sum();
    assert!(gate_ms >= 1_000, "{change}");
    assert_eq!(segment(change, "gate"), gate_ms, "{change}");
    let agent: i64 = agent_intervals(&journal, &["service"]).iter().sum();
    assert!(agent >= 500, "{change}");
    assert_eq!(segment(change, "agent"), agent, "{change}");
    assert_eq!(change["not_measured"], json!([]), "{change}");
}

/// A run whose records carry no `gate-run` — what a store an older `onevcs`
/// wrote reads as, and what a repository with no hook writes — is read all the
/// same: no gate runs, and the gate named as not measured. The publication the
/// gate would have run inside cannot be told apart from it either, so that time
/// is counted in `other` and the publication is named beside it.
#[test]
fn a_run_with_no_gate_run_reads_its_gate_as_not_measured_and_counts_it_in_other() {
    let world = World::new("changes-ungated");
    let repo = world.repository("local-direct", &[]);
    world.script("service.work", "the worker wrote this\n");

    let run = "ungated";
    let path = world.plan(run, &plan_of(run, vec![node("service", &repo.checkout)]));
    world.run(&["start", &path, "--attach"]).settled();
    assert!(world.events_of(run, "gate-run").is_empty());

    let document = changes(&world, run);
    let change = &document["changes"][0];
    assert_eq!(change["outcome"], "merged", "{change}");
    assert_eq!(change["gate_runs"], json!([]), "{change}");
    assert_eq!(change["gate_seconds"], json!(0.0), "{change}");
    assert_eq!(
        change["not_measured"],
        json!(["gate", "publication"]),
        "{change}"
    );
    assert_eq!(segment(change, "gate"), 0, "{change}");
    assert_eq!(segment(change, "publication"), 0, "{change}");

    // What `other` holds is exactly the closeout — from the agent settling to
    // the landing, the span a gate could have run inside — less the wait for the
    // landing's turn, which `merge-queued` records apart.
    let journal = world.journal(run);
    let settled = world.events_of(run, "member-settled");
    let queued = world.events_of(run, "merge-queued");
    let landed_at = ms(change["landed_at"].as_str().expect("landed"));
    let closeout = landed_at - ms(settled[0]["ts"].as_str().expect("a stamp"));
    let queue = landed_at - ms(queued[0]["ts"].as_str().expect("a stamp"));
    assert!(closeout > queue, "{change}");
    assert_eq!(segment(change, "merge_queue"), queue, "{change}");
    assert_eq!(segment(change, "other"), closeout - queue, "{change}");
    let agent: i64 = agent_intervals(&journal, &["service"]).iter().sum();
    assert_eq!(segment(change, "agent"), agent, "{change}");
    assert_eq!(
        total(change),
        millis(&change["cycle_seconds"]),
        "{:?}",
        segments(change)
    );

    world
        .run(&["telemetry", run, "--changes"])
        .exited(0)
        .out_has("gate not measured  publication not measured");
}

/// The run's last record, in milliseconds.
fn last_record(journal: &[Value]) -> i64 {
    journal
        .iter()
        .filter_map(|event| event["ts"].as_str().map(ms))
        .max()
        .expect("the run recorded something")
}

/// The one record of `kind` under `node`.
fn only<'a>(journal: &'a [Value], node: &str, kind: &str) -> &'a Value {
    let found: Vec<&Value> = under(journal, &[node])
        .into_iter()
        .filter(|event| event["kind"] == kind)
        .collect();
    assert_eq!(found.len(), 1, "{kind} under {node}: {found:?}");
    found[0]
}

/// A green change a team repository keeps as a draft for its user's review is
/// waiting on that person from the moment it is kept, its required checks are a
/// gate run of their own, and the node that depended on it is a change of its
/// own — a direct one, listed after it because it was dispatched after it.
#[test]
fn a_change_kept_for_review_waits_on_review_and_its_dependent_is_its_own_change() {
    let world = World::new("changes-review");
    team_repository(&world);
    world.script("service.work", "the worker wrote this\n");
    world.script("gh.checks", GREEN);

    let run = "kept";
    settled_run(
        &world,
        run,
        vec![lifecycle("service", &[]), agent("after", &["service"])],
        &[],
    )
    .settled();

    let document = changes(&world, run);
    let entries = document["changes"].as_array().expect("a list of changes");
    let nodes: Vec<&Value> = entries.iter().map(|change| &change["node"]).collect();
    assert_eq!(nodes, [&json!("service"), &json!("after")], "{document}");
    let (service, after) = (&entries[0], &entries[1]);
    assert!(
        ms(service["dispatched_at"].as_str().expect("a stamp"))
            < ms(after["dispatched_at"].as_str().expect("a stamp")),
        "{document}"
    );

    let journal = world.journal(run);
    assert_eq!(service["outcome"], "change-review-draft", "{service}");
    assert_eq!(
        service["change_url"], "https://github.com/owner/service/pull/1",
        "{service}"
    );
    assert_eq!(
        service["branch"],
        only(&journal, "service", "node-settled")["payload"]["branch"],
        "{service}"
    );
    assert_eq!(service["landed_at"], Value::Null, "{service}");
    assert_eq!(service["publication_attempts"], 1, "{service}");
    // The required checks are a gate run of their own, read off the record.
    let checks = only(&journal, "service", "gate-run");
    assert_eq!(checks["payload"]["gate"], "required-checks");
    assert_eq!(
        service["gate_runs"][0]["gate"], "required-checks",
        "{service}"
    );
    assert_eq!(service["gate_runs"][0]["verdict"], "passed", "{service}");
    assert_eq!(
        segment(service, "gate"),
        millis(&checks["payload"]["seconds"]),
        "{service}"
    );
    // Waiting on review from the moment its checks settled — which is what
    // keeping it for review says — until the run's last record, and nothing
    // left undecided.
    assert_eq!(
        only(&journal, "service", "draft-kept-for-review")["phase"],
        "review"
    );
    let settled = ms(only(&journal, "service", "checks-settled")["ts"]
        .as_str()
        .expect("a stamp"));
    let ended = ms(service["gate_runs"][0]["ended_at"]
        .as_str()
        .expect("a stamp"));
    let last = last_record(&journal);
    assert_eq!(
        segment(service, "review_wait"),
        last - settled.max(ended),
        "{service}"
    );
    assert_eq!(segment(service, "other"), 0, "{service}");
    assert_eq!(service["not_measured"], json!([]), "{service}");
    assert_eq!(
        total(service),
        last - ms(service["dispatched_at"].as_str().expect("a stamp")),
        "{:?}",
        segments(service)
    );

    // The dependent never opened a session: a direct change, never published.
    assert_eq!(after["repository"], Value::Null, "{after}");
    assert_eq!(after["lineage"], json!(["after"]), "{after}");
    assert_eq!(after["dispatches"], 1, "{after}");
    assert_eq!(after["publication_attempts"], 0, "{after}");
}

/// A draft the plan asked for is held for a person to mark ready, so its wait
/// from its settling on is a review wait. A held draft watches no checks, so
/// this run recorded no gate run at all: its publication, which a gate could
/// have run inside, is counted in `other` beside the unmeasured gate.
#[test]
fn a_draft_the_plan_held_waits_on_review() {
    let world = World::new("changes-held");
    world.repository("change-auto", &[]);
    world.script("service.work", "the worker wrote this\n");
    world.script("gh.checks", GREEN);
    let mut node = lifecycle("service", &[]);
    node["draft"] = json!(true);

    let run = "held";
    settled_run(&world, run, vec![node], &[]).settled();

    let change = &changes(&world, run)["changes"][0];
    assert_eq!(change["outcome"], "change-draft", "{change}");
    let journal = world.journal(run);
    assert_eq!(
        only(&journal, "service", "change-drafted")["payload"]["kind"],
        "held"
    );
    let settled = ms(only(&journal, "service", "node-settled")["ts"]
        .as_str()
        .expect("a stamp"));
    let last = last_record(&journal);
    assert!(last > settled, "{change}");
    assert_eq!(segment(change, "review_wait"), last - settled, "{change}");
    assert_eq!(segment(change, "release_wait"), 0, "{change}");
    assert_eq!(change["gate_runs"], json!([]), "{change}");
    assert_eq!(
        change["not_measured"],
        json!(["gate", "publication"]),
        "{change}"
    );
    assert!(segment(change, "other") > 0, "{change}");
    assert_eq!(
        total(change),
        last_record(&journal) - ms(change["dispatched_at"].as_str().expect("a stamp")),
        "{:?}",
        segments(change)
    );
}

/// An open change request whose checks settled and which has not merged is
/// waiting on a reviewer or on its host, and nothing it recorded says which: that
/// time is `other`, and `review_wait` is named as not measured rather than
/// guessed at. Its checks settled with one skipped, which the gate run says.
#[test]
fn an_open_change_awaiting_its_merge_names_review_wait_as_not_measured() {
    let world = World::new("changes-open");
    world.repository("change-open", &[]);
    rules(&world, "change-open", "none", Some("{disabled: true}"));
    world.script("service.work", "the worker wrote this\n");
    world.script(
        "gh.checks",
        "lint completed skipped required\ntest completed success required",
    );

    let run = "open";
    settled_run(&world, run, vec![lifecycle("service", &[])], &[]).settled();

    let change = &changes(&world, run)["changes"][0];
    assert_eq!(change["outcome"], "change-open", "{change}");
    assert_eq!(change["cycle_seconds"], Value::Null, "{change}");
    assert_eq!(
        change["gate_runs"][0]["verdict"], "passed-with-skipped",
        "{change}"
    );
    assert_eq!(change["not_measured"], json!(["review_wait"]), "{change}");
    assert_eq!(segment(change, "review_wait"), 0, "{change}");

    // `other` is the wait from the checks settling — or the gate run ending, if
    // later — to the run's last record.
    let journal = world.journal(run);
    let settled = ms(only(&journal, "service", "checks-settled")["ts"]
        .as_str()
        .expect("a stamp"));
    let ended = ms(change["gate_runs"][0]["ended_at"]
        .as_str()
        .expect("a stamp"));
    let last = last_record(&journal);
    assert!(last > settled.max(ended), "{change}");
    assert_eq!(
        segment(change, "other"),
        last - settled.max(ended),
        "{change}"
    );
    assert_eq!(
        total(change),
        last - ms(change["dispatched_at"].as_str().expect("a stamp")),
        "{:?}",
        segments(change)
    );
    world
        .run(&["telemetry", run, "--changes"])
        .exited(0)
        .out_has("review_wait not measured");
}

/// `--changes` is about one run, `--json` is a form of `--changes`, and the
/// per-change view is not the run breakdown.
#[test]
fn changes_names_one_run_and_json_needs_changes() {
    let world = World::new("changes-usage");
    world
        .run(&["telemetry", "a-run", "--changes", "--breakdown"])
        .exited(2)
        .err_has("cannot be used with");
    world
        .run(&["telemetry", "--changes"])
        .exited(2)
        .err_has("<RUN>");
    world
        .run(&["telemetry", "a-run", "--json"])
        .exited(2)
        .err_has("--changes");
    world
        .run(&["telemetry", "nowhere", "--changes"])
        .exited(2)
        .err_has("nowhere");
}
