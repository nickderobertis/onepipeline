//! A settling node's criteria, read against the branch it settled on.
//!
//! Where a criterion names a literal value in a named file, the file is on the
//! node's own branch and the comparison costs one read. These journeys drive
//! that against a **real** branch: a real `onevcs` session over a real git
//! origin on disk, with a real dispatch writing a real file into the worktree,
//! and the answer read back off the run's own record and the manager's own
//! queue — never off an internal.
//!
//! What is proven here is as much what the check *declines* to say as what it
//! says. A criterion it cannot parse is silence; a file it cannot read is a
//! third answer of its own; and a mismatch never changes what the node settled
//! on. See `docs/contract-divergences.md` entry 47.

// llmlint: ignore-file[e2e_not_mocked] the same substitution every journey in this suite
// makes and no other: `oneagentgraph` stands in at its subprocess boundary so a dispatch
// states what it wrote instead of paying for a model turn, and GitHub stands in at
// `onevcs`'s own `ONEVCS_GH` override. The crate under test is the compiled binary, and
// the branch these journeys read is a real git branch in a real session worktree — which
// is the whole point of them. `harness.rs` carries the full rationale.

use crate::harness::{git, plan_of, Repository, World};
use serde_json::{json, Value};

/// The file the scripted dispatch writes into the node's worktree.
///
/// The double names a dispatch's work after the node it ran for, so a criterion
/// here names this and the journey's `work` script decides what it holds.
const WORK: &str = "service.md";

/// One lifecycle node whose bar is the criteria given, stated as a plan states
/// them.
fn node_bounded_by(criteria: &[&str]) -> Value {
    let bar = criteria
        .iter()
        .map(|criterion| format!("- {criterion}"))
        .collect::<Vec<_>>()
        .join("\n");
    json!({
        "id": "service",
        "repo": "service",
        "persona": "engineer",
        "title": "feat: ship the row",
        "task": format!(
            "## What\nWrite the row.\n\n## Why\nA consumer reads it.\n\n\
             ## Acceptance criteria\n\n{bar}\n"
        ),
    })
}

/// Drive one plan to settlement and hand back the run's name.
fn settle(world: &World, name: &str, nodes: Vec<Value>) -> String {
    let path = world.plan(name, &plan_of(name, nodes));
    world.run(&["start", &path, "--attach"]).settled();
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });
    name.to_string()
}

/// A world with a repository that publishes with git alone, and a dispatch that
/// writes `body` into [`WORK`] on the node's branch.
fn writing(name: &str, body: &str) -> World {
    let world = World::new(name);
    world.repository("local-direct", &[]);
    world.script("service.work", body);
    world
}

/// Every comparison one run recorded, as the run's own record carries it.
fn comparisons(world: &World, run: &str) -> Vec<Value> {
    world
        .events_of(run, "criterion-checked")
        .into_iter()
        .map(|event| event["payload"].clone())
        .collect()
}

/// Every finding one run raised to its manager.
fn findings(world: &World, run: &str) -> Vec<String> {
    world
        .events_of(run, "planner-surface-queued")
        .into_iter()
        .filter(|event| event["payload"]["kind"] == "finding")
        .filter_map(|event| {
            event["payload"]["message"]
                .as_str()
                .map(std::string::ToString::to_string)
        })
        .collect()
}

/// How a node settled, in the three fields that say it.
///
/// Not the whole record: two runs cut two branches and name two sessions, and a
/// comparison including those would fail on the one difference that has nothing
/// to do with what is being compared.
fn settled_as(world: &World, run: &str) -> Value {
    let node = world.run_json(run, "result.json")["nodes"][0].clone();
    json!({
        "status": node["status"],
        "outcome": node["outcome"],
        "detail": node["detail"],
    })
}

#[test]
fn a_criterion_the_branch_contradicts_is_a_finding_naming_the_file_and_both_values() {
    let world = writing("criteria-mismatch", "complete_dataset: false\n");
    let run = settle(
        &world,
        "contradicted",
        vec![node_bounded_by(&[
            "the shared journey row in `service.md` is `complete_dataset: true`",
        ])],
    );

    // The comparison is on the run's own record, as a mismatch, carrying both
    // values: the one the bar named and the one the branch holds.
    let compared = comparisons(&world, &run);
    assert_eq!(compared.len(), 1, "{compared:?}");
    assert_eq!(compared[0]["answer"], "mismatch", "{compared:?}");
    assert_eq!(compared[0]["file"], WORK);
    assert_eq!(compared[0]["expected"], "complete_dataset: true");
    assert_eq!(compared[0]["holds"], "`complete_dataset: false`");

    // And the manager is handed a finding naming all four, because a manager
    // who has to open the branch to learn what the run already read is being
    // told nothing.
    let raised = findings(&world, &run);
    assert_eq!(raised.len(), 1, "{raised:?}");
    for named in [
        "the shared journey row in `service.md` is `complete_dataset: true`",
        WORK,
        "complete_dataset: true",
        "complete_dataset: false",
    ] {
        assert!(
            raised[0].contains(named),
            "the finding does not name {named}:\n{}",
            raised[0]
        );
    }
}

