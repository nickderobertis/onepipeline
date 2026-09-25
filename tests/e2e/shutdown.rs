//! `onepipeline shutdown`: ending the running work on a host the way a person
//! would want it ended.
//!
//! Every journey drives the compiled binary against real dispatches — processes
//! the run's own driver started, recorded in the run's own registry — and reads
//! what the verb did off the places a reader would: its report, its exit status,
//! the run's journal, the process table, and, for the preserving push, a real git
//! origin on disk. The two halves the verb is made of are each driven the way
//! their own journeys drive them: the interrupt the way `cancellation.rs` drives
//! a cancel, and the teardown the way `driver.rs` drives a `stop`.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. The double answers `interrupt` the way the real CLI does, and the worker
// it acts out either takes the redirection or ignores it — which is the distinction a
// shutdown's report is about. `onevcs` is linked, not substituted, so every preserving push
// here is real git against a real origin. `harness.rs` carries the same suppression and the
// full rationale.

// llmlint: ignore-file[expensive_tests_stay_behind_their_own_edge] measured rather than
// assumed: the twenty-eight journeys here take between about 90 and 195 seconds summed and
// 25 to 50 on the wall under nextest's parallelism, by the host's load — each waits out a real grace
// against real dispatches and pushes to a real origin, because a bound on how long a worker
// is given cannot be stated without a clock running it. What they exercise is the shutdown
// verb over `driver`'s teardown, `engine`'s interrupt, `views` and the linked `onevcs`
// together, which any change under `src/` can move, so a project edged narrower than the
// crate would drop them out of `nx affected` for the very changes they exist to catch. Same
// grounds as `driver.rs`, `cancellation.rs` and `listing.rs`.

use std::path::Path;
// Every journey that times a grace or reads a dispatch's report line is
// `#[cfg(unix)]` — it signals real processes — so the clock types and the two
// helpers only those journeys call are gated on the same terms. Ungated, each is
// dead code under `-D warnings` for a Windows target, which this host's own gate
// never compiles.
#[cfg(unix)]
use std::time::{Duration, Instant};

#[cfg(unix)]
use crate::harness::end_process;
use crate::harness::{agent, git, lifecycle, plan_of, World, REFUSED, USAGE_ERROR};
use serde_json::{json, Value};

/// A grace a journey can wait out, and long enough that a worker that takes
/// the ask has ended well inside it on a loaded host.
#[cfg(unix)]
const GRACE: &str = "4";

/// A grace for a worker that takes the ask and then publishes: its last commit
/// goes through a real publication before it exits, which a Windows runner has
/// been measured taking longer than four seconds over. The wait ends the moment
/// the worker does, so this lengthens nothing but a failure.
const PUBLISHING_GRACE: &str = "120";

/// A launch's run-end hooks, both pointed at one script that records it ran.
///
/// A shutdown fires neither: the run has not ended. A hook firing here would
/// launch follow-up work on a machine that is going away, which is what the
/// absence of this log is the evidence against.
fn hooks(world: &World) -> [String; 6] {
    let log = world.root.join("run-end-hook.log");
    // Its own name: the harness writes the repository's `pre-push` hook body to
    // `hook.sh` in the same directory, and one overwriting the other turns a
    // held publication into one that goes straight through.
    let script = world.root.join("run-end-hook.sh");
    onepipeline_testfakes::executable(
        &script,
        format!("#!/bin/sh\necho fired >> '{}'\n", log.display()),
    );
    let script = script.to_string_lossy().into_owned();
    [
        "--success-hook".into(),
        script.clone(),
        "--failure-hook".into(),
        script,
        "--hook-timeout".into(),
        "60".into(),
    ]
}

#[cfg(unix)]
fn hook_fired(world: &World) -> bool {
    world.root.join("run-end-hook.log").exists()
}

/// Launch a run detached, with both run-end hooks named.
fn launch(world: &World, name: &str, nodes: Vec<Value>) -> String {
    launched_from(world, name, nodes).0
}

/// The same launch, with the project the plan was read out of.
fn launched_from(world: &World, name: &str, nodes: Vec<Value>) -> (String, String) {
    let project = world.plan(name, &plan_of(name, nodes));
    let hooks = hooks(world);
    let mut args = vec!["start", project.as_str(), "--detach"];
    args.extend(hooks.iter().map(String::as_str));
    world.run(&args).exited(0);
    (name.to_string(), project)
}

/// Hold a node's dispatch open with an addressable turn, as a worker mid-task.
fn held(world: &World, node: &str) {
    world.script(&format!("{node}.turn-open"), "");
    world.script(&format!("{node}.wait"), "hold");
}

/// Wait until every named node's turn is open and its dispatch is registered —
/// the two things a shutdown reads to find and address it.
fn until_in_flight(world: &World, run: &str, nodes: &[&str]) {
    world.until("every held node's turn to open", |world| {
        nodes.iter().all(|node| {
            world
                .events_of(run, "turn-started")
                .iter()
                .any(|event| event["labels"]["node"] == *node)
        })
    });
    world.until("every held dispatch to be registered", |world| {
        world.dispatch_records(run).len() >= nodes.len()
    });
}

/// The pid the run's registry names for one node's dispatch.
fn registered_pid(world: &World, run: &str, node: &str) -> u32 {
    world
        .dispatch_records(run)
        .iter()
        .map(|path| {
            serde_json::from_str::<Value>(&std::fs::read_to_string(path).expect("an entry"))
                .expect("an entry is JSON")
        })
        .find(|entry| entry["node"] == node)
        .and_then(|entry| entry["pid"].as_u64())
        .and_then(|pid| u32::try_from(pid).ok())
        .unwrap_or_else(|| panic!("the registry names no dispatch of {node}"))
}

/// The `dispatch-stopped` a shutdown journalled for one node.
fn stopped_for(world: &World, run: &str, node: &str) -> Value {
    let stopped = world.events_of(run, "dispatch-stopped");
    stopped
        .into_iter()
        .find(|event| event["labels"]["node"] == node)
        .unwrap_or_else(|| panic!("no dispatch-stopped for {node}: {}", world.dump()))
}

/// The one `host-shutdown` a run carries.
fn host_shutdown(world: &World, run: &str) -> Value {
    let recorded = world.events_of(run, "host-shutdown");
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    recorded[0]["payload"].clone()
}

/// The report line for one node's dispatch.
#[cfg(unix)]
fn line_for<'a>(report: &'a str, node: &str) -> &'a str {
    report
        .lines()
        .find(|line| line.trim_start().starts_with(&format!("{node} (pid ")))
        .unwrap_or_else(|| panic!("the report names no dispatch of {node}:\n{report}"))
}

/// Whether the origin carries a branch, and at which commit.
fn on_origin(world: &World, origin: &Path, branch: &str) -> Option<String> {
    let listed = git(
        world,
        origin,
        &["ls-remote", ".", &format!("refs/heads/{branch}")],
    );
    listed.split_whitespace().next().map(str::to_string)
}

/// The branch the run's record says one node's session is working on.
fn session_branch(world: &World, run: &str, node: &str) -> String {
    world
        .events_of(run, "session-opened")
        .iter()
        .filter(|event| event["labels"]["node"] == node)
        .find_map(|event| event["payload"]["branch"].as_str().map(str::to_string))
        .unwrap_or_else(|| panic!("no session opened for {node}: {}", world.dump()))
}

