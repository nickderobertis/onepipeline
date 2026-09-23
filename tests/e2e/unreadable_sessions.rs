//! What this host does when it cannot read its own session records.
//!
//! The linked `onevcs` release refuses the operations this crate performs over the
//! host's session records — `session_holders`, `recoverable`, `pool_maintain`,
//! `workspace_capacity`, `preserve` and `session` — where it used to act on
//! partial state: an unreadable *directory* is no longer an empty listing, and a
//! record it enumerated and then could not load is no longer a session nobody
//! holds. "Nobody is in this workspace" and "this host could not say who is"
//! are opposite facts, and the only safe one to act on is the first.
//!
//! So every journey here puts the host into one of those two states and drives a
//! real command at it, once per state, asserting that what comes back **names the
//! refusal** rather than reporting an absence. The states are made the way a
//! broken host arrives at them — a sessions directory that is not a directory, and
//! a record that is not a document — and never by substituting anything: the
//! `onevcs` under test is the linked library, answering out of the real state root.
// llmlint: ignore-file[e2e_not_mocked] nothing here stands in for the layer under test.
// `onevcs` is the linked library reading a real state root, and what is arranged is
// the *state* it reads — the same state a host with a lost mount or a truncated write
// hands it. `oneagentgraph` is the established double, because these journeys are
// about a read of the host's records and a real agent turn is a paid one.

use std::path::PathBuf;

use serde_json::json;

use crate::harness::{lifecycle, plan_of, World, REFUSED};

/// The two ways this host cannot read its own session records.
///
/// Both are `onevcs`'s own refusals and not this crate's: the first is the
/// listing that could not be made at all, and the second the record the listing
/// enumerated and then could not load. Each is applied by [`break_records`] and
/// taken back by [`mend_records`], which leave everything else as they found it —
/// the journeys arrange one state after another over one host.
#[derive(Clone, Copy, Debug)]
enum Unreadable {
    /// The sessions directory is not a directory.
    Directory,
    /// One record inside it is not a document this host can read.
    Record,
}

impl Unreadable {
    /// Both, so a journey states its claim once and is held to it twice.
    const BOTH: [Self; 2] = [Self::Directory, Self::Record];

    /// A word for a run name, so the two states' runs do not collide.
    fn slug(self) -> &'static str {
        match self {
            Self::Directory => "directory",
            Self::Record => "record",
        }
    }
}

fn sessions(world: &World) -> PathBuf {
    world.onevcs_home().join("sessions")
}

fn unreadable_record(world: &World) -> PathBuf {
    sessions(world).join("s-unreadable.json")
}

fn aside(world: &World) -> PathBuf {
    world.onevcs_home().join("sessions.aside")
}

fn break_records(world: &World, which: Unreadable) {
    match which {
        Unreadable::Directory => {
            let dir = sessions(world);
            if dir.is_dir() {
                std::fs::rename(&dir, aside(world)).expect("the sessions directory moves aside");
            }
            std::fs::write(&dir, "this is not a directory")
                .expect("a file takes the sessions directory's place");
        }
        Unreadable::Record => {
            let dir = sessions(world);
            std::fs::create_dir_all(&dir).expect("the sessions directory exists");
            std::fs::write(unreadable_record(world), "{ not a session record")
                .expect("the unreadable record is written");
        }
    }
}

fn mend_records(world: &World, which: Unreadable) {
    match which {
        Unreadable::Directory => {
            let dir = sessions(world);
            std::fs::remove_file(&dir).expect("the file in the directory's place goes");
            if aside(world).is_dir() {
                std::fs::rename(aside(world), &dir).expect("the sessions directory comes back");
            }
        }
        Unreadable::Record => {
            std::fs::remove_file(unreadable_record(world)).expect("the unreadable record goes");
        }
    }
}

