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
//! [`Stall`]: oneagentgraph::member::Stall
//!
//! # Two halves, because the change had two
//!
//! [`the_linked_default_bound_outlasts_a_member_writing_its_report`] is the
//! **number** 0.3.8 moved, read the way the sibling reads it at launch. It is
//! the half that fails against a stale lock.
//! [`the_activity_rule_condemns_a_member_once_the_work_under_it_stops_and_not_while_it_arrives`]
//! is what that number is a bound *on*, driven over real processes and real
//! elapsed time under a bound this test's own environment sets small — seconds
//! rather than half an hour, which is the only reason the pair is quick.
//!
//! # What the second half judges, and what it no longer does
//!
//! It judges the **order and spacing of activity**: a tree whose work keeps
//! arriving inside a generous watchdog interval is spared, and one whose work
//! has stopped is condemned once the bound elapses. It does not judge how long
//! the host took to get there.
//!
//! It used to, against absolute elapsed-time windows, and a GitHub-hosted macOS
//! runner starves a spin loop past them under ordinary load: three such runs
//! condemned a spinning tree 5.3–5.45 s in and each cost a valid change a manual
//! rerun (issue #415). A starved tree is charged no CPU and the rule reads
//! exactly that, so the window was measuring the runner and reporting it as this
//! crate's defect. Those readings survive as
//! [`the_timings_the_old_gate_asserted_are_measured_and_never_judged`].

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

/// The bound the two driven halves supervise under, set the way an operator sets
/// it.
///
/// Small enough to spend seconds and not half an hour, and above
/// `oneagentgraph`'s own probe floor so the rule gets a baseline and a
/// comparison inside it.
#[cfg(unix)]
const BOUND: &str = "2";

/// The cadence every look in this file is taken on.
///
/// The sibling's own floor on how often it examines a quiet member's tree — how
/// much rarer than that it examines one is its business and changes without this
/// file. Looking on the floor is what this half needs from it: the rule can
/// never take a reading this loop did not take one beside, and
/// [`Stall::condemns`](oneagentgraph::member::Stall::condemns) is driven from
/// the same iteration, so the pair the rule compares lies inside the span this
/// loop recorded look by look.
#[cfg(unix)]
const LOOK_EVERY: Duration = oneagentgraph::member::HEARTBEAT_INTERVAL;

/// The longest this file waits for anything, and a **backstop rather than a
/// bound**: nothing is asserted about how much of it a verdict spends, and a
/// loaded runner is free to spend all of it.
///
/// It exists so a look that never answers ends as a named failure well inside
/// nextest's `terminate-after` rather than as a binary that hangs.
#[cfg(unix)]
const BACKSTOP: Duration = Duration::from_secs(60);

/// How many activity events the busy half watches for before it has seen enough.
///
/// The watch ends on the **evidence** rather than on a clock: enough events that
/// the rule has been given its opportunities to condemn and declined them — see
/// the `enough` closure at that call site for the other half of that. A runner
/// that needs a minute to produce them is slow, not wrong, and nothing here
/// reads it as wrong.
#[cfg(unix)]
const ACTIVITY_EVENTS: usize = 8;

/// A member that publishes nothing while nothing under it does any work, which
/// is what a member composing a report looks like from here.
#[cfg(unix)]
const IDLE: &[&str] = &["sleep", "600"];

/// A member that publishes nothing while its tree is charged CPU the whole time.
#[cfg(unix)]
const BUSY: &[&str] = &["sh", "-c", "while :; do :; done"];

/// The bound this file's driven halves run under, read the way the sibling reads
/// it at launch.
#[cfg(unix)]
fn bound() -> Duration {
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
    bound
}

/// The interval activity is held to arriving inside: **a gap between activity
/// events**, never a total elapsed time.
///
/// Twice the bound the rule condemns after — deliberately generous, because
/// what this has to separate is a tree that
/// stopped from a runner that was merely slow, and the window it replaces could
/// not (issue #415). A spinning process charged less than
/// [`WORKING_PERCENT_OF_A_CORE`](oneagentgraph::scratch::WORKING_PERCENT_OF_A_CORE)%
/// of a core across a window this long has not been delayed; it has stopped, and
/// the rule condemning it would be the rule reading the truth.
#[cfg(unix)]
fn watchdog(bound: Duration) -> Duration {
    bound * 2
}

