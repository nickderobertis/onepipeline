//! Where a node's change is **now**, as the views report it.
//!
//! A run reported four of eleven nodes done while nine of the eleven had landed:
//! every settlement was an observation of the moment it published, and nothing
//! read it again. An adoption node was dispatched three times against work that
//! was already on its base. So the three places a landing is reported — the node
//! line, the run summary's count, and the status line — take a read when they
//! render, and these journeys hold what that read says and what it costs.
//!
//! Everything here is real. The run is driven through the binary, its nodes open
//! `onevcs` sessions and publish through them, and every landing on the base
//! afterwards is git this test performs on the repository it created: nothing is
//! published, merged, or released by anything outside the test, and no landing is
//! stated to the binary.
//!
//! # What a render cost when this was measured
//!
//! <!-- llmlint: ignore-block[comments_earn_their_place] this table is a deliverable
//! rather than an aside: the change was required to *state* what the read it adds costs a
//! supervisory look, and a figure nobody writes down is one nobody can weigh a later
//! regression against. It is deliberately not asserted — see the sentence under it — so
//! there is nowhere else it could live. The run prints the same three rows, so a reader
//! who doubts it re-reads it rather than trusting the snapshot. -->
//! Over the [`FIXTURE_NODES`]-node fixture below, on one developer host, each
//! figure averaged over [`REPETITIONS`] renders of that view — every render a
//! separate invocation of the binary, so the wall clock is the whole cost a
//! supervisor pays at a terminal rather than the render alone:
//!
//! | render | landing reads | repository resolutions | lines printed | wall clock |
//! | --- | --- | --- | --- | --- |
//! | `results` | 5 | 5, all of `service` | 8 | 636 ms |
//! | `goals` (the run summary) | 4 | 4, all of `service` | 4 | 612 ms |
//! | `status` | 4 | 4, all of `service` | 4 | 955 ms |
//!
//! The resolutions are one per read and not one per render — `vcs::landing_now`
//! says why.
//!
//! The reads, the resolutions and the lines are the same on every run; the clock is
//! not. Seven readings taken while this was written put each render between about
//! 0.12 s and 0.96 s, and the difference was what else the host was doing — which
//! is why **no assertion below is a threshold on that time**: what a render costs is
//! held as *work* — see
//! [`a_render_asks_the_landing_read_once_per_node_it_prints_and_does_nothing_else_per_node`]
//! — because the work a render performs is a fact about this code while the
//! seconds it takes are a fact about the host's load at that instant.
//! <!-- llmlint: ignore-end[comments_earn_their_place] -->

// llmlint: ignore-file[expensive_tests_stay_behind_their_own_edge] measured rather than
// assumed: this whole module runs in about 20 seconds and its slowest journey in about
// six, in a binary that already holds three deliberately minute-long ones in
// `loopcost.rs` — the journeys this rule's own suppression in that file is about. What it
// exercises is `views`, `vcs` and `rendercost`, which any change under `src/` can move,
// so a project edged narrower than the crate could not honestly run it: it would drop out
// of `nx affected` for the very changes it exists to catch.

// llmlint: ignore-file[e2e_not_mocked] the crate under test is driven as a real compiled
// binary and the sibling these journeys are about — `onevcs` — is the real library, over
// real git and a real origin on disk. `oneagentgraph` is substituted at its subprocess
// boundary so a journey states a dispatch outcome rather than paying for a model turn, and
// GitHub is substituted at `onevcs`'s own `ONEVCS_GH` override so a change request can be
// opened offline. `harness.rs` carries the same suppression and the full rationale.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use crate::harness::{git, lifecycle, plan_of, World};

