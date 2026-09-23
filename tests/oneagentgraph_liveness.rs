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
//!
//! Three trees, because one quiet look is not a tree that stopped. A spinning
//! tree and a silent one bracket the rule; between them is one working in
//! bursts, whose pauses swallow whole looks while activity keeps arriving either
//! side. So a condemnation answers to the **gap between activity events** ending
//! at it, never to the look at it.

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
/// Small enough to spend seconds and not half an hour, above `oneagentgraph`'s
/// own probe floor so the rule gets a baseline and a comparison inside it, and
/// wide enough for [`watchdog`] to fit between the two windows [`bound`] asserts
/// it between — which the two seconds this began at was not.
#[cfg(unix)]
const BOUND: &str = "6";

/// The cadence every look in this file is taken on: under the sibling's own
/// probe floor, so the rule never takes a reading without one of this loop's
/// beside it, and a pause is resolved finer than the rule resolves it — which is
/// what lets a look find nothing while the rule's wider window over the same
/// pause still finds work.
#[cfg(unix)]
fn look_every() -> Duration {
    oneagentgraph::member::HEARTBEAT_INTERVAL / 2
}

/// The longest this file waits for anything, and a **backstop rather than a
/// bound**: nothing is asserted about how much of it a verdict spends, and a
/// loaded runner is free to spend all of it.
///
/// It exists so a look that never answers ends as a named failure well inside
/// nextest's `terminate-after` rather than as a binary that hangs.
#[cfg(unix)]
const BACKSTOP: Duration = Duration::from_secs(60);

/// How many activity events a failure message names, most recent last.
///
/// A watch takes four readings a second and runs for tens of seconds, so the
/// whole list is hundreds of timestamps and buries the count and the gap printed
/// beside it. Where activity was when the verdict landed is what a reader of one
/// of these is after.
#[cfg(unix)]
const REPORTED_ACTIVITY: usize = 8;

/// How many activity events the busy half watches for before it has seen enough.
///
/// The watch ends on the **evidence** rather than on a clock: enough events that
/// the rule has been given its opportunities to condemn and declined them — see
/// the `enough` closure at that call site for the other half of that. A runner
/// that needs a minute to produce them is slow, not wrong, and nothing here
/// reads it as wrong.
#[cfg(unix)]
const ACTIVITY_EVENTS: usize = 8;

/// The silence a report is written in: nothing published, and nothing under the
/// member charged for it either.
#[cfg(unix)]
const IDLE: &[&str] = &["sleep", "600"];

/// That same silence with live work under it.
#[cfg(unix)]
const BUSY: &[&str] = &["sh", "-c", "while :; do :; done"];

/// How long the tree below stops for between bursts of work.
///
/// Several looks wide, so whole look windows fall inside a pause even where a
/// loaded host stretches the cadence — and narrow enough that the gap a pause
/// opens stays inside [`watchdog`], which is what makes a verdict over one a
/// judgement rather than an excuse.
#[cfg(unix)]
fn pause() -> Duration {
    look_every() * 4
}

/// A member that publishes nothing while its tree works in bursts, stopping for
/// [`pause`] between them.
///
/// The burst is held to the clock rather than to a count of iterations, because
/// what a count costs is the host's to decide and what this needs is a burst
/// several looks wide on any of them. `date` is asked for the time rather than
/// the shell, whose `SECONDS` is not POSIX; the inner count is what keeps that
/// question from being asked thousands of times a second.
#[cfg(unix)]
fn intermittent() -> Vec<String> {
    vec![
        "sh".to_owned(),
        "-c".to_owned(),
        format!(
            "while :; do until=$(( $(date +%s) + 2 )); \
             while [ \"$(date +%s)\" -lt \"$until\" ]; do \
             i=0; while [ $i -lt 5000 ]; do i=$((i+1)); done; done; \
             sleep {}; done",
            pause().as_secs_f32()
        ),
    ]
}

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
    assert!(
        watchdog() > pause() + look_every() * 2,
        "an activity gap of {:?} is what a pause under the bursting tree opens, and the {:?} \
         watchdog interval is inside it — so that tree's missed observations would excuse a \
         verdict instead of being judged by one",
        pause() + look_every() * 2,
        watchdog()
    );
    assert!(
        watchdog() * 2 < bound,
        "the {:?} watchdog interval is not comfortably under the {bound:?} bound, so a quiet \
         stretch long enough for the rule to reach a verdict over would fit inside it and be \
         read as work still arriving",
        watchdog()
    );
    bound
}

/// The interval activity is held to arriving inside: **a gap between activity
/// events**, never a total elapsed time.
///
/// Counted in looks rather than in bounds, because what it separates is a tree
/// that stopped from a look that missed one that had not — the distinction the
/// elapsed-time window it replaces could not draw. It has to clear the gap a
/// pause under [`intermittent`] opens and stay under the shortest quiet a
/// verdict can sit at the end of; [`bound`] asserts both, and the driven half
/// prints the gap it measured, so a host that narrows either says so rather
/// than waiting to go flaky.
#[cfg(unix)]
fn watchdog() -> Duration {
    look_every() * 8
}

