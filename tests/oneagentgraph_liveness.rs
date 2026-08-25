//! The activity rule a dispatch is supervised by, driven through the linked
//! `oneagentgraph`'s own seam.
//!
//! Every node this crate dispatches runs as a **single-sided member** of an
//! agent graph, and that member is watched by `oneagentgraph`'s activity rule.
//! Two things clear that rule's clock — the member's own published events, and
//! live work under its tree — and a turn's prose is neither, so a member
//! spending a quarter of an hour composing a report is *silent* by its reading.
//! Under the ten-minute bound `oneagentgraph` 0.3.8 replaced, that member was
//! killed mid-report and its node lost with it. `Cargo.toml`'s pin block records
//! the floor; this is what holds it.
//!
//! # Why here, and not through the binary
//!
//! `src/agentgraph.rs` **spawns** `oneagentgraph`, so the rule runs in that
//! process and not in this one — and every journey under `tests/e2e/` puts a
//! double at that seam, which stands in for the whole supervisor. A journey
//! driving the compiled `onepipeline` therefore cannot reach this rule at all,
//! however long it waits. What it can reach is the linked *library*, whose
//! [`Stall`] is public and is the same code the spawned binary runs, so that is
//! what these drive: no double, no reimplementation, and the readings are the
//! kernel's own.
//!
//! # Two halves, because the change had two
//!
//! [`the_linked_default_bound_outlasts_a_member_writing_its_report`] is the
//! **number** 0.3.8 moved, read the way the sibling reads it at launch. It is
//! the half that fails against a stale lock.
//! [`the_activity_rule_condemns_a_silent_member_only_once_its_bound_elapses`] is
//! what that number is a bound *on*, driven over real processes and real elapsed
//! time under a bound this test's own environment sets small — seconds rather
//! than half an hour, which is the only reason the pair is quick.

use std::collections::BTreeMap;
use std::time::Duration;

use oneagentgraph::member::Bounds;

/// The bound `oneagentgraph` 0.3.8 replaced, and the window a report was being
/// killed inside.
const KILLED_REPORTS: Duration = Duration::from_secs(600);

/// The bound a dispatch really runs under outlasts the report it is writing.
///
/// Read through [`Bounds::from_env`] off an environment naming no override,
/// which is how the sibling resolves it when it launches a member — so this is
/// the value a dispatch gets rather than a constant copied beside it.
#[test]
fn the_linked_default_bound_outlasts_a_member_writing_its_report() {
    let bounds = Bounds::from_env(&BTreeMap::new())
        .expect("the linked oneagentgraph resolves the bounds it launches a member under");
    assert!(
        bounds.stall > KILLED_REPORTS,
        "the linked oneagentgraph condemns a silent member after {:?}, which is inside the \
         window a dispatch spends writing its report: the correction ships in 0.3.8, and \
         `Cargo.toml` requires the newest release, which is above that floor — so `Cargo.lock` \
         is behind the manifest too and `cargo update -p oneagentgraph` is the whole of the \
         fix",
        bounds.stall
    );
}