/// How many nodes the fixture run holds.
///
/// **Six**, driven as one run against one repository, and then landed — or not —
/// by this test:
///
/// | node | what its settlement recorded | what the test then did to its branch | reported |
/// | --- | --- | --- | --- |
/// | `landed-at-settlement` | landing `landed`, from the run's own close-out re-read | landed under a landing trailer, before that re-read | **landed** — and never read again |
/// | `merged-since` | landing `unlanded` | landed under a landing trailer | **landed** |
/// | `failed-but-landed` | `failed`, no landing at all | landed under a landing trailer | **landed** |
/// | `still-open` | landing `unlanded` | nothing; its change is still open | **not landed** |
/// | `landed-in-part` | landing `unlanded` | landed, then given a commit the landing does not carry | **not landed**, naming the landing |
/// | `merged-quietly` | landing `unlanded` | landed with nothing recording why | **undecidable** |
///
/// So of the six: **three landed** — one the run itself recorded and two a read
/// taken now decides — **two not landed**, and **one whose landing cannot be
/// decided**. The remaining answer, a read this host cannot make at all, is a
/// journey of its own below, because reaching it means taking the repository
/// away from under the run.
const FIXTURE_NODES: usize = 6;

/// The nodes the fixture's plan holds.
const NODES: [&str; FIXTURE_NODES] = [
    "landed-at-settlement",
    "merged-since",
    "failed-but-landed",
    "still-open",
    "landed-in-part",
    "merged-quietly",
];

/// The one node the run's own record says landed, which no render may read again.
const RECORDED_LANDED: &str = "landed-at-settlement";

/// How many times each render is performed where its cost is measured.
///
/// Three, because the figure reported is a mean and one reading of a process
/// start is mostly the host's page cache. Every *bound* is asserted of a single
/// render, so repeating changes nothing about what is enforced.
const REPETITIONS: usize = 3;

/// The fixture run, the world it was driven in, and the branch each node left.
struct Fixture {
    world: World,
    run: String,
}

/// Drive the fixture run, then move the repository under it.
///
/// The run is real: six lifecycle nodes open `onevcs` sessions, work in them, and
/// publish under a `change-open` policy, so each settles with its change
/// unlanded and a person left to merge it. This test is that person, and it
/// merges with git.
///
/// The order is load-bearing. `landed-at-settlement` is landed **before** the run
/// is adopted, so the driver's own close-out re-read is what records it as
/// landed — which is the state a render must not spend a read on, and the only
/// way to reach it is to let a driver find it. Everything else is landed after
/// that, so nothing but a render has ever looked at it.
fn fixture(name: &str) -> Fixture {
    let world = World::new(name);
    let repository = world.repository("change-open", &[]);
    let checkout = repository.checkout.clone();
    for node in NODES {
        world.script(&format!("{node}.work"), &format!("the work of {node}\n"));
    }
    // A node the run settles `failed`, with the work its dispatch committed still
    // on the branch it handed back. Nothing about a failure says where that
    // branch is now, which is why a view asks about one.
    world.script("failed-but-landed.fail", "1");

    let run = name.to_owned();
    let plan = world.plan(
        &run,
        &serde_json::json!({
            "schema_version": onepipeline::plan::PLAN_SCHEMA_VERSION,
            "name": run,
            "goal": {"text": format!("Deliver {run}")},
            // One session at a time. Six lifecycle nodes over one repository
            // otherwise have `onevcs` sweeping a spent run root while a live
            // session is still working in one — and a node whose worktree is
            // taken out from under it settles `publication-failed`, which is a
            // fixture that did not happen rather than a landing anybody can read.
            "concurrency": 1,
            "tasks": NODES.iter().map(|node| lifecycle(node, &[])).collect::<Vec<Value>>(),
        }),
    );
    world.run(&["start", &plan, "--attach"]).settled();
    world.until("the run to settle", |world| {
        world.run_file(&run, "result.json").is_file()
    });
    let branches = branches_of(&world, &run);
    // Brought here, because a branch is wherever the session that cut it left it:
    // a publication pushes it to this checkout, and a dispatch that failed before
    // publishing leaves it in the run clone `onevcs` opened. This test lands work
    // with git, so it needs the commits here whichever of those happened — and a
    // journey that raced the push would fail as a flake rather than say anything
    // about a landing.
    for branch in branches.values() {
        bring_the_branch_here(&world, &checkout, branch);
    }
    // Then kept under a ref of this test's own, so a sweep that reaps a spent
    // session's branch afterwards cannot take the work away mid-journey.
    for (node, branch) in &branches {
        git(&world, &checkout, &["branch", "-f", &kept(node), branch]);
    }

    // Landed before the run is adopted, which is what makes the driver's own
    // close-out re-read the thing that records it.
    land(&world, &checkout, RECORDED_LANDED, &branches, None);
    git(&world, &checkout, &["push", "origin", "main"]);
    world.run(&["adopt", &run]).settled();

    for node in ["merged-since", "failed-but-landed", "landed-in-part"] {
        land(&world, &checkout, node, &branches, None);
    }
    // Landed with nothing recording why, which is equally what somebody else
    // making the same change leaves behind — so it is undecidable rather than a
    // landing.
    land(
        &world,
        &checkout,
        "merged-quietly",
        &branches,
        Some("chore: somebody else made this change\n"),
    );
    git(&world, &checkout, &["push", "origin", "main"]);

    // A branch that landed and then went on, which is the ordinary shape a
    // retried dispatch leaves: the landing is real and there is still work to
    // publish, so it is not a landing *of this branch*. The commit goes on this
    // checkout's own copy, which is one of the copies `onevcs` asks — and the
    // copy still holding work is the one whose answer is true of the work.
    let branch = &branches["landed-in-part"];
    git(
        &world,
        &checkout,
        &["branch", "-f", branch, &kept("landed-in-part")],
    );
    git(&world, &checkout, &["checkout", branch]);

    std::fs::write(checkout.join("after-the-landing.txt"), "more work\n")
        .expect("the commit above the landing is written");
    git(&world, &checkout, &["add", "-A"]);
    git(
        &world,
        &checkout,
        &["commit", "-m", "feat: after the landing"],
    );
    git(&world, &checkout, &["checkout", "main"]);

    Fixture { world, run }
}

