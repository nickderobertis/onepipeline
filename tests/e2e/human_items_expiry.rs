//! Expiry of human items, end to end: the compiled binary and the SDK over a real runs
//! root, its real host journal, check record and run journals, with real runs of
//! `kind: command` and `kind: human` nodes driven by the real reconcile loop.
//!
//! Each journey declares when an item stops mattering, makes the condition hold for
//! real — a moment passing, a node or a run settling, a file appearing for a check to
//! find — and reads what the engine did off the journal, the CLI and the run.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes the `oneagentgraph` sibling at its
// subprocess boundary and nothing inside the crate under test, which is driven as the real
// compiled binary over real run, plan and human-item stores; `harness.rs` carries the full
// rationale. Every check is a real program the binary spawns, and the projection journey's
// tracker is the GitHub loopback `private_github_home.rs` documents.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};

use crate::harness::{human, plan_of, posix_script, shell_path, World};

/// A moment every host has passed, and one none will reach.
const PAST: &str = "2020-01-02T03:04:05Z";
const FUTURE: &str = "2099-01-02T03:04:05Z";
/// The owning session of every item a journey raises directly.
const OWNER: &str = "mgr";

/// Human items enabled for this world, through its host-local configuration.
fn enabled(world: &World) {
    let dir = world.root.join("xdg/onepipeline");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.yaml"), "human_items:\n  enabled: true\n").unwrap();
}

/// A world with human items enabled.
fn world(name: &str) -> World {
    let world = World::new(name);
    enabled(&world);
    world
}

/// A raise input for `dedup`, a question owned by [`OWNER`] holding nothing.
fn item(dedup: &str, expiry: Value) -> Value {
    json!({
        "schema_version": 1,
        "dedup_key": dedup,
        "kind": "question",
        "title": format!("Is {dedup} still needed?"),
        "body": format!("What {dedup} needs from a person."),
        "priority": "medium",
        "owner": {"session": OWNER, "run": null, "plan": null},
        "block": {"scope": "nothing"},
        "approval": null,
        "expiry": expiry,
    })
}

/// Raise `value` through `raise --input`; answers the receipt's item.
fn raise(world: &World, value: &Value) -> Value {
    let name = value["dedup_key"]
        .as_str()
        .unwrap()
        .replace(['/', ':'], "-");
    let path = world.root.join(format!("raise-{name}.json"));
    std::fs::write(&path, value.to_string()).unwrap();
    let run = world.run(&["human-items", "raise", "--input", path.to_str().unwrap()]);
    run.exited(0);
    serde_json::from_str::<Value>(&run.stdout).unwrap()["item"]["item"].clone()
}

fn id_of(item: &Value) -> String {
    item["id"].as_str().unwrap().to_owned()
}

