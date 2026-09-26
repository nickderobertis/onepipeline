//! A landed retry tells `onevcs` what it superseded, and an idle driver retires
//! what provably holds no work.
//!
//! Every journey drives the compiled binary against the **real** linked `onevcs`
//! over real git: a registered checkout, a bare origin on disk, and the sibling's
//! own state root. What a journey asserts about a branch is that library's own
//! answer, read through its own `onevcs retire --dry-run --json`, so the class a
//! branch is given is the class the retirement pass acts on rather than one this
//! suite believes it would be given.
//!
//! A lineage here is the one the run's journal holds: a node whose first attempt
//! failed on one branch, `retry`'d with a replacement on a branch of its own, and
//! that replacement landing — through its publication closeout, through a
//! planner's `settle` stating where it landed, or through a `merge-completed` its
//! own worker's publication recorded on the session's stream.

// llmlint: ignore-file[e2e_not_mocked] the crate under test is driven as a real compiled
// binary and `onevcs` — the sibling these journeys are about — is the real library, over
// real git and a real origin on disk. `oneagentgraph` is substituted at its subprocess
// boundary so a journey states a dispatch outcome rather than paying for a model turn, and
// GitHub at `onevcs`'s own `ONEVCS_GH` override. `harness.rs` carries the same suppression
// and the full rationale.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::harness::{git, lifecycle, onevcs_binary, plan_of, Repository, World};

/// The branch every lineage's first attempt works on.
const FIRST: &str = "try/first";
/// The branch a replacement on a branch of its own works on.
const SECOND: &str = "try/second";
/// What the first attempt's worker writes, into `svc.md`.
const FIRST_WORK: &str = "the first attempt wrote this\n";

/// The first attempt of the lineage every journey here retries: a lifecycle node
/// pinned to [`FIRST`] whose worker writes [`FIRST_WORK`] and fails its task, so
/// its work is preserved on that branch.
fn first_attempt(world: &World, run: &str) {
    world.script("svc.work", FIRST_WORK);
    world.script("svc.fail", "1");
    let mut node = lifecycle("svc", &[]);
    node["branch"] = json!(FIRST);
    let path = world.plan(run, &plan_of(run, vec![node]));
    world.run(&["start", &path, "--attach"]).settled();
    world.until("the first attempt to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(
        settlement(world, run, "svc")["status"],
        "failed",
        "{}",
        world.dump()
    );
}

/// Replace `svc` with `svc-2`, on `branch` or — `None` — on whatever the
/// reconciler pins it to, which is the branch `svc` preserved.
fn retry(world: &World, run: &str, branch: Option<&str>) {
    let mut node = json!({
        "id": "svc-2", "repo": "service", "persona": "engineer",
        "title": "feat: ship svc",
        "task": "## What\nShip it again.\n\n## Why\nThe first try failed.\n\n\
                 ## Acceptance criteria\n- shipped.",
    });
    if let Some(branch) = branch {
        node["branch"] = json!(branch);
    }
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [{"op": "retry", "id": "svc", "node": node}]})
                .to_string(),
        )
        .exited(0);
}

/// Drive the run again until it settles, as `adopt` does for a run nothing drives.
fn adopted(world: &World, run: &str) {
    world.run(&["adopt", run]).settled();
}

/// The newest settlement a node recorded.
fn settlement(world: &World, run: &str, node: &str) -> Value {
    world
        .events_of(run, "node-settled")
        .into_iter()
        .rfind(|event| event["labels"]["node"] == node)
        .unwrap_or_else(|| panic!("{node} never settled\n{}", world.dump()))["payload"]
        .clone()
}

/// Every `branches-superseded` a run journalled.
fn superseded(world: &World, run: &str) -> Vec<Value> {
    world.events_of(run, "branches-superseded")
}