/// The branch one node's dispatch left behind.
fn branches_of_one(world: &World, run: &str, node: &str) -> String {
    let result = world.run_json(run, "result.json");
    result["nodes"]
        .as_array()
        .and_then(|nodes| nodes.iter().find(|entry| entry["id"] == node))
        .and_then(|entry| entry["branch"].as_str())
        .unwrap_or_else(|| panic!("{node} settled without naming its branch: {result}"))
        .to_owned()
}

/// The branch each node's dispatch left behind, off the run's own result.
fn branches_of(world: &World, run: &str) -> BTreeMap<String, String> {
    let result = world.run_json(run, "result.json");
    let nodes = result["nodes"]
        .as_array()
        .unwrap_or_else(|| panic!("the run's result names its nodes: {result}"));
    let branches: BTreeMap<String, String> = nodes
        .iter()
        .filter_map(|node| {
            Some((
                node["id"].as_str()?.to_owned(),
                node["branch"].as_str()?.to_owned(),
            ))
        })
        .collect();
    for node in NODES {
        assert!(
            branches.contains_key(node),
            "{node} settled without naming the branch its work is on: {result}"
        );
    }
    // The fixture the journeys are written against, asserted rather than
    // assumed: a node that settled some other way is a run that did not happen,
    // and every assertion after it would be about the wrong thing.
    for node in nodes {
        let id = node["id"].as_str().unwrap_or_default();
        let (status, landing) = match id {
            "failed-but-landed" => ("failed", Value::Null),
            _ => ("done", Value::String("unlanded".into())),
        };
        assert_eq!(
            node["status"], status,
            "{id} settled unexpectedly: {result}"
        );
        assert_eq!(
            node["landing"], landing,
            "{id} settled unexpectedly: {result}"
        );
    }
    branches
}

/// This test's own name for the commit one node's branch stood at when the run
/// settled, so a later sweep cannot take the work away mid-journey.
fn kept(node: &str) -> String {
    format!("kept/{node}")
}