/// The launch interlock refuses rather than reading records it cannot read as a
/// workspace nobody holds.
///
/// `src/concurrency.rs` asks [`onevcs::session_holders`] about every repository a
/// plan names, and that answer is what decides whether a launch may proceed
/// beside work already going. An empty answer is a free workspace and a launch;
/// a refusal must never be read as one, because the live holder it would have
/// named is exactly what the interlock exists to find.
#[test]
fn a_launch_refuses_where_the_hosts_session_records_cannot_be_read() {
    let world = World::new("unreadable-launch");
    let _repository = world.repository("local-direct", &[]);
    world.script("build.work", "the interlock let this through\n");

    // Readable, the plan launches and settles, leaving nobody holding the
    // workspace — so what the refusals below name is the records and not the
    // plan, the repository, or the interlock itself.
    let admitted = world.plan("admitted", &plan_of("admitted", vec![lifecycle("build", &[])]));
    world
        .run(&["start", &admitted, "--attach"])
        .exited(0)
        .settled();

    for which in Unreadable::BOTH {
        break_records(&world, which);
        let name = format!("refused-{}", which.slug());
        let plan = world.plan(&name, &plan_of(&name, vec![lifecycle("build", &[])]));
        world
            .run(&["start", &plan, "--detach"])
            .exited(REFUSED)
            .err_has("cannot read the session holders of service")
            .err_has("onevcs");
        mend_records(&world, which);
    }
}

/// A view says it could not tell whether a workspace is free, rather than
/// reporting a queue.
///
/// `views::waiting_on` asks the same enumeration about the repository a ready
/// node would dispatch into. Three answers and not two: held, free, and a host
/// that could not be asked — because rendering the third as "queued for
/// dispatch" tells a supervisor to stop looking for what the node is waiting on,
/// which is a supervisor reading an unmeasured thing as a measured nothing.
#[test]
fn a_ready_nodes_line_says_the_host_could_not_be_asked_rather_than_that_it_is_queued() {
    let world = World::new("unreadable-view");
    let _repository = world.repository("local-direct", &[]);
    // One slot and two nodes, both held with an addressable turn: whichever the
    // driver takes, the other is the one left ready, and neither settles
    // underneath the reads below.
    let mut plan = plan_of(
        "waiting",
        vec![lifecycle("holder", &[]), lifecycle("waiter", &[])],
    );
    plan["concurrency"] = json!(1);
    let path = world.plan("waiting", &plan);
    for node in ["holder", "waiter"] {
        world.script(&format!("{node}.turn-open"), "");
        world.script(&format!("{node}.wait"), "hold");
    }
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("a node's turn to open", |world| {
        !world.events_of("waiting", "turn-started").is_empty()
    });

    // Readable, the answer is the **measured** one: the held node's own session
    // is in that workspace, and the line names it. That is what the refusals
    // below must not be collapsed into — nor into the third answer, a workspace
    // nothing holds.
    let queued = world.run(&["status", "waiting"]);
    queued.exited(0);
    let line = ready_line(&queued.stdout);
    assert!(
        line.contains("waiting for the 'service' workspace, held by session"),
        "{line}"
    );
    assert!(!line.contains("cannot say whether"), "{line}");

    for which in Unreadable::BOTH {
        break_records(&world, which);
        let asked = world.run(&["status", "waiting"]);
        asked.exited(0);
        let line = ready_line(&asked.stdout);
        assert!(
            line.contains("this host cannot say whether the 'service' workspace is free"),
            "{which:?}: {line}"
        );
        mend_records(&world, which);
    }

    for node in ["holder", "waiter"] {
        world.release(&format!("{node}.go"));
    }
    world.until("the run to settle", |world| {
        world.run_file("waiting", "result.json").is_file()
    });
}

/// The `status` line of whichever node is ready, whoever that turns out to be.
///
/// Which of two equal nodes a driver takes first is the driver's, so the claim is
/// about the node it left rather than about a name this journey chose.
fn ready_line(stdout: &str) -> &str {
    stdout
        .lines()
        .find(|line| line.contains(": ready — "))
        .unwrap_or_else(|| panic!("no node of the run is ready:\n{stdout}"))
}

