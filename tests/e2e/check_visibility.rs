//! Check visibility: every failing check on a change a run published reaches the
//! manager's default views once, labelled with whether it blocks the merge; checks
//! still running when the publication returns are followed until they settle, named
//! when the checks timeout stops that, and listed at the run's end — and none of it
//! moves a node, a dependent or a publication.
//!
//! Driven through the compiled binary over real git and a real origin on disk, the
//! publication through the real linked `onevcs`. GitHub stands in at `onevcs`'s
//! `ONEVCS_GH` seam: `fake-gh` answers the publication's watch from its scripts, and a
//! follow read — `onevcs::follow_checks`, the standings query — from `onevcs-testing`'s
//! remote double, seeded per read from `gh.follow.<NUMBER>`.

// llmlint: ignore-file[e2e_not_mocked] the crate under test is the real compiled binary and
// `onevcs` the real linked library over real git; GitHub is substituted at that library's
// own `ONEVCS_GH` override, answering follow reads through `onevcs-testing`'s remote
// double, and `oneagentgraph` at its subprocess boundary — the same substitutions, for the
// same reason, as `harness.rs` records.

use crate::draft_lifecycle::team_repository;
use crate::harness::{agent, lifecycle, plan_of, World};
use serde_json::Value;

/// The publication's watch: the required check green, a check that blocks nothing
/// still running when the required ones settle.
const WATCHED: &str = "lint completed success required\nslow in_progress - advisory";

/// What the follow reads after it answer: still running twice, then red.
const FAILS_ON_THE_THIRD_READ: &str = "lint completed success required\nslow in_progress - \
     advisory\n---\nlint completed success required\nslow in_progress - advisory\n---\nlint \
     completed success required\nslow completed failure advisory";

/// The same, ending green.
const PASSES_ON_THE_THIRD_READ: &str = "lint completed success required\nslow in_progress - \
     advisory\n---\nlint completed success required\nslow in_progress - advisory\n---\nlint \
     completed success required\nslow completed success advisory";

/// The address `fake-gh` gives the second check it reports on `owner/service`.
const SLOW_LOG: &str = "https://github.com/owner/service/actions/runs/1/job/2";

/// The follow interval every journey here runs under, in seconds.
const FOLLOW: &str = "0.2";

/// A world publishing `service` on a team repository whose watch sees [`WATCHED`] and
/// whose follow reads answer `follow`, and whose dependent `after` holds until told.
fn world_following(name: &str, follow: &str) -> World {
    // The harness's two-second checks timeout bounds a watch over a host that
    // answers at once; here it is also how long a change is followed, and the
    // journeys need several follow reads — each a subprocess — inside it, which a
    // loaded macOS runner did not fit into two seconds. A journey about that
    // timeout sets its own, after this one.
    let world = World::new(name)
        .with_env("ONEPIPELINE_CHECK_FOLLOW_SECONDS", FOLLOW)
        .with_env("ONEVCS_CHECKS_TIMEOUT_SECONDS", "60");
    team_repository(&world);
    world.script("service.work", "the worker wrote this\n");
    world.script("gh.checks", WATCHED);
    world.script("gh.follow.1", follow);
    world.script("after.wait", "hold");
    world
}

/// Launch `run` detached over `service` and its held dependent `after`.
fn launch(world: &World, run: &str) {
    launch_with(world, run, &[]);
}

/// The same, with `extra` arguments to `start`.
fn launch_with(world: &World, run: &str, extra: &[&str]) {
    let path = world.plan(
        run,
        &plan_of(
            run,
            vec![lifecycle("service", &[]), agent("after", &["service"])],
        ),
    );
    let mut argv = vec!["start", path.as_str(), "--detach"];
    argv.extend_from_slice(extra);
    world.run_from(&world.project, &argv).exited(0);
    world.until("the dependent to be dispatched", |world| {
        world
            .events_of(run, "node-dispatched")
            .iter()
            .any(|event| event["labels"]["node"] == "after")
    });
}

/// Let the held dependent finish, and wait for the run to settle.
fn release(world: &World, run: &str) {
    release_node(world, run, "after");
}

/// Let held node `node` finish, and wait for the run to settle.
fn release_node(world: &World, run: &str, node: &str) {
    world.script(&format!("{node}.go"), "");
    world.until("the run to settle and its driver to let go", |world| {
        world.run_file(run, "result.json").is_file()
            && !world.events_of(run, "driver-exited").is_empty()
    });
}

