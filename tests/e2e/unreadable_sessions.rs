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
/// enumerated and then could not load.
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

/// Where the host keeps its session records.
fn sessions(world: &World) -> PathBuf {
    world.onevcs_home().join("sessions")
}

/// The record this journey writes that is not a document.
fn unreadable_record(world: &World) -> PathBuf {
    sessions(world).join("s-unreadable.json")
}

/// Where the directory is put while a journey holds a file in its place.
fn aside(world: &World) -> PathBuf {
    world.onevcs_home().join("sessions.aside")
}

/// Put the host into `which`, and leave everything else exactly as it was.
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

/// Put it back, so the next state is arranged over the host the last one was.
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
/// unpublished branch on this host — a fact a person decommissioning the machine
/// acts on, so an empty list from a failed enumeration is the one answer it may
/// never give — and `preserve` is the push that puts this run's own branches on
/// their origins.
#[test]
fn a_shutdown_names_the_read_it_could_not_make_and_the_branch_it_could_not_preserve() {
    let world = World::new("unreadable-shutdown");
    let _repository = world.repository("local-direct", &[]);
    let path = world.plan("preserving", &plan_of("preserving", vec![lifecycle("build", &[])]));
    world.script("build.wait", "hold");
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the node's session to open", |world| {
        !world.events_of("preserving", "session-opened").is_empty()
    });

    for which in Unreadable::BOTH {
        break_records(&world, which);
        // The shutdown itself is not refused — the other branches on this host
        // are a fact rather than a failure — so the report is where it says so.
        let report = world.run(&["shutdown", "--mine", "--grace", "1"]);
        report
            .out_has("other unpublished branches on this host: not read — ")
            .out_has("This is not a count");
        mend_records(&world, which);
    }
}