/// What that bound is a bound *on*: work stopping, and only past the bound.
///
/// A member writing a report is silent **and idle** — it waits on a model, so
/// nothing under it is charged CPU — which is the reading this drives. Both
/// directions, because the sparing half alone would pass against a watchdog
/// switched off: a stamped tree doing nothing is condemned and not before its
/// bound elapses, and a stamped tree whose work keeps arriving inside the
/// watchdog interval is never condemned.
///
/// Neither direction reads a clock the host controls. The idle verdict is held
/// against the rule's *own* bound, which load can only lengthen. The busy one is
/// a gap between activity events — and where a runner starves the spin loop past
/// the watchdog, the rule is judging precisely the readings this test took, so
/// what is asserted is that the work had stopped before the verdict.
///
/// POSIX only, because the evidence is: a member's tree is the [`SCRATCH_ENV`]
/// stamp the kernel fixes at `exec`, and on Windows it is a job object, which
/// only the launcher of a tree can create — so a scratch this test stamped from
/// outside has no tree there to read.
///
/// [`SCRATCH_ENV`]: oneagentgraph::scratch::SCRATCH_ENV
// llmlint: ignore-block[live_tier_compiles_and_requires_credential] that rule's
// `**/*live*` glob matches this file on the word "liveness", but this is not the
// credentialled tier — that is the `smoke` binary. The POSIX gate is the
// paragraph above, not a skip: compiled everywhere with an early return, this
// would report a platform green for a mechanism it never exercised.
#[cfg(unix)]
#[test]
fn the_activity_rule_condemns_a_member_once_the_work_under_it_stops_and_not_while_it_arrives() {
    let bound = bound();

    let idle_tree = Tree::spawn("idle", IDLE).unwrap_or_else(|why| panic!("{why}"));
    let idle = watch(&idle_tree, bound, |_| false);
    assert_eq!(
        idle.events(),
        0,
        "the tree this half calls idle was charged CPU at {:?} into its life, so what the rule \
         judged is not a member doing nothing and nothing here is measuring the silence a report \
         is written in",
        idle.activity()
    );
    let Some(condemned) = idle.verdict() else {
        panic!(
            "the activity rule never condemned a member that published nothing and did no work, \
             over {} looks and {:?} — so nothing here is measuring the silence a report is \
             written in. That window is a backstop and not a bound: what failed is that no \
             verdict arrived at all, however long it was given",
            idle.looks.len(),
            idle.spent
        )
    };
    // The member published nothing, so its silence *is* its life: `condemned` is
    // the reading the rule compared against its own bound, not a wall-clock
    // window this test chose.
    assert!(
        condemned > bound,
        "the activity rule condemned a silent member {condemned:?} into its life, inside its own \
         {bound:?} bound — if the bound is not the whole of the judgement then raising it is not \
         what saves a report"
    );

    let busy_tree = Tree::spawn("working", BUSY).unwrap_or_else(|why| panic!("{why}"));
    let busy = watch(&busy_tree, bound, |watch| {
        watch.events() >= ACTIVITY_EVENTS && watch.spent > bound * 2
    });
    assert!(
        busy.events() > 0,
        "nothing under the busy tree was charged {}% of a core in any of {} looks over {:?}, so \
         the spin loop this half spawns never ran and neither direction of the rule is under \
         test here",
        oneagentgraph::scratch::WORKING_PERCENT_OF_A_CORE,
        busy.looks.len(),
        busy.spent
    );
    match busy.verdict() {
        None => {
            assert!(
                busy.events() >= ACTIVITY_EVENTS,
                "the busy tree produced {} activity events in {:?}, short of the {ACTIVITY_EVENTS} \
                 this half watches for, so the backstop ended the watch before the rule had been \
                 given its opportunities to condemn: {:?}",
                busy.events(),
                busy.spent,
                busy.activity()
            );
            let quiet = busy.longest_quiet();
            assert!(
                quiet <= watchdog(bound),
                "the work under the busy tree went {quiet:?} without being charged {}% of a core \
                 — past the {:?} watchdog interval — while the rule spared it anyway, so the rule \
                 is sparing a member on something other than the evidence under it. Activity \
                 arrived at {:?} over {:?}",
                oneagentgraph::scratch::WORKING_PERCENT_OF_A_CORE,
                watchdog(bound),
                busy.activity(),
                busy.spent
            );
        }
        Some(at) => {
            // The runner starved the spin loop, and the rule is then judging the
            // readings this loop took: what is asserted is the *order* — that the
            // work had stopped before the verdict. Stopped means a look that
            // found none, anywhere in the bound the rule judged, rather than a
            // gap measured against the rule's examination cadence: a tree
            // charged a working rate in every one of these windows is charged
            // one over any span built out of them, so this holds whatever
            // cadence the rule examines on.
            assert!(
                !busy.worked_throughout(at.saturating_sub(bound), at),
                "the activity rule condemned a member {at:?} into its life though every look \
                 across the {bound:?} before the verdict — the same readings, taken in the same \
                 iterations the rule took its own in — found work under it. Activity arrived at \
                 {:?} over {:?}",
                busy.activity(),
                busy.spent
            );
        }
    }
}