/// `onevcs`'s own classification of one branch of `service`, through the verb a
/// person asks it with.
fn classified(world: &World, branch: &str) -> Value {
    let output = world
        .cmd_on(
            &onevcs_binary(),
            &["retire", branch, "--repo", "service", "--dry-run", "--json"],
        )
        .output()
        .expect("onevcs runs");
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "`onevcs retire {branch} --dry-run --json` printed no answer ({error}): {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// A person's own clone of the origin, for the commits a journey makes on the
/// base by hand.
fn person(world: &World, repo: &Repository) -> PathBuf {
    let clone = world.root.join("person");
    if !clone.is_dir() {
        git(
            world,
            &world.root,
            &["clone", &repo.origin.to_string_lossy(), "person"],
        );
    }
    git(world, &clone, &["checkout", "main"]);
    git(world, &clone, &["pull", "--ff-only", "origin", "main"]);
    clone
}

/// Put `contents` at `file` on the origin's base, from a person's clone, and bring
/// the registered checkout up to it.
fn on_the_base(world: &World, repo: &Repository, file: &str, contents: &str) {
    let clone = person(world, repo);
    std::fs::write(clone.join(file), contents).expect("the file is written");
    git(world, &clone, &["add", "-A"]);
    git(
        world,
        &clone,
        &["commit", "-m", &format!("chore: carry {file}")],
    );
    git(world, &clone, &["push", "origin", "main"]);
    synced(world, repo);
}

/// Fast-forward the publication checkout to its origin, through the sibling.
fn synced(world: &World, repo: &Repository) {
    let output = world
        .cmd_on(&onevcs_binary(), &["sync"])
        .current_dir(&repo.checkout)
        .output()
        .expect("onevcs runs");
    assert!(
        output.status.success(),
        "onevcs sync refused: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// What the first attempt's branch is to `onevcs` once a retry superseded it,
/// before and after the base carries the one path it changed.
///
/// While the base lacks `svc.md` the branch still differs from it in a path the
/// retry did not carry, so it is `superseded-with-changes`, naming the retry's
/// branch as what superseded it. Once the base reads `svc.md` exactly as the
/// branch does, every path it changed is on the base and it is `retirable`.
fn first_is_superseded_then_retirable(world: &World, repo: &Repository) {
    let before = classified(world, FIRST);
    assert_eq!(before["class"], "superseded-with-changes", "{before}");
    assert_eq!(before["superseded_by"]["branch"], SECOND, "{before}");
    assert_eq!(before["differing_paths"], json!(["svc.md"]), "{before}");
    // Exactly what the branch carries, read off the branch.
    let carried = git(world, &repo.checkout, &["show", &format!("{FIRST}:svc.md")]);
    on_the_base(world, repo, "svc.md", &carried);
    let after = classified(world, FIRST);
    assert_eq!(after["class"], "retirable", "{after}");
}

/// The one `branches-superseded` a lineage wrote, naming its first attempt.
fn recorded_the_first_attempt(world: &World, run: &str) -> Value {
    let records = superseded(world, run);
    assert_eq!(records.len(), 1, "{records:?}\n{}", world.dump());
    let payload = records[0]["payload"].clone();
    assert_eq!(payload["node"], "svc-2", "{payload}");
    assert_eq!(
        payload["superseded"],
        json!([{"node": "svc", "branch": FIRST}]),
        "{payload}"
    );
    assert_eq!(payload["failed"], json!([]), "{payload}");
    payload
}

/// A retry on a branch of its own that lands through its publication closeout
/// records its first attempt as superseded, at the commit it landed at.
#[test]
fn a_retry_its_closeout_lands_records_the_attempt_it_superseded() {
    let world = World::new("retirement-closeout");
    let repo = world.repository("local-direct", &[]);
    let run = "closeout";
    first_attempt(&world, run);
    assert!(repo.has_branch(&world, FIRST), "{}", world.dump());

    world.script("svc-2.work", "the second attempt wrote this\n");
    retry(&world, run, Some(SECOND));
    adopted(&world, run);
    let landed = settlement(&world, run, "svc-2");
    assert_eq!(landed["status"], "done", "{landed}\n{}", world.dump());
    assert_eq!(landed["outcome"], "merged", "{landed}");

    let payload = recorded_the_first_attempt(&world, run);
    let landing = payload["landing"].as_str().expect("a landing");
    assert_eq!(landing.len(), 40, "the landing is not a commit: {payload}");
    assert_eq!(
        git(&world, &repo.origin, &["rev-parse", "main"]).trim(),
        landing,
        "the landing recorded is not where the base stands"
    );
    first_is_superseded_then_retirable(&world, &repo);
}

/// The first attempt again, in a run a held direct node keeps a driver alive for,
/// so what follows is applied by the loop rather than by the process replying.
fn first_attempt_beside_a_hold(world: &World, run: &str) {
    world.script("svc.work", FIRST_WORK);
    world.script("svc.fail", "1");
    world.script("hold.wait", "hold");
    let mut node = lifecycle("svc", &[]);
    node["branch"] = json!(FIRST);
    let path = world.plan(
        run,
        &plan_of(run, vec![node, crate::harness::agent("hold", &[])]),
    );
    world.run(&["start", &path, "--detach"]).exited(0);
    until_settled(world, run, "svc");
    assert_eq!(settlement(world, run, "svc")["status"], "failed");
}

/// Wait until a node has recorded a settlement.
fn until_settled(world: &World, run: &str, node: &str) {
    world.until(&format!("{node} to settle"), |world| {
        world
            .events_of(run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == node)
    });
}

/// Land the replacement's preserved branch on the base by hand, from a person's
/// clone, and answer the commit it landed at.
fn landed_by_hand(world: &World, repo: &Repository) -> String {
    let clone = person(world, repo);
    git(
        world,
        &clone,
        &["fetch", &repo.checkout.to_string_lossy(), SECOND],
    );
    let tip = git(world, &clone, &["rev-parse", "FETCH_HEAD"])
        .trim()
        .to_owned();
    git(world, &clone, &["push", "origin", "FETCH_HEAD:main"]);
    synced(world, repo);
    tip
}

/// Settle `svc-2` done at `landing`, as a planner does once a person landed it.
fn settle_at(world: &World, run: &str, landing: &str) {
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [{
                "op": "settle", "id": "svc-2", "outcome": "done",
                "evidence": "a person landed its branch on the base by hand",
                "landing": landing,
            }]})
            .to_string(),
        )
        .exited(0)
        .out_has("\"applied\"");
}

