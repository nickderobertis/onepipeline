//! The pool-maintenance schedule, driven through the real linked `onevcs`.
//!
//! Every journey here runs over a scratch state root holding a pooled identity
//! with a real idle slot — cut by a real session opened and closed through the
//! linked library — and a `workspaces.yml` naming a `maintain` command that
//! writes a marker into the slot's worktree. What is asserted is the sibling's
//! own account, read through its own commands: `onevcs pool status` for the
//! stamp the schedule is a function of, and the marker for what ran.
//!
//! **How many sweeps a driver made is the driver's own count.** The identities a
//! sweep visits come off `onevcs::registered_identities`, so nothing is spawned
//! per sweep any more and there is no call record to count; and a sweep that
//! finds every slot not due is invisible by design — it writes no journal record
//! and moves no stamp. So the number comes from `loop-stats.json`, the driver's
//! report of what its reconcile loop did, which is where the release probes it
//! asks are already counted.
//!
// llmlint: ignore-file[e2e_not_mocked] the sibling under test is *not* substituted:
// `onevcs` is the library this crate links, and every answer these journeys read is
// the linked release's own. `oneagentgraph` is still the double, because what these
// journeys are about is the idle capacity beside a dispatch and a real agent turn is
// a paid one.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::harness::{agent, onevcs_binary, plan_of, repo_file, Run, World, REFUSED, USAGE_ERROR};

/// The identity the world's `service` checkout registers as.
const SERVICE_IDENTITY: &str = "github.com/owner/service";

/// The pace the paced journeys run under: every second, so a second sweep is
/// something a journey can wait for rather than a thing that happens next week.
const FAST_PACE: &str = "1";

/// A world whose driver names its pool-maintenance schedule and reports what its
/// reconcile loop did.
///
/// `pace` is [`onepipeline::maintenance::PACE_ENV`], or `None` for the shipped
/// default.
fn pooled_world(name: &str, pace: Option<&str>) -> World {
    // Held still: the host's own load is its neighbours' — every other journey
    // this suite runs at once — and a driver reading it as no free slot withholds
    // exactly what these journeys exist to see.
    let mut world = World::new(name)
        .with_env(onepipeline::executor::LOAD1_ENV, "0")
        .with_env(crate::harness::LOOP_STATS_ENV, "1");
    if let Some(pace) = pace {
        world = world.with_env(onepipeline::maintenance::PACE_ENV, pace);
    }
    world
}

/// How many pool-maintenance sweeps `run`'s driver has started.
///
/// The driver's own count, waited for rather than read optimistically: a driver
/// writes its counts on its first wait, so a read taken before that is a read of
/// a file that is not there yet rather than a driver that has swept nothing.
fn sweeps(world: &World, run: &str) -> u64 {
    crate::harness::reporting(world, run);
    crate::harness::counts(world, run).maintenance_sweeps
}

/// One of this suite's script fixtures, written into the world's scratch as the
/// executable this platform runs it as.
///
/// A `.bat` on Windows, with CRLF for the reason `harness::write_hook_script`
/// gives; the POSIX script everywhere else, marked executable.
fn interpreted_script(world: &World, stem: &str) -> String {
    #[cfg(windows)]
    {
        let path = world.root.join(format!("{stem}.bat"));
        let body = std::fs::read_to_string(repo_file(&format!("tests/e2e/{stem}.bat")))
            .expect("the fixture ships");
        std::fs::write(&path, body.replace('\n', "\r\n")).expect("the script is written");
        path.to_string_lossy().into_owned()
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = world.root.join(format!("{stem}.sh"));
        std::fs::copy(repo_file(&format!("tests/e2e/{stem}.sh")), &path)
            .expect("the fixture ships");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("the script is executable");
        path.to_string_lossy().into_owned()
    }
}

/// Size the host's pools and name the `service` identity's `maintain` command:
/// one warm slot, no overflow, and the marker-writing script — holding on
/// `hold` where a journey names one — under a generous bound. Every other
/// identity takes the shipped default, which names no maintenance at all.
fn pooled_with_maintenance(world: &World, hold: Option<&Path>) {
    pooled_with_maintenance_bounded(world, hold, "120s");
}

/// The same, under the bound given.
fn pooled_with_maintenance_bounded(world: &World, hold: Option<&Path>, timeout: &str) {
    let maintain = interpreted_script(world, "maintain");
    let mut command = vec![maintain];
    if let Some(hold) = hold {
        command.push(hold.to_string_lossy().into_owned());
    }
    pooled_with_command(
        world,
        &serde_json::to_string(&command).expect("an argv serializes"),
        timeout,
    );
}

/// Size the host's pools and name the `service` identity's `maintain` command
/// verbatim, as the argv's JSON.
fn pooled_with_command(world: &World, command: &str, timeout: &str) {
    pooled_with_commands(world, &[("service", command)], timeout);
}

fn pooled_with_commands(world: &World, named: &[(&str, &str)], timeout: &str) {
    let mut document = "version: 1\nrules:\n".to_owned();
    for (name, command) in named {
        document.push_str(&format!(
            "  - match: {{host: github.com, owner: owner, name: {name}}}\n    pool: 1\n    \
             overflow: 0\n    maintain:\n      command: {command}\n      timeout: {timeout}\n"
        ));
    }
    std::fs::write(world.onevcs_home().join("workspaces.yml"), document)
        .expect("the workspaces file is written");
}

/// Cut the identity's one slot: a real session opened and closed through the
/// linked library, which is the only thing that places a slot under a pool.
fn cut_a_slot(world: &World, checkout: &Path) {
    world.on_onevcs(|| {
        let vcs = onevcs::Providers::real().vcs;
        let session = vcs
            .open_session(onevcs::SessionRequest {
                repo: checkout.to_string_lossy().into_owned(),
                branch: None,
                base: None,
                execution_checkout: None,
                pool: None,
                overflow: None,
                labels: Default::default(),
            })
            .expect("a session opens on the pooled identity");
        vcs.close_session(&session.token)
            .expect("the session closes and hands the slot back");
    });
}

/// A schedule file with the given default `every`, and the rules given verbatim.
fn schedule(world: &World, name: &str, every: &str, rules: &str) -> String {
    let path = world.root.join(format!("{name}.yml"));
    std::fs::write(
        &path,
        format!("version: 1\ndefault:\n  every: {every}\n{rules}"),
    )
    .expect("the schedule is written");
    path.to_string_lossy().into_owned()
}