/// The journey the verb exists for: one worker that ends when it is asked and
/// one that does not, told apart on the report, on the journal, and on the exit
/// status.
///
/// Both dispatches have an addressable turn, so both interrupts are delivered —
/// the difference is entirely in what each worker did with the ask, which is the
/// difference between a shutdown that lost nothing and one that had to reap.
#[cfg(unix)]
#[test]
fn a_shutdown_asks_every_dispatch_to_stop_and_reaps_the_one_that_does_not() {
    let world = World::new("shutdown-asks");
    held(&world, "polite");
    world.script("polite.stops-when-interrupted", "");
    held(&world, "stubborn");
    let run = launch(
        &world,
        "asks",
        vec![agent("polite", &[]), agent("stubborn", &[])],
    );
    until_in_flight(&world, &run, &["polite", "stubborn"]);
    let polite = registered_pid(&world, &run, "polite");
    let stubborn = registered_pid(&world, &run, "stubborn");

    let since = Instant::now();
    let shutdown = world.run(&["shutdown", &run, "--grace", GRACE]);
    // Killed at the deadline is a verb that did not fully do what it was asked.
    shutdown.exited(REFUSED);
    assert!(
        since.elapsed() >= Duration::from_secs(4),
        "the shutdown returned before the grace a worker that ignored the ask was owed"
    );

    // The report: the run, its owner and scope, each dispatch with the answer
    // its interrupt got and how it ended, and the root it read.
    let report = &shutdown.stdout;
    assert!(
        report.contains(&format!("runs root {}", world.runs.display())),
        "{report}"
    );
    assert!(
        report.contains("owner [mine]  selected by --run"),
        "{report}"
    );
    let polite_line = line_for(report, "polite");
    assert!(
        polite_line.contains(&format!("(pid {polite})")),
        "{polite_line}"
    );
    assert!(polite_line.contains("interrupt delivered"), "{polite_line}");
    assert!(polite_line.contains("ended graceful"), "{polite_line}");
    let stubborn_line = line_for(report, "stubborn");
    assert!(
        stubborn_line.contains(&format!("(pid {stubborn})")),
        "{stubborn_line}"
    );
    assert!(
        stubborn_line.contains("interrupt delivered"),
        "{stubborn_line}"
    );
    assert!(stubborn_line.contains("ended killed"), "{stubborn_line}");
    assert!(report.contains("teardown: signalled"), "{report}");

    // The journal: one `dispatch-stopped` per dispatch, in the contract's shape.
    let graceful = stopped_for(&world, &run, "polite");
    assert_eq!(graceful["payload"]["pid"], json!(polite));
    assert_eq!(graceful["payload"]["interrupt"], "delivered");
    assert_eq!(graceful["payload"]["ended"], "graceful");
    assert!(graceful["payload"]["detail"].is_string(), "{graceful}");
    let graceful_ms = graceful["payload"]["waited_ms"]
        .as_u64()
        .expect("waited_ms");
    assert!(graceful_ms < 4_000, "{graceful}");
    let killed = stopped_for(&world, &run, "stubborn");
    assert_eq!(killed["payload"]["pid"], json!(stubborn));
    assert_eq!(killed["payload"]["interrupt"], "delivered");
    assert_eq!(killed["payload"]["ended"], "killed");
    assert!(
        killed["payload"]["waited_ms"].as_u64().expect("waited_ms") >= 4_000,
        "{killed}"
    );

    // And one `host-shutdown`, carrying the tallies and `stop`'s own teardown
    // word — and nothing that would read as the run having ended.
    let recorded = host_shutdown(&world, &run);
    assert_eq!(recorded["scope"], "run");
    assert_eq!(recorded["owner"], "[mine]");
    assert_eq!(recorded["forced"], false);
    assert_eq!(recorded["grace_seconds"], 4);
    assert_eq!(recorded["dispatches"], 2);
    assert_eq!(recorded["graceful"], 1);
    assert_eq!(recorded["killed"], 1);
    assert_eq!(recorded["teardown"], "signalled");
    assert_eq!(recorded["root"], json!(world.runs.display().to_string()));
    assert_eq!(recorded["branches"], json!([]));
    assert!(
        world.events_of(&run, "run-stopped").is_empty(),
        "a shutdown journalled a stop"
    );
    assert!(
        world.events_of(&run, "run-hook-fired").is_empty(),
        "a shutdown fired a run-end hook"
    );
    // The hook the launch named, which would have written this had anything
    // fired it — including a driver that settled on its way out.
    std::thread::sleep(Duration::from_millis(500));
    assert!(!hook_fired(&world), "a run-end hook ran on a host shutdown");
}

/// `--force` asks nothing and waits for nothing, and `--grace 0` is the same
/// path.
///
/// Run with the ten-minute default grace still in force, so the only thing that
/// can make the command return quickly is the force itself.
#[cfg(unix)]
#[test]
fn a_forced_shutdown_asks_nothing_and_waits_for_nothing() {
    let world = World::new("shutdown-force");
    for node in ["alpha", "beta"] {
        held(&world, node);
        // A worker that would have taken the ask, so an ask that was made would
        // show as a graceful ending rather than `not-asked`.
        world.script(&format!("{node}.stops-when-interrupted"), "");
    }
    let forced = launch(&world, "forced", vec![agent("alpha", &[])]);
    until_in_flight(&world, &forced, &["alpha"]);
    let zero = launch(&world, "zero", vec![agent("beta", &[])]);
    until_in_flight(&world, &zero, &["beta"]);

    for (run, node, args) in [
        (
            forced.as_str(),
            "alpha",
            vec!["shutdown", "forced", "--force"],
        ),
        (
            zero.as_str(),
            "beta",
            vec!["shutdown", "zero", "--grace", "0"],
        ),
    ] {
        let since = Instant::now();
        let shutdown = world.run(&args);
        // Nothing was killed at a deadline and the tree was reached, so the
        // forced path is a clean one.
        shutdown.exited(0);
        assert!(
            since.elapsed() < Duration::from_secs(60),
            "`{}` took {:?}, which is the grace it was told to skip",
            args.join(" "),
            since.elapsed()
        );
        let stopped = stopped_for(&world, run, node);
        assert_eq!(stopped["payload"]["interrupt"], "not-asked", "{stopped}");
        assert!(
            world.events_of(run, "turn-interrupted").is_empty(),
            "a forced shutdown pulled the lever anyway"
        );
        assert!(
            line_for(&shutdown.stdout, node).contains("interrupt not-asked"),
            "{}",
            shutdown.stdout
        );
        // `--grace 0` is the same path as `--force`, and is recorded as one.
        let recorded = host_shutdown(&world, run);
        assert_eq!(recorded["forced"], true, "{recorded}");
        assert!(
            shutdown.stdout.contains("  forced  "),
            "{}",
            shutdown.stdout
        );
    }
}

/// Once a run's shutdown has begun it dispatches no further node.
///
/// A ready node waits behind a running one; the running one takes the ask and
/// settles inside the grace, which is exactly the moment a driver left to
/// itself dispatches what was waiting behind it. A second worker ignores the ask,
/// so the grace is waited out in full and the driver has every chance to.
#[cfg(unix)]
#[test]
fn nothing_new_starts_once_a_shutdown_has_begun() {
    let world = World::new("shutdown-nothing-new");
    held(&world, "first");
    world.script("first.stops-when-interrupted", "");
    held(&world, "holder");
    held(&world, "second");
    let run = launch(
        &world,
        "nothingnew",
        vec![
            agent("first", &[]),
            agent("holder", &[]),
            agent("second", &["first"]),
        ],
    );
    until_in_flight(&world, &run, &["first", "holder"]);

    world
        .run(&["shutdown", &run, "--grace", GRACE])
        .exited(REFUSED);

    // The node in front settled while the driver was still alive — so what
    // stood behind it became ready on the driver's watch.
    assert!(
        world
            .events_of(&run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "first"),
        "the node in front never settled, so this journey proves nothing: {}",
        world.dump()
    );
    assert!(
        world
            .events_of(&run, "node-dispatched")
            .iter()
            .all(|event| event["labels"]["node"] != "second"),
        "a node was dispatched after the shutdown began: {}",
        world.dump()
    );
}