/// Take the sibling's registry away from it — the document it resolves a
/// repository through, which it reads before it records anything — or give it
/// back.
///
/// A registry it cannot read is one naming nothing, so a supersession of any
/// repository is refused as naming no registered one. What does **not** refuse
/// it is a stream it cannot write: `onevcs` warns on a record it could not append
/// and goes on, by design, so that is not a refusal a journey can reach.
#[cfg(unix)]
fn registry_readable(world: &World, yes: bool) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        world.onevcs_home().join("registry.json"),
        std::fs::Permissions::from_mode(if yes { 0o644 } else { 0o000 }),
    )
    .expect("the permission changes");
}

/// A run whose replacement failed on [`SECOND`] and was then landed by hand and
/// settled at that landing by a planner, with a driver alive beside it —
/// `onevcs` unable to read its registry while the settle is taken up, where
/// `refused` says so. Answers the run's `result.json` entry for `svc-2` and the
/// `branches-superseded` it wrote.
#[cfg(unix)]
fn settled_at_a_landing(world: &World, run: &str, refused: bool) -> (Value, Value, String) {
    let repo = world.repository("local-direct", &[]);
    first_attempt_beside_a_hold(world, run);
    world.script("svc-2.work", "the second attempt wrote this\n");
    world.script("svc-2.fail", "1");
    retry(world, run, Some(SECOND));
    until_settled(world, run, "svc-2");
    assert_eq!(settlement(world, run, "svc-2")["status"], "failed");
    let landing = landed_by_hand(world, &repo);

    if refused {
        registry_readable(world, false);
    }
    settle_at(world, run, &landing);
    world.until("the supersession to be journalled", |world| {
        !superseded(world, run).is_empty()
    });
    registry_readable(world, true);

    world.release("hold.go");
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    let result = world.run_json(run, "result.json");
    let node = result["nodes"]
        .as_array()
        .and_then(|nodes| nodes.iter().find(|node| node["id"] == "svc-2"))
        .unwrap_or_else(|| panic!("the result names no svc-2: {result}"))
        .clone();
    let records = superseded(world, run);
    assert_eq!(records.len(), 1, "{records:?}");
    (node, records[0]["payload"].clone(), landing)
}

/// A planner's `settle` stating where a retry on a branch of its own landed
/// records its first attempt as superseded, at that landing.
#[cfg(unix)]
#[test]
fn a_settle_stating_where_a_retry_landed_records_the_attempt_it_superseded() {
    let world = World::new("retirement-settle");
    let (node, payload, landing) = settled_at_a_landing(&world, "settled", false);
    assert_eq!(node["status"], "done", "{node}");
    assert_eq!(node["landing"], "landed", "{node}");
    recorded_the_first_attempt(&world, "settled");
    assert_eq!(payload["landing"], landing, "{payload}");
    first_is_superseded_then_retirable(&world, &repository_of(&world));
}