/// A shutdown says what it could not read, and reports a preservation it could
/// not make, rather than reporting nothing to preserve.
///
/// Two callers on one command. `recoverable(&Scope::All)` is every other
/// unpublished branch on this host — a fact somebody decommissioning the machine
/// acts on, so an empty list from a failed enumeration is the one answer it may
/// never give — and `preserve` is the push that puts this run's own branches on
/// their origins, whose refusal is reported per branch and does not stop the
/// others.
///
/// A run per state, because a shutdown ends the run it read.
#[test]
fn a_shutdown_names_the_read_it_could_not_make_and_the_branch_it_could_not_preserve() {
    let world = World::new("unreadable-shutdown");
    let _repository = world.repository("local-direct", &[]);

    for which in Unreadable::BOTH {
        let name = format!("preserving-{}", which.slug());
        let path = world.plan(&name, &plan_of(&name, vec![lifecycle("build", &[])]));
        world.script("build.turn-open", "");
        world.script("build.wait", "hold");
        world.run(&["start", &path, "--detach"]).exited(0);
        world.until("the node's session to open", |world| {
            !world.events_of(&name, "session-opened").is_empty()
        });

        break_records(&world, which);
        // The shutdown itself is not refused for this — the other branches on
        // this host are a fact rather than a failure — so the report is where it
        // says so, and it says it as something other than a count of zero.
        let report = world.run(&["shutdown", &name, "--grace", "1"]);
        report
            .out_has("other unpublished branches on this host: not read — ")
            .out_has("This is not a count")
            .out_has("could not be preserved: ");
        mend_records(&world, which);
    }
}

/// A dispatch over records this host cannot read is refused by its open, and the
/// advisory read before it says it was the one that could not be made.
///
/// Three callers on one path, in the order the driver reaches them.
/// `pool::Workspaces::admit` asks `workspace_capacity` before the dispatch — an
/// advisory read whose refusal is **never** a reading that found room, so it is
/// said out loud and the open is left to decide; `vcs::session_open` is that
/// open, and its refusal is the node's, so the node is settled on it rather than
/// dispatched into a workspace nothing could account for; and `vcs::session` is
/// how the driver reads a session's record, which is what ends a follow.
///
/// The run is minted behind a human gate and then **adopted**, for two reasons a
/// simpler shape cannot give: the launch interlock asks the same records and
/// refuses first, so the dispatch is only reachable on a run accepted while they
/// read; and `adopt` drives in the invoking process, so what the driver says about
/// a read it could not make is on this command's own stderr, where a host reads it.
#[test]
fn a_dispatch_over_unreadable_records_is_refused_by_its_open_and_the_read_before_it_says_so() {
    let world = World::new("unreadable-dispatch");
    let _repository = world.repository("local-direct", &[]);
    world.script("build.work", "this never reaches a workspace\n");

    for which in Unreadable::BOTH {
        let name = format!("dispatch-{}", which.slug());
        let path = world.plan(
            &name,
            &plan_of(
                &name,
                vec![
                    crate::harness::human("approve", &[]),
                    lifecycle("build", &["approve"]),
                ],
            ),
        );
        world.run(&["start", &path, "--attach"]).exited(0);
        world.run(&["attest", &name, "approve"]).exited(0);

        break_records(&world, which);
        let adopted = world.run(&["adopt", &name]);
        adopted
            .err_has("cannot read what the 'service' workspace admits")
            .err_has("its own open decides");
        // The open's refusal is the node's, and it is the refusal of a dispatch
        // that never began rather than a verdict on the task: the sibling would
        // not cut a workspace, so nothing of the agent's ran.
        let settled = world.run_json(&name, "result.json");
        let build = settled["nodes"]
            .as_array()
            .expect("the result names its nodes")
            .iter()
            .find(|node| node["id"] == "build")
            .unwrap_or_else(|| panic!("{name} recorded no build node: {settled}"))
            .clone();
        assert_eq!(build["outcome"], "infrastructure-failure", "{settled}");

        // And the settlement carries the sibling's own reason, so a reader learns
        // what could not be read rather than only that something could not.
        let settlements = world.events_of(&name, "node-settled");
        let detail = settlements
            .iter()
            .filter(|event| event["labels"]["node"] == "build")
            .filter_map(|event| event["payload"]["detail"].as_str().map(str::to_owned))
            .next_back()
            .unwrap_or_else(|| panic!("{name}'s settlement carries no detail: {settlements:?}"));
        assert!(
            detail.contains("session") || detail.contains("onevcs"),
            "{detail}"
        );
        mend_records(&world, which);
    }
}