#[test]
fn a_mismatch_quotes_what_the_file_holds_and_says_so_when_it_holds_nothing() {
    let world = World::new("criteria-evidence");
    let repository = world.repository("local-direct", &[]);
    // One line far longer than a finding can carry, and one file with no line
    // naming the key at all: the two shapes of "what it holds instead" that are
    // not simply the other value.
    committed(
        &world,
        &repository,
        "long.md",
        format!("state: {}\n", "x".repeat(600)).as_bytes(),
    );
    committed(&world, &repository, "silent.md", b"nothing to say here\n");
    world.script("service.work", "complete_dataset: false\n");
    let run = settle(
        &world,
        "evidence",
        vec![node_bounded_by(&[
            "the row in `long.md` is `state: done`",
            "the row in `silent.md` is `owner: nobody`",
        ])],
    );

    let holds = |named: &str| {
        comparisons(&world, &run)
            .into_iter()
            .find(|payload| payload["file"] == named)
            .unwrap_or_else(|| panic!("`{named}` was never read"))["holds"]
            .as_str()
            .unwrap_or_else(|| panic!("`{named}` mismatched without saying what it holds"))
            .to_string()
    };

    // The line the file does hold, cut short: a finding is read by a person and
    // a six-hundred-character line is not a sentence.
    let long = holds("long.md");
    assert!(long.starts_with("`state: xxx"), "{long}");
    assert!(
        long.contains('…'),
        "the long line was not cut short: {long}"
    );
    assert!(long.chars().count() < 260, "{long}");

    // And where nothing in the file names the key, the absence is the answer
    // rather than a blank where the evidence should be.
    assert_eq!(holds("silent.md"), "nothing naming `owner`");

    // Both reached the manager as findings, each quoting its own evidence.
    let raised = findings(&world, &run);
    assert_eq!(raised.len(), 2, "{raised:?}");
    assert!(
        raised
            .iter()
            .any(|message| message.contains("nothing naming `owner`")),
        "{raised:?}"
    );
}

#[test]
fn a_criterion_the_branch_holds_raises_nothing() {
    let world = writing("criteria-match", "complete_dataset: true\n");
    let run = settle(
        &world,
        "held",
        vec![node_bounded_by(&[
            "the shared journey row in `service.md` is `complete_dataset: true`",
        ])],
    );

    // Read and answered — the comparison is on the record, so this is a check
    // that ran rather than one that was skipped …
    let compared = comparisons(&world, &run);
    assert_eq!(compared.len(), 1, "{compared:?}");
    assert_eq!(compared[0]["answer"], "match", "{compared:?}");
    // … and nothing was raised for it. A tier that reported every comparison it
    // made is one a manager learns to skim.
    assert_eq!(findings(&world, &run), Vec::<String>::new());
}

#[test]
fn a_contradicted_criterion_settles_the_node_exactly_as_an_uncontradicted_one_does() {
    // Two runs over the same repository shape, the same dispatch and the same
    // file on the branch. The one difference is the sentence the bar is written
    // in: one names the row and the value, and the other says the same thing in
    // prose this check cannot parse.
    let contradicted = writing("criteria-unchanged-a", "complete_dataset: false\n");
    let control = writing("criteria-unchanged-b", "complete_dataset: false\n");
    let checked = settle(
        &contradicted,
        "contradicted",
        vec![node_bounded_by(&[
            "the shared journey row in `service.md` is `complete_dataset: true`",
        ])],
    );
    let plain = settle(
        &control,
        "plain",
        vec![node_bounded_by(&["the shared journey row is complete"])],
    );

    // The node settled on its own work, both times, identically: the check is
    // beside the settlement and never part of it.
    assert_eq!(
        settled_as(&contradicted, &checked),
        settled_as(&control, &plain)
    );
    assert_eq!(
        contradicted.run_json(&checked, "result.json")["state"],
        control.run_json(&plain, "result.json")["state"]
    );
    assert_eq!(
        settled_as(&contradicted, &checked)["status"],
        "done",
        "the journey did not reach the settlement it is comparing"
    );

    // And the finding is beside it: raised on the run whose branch contradicted
    // its bar, and on no other.
    assert_eq!(findings(&contradicted, &checked).len(), 1);
    assert_eq!(findings(&control, &plain), Vec::<String>::new());
    assert_eq!(comparisons(&control, &plain), Vec::<Value>::new());
}