/// The world's `service` repository, as [`World::repository`] laid it out.
fn repository_of(world: &World) -> Repository {
    Repository {
        origin: world.root.join("origin.git"),
        checkout: world.root.join("service"),
    }
}

/// A recording `onevcs` refuses is journalled — the attempt, its branch and what
/// was answered — and the settlement is exactly what it is when the recording
/// succeeds.
#[cfg(unix)]
#[test]
fn a_recording_onevcs_refuses_is_journalled_and_changes_nothing_about_the_settlement() {
    let refused = World::new("retirement-refused");
    let (node, payload, landing) = settled_at_a_landing(&refused, "refused", true);
    assert_eq!(payload["node"], "svc-2", "{payload}");
    assert_eq!(payload["landing"], landing, "{payload}");
    assert_eq!(payload["superseded"], json!([]), "{payload}");
    let failed = payload["failed"].as_array().expect("failed");
    assert_eq!(failed.len(), 1, "{payload}");
    assert_eq!(failed[0]["node"], "svc", "{payload}");
    assert_eq!(failed[0]["branch"], FIRST, "{payload}");
    assert!(
        failed[0]["error"]
            .as_str()
            .is_some_and(|error| !error.is_empty()),
        "{payload}"
    );
    // `onevcs` holds no record of it, so the branch is still unknown work to it.
    let class = classified(&refused, FIRST);
    assert_ne!(class["class"], "superseded-with-changes", "{class}");

    // The same run with the recording accepted settles the node identically.
    let accepted = World::new("retirement-accepted");
    let (control, _, _) = settled_at_a_landing(&accepted, "refused", false);
    for field in ["status", "outcome", "landing"] {
        assert_eq!(node[field], control[field], "{field}: {node} vs {control}");
    }
}

/// A `merge-completed` its own worker's publication recorded on the session's
/// stream is a landing: the replacement settles `failed` on its task, and its
/// first attempt is still recorded as superseded, at that merge.
#[test]
fn a_merge_completed_a_retry_recorded_records_the_attempt_it_superseded() {
    let world = World::new("retirement-merge-completed");
    let repo = world.repository("local-direct", &[]);
    let run = "merged";
    first_attempt(&world, run);

    world.script("svc-2.work", "the second attempt wrote this\n");
    world.script("svc-2.publishes", "feat: ship svc");
    world.script("svc-2.fail", "1");
    retry(&world, run, Some(SECOND));
    adopted(&world, run);
    let settled = settlement(&world, run, "svc-2");
    assert_eq!(settled["status"], "failed", "{settled}\n{}", world.dump());
    assert!(
        world
            .journal(run)
            .iter()
            .any(|event| event["source"] == "vcs"
                && event["kind"] == "merge-completed"
                && event["labels"]["node"] == "svc-2"),
        "the worker's publication recorded no merge\n{}",
        world.dump()
    );

    let payload = recorded_the_first_attempt(&world, run);
    assert_eq!(
        git(&world, &repo.origin, &["rev-parse", "main"]).trim(),
        payload["landing"],
        "the landing recorded is not the merge"
    );
    first_is_superseded_then_retirable(&world, &repo);
}

/// A retry continuing the branch its first attempt preserved is one branch
/// `onevcs` chains itself: nothing is recorded, and the backfill says so.
#[test]
fn a_retry_on_the_branch_its_first_attempt_left_records_nothing() {
    let world = World::new("retirement-same-branch");
    world.repository("local-direct", &[]);
    let run = "same";
    first_attempt(&world, run);

    world.script("svc-2.work", "the second attempt wrote this\n");
    retry(&world, run, None);
    adopted(&world, run);
    let landed = settlement(&world, run, "svc-2");
    assert_eq!(landed["status"], "done", "{landed}\n{}", world.dump());
    assert_eq!(landed["branch"], FIRST, "{landed}");

    assert!(superseded(&world, run).is_empty(), "{}", world.dump());
    world
        .run(&["supersessions", run])
        .exited(0)
        .out_has(&format!("svc-2 landed {FIRST} at "))
        .out_has("every earlier attempt was on the branch that landed")
        .out_lacks("superseded svc");
    let class = classified(&world, FIRST);
    assert_ne!(class["class"], "superseded-with-changes", "{class}");
}