/// Every follow read the host answered, in order.
fn follow_reads(world: &World) -> Vec<Value> {
    std::fs::read_to_string(world.fakes.join("gh").join("follow-reads.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).expect("a follow read record"))
        .collect()
}

/// One node of a run's result: its status, outcome and landing.
fn settlement(world: &World, run: &str, id: &str) -> (Value, Value, Value) {
    let result = world.run_json(run, "result.json");
    let node = result["nodes"]
        .as_array()
        .expect("the result lists nodes")
        .iter()
        .find(|node| node["id"] == id)
        .cloned()
        .unwrap_or_else(|| panic!("{id} is missing from {result}"));
    (
        node["status"].clone(),
        node["outcome"].clone(),
        node["landing"].clone(),
    )
}

/// The `check-failed` surfaces a run raised, as `planner-surface-queued` records.
fn raised(world: &World, run: &str, kind: &str) -> Vec<Value> {
    world
        .events_of(run, "planner-surface-queued")
        .into_iter()
        .filter(|event| event["payload"]["kind"] == kind)
        .collect()
}

/// The journey the follow exists for: a check that blocks nothing is still running
/// when the required checks settle, and fails two follow reads later. The manager is
/// told once — on a non-blocking `check-failed` surface, a `check-failed` record, and
/// the `change-check` line every default view prints — and the node settles exactly as
/// it does when that check passes, unretried, its dependent dispatched; following
/// stops at the read that answered nothing outstanding.
#[test]
fn a_late_failing_check_that_blocks_nothing_is_shown_once_and_moves_nothing() {
    let world = world_following("late-red", FAILS_ON_THE_THIRD_READ);
    let run = "latered";
    launch(&world, run);
    world.until("the late failure to be raised", |world| {
        !world.events_of(run, "check-failed").is_empty()
    });

    // A run watch armed with no `--until` returns on the surface.
    let watched = world.run(&["watch", run, "--timeout", "60", "--tick-interval", "0"]);
    watched.out_has("check-failed");
    let surfaces = raised(&world, run, "check-failed");
    assert_eq!(surfaces.len(), 1, "{}", world.dump());
    let message = surfaces[0]["payload"]["message"]
        .as_str()
        .expect("a message");
    assert!(!message.contains('\n'), "{message}");
    assert_eq!(surfaces[0]["payload"]["blocking"], false);
    for named in [
        "check 'slow' failed on https://github.com/owner/service/pull/1",
        "(node 'service', head ",
        "not required — does not block",
        SLOW_LOG,
        "Nothing is held, failed or retried for it.",
    ] {
        assert!(message.contains(named), "{named:?} is not in {message}");
    }

    let records = world.events_of(run, "check-failed");
    assert_eq!(records.len(), 1, "{}", world.dump());
    let payload = &records[0]["payload"];
    assert_eq!(payload["node"], "service");
    assert_eq!(payload["change"], "https://github.com/owner/service/pull/1");
    assert_eq!(payload["check"], "slow");
    assert_eq!(payload["state"], "failed");
    assert_eq!(payload["required"], false);
    assert_eq!(payload["requirement"], "not required — does not block");
    assert_eq!(payload["url"], SLOW_LOG);
    assert_eq!(
        payload["head"].as_str().map(str::len),
        Some(40),
        "{payload}"
    );

    // The planner profile — what `monitor` and `next` read by default — shows the
    // check's line, stamped `attention`.
    let line = format!(
        "change-check 'slow' failed (not required — does not block) [attention] log: {SLOW_LOG}"
    );
    world.run(&["monitor", run]).exited(0).out_has(&line);
    let next = world.run(&["next", run]);
    next.exited(0);
    let read: Value = serde_json::from_str(next.stdout.trim()).expect("next prints one object");
    assert!(
        read["check_lines"].as_array().is_some_and(|lines| lines
            .iter()
            .any(|said| said.as_str() == Some(line.as_str()))),
        "{read}"
    );
    assert!(
        read["events"]
            .as_array()
            .is_some_and(|events| events.iter().any(|event| {
                event["kind"] == "change-check"
                    && event["payload"]["name"] == "slow"
                    && event["labels"]["significance"] == "attention"
            })),
        "{read}"
    );

    // Following stopped at the read that answered nothing outstanding.
    let reads = follow_reads(&world).len();
    std::thread::sleep(std::time::Duration::from_secs(1));
    assert_eq!(follow_reads(&world).len(), reads, "a read after the last");
    assert_eq!(reads, 3, "{:?}", follow_reads(&world));

    release(&world, run);
    let red = settlement(&world, run, "service");
    assert_eq!(settlement(&world, run, "after").0, "done");
    let dispatched = world
        .events_of(run, "node-dispatched")
        .into_iter()
        .filter(|event| event["labels"]["node"] == "service")
        .count();
    assert_eq!(dispatched, 1, "the node was retried: {}", world.dump());

    // Exactly what the same journey settles at when the check passes.
    let green_world = world_following("late-green", PASSES_ON_THE_THIRD_READ);
    let green = "lategreen";
    launch(&green_world, green);
    green_world.until("the follow to finish", |world| {
        follow_reads(world).len() >= 3
    });
    release(&green_world, green);
    assert_eq!(red, settlement(&green_world, green, "service"));
    assert!(green_world.events_of(green, "check-failed").is_empty());
}

/// The `driver-exited` the run's last driver wrote.
fn last_exit(world: &World, run: &str) -> Value {
    world
        .events_of(run, "driver-exited")
        .into_iter()
        .last()
        .unwrap_or_else(|| panic!("no driver-exited in {}", world.dump()))
}

/// The checks one `driver-exited` list names, as `(check, requirement)`.
fn listed(exited: &Value, key: &str) -> Vec<(String, String)> {
    exited["payload"][key]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|check| {
                    (
                        check["check"].as_str().unwrap_or_default().to_owned(),
                        check["requirement"].as_str().unwrap_or_default().to_owned(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A red required check is shown once per head it failed on, labelled as blocking
/// the merge, and rules on the publication exactly as it always has: the node is sent
/// back on its branch until its budget is spent and settles `failed` /
/// `checks-failed`. An attempt republishing the same commit reads the same failed
/// check again and raises nothing more; an attempt on a new commit raises a new one.
#[test]
fn a_required_check_that_fails_is_shown_once_per_head_and_rules_as_before() {
    // Each retry republishes a new commit: three heads, three surfaces. In the second
    // world a check that blocks nothing is still running when the last publication
    // returns, and the host re-runs the red check on that same head while it is
    // followed — a second reading of it, which raises nothing more. A held node
    // beside it keeps the run driven while that happens.
    for (name, rerun) in [("required-red", false), ("required-red-rerun", true)] {
        let world = World::new(name).with_env("ONEPIPELINE_CHECK_FOLLOW_SECONDS", FOLLOW);
        world.repository("change-open", &[]);
        world.script("service.work-anew", "the worker wrote this\n");
        let run = name.replace('-', "");
        if rerun {
            world.script(
                "gh.checks",
                "lint completed failure required\nslow in_progress - advisory",
            );
            world.script(
                "gh.follow.1",
                "lint completed failure required\nslow in_progress - advisory\n---\nrerun\nlint \
                 completed failure required\nslow completed success advisory",
            );
            world.script("beside.wait", "hold");
            let path = world.plan(
                &run,
                &plan_of(&run, vec![lifecycle("service", &[]), agent("beside", &[])]),
            );
            world.run_from(&world.project, &["start", path.as_str(), "--detach"]);
            world.until("the re-run to be read", |world| {
                follow_reads(world).len() >= 2
            });
            release_node(&world, &run, "beside");
        } else {
            world.script("gh.checks", "lint completed failure required");
            crate::draft_lifecycle::settled_run(&world, &run, vec![lifecycle("service", &[])], &[]);
        }

        let (status, outcome, _) = settlement(&world, &run, "service");
        assert_eq!(status, "failed", "{}", world.dump());
        assert_eq!(outcome, "checks-failed");
        let dispatched = world
            .events_of(&run, "node-dispatched")
            .into_iter()
            .filter(|event| event["labels"]["node"] == "service")
            .count();
        assert_eq!(dispatched, 3, "the retry budget moved: {}", world.dump());

        let readings = world
            .events_of(&run, "change-check")
            .into_iter()
            .filter(|event| {
                event["payload"]["name"] == "lint" && event["payload"]["state"] == "failed"
            })
            .count();
        let records = world.events_of(&run, "check-failed");
        let distinct: std::collections::BTreeSet<String> = records
            .iter()
            .map(|record| {
                record["payload"]["head"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect();
        assert_eq!(records.len(), 3, "one per head: {}", world.dump());
        assert_eq!(distinct.len(), 3, "one surface per head: {records:?}");
        if rerun {
            assert!(
                readings > records.len(),
                "no head's red check was read twice: {readings} readings"
            );
        }
        let surfaces = raised(&world, &run, "check-failed");
        assert_eq!(surfaces.len(), 3);
        for surface in &surfaces {
            let message = surface["payload"]["message"].as_str().unwrap_or_default();
            assert!(
                message.contains("check 'lint' failed on ")
                    && message.contains("required — blocks the merge")
                    && message.contains(
                        "The publication's own outcome decides what happens to the node."
                    ),
                "{message}"
            );
            assert_eq!(surface["payload"]["blocking"], false);
        }
        // This identity's watch reports no address for its checks: the line ends at the
        // label's bracket, and the surface says the host gave none.
        if !rerun {
            let monitor = world.run(&["monitor", &run, "--all"]);
            monitor.exited(0);
            let line = monitor
                .stdout
                .lines()
                .find(|line| line.contains("change-check 'lint' failed"))
                .unwrap_or_else(|| panic!("{}", monitor.stdout));
            assert!(
                line.ends_with(
                    "change-check 'lint' failed (required — blocks the merge) [blocker]"
                ),
                "{line}"
            );
            assert!(surfaces.iter().all(|surface| surface["payload"]["message"]
                .as_str()
                .is_some_and(
                    |message| message.contains("log: the host reported no address for this check.")
                )));
        }
        for record in &records {
            assert_eq!(record["payload"]["required"], true);
            assert_eq!(
                record["payload"]["requirement"],
                "required — blocks the merge"
            );
        }
    }
}

/// A run that ends while a followed check is still running names that check, with
/// its requirement, on the record of its ending and in `results` and `status`.
#[test]
fn a_check_still_running_when_the_run_ends_is_listed_at_its_end() {
    let world = world_following(
        "still-running",
        "lint completed success required\nslow in_progress - advisory",
    );
    let run = "stillrunning";
    launch(&world, run);
    world.until("a follow read", |world| !follow_reads(world).is_empty());
    release(&world, run);

    let exited = last_exit(&world, run);
    assert_eq!(
        listed(&exited, "checks_running"),
        [(
            "slow".to_owned(),
            "not required — does not block".to_owned()
        )],
        "{exited}"
    );
    assert!(
        exited["payload"].get("checks_unfollowed").is_none(),
        "{exited}"
    );
    let line = "slow on https://github.com/owner/service/pull/1 (node 'service') — not required \
                — does not block";
    for verb in ["results", "status"] {
        let shown = world.run(&[verb, run]);
        shown
            .exited(0)
            .out_has("checks still running at run end:")
            .out_has(line);
        shown.out_lacks("checks no longer followed (timed out):");
    }
}

/// A driver stopped while a published change still has checks running, and the run
/// adopted by a new one: the new driver resumes following that change, raises the
/// later failure's one surface, raises nothing its predecessor already raised, and
/// lists the check still running when the run ends.
#[cfg(unix)]
#[test]
fn an_adopting_driver_resumes_following_and_raises_nothing_twice() {
    let world = World::new("follow-adopted").with_env("ONEPIPELINE_CHECK_FOLLOW_SECONDS", FOLLOW);
    team_repository(&world);
    world.script("service.work", "the worker wrote this\n");
    world.script(
        "gh.checks",
        "lint completed success required\nearly completed failure advisory\nslow in_progress - \
         advisory\nslower in_progress - advisory",
    );
    world.script(
        "gh.follow-clock.1",
        "after_ms 0\nlint completed success required\nearly completed failure advisory\nslow \
         in_progress - advisory\nslower in_progress - advisory\n---\nafter_ms 5000\nlint \
         completed success required\nearly completed failure advisory\nslow completed failure \
         advisory\nslower in_progress - advisory",
    );
    world.script("after.wait", "hold");
    let run = "followadopted";
    launch(&world, run);
    world.until(
        "the watch's own failure to be raised and a follow read",
        |world| !world.events_of(run, "check-failed").is_empty() && !follow_reads(world).is_empty(),
    );
    assert!(world
        .events_of(run, "check-failed")
        .iter()
        .all(|record| record["payload"]["check"] == "early"));
    crate::harness::end_driver(&world, run);
    let read_before = follow_reads(&world).len();

    let mut adopting = world
        .cmd(&["adopt", run])
        .env("ONEPIPELINE_CHECK_FOLLOW_SECONDS", FOLLOW)
        .env("ONEVCS_CHECKS_TIMEOUT_SECONDS", "60")
        .stdout(std::process::Stdio::null())
        .stderr(std::fs::File::create(world.fakes.join("adopt.stderr")).expect("a stderr file"))
        .spawn()
        .expect("the adoption starts");
    world.until("the adopting driver to follow the change", |world| {
        follow_reads(world).len() > read_before
    });
    let late = |world: &World| {
        world
            .events_of(run, "check-failed")
            .iter()
            .any(|record| record["payload"]["check"] == "slow")
    };
    if !crate::harness::waited(|| late(&world)) {
        panic!(
            "the late failure was not raised; adopt said: {}\nreads: {:?}\njournal kinds: {:?}",
            std::fs::read_to_string(world.fakes.join("adopt.stderr")).unwrap_or_default(),
            follow_reads(&world),
            world
                .journal(run)
                .iter()
                .map(|event| format!("{} {}", event["kind"], event["payload"]["name"]))
                .collect::<Vec<_>>()
        );
    }
    let early = world
        .events_of(run, "check-failed")
        .into_iter()
        .filter(|record| record["payload"]["check"] == "early")
        .count();
    assert_eq!(
        early,
        1,
        "the adopting driver raised `early` again: {}",
        world.dump()
    );
    let slow: Vec<Value> = raised(&world, run, "check-failed")
        .into_iter()
        .filter(|surface| {
            surface["payload"]["message"]
                .as_str()
                .is_some_and(|message| message.starts_with("check 'slow' failed"))
        })
        .collect();
    assert_eq!(slow.len(), 1, "{}", world.dump());

    world.script("after.go", "");
    let ended = adopting.wait().expect("the adoption ends");
    assert!(ended.success(), "adopt exited {ended}: {}", world.dump());
    let exited = last_exit(&world, run);
    assert_eq!(
        listed(&exited, "checks_running"),
        [(
            "slower".to_owned(),
            "not required — does not block".to_owned()
        )],
        "{exited}"
    );
    world
        .run(&["results", run])
        .exited(0)
        .out_has("checks still running at run end:")
        .out_has("slower on https://github.com/owner/service/pull/1 (node 'service') — not required — does not block");
}

// llmlint: ignore-block[tests_mirror_real_usage] this block is the helper and the one journey that
// calls it. The records the journey plants — a `check-failed` and a `checks-unfollowed` short of
// fields their payloads require, and an ending entry that does not say whether its check blocks —
// are written by no build of onepipeline, which writes each from a typed payload, so no interface
// reaches them; they are what a journal another build or a person edited reads like, which is the
// input under test. Everything after each edit is the compiled binary's own `adopt` and `results`.
/// Rewrite `run`'s journal through `edit`, which sees every record and appends any it
/// returns after the last; the fold checkpoint is dropped, as a journal it no longer
/// corroborates leaves it, so the run is read from the journal alone.
#[cfg(unix)]
fn edit_journal(world: &World, run: &str, edit: impl Fn(&mut Vec<Value>)) {
    let path = world.run_file(run, "events.jsonl");
    let mut records: Vec<Value> = std::fs::read_to_string(&path)
        .expect("the run's journal reads")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a journal line is JSON"))
        .collect();
    edit(&mut records);
    let text: String = records.iter().map(|record| format!("{record}\n")).collect();
    std::fs::write(&path, text).expect("the journal is rewritten");
    let _ = std::fs::remove_file(world.run_file(run, "checkpoint.json"));
}
/// A journal whose `check-failed` and `checks-unfollowed` records do not say all they
/// must, or whose ending lists a check without saying whether it blocks, is not taken
/// at its word: the adopting driver names each record it could not read, still raises
/// the failure the unreadable `check-failed` claimed, still follows and lists the check
/// the unreadable `checks-unfollowed` claimed, and `results` shows an unreadable ending
/// entry as unreadable rather than as a check that does not block.
#[cfg(unix)]
#[test]
fn a_check_record_that_does_not_say_what_it_names_is_not_taken_at_its_word() {
    let world =
        World::new("malformed-records").with_env("ONEPIPELINE_CHECK_FOLLOW_SECONDS", FOLLOW);
    team_repository(&world);
    world.script("service.work", "the worker wrote this\n");
    world.script(
        "gh.checks",
        "lint completed success required\nslow in_progress - advisory\nslower in_progress - \
         advisory",
    );
    world.script(
        "gh.follow-clock.1",
        "after_ms 0\nlint completed success required\nslow in_progress - advisory\nslower \
         in_progress - advisory\n---\nafter_ms 5000\nlint completed success required\nslow \
         completed failure advisory\nslower in_progress - advisory",
    );
    world.script("after.wait", "hold");
    let run = "malformedrecords";
    launch(&world, run);
    world.until("a follow read", |world| !follow_reads(world).is_empty());
    crate::harness::end_driver(&world, run);
    let change = "https://github.com/owner/service/pull/1";
    edit_journal(&world, run, |records| {
        let head = records
            .iter()
            .rev()
            .find_map(|record| record["payload"]["head"].as_str().map(str::to_owned))
            .expect("a record naming the published head");
        let mut planted = records.last().expect("a journal record").clone();
        let seq = planted["seq"].as_u64().unwrap_or(0);
        // A `check-failed` with no state, requirement or address, and a
        // `checks-unfollowed` with no reason whose check does not say whether it blocks:
        // each names the real change and head, so a reader taking either at its word
        // would silence `slow`'s failure or stop following `slower`.
        planted["seq"] = serde_json::json!(seq + 1);
        planted["kind"] = serde_json::json!("check-failed");
        planted["payload"] = serde_json::json!({
            "node": "service", "change": change, "head": head, "check": "slow",
        });
        records.push(planted.clone());
        planted["seq"] = serde_json::json!(seq + 2);
        planted["kind"] = serde_json::json!("checks-unfollowed");
        planted["payload"] = serde_json::json!({
            "node": "service", "change": change, "head": head,
            "checks": [{"check": "slower"}],
        });
        records.push(planted);
    });

    let stderr = world.fakes.join("adopt-malformed.stderr");
    let mut adopting = world
        .cmd(&["adopt", run])
        .env("ONEPIPELINE_CHECK_FOLLOW_SECONDS", FOLLOW)
        .env("ONEVCS_CHECKS_TIMEOUT_SECONDS", "60")
        .stdout(std::process::Stdio::null())
        .stderr(std::fs::File::create(&stderr).expect("a stderr file"))
        .spawn()
        .expect("the adoption starts");
    let failed = |world: &World| {
        raised(world, run, "check-failed")
            .iter()
            .filter(|surface| {
                surface["payload"]["message"]
                    .as_str()
                    .is_some_and(|message| message.starts_with("check 'slow' failed"))
            })
            .count()
    };
    if !crate::harness::waited(|| failed(&world) > 0) {
        panic!(
            "the unreadable check-failed silenced slow's failure; adopt said: {}",
            std::fs::read_to_string(&stderr).unwrap_or_default()
        );
    }
    world.script("after.go", "");
    let ended = adopting.wait().expect("the adoption ends");
    assert!(ended.success(), "adopt exited {ended}: {}", world.dump());
    assert_eq!(failed(&world), 1, "{}", world.dump());
    let said = std::fs::read_to_string(&stderr).unwrap_or_default();
    for kind in ["check-failed", "checks-unfollowed"] {
        assert!(
            said.contains(&format!("a {kind} record of this run is not one")),
            "{said}"
        );
    }
    let exited = last_exit(&world, run);
    assert_eq!(
        listed(&exited, "checks_running"),
        [(
            "slower".to_owned(),
            "not required — does not block".to_owned()
        )],
        "{exited}"
    );
    assert!(
        exited["payload"].get("checks_unfollowed").is_none(),
        "{exited}"
    );

    edit_journal(&world, run, |records| {
        let exited = records
            .iter_mut()
            .rev()
            .find(|record| record["kind"] == "driver-exited")
            .expect("the ending record");
        let entry = exited["payload"]["checks_running"][0]
            .as_object_mut()
            .expect("the running check");
        entry.remove("required");
        entry.remove("requirement");
    });
    world
        .run(&["results", run])
        .exited(0)
        .out_has("checks still running at run end:")
        .out_has("  an entry this record does not state readably (")
        .out_lacks("does not block");

    // An ending whose list is not a list is shown as unreadable, not as naming nothing.
    edit_journal(&world, run, |records| {
        let exited = records
            .iter_mut()
            .rev()
            .find(|record| record["kind"] == "driver-exited")
            .expect("the ending record");
        exited["payload"]["checks_unfollowed"] = serde_json::json!("slower");
    });
    world
        .run(&["status", run])
        .exited(0)
        .out_has("checks no longer followed (timed out):")
        .out_has(
            "  this record's checks_unfollowed is not a list, so what it names is not shown: \
             \"slower\"",
        );
}

// llmlint: ignore-end[tests_mirror_real_usage]

/// A check that blocks nothing fails while the publication's own watch is still
/// reading, before its required check settles: the node settles exactly as it does
/// when that check passes, is not retried, its dependent runs, and the failure is
/// raised once.
#[test]
fn an_optional_check_failing_during_the_watch_moves_nothing_and_is_raised_once() {
    let run_with = |name: &str, slow: &str| {
        let world = World::new(name);
        team_repository(&world);
        world.script("service.work", "the worker wrote this\n");
        world.script(
            "gh.checks",
            &format!("lint in_progress - required\nslow {slow} advisory"),
        );
        world.script(
            "gh.checks.later",
            &format!("lint completed success required\nslow {slow} advisory"),
        );
        world.script("gh.checks.later-ms", "1500");
        let run = name.replace('-', "");
        crate::draft_lifecycle::settled_run(
            &world,
            &run,
            vec![lifecycle("service", &[]), agent("after", &["service"])],
            &[],
        );
        (world, run)
    };
    let (world, run) = run_with("watch-red", "completed failure");
    let (green_world, green) = run_with("watch-green", "completed success");

    let node = |world: &World, run: &str| {
        let result = world.run_json(run, "result.json");
        let node = result["nodes"]
            .as_array()
            .and_then(|nodes| nodes.iter().find(|node| node["id"] == "service").cloned())
            .expect("the node");
        (
            node["status"].clone(),
            node["outcome"].clone(),
            node["landing"].clone(),
            node["change_url"].clone(),
        )
    };
    assert_eq!(node(&world, &run), node(&green_world, &green));
    assert_eq!(settlement(&world, &run, "after").0, "done");
    let dispatched = world
        .events_of(&run, "node-dispatched")
        .into_iter()
        .filter(|event| event["labels"]["node"] == "service")
        .count();
    assert_eq!(dispatched, 1, "{}", world.dump());

    let surfaces = raised(&world, &run, "check-failed");
    assert_eq!(surfaces.len(), 1, "{}", world.dump());
    assert!(surfaces[0]["payload"]["message"]
        .as_str()
        .is_some_and(|message| message.starts_with("check 'slow' failed")
            && message.contains("not required — does not block")));
    assert!(green_world.events_of(&green, "check-failed").is_empty());
    // Raised while the watch was still reading: before the publication's verdict.
    let journal = world.journal(&run);
    let at = |kind: &str| {
        journal
            .iter()
            .position(|event| event["kind"] == kind)
            .unwrap_or_else(|| panic!("no {kind}"))
    };
    assert!(
        at("check-failed") < at("checks-settled"),
        "{}",
        world.dump()
    );
}

/// A check that blocks nothing ends with no verdict: one surface reading `got no
/// verdict (cancelled)`, a record at `no-verdict`, the check's line under the planner
/// profile, the node's outcome exactly as when the check passes — and a second
/// reading of it at the same head raises nothing more.
#[test]
fn an_optional_check_with_no_verdict_is_raised_once_and_moves_nothing() {
    let run_with = |name: &str, slow: &str| {
        // A draft first, its checks read again once it is lifted: the same head's
        // check read twice.
        let world = World::new(name);
        world.repository("change-auto", &[]);
        world.script("service.work", "the worker wrote this\n");
        let checks = format!("lint completed success required\nslow completed {slow} advisory");
        world.script("gh.checks.draft", &checks);
        world.script("gh.checks", &checks);
        let run = name.replace('-', "");
        crate::draft_lifecycle::settled_run(&world, &run, vec![lifecycle("service", &[])], &[]);
        (world, run)
    };
    let (world, run) = run_with("cancelled-slow", "cancelled");
    let (passing, passed) = run_with("passing-slow", "success");
    assert_eq!(
        settlement(&world, &run, "service"),
        settlement(&passing, &passed, "service")
    );

    let slow: Vec<Value> = world
        .events_of(&run, "check-failed")
        .into_iter()
        .filter(|record| record["payload"]["check"] == "slow")
        .collect();
    assert_eq!(slow.len(), 1, "{}", world.dump());
    assert_eq!(slow[0]["payload"]["state"], "no-verdict");
    assert_eq!(slow[0]["payload"]["conclusion"], "cancelled");
    assert_eq!(
        slow[0]["payload"]["requirement"],
        "not required — does not block"
    );
    let readings = world
        .events_of(&run, "change-check")
        .into_iter()
        .filter(|event| event["payload"]["name"] == "slow")
        .count();
    assert!(
        readings >= 2,
        "the same head's check was read once: {readings}"
    );
    let surfaces: Vec<Value> = raised(&world, &run, "check-failed")
        .into_iter()
        .filter(|surface| {
            surface["payload"]["message"]
                .as_str()
                .is_some_and(|message| message.starts_with("check 'slow'"))
        })
        .collect();
    assert_eq!(surfaces.len(), 1);
    let message = surfaces[0]["payload"]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(
        message.starts_with("check 'slow' got no verdict (cancelled) on ")
            && message.contains("not required — does not block"),
        "{message}"
    );
    world
        .run(&["monitor", &run])
        .exited(0)
        .out_has("change-check 'slow' no-verdict (not required — does not block) [attention]");
}

/// Under a short checks timeout, a check that blocks nothing is still running when it
/// expires: the driver stops reading that change, says so once on a
/// `checks-unfollowed` surface and record, raises nothing for the check's later
/// failure, leaves the node as it would have been, and lists the check at the end.
#[test]
fn a_check_the_timeout_stops_following_is_named_and_its_later_failure_is_not() {
    let world = world_following(
        "unfollowed",
        "lint completed success required\nslow in_progress - advisory",
    )
    .with_env("ONEVCS_CHECKS_TIMEOUT_SECONDS", "3");
    std::fs::remove_file(world.fakes.join("gh.follow.1")).expect("the read script");
    world.script(
        "gh.follow-clock.1",
        "after_ms 0\nlint completed success required\nslow in_progress - advisory\n---\nafter_ms \
         6000\nlint completed success required\nslow completed failure advisory",
    );
    let run = "unfollowed";
    launch(&world, run);
    world.until("the follow to stop", |world| {
        !world.events_of(run, "checks-unfollowed").is_empty()
    });
    let watched = world.run(&["watch", run, "--timeout", "60", "--tick-interval", "0"]);
    watched.out_has("checks-unfollowed");
    let reads = follow_reads(&world).len();
    // Past the moment the double turns the check red.
    std::thread::sleep(std::time::Duration::from_secs(7));
    assert_eq!(
        follow_reads(&world).len(),
        reads,
        "the change was read again"
    );

    let records = world.events_of(run, "checks-unfollowed");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["payload"]["reason"], "timeout");
    assert_eq!(records[0]["payload"]["checks"][0]["check"], "slow");
    assert_eq!(records[0]["payload"]["checks"][0]["required"], false);
    let surfaces = raised(&world, run, "checks-unfollowed");
    assert_eq!(surfaces.len(), 1);
    let message = surfaces[0]["payload"]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(
        message.starts_with(
            "stopped following 1 check(s) still running on https://github.com/owner/service/pull/1"
        ) && message.contains("'slow' (not required — does not block)")
            && !message.contains('\n'),
        "{message}"
    );
    assert!(
        world.events_of(run, "check-failed").is_empty(),
        "{}",
        world.dump()
    );

    release(&world, run);
    assert_eq!(
        settlement(&world, run, "service"),
        (
            Value::from("done"),
            Value::from("change-review-draft"),
            Value::from("unlanded")
        )
    );
    let exited = last_exit(&world, run);
    assert_eq!(
        listed(&exited, "checks_unfollowed"),
        [(
            "slow".to_owned(),
            "not required — does not block".to_owned()
        )]
    );
    for verb in ["results", "status"] {
        world
            .run(&[verb, run])
            .exited(0)
            .out_has("checks no longer followed (timed out):")
            .out_has("slow on https://github.com/owner/service/pull/1 (node 'service') — not required — does not block");
    }
}

/// The recorded listing of `printobserver`'s open release change request.
const HOLDUP_LISTING: &str =
    include_str!("../fixtures/check-visibility/printobserver-holdup-listing.json");

/// The recorded standings read of that change request, as recorded: three of its checks
/// red and three skipped, which [`holdup_standings`] sets green before a journey's own.
const HOLDUP_STANDINGS: &str =
    include_str!("../fixtures/check-visibility/printobserver-holdup-standings.json");

/// The release change request those recordings are about.
const RELEASE_CHANGE: &str = "https://github.com/nickderobertis/printobserver/pull/298";

/// The three checks the recording itself reports red, each set green, so a journey
/// states every failing check it is about.
const RECORDED_RED: [(&str, &str, Option<&str>); 3] = [
    (
        "gate-shard (windows-x86_64, 1/5)",
        "COMPLETED",
        Some("SUCCESS"),
    ),
    ("supply-chain", "COMPLETED", Some("SUCCESS")),
    ("gate (windows-x86_64)", "COMPLETED", Some("SUCCESS")),
];

/// The recorded standings read with its recorded red checks set green and then the
/// named contexts set to `(status, conclusion)`, nothing else about the answer changed.
fn holdup_standings(set: &[(&str, &str, Option<&str>)]) -> String {
    let mut answer: Value = serde_json::from_str(HOLDUP_STANDINGS).expect("the recording");
    let nodes = answer
        .pointer_mut("/data/repository/c0/headRef/target/statusCheckRollup/contexts/nodes")
        .and_then(Value::as_array_mut)
        .expect("the recording carries its rollup");
    for (name, status, conclusion) in RECORDED_RED.iter().chain(set) {
        let node = nodes
            .iter_mut()
            .find(|node| node["name"] == *name)
            .unwrap_or_else(|| panic!("the recording has no check {name}"));
        node["status"] = Value::from(*status);
        node["conclusion"] = conclusion.map_or(Value::Null, Value::from);
        if conclusion.is_none() {
            node["completedAt"] = Value::Null;
        }
    }
    answer.to_string()
}

/// A world holding `consumer` under published adoption on the engine repository's
/// automated release, whose host rule declares `release_change` when `declared`, and
/// whose host answers a holdup read with `standings`.
fn held_world(name: &str, declared: bool, standings: &str) -> (World, std::path::PathBuf) {
    held_world_every(name, "0.5", declared, standings)
}

/// [`held_world`], reading holdups at most every `holdup` seconds.
fn held_world_every(
    name: &str,
    holdup: &str,
    declared: bool,
    standings: &str,
) -> (World, std::path::PathBuf) {
    let world =
        crate::adoption::watching(name).with_env("ONEPIPELINE_RELEASE_HOLDUP_SECONDS", holdup);
    world.write_graphs();
    let (engine_repo, _consumer) = crate::adoption::two_repositories(&world);
    let (script, answer) = world.probe_in(&engine_repo, "engine");
    let release_change = if declared {
        "\x20   release_change: {head: \"release-plz-*\"}\n"
    } else {
        ""
    };
    world.releases(&format!(
        "version: 1\n\
         repositories:\n\
         \x20 - match: {{host: github.com, owner: owner, name: engine}}\n\
         {release_change}\
         \x20   default_target: crate\n\
         \x20   targets:\n\
         \x20   - name: crate\n\
         \x20     style: automated\n\
         \x20     probe: {{script: {script}, timeout_seconds: 30}}\n\
         default:\n\x20 adoption: fast\n"
    ));
    crate::adoption::releases_at(&answer, "0.1.0");
    world.script("gh.holdup-listing", HOLDUP_LISTING);
    world.script("gh.holdup-standings", standings);
    world.script("gh.holdup-numbers", "298");
    (world, answer)
}

/// Every holdup read the host answered, in order.
fn holdup_reads(world: &World) -> Vec<Value> {
    std::fs::read_to_string(world.fakes.join("gh").join("holdup-reads.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).expect("a holdup read record"))
        .collect()
}

/// A held release whose own change request has a failing required check, a failing
/// check that blocks nothing and a pending required check names all three on the
/// `release-wait` surface and in that dependency's `holdup`, each with its label.
#[test]
fn a_release_wait_names_what_holds_the_release_and_whether_each_check_blocks() {
    let (world, _answer) = held_world(
        "release-holdup",
        true,
        &holdup_standings(&[
            ("gate (linux-x86_64)", "COMPLETED", Some("FAILURE")),
            ("classify", "COMPLETED", Some("FAILURE")),
            ("integration (linux-x86_64)", "IN_PROGRESS", None),
        ]),
    );
    let run = crate::adoption::start(
        &world,
        "releaseholdup",
        vec![
            crate::adoption::engine(),
            crate::adoption::consumer(Some("published")),
        ],
    );
    world.until("the wait to name what holds the release", |world| {
        crate::adoption::wait_surface(world, &run, "consumer").contains("held by")
    });
    let surface = crate::adoption::wait_surface(&world, &run, "consumer");
    let line = surface
        .lines()
        .find(|line| line.starts_with("  held by "))
        .unwrap_or_else(|| panic!("{surface}"));
    assert!(
        line.starts_with(&format!(
            "  held by {RELEASE_CHANGE} (the release's own change request): failing — "
        )),
        "{line}"
    );
    for named in [
        "gate (linux-x86_64) (required — blocks the merge)",
        "classify (not required — does not block)",
        "; pending — integration (linux-x86_64) (required — blocks the merge)",
    ] {
        assert!(line.contains(named), "{named:?} is not in {line}");
    }
    assert!(
        surface.contains("- github.com/owner/engine crate — automated release, waited "),
        "{surface}"
    );

    world.until("the record to carry the holdup", |world| {
        crate::adoption::awaiting(world, &run, "consumer")
            .first()
            .is_some_and(|entry| entry.get("holdup").is_some())
    });
    let entry = crate::adoption::awaiting(&world, &run, "consumer")[0].clone();
    let holdup = &entry["holdup"];
    assert!(holdup["read_at"].is_string(), "{holdup}");
    let change = &holdup["changes"][0];
    assert_eq!(change["url"], RELEASE_CHANGE);
    assert_eq!(change["draft"], false);
    let failing: Vec<(Value, Value, Value)> = change["failing"]
        .as_array()
        .expect("failing")
        .iter()
        .map(|check| {
            (
                check["check"].clone(),
                check["required"].clone(),
                check["requirement"].clone(),
            )
        })
        .collect();
    assert!(failing.contains(&(
        Value::from("gate (linux-x86_64)"),
        Value::from(true),
        Value::from("required — blocks the merge")
    )));
    assert!(failing.contains(&(
        Value::from("classify"),
        Value::from(false),
        Value::from("not required — does not block")
    )));
    assert!(change["failing"][0]["url"].is_string(), "{change}");
    assert_eq!(
        change["pending"],
        serde_json::json!([{"check": "integration (linux-x86_64)", "required": true,
            "requirement": "required — blocks the merge"}])
    );
}

/// An identity whose rule declares no release change request is waited on exactly
/// as it was: no holdup is read, and the surface and record carry nothing new.
#[test]
fn a_release_wait_on_an_identity_with_no_release_change_reads_as_it_did() {
    let (world, _answer) = held_world("release-undeclared", false, HOLDUP_STANDINGS);
    let run = crate::adoption::start(
        &world,
        "releaseundeclared",
        vec![
            crate::adoption::engine(),
            crate::adoption::consumer(Some("published")),
        ],
    );
    world.until("the wait to have answered not-released", |world| {
        crate::adoption::wait_surface(world, &run, "consumer").contains("last answer: not-released")
    });
    // Past several holdup intervals.
    std::thread::sleep(std::time::Duration::from_secs(2));
    let surface = crate::adoption::wait_surface(&world, &run, "consumer");
    let lines: Vec<&str> = surface.lines().collect();
    assert_eq!(lines.len(), 3, "{surface}");
    assert!(lines[0]
        .starts_with("node 'consumer' is held under published adoption, waiting on 1 release(s):"));
    assert!(
        lines[1].starts_with("- github.com/owner/engine crate — automated release, waited ")
            && lines[1].ends_with("last answer: not-released")
    );
    assert!(lines[2].starts_with("Nothing times this out"));
    assert!(
        holdup_reads(&world).is_empty(),
        "{:?}",
        holdup_reads(&world)
    );
    for event in world.events_of(&run, "release-wait") {
        for entry in event["payload"]["awaiting"]
            .as_array()
            .into_iter()
            .flatten()
        {
            assert!(entry.get("holdup").is_none(), "{entry}");
        }
    }
}

/// A release whose own change request has only a failing check that blocks nothing
/// holds nothing more than any wait: the surface names that check as not blocking,
/// and when the release arrives the held node is dispatched on the same pass as when
/// the holdup reads nothing failing.
#[test]
fn a_failing_optional_check_on_the_release_change_holds_nothing_when_the_release_arrives() {
    let arrive = |name: &str, standings: &str| {
        let (world, answer) = held_world(name, true, standings);
        let run = crate::adoption::start(
            &world,
            &name.replace('-', ""),
            vec![
                crate::adoption::engine(),
                crate::adoption::consumer(Some("published")),
            ],
        );
        world.until("the wait to name the release change request", |world| {
            crate::adoption::wait_surface(world, &run, "consumer").contains("held by")
        });
        let surface = crate::adoption::wait_surface(&world, &run, "consumer");
        crate::adoption::releases_at(&answer, "0.2.0");
        world.until("the held node to be dispatched", |world| {
            world
                .events_of(&run, "node-dispatched")
                .iter()
                .any(|event| event["labels"]["node"] == "consumer")
        });
        // What the pass that took up the arrival wrote, from the arrival to the
        // dispatch.
        let journal = world.journal(&run);
        let arrived = journal
            .iter()
            .position(|event| event["kind"] == "release-arrived")
            .expect("the arrival");
        let dispatched = journal
            .iter()
            .position(|event| {
                event["kind"] == "node-dispatched" && event["labels"]["node"] == "consumer"
            })
            .expect("the dispatch");
        let between: Vec<String> = journal[arrived..=dispatched]
            .iter()
            .filter(|event| event["source"] == "pipeline")
            .map(|event| event["kind"].as_str().unwrap_or_default().to_owned())
            .collect();
        (surface, between)
    };
    let (red, red_pass) = arrive(
        "release-optional-red",
        &holdup_standings(&[("classify", "COMPLETED", Some("FAILURE"))]),
    );
    assert!(
        red.contains(&format!(
            "  held by {RELEASE_CHANGE} (the release's own change request): failing — classify \
             (not required — does not block)"
        )),
        "{red}"
    );
    let (green, green_pass) = arrive("release-optional-green", &holdup_standings(&[]));
    assert!(
        green.contains(&format!(
            "  held by {RELEASE_CHANGE}: its required checks passed and it has not been merged"
        )),
        "{green}"
    );
    assert_eq!(red_pass, green_pass);
    assert!(
        !red_pass.iter().any(|kind| kind == "release-wait"),
        "{red_pass:?}"
    );
}

/// Where the budget journey's telemetry is written for `check-visibility-github-quota`'s
/// command to read.
pub(crate) const RECORD: &str = "target/budget-records/check-visibility-quota.json";

/// The budget's scale: every interval and duration of the workload divided by it.
const SCALE: f64 = 1000.0;

/// The slack one recorded call time may carry past the engine's own clock: the double
/// stamps a call when its process starts, a few milliseconds after the engine asked.
/// Both read the host's monotonic clock (`monotonic_ms` in the doubles' library), so a
/// step of the wall clock — a virtual machine's guest agent re-synchronising it — moves
/// neither, and the slack need not cover one.
const STAMP_SLACK_MS: u64 = 25;

/// A lifecycle node of repository `repo`.
fn published_in(id: &str, repo: &str, deps: &[&str]) -> Value {
    let mut node = lifecycle(id, deps);
    node["repo"] = Value::from(repo);
    node
}

/// Every pair of successive reads of one key closer together than `every_ms`, by the
/// double's own stamps.
fn closer_than(reads: &[Value], key: &str, every_ms: u64) -> Vec<(u64, u64)> {
    let mut last: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    let mut close = Vec::new();
    for read in reads {
        let at = read["at_ms"].as_u64().expect("a stamp");
        let of = read[key].to_string();
        if let Some(before) = last.insert(of, at) {
            if at.saturating_sub(before) + STAMP_SLACK_MS < every_ms {
                close.push((before, at));
            }
        }
    }
    close
}

/// The budget's workload, scaled by [`SCALE`]: one run publishing 12 change requests
/// across 3 repositories — 6 of them with 3 checks that block nothing still running for
/// 20 minutes after their required check settles — and 2 nodes held 3 hours on one
/// automated release whose own change request has a failing required check. It asserts
/// the engine's cadence and sharing, and records every follow and holdup read with what
/// it cost for `scripts/check-visibility-quota-budget.sh`; it asserts no quota figure.
#[test]
fn the_check_visibility_workload_reads_at_its_cadence_and_records_what_it_spent() {
    let follow = 60.0 / SCALE;
    let holdup = 600.0 / SCALE;
    let timeout = 3600.0 / SCALE;
    let running_ms = (20.0 * 60.0 * 1000.0 / SCALE) as u64;
    let hold = std::time::Duration::from_secs_f64(3.0 * 3600.0 / SCALE);
    let world = crate::adoption::watching("check-visibility-budget")
        .with_env("ONEPIPELINE_CHECK_FOLLOW_SECONDS", &follow.to_string())
        .with_env("ONEPIPELINE_RELEASE_HOLDUP_SECONDS", &holdup.to_string())
        .with_env("ONEVCS_CHECKS_TIMEOUT_SECONDS", &timeout.to_string());
    world.write_graphs();
    // Every change request watched for its checks and kept for review; the release
    // producer's repository lands directly, so its release can be asked about.
    world.repository("change-open", &[]);
    std::fs::write(
        world.onevcs_home().join("rules.yml"),
        "version: 3\nrules:\n  - match: {owner: owner, name: engine}\n    publication: \
         local-direct\n    approvals: none\ndefault:\n  publication: change-open\n  approvals: \
         required\n",
    )
    .expect("the rules file is written");
    let _tools = world.extra_repository("tools");
    let engine_repo = world.extra_repository("engine");
    let (script, answer) = world.probe_in(&engine_repo, "engine");
    world.releases(&format!(
        "version: 1\n\
         repositories:\n\
         \x20 - match: {{host: github.com, owner: owner, name: engine}}\n\
         \x20   release_change: {{head: \"release-plz-*\"}}\n\
         \x20   default_target: crate\n\
         \x20   targets:\n\
         \x20   - name: crate\n\
         \x20     style: automated\n\
         \x20     probe: {{script: {script}, timeout_seconds: 30}}\n\
         default:\n\x20 adoption: fast\n"
    ));
    crate::adoption::releases_at(&answer, "0.1.0");
    world.script("gh.holdup-listing", HOLDUP_LISTING);
    world.script(
        "gh.holdup-standings",
        &holdup_standings(&[("gate (linux-x86_64)", "COMPLETED", Some("FAILURE"))]),
    );
    world.script("gh.holdup-numbers", "298");
    let settled = "lint completed success required";
    let running = "lint completed success required\nslow-a in_progress - advisory\nslow-b \
                   in_progress - advisory\nslow-c in_progress - advisory";
    world.script("gh.checks.repo.owner.service", running);
    world.script("gh.checks.repo.owner.tools", settled);
    world.script("gh.checks.repo.owner.engine", settled);
    world.script(
        "gh.follow-clock.repo.owner.service",
        &format!(
            "after_ms 0\n{running}\n---\nafter_ms {running_ms}\n{settled}\nslow-a completed \
             success advisory\nslow-b completed success advisory\nslow-c completed success \
             advisory"
        ),
    );

    // The release producer lands directly; every other node opens a change request.
    let mut producer = lifecycle("producer", &[]);
    producer["repo"] = Value::from("engine");
    let mut nodes = vec![producer];
    for id in ["s1", "s2", "s3", "s4", "s5", "s6"] {
        nodes.push(published_in(id, "service", &[]));
    }
    for id in ["t1", "t2"] {
        nodes.push(published_in(id, "tools", &[]));
    }
    for id in ["c1", "c2"] {
        let mut held = published_in(id, "tools", &["producer"]);
        held["adoption"] = Value::from("published");
        nodes.push(held);
    }
    for id in ["e1", "e2"] {
        let mut node = published_in(id, "engine", &[]);
        node["merge_policy"] = Value::from("change-open");
        nodes.push(node);
    }
    let run = crate::adoption::start(&world, "checkvisibilitybudget", nodes);

    world.until("the first holdup read", |world| {
        !holdup_reads(world).is_empty()
    });
    let held_from = holdup_reads(&world)[0]["at_ms"].as_u64().expect("a stamp");
    std::thread::sleep(hold);
    crate::adoption::releases_at(&answer, "0.2.0");
    world.until("the run to settle and its driver to let go", |world| {
        world.run_file(&run, "result.json").is_file()
            && !world.events_of(&run, "driver-exited").is_empty()
    });
    let result = world.run_json(&run, "result.json");
    let opened = result["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .filter(|node| node["change_url"].is_string())
        .count();
    assert_eq!(opened, 12, "{result}");

    // The cadence: one repository's follow reads never closer than the interval, and
    // its change requests read together; one identity's holdup reads never closer
    // than its interval, and shared by both held nodes.
    let follows = follow_reads(&world);
    let holdups = holdup_reads(&world);
    let listings: Vec<Value> = holdups
        .iter()
        .filter(|read| read["query"] == "listing")
        .cloned()
        .collect();
    assert!(
        closer_than(&follows, "repo", (follow * 1000.0) as u64).is_empty(),
        "{follows:?}"
    );
    assert!(
        follows.iter().any(|read| read["changes"]
            .as_array()
            .is_some_and(|changes| changes.len() >= 2)),
        "no read followed two change requests of one repository together: {follows:?}\n{:?}\n{:?}",
        world
            .events_of(&run, "change-check")
            .iter()
            .map(|event| format!(
                "{} {} {}",
                event["labels"]["node"], event["payload"]["name"], event["payload"]["state"]
            ))
            .collect::<Vec<_>>(),
        world
            .events_of(&run, "node-settled")
            .iter()
            .map(|event| format!(
                "{} {} {}",
                event["labels"]["node"], event["payload"]["outcome"], event["ts"]
            ))
            .collect::<Vec<_>>()
    );
    assert!(
        follows.iter().all(|read| read["repo"] == "owner/service"),
        "a change request with nothing running was followed: {follows:?}"
    );
    assert!(
        closer_than(&listings, "query", (holdup * 1000.0) as u64).is_empty(),
        "{listings:?}"
    );
    let held_for = listings
        .last()
        .and_then(|read| read["at_ms"].as_u64())
        .unwrap_or(held_from)
        .saturating_sub(held_from);
    let shared_bound = held_for / (holdup * 1000.0) as u64 + 2;
    assert!(
        (listings.len() as u64) <= shared_bound,
        "{} listings over {held_for} ms: each held node made its own",
        listings.len()
    );

    let record = serde_json::json!({
        "schema_version": 1,
        "workload": {"changes": 12, "repositories": 3, "followed_changes": 6,
                     "running_checks_per_followed_change": 3, "running_minutes": 20,
                     "held_nodes": 2, "held_hours": 3, "scale": SCALE},
        "phases": [
            {"phase": "follow", "calls": follows},
            {"phase": "holdup", "calls": holdups},
        ],
    });
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/budget-records");
    std::fs::create_dir_all(&directory).expect("the record directory");
    std::fs::write(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(RECORD),
        serde_json::to_string_pretty(&record).expect("the record serializes"),
    )
    .expect("the record is written");
}

/// Every refused call one of the double's refusal logs holds.
fn refused(world: &World, log: &str) -> usize {
    std::fs::read_to_string(world.fakes.join("gh").join(log))
        .unwrap_or_default()
        .lines()
        .count()
}

/// A follow read the host refuses leaves the change request followed: the next read
/// is made on the interval, and the late failure is raised once it is read.
#[test]
fn a_refused_follow_read_leaves_the_change_followed_until_it_answers() {
    let world = world_following("follow-refused", FAILS_ON_THE_THIRD_READ);
    world.script("gh.follow-outage", "2");
    let run = "followrefused";
    launch(&world, run);
    world.until("the late failure to be raised", |world| {
        !world.events_of(run, "check-failed").is_empty()
    });
    assert_eq!(refused(&world, "follow-refused.jsonl"), 2);
    assert_eq!(follow_reads(&world).len(), 3, "{:?}", follow_reads(&world));
    assert_eq!(world.events_of(run, "check-failed").len(), 1);
    release(&world, run);
    assert_eq!(settlement(&world, run, "service").0, "done");
}

/// A followed change request whose head moved on is answered superseded, and is not
/// read again: nothing is raised about checks on a head it no longer carries.
#[test]
fn a_superseded_head_stops_being_followed() {
    let world = world_following("follow-superseded", FAILS_ON_THE_THIRD_READ);
    world.script("gh.follow-superseded.1", "");
    let run = "followsuperseded";
    launch(&world, run);
    world.until("a follow read", |world| !follow_reads(world).is_empty());
    std::thread::sleep(std::time::Duration::from_secs(1));
    assert_eq!(follow_reads(&world).len(), 1, "{:?}", follow_reads(&world));
    release(&world, run);
    assert!(world.events_of(run, "check-failed").is_empty());
    assert!(world.events_of(run, "checks-unfollowed").is_empty());
    let exited = last_exit(&world, run);
    assert!(
        exited["payload"].get("checks_unfollowed").is_none(),
        "{exited}"
    );
    // The head it was following is not the change request's any more, so the ending
    // lists nothing as still running on it.
    assert!(
        exited["payload"].get("checks_running").is_none(),
        "{exited}"
    );
    world
        .run(&["results", run])
        .exited(0)
        .out_lacks("checks still running at run end:");
}

/// A follow interval this build cannot use leaves the run following at the shipped
/// minute: one read as the publication returns, and none for the seconds after it.
#[test]
fn an_unusable_follow_interval_follows_at_the_shipped_one() {
    let world = World::new("follow-unusable").with_env("ONEPIPELINE_CHECK_FOLLOW_SECONDS", "soon");
    team_repository(&world);
    world.script("service.work", "the worker wrote this\n");
    world.script("gh.checks", WATCHED);
    world.script("gh.follow.1", FAILS_ON_THE_THIRD_READ);
    world.script("after.wait", "hold");
    let run = "followunusable";
    launch(&world, run);
    world.until("the first follow read", |world| {
        !follow_reads(world).is_empty()
    });
    std::thread::sleep(std::time::Duration::from_secs(2));
    assert_eq!(follow_reads(&world).len(), 1, "{:?}", follow_reads(&world));
    release(&world, run);
}

/// An identity that declares a release change request and has none open says so on
/// the wait, and records a holdup with no change requests.
#[test]
fn a_release_wait_with_no_release_change_open_says_so() {
    let (world, _answer) = held_world("release-none-open", true, &holdup_standings(&[]));
    let mut listing: Value = serde_json::from_str(HOLDUP_LISTING).expect("the recording");
    listing["data"]["repository"]["pullRequests"]["nodes"] = serde_json::json!([]);
    world.script("gh.holdup-listing", &listing.to_string());
    let run = crate::adoption::start(
        &world,
        "releasenoneopen",
        vec![
            crate::adoption::engine(),
            crate::adoption::consumer(Some("published")),
        ],
    );
    world.until(
        "the wait to say no release change request is open",
        |world| {
            crate::adoption::wait_surface(world, &run, "consumer")
                .contains("\n  no release change request is open\n")
        },
    );
    world.until("the record to carry the holdup", |world| {
        crate::adoption::awaiting(world, &run, "consumer")
            .first()
            .is_some_and(|entry| entry.get("holdup").is_some())
    });
    let entry = crate::adoption::awaiting(&world, &run, "consumer")[0].clone();
    assert_eq!(entry["holdup"]["changes"], serde_json::json!([]), "{entry}");
    // The listing found nothing, so nothing was read about any change request.
    assert!(holdup_reads(&world)
        .iter()
        .all(|read| read["query"] == "listing"));
}

/// A holdup read the host refuses after one it answered keeps that answer: the wait
/// goes on naming what held the release, and the next answered read replaces it.
#[test]
fn a_refused_holdup_read_keeps_the_last_answer_until_one_is_answered() {
    let (world, _answer) = held_world(
        "release-holdup-refused",
        true,
        &holdup_standings(&[("gate (linux-x86_64)", "COMPLETED", Some("FAILURE"))]),
    );
    let run = crate::adoption::start(
        &world,
        "releaseholduprefused",
        vec![
            crate::adoption::engine(),
            crate::adoption::consumer(Some("published")),
        ],
    );
    let read_at = |world: &World| {
        crate::adoption::awaiting(world, &run, "consumer")
            .first()
            .and_then(|entry| entry["holdup"]["read_at"].as_str().map(str::to_owned))
    };
    world.until("the first holdup to be recorded", |world| {
        read_at(world).is_some()
    });
    let first = read_at(&world).expect("a read");
    world.script("holdup-outage", "");
    std::fs::rename(
        world.fakes.join("holdup-outage"),
        world.fakes.join("gh.holdup-outage"),
    )
    .expect("the outage begins");
    world.until("two refused holdup reads", |world| {
        refused(world, "holdup-refused.jsonl") >= 2
    });
    let recorded = world.events_of(&run, "release-wait").len();
    crate::adoption::read_waits(&world, &run);
    world.until("the wait to be recorded again", |world| {
        world.events_of(&run, "release-wait").len() > recorded
    });
    assert_eq!(
        read_at(&world).as_deref(),
        Some(first.as_str()),
        "the answer was lost"
    );
    assert!(crate::adoption::wait_surface(&world, &run, "consumer")
        .contains("gate (linux-x86_64) (required — blocks the merge)"));

    std::fs::remove_file(world.fakes.join("gh.holdup-outage")).expect("the outage ends");
    let answered = holdup_reads(&world).len();
    world.until("a holdup read answered again", |world| {
        holdup_reads(world).len() > answered
    });
    world.until("the newer answer to be recorded", |world| {
        crate::adoption::read_waits(world, &run);
        read_at(world).is_some_and(|at| at != first)
    });
}

/// A launch whose `vcs` filter keeps `change-check` out of the run is followed all the
/// same, and what each read appends is held to that filter: nothing of it is relayed, so
/// nothing is raised about it.
#[test]
fn a_launch_filtering_out_change_checks_relays_and_raises_nothing_its_follow_reads_append() {
    let world = world_following("follow-filtered", FAILS_ON_THE_THIRD_READ);
    let run = "followfiltered";
    launch_with(
        &world,
        run,
        &["--filter-vcs", r#"{"exclude": [{"kind": "change-check"}]}"#],
    );
    world.until("the follow to read the late failure", |world| {
        follow_reads(world).len() >= 3
    });
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert_eq!(follow_reads(&world).len(), 3, "{:?}", follow_reads(&world));
    release(&world, run);
    assert!(
        world.events_of(run, "change-check").is_empty(),
        "{}",
        world.dump()
    );
    assert!(world.events_of(run, "check-failed").is_empty());
    assert!(raised(&world, run, "check-failed").is_empty());
    assert_eq!(settlement(&world, run, "service").0, "done");
}

/// A driver adopting a run whose checks the timeout already stopped following does not
/// follow them again nor name them twice, and the run's end still lists them once.
#[cfg(unix)]
#[test]
fn an_adopting_driver_does_not_follow_again_what_the_timeout_stopped_following() {
    let world = world_following(
        "unfollowed-adopted",
        "lint completed success required\nslow in_progress - advisory",
    )
    .with_env("ONEVCS_CHECKS_TIMEOUT_SECONDS", "3");
    let run = "unfollowedadopted";
    launch(&world, run);
    world.until("the follow to stop", |world| {
        !world.events_of(run, "checks-unfollowed").is_empty()
    });
    crate::harness::end_driver(&world, run);
    let mut adopting = world
        .cmd(&["adopt", run])
        .env("ONEPIPELINE_CHECK_FOLLOW_SECONDS", FOLLOW)
        .env("ONEVCS_CHECKS_TIMEOUT_SECONDS", "3")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the adoption starts");
    world.until("the adopting driver to take the run", |world| {
        !world.events_of(run, "driver-adopted").is_empty()
    });
    // Past the adoption's own first pass, whose readjustment reads the draft's standing
    // through the same query a follow read is; a follow would read every 0.2 s after it.
    std::thread::sleep(std::time::Duration::from_secs(1));
    let reads = follow_reads(&world).len();
    std::thread::sleep(std::time::Duration::from_millis(1500));
    assert_eq!(
        follow_reads(&world).len(),
        reads,
        "the adopting driver followed again: {:?}\nsettled: {:?}\npushes: {:?}",
        world.events_of(run, "checks-unfollowed"),
        world
            .events_of(run, "node-settled")
            .iter()
            .map(|e| (
                e["payload"]["head"].clone(),
                e["payload"]["change_url"].clone()
            ))
            .collect::<Vec<_>>(),
        world
            .events_of(run, "push")
            .iter()
            .map(|e| e["payload"]["head"].clone())
            .collect::<Vec<_>>(),
    );
    assert_eq!(world.events_of(run, "checks-unfollowed").len(), 1);
    assert_eq!(raised(&world, run, "checks-unfollowed").len(), 1);

    world.script("after.go", "");
    let ended = adopting.wait().expect("the adoption ends");
    assert!(ended.success(), "adopt exited {ended}: {}", world.dump());
    let exited = last_exit(&world, run);
    assert_eq!(
        listed(&exited, "checks_unfollowed"),
        [(
            "slow".to_owned(),
            "not required — does not block".to_owned()
        )]
    );
    assert!(
        exited["payload"].get("checks_running").is_none(),
        "{exited}"
    );
}

/// What holds the release changing between two holdup reads is news: the wait is raised
/// again with the new answer — here a required check that was red now pending, and a
/// check that blocks nothing ended with no verdict, which reads as failing — and the
/// record beside it carries the same.
#[test]
fn a_holdup_that_changes_raises_the_wait_again_with_what_holds_the_release_now() {
    let (world, _answer) = held_world(
        "release-holdup-changes",
        true,
        &holdup_standings(&[("gate (linux-x86_64)", "COMPLETED", Some("FAILURE"))]),
    );
    let run = crate::adoption::start(
        &world,
        "releaseholdupchanges",
        vec![
            crate::adoption::engine(),
            crate::adoption::consumer(Some("published")),
        ],
    );
    world.until("the first holdup on the wait", |world| {
        crate::adoption::wait_surface(world, &run, "consumer")
            .contains("failing — gate (linux-x86_64) (required — blocks the merge)")
    });
    world.script(
        "gh.holdup-standings",
        &holdup_standings(&[
            ("gate (linux-x86_64)", "IN_PROGRESS", None),
            ("classify", "COMPLETED", Some("CANCELLED")),
        ]),
    );
    let now = format!(
        "  held by {RELEASE_CHANGE} (the release's own change request): failing — classify \
         (not required — does not block); pending — gate (linux-x86_64) (required — blocks the \
         merge)"
    );
    world.until("the wait to say what holds the release now", |world| {
        crate::adoption::read_waits(world, &run);
        crate::adoption::wait_surface(world, &run, "consumer").contains(&now)
    });
    world.until("the record to carry it", |world| {
        crate::adoption::awaiting(world, &run, "consumer")
            .first()
            .is_some_and(|entry| {
                entry["holdup"]["changes"][0]["failing"]
                    == serde_json::json!([{"check": "classify", "required": false,
                        "requirement": "not required — does not block",
                        "url": entry["holdup"]["changes"][0]["failing"][0]["url"]}])
            })
    });
}

/// A holdup interval this build cannot use leaves the wait reading what holds the
/// release at the shipped ten minutes: once at the first `not-released`, and not again
/// in the seconds after it.
#[test]
fn an_unusable_holdup_interval_reads_at_the_shipped_one() {
    let (world, _answer) = held_world_every(
        "release-holdup-unusable",
        "soon",
        true,
        &holdup_standings(&[]),
    );
    let run = crate::adoption::start(
        &world,
        "releaseholdupunusable",
        vec![
            crate::adoption::engine(),
            crate::adoption::consumer(Some("published")),
        ],
    );
    world.until("the first holdup read", |world| {
        !holdup_reads(world).is_empty()
    });
    std::thread::sleep(std::time::Duration::from_secs(3));
    let listings = holdup_reads(&world)
        .into_iter()
        .filter(|read| read["query"] == "listing")
        .count();
    assert_eq!(listings, 1, "{:?}", holdup_reads(&world));
    assert!(crate::adoption::wait_surface(&world, &run, "consumer").contains("held by"));
}