/// What that bound is a bound *on*: silence alone, and only past the bound.
///
/// The rule's own seam, over a real process tree and real elapsed time. A
/// member writing a report is silent **and idle** — it is waiting on a model,
/// so nothing under it is charged CPU — which is exactly the reading this drives
/// and exactly the one that used to be fatal. Both directions, because the
/// sparing half alone would pass against a watchdog switched off:
///
/// * a stamped tree doing nothing is condemned, and **not before its bound
///   elapses** — which is what makes the bound the whole of the judgement, and
///   therefore what makes the number above decide whether a report survives;
/// * a stamped tree doing work is never condemned, however long its member has
///   published nothing.
///
/// POSIX only, because the evidence is: a member's tree is the [`SCRATCH_ENV`]
/// stamp the kernel fixes at `exec`, and on Windows it is a job object, which
/// only the launcher of a tree can create — so a scratch this test stamped from
/// outside has no tree there to read.
///
/// [`SCRATCH_ENV`]: oneagentgraph::scratch::SCRATCH_ENV
#[cfg(unix)]
#[test]
fn the_activity_rule_condemns_a_silent_member_only_once_its_bound_elapses() {
    /// The bound this test supervises under, set the way an operator sets it.
    /// Small enough to spend seconds and not half an hour, and above
    /// `oneagentgraph`'s own probe floor so the rule gets a baseline and a
    /// comparison inside it.
    const BOUND: &str = "2";

    let bounds = Bounds::from_env(&BTreeMap::from([(
        oneagentgraph::liveness::STALL_TIMEOUT_ENV.to_owned(),
        BOUND.to_owned(),
    )]))
    .expect("the linked oneagentgraph reads the bound its environment names");
    let bound = bounds.stall;
    assert!(
        bound < KILLED_REPORTS,
        "this journey is only quick because the environment shortens the bound"
    );

    // A member that publishes nothing while nothing under it does any work,
    // which is what a member composing a report looks like from here.
    let idle = Tree::spawn("idle", &["sleep", "600"]);
    let condemned = drive(&idle, bound, bound * 8).expect(
        "the activity rule never condemned a member that published nothing and did no work, so \
         nothing here is measuring the silence a report is written in",
    );
    assert!(
        condemned > bound,
        "the activity rule condemned a silent member {condemned:?} into its life, inside its own \
         {bound:?} bound — if the bound is not the whole of the judgement then raising it is not \
         what saves a report"
    );

    // And the other direction: silence is not the finding, an *idle* tree is.
    let working = Tree::spawn("working", &["sh", "-c", "while :; do :; done"]);
    assert_eq!(
        drive(&working, bound, bound * 3),
        None,
        "the activity rule condemned a member with live work under it, so it is judging silence \
         rather than the evidence the silence is explained by"
    );
}

/// Ask the rule until it condemns, and answer how far into the member's life it
/// did — or `None` where it never did inside `give_up_after`.
#[cfg(unix)]
fn drive(tree: &Tree, bound: Duration, give_up_after: Duration) -> Option<Duration> {
    let started = Instant::now();
    let mut stall = oneagentgraph::member::Stall::new(bound, started);
    while started.elapsed() < give_up_after {
        // `0` is a member that has published nothing at all since it started,
        // which is the whole case: what is being judged is the silence.
        if stall.condemns(0, &tree.scratch) {
            return Some(started.elapsed());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

/// A real process tree under a scratch of its own, torn down with the test.
///
/// The stamp is applied to the child's environment rather than written down
/// anywhere, because that is what the sibling reads: the kernel fixes it at
/// `exec`, so it is a fact about a running process and not a claim this test
/// makes about one.
#[cfg(unix)]
struct Tree {
    scratch: std::path::PathBuf,
    child: std::process::Child,
}

#[cfg(unix)]
use std::time::Instant;

#[cfg(unix)]
impl Tree {
    fn spawn(name: &str, argv: &[&str]) -> Self {
        let scratch = std::env::temp_dir().join(format!(
            "onepipeline-liveness-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).expect("a scratch directory for the member's tree");
        let child = std::process::Command::new(argv[0])
            .args(&argv[1..])
            .env(
                oneagentgraph::scratch::SCRATCH_ENV,
                scratch.display().to_string(),
            )
            .current_dir(&scratch)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap_or_else(|error| panic!("cannot start the member's tree {argv:?}: {error}"));
        let tree = Self { scratch, child };
        // The stamp is fixed at `exec`, so a look taken before the child has
        // reached it finds no tree — and the rule reads that as "nothing to ask"
        // rather than as an idle one. Wait for the tree to exist before handing
        // it to the rule, so what is under test is the verdict and not the race.
        let waiting = Instant::now();
        while oneagentgraph::scratch::work(&tree.scratch).is_none() {
            assert!(
                waiting.elapsed() < Duration::from_secs(10),
                "the member's tree never became visible to the sibling's own stamp"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        tree
    }
}

#[cfg(unix)]
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}