/// After a shutdown the run is intact and adoptable, and `adopt` takes up the
/// node that was in flight on the session and branch it was working in.
///
/// Also where `status` and `results` are read, because this is the state they
/// have to describe: a run put down mid-flight, with a node still recorded
/// running whose branch the shutdown pushed.
#[test]
fn a_shut_down_run_is_adoptable_and_resumes_its_node_on_its_own_branch() {
    let world = World::new("shutdown-adopt");
    let repository = world.repository("local-direct", &[]);
    held(&world, "service");
    world.script("service.work", "the worker wrote this\n");
    let (run, project) = launched_from(&world, "adoptable", vec![lifecycle("service", &[])]);
    until_in_flight(&world, &run, &["service"]);
    let branch = session_branch(&world, &run, "service");
    let tasks_before = world.store_tasks(&project);

    let shutdown = world.run(&["shutdown", &run, "--grace", "1"]);
    shutdown.exited(REFUSED);

    // The branch the node was working on reached its origin, and the report
    // says so in the words that say what that is and is not.
    let pushed = on_origin(&world, &repository.origin, &branch)
        .unwrap_or_else(|| panic!("{branch} is not on the origin:\n{}", shutdown.stdout));
    let line = shutdown
        .stdout
        .lines()
        .find(|line| line.contains(&format!("@{branch}:")))
        .unwrap_or_else(|| panic!("the report names no {branch}:\n{}", shutdown.stdout));
    assert!(line.contains("on its origin unproven"), "{line}");
    assert!(
        line.contains("without that repository's own hook or merge path having run"),
        "{line}"
    );
    assert!(
        line.contains("nothing can merge it without a publication that does run one"),
        "{line}"
    );
    assert!(line.contains(&pushed), "{line}");
    let recorded = host_shutdown(&world, &run);
    assert_eq!(recorded["branches"][0]["branch"], json!(branch));
    assert_eq!(recorded["branches"][0]["result"], "pushed");
    assert_eq!(recorded["branches"][0]["commit"], json!(pushed));

    // Nothing was settled, parked or written back by the shutdown: the node is
    // still in flight, and the store reads exactly as it did.
    assert!(
        world.events_of(&run, "node-settled").is_empty(),
        "the shutdown settled a node"
    );
    assert!(world.events_of(&run, "run-stopped").is_empty());
    assert!(
        !world.run_file(&run, "result.json").exists(),
        "the run closed out"
    );
    assert_eq!(
        world.store_tasks(&project),
        tasks_before,
        "the shutdown wrote the plan store"
    );

    // `status` reads it as a host shutdown and names what resumes it.
    let status = world.run(&["status", &run]);
    status
        .exited(0)
        .out_has("HOST SHUTDOWN")
        .out_has(&format!("onepipeline adopt {run} resumes it"))
        .out_has("service: worker killed by the host shutdown");
    // `results` names the in-flight node's branch and where it is.
    world
        .run(&["results", &run])
        .exited(0)
        .out_has(&format!("its work is on {branch} (on its origin unproven"));

    // And `adopt` takes the node up again, pinned to the session and branch it
    // was working in, and runs it to the end.
    world.release("service.go");
    world.run(&["adopt", &run]).exited(0);
    let adopted = world.events_of(&run, "driver-adopted");
    let abandoned = &adopted[0]["payload"]["abandoned"];
    assert_eq!(abandoned[0]["node"], "service", "{adopted:?}");
    assert_eq!(abandoned[0]["branch"], json!(branch), "{adopted:?}");
    let settled = world.events_of(&run, "node-settled");
    assert_eq!(settled.len(), 1, "{settled:?}");
    assert_eq!(settled[0]["payload"]["status"], "done", "{settled:?}");
    assert!(
        repository.base_file("service.md").is_some(),
        "the resumed node's work did not land"
    );
    // Adopted, the run is no longer described as shut down.
    world
        .run(&["status", &run])
        .exited(0)
        .out_lacks("HOST SHUTDOWN");
}

/// Every branch is offered to the preserving push once the teardown has been
/// attempted — whether or not it ended everything.
///
/// The worker here keeps working through the polite ask *and* the `SIGTERM`, so
/// the teardown cannot end it and says so. The branch it was working on is
/// still pushed, and the survivor is named by pid.
#[cfg(unix)]
#[test]
fn a_teardown_that_leaves_a_survivor_still_preserves_every_branch() {
    let world = World::new("shutdown-survivor");
    let repository = world.repository("local-direct", &[]);
    held(&world, "service");
    world.script("service.ignores-the-ask", "yes");
    let run = launch(&world, "survivor", vec![lifecycle("service", &[])]);
    until_in_flight(&world, &run, &["service"]);
    let worker = registered_pid(&world, &run, "service");
    let branch = session_branch(&world, &run, "service");

    let shutdown = world.run(&["shutdown", &run, "--grace", "1"]);
    shutdown.exited(REFUSED);
    assert!(
        on_origin(&world, &repository.origin, &branch).is_some(),
        "a teardown that could not end everything left {branch} unpushed:\n{}",
        shutdown.stdout
    );
    assert!(
        shutdown.stdout.contains("partly stopped"),
        "the teardown was not reported in stop's own words:\n{}",
        shutdown.stdout
    );
    assert!(
        shutdown
            .stdout
            .contains(&format!("Still running: service (pid {worker})")),
        "the survivor is not named by pid:\n{}",
        shutdown.stdout
    );
    assert_eq!(
        stopped_for(&world, &run, "service")["payload"]["ended"],
        "still-running"
    );
    assert_eq!(host_shutdown(&world, &run)["teardown"], "partly-signalled");

    // The one process this journey's run left that no `World` takes with it.
    end_process(worker);
    world.release("service.go");
}

/// One branch name in two repositories is read back for each of them, and never
/// as the one verdict another repository's push got.
///
/// Two lifecycle nodes, in two repositories, are pinned to the same branch name.
/// Somebody else's commit sits under that name on the first repository's origin,
/// so that push is refused, while the second repository's goes through: the two
/// branches share a name and nothing else, and `results` has to say so.
#[test]
fn one_branch_name_in_two_repositories_is_read_back_for_each() {
    const SHARED: &str = "feature/shared";
    let world = World::new("shutdown-shared-name");
    let service = world.repository("local-direct", &[]);
    let tool = world.extra_repository("tool");
    for node in ["alpha", "beta"] {
        held(&world, node);
    }
    let mut alpha = lifecycle("alpha", &[]);
    alpha["branch"] = json!(SHARED);
    let mut beta = lifecycle("beta", &[]);
    beta["repo"] = json!("tool");
    beta["branch"] = json!(SHARED);
    let run = launch(&world, "sharedname", vec![alpha, beta]);
    until_in_flight(&world, &run, &["alpha", "beta"]);
    for node in ["alpha", "beta"] {
        assert_eq!(session_branch(&world, &run, node), SHARED);
    }

    // Somebody else's commit, on the first origin only, under the shared name.
    let elsewhere = world.root.join("elsewhere");
    git(
        &world,
        &world.root,
        &["clone", &service.origin.to_string_lossy(), "elsewhere"],
    );
    std::fs::write(elsewhere.join("theirs.md"), "somebody else's work\n").expect("a file");
    git(&world, &elsewhere, &["checkout", "-b", SHARED]);
    git(&world, &elsewhere, &["add", "-A"]);
    git(
        &world,
        &elsewhere,
        &["commit", "-m", "chore: somebody else's"],
    );
    git(&world, &elsewhere, &["push", "origin", SHARED]);

    world.run(&["shutdown", &run, "--force"]).exited(REFUSED);
    assert!(
        on_origin(&world, &tool.origin, SHARED).is_some(),
        "the second repository's branch was not pushed"
    );

    let results = world.run(&["results", &run]).exited(0).stdout.clone();
    for node in ["alpha", "beta"] {
        let line = results
            .lines()
            .find(|line| line.trim_start().starts_with(node))
            .unwrap_or_else(|| panic!("results name no {node}:\n{results}"));
        assert!(
            line.contains("recorded under this name in 2 repositories"),
            "{line}"
        );
        assert!(
            line.contains("service: not on its origin: the preserving push was refused"),
            "{line}"
        );
        assert!(line.contains("tool: on its origin unproven"), "{line}");
    }
    for node in ["alpha", "beta"] {
        world.release(&format!("{node}.go"));
    }
}

/// A push the origin refuses is reported, and the branches after it are still
/// pushed.
///
/// The first branch in the order the shutdown offers them has a commit on its
/// origin the local branch does not carry — somebody else's work under the same
/// name — so git refuses a push that is not a fast-forward, which is exactly
/// what a preserving push must never overwrite.
#[test]
fn a_refused_push_is_reported_and_the_branches_after_it_are_still_pushed() {
    let world = World::new("shutdown-refused-push");
    let repository = world.repository("local-direct", &[]);
    for node in ["alpha", "beta"] {
        held(&world, node);
    }
    let run = launch(
        &world,
        "refusedpush",
        vec![lifecycle("alpha", &[]), lifecycle("beta", &[])],
    );
    until_in_flight(&world, &run, &["alpha", "beta"]);
    let alpha = session_branch(&world, &run, "alpha");
    let beta = session_branch(&world, &run, "beta");

    // Somebody else's commit, on the origin, under alpha's branch name.
    let elsewhere = world.root.join("elsewhere");
    git(
        &world,
        &world.root,
        &["clone", &repository.origin.to_string_lossy(), "elsewhere"],
    );
    std::fs::write(elsewhere.join("theirs.md"), "somebody else's work\n").expect("a file");
    git(&world, &elsewhere, &["checkout", "-b", &alpha]);
    git(&world, &elsewhere, &["add", "-A"]);
    git(
        &world,
        &elsewhere,
        &["commit", "-m", "chore: somebody else's"],
    );
    git(&world, &elsewhere, &["push", "origin", &alpha]);
    let theirs = on_origin(&world, &repository.origin, &alpha).expect("their commit is there");

    let shutdown = world.run(&["shutdown", &run, "--force"]);
    // The refusal is the verb not having done what it was asked.
    shutdown.exited(REFUSED);
    let refused = shutdown
        .stdout
        .lines()
        .find(|line| line.contains(&format!("@{alpha}:")))
        .unwrap_or_else(|| panic!("the report names no {alpha}:\n{}", shutdown.stdout));
    assert!(refused.contains("could not be preserved"), "{refused}");
    assert_eq!(
        on_origin(&world, &repository.origin, &alpha),
        Some(theirs),
        "a preserving push overwrote somebody else's commit"
    );
    assert!(
        on_origin(&world, &repository.origin, &beta).is_some(),
        "the branch after a refused one was not pushed:\n{}",
        shutdown.stdout
    );
    let branches = host_shutdown(&world, &run)["branches"].clone();
    let result_of = |branch: &str| {
        branches
            .as_array()
            .expect("branches")
            .iter()
            .find(|row| row["branch"] == branch)
            .map(|row| row["result"].clone())
    };
    assert_eq!(result_of(&alpha), Some(json!("refused")), "{branches}");
    assert_eq!(result_of(&beta), Some(json!("pushed")), "{branches}");
    for node in ["alpha", "beta"] {
        world.release(&format!("{node}.go"));
    }
}

