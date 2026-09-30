//! `onevcs`'s draft lifecycle, as a run settles, retries and reports what it produced.
//!
//! Every publication carrying no draft reason opens its change request as a draft
//! while the required checks run, and once they settle `onevcs` lifts it — or, on a
//! `change-open` identity whose approvals are required, keeps it as a draft for its
//! own user's review. The lifecycle is that library's; what these journeys hold is
//! what this crate makes of each answer: the node's status, its outcome word, its
//! landing, its detail, whether its dependents proceed, and whether a retry keeps a
//! lifted change lifted.
//!
//! Each journey drives the compiled binary publishing through the real linked
//! `onevcs` over a real origin on disk, with GitHub standing in at that library's
//! own `ONEVCS_GH` seam — the `fake-gh` double, seeded with the required checks a
//! journey needs and read back for whether the host holds the change as a draft.

// llmlint: ignore-file[e2e_not_mocked] the crate under test is driven as a real compiled
// binary and the sibling these journeys are about — `onevcs` — is the real library, over
// real git and a real origin on disk. `oneagentgraph` is substituted at its subprocess
// boundary so a journey states a dispatch outcome rather than paying for a model turn, and
// GitHub is substituted at `onevcs`'s own `ONEVCS_GH` override so a change request's
// checks and draft state can be stated offline. `harness.rs` carries the same suppression
// and the full rationale.

use crate::harness::{agent, lifecycle, plan_of, World};
use crate::run_end_hooks::{hook, invocations, records, HOOK_TIMEOUT, RECORD_ENV};
use serde_json::{json, Value};

/// Every required check the host reports, concluded green.
const GREEN: &str = "lint completed success required\ntest (linux) completed success required";

/// A required check the host reports red.
const RED: &str = "lint completed failure required";

/// A required check that has started and never finishes.
const NEVER_SETTLES: &str = "lint in_progress - required";

/// The draft grace window a journey about an early lift runs under, in seconds:
/// short enough that the lift happens inside the watch's two-second bound.
const SHORT_GRACE: &str = "0.2";

/// Give the world's identity a rules file: `publication`, `approvals`, and a
/// `drafts:` mapping where one is given.
fn rules(world: &World, publication: &str, approvals: &str, drafts: Option<&str>) {
    let drafts = drafts
        .map(|drafts| format!("  drafts: {drafts}\n"))
        .unwrap_or_default();
    std::fs::write(
        world.onevcs_home().join("rules.yml"),
        format!(
            "version: 3\nrules: []\ndefault:\n  publication: {publication}\n  approvals: \
             {approvals}\n{drafts}"
        ),
    )
    .expect("the rules file is written");
}

/// A `change-open` identity whose approvals are required: a team repository that
/// does not merge automatically.
fn team_repository(world: &World) {
    world.repository("change-open", &[]);
    rules(world, "change-open", "required", None);
}

/// Launch a plan of `nodes` attached, with any `extra` arguments, and wait for its
/// result.
fn settled_run(
    world: &World,
    name: &str,
    nodes: Vec<Value>,
    extra: &[&str],
) -> crate::harness::Run {
    let path = world.plan(name, &plan_of(name, nodes));
    let mut args = vec!["start", path.as_str(), "--attach"];
    args.extend_from_slice(extra);
    let launched = world.run_from(&world.project, &args);
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });
    launched
}

/// One node of a run's result.
fn node_of(world: &World, run: &str, id: &str) -> Value {
    let result = world.run_json(run, "result.json");
    result["nodes"]
        .as_array()
        .expect("the result lists nodes")
        .iter()
        .find(|node| node["id"] == id)
        .cloned()
        .unwrap_or_else(|| panic!("{id} is missing from {result}"))
}

/// The detail one node's newest settlement carries.
fn detail_of(world: &World, run: &str, id: &str) -> String {
    world
        .events_of(run, "node-settled")
        .into_iter()
        .rfind(|event| event["labels"]["node"] == id)
        .and_then(|event| event["payload"]["detail"].as_str().map(str::to_owned))
        .unwrap_or_else(|| panic!("{id} settled with no detail\n{}", why(world, run)))
}