/// The run's journal as a build before `branches-superseded` existed left it:
/// this build's, less the records of that one kind — and less the fold
/// checkpoint, which is a reader's shortcut over bytes that are no longer there.
fn as_an_older_build_left_it(world: &World, run: &str) {
    let journal = world.run_file(run, "events.jsonl");
    let kept: String = std::fs::read_to_string(&journal)
        .expect("the journal reads")
        .lines()
        .filter(|line| {
            serde_json::from_str::<Value>(line)
                .map_or(true, |event| event["kind"] != "branches-superseded")
        })
        .map(|line| format!("{line}\n"))
        .collect();
    std::fs::write(&journal, kept).expect("the journal is written");
    let _ = std::fs::remove_file(world.run_file(run, "checkpoint.json"));
    assert!(superseded(world, run).is_empty());
}

/// `onepipeline supersessions` over a settled run whose journal holds a landed
/// retry but no `branches-superseded`: it prints the pair, records it with
/// `--record`, answers `1` naming the branch when `onevcs` refuses, and records
/// nothing new the second time. An unknown run is refused.
#[cfg(unix)]
#[test]
fn the_backfill_verb_records_what_a_settled_run_never_did_once() {
    let world = World::new("retirement-backfill");
    let repo = world.repository("local-direct", &[]);
    let run = "backfill";
    first_attempt(&world, run);
    world.script("svc-2.work", "the second attempt wrote this\n");
    world.script("svc-2.fail", "1");
    retry(&world, run, Some(SECOND));
    adopted(&world, run);
    let landing = landed_by_hand(&world, &repo);
    // Settled with nothing driving the run, while `onevcs` can record nothing: the
    // sibling holds no supersession, which is where an older build left it too.
    registry_readable(&world, false);
    settle_at(&world, run, &landing);
    registry_readable(&world, true);
    as_an_older_build_left_it(&world, run);
    let class = classified(&world, FIRST);
    assert_ne!(class["class"], "superseded-with-changes", "{class}");

    // Answered: the pair, and nothing recorded.
    world
        .run(&["supersessions", run])
        .exited(0)
        .out_has(&format!("svc-2 landed {SECOND} at {landing}"))
        .out_has(&format!(
            "superseded svc on {FIRST}: to record: run again with --record"
        ));
    let answered = world
        .run(&["supersessions", run, "--json"])
        .exited(0)
        .json();
    assert_eq!(answered["record"], false, "{answered}");
    assert_eq!(
        answered["lineages"][0]["to_record"],
        json!([{"node": "svc", "branch": FIRST}]),
        "{answered}"
    );
    assert!(superseded(&world, run).is_empty());
    assert_ne!(
        classified(&world, FIRST)["class"],
        "superseded-with-changes"
    );

    // Refused by `onevcs`: exit 1, naming the branch.
    registry_readable(&world, false);
    world
        .run(&["supersessions", run, "--record"])
        .exited(1)
        .out_has(&format!("superseded svc on {FIRST}: not recorded: "))
        .out_has(&format!("onevcs refused to record {FIRST}"));
    registry_readable(&world, true);

    // Recorded: the same function a driver calls, and the same record.
    world
        .run(&["supersessions", run, "--record"])
        .exited(0)
        .out_has(&format!("superseded svc on {FIRST}: recorded"));
    let records = superseded(&world, run);
    let recorded: Vec<&Value> = records
        .iter()
        .filter(|record| record["payload"]["superseded"] != json!([]))
        .collect();
    assert_eq!(recorded.len(), 1, "{records:?}");
    assert_eq!(recorded[0]["payload"]["node"], "svc-2");
    assert_eq!(recorded[0]["payload"]["landing"], landing);
    assert_eq!(
        recorded[0]["payload"]["superseded"],
        json!([{"node": "svc", "branch": FIRST}])
    );

    // Twice: nothing new, and still a success.
    let before = superseded(&world, run).len();
    world
        .run(&["supersessions", run, "--record"])
        .exited(0)
        .out_has(&format!("superseded svc on {FIRST}: already recorded"));
    assert_eq!(superseded(&world, run).len(), before);
    first_is_superseded_then_retirable(&world, &repo);

    world
        .run(&["supersessions", "no-such-run"])
        .exited(crate::harness::REFUSED);
}