/// The timings the gate above used to assert, kept as a measurement and never as
/// a judgement.
///
/// Each reading is printed beside the window the old gate held it to and whether
/// it fell inside; a host that cannot start a tree says so and stops.
/// `.config/nextest.toml` prints this binary's output on success, so a green run
/// carries the numbers too.
// llmlint: ignore-block[tests_assert_real_behavior] this asserts nothing by
// design: what its readings move with is the runner's load rather than this
// crate, which is what made them a flaky gate (issue #415), so any assertion on
// one puts that flake back. The behaviour the two trees demonstrate is asserted
// in full immediately above, over the same helper and the same trees.
#[cfg(unix)]
#[test]
fn the_timings_the_old_gate_asserted_are_measured_and_never_judged() {
    let bound = bound();
    println!("the activity rule under a {bound:?} bound — measured, and judged by nothing:");

    match Tree::spawn("measured-idle", IDLE) {
        Err(why) => println!("  idle tree            not measured: {why}"),
        Ok(tree) => {
            let idle = watch(&tree, bound, |_| false);
            match idle.verdict() {
                Some(at) => println!(
                    "  idle tree condemned  {at:?} into its life — the old gate asserted after \
                     {bound:?} and inside {:?}: {}",
                    bound * 8,
                    if at > bound && at < bound * 8 {
                        "inside"
                    } else {
                        "outside"
                    }
                ),
                None => println!(
                    "  idle tree            never condemned over {} looks and {:?}",
                    idle.looks.len(),
                    idle.spent
                ),
            }
        }
    }

    match Tree::spawn("measured-working", BUSY) {
        Err(why) => println!("  busy tree            not measured: {why}"),
        Ok(tree) => {
            let window = bound * 3;
            let busy = watch(&tree, bound, |watch| watch.spent >= window);
            match busy.verdict() {
                Some(at) => println!(
                    "  busy tree condemned  {at:?} into its life — the old gate asserted not \
                     inside {window:?}: outside. A starved spin loop is charged no CPU and the \
                     rule reads exactly that, which is why this is a reading and not a failure"
                ),
                None => println!(
                    "  busy tree            not condemned over {:?} — the old gate asserted not \
                     inside {window:?}: inside",
                    busy.spent
                ),
            }
            println!(
                "  busy tree activity   {} of {} windows charged at least {}% of a core, longest \
                 quiet gap {:?}",
                busy.events(),
                busy.looks.len().saturating_sub(1),
                oneagentgraph::scratch::WORKING_PERCENT_OF_A_CORE,
                busy.longest_quiet()
            );
        }
    }
}
// llmlint: ignore-end[tests_assert_real_behavior]

#[cfg(unix)]
struct Look {
    at: Duration,
    /// The activity event this whole gate is written in terms of: the tree was
    /// charged enough CPU since the look before to count as working by
    /// [`Work::worked`](oneagentgraph::scratch::Work::worked), the sibling's own
    /// rate test, over the evidence the rule judges and at its cadence.
    working: bool,
    condemned: bool,
}

#[cfg(unix)]
struct Watch {
    looks: Vec<Look>,
    /// How far into the member's life the last look was taken, whatever ended
    /// the watch.
    spent: Duration,
}

#[cfg(unix)]
impl Watch {
    /// How far into the member's life the rule first condemned it, or `None`
    /// where it never did.
    fn verdict(&self) -> Option<Duration> {
        self.looks
            .iter()
            .find(|look| look.condemned)
            .map(|look| look.at)
    }

