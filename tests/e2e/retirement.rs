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
// llmlint: ignore-file[expensive_tests_stay_behind_their_own_edge] every journey here
// verifies behavior of the root crate's own source — `src/supersession.rs`'s recording and
// backfill verb and `src/maintenance.rs`'s retirement pass — through the real sibling, so
// the root source edge is the edge that owns it, as `dispatch.rs`'s real-sibling journeys
// state for theirs.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::harness::{git, lifecycle, onevcs_binary, plan_of, Repository, World};

const FIRST: &str = "try/first";
const SECOND: &str = "try/second";
const THIRD: &str = "try/third";
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
    retry_of(world, run, "svc", "svc-2", branch);
}

/// Replace `retried` with `replacement`, on `branch` or the one it preserved.
fn retry_of(world: &World, run: &str, retried: &str, replacement: &str, branch: Option<&str>) {
    let mut node = json!({
        "id": replacement, "repo": "service", "persona": "engineer",
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
            &json!({"version": 2, "commands": [{"op": "retry", "id": retried, "node": node}]})
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
    // A fresh driver is seeded from the journal, so it records nothing again.
    adopted(&world, run);
    assert_eq!(superseded(&world, run).len(), 1, "{}", world.dump());
    first_is_superseded_then_retirable(&world, &repo);
}

/// The first attempt again, in a run a held direct node keeps a driver alive for,
/// so what follows is applied by the loop rather than by the process replying.
/// Answers the pid the detached launch announced for that driver.
#[cfg(unix)]
fn first_attempt_beside_a_hold(world: &World, run: &str) -> u32 {
    world.script("svc.work", FIRST_WORK);
    world.script("svc.fail", "1");
    world.script("hold.wait", "hold");
    let mut node = lifecycle("svc", &[]);
    node["branch"] = json!(FIRST);
    let path = world.plan(
        run,
        &plan_of(run, vec![node, crate::harness::agent("hold", &[])]),
    );
    let started = world.run(&["start", &path, "--detach"]);
    started.exited(0);
    until_settled(world, run, "svc");
    assert_eq!(settlement(world, run, "svc")["status"], "failed");
    started.json()["pid"]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
        .expect("a detached launch announces its driver's pid")
}

/// Wait until a node has recorded a settlement.
#[cfg(unix)]
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
#[cfg(unix)]
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

/// Squash the replacement's preserved branch onto the base by hand, as a host
/// merges the change request at `url`: one commit whose subject names it.
#[cfg(unix)]
fn squashed_by_hand(world: &World, repo: &Repository, url: &str) {
    let clone = person(world, repo);
    git(
        world,
        &clone,
        &["fetch", &repo.checkout.to_string_lossy(), SECOND],
    );
    git(world, &clone, &["merge", "--squash", "FETCH_HEAD"]);
    let number = url.rsplit('/').next().expect("a change request's number");
    git(
        world,
        &clone,
        &["commit", "-m", &format!("feat: ship svc (#{number})")],
    );
    git(world, &clone, &["push", "origin", "main"]);
    synced(world, repo);
}

/// Settle `svc-2` done at `landing`, as a planner does once a person landed it.
#[cfg(unix)]
fn settle_at(world: &World, run: &str, landing: &str) {
    world
        .run_with_stdin(&["reply", run], &settling_at(landing).to_string())
        .exited(0)
        .out_has("\"applied\"");
}

/// The envelope a planner settles `svc-2` done at `landing` with.
#[cfg(unix)]
fn settling_at(landing: &str) -> Value {
    json!({"version": 2, "commands": [{
        "op": "settle", "id": "svc-2", "outcome": "done",
        "evidence": "a person landed its branch on the base by hand",
        "landing": landing,
    }]})
}

/// Take the sibling's registry away from it — the document it resolves a
/// repository through, which it reads before it records anything — or give it
/// back.
///
/// A registry it cannot read is one naming nothing, so a supersession of any
/// repository is refused as naming no registered one. What does **not** refuse
/// it is a stream it cannot write: `onevcs` warns on a record it could not append
/// and goes on, by design, so that is not a refusal a journey can reach.
// llmlint: ignore-block[tests_mirror_real_usage] no `onevcs` verb makes the library refuse
// a record, and the task names a host-side fault as the way a refusal happens; a registry
// the process cannot read is that fault, produced as a host produces it. Every call
// through it is still the real library's, reached through the real binary.
#[cfg(unix)]
fn registry_readable(world: &World, yes: bool) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        world.onevcs_home().join("registry.json"),
        std::fs::Permissions::from_mode(if yes { 0o644 } else { 0o000 }),
    )
    .expect("the permission changes");
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// A run whose replacement failed on [`SECOND`] and was then landed by hand and
/// settled at that landing by a planner, with a driver alive beside it —
/// `onevcs` unable to read its registry while the settle is taken up, where
/// `refused` says so. Landed as the change request at `stated` squashes, and
/// settled at that URL, where one is given; else pushed as it is and settled at
/// that commit. Answers the run's `result.json` entry for `svc-2`, the
/// `branches-superseded` it wrote, and the landing the settle stated.
#[cfg(unix)]
fn settled_at_a_landing(
    world: &World,
    run: &str,
    refused: bool,
    stated: Option<&str>,
) -> (Value, Value, String) {
    let repo = world.repository("local-direct", &[]);
    first_attempt_beside_a_hold(world, run);
    world.script("svc-2.work", "the second attempt wrote this\n");
    world.script("svc-2.fail", "1");
    retry(world, run, Some(SECOND));
    until_settled(world, run, "svc-2");
    assert_eq!(settlement(world, run, "svc-2")["status"], "failed");
    let landing = match stated {
        Some(url) => {
            squashed_by_hand(world, &repo, url);
            url.to_owned()
        }
        None => landed_by_hand(world, &repo),
    };

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
    let (node, payload, landing) = settled_at_a_landing(&world, "settled", false, None);
    assert_eq!(node["status"], "done", "{node}");
    assert_eq!(node["landing"], "landed", "{node}");
    recorded_the_first_attempt(&world, "settled");
    assert_eq!(payload["landing"], landing, "{payload}");
    first_is_superseded_then_retirable(&world, &repository_of(&world));
}

/// An attempt whose driver died mid-dispatch never settles, so nothing but the
/// session it opened names its branch — and that branch is what is recorded as
/// superseded once the retry an adopting driver runs lands.
///
/// **Unix**, because that driver is ended by pid and `harness::end_process` has
/// no Windows spelling — the terms `driver.rs`'s adoption journeys are gated on.
#[cfg(unix)]
#[test]
fn an_attempt_that_never_settled_is_recorded_on_the_branch_its_session_opened() {
    let world = World::new("retirement-unsettled");
    world.repository("local-direct", &[]);
    let run = "unsettled";
    world.script("svc.wait", "hold");
    let mut node = lifecycle("svc", &[]);
    node["branch"] = json!(FIRST);
    let path = world.plan(run, &plan_of(run, vec![node]));
    let started = world.run(&["start", &path, "--detach"]);
    started.exited(0);
    let driver = started.json()["pid"]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
        .expect("a detached launch announces its driver's pid");
    world.until("the first attempt's session to open", |world| {
        !world.events_of(run, "session-opened").is_empty()
    });
    crate::harness::end_process(driver);
    // llmlint: ignore-block[tests_mirror_real_usage] the worker double commits only when its
    // hold is released, and this journey's point is a driver that died before that, so the
    // commit the attempt would have made is made here, in the session's own worktree.
    let opened = &world.events_of(run, "session-opened")[0]["payload"];
    assert_eq!(opened["branch"], FIRST, "{opened}");
    let worktree = PathBuf::from(opened["worktree"].as_str().expect("a worktree"));
    std::fs::write(worktree.join("svc.md"), FIRST_WORK).expect("the work is written");
    git(&world, &worktree, &["add", "-A"]);
    git(&world, &worktree, &["commit", "-m", "feat: ship svc"]);
    // llmlint: ignore-end[tests_mirror_real_usage]

    world.script("svc-2.work", "the second attempt wrote this\n");
    retry(&world, run, Some(SECOND));
    world.release("svc.go");
    adopted(&world, run);
    let landed = settlement(&world, run, "svc-2");
    assert_eq!(landed["outcome"], "merged", "{landed}\n{}", world.dump());
    assert!(
        !world
            .events_of(run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "svc"),
        "the first attempt settled, so its branch was not read off its session\n{}",
        world.dump()
    );
    recorded_the_first_attempt(&world, run);
    let class = classified(&world, FIRST);
    assert_eq!(class["class"], "superseded-with-changes", "{class}");
    assert_eq!(class["superseded_by"]["branch"], SECOND, "{class}");
}

/// A `settle` whose landing is the change request's URL, with no commit stated,
/// tells `onevcs` that URL as the landing.
#[cfg(unix)]
#[test]
fn a_settle_stating_a_change_request_records_its_url_as_the_landing() {
    let world = World::new("retirement-settle-url");
    let url = "https://github.com/owner/service/pull/7";
    let (node, payload, _) = settled_at_a_landing(&world, "settled", false, Some(url));
    assert_eq!(node["status"], "done", "{node}");
    recorded_the_first_attempt(&world, "settled");
    assert_eq!(payload["landing"], url, "{payload}");
    let class = classified(&world, FIRST);
    assert_eq!(class["class"], "superseded-with-changes", "{class}");
    assert_eq!(class["superseded_by"]["branch"], SECOND, "{class}");
}

/// The world's `service` repository, as [`World::repository`] laid it out.
#[cfg(unix)]
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
    let (node, payload, landing) = settled_at_a_landing(&refused, "refused", true, None);
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
    let (control, _, _) = settled_at_a_landing(&accepted, "refused", false, None);
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

/// A lineage retried twice, each attempt on a branch of its own, records every
/// earlier attempt against the one landing, root first.
#[test]
fn a_lineage_retried_twice_records_every_earlier_attempt_root_first() {
    let world = World::new("retirement-chain");
    let repo = world.repository("local-direct", &[]);
    let run = "chain";
    first_attempt(&world, run);
    world.script("svc-2.work", "the second attempt wrote this\n");
    world.script("svc-2.fail", "1");
    retry(&world, run, Some(SECOND));
    adopted(&world, run);
    assert_eq!(settlement(&world, run, "svc-2")["status"], "failed");

    world.script("svc-3.work", "the third attempt wrote this\n");
    retry_of(&world, run, "svc-2", "svc-3", Some(THIRD));
    adopted(&world, run);
    let landed = settlement(&world, run, "svc-3");
    assert_eq!(landed["outcome"], "merged", "{landed}\n{}", world.dump());

    let records = superseded(&world, run);
    assert_eq!(records.len(), 1, "{records:?}");
    let payload = &records[0]["payload"];
    assert_eq!(payload["node"], "svc-3", "{payload}");
    assert_eq!(
        payload["superseded"],
        json!([{"node": "svc", "branch": FIRST}, {"node": "svc-2", "branch": SECOND}]),
        "{payload}"
    );
    assert_eq!(
        git(&world, &repo.origin, &["rev-parse", "main"]).trim(),
        payload["landing"]
    );
    for branch in [FIRST, SECOND] {
        let class = classified(&world, branch);
        assert_eq!(class["class"], "superseded-with-changes", "{class}");
        assert_eq!(class["superseded_by"]["branch"], THIRD, "{class}");
    }
}

/// A `settle` stating where a retry landed, queued behind a driver that then dies
/// holding the run, is applied by the `reply` that accepted it — which takes the
/// run over and reconciles its queue — and the attempt the retry superseded is
/// recorded there, with nothing driving the run.
#[cfg(unix)]
#[test]
fn a_settle_a_dead_drivers_queue_held_records_the_attempt_it_superseded() {
    use std::io::Write;

    let world = World::new("retirement-takeover");
    let repo = world.repository("local-direct", &[]);
    let run = "takeover";
    let driver = first_attempt_beside_a_hold(&world, run);
    world.script("svc-2.work", "the second attempt wrote this\n");
    world.script("svc-2.fail", "1");
    retry(&world, run, Some(SECOND));
    until_settled(&world, run, "svc-2");
    let landing = landed_by_hand(&world, &repo);

    // llmlint: ignore-block[tests_mirror_real_usage] an unwritable shadow store is a host
    // state no verb produces; `driver.rs`'s `unwritable_shadow_store` says why its fixture is.
    crate::driver::unwritable_shadow_store(&world, run);
    // llmlint: ignore-end[tests_mirror_real_usage]
    world.release("hold.go");
    until_settled(&world, run, "hold");

    let queue = world.run_file(run, "channel/commands.jsonl");
    let queued = || {
        std::fs::read_to_string(&queue)
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count()
    };
    let before = queued();
    let mut replying = world
        .cmd(&["reply", run])
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "3")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the reply starts");
    let mut stdin = replying.stdin.take().expect("stdin is piped");
    write!(stdin, "{}", settling_at(&landing)).expect("the envelope is written");
    drop(stdin);
    world.until("the settle to reach the run's command queue", |_| {
        queued() == before + 1
    });
    assert!(superseded(&world, run).is_empty(), "{}", world.dump());

    // The driver dies holding the run, and the reply takes it over.
    crate::harness::end_process(driver);
    let answered = replying.wait_with_output().expect("the reply answers");
    let said = String::from_utf8_lossy(&answered.stdout);
    assert!(
        answered.status.success() && said.contains("\"applied\""),
        "the queued settle was not applied by the reply: {said}{}",
        String::from_utf8_lossy(&answered.stderr)
    );
    assert!(
        world.events_of(run, "driver-adopted").is_empty(),
        "something adopted the run, so this journey proves nothing about the reply"
    );
    let payload = recorded_the_first_attempt(&world, run);
    assert_eq!(payload["landing"], landing, "{payload}");
    let class = classified(&world, FIRST);
    assert_eq!(class["class"], "superseded-with-changes", "{class}");
    assert_eq!(class["superseded_by"]["branch"], SECOND, "{class}");
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
// llmlint: ignore-block[tests_mirror_real_usage] the state the backfill verb exists for is
// one only a build *older* than this one writes — a run that settled with a landed retry
// and no `branches-superseded` — and this build writes that record at every landing,
// refused or not, so no entry point of it reaches the state. What an older build left is
// exactly this build's journal less the records of the one kind it did not know, which is
// what is written here; everything else in the run, the lineage and the landing included,
// is the real binary's own.
#[cfg(unix)]
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
// llmlint: ignore-end[tests_mirror_real_usage]

/// `onepipeline supersessions` over a settled run whose journal holds a landed
/// retry but no `branches-superseded`: it prints the pair, records it with
/// `--record`, answers `1` naming the branch on stderr when `onevcs` refuses, and
/// records nothing new the second time — in lines and in `--json` alike. A run
/// with no landed retry has nothing to record, and an unknown run is refused.
#[cfg(unix)]
#[test]
fn the_backfill_verb_records_what_a_settled_run_never_did_once() {
    let world = World::new("retirement-backfill");
    let repo = world.repository("local-direct", &[]);
    let run = "backfill";
    first_attempt(&world, run);
    // Nothing retried yet, so nothing landed that superseded anything.
    world
        .run(&["supersessions", run])
        .exited(0)
        .out_has(&format!(
            "run {run}: no retry that landed has an earlier attempt, so there is nothing to \
             record"
        ));
    let nothing = world
        .run(&["supersessions", run, "--record", "--json"])
        .exited(0)
        .json();
    assert_eq!(nothing["lineages"], json!([]), "{nothing}");

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
    // The `reply` that applied it, with nothing driving the run, asked and
    // journalled the refusal itself.
    let asked = superseded(&world, run);
    assert_eq!(asked.len(), 1, "{asked:?}\n{}", world.dump());
    assert_eq!(asked[0]["payload"]["superseded"], json!([]), "{asked:?}");
    assert_eq!(asked[0]["payload"]["failed"][0]["node"], "svc", "{asked:?}");
    assert_eq!(
        asked[0]["payload"]["failed"][0]["branch"], FIRST,
        "{asked:?}"
    );
    as_an_older_build_left_it(&world, run);
    let class = classified(&world, FIRST);
    assert_ne!(class["class"], "superseded-with-changes", "{class}");

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
    assert_eq!(answered["mode"], "answer", "{answered}");
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

    // Refused by `onevcs`: exit 1, naming the branch and what it answered on
    // stderr, whichever form stdout is in.
    registry_readable(&world, false);
    world
        .run(&["supersessions", run, "--record"])
        .exited(1)
        .out_has(&format!("superseded svc on {FIRST}: not recorded\n"))
        .err_has(&format!(
            "onevcs refused to record {FIRST} (svc) as superseded: "
        ))
        .err_has(&format!(
            "run `onepipeline supersessions {run} --record` again"
        ));
    let refused = world.run(&["supersessions", run, "--record", "--json"]);
    refused.exited(1).err_has(&format!(
        "onevcs refused to record {FIRST} (svc) as superseded: "
    ));
    let refused = refused.json();
    assert_eq!(refused["mode"], "record", "{refused}");
    assert_eq!(refused["lineages"][0]["recorded"], json!([]), "{refused}");
    assert_eq!(
        refused["lineages"][0]["failed"][0]["node"], "svc",
        "{refused}"
    );
    assert_eq!(
        refused["lineages"][0]["failed"][0]["branch"], FIRST,
        "{refused}"
    );
    registry_readable(&world, true);

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
    // The same recording in `--json`, over the journal as an older build left it:
    // the pair is asked again, and `onevcs` holds it once.
    as_an_older_build_left_it(&world, run);
    let recorded = world
        .run(&["supersessions", run, "--record", "--json"])
        .exited(0)
        .json();
    assert_eq!(recorded["mode"], "record", "{recorded}");
    assert_eq!(
        recorded["lineages"][0]["recorded"],
        json!([{"node": "svc", "branch": FIRST}]),
        "{recorded}"
    );
    assert_eq!(recorded["lineages"][0]["failed"], json!([]), "{recorded}");
    assert_eq!(superseded(&world, run).len(), 1);

    let before = superseded(&world, run).len();
    world
        .run(&["supersessions", run, "--record"])
        .exited(0)
        .out_has(&format!("superseded svc on {FIRST}: already recorded"))
        .err_lacks("refused");
    let again = world
        .run(&["supersessions", run, "--record", "--json"])
        .exited(0)
        .json();
    for list in ["to_record", "recorded", "failed"] {
        assert_eq!(again["lineages"][0][list], json!([]), "{list}: {again}");
    }
    assert_eq!(superseded(&world, run).len(), before);
    first_is_superseded_then_retirable(&world, &repo);

    world
        .run(&["supersessions", "no-such-run"])
        .exited(crate::harness::REFUSED);
}

/// The backfill verb reads the run's journal alone: over a settled run whose
/// launch record is missing, and then unreadable, and whose result is gone too, it
/// still previews the pair, records it with `--record`, and records nothing new
/// the second time — and a run whose journal is unreadable or missing is refused.
#[cfg(unix)]
#[test]
fn the_backfill_verb_answers_and_records_from_the_journal_alone() {
    let world = World::new("retirement-backfill-journal-alone");
    let repo = world.repository("local-direct", &[]);
    let run = "journal-alone";
    first_attempt(&world, run);
    world.script("svc-2.work", "the second attempt wrote this\n");
    world.script("svc-2.fail", "1");
    retry(&world, run, Some(SECOND));
    adopted(&world, run);
    let landing = landed_by_hand(&world, &repo);
    // Settled while `onevcs` can record nothing, and then stripped of the record,
    // so the sibling holds no supersession: where an older build left the run.
    registry_readable(&world, false);
    settle_at(&world, run, &landing);
    registry_readable(&world, true);
    as_an_older_build_left_it(&world, run);
    assert_ne!(
        classified(&world, FIRST)["class"],
        "superseded-with-changes"
    );

    let launch = world.run_file(run, "launch.json");
    // llmlint: ignore-block[tests_mirror_real_usage] no verb removes or corrupts a run's
    // launch record or result — a lost or truncated file on disk is the only way a run
    // reaches this state, so the journey makes one; the verb under test is the real binary's.
    std::fs::remove_file(&launch).expect("the launch record is removed");
    std::fs::remove_file(world.run_file(run, "result.json")).expect("the result is removed");
    world
        .run(&["supersessions", run])
        .exited(0)
        .out_has(&format!("svc-2 landed {SECOND} at {landing}"))
        .out_has(&format!(
            "superseded svc on {FIRST}: to record: run again with --record"
        ));
    std::fs::write(&launch, "{ not json").expect("an unreadable launch record is written");
    // llmlint: ignore-end[tests_mirror_real_usage]
    let previewed = world
        .run(&["supersessions", run, "--json"])
        .exited(0)
        .json();
    assert_eq!(
        previewed["lineages"][0]["to_record"],
        json!([{"node": "svc", "branch": FIRST}]),
        "{previewed}"
    );
    assert!(superseded(&world, run).is_empty());

    world
        .run(&["supersessions", run, "--record"])
        .exited(0)
        .out_has(&format!("superseded svc on {FIRST}: recorded"));
    let records = superseded(&world, run);
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(
        records[0]["payload"]["superseded"],
        json!([{"node": "svc", "branch": FIRST}])
    );
    assert_eq!(records[0]["payload"]["landing"], landing);

    world
        .run(&["supersessions", run, "--record"])
        .exited(0)
        .out_has(&format!("superseded svc on {FIRST}: already recorded"));
    assert_eq!(superseded(&world, run).len(), 1);
    first_is_superseded_then_retirable(&world, &repo);

    // The journal is the one thing it reads, so a run without a readable one is
    // refused rather than answered as having nothing to record.
    let journal = world.run_file(run, "events.jsonl");
    // llmlint: ignore-block[tests_mirror_real_usage] no verb removes or corrupts a run's
    // journal — a lost or truncated file on disk is the only way a run reaches this state.
    let whole = std::fs::read_to_string(&journal).expect("the journal reads");
    std::fs::write(&journal, format!("{whole}{{ not json\n")).expect("a line is damaged");
    for args in [
        &["supersessions", run][..],
        &["supersessions", run, "--record"][..],
    ] {
        world
            .run(args)
            .exited(crate::harness::REFUSED)
            .out_lacks("superseded svc")
            .err_has("the journal holds a line this build cannot read");
    }
    assert_eq!(
        std::fs::read_to_string(&journal).expect("the journal reads"),
        format!("{whole}{{ not json\n"),
        "a refused --record wrote to the journal"
    );
    // A record whose landing is neither a commit nor a change request's URL counts
    // for nothing, though every other field of it reads: its pair is asked of
    // `onevcs` again, which records a supersession once however often it is told.
    let recorded = format!("\"landing\":\"{landing}\"");
    let forged: String = whole
        .lines()
        .map(|line| {
            if line.contains("\"kind\":\"branches-superseded\"") {
                assert!(line.contains(&recorded), "{line}");
                format!(
                    "{}\n",
                    line.replace(&recorded, "\"landing\":\"not a landing\"")
                )
            } else {
                format!("{line}\n")
            }
        })
        .collect();
    assert_ne!(forged, whole, "no branches-superseded record was forged");
    std::fs::write(&journal, &forged).expect("a landing is forged");
    let unreadable = "a branches-superseded record in this run's journal could not be read \
                      (\"not a landing\" is neither a commit's object name nor a change \
                      request's URL); what it names is asked of onevcs again";
    world
        .run(&["supersessions", run])
        .exited(0)
        .out_has(&format!(
            "superseded svc on {FIRST}: to record: run again with --record"
        ))
        .err_has(unreadable);
    world
        .run(&["supersessions", run, "--record"])
        .exited(0)
        .out_has(&format!("superseded svc on {FIRST}: recorded"))
        .err_has(unreadable);
    let records = superseded(&world, run);
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(records[1]["payload"]["landing"], landing);
    assert_eq!(
        records[1]["payload"]["superseded"],
        json!([{"node": "svc", "branch": FIRST}])
    );
    assert_eq!(classified(&world, FIRST)["class"], "retirable");
    world
        .run(&["supersessions", run, "--record"])
        .exited(0)
        .out_has(&format!("superseded svc on {FIRST}: already recorded"));
    assert_eq!(superseded(&world, run).len(), 2);
    std::fs::write(&journal, "{ not json\n").expect("an unreadable journal is written");
    for args in [
        &["supersessions", run][..],
        &["supersessions", run, "--record"][..],
    ] {
        world
            .run(args)
            .exited(crate::harness::REFUSED)
            .out_lacks("nothing to record")
            .err_has("the journal holds no record this build can read");
    }
    std::fs::remove_file(&journal).expect("the journal is removed");
    // llmlint: ignore-end[tests_mirror_real_usage]
    world
        .run(&["supersessions", run, "--record"])
        .exited(crate::harness::REFUSED)
        .err_has("events.jsonl");
    assert!(!journal.exists(), "a refused --record wrote a journal");
}

/// `--record` beside a live driver is refused, naming that driver, and writes
/// nothing: the driver records its own landings. Answering without `--record`
/// still works, and once the driver has let go `--record` does too.
#[cfg(unix)]
#[test]
fn the_backfill_verb_refuses_to_record_a_run_a_driver_is_driving() {
    let world = World::new("retirement-backfill-driven");
    world.repository("local-direct", &[]);
    let run = "driven";
    let pid = first_attempt_beside_a_hold(&world, run);
    world.script("svc-2.work", "the second attempt wrote this\n");
    world.script("svc-2.fail", "1");
    retry(&world, run, Some(SECOND));
    until_settled(&world, run, "svc-2");
    let journal = world.run_file(run, "events.jsonl");
    let before = std::fs::read_to_string(&journal).expect("the journal reads");

    world
        .run(&["supersessions", run, "--record"])
        .exited(crate::harness::REFUSED)
        .err_has(&format!("run '{run}' is being written by pid {pid}"));
    assert_eq!(
        std::fs::read_to_string(&journal).expect("the journal reads"),
        before,
        "a refused --record wrote to the journal"
    );
    world.run(&["supersessions", run]).exited(0);

    world.release("hold.go");
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    world.run(&["supersessions", run, "--record"]).exited(0);
}

const SERVICE_IDENTITY: &str = "github.com/owner/service";

/// A world whose drivers read the host as idle and sweep every second.
fn sweeping_world(name: &str) -> World {
    World::new(name)
        .with_env(onepipeline::executor::LOAD1_ENV, "0")
        .with_env(onepipeline::maintenance::PACE_ENV, "1")
}

/// A maintenance schedule: every slot due hourly, which the retirement pass
/// beside it does not wait on.
fn schedule(world: &World) -> String {
    let path = world.root.join("schedule.yml");
    std::fs::write(&path, "version: 1\ndefault:\n  every: 1h\n").expect("the schedule is written");
    path.to_string_lossy().into_owned()
}

/// One warm slot for `service`, and room past it.
fn pooled(world: &World) {
    std::fs::write(
        world.onevcs_home().join("workspaces.yml"),
        "version: 1\nrules:\n  - match: {host: github.com, owner: owner, name: service}\n    \
         pool: 1\n    overflow: unlimited\n",
    )
    .expect("the workspaces file is written");
}

/// Launch `run` detached under the schedule, its one direct node `hold` holding
/// until released, beside the nodes given.
fn sweeping(world: &World, run: &str, hold: Value, nodes: Vec<Value>) {
    world.script("hold.wait", "hold");
    let mut plan = plan_of(run, [vec![hold], nodes].concat());
    plan["concurrency"] = json!(8);
    let path = world.plan(run, &plan);
    world
        .run(&[
            "start",
            &path,
            "--detach",
            "--maintenance-config",
            &schedule(world),
        ])
        .exited(0);
}

fn retired(world: &World, run: &str) -> Vec<Value> {
    world
        .events_of(run, "branches-retired")
        .into_iter()
        .flat_map(|event| {
            event["payload"]["retired"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .collect()
}

/// `onevcs pool status service --json`, as the sibling reports its slots.
fn pool_status(world: &World) -> Value {
    let status = world
        .cmd_on(&onevcs_binary(), &["pool", "status", "service", "--json"])
        .output()
        .expect("onevcs runs");
    serde_json::from_slice(&status.stdout).expect("the status is JSON")
}

/// The trigger `onevcs` recorded retiring `branch` with, as its own `status`
/// reads back the retirement of a branch nothing holds any more.
fn recorded_trigger(world: &World, repo: &Repository, branch: &str) -> Value {
    let output = world
        .cmd_on(&onevcs_binary(), &["status", branch, "--json"])
        .current_dir(&repo.checkout)
        .output()
        .expect("onevcs runs");
    let status: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "`onevcs status {branch} --json` printed no report ({error}): {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    status["retired"]["trigger"].clone()
}

/// Wait until a run has journalled the retirement of every branch given.
fn until_retired(world: &World, run: &str, branches: &[&str]) {
    world.until(&format!("{run} to retire {branches:?}"), |world| {
        let retired = retired(world, run);
        branches
            .iter()
            .all(|branch| retired.iter().any(|entry| entry["branch"] == *branch))
    });
}

fn holds(world: &World, repo: &Path, branch: &str) -> bool {
    !git(world, repo, &["branch", "--list", branch])
        .trim()
        .is_empty()
}

/// A branch holding no work beyond its base: its one commit adds `file`, and the
/// base carries `file` byte for byte. Unpublished — in the registered checkout
/// alone, as a branch a person cut there and never pushed is. Answers its tip.
fn lossless(world: &World, repo: &Repository, branch: &str, file: &str) -> String {
    let tip = unique(world, repo, branch, file);
    let clone = person(world, repo);
    let carried = git(world, &clone, &["show", &format!("{branch}:{file}")]);
    std::fs::write(clone.join(file), carried).expect("the file is written");
    git(world, &clone, &["add", "-A"]);
    git(
        world,
        &clone,
        &["commit", "-m", &format!("chore: carry {file}")],
    );
    git(world, &clone, &["push", "origin", "main"]);
    synced(world, repo);
    tip
}

/// A branch whose one commit adds `file`, which the base does not carry: work
/// beyond its base. Unpublished, in the registered checkout alone. Answers its tip.
fn unique(world: &World, repo: &Repository, branch: &str, file: &str) -> String {
    let clone = person(world, repo);
    git(world, &clone, &["checkout", "-b", branch]);
    std::fs::write(clone.join(file), format!("{branch} carried this\n")).expect("written");
    git(world, &clone, &["add", "-A"]);
    git(
        world,
        &clone,
        &["commit", "-m", &format!("chore: work on {branch}")],
    );
    let tip = git(world, &clone, &["rev-parse", "HEAD"]).trim().to_owned();
    git(world, &clone, &["checkout", "main"]);
    git(
        world,
        &repo.checkout,
        &[
            "fetch",
            &clone.to_string_lossy(),
            &format!("{branch}:{branch}"),
        ],
    );
    tip
}

/// An idle pass retires lossless branches other runs left — out of the registered
/// checkout, the run clone or slot clone that held each, and the origin — journals
/// `branches-retired` saying what proved each, and leaves the slot itself warm.
///
/// Each branch is one an earlier run published as a change request and settled
/// on, open; a person merged both requests afterwards, once their sessions had
/// closed. So nothing retired them at their close, and they are what an idle pass
/// is for: lossless, and left behind.
#[test]
fn an_idle_pass_retires_what_other_runs_left_and_keeps_the_slot_that_held_one() {
    let world = sweeping_world("retirement-pass");
    let repo = world.repository("change-open", &[]);
    pooled(&world);

    // Two runs before this one: the first works in the one warm slot, the second
    // is placed fresh under `runs/` by its node's own `pool: 0`.
    world.script("slotted.work", "the slotted run wrote this\n");
    let mut slotted = lifecycle("slotted", &[]);
    slotted["branch"] = json!("done/slotted");
    world.script("overflowed.work", "the overflowed run wrote this\n");
    let mut overflowed = lifecycle("overflowed", &[]);
    overflowed["branch"] = json!("done/overflowed");
    overflowed["pool"] = json!(0);
    for (run, node) in [("slotted", slotted), ("overflowed", overflowed)] {
        let path = world.plan(run, &plan_of(run, vec![node]));
        world.run(&["start", &path, "--attach"]).settled();
        let settled = settlement(&world, run, run);
        assert_eq!(
            settled["outcome"],
            "change-open",
            "{settled}\n{}",
            world.dump()
        );
    }
    world.script("gh.merged", "");
    for run in ["slotted", "overflowed"] {
        let url = settlement(&world, run, run)["change_url"]
            .as_str()
            .expect("the settlement names its change request")
            .to_owned();
        let number = url.rsplit('/').next().expect("a number").to_owned();
        let merged = world
            .cmd_on(
                &crate::harness::double("fake-gh"),
                &[
                    "pr",
                    "merge",
                    &number,
                    "--repo",
                    "owner/service",
                    "--squash",
                ],
            )
            .output()
            .expect("gh runs");
        assert!(
            merged.status.success(),
            "{}",
            String::from_utf8_lossy(&merged.stderr)
        );
    }

    let mut places: Vec<(String, Value)> = Vec::new();
    for branch in ["done/slotted", "done/overflowed"] {
        let class = classified(&world, branch);
        assert_eq!(class["class"], "retirable", "{class}");
        let holders = class["holders"].as_array().expect("holders").clone();
        for kind in ["checkout", "origin"] {
            assert!(
                holders.iter().any(|holder| holder["kind"] == kind),
                "{branch} is not in a {kind}: {class}"
            );
        }
        for holder in holders {
            places.push((branch.to_owned(), holder));
        }
    }
    assert!(
        places
            .iter()
            .any(|(of, holder)| of == "done/overflowed" && holder["kind"] == "run-clone"),
        "{places:?}"
    );
    // The slot the first run worked in: a returned slot is reset onto the base, so
    // it is the session's record rather than a ref that says the branch was there.
    let slot = PathBuf::from(
        pool_status(&world)["slots"][0]["path"]
            .as_str()
            .expect("the pool reports its slot"),
    );
    let worked_in = world
        .events_of("slotted", "session-opened")
        .into_iter()
        .find_map(|event| event["payload"]["clone"].as_str().map(PathBuf::from))
        .expect("the sibling named the clone the slotted run worked in");
    assert!(
        worked_in.starts_with(&slot),
        "{} is not in the slot {}",
        worked_in.display(),
        slot.display()
    );

    sweeping(
        &world,
        "sweeper",
        crate::harness::agent("hold", &[]),
        Vec::new(),
    );
    until_retired(&world, "sweeper", &["done/slotted", "done/overflowed"]);

    for entry in retired(&world, "sweeper") {
        assert_eq!(entry["identity"], SERVICE_IDENTITY, "{entry}");
        assert_eq!(entry["class"], "retirable", "{entry}");
        assert_eq!(entry["trigger"], "pass", "{entry}");
        assert_eq!(entry["proof"]["kind"], "merged-change-request", "{entry}");
        // The trigger journalled is the one `onevcs` recorded, which keeps its
        // vocabulary to itself: this is where the two spellings meet.
        let branch = entry["branch"].as_str().expect("a branch");
        assert_eq!(
            recorded_trigger(&world, &repo, branch),
            entry["trigger"],
            "{entry}"
        );
    }
    for (branch, holder) in &places {
        let location = holder["location"].as_str().expect("a location");
        let place = match holder["kind"].as_str() {
            Some("origin") => repo.origin.clone(),
            _ => PathBuf::from(location),
        };
        assert!(
            !place.is_dir() || !holds(&world, &place, branch),
            "{branch} is still in {holder}"
        );
    }
    // The slot that held one is still a slot: its directory is there and the pool
    // still reports it, at the same place.
    assert!(slot.is_dir(), "{} was removed", slot.display());
    let status = pool_status(&world);
    assert_eq!(
        status["slots"].as_array().map(Vec::len),
        Some(1),
        "{status}"
    );
    assert_eq!(
        status["slots"][0]["path"],
        json!(slot.to_string_lossy()),
        "{status}"
    );

    world.release("hold.go");
    world.until("the sweeper to settle", |world| {
        world.run_file("sweeper", "result.json").is_file()
    });
}

/// The control every exclusion case sets beside the branch it is about: a branch
/// another run left.
const LEFT: &str = "other/finished";

/// Run `other` to its end: its one lifecycle node, pinned to [`LEFT`], writes
/// `other.md` and fails its task, so `onevcs` preserves the branch. The base does
/// not carry `other.md` yet, so the branch holds work and no pass retires it until
/// [`control_made_lossless`].
fn left_by_another_run(world: &World) {
    world.script("other.work", "the other run wrote this\n");
    world.script("other.fail", "1");
    let mut node = lifecycle("other", &[]);
    node["branch"] = json!(LEFT);
    let path = world.plan("other", &plan_of("other", vec![node]));
    world.run(&["start", &path, "--attach"]).settled();
    assert_eq!(
        settlement(world, "other", "other")["status"],
        "failed",
        "{}",
        world.dump()
    );
    assert_eq!(classified(world, LEFT)["reason"], "unmerged-unique-commits");
}

/// Put `other.md` on the base exactly as [`LEFT`] carries it, which makes the
/// control lossless — so the pass that retires it is one that ran after this.
fn control_made_lossless(world: &World, repo: &Repository) {
    let carried = git(
        world,
        &repo.checkout,
        &["show", &format!("{LEFT}:other.md")],
    );
    on_the_base(world, repo, "other.md", &carried);
}

/// A world for one exclusion case: an idle, sweeping host with one warm slot for
/// `service`, and the control [`left_by_another_run`]. A cancelled dispatch is
/// reaped after a second, for the cases that park a running node.
fn excluding(name: &str) -> (World, Repository) {
    let world = sweeping_world(name).with_env(crate::harness::CANCEL_GRACE_ENV, "1");
    let repo = world.repository("local-direct", &[]);
    pooled(&world);
    left_by_another_run(&world);
    (world, repo)
}

/// `named` cut before the run starts, lossless: its one commit adds `named.md`,
/// which the base carries byte for byte, so `onevcs` answers it `retirable`.
fn named_lossless(world: &World, repo: &Repository, named: &str) -> String {
    lossless(world, repo, named, "named.md");
    let class = classified(world, named);
    assert_eq!(
        class["class"], "retirable",
        "{named} is not lossless: {class}"
    );
    named.to_owned()
}

/// What `onepipeline results live` says `node` is.
fn state_of(world: &World, node: &str) -> String {
    let results = world.run(&["results", "live"]).exited(0).stdout.clone();
    results
        .lines()
        .find_map(|line| {
            let mut words = line.split_whitespace();
            (words.next() == Some(node)).then(|| words.next().unwrap_or_default().to_owned())
        })
        .unwrap_or_else(|| panic!("results names no {node}:\n{results}"))
}

/// How many `node-dispatched` and `node-settled` records `live` holds.
fn moves(world: &World) -> (usize, usize) {
    (
        world.events_of("live", "node-dispatched").len(),
        world.events_of("live", "node-settled").len(),
    )
}

/// One exclusion case. Launch `live` — its direct node `hold` holding until
/// released, beside `nodes` — and let `reach` bring `node` into `state` and answer
/// the branch it then names, lossless by then. Only after that is the control
/// made lossless, so the pass that retires it runs while `node` is in `state`: no
/// node of `live` is dispatched or settles between the two readings, and
/// `results` reads `node` in `state` after the pass. That same pass retired the
/// control and nothing else, and the named branch is still there. Once `live` is
/// stopped and nothing names it, the next run's pass retires it, which shows the
/// name was all that kept it.
fn kept_by_the_pass(
    world: &World,
    repo: &Repository,
    (hold, nodes): (Value, Vec<Value>),
    (node, state): (&str, &str),
    reach: impl FnOnce(&World, &Repository) -> String,
) {
    sweeping(world, "live", hold, nodes);
    let named = reach(world, repo);
    assert_eq!(state_of(world, node), state, "{}", world.dump());
    let class = classified(world, &named);
    assert_eq!(
        class["class"], "retirable",
        "{named} is not lossless: {class}"
    );
    let reached = moves(world);
    let early = retired(world, "live");
    assert!(
        early.iter().all(|entry| entry["branch"] != named.as_str()),
        "{named} was retired while {node}, {state}, named it: {early:?}"
    );
    assert!(early.is_empty(), "{early:?}\n{}", world.dump());
    control_made_lossless(world, repo);
    until_retired(world, "live", &[LEFT]);

    let statuses = world.run(&["status", "live"]).exited(0).stdout.clone();
    assert!(
        !holds(world, &repo.checkout, LEFT),
        "the control was journalled retired and is still there"
    );
    assert!(
        holds(world, &repo.checkout, &named),
        "{named} was retired while {node}, {state}, named it\n{statuses}"
    );
    assert!(
        retired(world, "live")
            .iter()
            .all(|entry| entry["branch"] == LEFT),
        "beside {LEFT}, the pass retired {:?}",
        retired(world, "live")
    );
    assert_eq!(
        moves(world),
        reached,
        "a node was dispatched or settled while the pass ran\n{statuses}"
    );
    assert_eq!(state_of(world, node), state, "{statuses}");

    world.run(&["stop", "live", "--force"]).exited(0);
    sweeping(
        world,
        "after",
        crate::harness::agent("hold", &[]),
        Vec::new(),
    );
    until_retired(world, "after", &[named.as_str()]);
    world.release("hold.go");
    world.until("the after run to settle", |world| {
        world.run_file("after", "result.json").is_file()
    });
}

/// Wait until `live`'s node `id` has been dispatched.
fn until_dispatched(world: &World, id: &str) {
    world.until(&format!("{id} to be dispatched"), |world| {
        world
            .events_of("live", "node-dispatched")
            .iter()
            .any(|event| event["labels"]["node"] == id)
    });
}

/// Wait until `live` holds `id` ready for want of a workspace.
fn until_held_for_a_workspace(world: &World, id: &str) {
    world.until(&format!("{id} to be held for its workspace"), |world| {
        world.events_of("live", "node-held").iter().any(|event| {
            event["labels"]["node"] == id
                && event["payload"]["reasons"]
                    .as_array()
                    .is_some_and(|reasons| reasons.iter().any(|r| r["kind"] == "workspace"))
        })
    });
}

fn reply(world: &World, commands: Value) {
    world
        .run_with_stdin(
            &["reply", "live"],
            &json!({"version": 2, "commands": commands}).to_string(),
        )
        .exited(0);
}

/// A lifecycle node asking for the identity's one warm slot and no overflow, so
/// the engine holds it ready while another node sits in that slot rather than
/// dispatching it into a refusal that would settle it.
fn slot_bound(id: &str, deps: &[&str]) -> Value {
    let mut node = lifecycle(id, deps);
    node["pool"] = json!(1);
    node["overflow"] = json!(0);
    node
}

/// Script `id`'s worker to hold its turn open until the case parks it, and then
/// to stop, committing what the interruption asked of it.
fn parkable(world: &World, id: &str) {
    world.script(&format!("{id}.wait"), id);
    world.script(&format!("{id}.turn-open"), "");
    world.script(&format!("{id}.stops-when-interrupted"), "");
}

/// Park `id` while it runs: its dispatch stops, `onevcs` preserves what it
/// committed on its session's branch and closes the session, and the node settles
/// `cancelled` under its park — so the engine reads it `parked`, still naming
/// that session as its current one. Every path the branch differs from the base
/// in is then carried onto the base, so the branch is lossless and, its session
/// closed, nothing in `onevcs` holds it. Answers the branch.
fn parked_off_its_session(world: &World, repo: &Repository, id: &str) -> String {
    world.until(&format!("{id}'s session to open"), |world| {
        world
            .events_of("live", "session-opened")
            .iter()
            .any(|event| event["labels"]["node"] == id)
    });
    reply(world, json!([{"op": "cancel", "id": id}]));
    world.until(&format!("{id} to settle under its park"), |world| {
        world
            .events_of("live", "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == id)
    });
    let branch = world
        .events_of("live", "session-closed")
        .into_iter()
        .find(|event| event["labels"]["node"] == id)
        .and_then(|event| event["payload"]["branch"].as_str().map(str::to_owned))
        .unwrap_or_else(|| panic!("{id}'s session never closed\n{}", world.dump()));
    let class = classified(world, &branch);
    for path in class["differing_paths"].as_array().expect("paths") {
        let path = path.as_str().expect("a path");
        let carried = git(
            world,
            &repo.checkout,
            &["show", &format!("{branch}:{path}")],
        );
        on_the_base(world, repo, path, &carried);
    }
    branch
}

/// Pending: a node waiting on the held one, pinned to the branch.
#[test]
fn an_idle_pass_keeps_the_branch_a_pending_node_pins() {
    let (world, repo) = excluding("retirement-pending-pin");
    let mut pending = crate::harness::agent("pending", &["hold"]);
    pending["branch"] = json!("keep/pending");
    kept_by_the_pass(
        &world,
        &repo,
        (crate::harness::agent("hold", &[]), vec![pending]),
        ("pending", "pending"),
        |world, repo| {
            until_dispatched(world, "hold");
            named_lossless(world, repo, "keep/pending")
        },
    );
}

/// Held: a parked lifecycle node pinned to the branch.
#[test]
fn an_idle_pass_keeps_the_branch_a_held_node_pins() {
    let (world, repo) = excluding("retirement-held-pin");
    let mut parked = lifecycle("parked", &[]);
    parked["parked"] = json!(true);
    parked["branch"] = json!("keep/held");
    kept_by_the_pass(
        &world,
        &repo,
        (crate::harness::agent("hold", &[]), vec![parked]),
        ("parked", "parked"),
        |world, repo| {
            until_dispatched(world, "hold");
            named_lossless(world, repo, "keep/held")
        },
    );
}

/// Ready: a slot-bound lifecycle node pinned to the branch, held ready while a
/// working node sits in the one slot.
#[test]
fn an_idle_pass_keeps_the_branch_a_ready_node_pins() {
    let (world, repo) = excluding("retirement-ready-pin");
    world.script("working.wait", "hold");
    let mut ready = slot_bound("ready", &[]);
    ready["branch"] = json!("keep/ready");
    kept_by_the_pass(
        &world,
        &repo,
        (
            crate::harness::agent("hold", &[]),
            vec![slot_bound("working", &[]), ready],
        ),
        ("ready", "ready"),
        |world, repo| {
            until_held_for_a_workspace(world, "ready");
            named_lossless(world, repo, "keep/ready")
        },
    );
}

/// Running: the held direct node itself, pinned to the branch.
#[test]
fn an_idle_pass_keeps_the_branch_a_running_node_pins() {
    let (world, repo) = excluding("retirement-running-pin");
    let mut hold = crate::harness::agent("hold", &[]);
    hold["branch"] = json!("keep/running");
    kept_by_the_pass(
        &world,
        &repo,
        (hold, Vec::new()),
        ("hold", "running"),
        |world, repo| {
            until_dispatched(world, "hold");
            named_lossless(world, repo, "keep/running")
        },
    );
}

/// Pending: a lifecycle node waiting on the held one, set to continue the branch.
#[test]
fn an_idle_pass_keeps_the_branch_a_pending_node_resumes() {
    let (world, repo) = excluding("retirement-pending-resume");
    let mut resuming = lifecycle("resuming", &["hold"]);
    resuming["resume"] = json!({"branch": "keep/resumed"});
    kept_by_the_pass(
        &world,
        &repo,
        (crate::harness::agent("hold", &[]), vec![resuming]),
        ("resuming", "pending"),
        |world, repo| {
            until_dispatched(world, "hold");
            named_lossless(world, repo, "keep/resumed")
        },
    );
}

/// Held: a parked lifecycle node set to continue the branch.
#[test]
fn an_idle_pass_keeps_the_branch_a_held_node_resumes() {
    let (world, repo) = excluding("retirement-held-resume");
    let mut parked = lifecycle("parked", &[]);
    parked["parked"] = json!(true);
    parked["resume"] = json!({"branch": "keep/resumed"});
    kept_by_the_pass(
        &world,
        &repo,
        (crate::harness::agent("hold", &[]), vec![parked]),
        ("parked", "parked"),
        |world, repo| {
            until_dispatched(world, "hold");
            named_lossless(world, repo, "keep/resumed")
        },
    );
}

/// Ready: a slot-bound lifecycle node set to continue the branch, held ready
/// while a working node sits in the one slot.
#[test]
fn an_idle_pass_keeps_the_branch_a_ready_node_resumes() {
    let (world, repo) = excluding("retirement-ready-resume");
    world.script("working.wait", "hold");
    let mut ready = slot_bound("ready", &[]);
    ready["resume"] = json!({"branch": "keep/resumed"});
    kept_by_the_pass(
        &world,
        &repo,
        (
            crate::harness::agent("hold", &[]),
            vec![slot_bound("working", &[]), ready],
        ),
        ("ready", "ready"),
        |world, repo| {
            until_held_for_a_workspace(world, "ready");
            named_lossless(world, repo, "keep/resumed")
        },
    );
}

/// Running: the held direct node itself, set to continue the branch. A direct
/// node opens no session, so nothing in `onevcs` holds the branch for it.
#[test]
fn an_idle_pass_keeps_the_branch_a_running_node_resumes() {
    let (world, repo) = excluding("retirement-running-resume");
    let mut hold = crate::harness::agent("hold", &[]);
    hold["resume"] = json!({"branch": "keep/resumed"});
    kept_by_the_pass(
        &world,
        &repo,
        (hold, Vec::new()),
        ("hold", "running"),
        |world, repo| {
            until_dispatched(world, "hold");
            named_lossless(world, repo, "keep/resumed")
        },
    );
}

// By a node's current dispatch session's branch, in each live state. A pending,
// held or ready node names one only after a dispatch that has since ended, so
// each of those cases parks a running node off its session — which closes it, so
// `onevcs` holds nothing for the branch. Parking also pins the node's `branch` and
// `resume.branch` to the one its session preserved, so each case requeues it
// amended onto [`ELSEWHERE`] in both: the session is then the only thing naming
// the branch.

/// Where each session case re-pins its node, so its `branch` names another one.
const ELSEWHERE: &str = "x/elsewhere";

fn requeued_elsewhere() -> Value {
    json!({"op": "requeue", "id": "x", "amend": {
        "branch": ELSEWHERE,
        "resume": {"branch": ELSEWHERE},
    }})
}

/// Pending: parked off its session, then requeued elsewhere and reparented onto
/// the held node in one reply, so it waits on it again naming that session.
#[test]
fn an_idle_pass_keeps_the_branch_a_pending_nodes_last_session_is_on() {
    let (world, repo) = excluding("retirement-pending-session");
    parkable(&world, "x");
    kept_by_the_pass(
        &world,
        &repo,
        (
            crate::harness::agent("hold", &[]),
            vec![lifecycle("x", &[])],
        ),
        ("x", "pending"),
        |world, repo| {
            let branch = parked_off_its_session(world, repo, "x");
            reply(
                world,
                json!([
                    requeued_elsewhere(),
                    {"op": "reparent", "id": "x", "deps": ["hold"]},
                ]),
            );
            world.until("x to wait on the held node", |world| {
                state_of(world, "x") == "pending"
            });
            branch
        },
    );
}

/// Held: parked off its session, then requeued elsewhere and parked again in one
/// reply, before anything could dispatch it.
#[test]
fn an_idle_pass_keeps_the_branch_a_held_nodes_last_session_is_on() {
    let (world, repo) = excluding("retirement-held-session");
    parkable(&world, "x");
    kept_by_the_pass(
        &world,
        &repo,
        (
            crate::harness::agent("hold", &[]),
            vec![lifecycle("x", &[])],
        ),
        ("x", "parked"),
        |world, repo| {
            let branch = parked_off_its_session(world, repo, "x");
            reply(
                world,
                json!([requeued_elsewhere(), {"op": "cancel", "id": "x"}]),
            );
            world.until("x to be parked elsewhere", |world| {
                world
                    .events_of("live", "edit-committed")
                    .iter()
                    .filter(|event| event["payload"]["command"]["op"] == "cancel")
                    .count()
                    == 2
            });
            branch
        },
    );
}

/// Ready: a slot-bound node parked off its session, then requeued elsewhere once
/// another slot-bound node — held back behind `gate` until then — has taken the
/// slot.
#[test]
fn an_idle_pass_keeps_the_branch_a_ready_nodes_last_session_is_on() {
    let (world, repo) = excluding("retirement-ready-session");
    parkable(&world, "x");
    world.script("gate.wait", "gate");
    world.script("y.wait", "y");
    kept_by_the_pass(
        &world,
        &repo,
        (
            crate::harness::agent("hold", &[]),
            vec![
                slot_bound("x", &[]),
                crate::harness::agent("gate", &[]),
                slot_bound("y", &["gate"]),
            ],
        ),
        ("x", "ready"),
        |world, repo| {
            let branch = parked_off_its_session(world, repo, "x");
            world.release("gate.go");
            world.until("y's session to open in the slot", |world| {
                world
                    .events_of("live", "session-opened")
                    .iter()
                    .any(|event| event["labels"]["node"] == "y")
            });
            reply(world, json!([requeued_elsewhere()]));
            until_held_for_a_workspace(world, "x");
            branch
        },
    );
}

/// Running: a running lifecycle node's session, which `onevcs` cut fresh from the
/// base, so the branch is content-identical to it. That session is open under a
/// live owner, which `onevcs` keeps a branch for on that alone, so this case
/// shows the branch survives beside a retired control and cannot tell the
/// engine's exclusion from that hold; `maintenance`'s unit test
/// `a_running_node_names_its_open_sessions_branch` holds that the engine names it.
#[test]
fn an_idle_pass_keeps_the_branch_a_running_nodes_session_is_on() {
    let (world, repo) = excluding("retirement-running-session");
    world.script("working.wait", "hold");
    sweeping(
        &world,
        "live",
        crate::harness::agent("hold", &[]),
        vec![lifecycle("working", &[])],
    );
    world.until("the working node's session to open", |world| {
        world
            .events_of("live", "session-opened")
            .iter()
            .any(|event| event["labels"]["node"] == "working")
    });
    let session = world
        .events_of("live", "session-opened")
        .into_iter()
        .find(|event| event["labels"]["node"] == "working")
        .and_then(|event| event["payload"]["branch"].as_str().map(str::to_owned))
        .expect("the working node's session names its branch");
    assert_eq!(state_of(&world, "working"), "running");
    let reached = moves(&world);
    control_made_lossless(&world, &repo);
    until_retired(&world, "live", &[LEFT]);
    assert!(
        retired(&world, "live")
            .iter()
            .all(|entry| entry["branch"] == LEFT),
        "beside {LEFT}, the pass retired {:?} (the working session is on {session})",
        retired(&world, "live")
    );
    // The session's branch is in its slot's clone rather than the registered
    // checkout, so it is read where `onevcs` says it is held.
    let class = classified(&world, &session);
    assert_ne!(class["class"], "retirable", "{session}: {class}");
    assert!(
        class["holders"]
            .as_array()
            .is_some_and(|holders| !holders.is_empty()),
        "{session} was retired: {class}"
    );
    assert_eq!(moves(&world), reached, "{}", world.dump());
    world.run(&["stop", "live", "--force"]).exited(0);
}

/// An idle pass leaves a branch holding an unmerged unique commit and a
/// `superseded-with-changes` one, neither named by any node, while the same pass
/// retires the control another run left.
#[test]
fn an_idle_pass_leaves_what_holds_work() {
    let world = sweeping_world("retirement-work");
    let repo = world.repository("local-direct", &[]);
    pooled(&world);
    left_by_another_run(&world);
    lossless(&world, &repo, "other/superseder", "superseder.md");
    unique(&world, &repo, "keep/unmerged", "unmerged.md");
    unique(&world, &repo, "keep/superseded", "superseded.md");
    let base = git(&world, &repo.origin, &["rev-parse", "main"])
        .trim()
        .to_owned();
    let supersede = world
        .cmd_on(
            &onevcs_binary(),
            &[
                "supersede",
                "keep/superseded",
                "--repo",
                "service",
                "--by",
                "other/superseder",
                "--landing",
                &base,
            ],
        )
        .output()
        .expect("onevcs runs");
    assert!(
        supersede.status.success(),
        "{}",
        String::from_utf8_lossy(&supersede.stderr)
    );
    assert_eq!(
        classified(&world, "keep/unmerged")["reason"],
        "unmerged-unique-commits"
    );
    assert_eq!(
        classified(&world, "keep/superseded")["class"],
        "superseded-with-changes"
    );
    control_made_lossless(&world, &repo);

    sweeping(
        &world,
        "live",
        crate::harness::agent("hold", &[]),
        Vec::new(),
    );
    until_retired(&world, "live", &[LEFT]);
    for branch in ["keep/unmerged", "keep/superseded"] {
        assert!(
            holds(&world, &repo.checkout, branch),
            "{branch} was retired"
        );
    }
    assert!(
        retired(&world, "live")
            .iter()
            .all(|entry| entry["branch"] != "keep/unmerged" && entry["branch"] != "keep/superseded"),
        "{:?}",
        retired(&world, "live")
    );
    world.release("hold.go");
    world.until("the live run to settle", |world| {
        world.run_file("live", "result.json").is_file()
    });
}

/// A branch a node comes to name while a sweep is already running — added to the
/// run while the sweep's maintenance is held — is left alone by that sweep's
/// retirement pass, which retires the control another run left beside it: the
/// pass reads what the run names as it reaches an identity, not what it named
/// when the sweep began.
#[cfg(unix)]
#[test]
fn a_branch_a_node_comes_to_name_mid_sweep_is_left_by_that_sweeps_pass() {
    let world = sweeping_world("retirement-mid-sweep");
    let repo = world.repository("local-direct", &[]);
    let maintaining = world.root.join("maintain.go");
    crate::maintenance::pooled_with_maintenance(&world, Some(&maintaining));
    crate::maintenance::cut_a_slot(&world, &repo.checkout);
    left_by_another_run(&world);
    lossless(&world, &repo, "keep/late", "late.md");
    assert_eq!(classified(&world, "keep/late")["class"], "retirable");
    control_made_lossless(&world, &repo);

    sweeping(
        &world,
        "live",
        crate::harness::agent("hold", &[]),
        Vec::new(),
    );
    world.until("the sibling to report the slot maintaining", |world| {
        pool_status(world)["slots"][0]["state"]["state"] == "maintaining"
    });
    let mut late = lifecycle("late", &["hold"]);
    late["branch"] = json!("keep/late");
    world
        .run_with_stdin(
            &["reply", "live"],
            &json!({"version": 2, "commands": [{"op": "add", "node": late}]}).to_string(),
        )
        .exited(0);
    world.until("the driver to take the late node up", |world| {
        world
            .journal("live")
            .iter()
            .any(|event| event["labels"]["node"] == "late")
    });
    assert!(retired(&world, "live").is_empty(), "{}", world.dump());

    std::fs::write(&maintaining, "go").expect("the maintenance is released");
    until_retired(&world, "live", &[LEFT]);
    assert!(
        holds(&world, &repo.checkout, "keep/late"),
        "keep/late was retired by a sweep that began before a node named it"
    );
    assert!(
        retired(&world, "live")
            .iter()
            .all(|entry| entry["branch"] == LEFT),
        "{:?}",
        retired(&world, "live")
    );
    world.run(&["stop", "live", "--force"]).exited(0);
}

/// A pass whose `retire_finished` fails is journalled with the error, and the run
/// goes on scheduling: its nodes settle exactly as the same plan's do on a run
/// that runs no pass at all.
#[test]
fn a_pass_that_fails_is_journalled_and_the_run_settles_as_it_would_have() {
    let world = sweeping_world("retirement-failed");
    world.repository("local-direct", &[]);
    let sessions = world.onevcs_home().join("sessions");
    std::fs::create_dir_all(&sessions).expect("the sessions directory exists");

    world.script("hold.go", "go");
    let path = world.plan(
        "plain",
        &plan_of(
            "plain",
            vec![
                crate::harness::agent("hold", &[]),
                crate::harness::agent("after", &["hold"]),
            ],
        ),
    );
    world.run(&["start", &path, "--attach"]).settled();
    let plain = world.run_json("plain", "result.json");
    world.unscript("hold.go");

    // llmlint: ignore-block[tests_mirror_real_usage] no interface writes a session
    // record the sibling cannot read — every verb of its writes a valid one — so the
    // record is written onto the state root directly: it is what a truncated write or a
    // lost mount hands the linked library, and a pass over it is refused rather than
    // read as an empty host, which is the failure this journey needs.
    std::fs::write(sessions.join("s-unreadable.json"), "{ not a session record")
        .expect("the unreadable record is written");
    // llmlint: ignore-end[tests_mirror_real_usage]
    sweeping(
        &world,
        "failing",
        crate::harness::agent("hold", &[]),
        vec![crate::harness::agent("after", &["hold"])],
    );
    world.until("the failed pass to be journalled", |world| {
        !world.events_of("failing", "branches-retired").is_empty()
    });
    let record = world.events_of("failing", "branches-retired")[0]["payload"].clone();
    assert_eq!(record["retired"], json!([]), "{record}");
    assert_eq!(
        record["failed"][0]["identity"], SERVICE_IDENTITY,
        "{record}"
    );
    assert!(record["failed"][0].get("branch").is_none(), "{record}");
    assert!(
        record["failed"][0]["error"]
            .as_str()
            .is_some_and(|error| !error.is_empty()),
        "{record}"
    );

    world.release("hold.go");
    world.until("the failing run to settle", |world| {
        world.run_file("failing", "result.json").is_file()
    });
    std::fs::remove_file(sessions.join("s-unreadable.json")).expect("the record goes");
    let failing = world.run_json("failing", "result.json");
    for (ran, alone) in failing["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .zip(plain["nodes"].as_array().expect("nodes"))
    {
        for field in ["id", "status", "outcome"] {
            assert_eq!(ran[field], alone[field], "{field}: {ran} vs {alone}");
        }
    }
    assert_eq!(failing["state"], plain["state"], "{failing} vs {plain}");
}

/// A node pinned to a branch a pass has retired still dispatches: `onevcs` cuts
/// the name fresh from the base, and nothing of what was retired comes back.
#[test]
fn a_node_pinned_to_a_retired_branch_is_cut_fresh_from_the_base() {
    let world = sweeping_world("retirement-pinned");
    let repo = world.repository("local-direct", &[]);
    let retired_tip = lossless(&world, &repo, "pinned/work", "pinned.md");

    sweeping(
        &world,
        "sweeper",
        crate::harness::agent("hold", &[]),
        Vec::new(),
    );
    until_retired(&world, "sweeper", &["pinned/work"]);
    world.release("hold.go");
    world.until("the sweeper to settle", |world| {
        world.run_file("sweeper", "result.json").is_file()
    });
    assert!(!holds(&world, &repo.checkout, "pinned/work"));
    assert!(!holds(&world, &repo.origin, "pinned/work"));
    let base = git(&world, &repo.origin, &["rev-parse", "main"])
        .trim()
        .to_owned();

    world.script("again.work", "the pinned node wrote this\n");
    let mut again = lifecycle("again", &[]);
    again["branch"] = json!("pinned/work");
    let path = world.plan("pinned", &plan_of("pinned", vec![again]));
    world.run(&["start", &path, "--attach"]).settled();
    let settled = settlement(&world, "pinned", "again");
    assert_eq!(settled["status"], "done", "{settled}\n{}", world.dump());
    assert_eq!(settled["outcome"], "merged", "{settled}");
    assert_eq!(settled["branch"], "pinned/work", "{settled}");

    // Cut from the base: the landed commit's parent is where the base stood, and
    // the retired tip is nowhere in what landed.
    let landed = git(&world, &repo.origin, &["rev-parse", "main"])
        .trim()
        .to_owned();
    assert_eq!(
        git(&world, &repo.origin, &["rev-parse", &format!("{landed}^")]).trim(),
        base
    );
    let ancestry = std::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", &retired_tip, "main"])
        .current_dir(&repo.origin)
        .status()
        .expect("git runs");
    assert!(
        !ancestry.success(),
        "the retired tip {retired_tip} came back onto the base"
    );
    assert_eq!(
        repo.base_file("again.md").as_deref().map(str::trim),
        Some("the pinned node wrote this")
    );
}

/// Cut `branch` lossless, push it to the origin, and record it superseded by
/// `main`: a pass examines a branch an origin ref holds only where a record of
/// the host's names it, so this is one a pass retires by pushing a deletion.
#[cfg(unix)]
fn published_and_superseded(world: &World, repo: &Repository, branch: &str, file: &str) {
    lossless(world, repo, branch, file);
    git(world, &repo.checkout, &["push", "origin", branch]);
    let base = git(world, &repo.origin, &["rev-parse", "main"])
        .trim()
        .to_owned();
    let supersede = world
        .cmd_on(
            &onevcs_binary(),
            &[
                "supersede",
                branch,
                "--repo",
                "service",
                "--by",
                "main",
                "--landing",
                &base,
            ],
        )
        .output()
        .expect("onevcs runs");
    assert!(
        supersede.status.success(),
        "{}",
        String::from_utf8_lossy(&supersede.stderr)
    );
}

#[cfg(unix)]
fn origin_hook(repo: &Repository, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    let hook = repo.origin.join("hooks").join("pre-receive");
    std::fs::write(&hook, script).expect("the hook is written");
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700))
        .expect("the hook is executable");
}

/// Hold every push the origin receives until the journey writes `go`, having
/// written `entered` on arrival: a retirement pass deleting a published branch
/// held mid-pass. Answers the two paths.
#[cfg(unix)]
fn origin_held(world: &World, repo: &Repository) -> (PathBuf, PathBuf) {
    let entered = world.root.join("origin.entered");
    let go = world.root.join("origin.go");
    origin_hook(
        repo,
        &format!(
            "#!/bin/sh\ncat >/dev/null\ntouch '{}'\nn=0\nwhile [ ! -f '{}' ] && [ $n -lt 6000 ]; do \
             sleep 0.05; n=$((n+1)); done\n",
            entered.display(),
            go.display()
        ),
    );
    (entered, go)
}

/// A branch whose origin refuses its deletion is retired in part: the pass journals
/// a failure naming the identity, the branch and what the origin answered, the
/// copy the origin kept stays, and the run settles as it would have.
#[cfg(unix)]
#[test]
fn a_deletion_the_origin_refuses_is_journalled_against_its_branch() {
    let world = sweeping_world("retirement-incomplete");
    let repo = world.repository("local-direct", &[]);
    published_and_superseded(&world, &repo, "done/published", "published.md");
    origin_hook(
        &repo,
        "#!/bin/sh\nwhile read old new ref; do\n  case \"$new\" in\n    *[!0]*) ;;\n    \
         *) echo \"deleting $ref is not allowed here\" >&2; exit 1 ;;\n  esac\ndone\n",
    );
    let class = classified(&world, "done/published");
    assert_eq!(class["class"], "retirable", "{class}");
    assert!(
        class["holders"]
            .as_array()
            .is_some_and(|holders| holders.iter().any(|holder| holder["kind"] == "origin")),
        "{class}"
    );

    sweeping(
        &world,
        "sweeper",
        crate::harness::agent("hold", &[]),
        Vec::new(),
    );
    let refused = |world: &World| {
        world
            .events_of("sweeper", "branches-retired")
            .into_iter()
            .flat_map(|event| {
                event["payload"]["failed"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            })
            .find(|failed| failed["branch"] == "done/published")
    };
    world.until("the refused deletion to be journalled", |world| {
        refused(world).is_some()
    });
    let failed = refused(&world).expect("a failure");
    assert_eq!(failed["identity"], SERVICE_IDENTITY, "{failed}");
    let error = failed["error"].as_str().expect("an error");
    assert!(error.contains("origin"), "{failed}");
    assert!(error.contains("pre-receive hook declined"), "{failed}");
    assert!(
        retired(&world, "sweeper")
            .iter()
            .all(|entry| entry["branch"] != "done/published"),
        "a branch the origin kept was journalled as retired"
    );
    assert!(!holds(&world, &repo.checkout, "done/published"));
    assert!(holds(&world, &repo.origin, "done/published"));

    world.release("hold.go");
    world.until("the sweeper to settle", |world| {
        world.run_file("sweeper", "result.json").is_file()
    });
    let result = world.run_json("sweeper", "result.json");
    assert_eq!(result["nodes"][0]["status"], "done", "{result}");
}

/// A sweep maintains every identity before its retirement pass begins, and its
/// `pool-maintenance` record is journalled while that pass is still running; a
/// run whose last node settles then waits for the pass, and journals its
/// `branches-retired` before its result.
///
/// The pass is held at `service`'s origin, deleting a published lossless branch.
/// `tail` sorts after `service`, so a record naming both identities while the
/// hold stands is one written after every identity was maintained and before
/// the pass ended.
#[cfg(unix)]
#[test]
fn a_sweep_records_its_maintenance_before_its_retirement_pass_ends() {
    let world = sweeping_world("retirement-after-maintenance");
    let repo = world.repository("local-direct", &[]);
    let tail = world.extra_repository("tail");
    let maintain = crate::maintenance::interpreted_script(&world, "maintain");
    let command = serde_json::to_string(&[maintain.as_str()]).expect("an argv serializes");
    crate::maintenance::pooled_with_commands(
        &world,
        &[("service", command.as_str()), ("tail", command.as_str())],
        "120s",
    );
    crate::maintenance::cut_a_slot(&world, &repo.checkout);
    crate::maintenance::cut_a_slot(&world, &tail.checkout);
    published_and_superseded(&world, &repo, "done/published", "published.md");
    let (entered, go) = origin_held(&world, &repo);

    sweeping(
        &world,
        "sweeper",
        crate::harness::agent("hold", &[]),
        Vec::new(),
    );
    world.until("the retirement pass to reach the origin", |_| {
        entered.is_file()
    });
    world.until("the maintenance record", |world| {
        !crate::maintenance::records(world, "sweeper").is_empty()
    });
    let record = &crate::maintenance::records(&world, "sweeper")[0];
    let identities = record["payload"]["identities"]
        .as_array()
        .expect("identities");
    assert_eq!(
        identities
            .iter()
            .map(|entry| entry["identity"].as_str().expect("a key"))
            .collect::<Vec<_>>(),
        [SERVICE_IDENTITY, "github.com/owner/tail"],
        "{record}"
    );
    for entry in identities {
        assert_eq!(
            entry["outcome"]["slots"][0]["outcome"]["ran"]["outcome"], "succeeded",
            "{record}"
        );
    }
    assert!(
        world.events_of("sweeper", "branches-retired").is_empty(),
        "{}",
        world.dump()
    );
    assert!(
        world.run_file("sweeper", "maintenance.json").is_file(),
        "the sweep ended while its retirement pass was held"
    );
    world
        .run(&["status", "sweeper"])
        .exited(0)
        .out_has("pool maintenance: a sweep of the host's worktree pools is in progress");

    world.release("hold.go");
    world.until("the held node to settle", |world| {
        !world.events_of("sweeper", "node-settled").is_empty()
    });
    assert!(
        !world.run_file("sweeper", "result.json").is_file(),
        "the run settled while its sweep's retirement pass was held"
    );

    std::fs::write(&go, "go").expect("the origin is released");
    world.until("the sweeper to settle", |world| {
        world.run_file("sweeper", "result.json").is_file()
    });
    assert!(retired(&world, "sweeper")
        .iter()
        .any(|entry| entry["branch"] == "done/published"));
    assert!(!holds(&world, &repo.origin, "done/published"));
    let kinds: Vec<Value> = world
        .journal("sweeper")
        .into_iter()
        .map(|event| event["kind"].clone())
        .filter(|kind| kind == "pool-maintenance" || kind == "branches-retired")
        .collect();
    assert_eq!(
        kinds,
        [json!("pool-maintenance"), json!("branches-retired")],
        "{}",
        world.dump()
    );
}

/// A run whose last node settles while its sweep is still maintaining waits for
/// the sweep's retirement pass too, and journals both of the sweep's records —
/// maintenance first — before its result.
#[cfg(unix)]
#[test]
fn a_run_settling_mid_sweep_journals_its_maintenance_and_retirement_before_its_result() {
    let world = sweeping_world("retirement-closeout-sweep");
    let repo = world.repository("local-direct", &[]);
    let maintaining = world.root.join("maintain.go");
    crate::maintenance::pooled_with_maintenance(&world, Some(&maintaining));
    crate::maintenance::cut_a_slot(&world, &repo.checkout);
    published_and_superseded(&world, &repo, "done/published", "published.md");
    let (entered, go) = origin_held(&world, &repo);

    sweeping(
        &world,
        "closing",
        crate::harness::agent("hold", &[]),
        Vec::new(),
    );
    world.until("the sibling to report the slot maintaining", |world| {
        pool_status(world)["slots"][0]["state"]["state"] == "maintaining"
    });
    world.release("hold.go");
    world.until("the held node to settle", |world| {
        !world.events_of("closing", "node-settled").is_empty()
    });

    std::fs::write(&maintaining, "go").expect("the maintenance is released");
    world.until("the retirement pass to reach the origin", |_| {
        entered.is_file()
    });
    assert!(
        !world.run_file("closing", "result.json").is_file(),
        "the run settled while its sweep's retirement pass was held"
    );

    std::fs::write(&go, "go").expect("the origin is released");
    world.until("the run to settle", |world| {
        world.run_file("closing", "result.json").is_file()
    });
    let kinds: Vec<Value> = world
        .journal("closing")
        .into_iter()
        .map(|event| event["kind"].clone())
        .filter(|kind| kind == "pool-maintenance" || kind == "branches-retired")
        .collect();
    assert_eq!(
        kinds,
        [json!("pool-maintenance"), json!("branches-retired")],
        "{}",
        world.dump()
    );
    assert!(retired(&world, "closing")
        .iter()
        .any(|entry| entry["branch"] == "done/published"));
    assert!(!holds(&world, &repo.origin, "done/published"));
}