/// The longest quiet this half accepts a **spared** verdict over.
///
/// Twice the bound: a member left uncharged that long and spared anyway is the
/// rule failing to look. Wide where [`watchdog`] is narrow, because the two
/// answer opposite questions.
#[cfg(unix)]
fn longest_spared_quiet(bound: Duration) -> Duration {
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
/// The third tree is that second direction over a member a single look gets
/// wrong, and without it one missed observation excuses any verdict at all.
///
/// Neither direction reads a clock the host controls. The idle verdict is held
/// against the rule's *own* bound, which load can only lengthen. The other two
/// are gaps between activity events — and where a runner starves a tree past the
/// watchdog, the rule is judging precisely the readings this test took, so what
/// is asserted is that activity had stopped arriving before the verdict.
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
        "the tree this half calls idle was charged CPU {} times, latest at {:?} into its life, \
         so what the rule judged is not a member doing nothing and nothing here is measuring the \
         silence a report is written in",
        idle.events(),
        idle.recent_activity()
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
                 given its opportunities to condemn, the last of them at {:?}",
                busy.events(),
                busy.spent,
                busy.recent_activity()
            );
            let quiet = busy.longest_quiet();
            assert!(
                quiet <= longest_spared_quiet(bound),
                "the work under the busy tree went {quiet:?} without being charged {}% of a core \
                 — past the {:?} a sparing verdict is accepted over — while the rule spared it \
                 anyway, so the rule is sparing a member on something other than the evidence \
                 under it. {} activity events over {:?}, the last of them at {:?}",
                oneagentgraph::scratch::WORKING_PERCENT_OF_A_CORE,
                longest_spared_quiet(bound),
                busy.events(),
                busy.spent,
                busy.recent_activity()
            );
        }
        Some(at) => condemned_only_past_the_watchdog(&busy, at, "spinning"),
    }

    let bursts = intermittent();
    let bursts: Vec<&str> = bursts.iter().map(String::as_str).collect();
    let bursting_tree = Tree::spawn("bursting", &bursts).unwrap_or_else(|why| panic!("{why}"));
    let bursting = watch(&bursting_tree, bound, |watch| {
        watch.missed_between_activity() > 0 && watch.spent > bound * 2
    });
    assert!(
        bursting.missed_between_activity() > 0,
        "no look at the bursting tree found it charged nothing between two that found it \
         charged {}% of a core, over {} looks and {:?} — so the missed observation this tree \
         exists to put in front of the rule was never taken, and a verdict excused by one would \
         pass here unseen. The last of its {} activity events arrived at {:?}",
        oneagentgraph::scratch::WORKING_PERCENT_OF_A_CORE,
        bursting.looks.len(),
        bursting.spent,
        bursting.events(),
        bursting.recent_activity()
    );
    if let Some(at) = bursting.verdict() {
        condemned_only_past_the_watchdog(&bursting, at, "bursting");
    }
    // What the watchdog interval has to be wider than, on the host that ran it.
    // The margin between these two is the whole of why the bursting tree's
    // pauses are judged rather than excused, and it is the host's to narrow, so
    // it is printed rather than left to be re-derived from the constants.
    println!(
        "the bursting tree: {} activity events over {:?}, {} looks found nothing between two \
         that found work, longest gap {:?} against a {:?} watchdog interval",
        bursting.events(),
        bursting.spent,
        bursting.missed_between_activity(),
        bursting.longest_quiet(),
        watchdog()
    );
}

/// What a condemnation has to rest on: activity that stopped **arriving**.
///
/// The gap ending at the verdict, never the look at it — a look that found
/// nothing under a tree still working is a missed observation, which the
/// bursting tree produces on purpose and a loaded runner produces by accident.
/// Where the gap is past [`watchdog`] the rule is reading the same stop this
/// loop recorded, and there is nothing left to disagree about.
#[cfg(unix)]
fn condemned_only_past_the_watchdog(watch: &Watch, at: Duration, tree: &str) {
    assert!(
        watch.quiet_at(at) > watchdog(),
        "the activity rule condemned a member {at:?} into its life, {:?} after the last look \
         that found work under its {tree} tree — inside the {:?} activity is held to arriving \
         within, so the verdict rests on something other than work that stopped. {} activity \
         events over {:?}, the last of them at {:?}",
        watch.quiet_at(at),
        watchdog(),
        watch.events(),
        watch.spent,
        watch.recent_activity()
    );
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
    /// How much of the member's life this watch covers — a verdict, `enough` and
    /// the backstop all end one, and which of them did is not recorded.
    spent: Duration,
}

#[cfg(unix)]
impl Watch {
    /// How far into the member's life the rule first condemned it.
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

    fn recent_activity(&self) -> Vec<Duration> {
        let activity = self.activity();
        activity[activity.len().saturating_sub(REPORTED_ACTIVITY)..].to_vec()
    }

    /// How long the tree had gone without an activity event by `at`, which for a
    /// verdict is the gap the rule reached it at the end of.
    fn quiet_at(&self, at: Duration) -> Duration {
        let last = self
            .looks
            .iter()
            .filter(|look| look.working && look.at <= at)
            .map(|look| look.at)
            .next_back()
            .unwrap_or(Duration::ZERO);
        at.saturating_sub(last)
    }

    /// Looks that found no work with activity arriving on both sides of them: a
    /// work observation missed under a tree that never stopped.
    fn missed_between_activity(&self) -> usize {
        let (Some(first), Some(last)) = (
            self.looks.iter().position(|look| look.working),
            self.looks.iter().rposition(|look| look.working),
        ) else {
            return 0;
        };
        self.looks[first..last]
            .iter()
            .filter(|look| !look.working)
            .count()
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
        std::thread::sleep(look_every());
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