/// What the host holds change request `number` as: `draft`, `open` or `merged`.
fn host_holds(world: &World, number: u64) -> String {
    std::fs::read_to_string(world.fakes.join("gh").join(number.to_string()))
        .unwrap_or_else(|error| panic!("the host holds no change request {number}: {error}"))
        .trim()
        .to_owned()
}

/// Every `gh` invocation the host was asked, in order.
fn gh_calls(world: &World) -> Vec<Vec<String>> {
    world
        .invocations()
        .into_iter()
        .filter(|call| call["tool"] == "gh")
        .map(|call| {
            call["args"]
                .as_array()
                .expect("an invocation carries args")
                .iter()
                .map(|arg| arg.as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .collect()
}

/// The `gh pr <verb>` invocations the host was asked.
fn gh_pr(world: &World, verb: &str) -> Vec<Vec<String>> {
    gh_calls(world)
        .into_iter()
        .filter(|call| call.first().map(String::as_str) == Some("pr"))
        .filter(|call| call.get(1).map(String::as_str) == Some(verb))
        .collect()
}

/// Why a run settled the way it did, and what the sibling recorded.
fn why(world: &World, run: &str) -> String {
    let settled: Vec<String> = world
        .events_of(run, "node-settled")
        .iter()
        .map(|event| {
            format!(
                "{} {} {}: {}",
                event["labels"]["node"],
                event["payload"]["status"],
                event["payload"]["outcome"],
                event["payload"]["detail"]
            )
        })
        .collect();
    let recorded: Vec<String> = world
        .journal(run)
        .iter()
        .filter(|event| event["source"] == "vcs")
        .filter_map(|event| event["kind"].as_str().map(str::to_owned))
        .collect();
    format!(
        "what the nodes settled on:\n  {}\n  the sibling recorded: {recorded:?}",
        settled.join("\n  ")
    )
}

/// Each dispatch of `node`, in order: a re-dispatch is another `node-dispatched`.
fn dispatches_of(world: &World, run: &str, node: &str) -> Vec<Value> {
    world
        .events_of(run, "node-dispatched")
        .into_iter()
        .filter(|event| event["labels"]["node"] == node)
        .collect()
}

/// The branch every session a run opened was cut on, once each, in order.
fn session_branches(world: &World, run: &str) -> Vec<String> {
    world
        .journal(run)
        .iter()
        .filter(|event| event["source"] == "vcs" && event["kind"] == "session-opened")
        .filter_map(|event| event["payload"]["branch"].as_str().map(str::to_owned))
        .fold(Vec::new(), |mut seen, branch| {
            if !seen.contains(&branch) {
                seen.push(branch);
            }
            seen
        })
}

/// The journey the new outcome exists for: on a team repository, a change whose
/// required checks all come back green is **kept as a draft** for the person who
/// dispatched it, and the node is done — its dependent proceeds and the run ends
/// settled, firing its success hook.
#[test]
fn a_green_change_on_a_team_repository_is_kept_as_a_draft_and_the_run_succeeds() {
    let world = World::new("draft-review");
    std::fs::create_dir_all(records(&world)).expect("a record directory");
    let record = records(&world).to_string_lossy().into_owned();
    let world = world.with_env(RECORD_ENV, &record);
    let hook = hook(&world);
    team_repository(&world);
    world.script("service.work", "the worker wrote this\n");
    world.script("gh.checks", GREEN);

    let run = "reviewdraft";
    settled_run(
        &world,
        run,
        vec![lifecycle("service", &[]), agent("after", &["service"])],
        &[
            "--success-hook",
            &hook,
            "--failure-hook",
            &hook,
            "--hook-timeout",
            HOOK_TIMEOUT,
        ],
    )
    .exited(0)
    .out_has("\"settlement\":\"complete\"");

    let node = node_of(&world, run, "service");
    assert_eq!(node["status"], "done", "{node}\n{}", why(&world, run));
    assert_eq!(node["outcome"], "change-review-draft", "{node}");
    assert_eq!(node["landing"], "unlanded", "{node}");
    assert_eq!(
        node["change_url"], "https://github.com/owner/service/pull/1",
        "{node}"
    );
    let detail = detail_of(&world, run, "service");
    assert!(
        detail.starts_with(
            "the required checks are green and the change request is kept as a draft for its \
             user's review: lift it on the host or with `onevcs change ready s-"
        ) && !detail.contains('\n'),
        "{detail}"
    );

    // The host still holds it as a draft: nothing lifted it, and nothing merged it.
    assert_eq!(host_holds(&world, 1), "draft");
    assert!(gh_pr(&world, "ready").is_empty(), "{:?}", gh_calls(&world));
    assert!(gh_pr(&world, "merge").is_empty(), "{:?}", gh_calls(&world));
    let kept = world.events_of(run, "draft-kept-for-review");
    assert_eq!(kept.len(), 1, "{}", why(&world, run));
    assert_eq!(kept[0]["phase"], "review");
    let settled = world.events_of(run, "checks-settled");
    assert_eq!(settled.len(), 1, "{}", why(&world, run));
    assert_eq!(settled[0]["payload"]["verdict"], "passed");

    // Its dependent dispatched and settled, and the run succeeded.
    assert_eq!(dispatches_of(&world, run, "after").len(), 1);
    assert_eq!(node_of(&world, run, "after")["status"], "done");
    assert_eq!(invocations(&world, run), ["success"], "{}", world.dump());

    // And `results` shows the word and where the change is.
    world
        .run(&["results", run])
        .exited(0)
        .out_has("change-review-draft")
        .out_has("https://github.com/owner/service/pull/1");
}

/// A node the plan asked to leave as a draft is **never lifted by green checks**:
/// under `change-auto`, with every required check green, it publishes with its
/// held reason and settles `done` / `change-draft`, the host still holding it as a
/// draft and nothing merging it — told apart by word from the green team draft.
#[test]
fn a_plan_s_own_draft_is_never_lifted_by_green_checks() {
    let world = World::new("draft-held-green");
    world.repository("change-auto", &[]);
    world.script("service.work", "the worker wrote this\n");
    world.script("gh.checks", GREEN);
    world.script("gh.merged", "");
    let mut node = lifecycle("service", &[]);
    node["draft"] = json!(true);

    let run = "heldgreen";
    settled_run(&world, run, vec![node], &[]).settled();

    let node = node_of(&world, run, "service");
    assert_eq!(node["status"], "done", "{node}\n{}", why(&world, run));
    assert_eq!(node["outcome"], "change-draft", "{node}");
    assert_eq!(
        detail_of(&world, run, "service"),
        "complete, and left as a draft as the plan asked, for a person to mark ready for review"
    );
    let drafted = world.events_of(run, "change-drafted");
    assert_eq!(drafted.len(), 1, "{}", why(&world, run));
    assert_eq!(drafted[0]["payload"]["kind"], "held", "{}", drafted[0]);
    assert!(
        world.events_of(run, "draft-lifted").is_empty(),
        "{}",
        why(&world, run)
    );
    assert_eq!(host_holds(&world, 1), "draft");
    assert!(gh_pr(&world, "ready").is_empty(), "{:?}", gh_calls(&world));
    assert!(gh_pr(&world, "merge").is_empty(), "{:?}", gh_calls(&world));
}

/// `change-open` now waits for its checks: a red required check sends the node back
/// to the same branch with `checks-failed`, exactly as a `change-auto` node is sent,
/// and a spent budget settles it `failed` / `checks-failed`.
#[test]
fn a_red_check_on_a_change_open_identity_is_retried_on_its_branch_until_the_budget_is_spent() {
    let world = World::new("draft-open-red");
    world.repository("change-open", &[]);
    // A new commit on every attempt, so each retry republishes work of its own
    // rather than meeting the same refusal over the same commit.
    world.script("service.work-anew", "the worker wrote this\n");
    world.script("gh.checks", RED);

    let run = "openred";
    settled_run(&world, run, vec![lifecycle("service", &[])], &[]);

    let node = node_of(&world, run, "service");
    assert_eq!(node["status"], "failed", "{node}\n{}", why(&world, run));
    assert_eq!(node["outcome"], "checks-failed", "{node}");
    let dispatched = dispatches_of(&world, run, "service");
    assert_eq!(dispatched.len(), 3, "{}", why(&world, run));
    for again in &dispatched[1..] {
        let reason = again["payload"]["reason"].as_str().unwrap_or_default();
        assert!(reason.starts_with("checks-failed:"), "{again}");
        assert!(reason.contains("lint"), "{again}");
    }
    let branches = session_branches(&world, run);
    assert_eq!(
        branches.len(),
        1,
        "every attempt was not on one branch: {branches:?}"
    );
    assert!(
        detail_of(&world, run, "service")
            .contains("1 checks-failed, 2 checks-failed, 3 checks-failed"),
        "{}",
        why(&world, run)
    );
    // Never lifted: a red draft stays a draft.
    assert_eq!(host_holds(&world, 1), "draft");
}

/// And a required check that never settles within the watch's bound sends it back
/// with `checks-unsettled`, settling `failed` / `checks-unsettled` once the budget
/// is spent.
#[test]
fn a_check_that_never_settles_on_a_change_open_identity_is_retried_as_unsettled() {
    let world = World::new("draft-open-unsettled");
    world.repository("change-open", &[]);
    // A new commit on every attempt, so each retry republishes work of its own
    // rather than meeting the same refusal over the same commit.
    world.script("service.work-anew", "the worker wrote this\n");
    world.script("gh.checks", NEVER_SETTLES);

    let run = "openunsettled";
    settled_run(&world, run, vec![lifecycle("service", &[])], &[]);

    let node = node_of(&world, run, "service");
    assert_eq!(node["status"], "failed", "{node}\n{}", why(&world, run));
    assert_eq!(node["outcome"], "checks-unsettled", "{node}");
    let dispatched = dispatches_of(&world, run, "service");
    assert_eq!(dispatched.len(), 3, "{}", why(&world, run));
    for again in &dispatched[1..] {
        let reason = again["payload"]["reason"].as_str().unwrap_or_default();
        assert!(reason.starts_with("checks-unsettled:"), "{again}");
    }
    assert_eq!(
        session_branches(&world, run).len(),
        1,
        "{}",
        why(&world, run)
    );
}

/// **The lift is one-way**, and the engine does not work against it: a node with
/// no `draft` and no release to await, whose change request was lifted, is
/// re-dispatched onto the same branch and republishes with no draft reason — its
/// new sessions record no `change-drafted`, and the host still holds it ready.
#[test]
fn a_lifted_change_stays_lifted_across_a_retry_on_its_branch() {
    let world = World::new("draft-lifted-retry");
    world.repository("change-auto", &[]);
    // A new commit on every attempt, so each retry republishes work of its own
    // rather than meeting the same refusal over the same commit.
    world.script("service.work-anew", "the worker wrote this\n");
    // Green while it is a draft, so the lifecycle lifts it; red once it is ready,
    // so the merge path refuses it and the node goes back to its branch.
    world.script("gh.checks.draft", "lint completed success required");
    world.script("gh.checks", RED);

    let run = "liftedretry";
    settled_run(&world, run, vec![lifecycle("service", &[])], &[]);

    let node = node_of(&world, run, "service");
    assert_eq!(
        node["outcome"],
        "checks-failed",
        "{node}\n{}",
        why(&world, run)
    );
    let dispatched = dispatches_of(&world, run, "service");
    assert!(dispatched.len() >= 2, "{}", why(&world, run));
    assert_eq!(
        session_branches(&world, run).len(),
        1,
        "{}",
        why(&world, run)
    );

    // One change request, drafted once — by the first session — and lifted once.
    assert_eq!(world.changes_opened().len(), 1);
    let drafted = world.events_of(run, "change-drafted");
    assert_eq!(drafted.len(), 1, "{}", why(&world, run));
    assert_eq!(drafted[0]["payload"]["kind"], "awaiting-checks");
    let first = drafted[0]["labels"]["session"].clone();
    assert_eq!(world.events_of(run, "draft-lifted").len(), 1);
    // Every later session published without a reason and recorded no draft.
    let publishing: Vec<Value> = world
        .events_of(run, "change-opened")
        .into_iter()
        .map(|event| event["labels"]["session"].clone())
        .collect();
    assert_eq!(publishing.len(), dispatched.len(), "{}", why(&world, run));
    assert!(
        publishing[1..].iter().all(|session| *session != first),
        "{publishing:?}"
    );
    assert_eq!(host_holds(&world, 1), "open");
    assert_eq!(gh_pr(&world, "ready").len(), 1, "{:?}", gh_calls(&world));
}

/// A worker's own draft is not lifted on the spot by the closeout: on a team
/// repository with every check green the lifecycle keeps it, and the node says so.
#[test]
fn a_worker_s_draft_on_a_team_repository_is_kept_for_its_user_s_review() {
    let world = World::new("draft-worker-team");
    team_repository(&world);
    world.script("service.work", "the worker wrote this\n");
    world.script("service.drafts", "wip: opened while working");
    world.script("gh.checks", GREEN);

    let run = "workerteam";
    settled_run(&world, run, vec![lifecycle("service", &[])], &[]).settled();

    let node = node_of(&world, run, "service");
    assert_eq!(node["status"], "done", "{node}\n{}", why(&world, run));
    assert_eq!(node["outcome"], "change-review-draft", "{node}");
    let detail = detail_of(&world, run, "service");
    assert!(
        detail.contains(
            "the worker opened the change request as a draft; the closeout left the description \
             as the worker left it, and the publication kept it as a draft for its user's review"
        ),
        "{detail}"
    );
    assert!(!detail.contains("marked it ready"), "{detail}");
    assert_eq!(host_holds(&world, 1), "draft");
    assert!(gh_pr(&world, "ready").is_empty(), "{:?}", gh_calls(&world));
}

/// The same worker's draft on a `change-auto` identity is lifted by the lifecycle
/// once its checks are green, and merged.
#[test]
fn a_worker_s_draft_on_an_auto_merging_repository_is_lifted_and_merged() {
    let world = World::new("draft-worker-auto");
    world.repository("change-auto", &[]);
    world.script("service.work", "the worker wrote this\n");
    world.script("service.drafts", "wip: opened while working");
    world.script("gh.checks", GREEN);
    world.script("gh.merged", "");

    let run = "workerauto";
    settled_run(&world, run, vec![lifecycle("service", &[])], &[]).settled();

    let node = node_of(&world, run, "service");
    assert_eq!(node["status"], "done", "{node}\n{}", why(&world, run));
    assert_eq!(node["outcome"], "merged", "{node}");
    let detail = detail_of(&world, run, "service");
    assert!(
        detail.ends_with("and the publication lifted it out of its draft and merged it"),
        "{detail}"
    );
    assert!(!detail.contains("marked it ready"), "{detail}");
    assert_eq!(host_holds(&world, 1), "merged");
    assert_eq!(gh_pr(&world, "ready").len(), 1, "{:?}", gh_calls(&world));
}

/// A repository whose checks skip drafts: none of the required checks runs on the
/// draft, so `onevcs` lifts it before they are green so that they can run, and
/// warns — and the node's detail says so in one sentence.
#[test]
fn a_draft_lifted_before_its_checks_ran_says_so_on_the_node() {
    let world =
        World::new("draft-early").with_env("ONEVCS_DRAFT_CHECKS_GRACE_SECONDS", SHORT_GRACE);
    world.repository("change-open", &[]);
    world.script("service.work", "the worker wrote this\n");
    world.script("gh.checks.draft", "lint completed skipped required");
    world.script("gh.checks", "lint completed success required");

    let run = "early";
    settled_run(&world, run, vec![lifecycle("service", &[])], &[]).settled();

    let node = node_of(&world, run, "service");
    assert_eq!(node["status"], "done", "{node}\n{}", why(&world, run));
    assert_eq!(node["outcome"], "change-open", "{node}");
    let early = world.events_of(run, "draft-lifted-early");
    assert_eq!(early.len(), 1, "{}", why(&world, run));
    assert_eq!(early[0]["payload"]["warned"], true);
    assert_eq!(
        detail_of(&world, run, "service"),
        "the required checks did not start on the draft, so it was lifted before its checks \
         were green"
    );
    assert_eq!(host_holds(&world, 1), "open");
}

/// And an identity that asked not to be warned — `drafts: {warn_on_early_lift:
/// false}` — is not told on the node either.
#[test]
fn an_early_lift_the_identity_silenced_says_nothing_on_the_node() {
    let world =
        World::new("draft-early-quiet").with_env("ONEVCS_DRAFT_CHECKS_GRACE_SECONDS", SHORT_GRACE);
    world.repository("change-open", &[]);
    rules(
        &world,
        "change-open",
        "none",
        Some("{warn_on_early_lift: false}"),
    );
    world.script("service.work", "the worker wrote this\n");
    world.script("gh.checks.draft", "lint completed skipped required");
    world.script("gh.checks", "lint completed success required");

    let run = "earlyquiet";
    settled_run(&world, run, vec![lifecycle("service", &[])], &[]).settled();

    let node = node_of(&world, run, "service");
    assert_eq!(
        node["outcome"],
        "change-open",
        "{node}\n{}",
        why(&world, run)
    );
    let early = world.events_of(run, "draft-lifted-early");
    assert_eq!(early.len(), 1, "{}", why(&world, run));
    assert_eq!(early[0]["payload"]["warned"], false);
    let detail = world
        .events_of(run, "node-settled")
        .into_iter()
        .find_map(|event| event["payload"]["detail"].as_str().map(str::to_owned));
    assert!(
        detail
            .as_deref()
            .is_none_or(|detail| !detail.contains("did not start on the draft")),
        "{detail:?}"
    );
}

/// A required check that concluded **skipped** is never shown as passed: the
/// node's detail names it, the relayed `change-check` carries its `skipped` state,
/// and `monitor` renders that state.
#[test]
fn a_skipped_required_check_is_named_and_never_shown_as_passed() {
    let world = World::new("draft-skipped");
    world.repository("change-open", &[]);
    // No draft, so the check is read the way a ready change's is: skipped satisfies
    // the host's merge path, and is recorded as skipped rather than green.
    rules(&world, "change-open", "none", Some("{disabled: true}"));
    world.script("service.work", "the worker wrote this\n");
    world.script(
        "gh.checks",
        "lint completed skipped required\ntest completed success required",
    );

    let run = "skipped";
    settled_run(&world, run, vec![lifecycle("service", &[])], &[]).settled();

    let node = node_of(&world, run, "service");
    assert_eq!(node["status"], "done", "{node}\n{}", why(&world, run));
    assert_eq!(node["outcome"], "change-open", "{node}");
    assert_eq!(
        detail_of(&world, run, "service"),
        "the required check `lint` concluded skipped: it did not run, so it is not counted as \
         passed"
    );
    let settled = world.events_of(run, "checks-settled");
    assert_eq!(settled.len(), 1, "{}", why(&world, run));
    assert_eq!(settled[0]["payload"]["verdict"], "passed-with-skipped");
    assert_eq!(settled[0]["payload"]["skipped"], json!(["lint"]));

    // The relayed records carry each check's state as `onevcs` classified it.
    let checks = world.events_of(run, "change-check");
    let lint: Vec<&Value> = checks
        .iter()
        .filter(|event| event["payload"]["name"] == "lint")
        .collect();
    assert!(!lint.is_empty(), "{}", why(&world, run));
    for event in &lint {
        assert_eq!(event["payload"]["state"], "skipped", "{event}");
    }
    // And the view that renders them shows `skipped`, never `passed`, for it.
    let monitor = world.run(&["monitor", run, "--all"]);
    monitor.exited(0);
    let lines: Vec<&str> = monitor
        .stdout
        .lines()
        .filter(|line| line.contains(" change-check "))
        .collect();
    assert_eq!(lines.len(), checks.len(), "{}", monitor.stdout);
    assert!(
        lines.iter().any(|line| line.ends_with("completed skipped")),
        "{}",
        monitor.stdout
    );
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.ends_with("completed passed"))
            .count(),
        checks.len() - lint.len(),
        "{}",
        monitor.stdout
    );
}