fn pool_status_of(world: &World, repo: &str) -> Value {
    let output = world
        .cmd_on(&onevcs_binary(), &["pool", "status", repo, "--json"])
        .output()
        .expect("onevcs runs");
    assert!(
        output.status.success(),
        "onevcs pool status refused: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("the status is JSON")
}

fn the_slot(world: &World) -> Value {
    the_slot_of(world, "service")
}

fn the_slot_of(world: &World, repo: &str) -> Value {
    let status = pool_status_of(world, repo);
    let slots = status["slots"].as_array().expect("slots");
    assert_eq!(slots.len(), 1, "{status}");
    slots[0].clone()
}

/// Everything the maintain command has left in the slot's worktree, one line per
/// run of it — each carrying the arguments that run was given.
fn marker_text(world: &World, repo: &str) -> String {
    let worktree = Path::new(
        the_slot_of(world, repo)["path"]
            .as_str()
            .expect("a slot path"),
    )
    .join("worktree");
    std::fs::read_to_string(worktree.join("maintained.log")).unwrap_or_default()
}

fn marker_lines(world: &World) -> usize {
    marker_text(world, "service").lines().count()
}

/// Every `pool-maintenance` record one run wrote.
fn records(world: &World, run: &str) -> Vec<Value> {
    world.events_of(run, "pool-maintenance")
}

/// Whether the driver's marker of a live sweep is on disk.
fn sweeping(world: &World, run: &str) -> bool {
    world.run_file(run, "maintenance.json").is_file()
}

/// Launch a run detached whose one direct node holds until released, on a
/// plan at `concurrency`, with the extra flags given.
fn held_run(world: &World, name: &str, concurrency: u64, extra: &[&str]) -> Run {
    world.script(&format!("{name}-build.wait"), "hold");
    let mut plan = plan_of(name, vec![agent(&format!("{name}-build"), &[])]);
    plan["concurrency"] = json!(concurrency);
    let path = world.plan(name, &plan);
    let mut args = vec!["start", path.as_str(), "--detach"];
    args.extend_from_slice(extra);
    world.run(&args)
}

/// Let a held run's node go, and wait for the run to settle.
fn release(world: &World, name: &str) {
    world.release(&format!("{name}-build.go"));
    world.until("the released run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });
}

/// Wait until `run`'s driver has made at least `n` sweeps.
fn until_sweeps(world: &World, run: &str, n: u64) {
    world.until(
        &format!("{run}'s driver to have swept the registry {n} time(s)"),
        |world| sweeps(world, run) >= n,
    );
}

/// The schedule is persistent rather than per run, and the driver's idle
/// capacity is what pays for it.
///
/// One slot, due because it has never been maintained. An idle driver — one
/// dispatch held, on a plan with room for two — sweeps on the schedule: the
/// marker appears, the sibling's `pool status` carries `last_maintained`, the
/// run's journal carries one `pool-maintenance` record with the slot's outcome,
/// and `status` names the sweep while its thread is live. Further idle ticks
/// inside `every` sweep the registry again — the sibling's call record says so
/// — and run nothing and write nothing. A fresh driver launched inside `every`
/// of the recorded stamp runs nothing and writes nothing, and one launched after
/// `every` has elapsed maintains the slot again: the only state is the slot's.
#[test]
fn an_idle_driver_maintains_a_due_slot_once_and_a_fresh_driver_inside_every_runs_nothing() {
    let world = pooled_world("maintenance-idle", Some(FAST_PACE));
    let repo = world.repository("local-direct", &[]);
    let hold = world.root.join("maintain.go");
    pooled_with_maintenance(&world, Some(&hold));
    cut_a_slot(&world, &repo.checkout);
    assert_eq!(the_slot(&world)["last_maintained"], Value::Null);
    let thirty = schedule(&world, "thirty", "30s", "");

    // The first driver: idle beside its one held dispatch.
    held_run(&world, "first", 2, &["--maintenance-config", &thirty]).exited(0);
    let launch = world.run_json("first", "launch.json");
    assert_eq!(launch["maintenance_config"]["version"], 1, "{launch}");
    assert_eq!(
        launch["maintenance_config"]["default"]["every"], "30s",
        "{launch}"
    );
    assert!(
        launch["maintenance_config"].get("rules").is_none(),
        "{launch}"
    );

    // The sweep starts, and its command holds: `status` names it, and the
    // sibling reports the slot claimed for maintenance.
    world.until("the driver's sweep to be live", |world| {
        sweeping(world, "first")
    });
    world.until("the sibling to report the slot maintaining", |world| {
        the_slot(world)["state"]["state"] == "maintaining"
    });
    world
        .run(&["status", "first"])
        .exited(0)
        .out_has("pool maintenance: a sweep of the host's worktree pools is in progress");
    assert!(records(&world, "first").is_empty(), "{}", world.dump());

    // Released, the command writes its marker and the sweep ends with one
    // record carrying the slot's outcome.
    std::fs::write(&hold, "go").expect("the hold is released");
    world.until("the maintenance record", |world| {
        !records(world, "first").is_empty()
    });
    let record = &records(&world, "first")[0];
    let identities = record["payload"]["identities"]
        .as_array()
        .expect("identities");
    assert_eq!(identities.len(), 1, "{record}");
    assert_eq!(identities[0]["identity"], SERVICE_IDENTITY, "{record}");
    assert_eq!(identities[0]["every"], "30s", "{record}");
    let slots = identities[0]["outcome"]["slots"].as_array().expect("slots");
    assert_eq!(slots.len(), 1, "{record}");
    assert_eq!(slots[0]["number"], 1, "{record}");
    assert_eq!(
        slots[0]["outcome"]["ran"]["outcome"], "succeeded",
        "{record}"
    );
    assert!(
        slots[0]["outcome"]["ran"]["duration_ms"].is_u64(),
        "{record}"
    );
    assert!(record["payload"].get("error").is_none(), "{record}");
    assert_eq!(marker_lines(&world), 1);
    let slot = the_slot(&world);
    assert!(slot["last_maintained"].is_string(), "{slot}");
    assert_eq!(slot["last_outcome"], "succeeded", "{slot}");
    assert_eq!(slot["state"]["state"], "idle", "{slot}");
    // Polled rather than read once: the pace is one second and the sweep above
    // held for longer, so the next idle pass may already have started a
    // not-due sweep by the time `status` is asked, and names it while it is live.
    world.until("`status` to stop naming a sweep", |world| {
        !sweeping(world, "first") && {
            let status = world.run(&["status", "first"]);
            !status.exited(0).stdout.contains("pool maintenance")
        }
    });
    world
        .run(&["results", "first"])
        .exited(0)
        .out_has("pool maintenance: last sweep that did something started")
        .out_has(&format!(
            "{SERVICE_IDENTITY} (every 30s): slot 1 ran — succeeded in"
        ));

    // Further idle ticks inside `every`: the registry is swept again, and the
    // sibling answers not-due — nothing runs, nothing is written.
    let swept = sweeps(&world, "first");
    until_sweeps(&world, "first", swept + 2);
    assert_eq!(marker_lines(&world), 1);
    assert_eq!(records(&world, "first").len(), 1, "{}", world.dump());
    let stamped = the_slot(&world)["last_maintained"].clone();
    release(&world, "first");
    assert_eq!(records(&world, "first").len(), 1, "{}", world.dump());
    world
        .run(&["results", "first"])
        .exited(0)
        .out_has(&format!("{SERVICE_IDENTITY} (every 30s): slot 1 ran"));

    // A fresh driver inside `every` of the recorded stamp: it sweeps, and the
    // slot's own stamp says not due.
    let hour = schedule(&world, "hour", "1h", "");
    held_run(&world, "second", 2, &["--maintenance-config", &hour]).exited(0);
    let swept = sweeps(&world, "second");
    until_sweeps(&world, "second", swept + 2);
    assert!(records(&world, "second").is_empty(), "{}", world.dump());
    assert_eq!(marker_lines(&world), 1);
    assert_eq!(the_slot(&world)["last_maintained"], stamped);
    release(&world, "second");
    assert!(records(&world, "second").is_empty(), "{}", world.dump());

    // And one launched after `every` has elapsed maintains the slot again.
    let second = schedule(&world, "second", "1s", "");
    std::thread::sleep(std::time::Duration::from_millis(1_100));
    held_run(&world, "third", 2, &["--maintenance-config", &second]).exited(0);
    world.until("the third driver's record", |world| {
        !records(world, "third").is_empty()
    });
    assert_eq!(marker_lines(&world), 2);
    assert_ne!(the_slot(&world)["last_maintained"], stamped);
    release(&world, "third");
}

/// Two drivers idle at once on one state root maintain the slot once between
/// them: the second meets the first inside the identity and is answered
/// `claimed`, writing no record of a run.
#[test]
fn two_drivers_idle_at_once_maintain_the_slot_once_between_them() {
    let world = pooled_world("maintenance-two", Some(FAST_PACE));
    let repo = world.repository("local-direct", &[]);
    let hold = world.root.join("maintain.go");
    pooled_with_maintenance(&world, Some(&hold));
    cut_a_slot(&world, &repo.checkout);
    let thirty = schedule(&world, "thirty", "30s", "");

    held_run(&world, "one", 2, &["--maintenance-config", &thirty]).exited(0);
    world.until("the first driver to claim the slot", |world| {
        the_slot(world)["state"]["state"] == "maintaining"
    });
    held_run(&world, "two", 2, &["--maintenance-config", &thirty]).exited(0);
    world.until("the second driver to be answered claimed", |world| {
        !records(world, "two").is_empty()
    });
    let claimed = &records(&world, "two")[0];
    let identities = claimed["payload"]["identities"]
        .as_array()
        .expect("identities");
    assert_eq!(identities.len(), 1, "{claimed}");
    assert_eq!(identities[0]["identity"], SERVICE_IDENTITY, "{claimed}");
    assert!(
        identities[0]["outcome"]["claimed"]["by_pid"].is_u64(),
        "{claimed}"
    );
    assert_eq!(marker_lines(&world), 0);

    std::fs::write(&hold, "go").expect("the hold is released");
    world.until("the first driver's record", |world| {
        !records(world, "one").is_empty()
    });
    let ran = &records(&world, "one")[0];
    assert_eq!(
        ran["payload"]["identities"][0]["outcome"]["slots"][0]["outcome"]["ran"]["outcome"],
        "succeeded",
        "{ran}"
    );
    assert_eq!(marker_lines(&world), 1);
    world
        .run(&["results", "two"])
        .exited(0)
        .out_has("claimed — another pool maintain (pid");

    // Every further sweep of either driver is answered not-due. Counted per driver,
    // because each keeps its own account of what its loop did — and waited for
    // **together** rather than in turn: two waits in sequence take twice the
    // wall-clock of one, and what this then asserts is that the slot was not
    // maintained again, which stops being true the moment `every` elapses. A wait
    // whose length decides its own assertion is a wait that passes on an idle host
    // and fails on a loaded one.
    let before: Vec<u64> = ["one", "two"]
        .iter()
        .map(|driver| sweeps(&world, driver))
        .collect();
    world.until(
        "both drivers to have swept the registry twice more",
        |world| {
            ["one", "two"]
                .iter()
                .zip(&before)
                .all(|(driver, swept)| sweeps(world, driver) >= swept + 2)
        },
    );
    assert_eq!(marker_lines(&world), 1);
    assert_eq!(records(&world, "one").len(), 1, "{}", world.dump());

    // Until the first driver's command exits — on a slow host a pace or more
    // after the hold is released — a sweep of the second meets the claim again
    // and is answered `claimed` again. So its records may be several, but each
    // is `claimed`, from a sweep started before the first driver recorded.
    let recorded_at = ran["ts"].as_str().expect("a record's timestamp");
    for met in records(&world, "two") {
        let started = met["payload"]["started_at"]
            .as_str()
            .expect("a sweep's start");
        assert!(
            met["payload"]["identities"][0]["outcome"]["claimed"].is_object()
                && started < recorded_at,
            "{}",
            world.dump()
        );
    }
    release(&world, "one");
    release(&world, "two");
}

/// Maintenance is withheld — no thread, no process, no record — while a pass is
/// not idle: the run at its own concurrency ceiling, or the local executor
/// reporting no free slot. And a launch naming no schedule sweeps nothing,
/// whatever its capacity.
#[test]
fn maintenance_is_withheld_at_the_ceiling_without_a_free_slot_and_under_no_schedule() {
    let world = pooled_world("maintenance-withheld", Some(FAST_PACE));
    let repo = world.repository("local-direct", &[]);
    pooled_with_maintenance(&world, None);
    cut_a_slot(&world, &repo.checkout);
    let thirty = schedule(&world, "thirty", "30s", "");

    // Long enough for several paces to have come and gone.
    let paces = std::time::Duration::from_secs(4);

    // At the ceiling: one dispatch on a plan that admits one.
    held_run(&world, "ceiling", 1, &["--maintenance-config", &thirty]).exited(0);
    world.until("the dispatch", |world| {
        !world.events_of("ceiling", "node-dispatched").is_empty()
    });
    std::thread::sleep(paces);
    assert_eq!(
        sweeps(&world, "ceiling"),
        0,
        "a driver at its ceiling swept the registry"
    );
    assert!(!sweeping(&world, "ceiling"));
    assert_eq!(marker_lines(&world), 0);
    release(&world, "ceiling");
    assert_eq!(sweeps(&world, "ceiling"), 0);
    assert!(records(&world, "ceiling").is_empty(), "{}", world.dump());

    // No free slot: a load average of every core the host has, which the
    // executor reports as no slot free — the dispatch still falls back onto the
    // local executor, and the sweep does not.
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    let mut full = world.as_session(&world.session);
    full.environment.push((
        onepipeline::executor::LOAD1_ENV.to_owned(),
        cores.to_string(),
    ));
    held_run(&full, "full", 2, &["--maintenance-config", &thirty]).exited(0);
    full.until("the dispatch", |world| {
        !world.events_of("full", "node-dispatched").is_empty()
    });
    std::thread::sleep(paces);
    assert_eq!(
        sweeps(&full, "full"),
        0,
        "a driver with no free slot swept the registry"
    );
    assert!(!sweeping(&full, "full"));
    assert_eq!(marker_lines(&full), 0);
    release(&full, "full");
    assert!(records(&full, "full").is_empty(), "{}", full.dump());

    // No schedule: nothing, whatever the capacity.
    held_run(&world, "unscheduled", 2, &[]).exited(0);
    let launch = world.run_json("unscheduled", "launch.json");
    assert!(launch.get("maintenance_config").is_none(), "{launch}");
    world.until("the dispatch", |world| {
        !world.events_of("unscheduled", "node-dispatched").is_empty()
    });
    std::thread::sleep(paces);
    assert_eq!(
        sweeps(&world, "unscheduled"),
        0,
        "a driver naming no schedule swept the registry"
    );
    assert!(!sweeping(&world, "unscheduled"));
    assert_eq!(marker_lines(&world), 0);
    release(&world, "unscheduled");
    assert!(
        records(&world, "unscheduled").is_empty(),
        "{}",
        world.dump()
    );
    world
        .run(&["results", "unscheduled"])
        .exited(0)
        .out_lacks("pool maintenance");
    assert_eq!(the_slot(&world)["last_maintained"], Value::Null);
}

/// Successive sweeps respect the driver's in-memory pace: with every identity
/// answering not-due the first sweep finishes at once, and the idle passes that
/// follow inside the shipped pace start no second thread and sweep the registry
/// no second time — read off the driver's own pass counts and the sibling's
/// call record rather than a clock.
///
/// Also the identity that names no maintain command: it is visited inside the
/// one thread and skipped, with no process and no record for it.
#[test]
fn idle_passes_inside_the_pace_sweep_no_second_time_and_an_unmaintained_identity_is_skipped() {
    let world =
        pooled_world("maintenance-pace", None).with_env(crate::harness::LOOP_STATS_ENV, "1");
    let repo = world.repository("local-direct", &[]);
    let other = world.extra_repository("other");
    pooled_with_maintenance(&world, None);
    cut_a_slot(&world, &repo.checkout);
    // Stamped by the sibling's own verb, so the driver's first sweep finds every
    // slot not due.
    let stamped = world
        .cmd_on(&onevcs_binary(), &["pool", "maintain", "service"])
        .output()
        .expect("onevcs runs");
    assert!(
        stamped.status.success(),
        "{}",
        String::from_utf8_lossy(&stamped.stderr)
    );
    assert_eq!(marker_lines(&world), 1);
    let _ = other;
    let hour = schedule(&world, "hour", "1h", "");

    held_run(&world, "paced", 2, &["--maintenance-config", &hour]).exited(0);
    until_sweeps(&world, "paced", 1);
    world.until("the sweep to end", |world| !sweeping(world, "paced"));
    crate::harness::reporting(&world, "paced");
    let before = crate::harness::counts(&world, "paced");

    // Idle passes, each provoked by a channel command the loop consumes and
    // then sits idle after.
    for nth in 0..3 {
        world
            .run_with_stdin(
                &["reply", "paced"],
                &json!({"version": 2, "commands": [
                    {"op": "note", "id": "paced-build", "addressee": "worker",
                     "text": format!("idle pass {nth}"), "deliver": "next"}
                ]})
                .to_string(),
            )
            .exited(0);
        world.until("the loop to pass again", |world| {
            crate::harness::counts(world, "paced").since(before).passes > nth
        });
    }
    assert!(
        crate::harness::counts(&world, "paced").since(before).passes >= 3,
        "{:?}",
        crate::harness::counts(&world, "paced")
    );
    assert_eq!(
        sweeps(&world, "paced"),
        1,
        "the idle passes inside the pace swept again"
    );
    assert!(records(&world, "paced").is_empty(), "{}", world.dump());
    assert_eq!(marker_lines(&world), 1);
    release(&world, "paced");
    assert_eq!(sweeps(&world, "paced"), 1);
    assert!(records(&world, "paced").is_empty(), "{}", world.dump());
}

/// A rule's `every` beats the default for the identity it matches, resolved
/// through the linked sibling's matcher: a default measured in days and a rule
/// of one second for `service` maintains the slot the rule names.
#[test]
fn a_rules_every_beats_the_default_for_the_identity_it_matches() {
    let world = pooled_world("maintenance-rule", Some(FAST_PACE));
    let repo = world.repository("local-direct", &[]);
    pooled_with_maintenance(&world, None);
    cut_a_slot(&world, &repo.checkout);
    // Stamped now, so only a cadence shorter than the default reaches it.
    let stamped = world
        .cmd_on(&onevcs_binary(), &["pool", "maintain", "service"])
        .output()
        .expect("onevcs runs");
    assert!(stamped.status.success());
    assert_eq!(marker_lines(&world), 1);

    // The default would never reach it; a rule for another identity says
    // nothing about it; the rule that matches it does.
    let ruled = schedule(
        &world,
        "ruled",
        "7d",
        "rules:\n  - match: {host: github.com, owner: owner, name: other}\n    every: 1s\n  - \
         match: {host: github.com, owner: owner, name: service}\n    every: 1s\n",
    );
    std::thread::sleep(std::time::Duration::from_millis(1_100));
    held_run(&world, "ruled", 2, &["--maintenance-config", &ruled]).exited(0);
    let launch = world.run_json("ruled", "launch.json");
    assert_eq!(
        launch["maintenance_config"]["rules"][1]["match"],
        json!({"host": "github.com", "owner": "owner", "name": "service"}),
        "{launch}"
    );
    world.until("the record", |world| !records(world, "ruled").is_empty());
    let record = &records(&world, "ruled")[0];
    assert_eq!(
        record["payload"]["identities"][0]["every"], "1s",
        "{record}"
    );
    assert_eq!(marker_lines(&world), 2);
    release(&world, "ruled");

    // And under the default alone the slot is left as it is.
    let unruled = schedule(&world, "unruled", "7d", "");
    held_run(&world, "unruled", 2, &["--maintenance-config", &unruled]).exited(0);
    let swept = sweeps(&world, "unruled");
    until_sweeps(&world, "unruled", swept + 2);
    assert!(records(&world, "unruled").is_empty(), "{}", world.dump());
    assert_eq!(marker_lines(&world), 2);
    release(&world, "unruled");
}

/// The flag and the key: the flag beats the config even when blank, a blank key
/// names none, the parsed document is retained and replayed by `adopt` — which
/// takes no flag — and a document this build does not accept is refused before
/// a run is minted, naming the key at fault.
#[test]
fn the_flag_beats_the_key_a_blank_names_none_and_a_bad_schedule_is_refused_before_a_run() {
    let world = pooled_world("maintenance-launch", None);
    let repo = world.repository("local-direct", &[]);
    pooled_with_maintenance(&world, None);
    cut_a_slot(&world, &repo.checkout);
    let at = onepipeline::filter::LAUNCH_CONFIG_SCHEMA_VERSION;
    let hour = schedule(&world, "hour", "1h", "");
    let config = world.root.join("launch.yaml");
    std::fs::write(
        &config,
        format!("schema_version: {at}\nmaintenance_config: {hour:?}\n"),
    )
    .expect("the config is written");
    let config = config.to_string_lossy().into_owned();
    let plan = |name: &str| world.plan(name, &plan_of(name, vec![agent("build", &[])]));

    // A blank flag names none over the config's schedule.
    let path = plan("blanked");
    world
        .run(&[
            "start",
            &path,
            "--attach",
            "--launch-config",
            &config,
            "--maintenance-config",
            "",
        ])
        .exited(0)
        .settled();
    assert!(
        world
            .run_json("blanked", "launch.json")
            .get("maintenance_config")
            .is_none(),
        "{}",
        world.dump()
    );

    // The config alone names it, retained as the parsed document. This run is
    // idle beside its one quick dispatch, so its driver sweeps — and, the run
    // ending under it, closes out only once the sweep is joined and its record
    // written: a driver closing out waits for the sweep it started.
    let path = plan("configured");
    world
        .run(&["start", &path, "--attach", "--launch-config", &config])
        .exited(0)
        .settled();
    let launch = world.run_json("configured", "launch.json");
    assert_eq!(
        launch["maintenance_config"],
        json!({"version": 1, "default": {"every": "1h"}}),
        "{launch}"
    );
    assert_eq!(sweeps(&world, "configured"), 1, "{}", world.dump());
    assert_eq!(records(&world, "configured").len(), 1, "{}", world.dump());
    assert_eq!(marker_lines(&world), 1);
    assert!(!sweeping(&world, "configured"));

    // A blank key names none, as a blank flag does.
    let blank = world.root.join("blank.yaml");
    std::fs::write(
        &blank,
        format!("schema_version: {at}\nmaintenance_config: \"  \"\n"),
    )
    .expect("the config is written");
    let path = plan("blankkey");
    world
        .run(&[
            "start",
            &path,
            "--attach",
            "--launch-config",
            &blank.to_string_lossy(),
        ])
        .exited(0)
        .settled();
    assert!(world
        .run_json("blankkey", "launch.json")
        .get("maintenance_config")
        .is_none());

    // Refused before a run is minted, naming the key at fault, by either spelling.
    let path = plan("refused");
    let bad = |name: &str, body: &str| -> String {
        let path = world.root.join(format!("{name}.yml"));
        std::fs::write(&path, body).expect("the schedule is written");
        path.to_string_lossy().into_owned()
    };
    for (document, names) in [
        (
            bad(
                "unknown",
                "version: 1\ndefault:\n  every: 7d\nat: \"03:00\"\n",
            ),
            "unknown field `at`",
        ),
        (
            bad("versioned", "version: 2\ndefault:\n  every: 7d\n"),
            "`version` is 2",
        ),
        (
            bad(
                "unmatched",
                "version: 1\ndefault:\n  every: 7d\nrules:\n  - match: {}\n    every: 1d\n",
            ),
            "`match` names no field",
        ),
        (
            bad("spanless", "version: 1\ndefault:\n  every: 7x\n"),
            "`every`",
        ),
    ] {
        world
            .run_from(
                &world.project,
                &["start", &path, "--maintenance-config", &document],
            )
            .exited(REFUSED)
            .err_has("--maintenance-config")
            .err_has(names);
        let keyed = world.root.join("keyed.yaml");
        std::fs::write(
            &keyed,
            format!("schema_version: {at}\nmaintenance_config: {document:?}\n"),
        )
        .expect("the config is written");
        world
            .run_from(
                &world.project,
                &["start", &path, "--launch-config", &keyed.to_string_lossy()],
            )
            .exited(REFUSED)
            .err_has("maintenance_config")
            .err_has(names);
    }
    // And the key under every earlier schema version this build still reads,
    // refused by name and pointed at the version that admits it.
    for earlier in onepipeline::filter::LAUNCH_CONFIG_SCHEMA_VERSIONS_READ
        .iter()
        .filter(|version| **version < at)
    {
        let early = world.root.join(format!("early-{earlier}.yaml"));
        std::fs::write(
            &early,
            format!("schema_version: {earlier}\nmaintenance_config: {hour:?}\n"),
        )
        .expect("the config is written");
        world
            .run_from(
                &world.project,
                &["start", &path, "--launch-config", &early.to_string_lossy()],
            )
            .exited(REFUSED)
            .err_has(&format!(
                "`maintenance_config` is a schema {at} key and this config declares \
                 schema_version {earlier}"
            ));
    }
    assert!(!world.run_file("refused", "launch.json").is_file());

    // `adopt` takes no flag, and replays what the record retained: the adopting
    // driver sweeps on the launch's schedule.
    world
        .run(&["adopt", "configured", "--maintenance-config", &hour])
        .exited(USAGE_ERROR)
        .err_has("--maintenance-config");
    let path = world.plan(
        "adopted",
        &plan_of(
            "adopted",
            vec![
                crate::harness::human("approve", &[]),
                agent("build", &["approve"]),
            ],
        ),
    );
    world
        .run(&["start", &path, "--attach", "--launch-config", &config])
        .settled();
    // A fresh run, so a fresh driver and a fresh account of what its loop did.
    assert_eq!(
        sweeps(&world, "adopted"),
        0,
        "a run paused on a human gate swept the registry"
    );
    world.run(&["attest", "adopted", "approve"]).exited(0);
    world.script("build.wait", "hold");
    let adopting = world
        .cmd(&["adopt", "adopted"])
        .spawn()
        .expect("adopt starts");
    // The adopting driver is a second process, and the counts are one process's
    // own: it opens the account at nothing and sweeps once.
    until_sweeps(&world, "adopted", 1);
    world.release("build.go");
    crate::harness::ended(adopting);
    assert_eq!(
        world.run_json("adopted", "launch.json")["maintenance_config"]["default"]["every"],
        "1h"
    );
}

/// What a sweep records when it could not do its work — each one record, each
/// named by `results`: a maintain command that failed, one that timed out under
/// the identity's own bound, an identity the sibling could not maintain, and a
/// host whose identities could not be enumerated at all.
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] This
// src/maintenance.rs journey has no narrower Nx edge: noteJourneySource includes src/**/*.
#[test]
fn a_failed_or_timed_out_command_an_unmaintainable_identity_and_an_unlistable_host_are_recorded() {
    let world = pooled_world("maintenance-failures", Some(FAST_PACE))
        .with_env("ONEPIPELINE_E2E_MAINTAIN_EXIT", "3");
    let repo = world.repository("local-direct", &[]);
    pooled_with_maintenance(&world, None);
    cut_a_slot(&world, &repo.checkout);
    let second = schedule(&world, "second", "1s", "");

    // A command that failed: recorded as the sibling records it, and — the
    // attempt being what is stamped — not run again inside `every`.
    held_run(&world, "failing", 2, &["--maintenance-config", &second]).exited(0);
    world.until("the record of the failure", |world| {
        !records(world, "failing").is_empty()
    });
    let failed = &records(&world, "failing")[0];
    assert_eq!(
        failed["payload"]["identities"][0]["outcome"]["slots"][0]["outcome"]["ran"]["outcome"],
        json!({"failed": {"exit": 3}}),
        "{failed}"
    );
    assert_eq!(marker_lines(&world), 1);
    assert_eq!(
        the_slot(&world)["last_outcome"],
        json!({"failed": {"exit": 3}})
    );
    world
        .run(&["results", "failing"])
        .exited(0)
        .out_has(&format!(
            "{SERVICE_IDENTITY} (every 1s): slot 1 ran — failed (exit 3) in"
        ));
    release(&world, "failing");

    // A command that timed out under the identity's own bound: a hold nothing
    // releases, bounded at a second.
    let hold = world.root.join("never.go");
    pooled_with_maintenance_bounded(&world, Some(&hold), "1s");
    std::thread::sleep(std::time::Duration::from_millis(1_100));
    held_run(&world, "timing", 2, &["--maintenance-config", &second]).exited(0);
    world.until("the record of the timeout", |world| {
        !records(world, "timing").is_empty()
    });
    let timed = &records(&world, "timing")[0];
    assert_eq!(
        timed["payload"]["identities"][0]["outcome"]["slots"][0]["outcome"]["ran"]["outcome"],
        "timed-out",
        "{timed}"
    );
    assert_eq!(
        marker_lines(&world),
        1,
        "a timed-out command wrote its marker"
    );
    world
        .run(&["results", "timing"])
        .exited(0)
        .out_has(&format!(
            "{SERVICE_IDENTITY} (every 1s): slot 1 ran — timed out in"
        ));
    release(&world, "timing");

    // An identity the sibling could not maintain: a workspaces file it refuses —
    // a `maintain.command` that is the empty sequence, which names no program to
    // spawn — is that identity's error, and the sweep's one record carries it,
    // naming the key an operator edits.
    pooled_with_command(&world, "[]", "120s");
    std::thread::sleep(std::time::Duration::from_millis(1_100));
    held_run(
        &world,
        "unmaintainable",
        2,
        &["--maintenance-config", &second],
    )
    .exited(0);
    world.until("the record of the refusal", |world| {
        !records(world, "unmaintainable").is_empty()
    });
    let refused = &records(&world, "unmaintainable")[0];
    let identity = &refused["payload"]["identities"][0];
    assert_eq!(identity["identity"], SERVICE_IDENTITY, "{refused}");
    assert!(identity.get("outcome").is_none(), "{refused}");
    let error = identity["error"].as_str().expect("an error");
    assert!(error.contains("maintain.command"), "{refused}");
    assert!(error.contains("empty list"), "{refused}");
    world
        .run(&["results", "unmaintainable"])
        .exited(0)
        .out_has(&format!("{SERVICE_IDENTITY} (every 1s): failed — "));
    release(&world, "unmaintainable");

    // A host whose registered identities could not be enumerated: the registry
    // the sibling keys them by is not a document it can read while the sweep
    // asks, and the record says so with no identity at all.
    //
    // Made unreadable after the dispatch is placed and put back before the run is
    // let go, because that one document is what every other thing a run does over
    // a repository resolves through — so a world that never had it would have no
    // run to sweep beside.
    //
    // llmlint: ignore-block[tests_mirror_real_usage] the registry is written to directly
    // because no command of the sibling's leaves a host with one it cannot read: every
    // verb that writes that document writes a valid one, and `register` is how a host
    // arrives at a *readable* registry rather than this. What is arranged is the state a
    // truncated write or a half-restored state root hands the loader, which is the state
    // the sweep's refusal exists for; everything asserted about it is read back off the
    // compiled binary — its own journal record and its own `results`.
    pooled_with_maintenance(&world, None);
    let registry = world.onevcs_home().join("registry.json");
    let saved = std::fs::read_to_string(&registry).expect("the host has a registry");
    held_run(&world, "unlistable", 2, &["--maintenance-config", &second]).exited(0);
    world.until("the dispatch", |world| {
        !world.events_of("unlistable", "node-dispatched").is_empty()
    });
    std::fs::write(&registry, "{ not a registry").expect("the registry is made unreadable");
    let failed = |record: &Value| record["payload"].get("error").is_some();
    world.until("the record of the enumeration failure", |world| {
        records(world, "unlistable").iter().any(failed)
    });
    let unlisted = records(&world, "unlistable")
        .into_iter()
        .find(failed)
        .expect("the record of the enumeration failure");
    assert_eq!(unlisted["payload"]["identities"], json!([]), "{unlisted}");
    let error = unlisted["payload"]["error"].as_str().expect("an error");
    assert!(
        error.contains("registered identities could not be read"),
        "{unlisted}"
    );
    // Read back while the registry is still unreadable, so the last sweep that
    // did something is this one.
    world
        .run(&["results", "unlistable"])
        .exited(0)
        .out_has("the host's identities could not be enumerated:");
    std::fs::write(&registry, &saved).expect("the registry is put back");
    release(&world, "unlistable");
} // llmlint: ignore-end[tests_mirror_real_usage]

/// A slot something else holds right now is reported **busy**, and one whose
/// worktree is gone is reported **broken**.
///
/// The one distinction `onevcs` added the `unavailable` outcome for: a healthy
/// slot a later sweep finds clear, against a clone, worktree or record that is
/// not usable and that nothing waiting clears. Reported as breakage, a busy pool
/// sends a supervisor looking for damaged worktrees on a host where nothing is
/// wrong, so the two are driven apart here rather than only held apart in a unit
/// test.
///
/// The slot's occupancy lease is taken by this journey, for the reason
/// `session_reuse.rs` gives for taking a run root's. The name is computed the
/// way `onevcs` computes it and the file has to already exist, which catches a
/// rename; and what proves it is still the lock occupancy is decided by is the
/// outcome itself — a sweep that claimed the slot anyway would run the command
/// and be recorded as having run it.
// llmlint: ignore-block[tests_mirror_real_usage] the lease alone, for the reason above:
// there is no command that leaves a slot occupied past its own exit, so there is no
// interface to reach this state through. Everything else here is the compiled binary
// driven the way a host drives it.
#[test]
fn a_slot_another_process_holds_is_busy_and_one_whose_worktree_is_gone_is_broken() {
    use sha2::{Digest, Sha256};

    let world = pooled_world("maintenance-busy", Some(FAST_PACE));
    let repo = world.repository("local-direct", &[]);
    pooled_with_maintenance(&world, None);
    cut_a_slot(&world, &repo.checkout);
    let slot = the_slot(&world);
    let run_root = slot["path"].as_str().expect("a slot path").to_owned();

    let lease = format!("run:{run_root}");
    let digest: String = Sha256::digest(lease.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let path: PathBuf = world
        .onevcs_home()
        .join("locks")
        .join(format!("{digest}.lock"));
    assert!(
        path.is_file(),
        "onevcs keeps no lease for {lease} at {}; the lock it names a slot's occupancy \
         after has moved, and this journey is no longer holding one",
        path.display()
    );
    let held = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap_or_else(|e| panic!("cannot open the lease at {}: {e}", path.display()));
    assert!(
        matches!(fs4::fs_std::FileExt::try_lock_exclusive(&held), Ok(true)),
        "the lease at {} is already held, so this journey never made the slot busy",
        path.display()
    );

    assert_eq!(slot["last_maintained"], Value::Null, "{slot}");
    let second = schedule(&world, "busy", "1s", "");
    held_run(&world, "busy", 2, &["--maintenance-config", &second]).exited(0);
    world.until("the record of the busy slot", |world| {
        !records(world, "busy").is_empty()
    });
    let record = &records(&world, "busy")[0];
    let outcome = &record["payload"]["identities"][0]["outcome"]["slots"][0]["outcome"];
    let holder = outcome["unavailable"]["holder"]
        .as_str()
        .unwrap_or_else(|| panic!("the slot was not reported unavailable: {record}"));
    assert!(holder.contains("working in it"), "{record}");
    assert!(outcome.get("broken").is_none(), "{record}");
    assert!(outcome.get("ran").is_none(), "{record}");

    world
        .run(&["results", "busy"])
        .exited(0)
        .out_has(&format!(
            "{SERVICE_IDENTITY} (every 1s): slot 1 kept: busy — "
        ))
        .out_lacks("kept: broken");

    // Nothing ran, nothing was stamped, and the sibling still calls the slot
    // healthy — which is what separates busy from broken.
    assert_eq!(marker_lines(&world), 0);
    let after = the_slot(&world);
    assert_eq!(after["last_maintained"], Value::Null, "{after}");
    assert_eq!(after["state"]["state"], "idle", "{after}");

    drop(held);
    world.until("the slot to be maintained once the lease goes", |world| {
        marker_lines(world) == 1
    });

    // The other half of the distinction, arranged the way a host arrives at it and
    // not by forging a record: the slot's worktree is gone — a scratch directory
    // cleaned out, a clone that never finished — so the next sweep that finds the
    // slot due finds it unusable and says broken rather than busy. Nothing waiting
    // clears this one, which is what the two words are for.
    std::fs::remove_dir_all(Path::new(&run_root).join("worktree"))
        .expect("the slot's worktree is removed");
    let broke = |record: &Value| {
        record["payload"]["identities"][0]["outcome"]["slots"][0]["outcome"]
            .get("broken")
            .is_some()
    };
    world.until("the record of the broken slot", |world| {
        records(world, "busy").iter().any(broke)
    });
    let record = records(&world, "busy")
        .into_iter()
        .find(broke)
        .expect("the record of the broken slot");
    let reason = record["payload"]["identities"][0]["outcome"]["slots"][0]["outcome"]["broken"]
        ["reason"]
        .as_str()
        .expect("a reason");
    assert!(reason.contains("worktree"), "{record}");
    world
        .run(&["results", "busy"])
        .exited(0)
        .out_has(&format!(
            "{SERVICE_IDENTITY} (every 1s): slot 1 kept: broken — "
        ))
        .out_lacks("kept: busy");
    release(&world, "busy");
} // llmlint: ignore-end[tests_mirror_real_usage]

#[test]
fn every_registered_identity_is_discovered_and_maintained_in_one_sweep() {
    let world = pooled_world("maintenance-identities", Some(FAST_PACE));
    let service = world.repository("local-direct", &[]);
    let other = world.extra_repository("other");
    let third = world.extra_repository("third");
    let maintain = interpreted_script(&world, "maintain");
    let command = serde_json::to_string(&[maintain.as_str()]).expect("an argv serializes");
    pooled_with_commands(
        &world,
        &[
            ("service", command.as_str()),
            ("other", command.as_str()),
            ("third", command.as_str()),
        ],
        "120s",
    );
    for checkout in [&service.checkout, &other.checkout, &third.checkout] {
        cut_a_slot(&world, checkout);
    }

    let second = schedule(&world, "all", "1s", "");
    held_run(&world, "all", 2, &["--maintenance-config", &second]).exited(0);
    world.until("the record of the sweep", |world| {
        !records(world, "all").is_empty()
    });
    let record = &records(&world, "all")[0];
    let identities = record["payload"]["identities"]
        .as_array()
        .expect("identities");
    assert_eq!(
        identities
            .iter()
            .map(|entry| entry["identity"].as_str().expect("a key"))
            .collect::<Vec<_>>(),
        [
            "github.com/owner/other",
            SERVICE_IDENTITY,
            "github.com/owner/third"
        ],
        "{record}"
    );
    for entry in identities {
        assert_eq!(
            entry["outcome"]["slots"][0]["outcome"]["ran"]["outcome"], "succeeded",
            "{record}"
        );
    }
    for repo in ["service", "other", "third"] {
        assert_eq!(
            marker_text(&world, repo).lines().count(),
            1,
            "{repo} was not maintained"
        );
        let slot = the_slot_of(&world, repo);
        assert!(slot["last_maintained"].is_string(), "{repo}: {slot}");
    }
    release(&world, "all");
}

/// A non-empty command is spawned with the program and the argument the document
/// named, and neither is mangled on the way.
///
/// `maintain.command` is an argv — a program beside its arguments — spawned with no
/// shell, and what a journey has to see is that both elements arrive. So the
/// argument is a path **with a space in it**, and the program's only way past its
/// own wait is to have received that path byte for byte as one argument: split on
/// the space, or quoted, or shell-expanded, it would be a program waiting for a
/// file nothing will ever write, which the sibling records as a maintenance that
/// failed rather than one that ran.
///
/// Read off the sibling's own account in both directions — the slot claimed for
/// maintenance while the program waits, and a run that succeeded once the path is
/// written — so the claim rests on what the spawn did rather than on anything the
/// program was made to print about itself.
#[test]
fn a_non_empty_command_runs_with_the_program_and_argument_the_document_named() {
    let world = pooled_world("maintenance-argv", Some(FAST_PACE));
    let repo = world.repository("local-direct", &[]);
    let hold = world.root.join("release this sweep.go");
    pooled_with_maintenance(&world, Some(&hold));
    cut_a_slot(&world, &repo.checkout);
    let second = schedule(&world, "argv", "1s", "");
    held_run(&world, "argv", 2, &["--maintenance-config", &second]).exited(0);

    world.until("the sibling to report the slot maintaining", |world| {
        the_slot(world)["state"]["state"] == "maintaining"
    });
    assert_eq!(marker_lines(&world), 0);
    assert!(records(&world, "argv").is_empty(), "{}", world.dump());

    std::fs::write(&hold, "go").expect("the hold is released");
    world.until("the record of the run", |world| {
        !records(world, "argv").is_empty()
    });
    let record = &records(&world, "argv")[0];
    assert_eq!(
        record["payload"]["identities"][0]["outcome"]["slots"][0]["outcome"]["ran"]["outcome"],
        "succeeded",
        "{record}"
    );
    assert_eq!(marker_lines(&world), 1);
    release(&world, "argv");
}
/// The silent half of the record clause: `in-use`, `no-slots` and
/// `no-maintain-command` journal nothing. Closing the session is the control — the
/// same driver then journals a run — so the silence is the outcomes' and not a
/// driver that never records.
#[test]
fn a_sweep_answered_in_use_no_slots_and_no_command_journals_nothing() {
    let world = pooled_world("maintenance-quiet", Some(FAST_PACE));
    let service = world.repository("local-direct", &[]);
    let _other = world.extra_repository("other");
    let _third = world.extra_repository("third");
    let maintain = interpreted_script(&world, "maintain");
    let command = serde_json::to_string(&[maintain.as_str()]).expect("an argv serializes");
    pooled_with_commands(
        &world,
        &[("service", command.as_str()), ("other", command.as_str())],
        "120s",
    );
    cut_a_slot(&world, &service.checkout);
    let token = world.on_onevcs(|| {
        onevcs::Providers::real()
            .vcs
            .open_session(onevcs::SessionRequest {
                repo: service.checkout.to_string_lossy().into_owned(),
                branch: None,
                base: None,
                execution_checkout: None,
                pool: None,
                overflow: None,
                labels: Default::default(),
            })
            .expect("a session opens into the slot")
            .token
    });
    assert_eq!(the_slot(&world)["state"]["state"], "in-use");
    assert_eq!(the_slot(&world)["last_maintained"], Value::Null);

    let every = schedule(&world, "quiet", "1s", "");
    held_run(&world, "quiet", 2, &["--maintenance-config", &every]).exited(0);
    until_sweeps(&world, "quiet", 2);
    world.until("the second sweep to end", |world| !sweeping(world, "quiet"));
    assert!(records(&world, "quiet").is_empty(), "{}", world.dump());
    assert_eq!(marker_lines(&world), 0);
    assert_eq!(the_slot(&world)["state"]["state"], "in-use");

    world.on_onevcs(|| {
        onevcs::Providers::real()
            .vcs
            .close_session(&token)
            .expect("the session closes and hands the slot back")
    });
    world.until("the record of the slot once it is free", |world| {
        !records(world, "quiet").is_empty()
    });
    let record = &records(&world, "quiet")[0];
    let identities = record["payload"]["identities"]
        .as_array()
        .expect("identities");
    assert_eq!(
        identities.len(),
        1,
        "only the identity that ran is written: {record}"
    );
    assert_eq!(identities[0]["identity"], SERVICE_IDENTITY, "{record}");
    assert_eq!(
        identities[0]["outcome"]["slots"][0]["outcome"]["ran"]["outcome"], "succeeded",
        "{record}"
    );
    release(&world, "quiet");
}

/// A sweep over session records this host cannot read records that refusal
/// rather than maintaining a slot as though no session held it.
///
/// `pool_maintain` reads the host's open sessions to decide whether a due slot is
/// free; a listing it could not make is not a listing of none, and a sweep that
/// read it as one would run a command in a worktree a session may be working in.
/// So each of the two unreadable states — the sessions directory not a directory,
/// and one record in it not a document — is met by a due slot and a maintain
/// command, and the sweep's record carries the identity's error naming the session
/// records, with nothing run and nothing stamped. Put back, the next sweep
/// maintains the slot, so what the refusal named was the records.
///
/// Broken after the dispatch is placed and mended before the run is let go, as the
/// unlistable-registry journey does, because opening that dispatch reads the same
/// records.
// llmlint: ignore-block[tests_mirror_real_usage] no verb of the sibling's leaves session
// records unreadable, so the state root is written directly.
#[test]
fn a_sweep_over_session_records_it_cannot_read_records_the_refusal_and_runs_nothing() {
    let world = pooled_world("maintenance-unreadable", Some(FAST_PACE));
    let repo = world.repository("local-direct", &[]);
    pooled_with_maintenance(&world, None);
    cut_a_slot(&world, &repo.checkout);
    // Read off the worktree rather than through `pool status`, which refuses the
    // same records the sweep does.
    let marker = Path::new(the_slot(&world)["path"].as_str().expect("a slot path"))
        .join("worktree")
        .join("maintained.log");
    let ran = || {
        std::fs::read_to_string(&marker)
            .unwrap_or_default()
            .lines()
            .count()
    };
    let every = schedule(&world, "unreadable", "1s", "");
    let sessions = world.onevcs_home().join("sessions");
    let aside = world.onevcs_home().join("sessions.aside");
    let record = sessions.join("s-unreadable.json");
    let refused = |record: &Value| {
        record["payload"]["identities"]
            .as_array()
            .is_some_and(|identities| identities.iter().any(|entry| entry.get("error").is_some()))
    };

    for (run, directory) in [("unreadable-directory", true), ("unreadable-record", false)] {
        held_run(&world, run, 2, &["--maintenance-config", &every]).exited(0);
        world.until("the dispatch", |world| {
            !world.events_of(run, "node-dispatched").is_empty()
        });
        let before = ran();
        if directory {
            std::fs::rename(&sessions, &aside).expect("the sessions directory moves aside");
            std::fs::write(&sessions, "this is not a directory")
                .expect("a file takes the sessions directory's place");
        } else {
            std::fs::write(&record, "{ not a session record")
                .expect("the unreadable record is written");
        }
        world.until("the record of the refusal", |world| {
            records(world, run).iter().any(refused)
        });
        let met = records(&world, run)
            .into_iter()
            .find(|record| refused(record))
            .expect("the record of the refusal");
        let entry = &met["payload"]["identities"][0];
        assert_eq!(entry["identity"], SERVICE_IDENTITY, "{met}");
        assert!(entry.get("outcome").is_none(), "{met}");
        let error = entry["error"].as_str().expect("an error");
        assert!(error.contains("session"), "{met}");
        assert_eq!(ran(), before, "a command ran in the slot: {met}");
        if directory {
            std::fs::remove_file(&sessions).expect("the file in the directory's place goes");
            std::fs::rename(&aside, &sessions).expect("the sessions directory comes back");
        } else {
            std::fs::remove_file(&record).expect("the unreadable record goes");
        }
        release(&world, run);
    }

    let before = ran();
    held_run(&world, "readable", 2, &["--maintenance-config", &every]).exited(0);
    world.until("the slot to be maintained", |_| ran() > before);
    release(&world, "readable");
}
// llmlint: ignore-end[tests_mirror_real_usage]
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]

/// Both halves of each script fixture answer the same way: what one platform's
/// half names, the other's names too.
///
/// One contract in two languages, because no platform runs both — so a name
/// added to one and not the other is a journey that passes here and fails on the
/// Windows leg, a fortnight later, with nothing pointing at the fixture. Held by
/// reading the scripts, on `harness::both_hook_scripts_answer_the_same_verbs`'s
/// grounds: no platform executes both halves, and reading them is the only way
/// to compare them; each half is driven as a real subprocess by every journey
/// above.
// llmlint: ignore-block[tests_mirror_real_usage] the subject is the suite's own
// scaffolding — that its two halves agree — which no platform can execute both sides
// of; the fixtures themselves are run the way their callers run them, by `onevcs`
// and by the engine, in every journey of this module.
#[test]
fn both_halves_of_each_fixture_take_the_same_arguments() {
    for (sh, bat, marks) in [(
        "maintain.sh",
        "maintain.bat",
        ["maintained.log", "ONEPIPELINE_E2E_MAINTAIN_EXIT"],
    )] {
        let shell = std::fs::read_to_string(repo_file(&format!("tests/e2e/{sh}")))
            .expect("the shell half ships");
        let batch = std::fs::read_to_string(repo_file(&format!("tests/e2e/{bat}")))
            .expect("the batch half ships");
        for mark in marks {
            assert!(shell.contains(mark), "{sh} no longer names {mark}");
            assert!(batch.contains(mark), "{bat} no longer names {mark}");
        }
    }
} // llmlint: ignore-end[tests_mirror_real_usage]