/// `--host` acts on a run another session owns and names that owner; `RUN` and
/// `--mine` refuse it, name the owner, and signal nothing.
#[cfg(unix)]
#[test]
fn only_host_acts_on_another_sessions_run() {
    let world = World::new("shutdown-owners");
    held(&world, "theirs");
    let run = launch(&world, "theirs", vec![agent("theirs", &[])]);
    until_in_flight(&world, &run, &["theirs"]);
    let worker = registered_pid(&world, &run, "theirs");
    let stranger = world.as_session("another-planner");

    stranger
        .run(&["shutdown", &run])
        .exited(REFUSED)
        .err_has("belongs to");
    let mine = stranger.run(&["shutdown", "--mine"]);
    mine.exited(0).err_has(&format!("run '{run}' belongs to"));
    assert!(mine.stdout.contains("no runs selected"), "{}", mine.stdout);
    // Nothing was signalled and nothing was recorded by either.
    assert!(
        world.registered(&run, worker),
        "a refused shutdown ended the dispatch"
    );
    assert!(world.events_of(&run, "host-shutdown").is_empty());
    assert!(world.events_of(&run, "dispatch-stopped").is_empty());
    assert!(world.events_of(&run, "turn-interrupted").is_empty());

    let host = stranger.run(&["shutdown", "--host", "--force"]);
    host.exited(0);
    // The owner is named, as the other session's rather than as this one's.
    assert!(
        host.stdout.contains(&format!("{run}  owner [e2e:"))
            && host.stdout.contains("which is another session's run"),
        "{}",
        host.stdout
    );
    assert_eq!(host_shutdown(&world, &run)["scope"], "host");
    world.release("theirs.go");
}

