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
    let world = World::new(name).with_env("ONEPIPELINE_CHECK_FOLLOW_SECONDS", FOLLOW);
    team_repository(&world);
    world.script("service.work", "the worker wrote this\n");
    world.script("gh.checks", WATCHED);
    world.script("gh.follow.1", follow);
    world.script("after.wait", "hold");
    world
}

/// Launch `run` detached over `service` and its held dependent `after`.
fn launch(world: &World, run: &str) {
    let path = world.plan(
        run,
        &plan_of(
            run,
            vec![lifecycle("service", &[]), agent("after", &["service"])],
        ),
    );
    world.run_from(&world.project, &["start", path.as_str(), "--detach"]);
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

    // One record, carrying the check whole.
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
        read["check_lines"]
            .as_array()
            .is_some_and(|lines| lines.iter().any(|said| *said == Value::from(line.clone()))),
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