    fn activity(&self) -> Vec<Duration> {
        self.looks
            .iter()
            .filter(|look| look.working)
            .map(|look| look.at)
            .collect()
    }

    fn events(&self) -> usize {
        self.looks.iter().filter(|look| look.working).count()
    }

    /// Whether every look taken in `from..=to` found the tree working, over at
    /// least one look.
    fn worked_throughout(&self, from: Duration, to: Duration) -> bool {
        let mut looks = self
            .looks
            .iter()
            .filter(|look| look.at > from && look.at <= to)
            .peekable();
        looks.peek().is_some() && looks.all(|look| look.working)
    }

    /// The longest the tree went without an activity event — counting the wait
    /// for the first one, and the wait after the last that nothing ended.
    fn longest_quiet(&self) -> Duration {
        let mut previous = Duration::ZERO;
        let mut longest = Duration::ZERO;
        for at in self.activity() {
            longest = longest.max(at.saturating_sub(previous));
            previous = at;
        }
        longest.max(self.spent.saturating_sub(previous))
    }
}

/// Watch `tree` under the rule, reading the rule's own evidence at the rule's
/// own cadence, until `enough` says the watch has seen what it came for — or the
/// member is condemned, or [`BACKSTOP`] expires.
///
/// The reading above each verdict and the reading the rule takes inside it are
/// two calls in one iteration of this loop, so they are of the same tree at the
/// same moment and the pairs they compare are the same pair. That is what lets
/// either half say what the rule saw when it decided, rather than guessing from
/// a clock.
#[cfg(unix)]
fn watch(tree: &Tree, bound: Duration, enough: impl Fn(&Watch) -> bool) -> Watch {
    let started = Instant::now();
    let mut stall = oneagentgraph::member::Stall::new(bound, started);
    let mut watch = Watch {
        looks: Vec::new(),
        spent: Duration::ZERO,
    };
    let mut before: Option<(Instant, oneagentgraph::scratch::Work)> = None;
    while started.elapsed() < BACKSTOP {
        let now = Instant::now();
        let work = oneagentgraph::scratch::work(&tree.scratch);
        let working = match (before, work) {
            (Some((taken, before)), Some(work)) => before.worked(work, now.duration_since(taken)),
            // Nothing stamped is not a reading of zero, and the look after it is
            // a fresh baseline rather than half of a comparison — the rule
            // discards its own sample there for exactly the same reason.
            _ => false,
        };
        before = work.map(|work| (now, work));
        // `0` is a member that has published nothing at all since it started,
        // which is the whole case: what is being judged is the silence.
        let condemned = stall.condemns(0, &tree.scratch);
        watch.spent = started.elapsed();
        watch.looks.push(Look {
            at: watch.spent,
            working,
            condemned,
        });
        if condemned || enough(&watch) {
            break;
        }
        std::thread::sleep(LOOK_EVERY);
    }
    watch
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
    /// Start a tree, or say what stopped it from starting.
    ///
    /// Fallible rather than panicking, because one of the two callers is a
    /// measurement that may not fail the gate: a host that cannot start `sh` has
    /// nothing to report, which is not the same fact as a rule that misjudged a
    /// tree.
    fn spawn(name: &str, argv: &[&str]) -> Result<Self, String> {
        let scratch = std::env::temp_dir().join(format!(
            "onepipeline-liveness-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch)
            .map_err(|error| format!("cannot make a scratch directory for {name}: {error}"))?;
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
            .map_err(|error| format!("cannot start the member's tree {argv:?}: {error}"))?;
        let tree = Self { scratch, child };
        // The stamp is fixed at `exec`, so a look taken before the child has
        // reached it finds no tree — and the rule reads that as "nothing to ask"
        // rather than as an idle one. Wait for the tree to exist before handing
        // it to the rule, so what is under test is the verdict and not the race.
        let waiting = Instant::now();
        while oneagentgraph::scratch::work(&tree.scratch).is_none() {
            if waiting.elapsed() >= BACKSTOP {
                return Err(format!(
                    "the tree {argv:?} never became visible to the sibling's own stamp, over {:?}",
                    waiting.elapsed()
                ));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(tree)
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

// llmlint: ignore-end[live_tier_compiles_and_requires_credential]