/// Give this checkout the commits one branch carries, wherever they are.
///
/// A publication pushes the branch here, so most of the time it is already a ref
/// of this checkout. A dispatch that failed before publishing pushed nothing, and
/// a publication still on its way has not pushed yet — and in both cases the work
/// is in the run clone `onevcs` cut for the session, under this world's own state
/// root. Searched rather than derived, because the path a session's clone sits at
/// is that library's to decide and this test has no business restating it.
fn bring_the_branch_here(world: &World, checkout: &Path, branch: &str) {
    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    // Waited on rather than asked once: a session's publishing push arrives on
    // its own clock, and `onevcs` moves a spent clone out from under the search
    // while it reaps it — so a single miss is as likely to be a copy in motion as
    // a copy that is gone.
    world.until(
        &format!("a copy of {branch} to reach this world"),
        |world| {
            !git(world, checkout, &["branch", "--list", branch])
            .trim()
            .is_empty()
            // The origin, where a publishing push put it.
            || fetched(world, checkout, "origin", &refspec)
            // Otherwise the run clone the session worked in, which is where a
            // dispatch that failed before publishing left its work. Searched
            // rather than derived, because the path a session's clone sits at is
            // that library's to decide and this test has no business restating it.
            || clone_holding(&world.onevcs_home(), branch)
                .is_some_and(|holder| fetched(world, checkout, &holder.to_string_lossy(), &refspec))
        },
    );
}

/// Fetch one refspec, answering whether the remote had it.
///
/// Not through [`git`](fn@git), which asserts: "the branch is not there" is one
/// of the answers this asks for.
fn fetched(world: &World, checkout: &Path, remote: &str, refspec: &str) -> bool {
    std::process::Command::new("git")
        .args(["fetch", "--force", remote, refspec])
        .current_dir(checkout)
        .env("GIT_CONFIG_GLOBAL", world.gitconfig())
        .output()
        .is_ok_and(|fetched| fetched.status.success())
}

/// The first repository under `dir` that holds `branch` as a ref of its own.
fn clone_holding(dir: &Path, branch: &str) -> Option<std::path::PathBuf> {
    if dir.join(".git").exists() {
        let held = std::process::Command::new("git")
            .args(["rev-parse", "--verify", &format!("refs/heads/{branch}")])
            .current_dir(dir)
            .output();
        if held.is_ok_and(|held| held.status.success()) {
            return Some(dir.to_path_buf());
        }
    }
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .find_map(|entry| clone_holding(&entry.path(), branch))
}

/// Take one branch onto the base, the way a landing does.
///
/// The branch is a ref of this checkout, because that is where a session's
/// publishing push put it: a session clone takes its remote from the execution
/// checkout, so what a journey merges is the branch that push left here.
///
/// `message` is the whole commit message where a journey states one. The default
/// carries `Onevcs-Landed-Commit:` naming the branch commit the landing lands,
/// which is the trailer `onevcs` reads for a landing that opened no change
/// request — and the record a landing this host performed leaves behind.
fn land(
    world: &World,
    checkout: &Path,
    node: &str,
    branches: &BTreeMap<String, String>,
    message: Option<&str>,
) {
    let branch = &branches[node];
    // Back to where the run left it, in case a sweep has taken the name away
    // since; the commit is still here because [`kept`] is holding it.
    git(world, checkout, &["branch", "-f", branch, &kept(node)]);
    let tip = git(world, checkout, &["rev-parse", branch])
        .trim()
        .to_owned();
    let message = message
        .map(str::to_owned)
        .unwrap_or_else(|| format!("chore: land {branch}\n\nOnevcs-Landed-Commit: {tip}\n"));
    git(
        world,
        checkout,
        &["merge", "--no-ff", "-m", &message, branch],
    );
}

/// Every act one render recorded, as JSON lines.
fn acts(world: &World, path: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|error| {
        panic!(
            "the render recorded nothing at {}: {error}\n{}",
            path.display(),
            world.dump()
        )
    });
    text.lines()
        .map(|line| serde_json::from_str(line).expect("a JSON act"))
        .collect()
}

/// The nodes one render asked the landing read about, in the order it asked.
fn asked(acts: &[Value]) -> Vec<String> {
    acts.iter()
        .filter(|act| act["act"] == "landing-read")
        .map(|act| {
            act["node"]
                .as_str()
                .unwrap_or_else(|| panic!("a landing read outside any node's decision: {act}"))
                .to_owned()
        })
        .collect()
}

/// The `results` line for one node.
fn line_for(rendered: &str, node: &str) -> String {
    rendered
        .lines()
        .find(|line| line.trim_start().starts_with(node))
        .unwrap_or_else(|| panic!("{node} has no line in:\n{rendered}"))
        .to_owned()
}