/// A run root `--host` and `--mine` cannot read is named on stderr as one this
/// shutdown did not act on, and every run beside it is still shut down.
///
/// Such a root may hold live dispatches and branches that exist only on this
/// host, so leaving it out without a word would report a partial shutdown as
/// the whole host's.
// llmlint: ignore-block[tests_mirror_real_usage] the one state set by hand is a run root with no launch record, exactly as `views.rs`'s `a_run_root_the_views_refuse_is_named_with_its_reason` plants it: it is a crash between the directory and the record, and no verb writes one. The run beside it and both shutdowns are the real binary.
#[cfg(unix)]
#[test]
fn a_run_root_the_survey_refuses_is_named_and_the_rest_is_still_shut_down() {
    let world = World::new("shutdown-refused-root");
    held(&world, "build");
    let run = launch(&world, "readable", vec![agent("build", &[])]);
    until_in_flight(&world, &run, &["build"]);
    std::fs::create_dir_all(world.runs.join("half-written")).expect("a run root with no launch");

    for scope in ["--mine", "--host"] {
        let shutdown = world.run(&["shutdown", scope, "--force"]);
        shutdown.exited(0).err_has("half-written");
        assert!(
            shutdown
                .stderr
                .contains("this shutdown signalled nothing in it and pushed none of its branches"),
            "{scope} did not say what it left undone in the root it could not read:\n{}",
            shutdown.stderr
        );
    }
    // The readable run was shut down by the first pass and read again by the second.
    assert_eq!(world.events_of(&run, "host-shutdown").len(), 2);
    world.release("build.go");
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// A node killed inside a publication of its own is named as such on the
/// report, with the state that leaves and the verb that resumes it.
///
/// The publication is held at the repository's own `pre-push` hook, so the
/// node's agent has finished and its work is the driver's: there is no turn to
/// interrupt, and the grace is what bounds it.
#[cfg(unix)]
#[test]
fn a_dispatch_killed_inside_its_publication_is_named_with_what_that_leaves() {
    let world = World::new("shutdown-mid-publication");
    let go = world.fakes.join("push.go");
    let held_push = crate::harness::held_publication(&world, &go);
    world.repository("local-direct", &held_push.argv());
    world.script("service.work", "the worker wrote this\n");
    let run = launch(&world, "midpublication", vec![lifecycle("service", &[])]);
    world.until("the publication to reach its merge path", |world| {
        !world.events_of(&run, "merge-queued").is_empty()
    });

    let shutdown = world.run(&["shutdown", &run, "--grace", "1"]);
    shutdown.exited(REFUSED);
    let line = line_for(&shutdown.stdout, "service");
    assert!(line.contains("interrupt no-turn"), "{line}");
    assert!(line.contains("inside a publication of its own"), "{line}");
    assert!(line.contains("ended killed"), "{line}");
    assert!(
        line.contains("when the deadline reaped it"),
        "the report does not name the publication it was killed inside:\n{line}"
    );
    assert!(line.contains("its work is on branch"), "{line}");
    assert!(
        line.contains(&format!("`onepipeline adopt {run}` re-dispatches the node")),
        "the report does not name the verb that resumes it:\n{line}"
    );
    held_push.release();
}

/// A shutdown's last section names every other unpublished branch on the host
/// it did not push — and, where that enumeration cannot be read, says so rather
/// than reporting none.
// llmlint: ignore-block[tests_mirror_real_usage] the one thing set by hand is `onevcs`'s state-root registry, overwritten with bytes that are not one, because no verb produces a host whose enumeration cannot be read — that is a damaged or foreign state root, which is the state under test. The shutdown is the real binary against a real run, and the assertion is on what its report says.
#[cfg(unix)]
#[test]
fn the_last_section_says_when_the_host_could_not_be_read() {
    let world = World::new("shutdown-unread");
    held(&world, "build");
    let run = launch(&world, "unread", vec![agent("build", &[])]);
    until_in_flight(&world, &run, &["build"]);

    // `onevcs`'s state root, made unreadable as a registry: its enumeration of
    // this host's branches can now only fail.
    let home = world.onevcs_home();
    std::fs::create_dir_all(&home).expect("a state root");
    std::fs::write(home.join("registry.json"), "not a registry").expect("a broken registry");

    let shutdown = world.run(&["shutdown", &run, "--force"]);
    let last = shutdown
        .stdout
        .lines()
        .last()
        .unwrap_or_default()
        .to_string();
    assert!(
        shutdown
            .stdout
            .contains("other unpublished branches on this host: not read"),
        "{}",
        shutdown.stdout
    );
    assert!(
        shutdown.stdout.contains("This is not a count of zero"),
        "{}",
        shutdown.stdout
    );
    assert!(
        last.contains("This is not a count of zero"),
        "the enumeration is not the report's last section:\n{}",
        shutdown.stdout
    );
    // A fact, never a failure: nothing was killed and the tree was reached.
    shutdown.exited(0);
    world.release("build.go");
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// A worker that ends when it is asked has its branch on the origin afterwards.
///
/// The preserving push's other half beside the survivor journey: here the
/// teardown ended everything and nothing was killed. The worker took the
/// redirection, wrote what it was told to and ended, its node published inside
/// the grace, and the branch it had been on is still offered to `onevcs` and is
/// on the origin afterwards — a clean shutdown, at exit 0.
#[test]
fn a_worker_that_ends_on_its_interrupt_has_its_branch_on_the_origin_afterwards() {
    let world = World::new("shutdown-graceful-branch");
    let repository = world.repository("local-direct", &[]);
    held(&world, "service");
    world.script("service.stops-when-interrupted", "");
    let run = launch(&world, "graceful", vec![lifecycle("service", &[])]);
    until_in_flight(&world, &run, &["service"]);
    let branch = session_branch(&world, &run, "service");

    let shutdown = world.run(&["shutdown", &run, "--grace", PUBLISHING_GRACE]);
    shutdown.exited(0);
    let stopped = stopped_for(&world, &run, "service");
    assert_eq!(stopped["payload"]["interrupt"], "delivered", "{stopped}");
    assert_eq!(stopped["payload"]["ended"], "graceful", "{stopped}");
    // What the redirection asked for is what the worker did: it finished and
    // committed, and that work reached the base inside the grace.
    assert!(
        repository.base_file("service-redirected.md").is_some(),
        "the worker's last work did not survive the shutdown:\n{}",
        shutdown.stdout
    );
    assert!(
        on_origin(&world, &repository.origin, &branch).is_some(),
        "{branch} is not on the origin after a worker that took the ask:\n{}",
        shutdown.stdout
    );
    assert_eq!(
        host_shutdown(&world, &run)["branches"][0]["result"],
        "pushed"
    );
}

/// Exactly one scope, and it is required: naming none, or naming two, is refused
/// before anything is signalled.
#[test]
fn a_shutdown_names_exactly_one_scope_or_is_refused_before_it_signals_anything() {
    let world = World::new("shutdown-scope");
    held(&world, "build");
    let run = launch(&world, "scoped", vec![agent("build", &[])]);
    until_in_flight(&world, &run, &["build"]);
    let worker = registered_pid(&world, &run, "build");

    for args in [
        vec!["shutdown"],
        vec!["shutdown", "--grace", "0"],
        vec!["shutdown", run.as_str(), "--host"],
        vec!["shutdown", run.as_str(), "--mine"],
        vec!["shutdown", "--mine", "--host"],
    ] {
        world
            .run(&args)
            .exited(USAGE_ERROR)
            .err_has("--mine")
            .err_has("--host");
    }
    assert!(
        world.registered(&run, worker),
        "a refused shutdown ended the dispatch"
    );
    assert!(world.events_of(&run, "host-shutdown").is_empty());
    assert!(!world.run_file(&run, "shutting-down.json").exists());
    world.release("build.go");
}

/// A node whose change already landed has no branch left to preserve, and the
/// work still in flight beside it does.
///
/// The landed node's work is on its base, on the origin, before the shutdown
/// begins — the line `onevcs recoverable` draws too — so offering its finished
/// session branch would only risk a refusal over work nobody could lose.
#[test]
fn a_landed_nodes_branch_is_not_offered_and_the_work_in_flight_is() {
    let world = World::new("shutdown-landed");
    let repository = world.repository("local-direct", &[]);
    world.script("done.work", "the finished worker wrote this\n");
    held(&world, "live");
    let run = launch(
        &world,
        "landed",
        vec![lifecycle("done", &[]), lifecycle("live", &[])],
    );
    world.until("the finished node to land", |world| {
        world
            .events_of(&run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "done")
    });
    until_in_flight(&world, &run, &["live"]);
    assert!(
        repository.base_file("done.md").is_some(),
        "the node did not land"
    );
    let live = session_branch(&world, &run, "live");

    let shutdown = world.run(&["shutdown", &run, "--force"]);
    shutdown.exited(0);
    let branches = host_shutdown(&world, &run)["branches"].clone();
    assert_eq!(
        branches.as_array().map(Vec::len),
        Some(1),
        "the shutdown offered a landed node's branch: {branches}"
    );
    assert_eq!(branches[0]["branch"], json!(live), "{branches}");
    assert_eq!(branches[0]["result"], "pushed", "{branches}");
    assert!(on_origin(&world, &repository.origin, &live).is_some());
}

/// A run whose dispatch registry cannot be read is reported rather than refused,
/// its branches are still pushed, and the rest of the host is still shut down.
///
/// A host shutdown works through every run on the host, and the worst answer it
/// could give to one unreadable run is to stop there: every run after it would
/// be left running and unpushed on a machine about to go away.
// llmlint: ignore-block[tests_mirror_real_usage] the one thing set by hand is a planted entry in the run's dispatch registry that no build can read, exactly as `driver.rs`'s `stopping_a_run_whose_registry_cannot_be_read_refuses_and_leaves_the_run_retryable` plants it: no verb writes an unreadable entry, so reaching this state takes a host that corrupted one. Both runs, their dispatches and the shutdown are the real binary.
#[cfg(unix)]
#[test]
fn an_unreadable_registry_is_reported_and_the_rest_of_the_host_is_still_shut_down() {
    let world = World::new("shutdown-unreadable-registry");
    let repository = world.repository("local-direct", &[]);
    held(&world, "service");
    held(&world, "build");
    let broken = launch(&world, "broken", vec![lifecycle("service", &[])]);
    until_in_flight(&world, &broken, &["service"]);
    let sound = launch(&world, "sound", vec![agent("build", &[])]);
    until_in_flight(&world, &sound, &["build"]);
    let branch = session_branch(&world, &broken, "service");
    let planted = world.run_file(&broken, "dispatches/planted.json");
    std::fs::write(&planted, "not an entry this build knows").expect("an unreadable entry");

    let shutdown = world.run(&["shutdown", "--host", "--force"]);
    // A teardown that could not be attempted is a shutdown that did not do what
    // it was asked, and it says why on stderr.
    shutdown
        .exited(REFUSED)
        .err_has("cannot establish what it is running");
    assert!(
        shutdown
            .stdout
            .contains("was not stopped: this host gave no answer"),
        "the unread teardown is not reported in stop's own words:\n{}",
        shutdown.stdout
    );
    assert_eq!(host_shutdown(&world, &broken)["teardown"], "not-attempted");
    assert!(
        on_origin(&world, &repository.origin, &branch).is_some(),
        "an unreadable registry cost the run its branch:\n{}",
        shutdown.stdout
    );
    // And the run after it was shut down all the same.
    assert_eq!(host_shutdown(&world, &sound)["teardown"], "signalled");
    assert_eq!(
        stopped_for(&world, &sound, "build")["payload"]["ended"],
        "killed"
    );

    // The run this journey broke, put back and stopped the ordinary way.
    std::fs::remove_file(&planted).expect("the entry is taken away again");
    world.run(&["stop", &broken]).exited(0);
    for node in ["service", "build"] {
        world.release(&format!("{node}.go"));
    }
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// A lever that breaks is an answer rather than a failure, and the deadline
/// still applies.
#[cfg(unix)]
#[test]
fn a_lever_that_breaks_is_reported_and_the_deadline_still_applies() {
    let world = World::new("shutdown-lever-broke");
    world.script("interrupt.fail", "");
    held(&world, "build");
    let run = launch(&world, "leverbroke", vec![agent("build", &[])]);
    until_in_flight(&world, &run, &["build"]);

    let shutdown = world.run(&["shutdown", &run, "--grace", "1"]);
    shutdown.exited(REFUSED);
    let line = line_for(&shutdown.stdout, "build");
    assert!(line.contains("interrupt failed"), "{line}");
    assert!(line.contains("the lever failed"), "{line}");
    assert!(line.contains("ended killed"), "{line}");
    let stopped = stopped_for(&world, &run, "build");
    assert_eq!(stopped["payload"]["interrupt"], "failed", "{stopped}");
}

/// A dispatch that has named no turn is recorded as having none to ask, and the
/// deadline still applies.
///
/// The worker has started and recorded nothing yet — no member, no turn — so the
/// run's own record carries no address to interrupt. That is an answer rather
/// than a failure of the lever, and the wait is the same one an asked worker gets.
#[cfg(unix)]
#[test]
fn a_dispatch_that_named_no_turn_has_none_to_ask_and_the_deadline_still_applies() {
    let world = World::new("shutdown-no-turn");
    world.script("build.wait", "hold");
    let run = launch(&world, "noturn", vec![agent("build", &[])]);
    world.until("the held dispatch to be registered", |world| {
        !world.dispatch_records(&run).is_empty()
    });

    let shutdown = world.run(&["shutdown", &run, "--grace", "1"]);
    shutdown.exited(REFUSED);
    let line = line_for(&shutdown.stdout, "build");
    assert!(line.contains("interrupt no-turn"), "{line}");
    assert!(
        line.contains("nothing of this dispatch has named a turn to interrupt"),
        "{line}"
    );
    assert!(line.contains("ended killed"), "{line}");
    let stopped = stopped_for(&world, &run, "build");
    assert_eq!(stopped["payload"]["interrupt"], "no-turn", "{stopped}");
    assert!(
        world.events_of(&run, "turn-started").is_empty(),
        "the worker named a turn, so this journey proves nothing: {}",
        world.dump()
    );
}

/// A branch already level with its origin is a fact reported at exit 0.
///
/// The ordinary way to meet one: a host shut down twice. The first shutdown
/// pushed the branch; the second finds nothing live, nothing to tear down, and
/// the origin already carrying that commit — so there is nothing it failed to do.
#[test]
fn a_branch_already_on_its_origin_is_reported_and_is_not_a_failure() {
    let world = World::new("shutdown-already-on-origin");
    let repository = world.repository("local-direct", &[]);
    held(&world, "service");
    let run = launch(&world, "twice", vec![lifecycle("service", &[])]);
    until_in_flight(&world, &run, &["service"]);
    let branch = session_branch(&world, &run, "service");

    world.run(&["shutdown", &run, "--force"]).exited(0);
    let pushed = on_origin(&world, &repository.origin, &branch).expect("the first push landed");

    let again = world.run(&["shutdown", &run, "--force"]);
    again.exited(0).out_has("no live dispatch to interrupt");
    let line = again
        .stdout
        .lines()
        .find(|line| line.contains(&format!("@{branch}:")))
        .unwrap_or_else(|| panic!("the report names no {branch}:\n{}", again.stdout));
    assert!(line.contains("already on its origin"), "{line}");
    assert!(line.contains(&pushed), "{line}");
    let recorded = world.events_of(&run, "host-shutdown");
    assert_eq!(recorded.len(), 2, "{recorded:?}");
    assert_eq!(
        recorded[1]["payload"]["branches"][0]["result"],
        "already-on-origin"
    );
    world.release("service.go");
}

/// A branch nothing outside this host can carry — its checkout has no origin —
/// is a fact the report names, with the commit that is at risk, at exit 0.
///
/// The one state here set by hand: the session's own clone losing its `origin`
/// remote. No verb produces it — `onevcs register` refuses an identity with no
/// origin — and it is what a clone whose remote an operator removed looks like to
/// the preserving push, which asks the location and never the identity.
// llmlint: ignore-block[tests_mirror_real_usage] the one value set by hand is the session
// clone's `origin` remote, removed with git, and no product surface sets it: registering an
// identity requires an origin, so a location with none is reached only by somebody removing
// it. Everything else is the real binary against a real run, and the assertion is on what
// the preserving push answered for that location.
#[test]
fn a_branch_with_no_origin_to_go_to_is_reported_with_its_commit() {
    let world = World::new("shutdown-no-remote");
    world.repository("local-direct", &[]);
    held(&world, "service");
    let run = launch(&world, "noremote", vec![lifecycle("service", &[])]);
    until_in_flight(&world, &run, &["service"]);
    let branch = session_branch(&world, &run, "service");
    let token = branch.trim_start_matches("onevcs/").to_string();
    let clone = clone_of(&world.onevcs_home().join("workspaces"), &token)
        .unwrap_or_else(|| panic!("no run clone for session {token}"));
    git(&world, &clone, &["remote", "remove", "origin"]);

    let shutdown = world.run(&["shutdown", &run, "--force"]);
    shutdown.exited(0);
    let line = shutdown
        .stdout
        .lines()
        .find(|line| line.contains(&format!("@{branch}:")))
        .unwrap_or_else(|| panic!("the report names no {branch}:\n{}", shutdown.stdout));
    assert!(line.contains("no origin to push to"), "{line}");
    let recorded = host_shutdown(&world, &run);
    assert_eq!(recorded["branches"][0]["result"], "no-remote");
    assert_eq!(recorded["branches"][0]["remote"], Value::Null);
    let commit = recorded["branches"][0]["commit"]
        .as_str()
        .expect("the commit nothing else carries is named");
    assert!(line.contains(commit), "{line}");
    world.release("service.go");
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// The clone a session's worktree commits into, found under the state root.
fn clone_of(workspaces: &Path, token: &str) -> Option<std::path::PathBuf> {
    std::fs::read_dir(workspaces)
        .ok()?
        .flatten()
        .map(|identity| identity.path().join("runs").join(token).join("clone"))
        .find(|clone| clone.is_dir())
}

/// `--mine` shuts down every run this session owns, exactly as `RUN` does, and
/// records the scope that selected it.
#[cfg(unix)]
#[test]
fn mine_shuts_down_every_run_this_session_owns() {
    let world = World::new("shutdown-mine");
    held(&world, "first");
    held(&world, "second");
    let one = launch(&world, "one", vec![agent("first", &[])]);
    until_in_flight(&world, &one, &["first"]);
    let two = launch(&world, "two", vec![agent("second", &[])]);
    until_in_flight(&world, &two, &["second"]);

    let shutdown = world.run(&["shutdown", "--mine", "--force"]);
    shutdown.exited(0);
    for (run, node) in [(&one, "first"), (&two, "second")] {
        assert!(
            shutdown
                .stdout
                .contains(&format!("{run}  owner [mine]  selected by --mine")),
            "{}",
            shutdown.stdout
        );
        assert_eq!(host_shutdown(&world, run)["scope"], "mine");
        assert_eq!(stopped_for(&world, run, node)["payload"]["ended"], "killed");
    }
    for node in ["first", "second"] {
        world.release(&format!("{node}.go"));
    }
}

/// A run whose own records will not take a write is still shut down: the hold
/// and the journal lines that could not be written are said on stderr, and the
/// dispatch is still torn down and its branch still pushed.
///
/// The records are made unwritable with the file mode — the run directory for
/// the hold, the journal for the two kinds — which is what a full or read-only
/// mount looks like to the one writer that has to go on anyway.
// llmlint: ignore-block[tests_mirror_real_usage] the one thing set by hand is the mode of
// the run's own directory and journal, and no product surface sets it: a record that will
// not take a write is a host's disk refusing, not anything a user types. Everything else is
// the real binary against a real run and a real origin.
#[cfg(unix)]
#[test]
fn a_run_whose_records_refuse_a_write_is_still_shut_down_and_says_so() {
    use std::os::unix::fs::PermissionsExt;
    let world = World::new("shutdown-unwritable");
    let repository = world.repository("local-direct", &[]);
    held(&world, "service");
    let run = launch(&world, "unwritable", vec![lifecycle("service", &[])]);
    until_in_flight(&world, &run, &["service"]);
    let branch = session_branch(&world, &run, "service");
    let dir = world.run_file(&run, "");
    let journal = world.run_file(&run, "events.jsonl");
    assert!(
        journal.is_file(),
        "the run keeps no journal at {}",
        journal.display()
    );
    let set = |path: &Path, mode: u32| {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .expect("the mode is set");
    };
    set(&journal, 0o444);
    set(&dir, 0o555);

    let shutdown = world.run(&["shutdown", &run, "--force"]);
    set(&dir, 0o755);
    set(&journal, 0o644);

    shutdown
        .exited(0)
        .err_has("the hold that stops it dispatching could not be written")
        .err_has("dispatch-stopped record could not be written")
        .err_has("host-shutdown record could not be written");
    assert!(
        line_for(&shutdown.stdout, "service").contains("ended killed"),
        "{}",
        shutdown.stdout
    );
    assert!(
        on_origin(&world, &repository.origin, &branch).is_some(),
        "a run whose journal refused a write lost its branch:\n{}",
        shutdown.stdout
    );
    assert!(world.events_of(&run, "host-shutdown").is_empty());
    world.release("service.go");
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// An interrupt whose record the journal refuses is still delivered, and the
/// shutdown carries on and says what it could not write.
///
/// The run's directory still takes the hold, so the worker is asked; only the
/// journal refuses a line — which is what a journal the host will no longer
/// append to looks like to the one writer that has to go on anyway.
// llmlint: ignore-block[tests_mirror_real_usage] the one thing set by hand is the mode of
// the run's own journal, and no product surface sets it: a journal that will not take a
// line is a host's disk refusing, not anything a user types. Everything else is the real
// binary against a real run.
#[cfg(unix)]
#[test]
fn an_interrupt_the_journal_cannot_record_is_still_delivered_and_says_so() {
    use std::os::unix::fs::PermissionsExt;
    let world = World::new("shutdown-unrelayed");
    held(&world, "build");
    world.script("build.stops-when-interrupted", "");
    let run = launch(&world, "unrelayed", vec![agent("build", &[])]);
    until_in_flight(&world, &run, &["build"]);
    let journal = world.run_file(&run, "events.jsonl");
    let set = |mode: u32| {
        std::fs::set_permissions(&journal, std::fs::Permissions::from_mode(mode))
            .expect("the mode is set");
    };
    set(0o444);

    let shutdown = world.run(&["shutdown", &run, "--grace", GRACE]);
    set(0o644);

    shutdown
        .err_has("the record of its interrupt could not be written to the run's journal")
        .err_has("dispatch-stopped record could not be written");
    let line = line_for(&shutdown.stdout, "build");
    assert!(line.contains("interrupt delivered"), "{line}");
    assert!(
        world.events_of(&run, "host-shutdown").is_empty(),
        "the journal took a line it was set to refuse, so this journey proves nothing: {}",
        world.dump()
    );
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// A run whose hold could not be written is given no grace, so nothing new starts
/// in it.
///
/// The hold is what stops the driver dispatching, and waiting out a grace without
/// one is the window a new dispatch starts in: the node in front would take the ask
/// and settle, and what waited behind it would go. So only the hold is refused here
/// — the run's directory takes no new file while its journal still takes a line —
/// and the shutdown goes straight to the teardown, asking nothing.
// llmlint: ignore-block[tests_mirror_real_usage] the one thing set by hand is the mode of
// the run's own directory, and no product surface sets it: a directory that will not take
// a new file is a host's disk refusing, not anything a user types. Everything else is the
// real binary against a real run.
#[cfg(unix)]
#[test]
fn a_run_the_hold_cannot_be_written_for_is_given_no_grace_and_starts_nothing_new() {
    use std::os::unix::fs::PermissionsExt;
    let world = World::new("shutdown-unheld");
    held(&world, "first");
    world.script("first.stops-when-interrupted", "");
    held(&world, "holder");
    held(&world, "second");
    let run = launch(
        &world,
        "unheld",
        vec![
            agent("first", &[]),
            agent("holder", &[]),
            agent("second", &["first"]),
        ],
    );
    until_in_flight(&world, &run, &["first", "holder"]);
    let dir = world.run_file(&run, "");
    let set = |mode: u32| {
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(mode))
            .expect("the mode is set");
    };
    set(0o555);

    let began = Instant::now();
    let shutdown = world.run(&["shutdown", &run, "--grace", "60"]);
    let took = began.elapsed();
    set(0o755);

    shutdown
        .exited(0)
        .err_has("the hold that stops it dispatching could not be written")
        .err_has("nothing of it is asked or waited for and it goes straight to the teardown");
    assert!(
        took < Duration::from_secs(30),
        "a run with no hold waited out its grace ({took:?}):\n{}",
        shutdown.stdout
    );
    for node in ["first", "holder"] {
        assert_eq!(
            stopped_for(&world, &run, node)["payload"]["interrupt"],
            "not-asked"
        );
        assert!(
            line_for(&shutdown.stdout, node).contains("could not be written"),
            "{}",
            shutdown.stdout
        );
    }
    assert!(
        world
            .events_of(&run, "node-dispatched")
            .iter()
            .all(|event| event["labels"]["node"] != "second"),
        "a node was dispatched in a run the hold could not be written for: {}",
        world.dump()
    );
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// The report's last section names, by count and by branch, every other
/// unpublished branch on this host that the shutdown did not push.
///
/// The other branch is one a publication preserved when the repository's own
/// merge path refused it — work nothing has landed, in a run the shutdown was not
/// asked about.
#[cfg(unix)]
#[test]
fn the_last_section_names_every_other_unpublished_branch_on_the_host() {
    let world = World::new("shutdown-others").with_env("ONEPIPELINE_PUBLICATION_ATTEMPTS", "1");
    world.repository("local-direct", &["false"]);
    world.script("service.work", "the worker wrote this\n");
    let refused = launch(&world, "refused", vec![lifecycle("service", &[])]);
    world.until("the refused publication to settle", |world| {
        world.run_file(&refused, "result.json").is_file()
    });
    let left = world.run_json(&refused, "result.json")["nodes"][0]["branch"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| panic!("the refused node names no branch: {}", world.dump()));
    held(&world, "build");
    let run = launch(&world, "other", vec![agent("build", &[])]);
    until_in_flight(&world, &run, &["build"]);

    let shutdown = world.run(&["shutdown", &run, "--force"]);
    shutdown.exited(0);
    let last = shutdown
        .stdout
        .split("other unpublished branches on this host, which this shutdown did not push (")
        .nth(1)
        .unwrap_or_else(|| panic!("no section naming other branches:\n{}", shutdown.stdout));
    assert!(
        last.starts_with("1):"),
        "the count is not stated:\n{}",
        shutdown.stdout
    );
    assert!(
        last.lines()
            .nth(1)
            .is_some_and(|line| line.trim().ends_with(&format!("@{left}"))),
        "the section does not name {left}:\n{}",
        shutdown.stdout
    );
    world.release("build.go");
}

/// `status` and `results` read the **latest** shutdown a run carries, and say a
/// shutdown record that does not decode is unreadable rather than guessing.
///
/// A host shut down twice is the ordinary way to carry two: the second one's
/// words — `already-on-origin` — are the ones read. The unreadable half appends
/// records by hand, which is what a journal a newer or broken writer left looks
/// like to this build's reader.
// llmlint: ignore-block[tests_mirror_real_usage] the two records appended at the end are
// written by hand, because no verb of this build writes a `host-shutdown` or a
// `dispatch-stopped` its own document refuses; that is the reader's boundary under test.
// Everything before them is the real binary shutting a real run down twice.
#[test]
fn status_and_results_read_the_latest_shutdown_and_name_an_unreadable_one() {
    let world = World::new("shutdown-read-back");
    world.repository("local-direct", &[]);
    held(&world, "service");
    let run = launch(&world, "readback", vec![lifecycle("service", &[])]);
    until_in_flight(&world, &run, &["service"]);
    let branch = session_branch(&world, &run, "service");

    world.run(&["shutdown", &run, "--force"]).exited(0);
    world.run(&["shutdown", &run, "--force"]).exited(0);
    assert_eq!(world.events_of(&run, "host-shutdown").len(), 2);
    // The first shutdown killed the worker and the second found nothing live:
    // what is read is the second's account, not the first's carried over.
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("service: no live dispatch when the host shutdown ran")
        .out_lacks("service: worker killed by the host shutdown");
    world.run(&["results", &run]).exited(0).out_has(&format!(
        "its work is on {branch} (already on its origin at this commit)"
    ));

    let journal = world.run_file(&run, "events.jsonl");
    let mut text = std::fs::read_to_string(&journal).expect("the journal reads");
    for (seq, kind, node) in [
        (0, "dispatch-stopped", Some("service")),
        (1, "host-shutdown", None),
    ] {
        let mut labels = json!({"run_id": run});
        if let Some(node) = node {
            labels["node"] = json!(node);
        }
        text.push_str(
            &json!({
                "v": 2,
                "ts": "2099-01-01T00:00:00.000Z",
                "stream": "hand-written",
                "seq": seq,
                "source": "pipeline",
                "kind": kind,
                "labels": labels,
                "payload": {"ended": 7},
                "artifacts": [],
            })
            .to_string(),
        );
        text.push('\n');
    }
    std::fs::write(&journal, text).expect("the journal is written");

    world
        .run(&["status", &run])
        .exited(0)
        .out_has("HOST SHUTDOWN")
        .out_has("service: its dispatch-stopped record could not be read");
    world
        .run(&["results", &run])
        .exited(0)
        .out_has("unknown: the host-shutdown record could not be read");
    world.release("service.go");
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// A dispatch an earlier shutdown killed is over, even once the host has given
/// its pid to a stranger — and a stranger on a pid this run never ended is still
/// declined. The first shutdown kills the worker and its driver, so nothing
/// takes the worker's registry entry back.
// llmlint: ignore-block[tests_mirror_real_usage] PID reuse is a host transition, not a
// product operation, and waiting for this particular pid to be recycled is unbounded. The
// fixture moves only the pid a registry entry names — the entry the worker itself wrote,
// stamp untouched — onto a real process this test started, and adds one entry for the second
// half; the real `shutdown` command reads the real run root either way and must leave the
// stranger alive.
#[cfg(unix)]
#[test]
fn a_dispatch_an_earlier_shutdown_killed_is_over_once_its_pid_is_reissued() {
    let world = World::new("shutdown-reissued-after-kill");
    held(&world, "build");
    let run = launch(&world, "reissue", vec![agent("build", &[])]);
    until_in_flight(&world, &run, &["build"]);
    let entry = world.dispatch_records(&run).remove(0);
    let recorded: Value =
        serde_json::from_str(&std::fs::read_to_string(&entry).expect("the entry reads"))
            .expect("the entry is JSON");
    let stamp = recorded["started"]
        .as_str()
        .expect("the entry carries its stamp")
        .to_string();

    world
        .run(&["shutdown", &run, "--force"])
        .exited(0)
        .out_has("signalled — every process this run named was reached");
    assert_eq!(
        stopped_for(&world, &run, "build")["payload"]["ended"],
        "killed"
    );

    // The host hands the worker's pid on. What the registry is left holding is
    // the entry the worker wrote, stamp and all, now naming the stranger.
    let mut stranger = crate::driver::stranger_started_after(std::slice::from_ref(&stamp));
    let taken = stranger.id();
    let mut reissued = recorded.clone();
    reissued["pid"] = json!(taken);
    std::fs::write(&entry, reissued.to_string()).expect("the reissued pid is planted");

    world
        .run(&["shutdown", &run, "--force"])
        .exited(0)
        .out_has("nothing-to-stop — every process this run named was reached")
        .out_has("no live dispatch to interrupt")
        .err_lacks("since given to another process");
    assert!(
        stranger
            .try_wait()
            .expect("this host answers about the stranger")
            .is_none(),
        "a shutdown signalled pid {taken}, which the host had reissued"
    );

    // The same stranger under a stamp no teardown of this run ever ended is a
    // claim nothing places, and is declined exactly as it always was.
    let mut stray = recorded;
    stray["pid"] = json!(taken);
    stray["started"] = json!("a process no teardown of this run ended");
    std::fs::write(
        entry.with_file_name(format!("{taken}-stray.json")),
        stray.to_string(),
    )
    .expect("the stray claim is planted");
    world
        .run(&["shutdown", &run, "--force"])
        .exited(REFUSED)
        .out_has("every recorded identity disagreed")
        .err_has("since given to another process");
    assert!(
        stranger
            .try_wait()
            .expect("this host answers about the stranger")
            .is_none(),
        "a declined stranger was signalled"
    );
    stranger.kill().expect("this test ends its own process");
    stranger.wait().expect("the stranger is reaped");
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// A driver an earlier shutdown killed is over, even once the host has given its
/// pid to a stranger — the `cross (windows-latest)` failure of
/// `status_and_results_read_the_latest_shutdown_and_name_an_unreadable_one`,
/// whose second `shutdown --force` met the killed driver's pid reissued under
/// both the launch record and the ownership lock and refused the run. A record
/// naming that pid under a stamp this run never ended is still declined.
// llmlint: ignore-block[tests_mirror_real_usage] PID reuse is a host transition, not a
// product operation, and waiting for this particular pid to be recycled is unbounded. A
// driver's claims are named by the pid they were written for, so the host handing that pid
// on is staged as the one pid changing everywhere this run wrote it — the launch record, the
// lock, and the record of what the first shutdown ended — to a real process this test
// started, every stamp untouched. The real `shutdown` command reads the real run root each
// time and must leave the stranger alive.
#[cfg(unix)]
#[test]
fn a_driver_an_earlier_shutdown_killed_is_over_once_its_pid_is_reissued() {
    let world = World::new("shutdown-driver-reissued");
    held(&world, "build");
    let run = launch(&world, "driverreissue", vec![agent("build", &[])]);
    until_in_flight(&world, &run, &["build"]);
    let launch_record = world.run_file(&run, "launch.json");
    let read = |path: &Path| -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).expect("the record reads"))
            .expect("the record is JSON")
    };
    let driver = read(&launch_record)["pid"].clone();
    let stamp = read(&launch_record)["started"]
        .as_str()
        .expect("the launch record carries its stamp")
        .to_string();

    world
        .run(&["shutdown", &run, "--force"])
        .exited(0)
        .out_has("signalled — every process this run named was reached");

    // The host hands the driver's pid on. Every record that named it now names
    // the stranger; the lock is where the killed driver left it.
    let mut stranger = crate::driver::stranger_started_after(std::slice::from_ref(&stamp));
    let taken = json!(stranger.id());
    let lock = world.run_file(&run, "owner.lock");
    for path in [&launch_record, &lock] {
        let mut record = read(path);
        assert_eq!(
            record["pid"],
            driver,
            "{} named another pid",
            path.display()
        );
        record["pid"] = taken.clone();
        std::fs::write(path, record.to_string()).expect("the reissued pid is planted");
    }
    // And so does the record of what the first shutdown ended, which is the run's
    // own and follows the pid wherever it names the driver. Read as it is rather
    // than required, so a shutdown that recorded nothing is refused by the
    // command below rather than by this fixture.
    let ended = world.run_file(&run, "ended.jsonl");
    let kept = std::fs::read_to_string(&ended).unwrap_or_default();
    let lines: Vec<String> = kept
        .lines()
        .map(|line| {
            let mut line: Value = serde_json::from_str(line).expect("a JSON line");
            for claim in ["launch-record", "ownership-lock"] {
                if line["claim"][claim]["pid"] == driver {
                    line["claim"][claim]["pid"] = taken.clone();
                }
            }
            line.to_string()
        })
        .collect();
    if !lines.is_empty() {
        std::fs::write(&ended, format!("{}\n", lines.join("\n")))
            .expect("the record follows the pid");
    }

    world
        .run(&["shutdown", &run, "--force"])
        .exited(0)
        .out_has("nothing-to-stop — every process this run named was reached")
        .err_lacks("since given to another process");
    assert!(
        stranger
            .try_wait()
            .expect("this host answers about the stranger")
            .is_none(),
        "a shutdown signalled pid {taken}, which the host had reissued"
    );

    // The launch record under a stamp no teardown of this run ended is a claim
    // nothing places, and is declined exactly as it always was.
    let mut stray = read(&launch_record);
    stray["started"] = json!("a process no teardown of this run ended");
    std::fs::write(&launch_record, stray.to_string()).expect("the stray stamp is planted");
    world
        .run(&["shutdown", &run, "--force"])
        .exited(REFUSED)
        .out_has("every recorded identity disagreed")
        .err_has(&format!("the launch record names pid {taken}, which this host has since given to another process"));
    assert!(
        stranger
            .try_wait()
            .expect("this host answers about the stranger")
            .is_none(),
        "a declined stranger was signalled"
    );
    stranger.kill().expect("this test ends its own process");
    stranger.wait().expect("the stranger is reaped");
    world.release("build.go");
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// A shutdown that names no `--grace` gives each dispatch the ten-minute
/// default, and says so on its report and its record.
///
/// Proved through a worker that takes the ask, so the grace is given and never
/// waited out: the command returns as soon as the worker has gone, and the
/// deadline it was bounded by is the one the report and the record name.
#[cfg(unix)]
#[test]
fn a_shutdown_naming_no_grace_gives_the_ten_minute_default() {
    let world = World::new("shutdown-default-grace");
    held(&world, "build");
    world.script("build.stops-when-interrupted", "");
    let run = launch(&world, "defaultgrace", vec![agent("build", &[])]);
    until_in_flight(&world, &run, &["build"]);

    let since = Instant::now();
    let shutdown = world.run(&["shutdown", &run]);
    shutdown.exited(0).out_has("grace 600s");
    assert!(
        since.elapsed() < Duration::from_secs(120),
        "a worker that took the ask was waited on as if it had not"
    );
    assert!(
        line_for(&shutdown.stdout, "build").contains("killed in 600s if it has not exited"),
        "{}",
        shutdown.stdout
    );
    let recorded = host_shutdown(&world, &run);
    assert_eq!(recorded["grace_seconds"], 600, "{recorded}");
    assert_eq!(recorded["forced"], false, "{recorded}");
    assert_eq!(
        stopped_for(&world, &run, "build")["payload"]["ended"],
        "graceful"
    );
}

/// A grace this host's clock cannot count to is refused, naming it, before
/// anything is signalled — rather than overflowing the deadline mid-shutdown.
#[test]
fn a_grace_the_clock_cannot_count_to_is_refused_before_anything_is_signalled() {
    let world = World::new("shutdown-huge-grace");
    held(&world, "build");
    let run = launch(&world, "hugegrace", vec![agent("build", &[])]);
    until_in_flight(&world, &run, &["build"]);
    let worker = registered_pid(&world, &run, "build");

    let grace = u64::MAX.to_string();
    world
        .run(&["shutdown", &run, "--grace", &grace])
        .exited(REFUSED)
        .err_has(&format!("a grace of {grace}s"))
        .err_has("nothing was signalled");
    assert!(
        world.registered(&run, worker),
        "a refused shutdown ended the dispatch"
    );
    assert!(world.events_of(&run, "host-shutdown").is_empty());
    assert!(!world.run_file(&run, "shutting-down.json").exists());
    world.release("build.go");
}

/// An adoption that cannot lift a shutdown's hold refuses, naming it, rather
/// than driving a run that would go on dispatching nothing while reporting it
/// adopted.
// llmlint: ignore-block[tests_mirror_real_usage] the one thing set by hand is the hold's
// path, replaced with a directory so that it cannot be removed: no verb leaves a hold an
// adoption cannot lift, which is a host refusing the removal. The shutdown before it and the
// adoption refused after it are the real binary.
#[cfg(unix)]
#[test]
fn an_adoption_that_cannot_lift_the_hold_refuses_and_names_it() {
    let world = World::new("shutdown-stuck-hold");
    held(&world, "build");
    let run = launch(&world, "stuckhold", vec![agent("build", &[])]);
    until_in_flight(&world, &run, &["build"]);
    world.run(&["shutdown", &run, "--force"]).exited(0);

    let hold = world.run_file(&run, "shutting-down.json");
    std::fs::remove_file(&hold).expect("the hold the shutdown wrote");
    std::fs::create_dir_all(hold.join("held")).expect("a hold nothing can remove as a file");

    world
        .run(&["adopt", &run])
        .exited(REFUSED)
        .err_has("shutting-down.json");
    assert!(
        world.events_of(&run, "driver-adopted").is_empty(),
        "an adoption that could not lift the hold went ahead"
    );
    world.release("build.go");
}
// llmlint: ignore-end[tests_mirror_real_usage]