/// A follow whose session record goes unreadable underneath it ends, and says
/// that is what ended it.
///
/// `vcs::settled` is the follow thread's poll: it reads the session's record each
/// pass and stops once the session has closed. The linked release refuses a record
/// it cannot read rather than answering that there is no such session, and a
/// follow that ended on the refusal is not one that ended on the close — ending it
/// is still the safe direction, since a thread reading a stream nobody will ever
/// close is one this process would never collect, but ending it in silence leaves
/// a reader thinking the stream ran out.
#[test]
fn a_follow_whose_session_record_goes_unreadable_says_that_is_what_ended_it() {
    for which in Unreadable::BOTH {
        a_follow_interrupted_by(which);
    }
}

/// The journey above, over one state.
///
/// A world each, because the hold on the publishing push is installed as the
/// repository's own hook when the repository is made, and a world has one
/// repository.
///
/// Two things have to be true at the same instant for the claim to mean anything,
/// and both are waited for rather than timed. The follow has to be **live** — it
/// starts only once the dispatch's step has drained, so it does not exist while
/// the worker is working — and the publication it follows has to still be
/// running, or the driver's teardown could stop the thread before it read
/// anything. A record the live follow has relayed is both at once: it proves the
/// thread is polling, and the publication is held at the repository's merge path
/// until this journey lets it through.
fn a_follow_interrupted_by(which: Unreadable) {
    let world = World::new(&format!("unreadable-follow-{}", which.slug()));
    let go = world.fakes.join("push.go");
    let held = crate::harness::held_publication(&world, &go);
    world.repository("local-direct", &held.argv());
    world.script("build.work", "the follow saw this\n");
    let path = world.plan("followed", &plan_of("followed", vec![lifecycle("build", &[])]));

    // Attached, so the child *is* the run's driver and the follow thread it starts
    // writes to a stderr this journey can read; in a file, so the read is a poll
    // of the child's own announcement rather than a clock.
    let log = world.root.join("driver.err");
    let errors = std::fs::File::create(&log).expect("the driver's stderr file");
    let driver = world
        .cmd(&["start", &path, "--attach"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::from(errors))
        .spawn()
        .expect("the driver starts");
    world.until("the follow to relay the publication it is following", |world| {
        !world.events_of("followed", "merge-queued").is_empty()
    });
    let token = world.events_of("followed", "session-opened")[0]["payload"]["token"]
        .as_str()
        .expect("the session names itself")
        .to_owned();
    let record = sessions(&world).join(format!("{token}.json"));
    let saved = std::fs::read_to_string(&record).expect("the session has a record");

    // For the record state it is **this** session's own record that goes, which is
    // the one the poll reads; for the other, the directory holding it.
    match which {
        Unreadable::Directory => break_records(&world, which),
        Unreadable::Record => std::fs::write(&record, "{ not a session record")
            .expect("the session's record is made unreadable"),
    }
    let said = format!("cannot read session {token}'s record, so the follow of its stream ends");
    world.until("the follow to say which read ended it", |_| {
        std::fs::read_to_string(&log)
            .unwrap_or_default()
            .contains(&said)
    });

    // Put back before the push goes through, so the run winds down over a host
    // that reads and the child ends rather than being left behind.
    match which {
        Unreadable::Directory => mend_records(&world, which),
        Unreadable::Record => std::fs::write(&record, &saved).expect("the record comes back"),
    }
    held.release();
    crate::harness::ended(driver);
}