/// Every answer a landing read gives reads back on the node line, as of the
/// moment the line is rendered.
///
/// The whole incident in one journey. Four of these nodes settled with their
/// change unlanded and nothing told the run otherwise; two of them are on the
/// base now, and a third is a node the run settled *failed* whose work somebody
/// landed anyway. Before this, every one of those lines carried the settlement's
/// dated claim, and a supervisor re-dispatched work that was already landed.
///
/// The two answers that are not landings are held just as hard, because what the
/// dated phrasing was protecting is that a read which cannot decide must not be
/// reported as a decision.
#[test]
fn every_answer_a_landing_read_gives_reads_back_on_the_node_line() {
    let Fixture { world, run } = fixture("landing-answers");
    let reads = world.root.join("results.reads");
    let results = world.run_recording_renders(&reads, &["results", &run]);
    results.exited(0);

    let trailer = "landed on its base — read now: a landing trailer on the base at ";
    let merged = line_for(&results.stdout, "merged-since");
    assert!(
        merged.contains(trailer) && !merged.contains("NOT landed"),
        "a change on its base is reported as one nobody landed: {merged}"
    );
    // A node that settled *failed*, with no landing recorded at all, whose work
    // the base has since taken. Nothing used to say anything about it.
    let failed = line_for(&results.stdout, "failed-but-landed");
    assert!(
        failed.contains(trailer),
        "a failed node whose work landed says nothing about it: {failed}"
    );
    // A change nobody landed still reads as one — and says so from a read rather
    // than from the settlement's own dated claim.
    let open = line_for(&results.stdout, "still-open");
    assert!(
        open.contains("NOT landed: read now, content comparison: the base does not carry"),
        "a change nobody landed is not reported as one: {open}"
    );
    // A landing the branch has gone past: there is work left to publish, so it is
    // not a landing of this branch — and the commit that *did* land is named
    // anyway, because a reader deciding what to do next needs both.
    let part = line_for(&results.stdout, "landed-in-part");
    assert!(
        part.contains("NOT landed: read now, a landing trailer on the base at ")
            && part.contains("commit(s) above it the landing did not carry"),
        "a branch that landed and went on is reported as though nothing landed: {part}"
    );
    // Undecidable, and reported as neither of the two answers nothing gave.
    let quiet = line_for(&results.stdout, "merged-quietly");
    assert!(
        quiet.contains(
            "landing UNDECIDED: read now, content comparison: the base already carries what \
             this branch changed, and nothing records why"
        ),
        "an undecidable landing is reported as a decision: {quiet}"
    );
    assert!(
        !quiet.contains("NOT landed") && !quiet.contains("landed on its base"),
        "an undecidable landing reads as one of the two answers nothing gave: {quiet}"
    );

    // And the landing the run itself recorded is reported from that record and
    // never asked about again: a base does not stop carrying work it has taken,
    // so a read there could only agree, at a cost.
    let recorded = line_for(&results.stdout, RECORDED_LANDED);
    assert!(
        recorded.contains("landed on its base — the run observed the change reach it"),
        "a landing the run observed is reported as something else: {recorded}"
    );
    assert!(
        !asked(&acts(&world, &reads)).contains(&RECORDED_LANDED.to_owned()),
        "a landing no later read can overturn was read again:\n{}",
        std::fs::read_to_string(&reads).unwrap_or_default()
    );
}

/// The views work is closed from count what a read taken now says, and count an
/// undecided landing apart from work nobody landed.
///
/// `status` otherwise reports only what is in flight, and the summary line is the
/// `n/n done` a planner reads to decide a run is finished — so these are the two
/// places the stale count actually cost dispatches.
#[test]
fn the_counting_views_report_what_a_read_taken_now_says() {
    let Fixture { world, run } = fixture("landing-counts");

    // `goals` and `runs` are the summary line; `status` carries it too, above its
    // own per-node lines.
    for view in [
        world.run(&["goals", &run]),
        world.run(&["runs"]),
        world.run(&["status", &run]),
    ] {
        view.exited(0);
        assert!(
            view.stdout.contains("2 not landed, 1 landing undecided"),
            "the count carries the settlement's answer rather than a read:\n{}",
            view.stdout
        );
    }
    let status = world.run(&["status", &run]);
    status.exited(0);
    status.out_has("2 node(s) have not landed: landed-in-part, still-open");
    status.out_has("1 node(s) whose landing this host could not decide: merged-quietly");
    // The three that landed are on neither line.
    for landed in [RECORDED_LANDED, "merged-since", "failed-but-landed"] {
        assert!(
            !status.stdout.contains(landed),
            "{landed} is on its base and is still reported as outstanding:\n{}",
            status.stdout
        );
    }
}