#[test]
fn criteria_this_check_cannot_parse_are_reported_nowhere() {
    let world = writing("criteria-prose", "complete_dataset: false\n");
    let run = settle(
        &world,
        "prose",
        vec![node_bounded_by(&[
            // Ordinary prose, naming neither half.
            "the row reads the way the consumer expects",
            // A file, and no literal to hold it to.
            "`service.md` is tidier than it was",
            // A literal, and no file to look in.
            "the row is `complete_dataset: true`",
            // Both halves, in a sentence whose meaning is the absence — which
            // this reading would answer backwards.
            "`service.md` no longer holds `complete_dataset: false`",
            // Three spans: which two are the pair is a guess.
            "`service.md` and `README.md` both hold `complete_dataset: true`",
            // Two paths, and two literals: neither says which is the value.
            "`service.md` matches `README.md`",
            "`complete_dataset: true` is not `complete_dataset: false`",
            // A span nobody closed. Two spans by the split, one quotation by
            // the writer.
            "`service.md` holds `complete_dataset: true",
            // Paths that are not files on this branch, each spelled the way a
            // host of its own spells leaving one.
            "`/etc/passwd` holds `root: yes`",
            "`../elsewhere/service.md` holds `complete_dataset: true`",
            "`C:\\elsewhere\\service.md` holds `complete_dataset: true`",
            "`some file.md` holds `complete_dataset: true`",
        ])],
    );

    // Nothing compared, nothing raised, and no warning that something was
    // skipped: a criterion this check cannot parse is silence, because a
    // checker that guessed would report false findings on sound work.
    assert_eq!(comparisons(&world, &run), Vec::<Value>::new());
    assert_eq!(findings(&world, &run), Vec::<String>::new());
    assert_eq!(
        settled_as(&world, &run)["status"],
        "done",
        "the journey did not reach a settlement"
    );
}

#[test]
fn a_bar_written_the_other_way_round_or_over_two_lines_is_still_read() {
    let world = writing("criteria-shapes", "complete_dataset: false\n");
    // Two shapes the sentence can take and one it cannot. The file and the
    // literal are read off the sentence in either order and across a line break;
    // an indented line after a *paragraph* belongs to the paragraph, and
    // stitching it onto the bullet above would compare a sentence the plan never
    // wrote.
    let node = json!({
        "id": "service",
        "repo": "service",
        "persona": "engineer",
        "title": "feat: ship the row",
        "task": "## What\nWrite the row.\n\n## Acceptance criteria\n\n\
                 - `complete_dataset: true` is what `service.md` holds\n\
                 - the row in `README.md`\n  is `the repository under test`\n\
                 - the origin is seeded\n\nAnd separately:\n  \
                 the row in `service.md` is `complete_dataset: true`\n",
    });
    let run = settle(&world, "shapes", vec![node]);

    let compared = comparisons(&world, &run);
    // Two read, and the paragraph's continuation joined neither of them: a third
    // comparison here would be a bar nobody stated.
    assert_eq!(compared.len(), 2, "{compared:?}");
    let answer_for = |named: &str| {
        compared
            .iter()
            .find(|payload| payload["file"] == named)
            .unwrap_or_else(|| panic!("`{named}` was never read: {compared:?}"))["answer"]
            .clone()
    };
    assert_eq!(answer_for("service.md"), "mismatch");
    assert_eq!(answer_for("README.md"), "match");
    assert_eq!(findings(&world, &run).len(), 1);
}