/// `human-items list --json` with `args`.
fn listed(world: &World, args: &[&str]) -> Vec<Value> {
    let mut all = vec!["human-items", "list", "--json"];
    all.extend_from_slice(args);
    let run = world.run(&all);
    run.exited(0);
    serde_json::from_str::<Value>(&run.stdout)
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

/// `human-items show ID --json`.
fn shown(world: &World, id: &str) -> Value {
    let run = world.run(&["human-items", "show", id, "--json"]);
    run.exited(0);
    serde_json::from_str(&run.stdout).unwrap()
}

/// Every record of the host journal, read off the file rather than through any read
/// that would sweep.
fn journal(world: &World) -> Vec<Value> {
    std::fs::read_to_string(world.runs.join("human-items/events.ndjson"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// The `human-item-expired` records the journal holds for `id`.
fn expiries(world: &World, id: &str) -> Vec<Value> {
    journal(world)
        .into_iter()
        .filter(|e| e["kind"] == "human-item-expired" && e["payload"]["item_id"] == id)
        .collect()
}

/// Hold `item`, as one read answered it, to being expired by `condition` for `reason`.
fn expired_by(item: &Value, condition: &Value, reason: &str) {
    assert_eq!(item["state"], "expired", "{item}");
    let resolution = &item["resolution"];
    assert_eq!(resolution["actor"], "engine", "{item}");
    assert_eq!(resolution["on_behalf_of"], Value::Null);
    assert_eq!(resolution["text"], Value::Null);
    assert!(resolution.get("choice").is_none(), "{item}");
    assert_eq!(&resolution["condition"], condition, "{item}");
    assert_eq!(resolution["reason"], reason, "{item}");
}

/// A fresh store holding one open item whose time condition already holds, and that
/// nothing has read: what each entry point's own journey reads once.
fn one_due_item(name: &str) -> (World, String, Value) {
    let world = world(name);
    let condition = json!({"when": "time", "at": PAST, "reason": "Past its moment"});
    let raised = raise(&world, &item("due", json!([condition])));
    assert_eq!(raised["state"], "open", "a raise does not sweep");
    let id = id_of(&raised);
    assert!(expiries(&world, &id).is_empty());
    (world, id, condition)
}

/// The one read made, the journal holds exactly the expiry it wrote.
fn written_by_that_read(world: &World, id: &str, condition: &Value) {
    let written = expiries(world, id);
    assert_eq!(written.len(), 1, "{written:?}");
    expired_by(&written[0]["payload"]["item"], condition, "Past its moment");
}

#[test]
fn a_list_sweeps_before_it_answers() {
    let (world, id, condition) = one_due_item("expiry-list");
    let read = listed(&world, &["--state", "expired"]);
    assert_eq!(read.len(), 1, "{read:?}");
    assert_eq!(read[0]["item"]["id"], id);
    expired_by(&read[0]["item"], &condition, "Past its moment");
    written_by_that_read(&world, &id, &condition);
}

#[test]
fn a_report_sweeps_before_it_answers() {
    let (world, id, condition) = one_due_item("expiry-report");
    let report = world.run(&["human-items", "report", "--all", "--state", "expired"]);
    report
        .exited(0)
        .out_has(&format!("({id})"))
        .out_has("· expired")
        .out_has("expired: Past its moment");
    written_by_that_read(&world, &id, &condition);
}

#[test]
fn a_show_sweeps_before_it_answers() {
    let (world, id, condition) = one_due_item("expiry-show");
    let detail = shown(&world, &id);
    expired_by(&detail["item"], &condition, "Past its moment");
    assert_eq!(
        detail["history"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["action"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["human-item-raised", "human-item-expired"]
    );
    written_by_that_read(&world, &id, &condition);
}

#[test]
fn a_watch_sweeps_and_delivers_the_expiry_to_its_owner() {
    let (world, id, condition) = one_due_item("expiry-watch");
    let watch = world.run(&["human-items", "watch", "--session", OWNER]);
    watch.exited(0);
    let printed: Vec<Value> = watch
        .stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let kinds: Vec<&str> = printed
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        ["human-item-raised", "human-item-expired"],
        "{printed:?}"
    );
    // Delivered as a dismissal is: the whole event, its item and resolution with it.
    let expired = &printed[1]["payload"];
    assert_eq!(expired["item_id"], id);
    expired_by(&expired["item"], &condition, "Past its moment");
    assert_eq!(expired["resolution"], expired["item"]["resolution"]);
    written_by_that_read(&world, &id, &condition);
}

/// The store an embedding consumer opens, as the page does, over `world`'s runs root.
fn sdk(world: &World) -> onepipeline::human_items::HumanItems {
    let repo = world.root.join("sdk-checkout");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(repo.join(".onepipeline")).unwrap();
    std::fs::write(
        repo.join(".onepipeline/config.yaml"),
        "human_items:\n  enabled: true\n",
    )
    .unwrap();
    onepipeline::human_items::HumanItems::open(world.runs.clone(), &repo).unwrap()
}

#[test]
fn the_sdks_query_sweeps_before_it_answers() {
    let (world, id, condition) = one_due_item("expiry-sdk-query");
    let read = sdk(&world)
        .query(&onepipeline::human_items::HumanItemFilter {
            state: Some(onepipeline::human_items::HumanItemState::Expired),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(read.len(), 1);
    expired_by(
        &serde_json::to_value(&read[0].item).unwrap(),
        &condition,
        "Past its moment",
    );
    assert!(!read[0].item.is_blocking());
    written_by_that_read(&world, &id, &condition);
}

#[test]
fn the_sdks_show_sweeps_before_it_answers() {
    let (world, id, condition) = one_due_item("expiry-sdk-show");
    let detail = sdk(&world).show(&id).unwrap();
    expired_by(
        &serde_json::to_value(&detail.item).unwrap(),
        &condition,
        "Past its moment",
    );
    written_by_that_read(&world, &id, &condition);
}

/// The time journey over the CLI: a time past expires on the next read and one ahead
/// does not; an `after_seconds` is stored as its resolved moment and expires once that
/// passes; a condition's own reason is recorded verbatim and the engine names one that
/// gives none; raising an open item again replaces its conditions; `list --state
/// expired` and `show` say all of it; and a sample bundle reads expiry as data and
/// never sweeps.
#[test]
fn time_conditions_expire_on_the_next_read_with_their_reason_or_the_engines_words() {
    let world = world("expiry-time");
    let past = json!({"when": "time", "at": PAST, "reason": "The release window has passed"});
    let gone = id_of(&raise(&world, &item("gone", json!([past]))));
    let ahead = id_of(&raise(
        &world,
        &item("ahead", json!([{"when": "time", "at": FUTURE}])),
    ));
    let before = crate::harness::now_millis();
    let soon = raise(
        &world,
        &item("soon", json!([{"when": "time", "after_seconds": 5}])),
    );
    let after = crate::harness::now_millis();
    let soon_id = id_of(&soon);
    // Stored as the moment it resolved to, five seconds after the raise, and open on a
    // read before it.
    let at = soon["expiry"][0]["at"].as_str().unwrap().to_owned();
    assert!(soon["expiry"][0].get("after_seconds").is_none(), "{soon}");
    let resolved = crate::harness::epoch_millis(&at);
    assert!(
        (before + 5000..=after + 5000).contains(&resolved),
        "{at} is not 5 s after the raise"
    );
    assert_eq!(
        shown(&world, &soon_id)["item"]["state"],
        "open",
        "read before its moment"
    );
    // Re-armed: raised again with different conditions, which replace the old ones.
    let rearmed = id_of(&raise(
        &world,
        &item(
            "rearmed",
            json!([{"when": "time", "at": FUTURE, "reason": "old"}]),
        ),
    ));
    let replacement = json!({"when": "time", "at": PAST, "reason": "new"});
    let again = raise(&world, &item("rearmed", json!([replacement])));
    assert_eq!(
        again["expiry"],
        json!([replacement]),
        "the conditions were replaced"
    );

    let report = world.run(&["human-items", "report", "--all"]);
    report.exited(0);
    let read = |id: &str| shown(&world, id)["item"].clone();
    expired_by(&read(&gone), &past, "The release window has passed");
    assert_eq!(read(&ahead)["state"], "open");
    expired_by(&read(&rearmed), &replacement, "new");

    world.until_within(Duration::from_secs(30), "the moment to pass", |_| {
        crate::harness::now_millis() > resolved
    });
    let stored = json!({"when": "time", "at": at});
    expired_by(
        &read(&soon_id),
        &stored,
        &format!("its expiry time {at} was reached"),
    );

    let expired: Vec<String> = listed(&world, &["--state", "expired"])
        .iter()
        .map(|v| v["item"]["id"].as_str().unwrap().to_owned())
        .collect();
    let mut wanted = vec![gone.clone(), soon_id.clone(), rearmed.clone()];
    wanted.sort();
    let mut expired_sorted = expired.clone();
    expired_sorted.sort();
    assert_eq!(
        expired_sorted, wanted,
        "every expired item and nothing open"
    );
    let open: Vec<String> = listed(&world, &[])
        .iter()
        .map(|v| v["item"]["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(open, [ahead.as_str()]);

    world
        .run(&["human-items", "show", &ahead])
        .exited(0)
        .out_has("expires:")
        .out_has(&format!("[0] at {FUTURE}"));
    world
        .run(&["human-items", "show", &gone])
        .exited(0)
        .out_has(&format!("[0] at {PAST} — The release window has passed"))
        .out_has(&format!("expired at {PAST}: The release window has passed"));

    // A sample bundle carries expiry, an expired item and its checks as data, and a
    // sample read never sweeps: its open item whose time has passed stays open.
    let sample = sample_with_expiry(&world);
    let read = world.run(&[
        "human-items",
        "--sample",
        sample.to_str().unwrap(),
        "list",
        "--json",
    ]);
    read.exited(0);
    let open: Value = serde_json::from_str(&read.stdout).unwrap();
    assert_eq!(open.as_array().unwrap().len(), 1, "{open}");
    assert_eq!(open[0]["item"]["state"], "open");
    assert_eq!(open[0]["item"]["expiry"][0]["at"], PAST);
    let bundle: Value = serde_json::from_str(&std::fs::read_to_string(&sample).unwrap()).unwrap();
    let expired = world.run(&[
        "human-items",
        "--sample",
        sample.to_str().unwrap(),
        "show",
        "sample-expired",
        "--json",
    ]);
    expired.exited(0);
    let detail: Value = serde_json::from_str(&expired.stdout).unwrap();
    assert_eq!(detail, bundle["items"][1], "read back as given");
    assert_eq!(
        std::fs::read_to_string(&sample).unwrap(),
        serde_json::to_string_pretty(&bundle).unwrap(),
        "a sample read writes nothing"
    );
    // A sample raise stores what a live one does: a moment resolved and a cwd defaulted.
    let input = world.root.join("sample-raise.json");
    std::fs::write(
        &input,
        item(
            "sample-raised",
            json!([{"when": "time", "after_seconds": 60}, {"when": "check", "command": ["true"]}]),
        )
        .to_string(),
    )
    .unwrap();
    let path = sample.to_str().unwrap();
    let raised = world.run(&[
        "human-items",
        "--sample",
        path,
        "raise",
        "--input",
        input.to_str().unwrap(),
    ]);
    raised.exited(0);
    let stored =
        serde_json::from_str::<Value>(&raised.stdout).unwrap()["item"]["item"]["expiry"].clone();
    assert!(
        stored[0]["at"].is_string() && stored[0].get("after_seconds").is_none(),
        "{stored}"
    );
    assert!(
        std::path::Path::new(stored[1]["cwd"].as_str().unwrap()).is_absolute(),
        "{stored}"
    );
    // Reopening the expired sample item takes its conditions and their checks with it.
    let reopened = world.run(&["human-items", "--sample", path, "reopen", "sample-expired"]);
    reopened.exited(0);
    let view = &serde_json::from_str::<Value>(&reopened.stdout).unwrap()["item"];
    assert_eq!(view["item"]["state"], "open");
    assert!(
        view["item"].get("expiry").is_none() && view.get("checks").is_none(),
        "{view}"
    );
}

/// A sample bundle holding one open item whose time has passed and one expired item
/// with a check that ran.
fn sample_with_expiry(world: &World) -> PathBuf {
    let check = json!({"when": "check", "command": ["/bin/df-check"], "cwd": "/srv"});
    let base = |id: &str, state: &str| {
        json!({
            "schema_version": 1, "id": id, "dedup_key": id, "kind": "question",
            "title": format!("Is {id} still needed?"), "body": "Say so.", "priority": "low",
            "owner": {"session": OWNER, "run": null, "plan": null},
            "block": {"scope": "nothing"}, "approval": null, "state": state,
            "resolution": null, "tracker": null,
            "created_at": "sample", "updated_at": "sample"
        })
    };
    let mut open = base("sample-open", "open");
    open["expiry"] = json!([{"when": "time", "at": PAST}]);
    let mut expired = base("sample-expired", "expired");
    expired["expiry"] = json!([check]);
    expired["resolution"] = json!({"actor": "engine", "on_behalf_of": null, "text": null,
        "reason": "The disk has space again", "condition": check});
    let detail = |item: Value, checks: Value, reopen: Value| {
        let mut detail = json!({"item": item, "blocked": {"total": 0, "plans": []},
            "nodes": [], "reopen": reopen});
        if checks != Value::Null {
            detail["checks"] = checks;
        }
        detail
    };
    let bundle = json!({
        "schema_version": 1,
        "items": [
            detail(open, Value::Null, json!({"available": false,
                "reason": "the item is open, so there is nothing to reopen"})),
            detail(expired, json!([{"condition": 0, "last_run_at": "2026-11-20T09:05:00.000Z",
                "outcome": "holds", "detail": null}]), json!({"available": true, "reason": null})),
        ],
        "approval_keys": {}
    });
    let path = world.root.join("expiry-sample.json");
    std::fs::write(&path, serde_json::to_string_pretty(&bundle).unwrap()).unwrap();
    path
}

/// A command node that waits for `go` under the world's root, then exits `code`.
fn waiting(world: &World, id: &str, go: &str, code: i32) -> Value {
    let flag = shell_path(&world.root.join(go));
    let script = posix_script(
        &world.root,
        &format!("node-{id}"),
        &format!("while [ ! -f '{flag}' ]; do sleep 0.05; done\nexit {code}"),
    );
    json!({"id": id, "kind": "command", "command": [script.display().to_string()], "deps": []})
}

/// Let the node waiting for `go` finish.
fn release(world: &World, go: &str) {
    std::fs::write(world.root.join(go), "go").unwrap();
}

/// The status `run`'s journal last settled `node` at.
fn settled(world: &World, run: &str, node: &str) -> Option<String> {
    world
        .events_of(run, "node-settled")
        .iter()
        .rev()
        .find(|e| e["labels"]["node"] == node)
        .map(|e| {
            e["payload"]["status"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
}

/// Whether `run`'s command `node` has started.
fn started(world: &World, run: &str, node: &str) -> bool {
    world
        .events_of(run, "command-started")
        .iter()
        .any(|e| e["labels"]["node"] == node)
}

/// Run and node conditions over real runs of `kind: command` nodes. A node condition
/// naming another run's node expires once that node settles and not before, written by
/// that run's live driver's pass with no CLI read in between; a run that has settled
/// expires `on: settled` with the condition's own reason, a run that ended failed
/// expires `on: ended` and leaves `on: settled` open.
#[test]
fn node_and_run_conditions_follow_real_runs_of_command_nodes() {
    let world = world("expiry-runs");
    let (x, y) = ("settling", "failing");
    let settling = world.plan(
        x,
        &plan_of(
            x,
            vec![
                waiting(&world, "a", "go-a", 0),
                waiting(&world, "b", "go-b", 0),
            ],
        ),
    );
    let failing = world.plan(y, &plan_of(y, vec![waiting(&world, "c", "go-c", 1)]));
    let w = "stopped";
    let stopping = world.plan(w, &plan_of(w, vec![waiting(&world, "d", "go-d", 0)]));
    world.run(&["start", &settling, "--detach"]).exited(0);
    world.run(&["start", &failing, "--detach"]).exited(0);
    world.run(&["start", &stopping, "--detach"]).exited(0);
    world.until("every command to start", |world| {
        started(world, x, "a")
            && started(world, x, "b")
            && started(world, y, "c")
            && started(world, w, "d")
    });
    let owned_elsewhere = |dedup: &str, condition: &Value| {
        let mut value = item(dedup, json!([condition]));
        value["owner"]["run"] = json!("the-asking-run");
        raise(&world, &value)
    };
    let node = json!({"when": "node", "run": x, "node": "a"});
    let all_done =
        json!({"when": "run", "run": x, "on": "settled", "reason": "Every step is done"});
    let ended = json!({"when": "run", "run": y, "on": "ended"});
    let never_settles = json!({"when": "run", "run": y, "on": "settled"});
    let n = id_of(&owned_elsewhere("node-a", &node));
    let s = id_of(&owned_elsewhere("all-done", &all_done));
    let e = id_of(&owned_elsewhere("y-ended", &ended));
    let f = id_of(&owned_elsewhere("y-settled", &never_settles));
    let failed_node = json!({"when": "node", "run": y, "node": "c"});
    let c = id_of(&owned_elsewhere("node-c", &failed_node));
    let halted = json!({"when": "run", "run": w, "on": "ended"});
    let h = id_of(&owned_elsewhere("w-ended", &halted));
    // Nothing holds yet: every run is live and every node unsettled.
    assert_eq!(listed(&world, &[]).len(), 6);
    assert!(journal(&world)
        .iter()
        .all(|e| e["kind"] != "human-item-expired"));

    // Its node settles: the run's own driver sweeps and expires it, with no read here.
    release(&world, "go-a");
    world.until("the driver to expire the node's item", |w| {
        !expiries(w, &n).is_empty()
    });
    assert!(
        started(&world, x, "b") && settled(&world, x, "b").is_none(),
        "x is live"
    );
    assert!(
        expiries(&world, &s).is_empty(),
        "x has a node still unsettled"
    );
    expired_by(
        &expiries(&world, &n)[0]["payload"]["item"],
        &node,
        &format!("node a of run {x} settled done"),
    );
    assert_eq!(shown(&world, &s)["item"]["state"], "open");

    // The whole run settles done.
    release(&world, "go-b");
    world.until("the settled run's item to expire", |w| {
        !expiries(w, &s).is_empty()
    });
    expired_by(&shown(&world, &s)["item"], &all_done, "Every step is done");

    // A run that ends failed has ended, and has not settled.
    assert_eq!(shown(&world, &e)["item"]["state"], "open", "y is live");
    release(&world, "go-c");
    world.until("the ended run's item to expire", |w| {
        !expiries(w, &e).is_empty()
    });
    assert_eq!(settled(&world, y, "c").as_deref(), Some("failed"));
    expired_by(
        &shown(&world, &e)["item"],
        &ended,
        &format!("run {y} ended failed"),
    );
    let still = shown(&world, &f)["item"].clone();
    assert_eq!(
        still["state"], "open",
        "a run that ended failed never settled: {still}"
    );
    assert!(expiries(&world, &f).is_empty());
    // A node that settled failed has a settlement of an outcome too.
    expired_by(
        &shown(&world, &c)["item"],
        &failed_node,
        &format!("node c of run {y} settled failed"),
    );

    // A stopped run has ended: the next read finds it so.
    assert_eq!(shown(&world, &h)["item"]["state"], "open", "w is live");
    world.run(&["stop", w]).exited(0);
    expired_by(
        &shown(&world, &h)["item"],
        &halted,
        &format!("run {w} ended stopped"),
    );
}

/// A live driver with nothing else due wakes for an item's pending moment and expires it
/// with no read in between, and the expiry reaches the run the item is tied to as a
/// `human-item` surface, as a dismissal does.
#[test]
fn a_live_driver_wakes_for_a_pending_time_and_its_tied_run_is_told() {
    let world = world("expiry-wake");
    let z = "waking";
    let plan = world.plan(z, &plan_of(z, vec![waiting(&world, "w", "go-w", 0)]));
    world.run(&["start", &plan, "--detach"]).exited(0);
    world.until("the command to start", |w| started(w, z, "w"));
    let mut tied = item(
        "tied",
        json!([{"when": "time", "after_seconds": 3, "reason": "The window closed"}]),
    );
    tied["owner"]["run"] = json!(z);
    let tied = raise(&world, &tied);
    let id = id_of(&tied);
    let at = crate::harness::epoch_millis(tied["expiry"][0]["at"].as_str().unwrap());
    world.until_within(Duration::from_secs(60), "the driver to expire it", |w| {
        !expiries(w, &id).is_empty()
    });
    let written = &expiries(&world, &id)[0];
    assert!(
        crate::harness::epoch_millis(written["ts"].as_str().unwrap()) >= at,
        "expired at its moment, not before: {written}"
    );
    assert!(
        settled(&world, z, "w").is_none(),
        "the run was live throughout"
    );
    world.until("the run to be told", |w| {
        surfaces(w, z)
            .iter()
            .any(|m| m.starts_with(&format!("human-item-expired: {id} ")))
    });
    let told = surfaces(&world, z);
    let told = told.iter().find(|m| m.contains(&id)).unwrap();
    assert!(told.ends_with("by engine: The window closed"), "{told}");
    release(&world, "go-w");
}

/// Every `human-item` surface message on `run`'s channel.
fn surfaces(world: &World, run: &str) -> Vec<String> {
    std::fs::read_to_string(world.run_file(run, "channel/surfaces.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|record| record["kind"] == "human-item")
        .map(|record| record["message"].as_str().unwrap().to_owned())
        .collect()
}

/// A `kind: human` node whose item declares an expiry that comes to hold: the step is
/// dropped with its dependents detached and the expiry's reason — never attested — the
/// item reads expired and refuses a reopen, and the dependent command runs. `plan check`
/// refuses a malformed `expiry` naming the node and the field.
#[test]
fn a_human_steps_expiry_drops_it_and_its_dependent_runs() {
    let world = world("expiry-human");
    let name = "signoff";
    let mut gate = human("gate", &[]);
    let condition =
        json!({"when": "time", "after_seconds": 8, "reason": "Nobody needed the sign-off"});
    gate["human_item"] = json!({
        "kind": "task", "priority": "low", "title": "Sign off the release",
        "body": "Say it may ship.", "expiry": [condition],
    });
    let after = posix_script(&world.root, "after", "exit 0");
    let dependent = json!({"id": "after", "kind": "command",
        "command": [after.display().to_string()], "deps": ["gate"]});
    let project = world.plan(name, &plan_of(name, vec![gate.clone(), dependent.clone()]));
    world.run(&["plan", "check", &project]).exited(0);
    world.run(&["start", &project, "--attach"]).exited(0);
    let open = listed(&world, &[]);
    assert_eq!(open.len(), 1, "{open:?}");
    let raised = open[0]["item"].clone();
    let id = id_of(&raised);
    let stored = json!({"when": "time", "at": raised["expiry"][0]["at"], "reason": "Nobody needed the sign-off"});
    assert_eq!(
        raised["expiry"],
        json!([stored]),
        "resolved when its item was raised"
    );
    // Adopting the run raises its step's item again, which moves no deadline.
    world.run(&["adopt", name]).exited(0);
    let again = shown(&world, &id);
    assert_eq!(again["item"]["expiry"], json!([stored]), "{again}");
    assert_eq!(again["history"].as_array().unwrap().len(), 1, "{again}");
    world.until_within(Duration::from_secs(30), "its moment", |_| {
        crate::harness::now_millis() > crate::harness::epoch_millis(stored["at"].as_str().unwrap())
    });
    expired_by(
        &shown(&world, &id)["item"],
        &stored,
        "Nobody needed the sign-off",
    );
    world.run(&["adopt", name]).exited(0);
    let commands: Vec<Value> = world
        .events_of(name, "edit-committed")
        .into_iter()
        .map(|e| e["payload"]["command"].clone())
        .collect();
    assert!(commands.iter().all(|c| c["op"] != "attest"), "{commands:?}");
    let drop = commands
        .iter()
        .find(|c| c["op"] == "drop")
        .expect("the step was dropped");
    assert_eq!(drop["id"], "gate");
    assert_eq!(drop["dependents"], "detach");
    assert_eq!(drop["reason"], "Nobody needed the sign-off");
    assert_ne!(settled(&world, name, "gate").as_deref(), Some("done"));
    assert_eq!(settled(&world, name, "after").as_deref(), Some("done"));
    world
        .run(&["human-items", "reopen", &id])
        .err_has("this item stands for a human step");
    assert_eq!(shown(&world, &id)["reopen"]["available"], false);

    let mut malformed = gate;
    malformed["human_item"]["expiry"] = json!([{"when": "time"}]);
    let bad = world.plan(
        "malformed",
        &plan_of("malformed", vec![malformed, dependent]),
    );
    world
        .run(&["plan", "check", &bad])
        .exited(1)
        .out_has("gate")
        .out_has("human_item")
        .out_has("exactly one of `at` and `after_seconds`");
}

/// A task raise input holding `block`, expiring `after_seconds` from now.
fn holding(dedup: &str, block: Value, after_seconds: u64) -> Value {
    let mut value = item(
        dedup,
        json!([{"when": "time", "after_seconds": after_seconds}]),
    );
    value["kind"] = json!("task");
    value["block"] = block;
    value
}

/// Expiry releases a work, a plan and a host hold — the held command runs once the
/// driver expires them — and an expired item is never blocking and never carries an
/// answer's or a completion's resolution. Reopened, an item is open with its hold back
/// and no conditions, and stays open; raised again with a condition that holds, it is
/// expired again.
#[test]
fn expiry_releases_every_hold_and_a_reopen_takes_its_conditions_back() {
    let world = world("expiry-holds");
    let name = "held";
    let script = posix_script(&world.root, "held-x", "exit 0");
    let project = world.plan(
        name,
        &plan_of(
            name,
            vec![json!({"id": "x", "kind": "command",
                "command": [script.display().to_string()], "deps": []})],
        ),
    );
    let work = id_of(&raise(
        &world,
        &holding(
            "work",
            json!({"scope": "work", "run": name, "node": "x"}),
            4,
        ),
    ));
    let plan = id_of(&raise(
        &world,
        &holding("plan", json!({"scope": "plan", "plan": project}), 4),
    ));
    let host = id_of(&raise(
        &world,
        &holding("host", json!({"scope": "host"}), 4),
    ));
    world.run(&["start", &project, "--detach"]).exited(0);
    let blocking = listed(&world, &["--blocking", "yes"]);
    assert_eq!(blocking.len(), 3, "{blocking:?}");
    assert!(!started(&world, name, "x"), "held by all three");
    world.until_within(Duration::from_secs(60), "the held step to run", |w| {
        settled(w, name, "x").as_deref() == Some("done")
    });
    for id in [&work, &plan, &host] {
        let detail = shown(&world, id);
        assert_eq!(detail["item"]["state"], "expired", "{detail}");
        assert_eq!(detail["blocked"]["total"], 0);
        let resolution = &detail["item"]["resolution"];
        assert_eq!(resolution["actor"], "engine");
        assert_eq!(resolution["text"], Value::Null);
        assert!(resolution.get("choice").is_none());
        world
            .run(&["human-items", "complete", id])
            .err_has("is already expired");
    }
    assert!(listed(&world, &["--blocking", "yes"]).is_empty());
    assert_eq!(
        listed(&world, &["--state", "expired", "--blocking", "no"]).len(),
        3
    );

    let reopened = world.run(&["human-items", "reopen", &work]);
    reopened.exited(0);
    let receipt: Value = serde_json::from_str(&reopened.stdout).unwrap();
    let item = &receipt["item"]["item"];
    assert_eq!(item["state"], "open");
    assert_eq!(item["resolution"], Value::Null);
    assert!(item.get("expiry").is_none(), "{item}");
    let back: Vec<String> = listed(&world, &["--blocking", "yes"])
        .iter()
        .map(|v| v["item"]["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(back, [work.as_str()], "its hold is back");
    assert_eq!(
        shown(&world, &work)["item"]["state"],
        "open",
        "the next sweep leaves it"
    );
    assert_eq!(expiries(&world, &work).len(), 1);
    let mut armed = holding(
        "work",
        json!({"scope": "work", "run": name, "node": "x"}),
        1,
    );
    armed["expiry"] = json!([{"when": "time", "at": PAST}]);
    raise(&world, &armed);
    assert_eq!(
        shown(&world, &work)["item"]["state"],
        "expired",
        "armed again"
    );
    assert_eq!(expiries(&world, &work).len(), 2);
}

/// A check condition running `script` from the world's root.
fn check_of(world: &World, script: &std::path::Path, extra: Value) -> Value {
    let mut condition = json!({"when": "check", "command": [script.display().to_string()],
        "cwd": world.root.display().to_string()});
    condition
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    condition
}

/// The lines a check appended to `file` under the world's root.
fn lines_of(world: &World, file: &str) -> Vec<String> {
    std::fs::read_to_string(world.root.join(file))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// The disk-space case's mechanics: a real script, told its item's id, answers not yet
/// while a real file-system condition does not hold and expires the item with words of
/// its own once it does, on the first sweep an interval after its last run; a check
/// that prints no reason, or a blank one, records its condition's own.
#[cfg(unix)]
#[test]
fn a_check_answers_not_yet_until_its_condition_holds_and_expires_with_its_own_words() {
    let world = world("expiry-check");
    let freed = shell_path(&world.root.join("freed"));
    let seen = shell_path(&world.root.join("seen"));
    let disk = posix_script(
        &world.root,
        "disk",
        &format!(
            "echo \"$ONEPIPELINE_HUMAN_ITEM_ID\" >> '{seen}'\n\
             if [ -f '{freed}' ]; then\n  printf '{{\"expired\": true, \"reason\": \"%s\"}}\\n' \"$(cat '{freed}')\"\n\
             else\n  echo '{{\"expired\": false, \"free\": \"2G\"}}'\nfi"
        ),
    );
    let condition = check_of(
        &world,
        &disk,
        json!({"interval_seconds": 10, "reason": "Unused"}),
    );
    let id = id_of(&raise(&world, &item("disk-space", json!([condition]))));
    let first = shown(&world, &id);
    assert_eq!(first["item"]["state"], "open");
    let checks = first["checks"].as_array().unwrap();
    assert_eq!(checks.len(), 1, "{first}");
    assert_eq!(checks[0]["condition"], 0);
    assert_eq!(checks[0]["outcome"], "not-yet");
    assert_eq!(checks[0]["detail"], Value::Null);
    let ran = crate::harness::epoch_millis(checks[0]["last_run_at"].as_str().unwrap());
    world
        .run(&["human-items", "show", &id])
        .exited(0)
        .out_has("not yet (it printed \"expired\": false)");
    assert_eq!(
        lines_of(&world, "seen"),
        [id.as_str()],
        "told its own item's id"
    );

    // The disk frees; within the interval the check is not due, so nothing has run.
    std::fs::write(world.root.join("freed"), "40G free on /srv").unwrap();
    assert_eq!(shown(&world, &id)["item"]["state"], "open");
    assert_eq!(lines_of(&world, "seen").len(), 1);
    world.until_within(Duration::from_secs(30), "the check's interval", |_| {
        crate::harness::now_millis() >= ran + 10_000
    });
    let after = shown(&world, &id);
    expired_by(&after["item"], &condition, "40G free on /srv");
    assert_eq!(after["checks"][0]["outcome"], "holds");
    assert_eq!(lines_of(&world, "seen"), [id.clone(), id.clone()]);

    let bare = posix_script(&world.root, "bare", "echo '{\"expired\": true}'");
    let blank = posix_script(
        &world.root,
        "blank",
        "echo '{\"expired\": true, \"reason\": \"  \"}'",
    );
    let own = |script: &std::path::Path| {
        check_of(
            &world,
            script,
            json!({"reason": "The condition's own words"}),
        )
    };
    let bare_id = id_of(&raise(&world, &item("bare", json!([own(&bare)]))));
    let blank_id = id_of(&raise(&world, &item("blank", json!([own(&blank)]))));
    listed(&world, &[]);
    expired_by(
        &shown(&world, &bare_id)["item"],
        &own(&bare),
        "The condition's own words",
    );
    expired_by(
        &shown(&world, &blank_id)["item"],
        &own(&blank),
        "The condition's own words",
    );
}

/// Every way a check can fail leaves its item open with a `failed` status naming the
/// cause — never `not-yet` and never expired — and `report` prints it under the item.
#[cfg(unix)]
#[test]
fn every_failing_check_leaves_its_item_open_and_names_the_cause() {
    let world = world("expiry-failing");
    let malformed = "its output was not one JSON object with a boolean \"expired\"";
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "exit-one",
            "echo 'nothing to say' >&2\nexit 1",
            json!({}),
            "the check exited with status 1",
        ),
        (
            "exit-two",
            "exit 2",
            json!({}),
            "the check exited with status 2",
        ),
        (
            "signal",
            "kill -9 $$",
            json!({}),
            "the check was stopped by signal 9",
        ),
        (
            "timeout",
            "sleep 30",
            json!({"timeout_seconds": 1}),
            "the check timed out after 1 s",
        ),
        ("silent", "exit 0", json!({}), malformed),
        ("text", "echo 'all clear'", json!({}), malformed),
        (
            "two",
            "echo '{\"expired\": true}{\"expired\": true}'",
            json!({}),
            malformed,
        ),
        (
            "array",
            "echo '[{\"expired\": true}]'",
            json!({}),
            malformed,
        ),
        (
            "unanswered",
            "echo '{\"free\": \"40G\"}'",
            json!({}),
            malformed,
        ),
        ("yes", "echo '{\"expired\": \"yes\"}'", json!({}), malformed),
        (
            "numbered",
            "echo '{\"expired\": true, \"reason\": 3}'",
            json!({}),
            malformed,
        ),
        (
            "lingering",
            "sleep 5 &\necho '{\"expired\": true}'",
            json!({}),
            "a process it started still held its output open after it exited",
        ),
        (
            "long",
            "head -c 70000 /dev/zero | tr '\\0' ' '\necho '{\"expired\": true}'",
            json!({}),
            "printed more than the 65536 bytes the engine reads",
        ),
    ];
    let mut raised = Vec::new();
    for (name, body, extra, cause) in &cases {
        let script = posix_script(&world.root, name, body);
        let id = id_of(&raise(
            &world,
            &item(name, json!([check_of(&world, &script, extra.clone())])),
        ));
        raised.push((id, *cause));
    }
    let missing = check_of(&world, &world.root.join("no-such-check"), json!({}));
    raised.push((
        id_of(&raise(&world, &item("missing", json!([missing])))),
        "the check could not be started:",
    ));
    let lost = posix_script(&world.root, "lost", "echo '{\"expired\": true}'");
    let gone = world.root.join("gone");
    let mut moved = check_of(&world, &lost, json!({}));
    moved["cwd"] = json!(gone.display().to_string());
    raised.push((
        id_of(&raise(&world, &item("lost", json!([moved])))),
        "does not exist",
    ));

    // One read sweeps them all, each check run once and none of them deciding anything.
    let open = listed(&world, &[]);
    assert_eq!(open.len(), raised.len());
    for (id, cause) in &raised {
        let view = open.iter().find(|v| v["item"]["id"] == *id).unwrap();
        assert_eq!(view["item"]["state"], "open", "{view}");
        let check = &view["checks"][0];
        assert_eq!(check["outcome"], "failed", "{view}");
        let detail = check["detail"].as_str().unwrap();
        assert!(
            detail.starts_with(cause) || detail.contains(cause),
            "{id}: {detail}"
        );
        if *cause == malformed {
            assert!(detail.starts_with(malformed), "{detail}");
        }
    }
    assert!(expiries_any(&world).is_empty());
    let exit_one = &raised[0].0;
    let stderr_tail =
        &open.iter().find(|v| v["item"]["id"] == *exit_one).unwrap()["checks"][0]["detail"];
    assert!(
        stderr_tail.as_str().unwrap().contains("nothing to say"),
        "{stderr_tail}"
    );
    let gone_detail = format!("its working directory {} does not exist", gone.display());
    let lost_view = open
        .iter()
        .find(|v| v["item"]["id"] == raised.last().unwrap().0)
        .unwrap();
    assert_eq!(lost_view["checks"][0]["detail"], gone_detail);

    let report = world.run(&["human-items", "report", "--all"]);
    report.exited(0);
    for (name, ..) in &cases {
        let entry = report
            .stdout
            .split("\n- ")
            .find(|entry| entry.starts_with(&format!("Is {name} still needed?")))
            .unwrap_or_else(|| panic!("{name} is reported:\n{}", report.stdout));
        assert!(entry.contains("check [0]"), "{entry}");
        assert!(entry.contains(": failed: "), "{entry}");
    }
    report.out_has("failed: the check exited with status 1");
}

/// Every `human-item-expired` record the journal holds.
fn expiries_any(world: &World) -> Vec<Value> {
    journal(world)
        .into_iter()
        .filter(|e| e["kind"] == "human-item-expired")
        .collect()
}

/// Two readers sweeping the same due check at once run its program once, and a sweep
/// after its interval runs it again.
#[cfg(unix)]
#[test]
fn concurrent_readers_run_a_due_check_once_per_interval() {
    let world = world("expiry-claims");
    let runs = shell_path(&world.root.join("runs.log"));
    let script = posix_script(
        &world.root,
        "counted",
        &format!("echo ran >> '{runs}'\nsleep 1\necho '{{\"expired\": false}}'"),
    );
    let id = id_of(&raise(
        &world,
        &item(
            "counted",
            json!([check_of(&world, &script, json!({"interval_seconds": 10}))]),
        ),
    ));
    let readers: Vec<_> = (0..2)
        .map(|_| {
            world
                .cmd(&["human-items", "list", "--json"])
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();
    for mut reader in readers {
        assert!(reader.wait().unwrap().success());
    }
    assert_eq!(
        lines_of(&world, "runs.log").len(),
        1,
        "one run for two readers"
    );
    let ran = crate::harness::epoch_millis(
        shown(&world, &id)["checks"][0]["last_run_at"]
            .as_str()
            .unwrap(),
    );
    assert_eq!(
        lines_of(&world, "runs.log").len(),
        1,
        "not due within its interval"
    );
    world.until_within(Duration::from_secs(30), "the check's interval", |_| {
        crate::harness::now_millis() >= ran + 10_000
    });
    listed(&world, &[]);
    assert_eq!(
        lines_of(&world, "runs.log").len(),
        2,
        "due again after its interval"
    );
}

/// A claim whose claimant was killed before its check finished holds the check until its
/// timeout and grace have passed, and the check is then claimed again, run and recorded.
#[cfg(unix)]
#[test]
fn a_dead_claimants_check_is_claimed_again_after_its_timeout_and_grace() {
    let world = world("expiry-stale");
    let runs = shell_path(&world.root.join("runs.log"));
    let script = posix_script(
        &world.root,
        "slow-first",
        &format!(
            "echo ran >> '{runs}'\n\
             if [ \"$(wc -l < '{runs}')\" -eq 1 ]; then sleep 3; fi\n\
             echo '{{\"expired\": false}}'"
        ),
    );
    let id = id_of(&raise(
        &world,
        &item(
            "stale",
            json!([check_of(&world, &script, json!({"timeout_seconds": 1}))]),
        ),
    ));
    let claimed = crate::harness::now_millis();
    let mut claimant = world
        .cmd(&["human-items", "list", "--json"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    world.until("the claimant's check to start", |w| {
        lines_of(w, "runs.log").len() == 1
    });
    claimant.kill().unwrap();
    claimant.wait().unwrap();
    // Its claim is live: another reader runs nothing and has nothing to show for it.
    let held = shown(&world, &id);
    assert!(held.get("checks").is_none(), "{held}");
    assert_eq!(lines_of(&world, "runs.log").len(), 1);
    // Its timeout (1 s) and grace (5 s) pass: the next reader claims and runs it.
    world.until_within(Duration::from_secs(30), "the claim to go stale", |_| {
        crate::harness::now_millis() >= claimed + 6_500
    });
    let recovered = shown(&world, &id);
    assert_eq!(recovered["checks"][0]["outcome"], "not-yet", "{recovered}");
    assert_eq!(lines_of(&world, "runs.log").len(), 2);
}

/// Four checks under `world.root/<tag>`, each of which records that it started, waits
/// until all four have, records that it saw them all running at once, starts a child
/// that never exits, records both process ids, and never exits itself.
#[cfg(unix)]
fn hanging(world: &World, tag: &str) -> Vec<String> {
    let dir = world.root.join(tag);
    std::fs::create_dir_all(&dir).unwrap();
    let at = |name: &str| shell_path(&dir.join(name));
    (0..4)
        .map(|n| {
            let script = posix_script(
                &dir,
                &format!("hang-{n}"),
                &format!(
                    "echo $$ >> '{started}'\n\
                     while [ \"$(wc -l < '{started}')\" -lt 4 ]; do sleep 0.05; done\n\
                     echo all >> '{saw}'\n\
                     sleep 100000 &\n\
                     echo \"$! $$\" >> '{pids}'\n\
                     while :; do sleep 1; done",
                    started = at("started"),
                    saw = at(&format!("saw-{n}")),
                    pids = at("pids"),
                ),
            );
            let condition = check_of(world, &script, json!({"timeout_seconds": 5}));
            id_of(&raise(
                world,
                &item(&format!("{tag}-{n}"), json!([condition])),
            ))
        })
        .collect()
}

/// What four [`hanging`] checks leave behind once a sweep has run them: each saw all four
/// running at once — which checks run one after another never do, since the first waits
/// alone until its timeout ends it — and no check or child it started is still alive.
#[cfg(unix)]
fn ran_at_once_and_were_stopped(world: &World, tag: &str) {
    let dir = world.root.join(tag);
    for n in 0..4 {
        assert!(
            dir.join(format!("saw-{n}")).is_file(),
            "check {n} never saw all four"
        );
    }
    let pids: Vec<u32> = std::fs::read_to_string(dir.join("pids"))
        .unwrap()
        .split_whitespace()
        .map(|pid| pid.parse().unwrap())
        .collect();
    assert_eq!(pids.len(), 8, "a check and its child each, four times");
    world.until_within(
        Duration::from_secs(20),
        "every check and child to end",
        |_| pids.iter().all(|pid| crate::harness::process_ended(*pid)),
    );
}

#[cfg(unix)]
fn timed_out(world: &World, ids: &[String]) {
    for id in ids {
        let detail = shown(world, id);
        assert_eq!(detail["item"]["state"], "open", "{detail}");
        assert_eq!(detail["checks"][0]["outcome"], "failed");
        assert_eq!(
            detail["checks"][0]["detail"],
            "the check timed out after 5 s"
        );
    }
}

/// Four hanging checks swept by one CLI read run at once and are all stopped at their
/// timeout; the read is held no longer than that.
#[cfg(unix)]
#[test]
fn four_hanging_checks_one_read_sweeps_run_at_once_and_are_all_stopped() {
    let world = world("expiry-hang-cli");
    let ids = hanging(&world, "cli");
    // The guard against a hang only: what tells concurrent checks from serial ones is
    // what each recorded, not how long the read took.
    let read = world.run(&["human-items", "list", "--json"]);
    read.exited(0);
    ran_at_once_and_were_stopped(&world, "cli");
    timed_out(&world, &ids);
}

/// The same four, swept by a live driver's pass with no CLI read in between.
#[cfg(unix)]
#[test]
fn four_hanging_checks_a_drivers_pass_sweeps_run_at_once_and_are_all_stopped() {
    let world = world("expiry-hang-driver");
    let ids = hanging(&world, "driver");
    let z = "sweeping";
    let plan = world.plan(z, &plan_of(z, vec![waiting(&world, "w", "go-w", 0)]));
    world.run(&["start", &plan, "--detach"]).exited(0);
    world.until_within(
        Duration::from_secs(60),
        "the driver to run the checks",
        |w| {
            (0..4).all(|n| w.root.join(format!("driver/saw-{n}")).is_file())
                && lines_of(w, "driver/pids").len() == 4
        },
    );
    ran_at_once_and_were_stopped(&world, "driver");
    timed_out(&world, &ids);
    assert_eq!(
        lines_of(&world, "driver/pids").len(),
        4,
        "the read ran none again"
    );
    release(&world, "go-w");
}

/// The SDK's own sweep answers the ids it expired and the checks it ran and failed.
#[cfg(unix)]
#[test]
fn the_sdks_expire_due_answers_what_it_expired_and_ran() {
    let world = world("expiry-sdk-sweep");
    let due = id_of(&raise(
        &world,
        &item("due", json!([{"when": "time", "at": PAST}])),
    ));
    let quiet = posix_script(&world.root, "quiet", "echo '{\"expired\": false}'");
    let broken = posix_script(&world.root, "broken", "exit 1");
    raise(
        &world,
        &item("quiet", json!([check_of(&world, &quiet, json!({}))])),
    );
    raise(
        &world,
        &item("broken", json!([check_of(&world, &broken, json!({}))])),
    );
    let store = sdk(&world);
    let swept = store.expire_due().unwrap();
    assert_eq!(swept.expired, [due.as_str()]);
    assert_eq!((swept.checks_run, swept.checks_failed), (2, 1));
    assert_eq!(expiries(&world, &due).len(), 1);
    let again = store.expire_due().unwrap();
    assert!(again.expired.is_empty());
    assert_eq!(
        (again.checks_run, again.checks_failed),
        (0, 0),
        "none is due again yet"
    );
}

/// A phase hook carrying `expiry` raises its items with it — an `after_seconds` resolved
/// when the phase raises and a check's `cwd` stored absolute — and a malformed hook
/// `expiry` is refused when the configuration loads, naming its file.
#[test]
fn a_phase_hook_raises_its_expiry_and_a_malformed_one_is_refused_at_load() {
    let world = World::new("expiry-hook");
    let dir = world.root.join("xdg/onepipeline");
    std::fs::create_dir_all(&dir).unwrap();
    let config = |expiry: &str| {
        format!(
            "human_items:\n  enabled: true\n  hooks:\n    - phase: review\n      kind: question\n      \
             title: 'Review {{{{ name }}}}'\n      body: 'Look at {{{{ name }}}}.'\n      priority: low\n      \
             block: {{scope: nothing}}\n      expiry: {expiry}\n"
        )
    };
    std::fs::write(
        dir.join("config.yaml"),
        config(
            "[{when: time, after_seconds: 3600, reason: Stale}, {when: check, command: ['true']}]",
        ),
    )
    .unwrap();
    let context = world.root.join("context.json");
    std::fs::write(
        &context,
        json!({"owner": {"session": OWNER, "run": null, "plan": null},
               "dedup_key": "r1", "vars": {"name": "the draft"}})
        .to_string(),
    )
    .unwrap();
    let before = crate::harness::now_millis();
    let raised = world.run(&[
        "human-items",
        "raise",
        "--phase",
        "review",
        "--context",
        context.to_str().unwrap(),
    ]);
    raised.exited(0);
    let item = serde_json::from_str::<Value>(&raised.stdout).unwrap()["item"]["item"].clone();
    let expiry = item["expiry"].as_array().unwrap();
    assert_eq!(expiry.len(), 2, "{item}");
    assert_eq!(expiry[0]["reason"], "Stale");
    let at = crate::harness::epoch_millis(expiry[0]["at"].as_str().unwrap());
    assert!(
        at >= before + 3_600_000 && at < before + 3_700_000,
        "{item}"
    );
    assert_eq!(expiry[1]["command"], json!(["true"]));
    assert!(
        std::path::Path::new(expiry[1]["cwd"].as_str().unwrap()).is_absolute(),
        "{item}"
    );

    std::fs::write(dir.join("config.yaml"), config("[{when: time}]")).unwrap();
    world
        .run(&["human-items", "config"])
        .err_has("config.yaml")
        .err_has("human_items.hooks[review]: expiry: an expiry time names exactly one");
}

/// An expired task is projected through the store's real `Engine::project_human` as a
/// dismissal with the expiry's reason, which the tracker closes as not planned and
/// cancelled — never completed — and a question with the same expiry projects nothing.
#[test]
fn an_expired_task_is_closed_cancelled_on_its_tracker() {
    use crate::private_github_home::{described, GitHub};
    let onevcs = crate::harness::onevcs_binary()
        .to_string_lossy()
        .into_owned();
    let world = World::new("expiry-projection")
        .with_env("HOSTNAME", "projection-host")
        .with_env("ONEPIPELINE_LOOPBACK_BOARD_TOKEN", "loopback-token")
        .with_env(
            "ONETASKGRAPH_WRITE_POLICY__CHECK_COMMAND",
            &format!("{onevcs},boundary,check,--destination,public,--input,-"),
        )
        .with_env(
            "ONETASKGRAPH_WRITE_POLICY__VISIBILITY_COMMAND",
            &format!("{onevcs},boundary,inspect,--input,-,--json"),
        );
    let github = GitHub::serve(
        &world,
        &json!({
            "owner": "owner",
            "repositories": {"owner/service": "public"},
            "boards": {"1": {"public": true, "priority": true}},
            "issues": [{"id": "human", "repository": "owner/service", "title": "Human work",
                        "board": 1, "status": "Todo", "parent": null,
                        "body": described("## Plan\n- Work for people.", &json!({}))}],
        }),
    );
    let repo = world.root.join("checkout");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(repo.join(".onepipeline")).unwrap();
    std::fs::write(
        repo.join(".onepipeline/config.yaml"),
        "human_items:\n  enabled: true\n  task_source: board\n",
    )
    .unwrap();
    let intents = world.root.join("intents");
    std::fs::create_dir_all(&intents).unwrap();
    std::fs::write(
        repo.join("onetaskgraph.yaml"),
        format!(
            "sources:\n  board:\n    plugin: github-projects\n    visibility: public\n    \
             task_projection:\n      host: projection-host\n      directory: {}\n      project: human\n    \
             config:\n      owner: owner\n      project_number: 1\n      repository: owner/service\n      \
             token_env: ONEPIPELINE_LOOPBACK_BOARD_TOKEN\n      endpoint: {}\n      status_mapping:\n        \
             todo: Todo\n        queued: Queued\n        in-progress: In Progress\n        unknown: Needs Attention\n        \
             done: Done\n        cancelled: Canceled\n      priority_mapping:\n        urgent: Urgent\n        \
             high: High\n        medium: Medium\n        low: Low\n",
            intents.display(),
            github.board(1)
        ),
    )
    .unwrap();
    let in_repo = |args: &[&str]| {
        let mut command = world.cmd(args);
        command.current_dir(&repo);
        world.run_on(command, &args.join(" "))
    };
    let soon = |kind: &str, dedup: &str| {
        let mut value = item(
            dedup,
            json!([{"when": "time", "after_seconds": 2,
            "reason": "Superseded by the new rollout"}]),
        );
        value["kind"] = json!(kind);
        let path = world.root.join(format!("{dedup}.json"));
        std::fs::write(&path, value.to_string()).unwrap();
        in_repo(&["human-items", "raise", "--input", path.to_str().unwrap()]).exited(0);
    };
    soon("task", "rollout-task");
    soon("question", "rollout-question");
    let projected = |state: &Value| -> Vec<Value> {
        state["issues"]
            .as_object()
            .unwrap()
            .values()
            .filter(|issue| issue["title"] != "Human work")
            .cloned()
            .collect()
    };
    let open = projected(&github.state());
    assert_eq!(open.len(), 1, "only the task is projected: {open:?}");
    assert_eq!(open[0]["title"], "Is rollout-task still needed?");
    std::thread::sleep(Duration::from_secs(3));
    let read = in_repo(&["human-items", "list", "--json", "--state", "expired"]);
    read.exited(0);
    assert_eq!(
        serde_json::from_str::<Value>(&read.stdout)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let state = github.state();
    let issues = projected(&state);
    assert_eq!(
        issues.len(),
        1,
        "the question was never projected: {issues:?}"
    );
    let issue = &issues[0];
    assert_eq!(issue["state"], "CLOSED", "{issue:#}");
    assert_eq!(issue["stateReason"], "NOT_PLANNED", "{issue:#}");
    assert!(
        issue["body"]
            .as_str()
            .unwrap()
            .contains("Superseded by the new rollout"),
        "{issue:#}"
    );
    let card = state["items"]
        .as_object()
        .unwrap()
        .values()
        .find(|card| card["content"] == issue["id"])
        .unwrap();
    assert_eq!(card["values"]["STATUS_1"], "Canceled", "{card:#}");
}

/// The release before expiry, v0.115.0, wrote `tests/fixtures/human-items-v0.115`: a live
/// journal and its read index, and a sample bundle it rewrote, each with no expiry, and
/// what it printed reading them (`reads.json`, `sample-*.json`). This build reads every one
/// of them unchanged — each item, view and detail byte for byte as that release printed it —
/// and acts on them.
#[test]
fn records_the_release_before_expiry_wrote_read_and_act_unchanged() {
    let world = world("expiry-compatibility");
    let fixture = crate::harness::repo_file("tests/fixtures/human-items-v0.115");
    let store = world.runs.join("human-items");
    std::fs::create_dir_all(&store).unwrap();
    for file in ["events.ndjson", "index.json"] {
        std::fs::copy(fixture.join(file), store.join(file)).unwrap();
    }
    let reads: serde_json::Map<String, Value> =
        serde_json::from_str(&std::fs::read_to_string(fixture.join("reads.json")).unwrap())
            .unwrap();
    assert!(reads.len() > 10, "{reads:?}");
    for (command, printed) in &reads {
        let mut args = vec!["human-items"];
        args.extend(command.split(' '));
        let read = world.run(&args);
        read.exited(0);
        assert_eq!(read.stdout, printed.as_str().unwrap(), "{command}");
    }
    let journal = std::fs::read_to_string(store.join("events.ndjson")).unwrap();
    assert!(!journal.contains("expiry") && !journal.contains("expired"));
    assert_eq!(
        std::fs::read_to_string(fixture.join("events.ndjson")).unwrap(),
        journal,
        "reading wrote nothing"
    );
    // Acted on: the open question is answered, and the earlier records stay as written.
    let open = listed(&world, &["--kind", "question"]);
    let id = id_of(&open[0]["item"]);
    world
        .run(&["human-items", "answer", &id, "--text", "Retire it."])
        .exited(0);
    assert_eq!(shown(&world, &id)["item"]["state"], "answered");
    let now = std::fs::read_to_string(store.join("events.ndjson")).unwrap();
    assert!(now.starts_with(&journal));

    let sample = world.root.join("sample.json");
    std::fs::copy(fixture.join("sample.json"), &sample).unwrap();
    let path = sample.to_str().unwrap();
    for (state, printed) in [
        (None, "sample-open.json"),
        (Some("dismissed"), "sample-dismissed.json"),
    ] {
        let mut args = vec!["human-items", "--sample", path, "list", "--json"];
        if let Some(state) = state {
            args.extend(["--state", state]);
        }
        let read = world.run(&args);
        read.exited(0);
        assert_eq!(
            read.stdout,
            std::fs::read_to_string(fixture.join(printed)).unwrap(),
            "{printed}"
        );
    }
    world
        .run(&[
            "human-items",
            "--sample",
            path,
            "dismiss",
            "item-1",
            "--reason",
            "Restored",
        ])
        .exited(0);
    let bundle: onepipeline::human_items::HumanItemSample =
        serde_json::from_str(&std::fs::read_to_string(&sample).unwrap()).unwrap();
    assert!(bundle
        .items
        .iter()
        .all(|d| d.item.expiry.is_empty() && d.checks.is_empty()));
}

/// Which condition expires an item: its time, run and node conditions are read before
/// any check runs, in the order declared, and of several checks that hold the first
/// declared is the one named.
#[cfg(unix)]
#[test]
fn the_first_condition_that_holds_is_the_one_an_expiry_names() {
    let world = world("expiry-order");
    let ran = shell_path(&world.root.join("ran"));
    let saying = |name: &str, reason: &str| {
        let script = posix_script(
            &world.root,
            name,
            &format!(
                "echo {name} >> '{ran}'\necho '{{\"expired\": true, \"reason\": \"{reason}\"}}'"
            ),
        );
        check_of(&world, &script, json!({}))
    };
    let unrun = saying("unrun", "never asked");
    let late = json!({"when": "time", "at": PAST, "reason": "Its time passed"});
    let before_checks = id_of(&raise(&world, &item("before-checks", json!([unrun, late]))));
    let ahead = json!({"when": "time", "at": FUTURE});
    let second = json!({"when": "time", "at": PAST, "reason": "The second time"});
    let ordered = id_of(&raise(&world, &item("ordered", json!([ahead, second]))));
    let (first, other) = (saying("first", "the first"), saying("other", "the other"));
    let checks = id_of(&raise(&world, &item("checks", json!([first, other]))));
    listed(&world, &[]);
    expired_by(
        &shown(&world, &before_checks)["item"],
        &late,
        "Its time passed",
    );
    expired_by(&shown(&world, &ordered)["item"], &second, "The second time");
    expired_by(&shown(&world, &checks)["item"], &first, "the first");
    let mut runs = lines_of(&world, "ran");
    runs.sort();
    assert_eq!(runs, ["first", "other"], "a time that held ran no check");
}

/// A check record the host cannot read runs no check and says so, and a journal that
/// will not take an expiry leaves the item open, said, until a sweep after it does.
#[cfg(unix)]
#[test]
fn a_record_or_journal_that_refuses_leaves_items_open_until_it_is_repaired() {
    use std::os::unix::fs::PermissionsExt;
    let world = world("expiry-refusals");
    let ran = shell_path(&world.root.join("ran"));
    let script = posix_script(
        &world.root,
        "noted",
        &format!("echo ran >> '{ran}'\necho '{{\"expired\": false}}'"),
    );
    let checked = id_of(&raise(
        &world,
        &item("checked", json!([check_of(&world, &script, json!({}))])),
    ));
    let record = world.runs.join("human-items/checks.json");
    std::fs::create_dir(&record).unwrap();
    let read = world.run(&["human-items", "list", "--json"]);
    read.exited(0)
        .err_has("the check record")
        .err_has("cannot be read")
        .err_has("no check is run, because its claim could not be recorded");
    assert!(lines_of(&world, "ran").is_empty(), "no claim, no run");
    std::fs::remove_dir(&record).unwrap();
    assert_eq!(shown(&world, &checked)["checks"][0]["outcome"], "not-yet");
    assert_eq!(lines_of(&world, "ran").len(), 1);

    let due = id_of(&raise(
        &world,
        &item("due", json!([{"when": "time", "at": PAST}])),
    ));
    let journal = world.runs.join("human-items/events.ndjson");
    let writable = std::fs::metadata(&journal).unwrap().permissions();
    std::fs::set_permissions(&journal, std::fs::Permissions::from_mode(0o444)).unwrap();
    let refused = world.run(&["human-items", "list", "--json"]);
    std::fs::set_permissions(&journal, writable).unwrap();
    refused
        .exited(0)
        .err_has("an expiry condition holds and the item could not be expired");
    let open: Value = serde_json::from_str(&refused.stdout).unwrap();
    assert!(
        open.as_array()
            .unwrap()
            .iter()
            .any(|v| v["item"]["id"] == due),
        "answered open: {open}"
    );
    assert!(expiries(&world, &due).is_empty());
    assert_eq!(
        shown(&world, &due)["item"]["state"],
        "expired",
        "the next sweep writes it"
    );
}