/// A landing this host cannot read at all is reported as undecided, saying what
/// refused.
///
/// The answer that is neither of the other two and must never be collapsed into
/// them. Here the repository the run's work is in is gone from this host — the
/// state a checkout somebody deleted leaves, with the registry still naming it —
/// so `onevcs` refuses the question rather than answering it. A supervisor
/// meeting "this host could not decide it" has something to fix; one meeting
/// `NOT landed` would go and publish landed work again.
#[test]
fn a_landing_this_host_cannot_read_is_reported_as_undecided_saying_what_refused() {
    let world = World::new("landing-unreadable");
    let repository = world.repository("change-open", &[]);
    world.script("service.work", "the work nobody can find again\n");
    let run = "unreadable".to_owned();
    let plan = world.plan(&run, &plan_of(&run, vec![lifecycle("service", &[])]));
    world.run(&["start", &plan, "--attach"]).settled();
    world.until("the run to settle", |world| {
        world.run_file(&run, "result.json").is_file()
    });
    // Answerable while the repository is there.
    world
        .run(&["results", &run])
        .exited(0)
        .out_has("NOT landed: read now");

    // And then the checkout every publication fast-forwards is gone, which is
    // what the registry still names.
    std::fs::remove_dir_all(&repository.checkout).expect("the checkout is taken away");

    let results = world.run(&["results", &run]);
    results.exited(0).out_has("landing UNDECIDED: read now");
    results.out_has("this host could not decide it:");
    results.out_lacks("NOT landed");
    results.out_lacks("landed on its base");
    // The count says the same thing, apart from work nobody landed.
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("1 node(s) whose landing this host could not decide: service")
        .out_lacks("node(s) have not landed");
}

/// A branch name two repositories both hold is answered about the one this run's
/// work is in, rather than refused as an ambiguity nobody has.
///
/// The reason the read is narrowed to the node's own repository at all. Branch
/// names are minted per session and collide across identities the moment two
/// runs of one host name work the same way — and `onevcs` will not guess: asked
/// about a name two of its identities hold, it refuses. A view that inherited
/// that refusal would report every such node as undecided, which is the same
/// silence the settlement's dated claim used to be.
///
/// The **driver's own close-out** read is narrowed the same way, and it is the
/// one asserted on here: `landing` in the run's result is what a consumer parses,
/// and it is written by the driver rather than by a render.
#[test]
fn a_branch_name_two_repositories_hold_is_answered_about_the_node_s_own() {
    let world = World::new("landing-ambiguous");
    let service = world.repository("change-open", &[]);
    let other = world.extra_repository("engine");
    world.script("service.work", "the work of the node under test\n");

    let run = "ambiguous".to_owned();
    let plan = world.plan(&run, &plan_of(&run, vec![lifecycle("service", &[])]));
    world.run(&["start", &plan, "--attach"]).settled();
    world.until("the run to settle", |world| {
        world.run_file(&run, "result.json").is_file()
    });
    let branch = branches_of_one(&world, &run, "service");

    // The other repository is given a branch of the same name, carrying work of
    // its own. Nothing about it says anything about this run's node, and asking
    // without naming a repository can no longer answer at all.
    git(
        &world,
        &other.checkout,
        &["checkout", "-b", &branch, "main"],
    );
    std::fs::write(other.checkout.join("elsewhere.txt"), "another repository\n")
        .expect("the other repository's work is written");
    git(&world, &other.checkout, &["add", "-A"]);
    git(
        &world,
        &other.checkout,
        &["commit", "-m", "feat: elsewhere"],
    );
    git(&world, &other.checkout, &["checkout", "main"]);

    // And this run's own branch lands, under the trailer a landing leaves.
    world.until(
        "the branch the run published to reach the checkout",
        |world| {
            !git(world, &service.checkout, &["branch", "--list", &branch])
                .trim()
                .is_empty()
        },
    );
    let tip = git(&world, &service.checkout, &["rev-parse", &branch])
        .trim()
        .to_owned();
    git(
        &world,
        &service.checkout,
        &[
            "merge",
            "--no-ff",
            "-m",
            &format!("chore: land it\n\nOnevcs-Landed-Commit: {tip}\n"),
            &branch,
        ],
    );
    git(&world, &service.checkout, &["push", "origin", "main"]);

    // The view answers about this node's repository rather than refusing.
    world
        .run(&["results", &run])
        .exited(0)
        .out_has("landed on its base — read now: a landing trailer on the base at ")
        .out_lacks("could not decide it");

    // And so does the driver's own close-out, which is what a consumer parses.
    world.run(&["adopt", &run]).settled();
    let settled = world.run_json(&run, "result.json");
    assert_eq!(
        settled["nodes"][0]["landing"], "landed",
        "the run's close-out was refused an answer a repository name resolves: {settled}"
    );
}