#[test]
fn a_file_the_branch_will_not_give_up_is_the_check_declining_to_answer() {
    let world = World::new("criteria-unread");
    let repository = world.repository("local-direct", &[]);
    // Three ways a branch withholds a file, all three on the branch this node
    // settles on: a directory where the criterion named a file, a file that is
    // not text, and — by naming it and committing nothing — a file that is not
    // there. They reach the check as three different refusals from the operating
    // system and have to come back as the same answer.
    committed(
        &world,
        &repository,
        "rows.md/keep.txt",
        b"this path is a directory\n",
    );
    committed(&world, &repository, "bytes.md", &[0xff, 0xfe, 0x00, 0x9f]);
    world.script("service.work", "complete_dataset: false\n");
    let run = settle(
        &world,
        "unread",
        vec![node_bounded_by(&[
            "the shared journey row in `rows.md` is `complete_dataset: true`",
            "the shared journey row in `bytes.md` is `complete_dataset: true`",
            "the shared journey row in `absent.md` is `complete_dataset: true`",
        ])],
    );

    // The third answer, in the run's own record, told apart from both the
    // others: nothing was compared, so nothing disagreed.
    let compared = comparisons(&world, &run);
    assert_eq!(compared.len(), 3, "{compared:?}");
    for named in ["rows.md", "bytes.md", "absent.md"] {
        let answered = compared
            .iter()
            .find(|payload| payload["file"] == named)
            .unwrap_or_else(|| panic!("`{named}` was never read: {compared:?}"));
        assert_eq!(answered["answer"], "unread", "{answered}");
        assert_ne!(answered["answer"], "match");
        assert_ne!(answered["answer"], "mismatch");
        assert!(
            answered["reason"]
                .as_str()
                .is_some_and(|why| !why.is_empty()),
            "the check declined to answer and said nothing about why: {answered}"
        );
        assert!(
            answered.get("holds").is_none(),
            "a file nobody could read was reported as holding something: {answered}"
        );
    }

    // A file that could not be read is not work that disagreed, so nobody is
    // asked to rule on it.
    assert_eq!(findings(&world, &run), Vec::<String>::new());
}

/// A path that is lexically inside the branch and resolves outside it.
///
/// The lexical rules cannot see this one: `notes.md` names no directory and
/// climbs out of nothing, and what makes it an escape is a symlink somebody
/// committed — which git tracks and a session's clone carries. Unix-only because
/// that is where this suite can commit one; the containment check itself is
/// platform-independent and the module's own test covers it either way.
#[cfg(unix)]
#[test]
fn a_criterion_resolving_off_the_branch_is_refused_rather_than_read() {
    let world = World::new("criteria-symlink");
    let repository = world.repository("local-direct", &[]);
    // Somewhere off the branch, holding exactly what the criterion asks for — so
    // a check that followed the link would answer `match` on evidence that is
    // not the node's work.
    let outside = world.root.join("outside");
    std::fs::create_dir_all(&outside).expect("somewhere off the branch");
    std::fs::write(outside.join("secret.md"), "complete_dataset: true\n")
        .expect("the file off the branch is written");
    std::os::unix::fs::symlink(
        outside.join("secret.md"),
        repository.checkout.join("notes.md"),
    )
    .expect("a symlink out of the worktree");
    git(&world, &repository.checkout, &["add", "-A"]);
    git(
        &world,
        &repository.checkout,
        &["commit", "-m", "chore: commit a symlink off the branch"],
    );
    git(&world, &repository.checkout, &["push", "origin", "main"]);
    world.script("service.work", "complete_dataset: false\n");
    let run = settle(
        &world,
        "symlink",
        vec![node_bounded_by(&[
            "the shared journey row in `notes.md` is `complete_dataset: true`",
        ])],
    );

    // Declined, and saying where the path went: not a match on a file off the
    // branch, and not a mismatch either.
    let compared = comparisons(&world, &run);
    assert_eq!(compared.len(), 1, "{compared:?}");
    assert_eq!(compared[0]["answer"], "unread", "{compared:?}");
    assert!(
        compared[0]["reason"]
            .as_str()
            .is_some_and(|why| why.contains("outside the node's branch")),
        "the refusal does not say the path left the branch: {compared:?}"
    );
    assert_eq!(findings(&world, &run), Vec::<String>::new());
}

#[test]
fn a_workstream_held_at_a_human_step_is_read_when_it_settles_and_not_before() {
    let world = World::new("criteria-humanstep");
    world.repository("local-direct", &[]);
    world.script("service.implement.work", "complete_dataset: false\n");
    let node = json!({
        "id": "service",
        "repo": "service",
        "title": "feat: land the workstream",
        "steps": [
            {
                "id": "implement",
                "persona": "engineer",
                "task": "## What\nimplement\n\n## Acceptance criteria\n\n\
                         - the row in `service-implement.md` is `complete_dataset: true`\n",
            },
            {
                "id": "staging-approval",
                "kind": "human",
                "task": "Exercise the staged service.",
                "deps": ["implement"],
            },
        ],
    });
    let run = settle(&world, "humanstep", vec![node]);

    // Held for a person — and read, because this is where the node settled and
    // the person about to approve it is exactly the reader a finding is for.
    assert_eq!(settled_as(&world, &run)["status"], "waiting");
    let compared = comparisons(&world, &run);
    assert_eq!(compared.len(), 1, "{compared:?}");
    assert_eq!(compared[0]["answer"], "mismatch", "{compared:?}");
    assert_eq!(compared[0]["file"], "service-implement.md");
    assert_eq!(findings(&world, &run).len(), 1);

    // The person acts, a fresh driver picks the run up, and the node settles for
    // real — dispatching nothing where the human step was, so there is no second
    // branch to read and no second finding for one thing already said.
    world.run(&["attest", &run, "service"]).exited(0);
    world.run(&["adopt", &run]).exited(0);
    world.until("the workstream to settle past its human step", |world| {
        world.run_json(&run, "result.json")["nodes"][0]["status"] != "waiting"
    });
    assert_eq!(
        comparisons(&world, &run).len(),
        1,
        "read twice for one branch"
    );
    assert_eq!(findings(&world, &run).len(), 1);
}

/// Put one file on the branch every session of this repository is cut from.
///
/// Bytes rather than text, because two of the journeys here need a path that is
/// not readable *as* text: `rows.md/keep.txt` makes `rows.md` a directory — git
/// tracks files rather than directories, so that is how one arrives in a real
/// tree — and `bytes.md` is committed as bytes no reader can decode.
fn committed(world: &World, repository: &Repository, path: &str, body: &[u8]) {
    let file = repository.checkout.join(path);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).expect("the directories above the file");
    }
    std::fs::write(&file, body).expect("the file is written into the checkout");
    git(world, &repository.checkout, &["add", "-A"]);
    git(
        world,
        &repository.checkout,
        &["commit", "-m", &format!("chore: commit {path}")],
    );
    git(world, &repository.checkout, &["push", "origin", "main"]);
}