// llmlint: ignore-block[tests_mirror_real_usage] the claim under test is *what a render
// did*, and no rendering says that on stdout: a supervisor cannot see a landing read from
// the terminal, which is exactly why it needed measuring. So the render records its own
// acts when a caller asks it to — the same seam and the same reason as
// `ONEPIPELINE_LOOP_STATS` and `loopcost.rs` beside it, which count a driver's own work
// over real run stores. Everything here is still the compiled binary over the fixture
// above: the commands are the ones a user types, their exit codes are asserted, and the
// record is read after they exit rather than substituted for anything they do.
/// What a render costs is bounded as **work**, over the fixture above.
///
/// Every claim here counts acts and times nothing, so it gives the same verdict
/// on a loaded host as on an idle one — and a later change that puts the
/// per-node cost back fails here rather than reaching a supervisor.
///
/// Five bounds, one per way a render could cost more than it must:
///
/// 1. at most one landing read for each node the render reports on;
/// 2. no read at all for a node the run already recorded as landed;
/// 3. no read for a node the render does not report on;
/// 4. at most one **repository resolution** per node, for a repository of a node
///    it prints, and exactly one per read — so a second resolution of a repository
///    a node has already been decided against fails here;
/// 5. **nothing else per node** — no process this crate started, no read of the
///    run's ledger or journal, and so no walk of a base's history taken here and
///    no request over a network, both of which from this crate are a process.
///
/// Bound 4 is one resolution per *node*, not one per render, because that is what
/// the dependency's read costs — `vcs::landing_now` says why, and divergence 33
/// records what would close it. So they are **counted and reported** here rather
/// than claimed away.
#[test]
fn a_render_asks_the_landing_read_once_per_node_it_prints_and_does_nothing_else_per_node() {
    let Fixture { world, run } = fixture("landing-cost");

    // Measured beside the bound and asserted on by nothing: what a render costs
    // in seconds is a fact about this host's load at that instant, and a
    // threshold on it would fail correct work. It is printed so the figure in
    // this module's own header is a reading rather than a guess.
    let mut measured_cost = String::new();
    // One invocation before anything is timed, so what is measured is a render
    // rather than the first exec of a debug binary nobody has paged in.
    world.run(&["runs"]).exited(0);
    for (view, argv, expected) in [
        // `results` prints a line for every node; the counting views report on
        // the five whose publication answered a landing at all.
        ("results", vec!["results", run.as_str()], 5),
        ("summary", vec!["goals", run.as_str()], 4),
        ("status", vec!["status", run.as_str()], 4),
    ] {
        // Each repetition records into a file of its own, so the acts asserted
        // on below are one render's rather than three renders' appended.
        let mut took = std::time::Duration::ZERO;
        let mut measured = None;
        for repetition in 0..REPETITIONS {
            let path = world.root.join(format!("{view}-{repetition}.reads"));
            let began = std::time::Instant::now();
            let rendered = world.run_recording_renders(&path, &argv);
            took += began.elapsed();
            rendered.exited(0);
            measured = Some((rendered, path));
        }
        let (measured, path) = measured.expect("at least one render is measured");
        let acts = acts(&world, &path);
        let rendered: Vec<&Value> = acts.iter().filter(|act| act["view"] == view).collect();
        assert!(
            !rendered.is_empty(),
            "{view} recorded no render at all:\n{}",
            measured.stdout
        );

        let asked: Vec<String> = asked(
            &rendered
                .iter()
                .map(|act| (*act).clone())
                .collect::<Vec<Value>>(),
        );
        let mut once = asked.clone();
        once.sort();
        once.dedup();
        assert_eq!(
            once.len(),
            asked.len(),
            "{view} asked the landing read twice for one node: {asked:?}"
        );
        assert_eq!(
            asked.len(),
            expected,
            "{view} performed {} landing read(s), not {expected}: {asked:?}",
            asked.len()
        );
        assert!(
            !asked.contains(&RECORDED_LANDED.to_owned()),
            "{view} read a landing the run had already recorded: {asked:?}"
        );

        let reported: Vec<String> = rendered
            .iter()
            .filter(|act| act["act"] == "reported")
            .filter_map(|act| act["node"].as_str().map(str::to_owned))
            .collect();
        for node in &asked {
            assert!(
                reported.contains(node),
                "{view} read a landing for {node}, whose line it does not print: {reported:?}"
            );
        }
        assert!(
            asked.len() <= reported.len(),
            "{view} performed more reads than it has nodes to report on: {asked:?} over \
             {reported:?}"
        );

        // The repositories the render made the sibling resolve to decide those
        // landings, by the node each was resolved for. The read resolves one
        // itself — see `vcs::landing_now` — so these are counted rather than
        // absent, and held to one per node: a second resolution of a repository a
        // node has already been decided against is work nobody is shown.
        let opened: Vec<(String, String)> = rendered
            .iter()
            .filter(|act| act["act"] == "repository-resolved")
            .map(|act| {
                (
                    act["repo"]
                        .as_str()
                        .unwrap_or_else(|| panic!("a resolution naming no repository: {act}"))
                        .to_owned(),
                    act["node"]
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("a resolution outside any node's decision: {act}")
                        })
                        .to_owned(),
                )
            })
            .collect();
        let mut opened_once = opened.clone();
        opened_once.sort();
        opened_once.dedup();
        assert_eq!(
            opened_once.len(),
            opened.len(),
            "{view} resolved one repository twice over for a node it had already decided: \
             {opened:?}"
        );
        for (repo, node) in &opened {
            assert!(
                reported.contains(node),
                "{view} resolved {repo} for {node}, whose line it does not print: {reported:?}"
            );
        }
        // One resolution per read and no other: one without a read would be this
        // crate resolving a repository of its own, which it does not do.
        assert_eq!(
            opened.len(),
            asked.len(),
            "{view} resolved {} repositor(ies) for {} landing read(s): {opened:?}",
            opened.len(),
            asked.len()
        );

        // And nothing else per node: every act recorded inside a node's landing
        // decision is that node's one landing read and the one resolution it makes.
        let per_node: Vec<&&Value> = rendered
            .iter()
            .filter(|act| act["act"] != "render" && act["act"] != "reported")
            .collect();
        for act in &per_node {
            assert!(
                act["act"] == "landing-read" || act["act"] == "repository-resolved",
                "{view} did per-node work no landing read accounts for: {act}"
            );
        }
        assert_eq!(
            per_node.len(),
            asked.len() + opened.len(),
            "{view} performed per-node work beside its landing reads: {per_node:?}"
        );

        // What each repository cost this render, which is the figure the bound
        // above cannot lower and the report below has to carry honestly.
        let mut per_repository: BTreeMap<&str, usize> = BTreeMap::new();
        for (repo, _) in &opened {
            *per_repository.entry(repo.as_str()).or_default() += 1;
        }
        measured_cost.push_str(&format!(
            "  {view:<8} {} landing read(s), {} repository resolution(s) {:?}, {} line(s) \
             printed, {:?} per render\n",
            asked.len(),
            opened.len(),
            per_repository,
            measured.stdout.lines().count(),
            took / u32::try_from(REPETITIONS).expect("a small count")
        ));
    }
    println!(
        "what one render of each view cost, averaged over {REPETITIONS} renders \
         each:\n{measured_cost}"
    );
}
// llmlint: ignore-end[tests_mirror_real_usage]