#[test]
fn a_bar_stated_in_an_amendment_or_in_a_step_is_read_and_read_once() {
    let world = World::new("criteria-sources");
    world.repository("local-direct", &[]);
    // Both steps write, so what the branch holds when the node settles is both
    // step's files beside the seed file the repository has carried all along.
    world.script("service.implement.work", "complete_dataset: false\n");
    world.script("service.verify.work", "rows: 3\n");
    let bar = |criteria: &[&str]| {
        format!(
            "## What\nWork.\n\n## Acceptance criteria\n\n{}\n",
            criteria
                .iter()
                .map(|criterion| format!("- {criterion}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    // The one criterion two documents state. A node with steps takes its task
    // from them, so the sources here are the amendment binding the node and the
    // steps' own bars.
    let shared = "the row in `README.md` is `the repository under test`";
    let node = json!({
        "id": "service",
        "repo": "service",
        "title": "feat: ship the row",
        "amendment": bar(&["the row in `service-verify.md` is `rows: 4`"]),
        "steps": [
            {"id": "implement", "persona": "engineer", "task": bar(&[shared])},
            {
                "id": "verify",
                "persona": "reviewer",
                "deps": ["implement"],
                "task": bar(&[shared, "the row in `service-implement.md` is `complete_dataset: true`"]),
            },
        ],
    });
    let run = settle(&world, "sources", vec![node]);

    // Every source was read, and the one criterion two of them state was read
    // once: a bar restated is one bar, and comparing it twice would put two
    // findings on the queue for one thing said once.
    let mut files: Vec<String> = comparisons(&world, &run)
        .iter()
        .filter_map(|payload| {
            payload["file"]
                .as_str()
                .map(std::string::ToString::to_string)
        })
        .collect();
    files.sort();
    assert_eq!(
        files,
        ["README.md", "service-implement.md", "service-verify.md"]
    );

    // And each answered on its own evidence: the amendment's names a value the
    // second step did not write, the second step's names one the first did not,
    // and the shared one names the seed file, which holds exactly what it says.
    let answer_for = |named: &str| {
        comparisons(&world, &run)
            .into_iter()
            .find(|payload| payload["file"] == named)
            .unwrap_or_else(|| panic!("`{named}` was never read"))["answer"]
            .clone()
    };
    assert_eq!(answer_for("service-verify.md"), "mismatch");
    assert_eq!(answer_for("service-implement.md"), "mismatch");
    assert_eq!(answer_for("README.md"), "match");
    assert_eq!(findings(&world, &run).len(), 2);
}

#[test]
fn a_node_that_failed_its_dispatch_is_still_read_against_its_bar() {
    let world = writing("criteria-failed", "complete_dataset: false\n");
    // The agent's own verdict on its task: it wrote its work and then failed.
    world.script("service.fail", "1");
    let run = settle(
        &world,
        "failed",
        vec![node_bounded_by(&[
            "the shared journey row in `service.md` is `complete_dataset: true`",
        ])],
    );

    // The node settled on its dispatch, exactly as it would have without this
    // check …
    assert_eq!(settled_as(&world, &run)["status"], "failed");
    assert_eq!(settled_as(&world, &run)["outcome"], "task-failed");
    // … and the branch it left behind was still read, because a node that failed
    // is a node somebody is about to look at.
    let compared = comparisons(&world, &run);
    assert_eq!(compared.len(), 1, "{compared:?}");
    assert_eq!(compared[0]["answer"], "mismatch", "{compared:?}");
    assert_eq!(findings(&world, &run).len(), 1);
}

/// A required check the host reports as red, which is what CI failing looks like
/// to a change request the host was asked to land.
const RED: &str = "llmlint completed failure required";

#[test]
fn a_node_redispatched_after_a_failed_publication_is_read_once_at_its_settlement() {
    let world = World::new("criteria-retry");
    // `change-auto` watches the host's own checks to their conclusion, which is
    // where a red one fails the publication while leaving the work on its
    // branch — the case the node is asked again for.
    world.repository("change-auto", &[]);
    world.script("service.work", "complete_dataset: false\n");
    world.script("gh.checks", RED);

    // Detached, so the world can move while the run is going: the host reports
    // the check red, and once it has, this test makes it green the way a re-run
    // of CI would, and the attempt that follows meets a different answer.
    let path = world.plan(
        "retry",
        &plan_of(
            "retry",
            vec![node_bounded_by(&[
                "the shared journey row in `service.md` is `complete_dataset: true`",
            ])],
        ),
    );
    world.run(&["start", &path, "--detach"]).exited(0);
    let run = "retry".to_string();
    world.until("the host to report its check red", |world| {
        world
            .events_of(&run, "change-check")
            .iter()
            .any(|event| event["payload"]["conclusion"] == "failure")
    });
    std::fs::remove_file(world.fakes.join("gh.checks")).expect("the red check is cleared");
    world.script("gh.merged", "");
    world.until("the run to settle", |world| {
        world.run_file(&run, "result.json").is_file()
    });

    // It was dispatched more than once, and the branch it was asked again on is
    // the one that already contradicted its bar …
    let dispatched = world.events_of(&run, "node-dispatched");
    assert!(
        dispatched.len() >= 2,
        "the publication was never re-dispatched: {dispatched:?}"
    );
    assert_eq!(settled_as(&world, &run)["status"], "done");

    // … so a check made per *attempt* would have reported it twice. It is made
    // where the node settles: one comparison, one finding.
    let compared = comparisons(&world, &run);
    assert_eq!(compared.len(), 1, "{compared:?}");
    assert_eq!(compared[0]["answer"], "mismatch", "{compared:?}");
    assert_eq!(findings(&world, &run).len(), 1);
}

#[test]
fn a_node_with_no_branch_to_read_reports_nothing() {
    let world = World::new("criteria-nobranch");
    world.repository("local-direct", &[]);
    // Every step declares no diff, so no session opens and there is no branch:
    // the criterion names a file that exists on the base, and is still not read,
    // because what this check reads is the node's own work.
    let node = json!({
        "id": "service",
        "repo": "service",
        "title": "feat: change nothing",
        "steps": [{
            "id": "note",
            "expects_no_diff": true,
            "task": "## What\nChange nothing.\n\n## Acceptance criteria\n\n\
                     - the row in `README.md` is `complete_dataset: true`\n",
        }],
    });
    let run = settle(&world, "nobranch", vec![node]);

    assert_eq!(settled_as(&world, &run)["status"], "done");
    assert_eq!(comparisons(&world, &run), Vec::<Value>::new());
    assert_eq!(findings(&world, &run), Vec::<String>::new());
}
