//! Readjustment: a stack of review drafts kept true while people merge drafts on the
//! host and the rest are still in review.
//!
//! Every journey drives the compiled binary publishing through the real linked
//! `onevcs` over a real origin on disk, with GitHub standing in at that library's own
//! `ONEVCS_GH` seam — the `fake-gh` double, which answers the batched standings read,
//! the retarget and the reopen out of the change requests it holds. A person's moves
//! on the host — a squash or merge commit onto the base, a head kept or deleted, a
//! draft closed or moved by hand, a verdict on a head — are made through that double's
//! own hidden `__person` entry, which writes the real commit onto the origin and is
//! never recorded on the ledger of what the host was asked.

// llmlint: ignore-file[e2e_not_mocked] the crate under test is driven as a real compiled
// binary, and `onevcs` — which reads the standings, restacks each draft, publishes, moves and
// reopens change requests — is the real library over real git and a real origin on disk.
// `oneagentgraph` is substituted at its subprocess boundary so a journey states what a worker
// wrote, and GitHub at `onevcs`'s own `ONEVCS_GH` override so a person's merge and a change
// request's base, state and description can be made and read offline. `harness.rs` carries
// the same suppression and the full rationale.

use crate::harness::{double, git, lifecycle, plan_of, World};
use crate::stacked_drafts::{
    change_from, changed_against, held_as_draft, service, settled, team_world, text,
};
use serde_json::{json, Value};

/// The repository slug every change request of a team world is opened on.
const SLUG: &str = "owner/service";

/// Make a person's move on the host: `__person <args…>` of the `fake-gh` double.
fn person(world: &World, args: &[&str]) {
    let output = std::process::Command::new(double("fake-gh"))
        .arg("__person")
        .args(args)
        .env(onepipeline_testfakes::SCRIPT_DIR_ENV, &world.fakes)
        .output()
        .expect("the host double runs");
    assert!(
        output.status.success(),
        "the person's move {args:?} was refused: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A person merges change request `number` on the host — `how` a squash or a merge
/// commit — and `after` says what becomes of its head.
fn merge(world: &World, number: &Value, how: &str, after: &str) {
    person(world, &["merge", SLUG, &number.to_string(), how, after]);
}

/// `onepipeline readjust RUN --json`, its receipt parsed.
fn readjust(world: &World, run: &str) -> (i32, Value) {
    let output = world.run(&["readjust", run, "--json"]);
    let receipt = serde_json::from_str(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "readjust printed no receipt ({error}):\n{}\n{}",
            output.stdout, output.stderr
        )
    });
    (output.code, receipt)
}

/// Every `gh` call the host was asked since `from`.
fn host_calls(world: &World, from: usize) -> Vec<Value> {
    world
        .invocations()
        .into_iter()
        .skip(from)
        .filter(|call| call["tool"] == "gh")
        .collect()
}

/// Whether `call` is a batched standings read.
fn is_standings_read(call: &Value) -> bool {
    call["args"][0] == "api"
        && call["args"][1] == "graphql"
        && call["args"].as_array().is_some_and(|args| {
            args.iter().any(|arg| {
                arg.as_str()
                    .is_some_and(|arg| arg.contains("c0: pullRequest(number:"))
            })
        })
}

/// The query a GraphQL call carried.
fn query_of(call: &Value) -> &str {
    call["args"]
        .as_array()
        .and_then(|args| {
            args.iter()
                .filter_map(Value::as_str)
                .find(|arg| arg.starts_with("query="))
        })
        .unwrap_or_default()
}

/// Whether `call` is one of the two reads the sibling makes of its own accord around
/// a session open and a write: the repository's visibility, and whether its rules
/// require a branch up to date with its base.
fn is_sibling_read(call: &Value) -> bool {
    call["args"][0] == "api"
        && call["args"].as_array().is_some_and(|args| args.len() == 2)
        && call["args"][1].as_str().is_some_and(|path| {
            path == format!("repos/{SLUG}")
                || path.starts_with(&format!("repos/{SLUG}/rules/branches/"))
        })
}

/// Whether `call` writes a change request's base, description or state.
fn is_write(call: &Value) -> bool {
    call["args"][0] == "api"
        && call["args"][1]
            .as_str()
            .is_some_and(|path| path.contains("/pulls/"))
        && call["args"][3] == "PATCH"
}

/// What the origin's `branch` stands at.
fn tip(world: &World, branch: &str) -> String {
    git(world, &service(world).origin, &["rev-parse", branch])
        .trim()
        .to_owned()
}

/// Whether `ancestor` is an ancestor of `of` on the origin.
fn is_ancestor(world: &World, ancestor: &str, of: &str) -> bool {
    std::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", ancestor, of])
        .current_dir(service(world).origin)
        .status()
        .expect("git runs")
        .success()
}

/// The newest record of `kind` about `node`.
fn record(world: &World, run: &str, kind: &str, node: &str) -> Option<Value> {
    world
        .events_of(run, kind)
        .into_iter()
        .rev()
        .find(|event| event["labels"]["node"] == node)
        .map(|event| event["payload"].clone())
}

/// Write `text` into change request `number`'s description on the host, outside its
/// Stack block, as a reviewer editing it would.
fn write_on_host(world: &World, number: &Value, text: &str) {
    let opened = world.fakes.join("gh").join("opened.jsonl");
    let changes: Vec<String> = std::fs::read_to_string(&opened)
        .expect("the host opened change requests")
        .lines()
        .map(|line| {
            let mut change: Value = serde_json::from_str(line).expect("a change request");
            if change["number"] == *number {
                change["body"] = Value::String(format!(
                    "{text}\n\n{}",
                    change["body"].as_str().unwrap_or_default()
                ));
            }
            change.to_string()
        })
        .collect();
    std::fs::write(&opened, format!("{}\n", changes.join("\n"))).expect("the edit is written");
}

/// What `status` and `results` print for `run`.
fn views(world: &World, run: &str) -> (String, String) {
    let status = world.run(&["status", run]);
    status.exited(0);
    let results = world.run(&["results", run]);
    results.exited(0);
    (status.stdout, results.stdout)
}

/// The store's projected settlement of `node`'s task.
fn projected_settlement(world: &World, project: &str, node: &str) -> Value {
    world
        .store_tasks(project)
        .into_iter()
        .find(|task| task["item"]["metadata"]["onepipeline.id"] == node)
        .map(|task| task["item"]["metadata"]["onepipeline.settlement"].clone())
        .unwrap_or(Value::Null)
}

/// Launch `nodes` attached from the Markdown store, one at a time so the host numbers
/// their change requests in plan order, wait for the run's result, and hand back the
/// project it was launched from.
fn launched(world: &World, run: &str, nodes: Vec<Value>) -> String {
    launched_at(world, run, nodes, 1)
}

/// [`launched`], with the plan's `concurrency` set to `concurrency` for what the run is
/// given after its launch.
fn launched_at(world: &World, run: &str, nodes: Vec<Value>, concurrency: u64) -> String {
    let mut plan = plan_of(run, nodes);
    plan["concurrency"] = json!(concurrency);
    let project = world.plan(run, &plan);
    let launched = world.run_from(&world.project, &["start", &project, "--attach"]);
    assert!(
        world.run_file(run, "result.json").is_file(),
        "the run did not settle:\n{}\n{}",
        launched.stdout,
        launched.stderr
    );
    project
}

/// What became of a merged parent's head in a case-1 journey, and what the host did to
/// the draft based on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Head {
    /// The branch stays, and `b` stays on it.
    Kept,
    /// The branch is deleted, and the host retargets `b` onto the root itself.
    DeletedRetargeted,
    /// The branch is deleted through the refs API, and the host closes `b`.
    DeletedClosed,
}

impl Head {
    fn person(self) -> &'static str {
        match self {
            Self::Kept => "kept",
            Self::DeletedRetargeted => "deleted-retarget",
            Self::DeletedClosed => "deleted-close",
        }
    }
}

/// Case 1, the head kept: `a → b` settles as stacked drafts on a team identity, a
/// person squash-merges `a` into the root keeping its branch, and `onepipeline
/// readjust` moves `b` onto the root. `b`'s change request targets the root and is
/// still a draft; its branch carries the root's head and its own previous head; its
/// diff against the root is only its own; its push watches no checks and the next
/// trigger reads its verdict; the readjustment's host calls are one batched standings
/// read and one retarget write carrying the Stack block, beside the two single reads
/// the linked `retarget_change` makes inside itself; `b`'s record, views, store
/// settlement and Stack block all name the root; and `b`'s title and the text a person
/// wrote outside the block are untouched, with no drafter dispatched for it.
#[test]
fn a_parent_merged_with_its_branch_kept_moves_the_draft_above_onto_the_root() {
    a_single_parent_lands("readjust-kept", Head::Kept);
}

/// Case 1, the head deleted and `b` retargeted by the host: everything the kept case
/// holds, with the retarget recorded `unchanged` — the host already targets the root,
/// so the one write carries only the description with its Stack block.
#[test]
fn a_parent_merged_and_deleted_with_the_host_retargeting_the_draft_records_it_unchanged() {
    a_single_parent_lands("readjust-retargeted", Head::DeletedRetargeted);
}

/// Case 1, the head deleted through the refs API and `b` closed by the host: `b` is
/// recovered first — `a`'s branch restored at `a`'s merged head and `b` reopened, a
/// draft still, journalled `change-reopened` — then moved onto the root with the
/// restored branch deleted again, and everything the kept case holds besides.
#[test]
fn a_parent_merged_and_deleted_with_the_host_closing_the_draft_reopens_and_moves_it() {
    a_single_parent_lands("readjust-closed", Head::DeletedClosed);
}

/// One case-1 journey: `a → b`, `a` squash-merged by a person with its head as `head`
/// says, and one `onepipeline readjust`.
#[allow(
    clippy::too_many_lines,
    reason = "one journey's whole bar, held in the order a reader checks it"
)]
fn a_single_parent_lands(name: &str, head: Head) {
    let world = team_world(name);
    world.script("a.work", "a's change\n");
    world.script("b.work", "b's change\n");
    let run = "kept";
    let project = launched(
        &world,
        run,
        vec![lifecycle("a", &[]), lifecycle("b", &["a"])],
    );
    let a = settled(&world, run, "a");
    let b = settled(&world, run, "b");
    let a_branch = text(&a["branch"], "a's branch").to_owned();
    let b_branch = text(&b["branch"], "b's branch").to_owned();
    let b_head = text(&b["head"], "b's head").to_owned();
    assert_eq!(b["base"], a_branch.as_str(), "{b}");
    let a_change = change_from(&world, &a_branch);
    let b_change = change_from(&world, &b_branch);
    write_on_host(&world, &b_change["number"], "A reviewer's note.");
    let before_title = change_from(&world, &b_branch)["title"].clone();

    merge(&world, &a_change["number"], "squash", head.person());
    if head == Head::DeletedClosed {
        assert!(!held_as_draft(&world, &b_change["number"]));
    }
    let root = tip(&world, "main");
    let from = world.invocations().len();
    let (code, receipt) = readjust(&world, run);
    assert_eq!(code, 0, "{receipt}");
    assert_eq!(receipt["state"], "performed", "{receipt}");

    // On the host: the root, still a draft, and the person's text where it was.
    let moved = change_from(&world, &b_branch);
    assert_eq!(moved["base"], "main", "{moved}");
    assert!(held_as_draft(&world, &moved["number"]), "{moved}");
    assert_eq!(moved["title"], before_title, "{moved}");
    let body = text(&moved["body"], "b's description");
    assert!(body.starts_with("A reviewer's note.\n\n"), "{body}");
    assert!(
        body.contains("Drafted against `main`, the root"),
        "the Stack block does not name the root:\n{body}"
    );
    assert_eq!(
        body.matches("<!-- onepipeline:stack -->").count(),
        1,
        "{body}"
    );

    // On the origin: the root's head and b's own under its new head, and only b's
    // change against the root.
    let new_head = tip(&world, &b_branch);
    assert!(is_ancestor(&world, &root, &new_head));
    assert!(is_ancestor(&world, &b_head, &new_head));
    assert_eq!(
        changed_against(&world, &service(&world), "main", &b_branch),
        ["b.md"]
    );

    // On the record.
    let landed = record(&world, run, "node-landed", "a").expect("a's landing is recorded");
    assert_eq!(landed["detected_by"], "readjust", "{landed}");
    assert_eq!(landed["trigger"], "readjust", "{landed}");
    let readjusted =
        record(&world, run, "node-readjusted", "b").expect("b's readjustment is recorded");
    assert_eq!(
        readjusted["retarget"],
        if head == Head::DeletedRetargeted {
            "unchanged"
        } else {
            "retargeted"
        },
        "{readjusted}"
    );
    let reopened = record(&world, run, "change-reopened", "b");
    if head == Head::DeletedClosed {
        let reopened = reopened.expect("b's recovery is recorded");
        assert_eq!(reopened["restored_base"], a_branch.as_str(), "{reopened}");
        assert_eq!(reopened["change_url"], b["change_url"], "{reopened}");
    } else {
        assert!(reopened.is_none(), "{reopened:?}");
    }
    let a_kept = std::process::Command::new("git")
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{a_branch}"),
        ])
        .current_dir(service(&world).origin)
        .status()
        .expect("git runs")
        .success();
    assert_eq!(
        a_kept,
        head == Head::Kept,
        "a's branch is not where the journey left it"
    );
    assert_eq!(readjusted["content"], "merged", "{readjusted}");
    assert_eq!(readjusted["cause"], json!(["a"]), "{readjusted}");
    assert_eq!(
        readjusted["from"]["base"],
        a_branch.as_str(),
        "{readjusted}"
    );
    assert_eq!(readjusted["to"]["base"], "main", "{readjusted}");
    assert_eq!(readjusted["agent_dispatched"], false, "{readjusted}");
    let resettled = settled(&world, run, "b");
    assert_eq!(resettled["base"], "main", "{resettled}");
    assert!(resettled.get("stack").is_none(), "{resettled}");
    assert_eq!(
        resettled["outcome"], "change-review-draft-unchecked",
        "{resettled}"
    );
    assert_eq!(resettled["change_url"], b["change_url"], "{resettled}");

    // The host was asked exactly this: one batched read naming both drafts, and the
    // one write carrying the block — beside the two single reads `retarget_change`
    // makes inside itself, and the sibling's own visibility and up-to-date reads its
    // session open and its write guard make. Nothing else: no check watch, no
    // description read or write, no change request opened.
    let calls = host_calls(&world, from);
    let reads: Vec<&Value> = calls
        .iter()
        .filter(|call| is_standings_read(call))
        .collect();
    let writes: Vec<&Value> = calls.iter().filter(|call| is_write(call)).collect();
    // A reopen reads its change request before and after its write, as a retarget does.
    let (expected_reads, expected_writes) = match head {
        Head::DeletedClosed => (5, 2),
        _ => (3, 1),
    };
    assert_eq!(reads.len(), expected_reads, "{calls:#?}");
    assert!(
        query_of(reads[0]).contains("c1: pullRequest(number:"),
        "the readjustment's read is not one batch over both drafts: {calls:#?}"
    );
    assert!(
        reads[1..]
            .iter()
            .all(|read| !query_of(read).contains("c1: pullRequest(")),
        "{calls:#?}"
    );
    assert_eq!(writes.len(), expected_writes, "{calls:#?}");
    if head == Head::DeletedClosed {
        assert!(
            writes[0]["args"]
                .as_array()
                .is_some_and(|args| args.iter().any(|arg| arg == "state=open")),
            "{writes:#?}"
        );
    }
    assert!(
        writes[expected_writes - 1]["args"]
            .as_array()
            .is_some_and(
                |args| args.iter().any(|arg| arg.as_str().is_some_and(|arg| arg
                    .starts_with("body=")
                    && arg.contains("Drafted against `main`")))
            ),
        "{writes:#?}"
    );
    let others: Vec<&Value> = calls
        .iter()
        .filter(|call| !is_standings_read(call) && !is_write(call))
        .filter(|call| !is_sibling_read(call))
        .collect();
    assert!(
        others.is_empty(),
        "b's push asked the host something more: {others:#?}"
    );
    assert!(
        world
            .events_of(run, "node-dispatched")
            .iter()
            .all(|event| event["labels"]["node"] != "b"
                || event["labels"]["persona"].as_str() != Some("pr-author")),
        "a drafter was dispatched for b"
    );

    // Views and the store name the root.
    let (status, results) = views(&world, run);
    assert!(
        status.contains("b: readjusted onto the root main"),
        "{status}"
    );
    assert!(
        results.contains("readjusted onto the root main"),
        "{results}"
    );
    assert!(status.contains("drafted against the root main"), "{status}");
    assert!(
        status.contains("check verdict outstanding"),
        "the outstanding verdict is not named:\n{status}"
    );
    world.until_store("b's new base to reach the store", |world| {
        projected_settlement(world, &project, "b")["base"] == "main"
    });

    // The next trigger reads b's verdict, and reads it green.
    let (code, receipt) = readjust(&world, run);
    assert_eq!(code, 0, "{receipt}");
    let checked = record(&world, run, "node-checks-read", "b").expect("b's verdict is read");
    assert_eq!(checked["verdict"], "passed", "{checked}");
    assert_eq!(checked["head"], new_head.as_str(), "{checked}");
    assert!(receipt.get("outstanding").is_none(), "{receipt}");
}

/// The `readjust` that every multi-step journey makes: performed, exit 0.
fn readjusted(world: &World, run: &str) {
    let (code, receipt) = readjust(world, run);
    assert_eq!(code, 0, "{receipt}");
    assert_eq!(receipt["state"], "performed", "{receipt}");
}

/// What `node`'s change request targets on the host, and its description.
fn on_host(world: &World, run: &str, node: &str) -> (String, String) {
    let branch = settled(world, run, node)["branch"]
        .as_str()
        .unwrap_or_else(|| panic!("{node} recorded no branch"))
        .to_owned();
    let change = change_from(world, &branch);
    assert!(
        held_as_draft(world, &change["number"]),
        "{node}'s change request is not a draft: {change}"
    );
    (
        text(&change["base"], "the base").to_owned(),
        text(&change["body"], "the description").to_owned(),
    )
}

/// The branch `node` settled on.
fn branch_of(world: &World, run: &str, node: &str) -> String {
    text(&settled(world, run, node)["branch"], "the branch").to_owned()
}

/// The change request number `node`'s branch holds.
fn number_of(world: &World, run: &str, node: &str) -> Value {
    change_from(world, &branch_of(world, run, node))["number"].clone()
}

/// Case 2: `a, c, d → e` stacks `e` on a three-parent stack base. A person merges `a`,
/// and after a readjustment the stack base carries the root's head merged in ahead of
/// `c` and `d`, `e`'s branch carries it, and `e`'s diff against the stack base is only
/// its own. A person merges `c`: `d`'s branch carries the root's head, `e` is moved onto
/// `d`'s branch — its change request retargeted there — with only its own change
/// against it. A person merges `d`: `e` targets the root, with only its own change.
/// `e` is a draft throughout, and its record, `status` and Stack block name its base at
/// every step.
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one journey's three landings, each held where it happens"
)]
fn one_of_several_parents_landing_advances_the_stack_base_then_moves_the_draft() {
    let world = team_world("readjust-several");
    for node in ["a", "c", "d", "e"] {
        world.script(&format!("{node}.work"), &format!("{node}'s change\n"));
    }
    let run = "several";
    launched(
        &world,
        run,
        vec![
            lifecycle("a", &[]),
            lifecycle("c", &[]),
            lifecycle("d", &[]),
            lifecycle("e", &["a", "c", "d"]),
        ],
    );
    let e = settled(&world, run, "e");
    let stack_base = text(&e["stack"]["stack_base"], "e's stack base").to_owned();
    assert_eq!(e["base"], stack_base.as_str(), "{e}");
    let e_branch = branch_of(&world, run, "e");

    // `a` lands: the stack base advances over the root, and `e` merges it.
    merge(&world, &number_of(&world, run, "a"), "squash", "kept");
    let root = tip(&world, "main");
    let base_before = tip(&world, &stack_base);
    readjusted(&world, run);
    let base_now = tip(&world, &stack_base);
    assert!(is_ancestor(&world, &root, &base_now));
    // The first commit the advance put on the stack base merges the root, ahead of
    // `c`'s and `d`'s new heads.
    let advanced = git(
        &world,
        &service(&world).origin,
        &[
            "rev-list",
            "--first-parent",
            "--reverse",
            &format!("{base_before}..{base_now}"),
        ],
    );
    let first = advanced
        .lines()
        .next()
        .expect("the stack base advanced")
        .trim();
    assert!(is_ancestor(&world, &root, first));
    for parent in ["c", "d"] {
        let head = tip(&world, &branch_of(&world, run, parent));
        assert!(
            !is_ancestor(&world, &head, first),
            "{parent}'s new head was merged before the root"
        );
    }
    assert!(is_ancestor(&world, &base_now, &tip(&world, &e_branch)));
    assert_eq!(
        changed_against(&world, &service(&world), &stack_base, &e_branch),
        ["e.md"]
    );
    let now = settled(&world, run, "e");
    assert_eq!(now["base"], stack_base.as_str(), "{now}");
    assert_eq!(
        now["stack"]["parents"].as_array().map(|parents| parents
            .iter()
            .map(|parent| parent["node"].clone())
            .collect::<Vec<_>>()),
        Some(vec![json!("c"), json!("d")]),
        "{now}"
    );
    let (base, body) = on_host(&world, run, "e");
    assert_eq!(base, stack_base);
    assert!(
        body.contains(&format!("stack base `{stack_base}`")) && !body.contains("`a` ("),
        "e's Stack block does not name its two remaining parents:\n{body}"
    );

    // `c` lands: `d` takes the root's head, and `e` moves onto `d`'s branch.
    merge(&world, &number_of(&world, run, "c"), "squash", "kept");
    let root = tip(&world, "main");
    readjusted(&world, run);
    let d_branch = branch_of(&world, run, "d");
    assert!(is_ancestor(&world, &root, &tip(&world, &d_branch)));
    let now = settled(&world, run, "e");
    assert_eq!(now["base"], d_branch.as_str(), "{now}");
    let (base, body) = on_host(&world, run, "e");
    assert_eq!(base, d_branch);
    assert!(
        body.contains(&format!("Drafted against `{d_branch}`, the branch of `d`")),
        "{body}"
    );
    assert_eq!(
        changed_against(&world, &service(&world), &d_branch, &e_branch),
        ["e.md"]
    );
    let (status, _) = views(&world, run);
    assert!(
        status.contains(&format!("drafted against d's branch {d_branch}")),
        "{status}"
    );

    // `d` lands: `e` targets the root.
    merge(&world, &number_of(&world, run, "d"), "squash", "kept");
    readjusted(&world, run);
    let now = settled(&world, run, "e");
    assert_eq!(now["base"], "main", "{now}");
    let (base, body) = on_host(&world, run, "e");
    assert_eq!(base, "main");
    assert!(body.contains("Drafted against `main`, the root"), "{body}");
    assert_eq!(
        changed_against(&world, &service(&world), "main", &e_branch),
        ["e.md"]
    );
    let (status, _) = views(&world, run);
    assert!(
        status.contains("e: readjusted onto the root main"),
        "{status}"
    );
}

/// Case 3: over `a ← b ← c ← d` and `b, x → y`, a person merges `a` and then `b` — each
/// head deleted, and the host retargeting what was based on it — with no trigger run
/// between. One `onepipeline readjust` leaves `c` on the root, `d` on `c`'s branch and
/// `y` on `x`'s branch, with `x`'s branch carrying the root's head; `c`'s push is
/// journalled before `d`'s restack starts; every remaining draft's diff against its base
/// is its own and every one is a draft; nothing targets `a`'s or `b`'s branch on the
/// host; and the origin took no forced or non-fast-forward push.
#[test]
fn a_whole_lower_stack_landing_moves_every_draft_above_it_in_one_readjustment() {
    let world = team_world("readjust-lower");
    for node in ["a", "b", "c", "d", "x", "y"] {
        world.script(&format!("{node}.work"), &format!("{node}'s change\n"));
    }
    let run = "lower";
    launched(
        &world,
        run,
        vec![
            lifecycle("a", &[]),
            lifecycle("b", &["a"]),
            lifecycle("c", &["b"]),
            lifecycle("d", &["c"]),
            lifecycle("x", &[]),
            lifecycle("y", &["b", "x"]),
        ],
    );
    let branch = |node: &str| branch_of(&world, run, node);
    merge(
        &world,
        &number_of(&world, run, "a"),
        "squash",
        "deleted-retarget",
    );
    merge(
        &world,
        &number_of(&world, run, "b"),
        "squash",
        "deleted-retarget",
    );
    let root = tip(&world, "main");
    readjusted(&world, run);

    for (node, base) in [
        ("c", "main".to_owned()),
        ("d", branch("c")),
        ("y", branch("x")),
    ] {
        let now = settled(&world, run, node);
        assert_eq!(now["base"], base.as_str(), "{node}: {now}");
        let (on, _) = on_host(&world, run, node);
        assert_eq!(on, base, "{node}");
        assert_eq!(
            changed_against(&world, &service(&world), &base, &branch(node)),
            [format!("{node}.md")],
            "{node}"
        );
    }
    let (x_base, _) = on_host(&world, run, "x");
    assert_eq!(x_base, "main");
    assert!(is_ancestor(&world, &root, &tip(&world, &branch("x"))));
    assert_eq!(
        changed_against(&world, &service(&world), "main", &branch("x")),
        ["x.md"]
    );
    for change in world.changes_opened() {
        assert!(
            change["base"] != branch("a").as_str() && change["base"] != branch("b").as_str(),
            "a change request still targets a landed branch: {change}"
        );
    }

    let journal = world.journal(run);
    let c_head = text(&settled(&world, run, "c")["head"], "c's head").to_owned();
    let c_pushed = journal
        .iter()
        .position(|event| {
            event["kind"] == "push"
                && event["labels"]["node"] == "c"
                && event["payload"]["head"] == c_head.as_str()
        })
        .expect("c's readjustment pushed");
    let d_started = journal
        .iter()
        .position(|event| event["kind"] == "node-restack-started" && event["labels"]["node"] == "d")
        .expect("d was restacked");
    assert!(c_pushed < d_started, "d was restacked before c pushed");
    assert_eq!(journal[d_started]["payload"]["cause"], "readjust");
    for push in world.events_of(run, "push") {
        assert_eq!(push["payload"]["accepted"], true, "{push}");
        assert!(
            !text(&push["payload"]["output"], "push output").contains("forced update"),
            "{push}"
        );
    }
    for node in ["a", "b"] {
        let landed = record(&world, run, "node-landed", node).expect("landed");
        assert_eq!(landed["detected_by"], "readjust", "{landed}");
    }
}

/// What a draft of the `other` repository looks like from every side a failed read must
/// leave alone: its branch's head on that origin, its change request's base and description on the host, and how
/// many records the run holds about it.
fn untouched(world: &World, run: &str, node: &str) -> (String, String, String, usize) {
    let branch = branch_of(world, run, node);
    let change = change_from(world, &branch);
    let origin = world.root.join("other.git");
    let records = world
        .journal(run)
        .iter()
        .filter(|event| event["labels"]["node"] == node)
        .count();
    (
        git(world, &origin, &["rev-parse", &branch])
            .trim()
            .to_owned(),
        text(&change["base"], "the base").to_owned(),
        text(&change["body"], "the description").to_owned(),
        records,
    )
}

/// A failed read: over two repositories, the standings read of `other` fails on the host
/// while parents have landed in both. `onepipeline readjust` journals
/// `readjust-read-failed` naming `other`, leaves `other`'s drafts exactly as they were —
/// heads, bases, records and Stack blocks — and still moves `service`'s draft onto the
/// root, exiting with the status a failed host read is given. Once the host answers
/// again, the next trigger readjusts `other`'s drafts too.
#[test]
fn a_repository_whose_read_failed_is_left_as_it_was_and_readjusted_by_the_next_trigger() {
    let world = team_world("readjust-failed-read");
    world.extra_repository("other");
    for node in ["a", "b", "p", "q"] {
        world.script(&format!("{node}.work"), &format!("{node}'s change\n"));
    }
    let on_other = |id: &str, deps: &[&str]| {
        let mut node = lifecycle(id, deps);
        node["repo"] = json!("github.com/owner/other");
        node
    };
    let run = "failed";
    launched(
        &world,
        run,
        vec![
            lifecycle("a", &[]),
            lifecycle("b", &["a"]),
            on_other("p", &[]),
            on_other("q", &["p"]),
        ],
    );
    let p_change = number_of(&world, run, "p");
    merge(&world, &number_of(&world, run, "a"), "squash", "kept");
    person(
        &world,
        &[
            "merge",
            "owner/other",
            &p_change.to_string(),
            "squash",
            "kept",
        ],
    );
    let before = untouched(&world, run, "q");
    world.script("gh.standings-outage.owner.other", "");

    let (code, receipt) = readjust(&world, run);
    assert_eq!(code, 1, "a failed host read is not said: {receipt}");
    assert_eq!(receipt["state"], "performed", "{receipt}");
    let failed = world.events_of(run, "readjust-read-failed");
    assert_eq!(failed.len(), 1, "{failed:#?}");
    assert_eq!(
        failed[0]["payload"]["repo"], "github.com/owner/other",
        "{failed:#?}"
    );
    assert_eq!(untouched(&world, run, "q"), before);
    assert!(record(&world, run, "node-landed", "p").is_none());
    assert_eq!(settled(&world, run, "b")["base"], "main");
    let (base, _) = on_host(&world, run, "b");
    assert_eq!(base, "main");

    std::fs::remove_file(world.fakes.join("gh.standings-outage.owner.other"))
        .expect("the outage ends");
    readjusted(&world, run);
    assert!(record(&world, run, "node-landed", "p").is_some());
    assert_eq!(settled(&world, run, "q")["base"], "main");
    let (base, body) = on_host(&world, run, "q");
    assert_eq!(base, "main");
    assert!(body.contains("Drafted against `main`, the root"), "{body}");
}

/// Reply to `run` with one envelope of `commands`, as the planner.
fn reply(world: &World, run: &str, commands: Value) -> crate::harness::Run {
    world.run_with_stdin(
        &["reply", run],
        &json!({"version": onepipeline::channel::REPLY_ENVELOPE_VERSION, "commands": commands})
            .to_string(),
    )
}

/// A `rework` of `id`, as a planner relays a reviewer's request.
fn rework(id: &str) -> Value {
    json!({
        "op": "rework",
        "id": id,
        "reason": "a reviewer asked for it on the draft",
        "instruction": "say what the review asked",
        "comments": [{"id": "101", "url": "https://github.com/owner/service/pull/1#discussion_r101"}],
    })
}

/// `onepipeline review RUN --json`, its receipt parsed.
fn review(world: &World, run: &str) -> Value {
    let output = world.run(&["review", run, "--json"]);
    assert_eq!(output.code, 0, "{}\n{}", output.stdout, output.stderr);
    serde_json::from_str(&output.stdout).expect("a receipt")
}

/// Whether `call` reads a change request's check rollup or a base's required checks —
/// what a publication's check watch asks.
fn is_check_read(call: &Value) -> bool {
    let args: Vec<&str> = call["args"]
        .as_array()
        .map(|args| args.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    (args.first() == Some(&"pr") && args.get(1) == Some(&"checks"))
        || (args.first() == Some(&"pr")
            && args.get(1) == Some(&"view")
            && args.iter().any(|arg| arg.contains("statusCheckRollup")))
        || args
            .iter()
            .any(|arg| arg.contains("/protection/required_status_checks"))
}

/// The outcome `results --json` reports for `node`: its settlement's word, as a later
/// verdict read moved it.
fn outcome_of(world: &World, run: &str, node: &str) -> Value {
    let output = world.run(&["results", run, "--json"]);
    output.exited(0);
    let results: Value = serde_json::from_str(&output.stdout).expect("results as JSON");
    results["nodes"]
        .as_array()
        .and_then(|nodes| nodes.iter().find(|entry| entry["id"] == node))
        .map(|entry| entry["outcome"].clone())
        .unwrap_or_else(|| panic!("results names no {node}: {results}"))
}

/// How many change requests the host was asked to open from `branch`.
fn opened_from(world: &World, branch: &str) -> usize {
    world
        .changes_opened()
        .iter()
        .filter(|change| change["head"] == branch)
        .count()
}

/// Deferred verdicts: a rework's republication, a restack's and a readjustment's each
/// settle `change-review-draft-unchecked`, and nothing reads a check rollup or a base's
/// required checks between their pushes and the next trigger — while every node's
/// first publication still watches its checks. The next `onepipeline review` records
/// each verdict: a green draft moves to `change-review-draft`, and one read red is
/// re-dispatched onto its same branch with the failed checks as its refusal, opening no
/// change request. Until each is read, `status`, `results` and the pass's output name
/// it as outstanding.
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one journey's three republications and the two reads that answer them"
)]
fn deferred_republications_are_read_by_the_next_trigger_green_or_repaired_red() {
    let world = team_world("readjust-verdicts");
    world.script("a.work", "a's change\n");
    world.script("b.work-anew", "b's change");
    world.script("x.work", "x's change\n");
    world.script("y.work", "y's change\n");
    world.script("a-rework-1.work", "a's change, as the review asked\n");
    world.script("a-rework-1.work-file", "a");
    let run = "verdicts";
    launched(
        &world,
        run,
        vec![
            lifecycle("a", &[]),
            lifecycle("b", &["a"]),
            lifecycle("x", &[]),
            lifecycle("y", &["x"]),
        ],
    );
    assert!(
        host_calls(&world, 0).iter().any(is_check_read),
        "a first publication did not watch its checks"
    );
    let from = world.invocations().len();

    // A rework's republication, and the restack it owes the draft above it.
    reply(&world, run, json!([rework("a")])).exited(0);
    world.run(&["adopt", run]).exited(0);
    for node in ["a-rework-1", "b"] {
        assert_eq!(
            settled(&world, run, node)["outcome"],
            "change-review-draft-unchecked",
            "{node}"
        );
    }
    let b_branch = branch_of(&world, run, "b");
    let b_head = text(&settled(&world, run, "b")["head"], "b's head").to_owned();
    person(&world, &["verdict", &b_head, "failed:lint"]);

    // A readjustment's republication, made by the next review's own readjustment.
    merge(&world, &number_of(&world, run, "x"), "squash", "kept");
    let before_review = world.invocations().len();
    let watched: Vec<Value> = world.invocations()[from..before_review]
        .iter()
        .filter(|call| call["tool"] == "gh" && is_check_read(call))
        .cloned()
        .collect();
    assert!(
        watched.is_empty(),
        "a republication watched its checks: {watched:#?}"
    );
    let (status, results) = views(&world, run);
    for node in ["a-rework-1", "b"] {
        assert!(
            status.contains(&format!("{node}: check verdict outstanding")),
            "{node}:\n{status}"
        );
    }
    assert!(results.contains("check verdict outstanding"), "{results}");

    let receipt = review(&world, run);
    let read = |node: &str| record(&world, run, "node-checks-read", node);
    assert_eq!(read("a-rework-1").expect("read")["verdict"], "passed");
    assert_eq!(read("b").expect("read")["verdict"], "failed");
    assert_eq!(outcome_of(&world, run, "a-rework-1"), "change-review-draft");
    assert_eq!(
        settled(&world, run, "y")["outcome"],
        "change-review-draft-unchecked",
        "y's readjustment republished without a watch"
    );
    // The red draft was dispatched again on its own branch, its refusal the checks
    // that failed, and published onto the change request it already had.
    let redispatched = world
        .events_of(run, "node-dispatched")
        .into_iter()
        .filter(|event| event["labels"]["node"] == "b")
        .last()
        .expect("b was dispatched");
    assert_eq!(redispatched["payload"]["attempt"], 2, "{redispatched}");
    assert!(
        redispatched["payload"]["reason"]
            .as_str()
            .is_some_and(|reason| reason.starts_with("checks-failed") && reason.contains("lint")),
        "{redispatched}"
    );
    let repaired = settled(&world, run, "b");
    assert_eq!(repaired["branch"], b_branch.as_str(), "{repaired}");
    assert_eq!(
        repaired["outcome"], "change-review-draft-unchecked",
        "{repaired}"
    );
    assert_ne!(repaired["head"], b_head.as_str(), "{repaired}");
    assert_eq!(opened_from(&world, &b_branch), 1);
    let outstanding: Vec<&str> = receipt["outstanding"]
        .as_array()
        .map(|nodes| nodes.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    assert!(
        outstanding.contains(&"y") && outstanding.contains(&"b"),
        "{receipt}"
    );
    let readjust_line = world.run(&["readjust", run]);
    readjust_line.exited(0);
    assert!(
        readjust_line.stdout.contains("readjust performed")
            && !readjust_line.stdout.contains("outstanding"),
        "the readjustment read the outstanding verdicts, so none is left: {}",
        readjust_line.stdout
    );
    assert_eq!(read("y").expect("read")["verdict"], "passed");
    assert_eq!(outcome_of(&world, run, "y"), "change-review-draft");
    assert_eq!(outcome_of(&world, run, "b"), "change-review-draft");
}

/// Every node id the write-back's landed projections carried since the first `from`
/// records.
fn projected_since(world: &World, run: &str, from: usize) -> Vec<String> {
    crate::linear_writeback::projection_records(world, run)
        .into_iter()
        .skip(from)
        .filter(|record| record["ended"]["state"] != "failed")
        .flat_map(|record| {
            record["items"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
        })
        .collect()
}

/// How many projection records the run holds.
fn projections(world: &World, run: &str) -> usize {
    crate::linear_writeback::projection_records(world, run).len()
}

/// Store writes: on a local Markdown store, a rework of `b` in `a ← b ← c ← d` restacks
/// `c` and `d` with no conflict, and the restacks write no task for either while the
/// rework writes `b`'s; a readjustment after `a` lands writes `a`, landed, and `b`, on
/// its new base, and no other task; a cold host continuing from that store places a
/// node added on `d` on `d`'s branch as it stands on the origin now; and a red verdict
/// read later writes that node's repair.
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one journey across the triggers whose writes the store is held to"
)]
fn the_store_is_written_only_for_what_its_projection_holds_changed() {
    let world = team_world("readjust-store");
    for node in ["a", "b", "c"] {
        world.script(&format!("{node}.work"), &format!("{node}'s change\n"));
    }
    world.script("d.work-anew", "d's change");
    world.script("b-rework-1.work", "b's change, as the review asked\n");
    world.script("b-rework-1.work-file", "b");
    let run = "store";
    let project = launched(
        &world,
        run,
        vec![
            lifecycle("a", &[]),
            lifecycle("b", &["a"]),
            lifecycle("c", &["b"]),
            lifecycle("d", &["c"]),
        ],
    );
    world.until_store("every settlement to reach the store", |world| {
        projected_settlement(world, &project, "d")["status"] == "done"
    });

    // The rework writes b's lineage; its restacks of c and d write nothing.
    let mark = projections(&world, run);
    reply(&world, run, json!([rework("b")])).exited(0);
    world.run(&["adopt", run]).exited(0);
    for node in ["c", "d"] {
        assert!(
            record(&world, run, "node-restacked", node)
                .is_some_and(|restacked| restacked["agent_dispatched"] == false),
            "{node} was not restacked by a merge alone"
        );
    }
    world.until_store("the rework to reach the store", |world| {
        world
            .store_tasks(&project)
            .iter()
            .any(|task| task["item"]["metadata"]["onepipeline.rework"]["replaces"] == "b")
    });
    let written = projected_since(&world, run, mark);
    assert!(
        written.iter().any(|item| item.starts_with('b')),
        "the rework wrote no task for b: {written:?}"
    );
    for node in ["c", "d"] {
        assert!(
            !written.contains(&node.to_owned()),
            "the restack wrote {node}'s task: {written:?}"
        );
    }

    // A readjustment after `a` lands writes `a` and `b`, and nothing else.
    merge(&world, &number_of(&world, run, "a"), "squash", "kept");
    let mark = projections(&world, run);
    readjusted(&world, run);
    world.until_store("b's new base to reach the store", |world| {
        world.store_tasks(&project).iter().any(|task| {
            task["item"]["metadata"]["onepipeline.settlement"]["base"] == "main"
                && task["item"]["metadata"]["onepipeline.settlement"]["change_url"]
                    == settled(world, run, "b-rework-1")["change_url"]
        })
    });
    let written = projected_since(&world, run, mark);
    assert!(written.contains(&"a".to_owned()), "{written:?}");
    assert!(
        written.iter().any(|item| item.starts_with('b')),
        "{written:?}"
    );
    for node in ["c", "d"] {
        assert!(
            !written.contains(&node.to_owned()),
            "the readjustment wrote {node}'s task: {written:?}"
        );
    }
    let d_branch = branch_of(&world, run, "d");
    let d_tip = tip(&world, &d_branch);
    assert_ne!(
        projected_settlement(&world, &project, "d")["head"],
        d_tip.as_str(),
        "d's restacked head reached the store"
    );

    // A cold host continuing from the store reads `d`'s verdict for the head its branch
    // carries on the origin — the store holds neither that head nor its unread verdict —
    // and the red one it reads is repaired, which writes `d`'s task.
    person(&world, &["verdict", &d_tip, "failed:lint"]);
    let cold = world.on_host("readjust-store-cold");
    let mark = 0;
    cold.run_from(&cold.project, &["start", &project, "--attach"])
        .exited(0);
    let read = record(&cold, run, "node-checks-read", "d").expect("d's verdict was read");
    assert_eq!(read["head"], d_tip.as_str(), "{read}");
    assert_eq!(read["verdict"], "failed", "{read}");
    cold.until_store("d's repair to reach the store", |world| {
        let head = projected_settlement(world, &project, "d")["head"].clone();
        head.as_str()
            .is_some_and(|head| head != d_tip && is_ancestor(world, &d_tip, head))
    });
    assert!(
        projected_since(&cold, run, mark).contains(&"d".to_owned()),
        "the repair wrote no task for d: {:#?}",
        crate::linear_writeback::projection_records(&cold, run)
            .into_iter()
            .skip(mark)
            .collect::<Vec<_>>()
    );

    // A node added on `d` is placed on `d`'s branch as the origin holds it now.
    let d_now = tip(&world, &d_branch);
    cold.script("e.work", "e's change\n");
    reply(
        &cold,
        run,
        json!([{"op": "add", "node": lifecycle("e", &["d"])}]),
    )
    .exited(0);
    cold.run(&["adopt", run]).exited(0);
    let e = settled(&cold, run, "e");
    assert_eq!(e["base"], d_branch.as_str(), "{e}");
    assert!(is_ancestor(&world, &d_now, text(&e["head"], "e's head")));
}

/// Every remaining draft's record, `status`, `results`, store settlement and Stack block
/// name the base `bases` gives it.
fn every_draft_names_its_base(world: &World, run: &str, project: &str, bases: &[(&str, &str)]) {
    let (status, results) = views(world, run);
    for (node, base) in bases {
        let settlement = settled(world, run, node);
        assert_eq!(settlement["base"], *base, "{node}: {settlement}");
        let (on, body) = on_host(world, run, node);
        assert_eq!(on, *base, "{node}");
        let named = if *base == "main" {
            "Drafted against `main`, the root".to_owned()
        } else {
            format!("Drafted against `{base}`")
        };
        assert!(body.contains(&named), "{node}'s Stack block:\n{body}");
        let against = if *base == "main" {
            "drafted against the root main".to_owned()
        } else {
            format!("branch {base}")
        };
        assert!(
            status
                .lines()
                .any(|line| line.starts_with(&format!("  {node}: ")) && line.contains(&against)),
            "{node} in status:\n{status}"
        );
        assert!(
            results
                .lines()
                .any(|line| line.trim_start().starts_with(&format!("{node} "))
                    && line.contains(&against)),
            "{node} in results:\n{results}"
        );
        let lineage = node.split("-rework-").next().unwrap_or(node);
        world.until_store(&format!("{node}'s base to reach the store"), |world| {
            projected_settlement(world, project, lineage)["base"] == *base
        });
    }
}

/// The branch the newest node of `node`'s lineage settled on, and the commit it stands at.
fn wait_for_hold(world: &World, run: &str, node: &str) {
    world.until(&format!("{node} to be dispatched and held"), |world| {
        world
            .events_of(run, "node-dispatched")
            .iter()
            .any(|event| event["labels"]["node"] == node)
            && world.fakes.join(format!("{node}.wait")).exists()
    });
}

/// Case 4(i): after `a` in `a ← b ← c` lands with no trigger run, a `rework` of `b`
/// places its replacement on the root, publishes onto `b`'s existing change request —
/// one opened from `b`'s branch across the journey — retargets it to the root, keeps it
/// a draft with only `b`'s change against it, and restacks `c` onto `b`'s new head.
#[test]
fn a_rework_after_a_landing_places_its_replacement_on_the_root() {
    let world = team_world("readjust-rework-after");
    for node in ["a", "b", "c"] {
        world.script(&format!("{node}.work"), &format!("{node}'s change\n"));
    }
    world.script("b-rework-1.work", "b's change, as the review asked\n");
    world.script("b-rework-1.work-file", "b");
    let run = "after";
    let project = launched(
        &world,
        run,
        vec![
            lifecycle("a", &[]),
            lifecycle("b", &["a"]),
            lifecycle("c", &["b"]),
        ],
    );
    let b_branch = branch_of(&world, run, "b");
    merge(&world, &number_of(&world, run, "a"), "squash", "kept");
    reply(&world, run, json!([rework("b")])).exited(0);
    world.run(&["adopt", run]).exited(0);

    found_first(
        &world,
        run,
        "a",
        "rework",
        ("node-dispatched", "b-rework-1"),
    );
    let replacement = settled(&world, run, "b-rework-1");
    assert_eq!(replacement["branch"], b_branch.as_str(), "{replacement}");
    assert_eq!(replacement["base"], "main", "{replacement}");
    assert_eq!(opened_from(&world, &b_branch), 1);
    assert_eq!(
        changed_against(&world, &service(&world), "main", &b_branch),
        ["b.md"]
    );
    let moved = record(&world, run, "node-readjusted", "b-rework-1").expect("readjusted");
    let trail: Vec<String> = world
        .journal(run)
        .iter()
        .filter(|event| {
            event["labels"]["node"]
                .as_str()
                .is_some_and(|node| node.starts_with('b') || node == "c")
        })
        .map(|event| {
            format!(
                "{} {} {}",
                event["labels"]["node"], event["kind"], event["payload"]["base"]
            )
        })
        .collect();
    assert_eq!(moved["retarget"], "retargeted", "{moved}\n{trail:#?}");
    assert_eq!(moved["cause"], json!(["a"]), "{moved}");
    let b_head = tip(&world, &b_branch);
    assert!(is_ancestor(
        &world,
        &b_head,
        &tip(&world, &branch_of(&world, run, "c"))
    ));
    every_draft_names_its_base(
        &world,
        run,
        &project,
        &[("b-rework-1", "main"), ("c", b_branch.as_str())],
    );
}

/// Case 4(ii): `a` lands while `c`'s rework replacement is in flight. A readjustment
/// made during it moves `b` and leaves `c` alone; once `c`'s replacement settles it is
/// readjusted onto `b`'s new head.
#[test]
fn a_landing_during_a_rework_readjusts_the_rework_once_it_settles() {
    let world = team_world("readjust-during-rework");
    for node in ["a", "b", "c"] {
        world.script(&format!("{node}.work"), &format!("{node}'s change\n"));
    }
    world.script("c-rework-1.work", "c's change, as the review asked\n");
    world.script("c-rework-1.work-file", "c");
    world.script("c-rework-1.wait", "");
    let run = "during";
    let mut plan = plan_of(
        run,
        vec![
            lifecycle("a", &[]),
            lifecycle("b", &["a"]),
            lifecycle("c", &["b"]),
        ],
    );
    plan["concurrency"] = json!(2);
    let project = world.plan(run, &plan);
    world
        .run_from(&world.project, &["start", &project, "--attach"])
        .exited(0);
    let b_branch = branch_of(&world, run, "b");
    let c_branch = branch_of(&world, run, "c");
    reply(&world, run, json!([rework("c")])).exited(0);
    let driver = world
        .cmd(&["adopt", run])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the adopting driver starts");
    wait_for_hold(&world, run, "c-rework-1");

    merge(&world, &number_of(&world, run, "a"), "squash", "kept");
    readjusted(&world, run);
    world.until("b to be readjusted", |world| {
        record(world, run, "node-readjusted", "b").is_some()
    });
    assert!(record(&world, run, "node-readjusted", "c-rework-1").is_none());
    assert!(record(&world, run, "node-restack-started", "c-rework-1").is_none());

    std::fs::write(world.fakes.join("c-rework-1.go"), "").expect("the hold is released");
    let ended = driver.wait_with_output().expect("the driver ends");
    assert!(ended.status.success(), "{ended:?}");
    let readjusted_c =
        record(&world, run, "node-readjusted", "c-rework-1").expect("c's rework was readjusted");
    assert_eq!(
        readjusted_c["to"]["base"],
        b_branch.as_str(),
        "{readjusted_c}"
    );
    assert!(is_ancestor(
        &world,
        &tip(&world, &b_branch),
        &tip(&world, &c_branch)
    ));
    let journal = world.journal(run);
    let c_settled = journal
        .iter()
        .position(|event| {
            event["kind"] == "node-settled" && event["labels"]["node"] == "c-rework-1"
        })
        .expect("c's rework settled");
    let c_restacked = journal
        .iter()
        .position(|event| {
            event["kind"] == "node-restack-started" && event["labels"]["node"] == "c-rework-1"
        })
        .expect("c's rework was restacked");
    assert!(c_settled < c_restacked);
    assert_eq!(
        changed_against(&world, &service(&world), &b_branch, &c_branch),
        ["c.md"]
    );
    every_draft_names_its_base(
        &world,
        run,
        &project,
        &[("b", "main"), ("c-rework-1", b_branch.as_str())],
    );
}

/// Case 4(iii) and the amendment's merged-after-dispatch refusal: `a`'s change request
/// is merged while `a`'s own rework is in flight. The replacement's republication reads
/// the standing first, finds it merged, and is refused before anything is pushed onto
/// the change request: it opens none, settles `failed` with `merged-during-rework`, its
/// work kept on its branch, and a surface names `add`. `b` above it is readjusted onto
/// the root.
#[test]
fn a_change_merged_during_its_own_rework_is_refused_before_the_push() {
    let world = team_world("readjust-merged-during");
    for node in ["a", "b"] {
        world.script(&format!("{node}.work"), &format!("{node}'s change\n"));
    }
    world.script("a-rework-1.work", "a's change, as the review asked\n");
    world.script("a-rework-1.work-file", "a");
    world.script("a-rework-1.wait", "");
    let run = "merged";
    let project = launched(
        &world,
        run,
        vec![lifecycle("a", &[]), lifecycle("b", &["a"])],
    );
    let a_branch = branch_of(&world, run, "a");
    reply(&world, run, json!([rework("a")])).exited(0);
    let driver = world
        .cmd(&["adopt", run])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the adopting driver starts");
    wait_for_hold(&world, run, "a-rework-1");
    merge(&world, &number_of(&world, run, "a"), "squash", "kept");
    let merged_head = tip(&world, &a_branch);
    let pushes_before = world.events_of(run, "push").len();
    std::fs::write(world.fakes.join("a-rework-1.go"), "").expect("the hold is released");
    // The replacement failed, so the run waits on the planner's answer: unattended.
    let ended = driver.wait_with_output().expect("the driver ends");
    assert_eq!(ended.status.code(), Some(3), "{ended:?}");

    let replacement = settled(&world, run, "a-rework-1");
    assert_eq!(replacement["status"], "failed", "{replacement}");
    assert_eq!(
        replacement["outcome"], "merged-during-rework",
        "{replacement}"
    );
    assert_eq!(opened_from(&world, &a_branch), 1);
    // Nothing was published onto the merged change request: the only push since is
    // the work kept on its branch.
    let published: Vec<Value> = world
        .events_of(run, "change-published")
        .into_iter()
        .filter(|event| event["labels"]["node"] == "a-rework-1")
        .collect();
    assert!(published.is_empty(), "{published:#?}");
    let kept = text(&replacement["head"], "the kept head");
    assert!(is_ancestor(&world, &merged_head, kept));
    assert!(world.events_of(run, "push").len() >= pushes_before);
    let surfaced: Vec<Value> = world
        .events_of(run, "planner-surface-queued")
        .into_iter()
        .filter(|event| event["payload"]["kind"] == "merged-during-rework")
        .collect();
    assert_eq!(surfaced.len(), 1, "{surfaced:#?}");
    assert!(
        surfaced[0]["payload"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("'a") && message.contains("`add`")),
        "{surfaced:#?}"
    );
    every_draft_names_its_base(&world, run, &project, &[("b", "main")]);
}

/// Case 4(iv): a `rework` of a node whose change request was merged since the last
/// trigger is refused naming `add` — the rework's own readjustment finds it landed first.
#[test]
fn a_rework_of_a_node_merged_since_the_last_trigger_is_refused_naming_add() {
    let world = team_world("readjust-rework-landed");
    for node in ["a", "b"] {
        world.script(&format!("{node}.work"), &format!("{node}'s change\n"));
    }
    let run = "landed";
    let project = launched(
        &world,
        run,
        vec![lifecycle("a", &[]), lifecycle("b", &["a"])],
    );
    merge(&world, &number_of(&world, run, "a"), "squash", "kept");
    let refused = reply(&world, run, json!([rework("a")]));
    world.run(&["adopt", run]).exited(0);
    let rejected: Vec<Value> = world
        .events_of(run, "edit-rejected")
        .into_iter()
        .chain(world.events_of(run, "command-rejected"))
        .collect();
    let said = format!("{}{}{rejected:?}", refused.stdout, refused.stderr);
    assert!(
        said.contains("landed") && said.contains("`add`"),
        "the rework was not refused naming add: {said}"
    );
    assert!(record(&world, run, "node-landed", "a").is_some());
    assert!(world
        .events_of(run, "node-dispatched")
        .iter()
        .all(|event| event["labels"]["node"] != "a-rework-1"));
    every_draft_names_its_base(&world, run, &project, &[("b", "main")]);
}

/// That the engine asked the host to merge, close or lift nothing since `from`, and
/// pushed nothing onto the root.
fn the_engine_merged_closed_and_lifted_nothing(world: &World, run: &str, from: usize) {
    for call in host_calls(world, from) {
        let args: Vec<&str> = call["args"]
            .as_array()
            .map(|args| args.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        assert!(
            !(args.first() == Some(&"pr")
                && matches!(args.get(1), Some(&"merge" | &"ready" | &"close"))),
            "the engine asked the host to {:?}: {call}",
            args.get(1)
        );
        assert!(
            !args.contains(&"state=closed"),
            "the engine closed a change request: {call}"
        );
    }
    for push in world.events_of(run, "push") {
        assert!(
            push["payload"]["branch"] != "main" && push["payload"]["ref"] != "refs/heads/main",
            "the engine pushed onto the root: {push}"
        );
    }
}

/// The non-blocking surfaces of `kind` the run raised.
fn surfaced(world: &World, run: &str, kind: &str) -> Vec<Value> {
    world
        .events_of(run, "planner-surface-queued")
        .into_iter()
        .filter(|event| event["payload"]["kind"] == kind)
        .map(|event| event["payload"].clone())
        .collect()
}

/// Case 5(a): a person merges `b`'s change request into `a`'s branch in `a ← b ← c`.
/// `b` is absorbed and surfaced, not landed; `c` is placed on `a`'s branch with only its
/// own change against it; a `rework` of `b` is refused naming `a`; and once `a` lands,
/// `b` reads landed via `a` and `c` is on the root.
#[test]
fn a_draft_merged_into_its_parent_s_branch_is_absorbed_and_lands_with_it() {
    let world = team_world("readjust-absorbed");
    for node in ["a", "b", "c"] {
        world.script(&format!("{node}.work"), &format!("{node}'s change\n"));
    }
    let run = "absorbed";
    launched(
        &world,
        run,
        vec![
            lifecycle("a", &[]),
            lifecycle("b", &["a"]),
            lifecycle("c", &["b"]),
        ],
    );
    let a_branch = branch_of(&world, run, "a");
    let c_branch = branch_of(&world, run, "c");
    let from = world.invocations().len();
    merge(&world, &number_of(&world, run, "b"), "merge-commit", "kept");
    readjusted(&world, run);

    let absorbed = record(&world, run, "node-absorbed", "b").expect("b was absorbed");
    assert_eq!(absorbed["into"], "a", "{absorbed}");
    assert!(record(&world, run, "node-landed", "b").is_none());
    assert_eq!(surfaced(&world, run, "stack-merged-out-of-order").len(), 1);
    assert_eq!(settled(&world, run, "c")["base"], a_branch.as_str());
    let (base, _) = on_host(&world, run, "c");
    assert_eq!(base, a_branch);
    assert_eq!(
        changed_against(&world, &service(&world), &a_branch, &c_branch),
        ["c.md"]
    );
    let (status, _) = views(&world, run);
    assert!(
        status.contains("b: merged into a's branch, and lands only with it"),
        "{status}\n{:#?}",
        world
            .journal(run)
            .iter()
            .filter(|event| event["labels"]["node"] == "b")
            .map(|event| format!("{} {}", event["kind"], event["payload"]))
            .collect::<Vec<_>>()
    );

    let refused = reply(&world, run, json!([rework("b")]));
    assert_ne!(refused.code, 0, "{}", refused.stdout);
    let said = format!("{}{}", refused.stdout, refused.stderr);
    assert!(
        said.contains("merged into the branch of 'a'") && said.contains("rework 'a'"),
        "{said}"
    );

    merge(&world, &number_of(&world, run, "a"), "squash", "kept");
    readjusted(&world, run);
    let landed = record(&world, run, "node-landed", "b").expect("b landed with a");
    assert_eq!(landed["detected_by"], "absorber-landed", "{landed}");
    assert_eq!(landed["via"], "a", "{landed}");
    assert_eq!(settled(&world, run, "c")["base"], "main");
    assert_eq!(
        changed_against(&world, &service(&world), "main", &c_branch),
        ["c.md"]
    );
    the_engine_merged_closed_and_lifted_nothing(&world, run, from);
}

/// Case 5(b): a person retargets `b` to the root and merges it while `a` is unlanded.
/// `b` is landed, `stack-landed-out-of-order` names `a`, and `a` is brought up to date
/// — its branch carrying the root's head.
#[test]
fn a_draft_merged_into_the_root_ahead_of_its_parent_carries_it_and_says_so() {
    let world = team_world("readjust-ahead");
    for node in ["a", "b"] {
        world.script(&format!("{node}.work"), &format!("{node}'s change\n"));
    }
    let run = "ahead";
    launched(
        &world,
        run,
        vec![lifecycle("a", &[]), lifecycle("b", &["a"])],
    );
    let from = world.invocations().len();
    let b_number = number_of(&world, run, "b");
    person(&world, &["retarget", SLUG, &b_number.to_string(), "main"]);
    merge(&world, &b_number, "squash", "kept");
    let root = tip(&world, "main");
    readjusted(&world, run);

    assert!(record(&world, run, "node-landed", "b").is_some());
    let carried =
        record(&world, run, "stack-landed-out-of-order", "b").expect("the carry is recorded");
    assert_eq!(carried["carried"], json!(["a"]), "{carried}");
    let raised = surfaced(&world, run, "stack-landed-out-of-order");
    assert_eq!(raised.len(), 1, "{raised:#?}");
    assert!(
        raised[0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("'a'")),
        "{raised:#?}"
    );
    assert!(record(&world, run, "node-readjusted", "a").is_some());
    assert!(is_ancestor(
        &world,
        &root,
        &tip(&world, &branch_of(&world, run, "a"))
    ));
    the_engine_merged_closed_and_lifted_nothing(&world, run, from);
}

/// Case 5(c): `b`'s node failed holding an open change request when `a` lands. Its change
/// request is retargeted to the root at once with no merge, recorded with `content:
/// pending`, and `readjust-pending` is surfaced; a later `retry` merges the root in, with
/// only `b`'s change against it.
#[test]
fn a_failed_draft_is_retargeted_with_no_merge_and_a_retry_brings_it_up_to_date() {
    let world =
        team_world("readjust-failed-draft").with_env("ONEPIPELINE_PUBLICATION_ATTEMPTS", "1");
    for node in ["a", "b"] {
        world.script(&format!("{node}.work"), &format!("{node}'s change\n"));
    }
    // `b`'s change request is the second opened, and its checks fail.
    world.script("gh.checks.2", "lint completed failure required");
    let run = "failed";
    launched(
        &world,
        run,
        vec![lifecycle("a", &[]), lifecycle("b", &["a"])],
    );
    let b = settled(&world, run, "b");
    assert_eq!(b["status"], "failed", "{b}");
    let b_branch = branch_of(&world, run, "b");
    let b_head = tip(&world, &b_branch);
    let from = world.invocations().len();
    merge(&world, &number_of(&world, run, "a"), "squash", "kept");
    readjusted(&world, run);

    let pending = record(&world, run, "node-readjusted", "b").expect("b was readjusted");
    assert_eq!(pending["content"], "pending", "{pending}");
    assert_eq!(pending["agent_dispatched"], false, "{pending}");
    assert_eq!(pending["retarget"], "retargeted", "{pending}");
    assert_eq!(tip(&world, &b_branch), b_head, "b's branch was merged into");
    let base = change_from(&world, &b_branch)["base"].clone();
    assert_eq!(base, "main");
    assert_eq!(surfaced(&world, run, "readjust-pending").len(), 1);
    the_engine_merged_closed_and_lifted_nothing(&world, run, from);

    world.script("gh.checks.2", crate::draft_lifecycle::GREEN);
    reply(
        &world,
        run,
        json!([{"op": "retry", "id": "b", "node": lifecycle("b-2", &["a"])}]),
    )
    .exited(0);
    world.run(&["adopt", run]).exited(0);
    let retried = settled(&world, run, "b-2");
    assert_eq!(retried["status"], "done", "{retried}");
    assert_eq!(retried["branch"], b_branch.as_str(), "{retried}");
    assert_eq!(retried["base"], "main", "{retried}");
    assert!(is_ancestor(
        &world,
        &tip(&world, "main"),
        &tip(&world, &b_branch)
    ));
    assert_eq!(
        changed_against(&world, &service(&world), "main", &b_branch),
        ["b.md"]
    );
}

/// Case 5(d): a person retargets a draft by hand. Its next readjustment moves it back
/// onto its computed base — the record's `from` naming the base the person put it on —
/// and surfaces `change-base-overridden`.
#[test]
fn a_draft_retargeted_by_hand_is_moved_back_and_said() {
    let world = team_world("readjust-overridden");
    for node in ["a", "b"] {
        world.script(&format!("{node}.work"), &format!("{node}'s change\n"));
    }
    let run = "overridden";
    launched(
        &world,
        run,
        vec![lifecycle("a", &[]), lifecycle("b", &["a"])],
    );
    let a_branch = branch_of(&world, run, "a");
    let from = world.invocations().len();
    person(
        &world,
        &[
            "retarget",
            SLUG,
            &number_of(&world, run, "b").to_string(),
            "main",
        ],
    );
    readjusted(&world, run);
    let moved = record(&world, run, "node-readjusted", "b").expect("b was moved back");
    assert_eq!(moved["from"]["base"], "main", "{moved}");
    assert_eq!(moved["to"]["base"], a_branch.as_str(), "{moved}");
    assert_eq!(moved["retarget"], "retargeted", "{moved}");
    let (base, _) = on_host(&world, run, "b");
    assert_eq!(base, a_branch);
    assert_eq!(surfaced(&world, run, "change-base-overridden").len(), 1);
    the_engine_merged_closed_and_lifted_nothing(&world, run, from);
}

/// Case 5(e): a draft closed without merging moves nothing — the draft above it stays
/// where it is — and is surfaced.
#[test]
fn a_draft_closed_unmerged_moves_nothing_and_is_said() {
    let world = team_world("readjust-closed-unmerged");
    for node in ["a", "b"] {
        world.script(&format!("{node}.work"), &format!("{node}'s change\n"));
    }
    let run = "closed";
    launched(
        &world,
        run,
        vec![lifecycle("a", &[]), lifecycle("b", &["a"])],
    );
    let a_branch = branch_of(&world, run, "a");
    let b_head = tip(&world, &branch_of(&world, run, "b"));
    let from = world.invocations().len();
    person(
        &world,
        &["close", SLUG, &number_of(&world, run, "a").to_string()],
    );
    readjusted(&world, run);
    let closed = record(&world, run, "change-closed", "a").expect("a's close is recorded");
    assert_eq!(
        closed["change_url"],
        settled(&world, run, "a")["change_url"]
    );
    assert_eq!(surfaced(&world, run, "change-closed").len(), 1);
    assert!(record(&world, run, "node-readjusted", "b").is_none());
    assert_eq!(settled(&world, run, "b")["base"], a_branch.as_str());
    assert_eq!(tip(&world, &branch_of(&world, run, "b")), b_head);
    the_engine_merged_closed_and_lifted_nothing(&world, run, from);
}

/// The position of the first record of `kind` about `node` in `run`'s journal.
fn first(world: &World, run: &str, kind: &str, node: &str) -> usize {
    world
        .journal(run)
        .iter()
        .position(|event| event["kind"] == kind && event["labels"]["node"] == node)
        .unwrap_or_else(|| panic!("no {kind} about {node}: {:#?}", world.kinds(run)))
}

/// That `node`'s landing was found by `trigger`'s own read, before `then` was recorded
/// about `about`.
fn found_first(world: &World, run: &str, node: &str, trigger: &str, then: (&str, &str)) {
    let landed = record(world, run, "node-landed", node)
        .unwrap_or_else(|| panic!("{node}'s landing was never found"));
    assert_eq!(landed["trigger"], trigger, "{landed}");
    assert!(
        first(world, run, "node-landed", node) < first(world, run, then.0, then.1),
        "{node}'s landing was recorded after {} of {}",
        then.0,
        then.1
    );
}

/// `onepipeline review` readjusts first: `a`'s landing, made before the pass, is found by
/// its own batched read — the one its discovery reads feedback from — before discovery
/// records anything, and `b` is moved onto the root.
#[test]
fn review_readjusts_before_discovery() {
    let world = team_world("readjust-trigger-review");
    for node in ["a", "b"] {
        world.script(&format!("{node}.work"), &format!("{node}'s change\n"));
    }
    let run = "review";
    launched(
        &world,
        run,
        vec![lifecycle("a", &[]), lifecycle("b", &["a"])],
    );
    merge(&world, &number_of(&world, run, "a"), "squash", "kept");
    let from = world.invocations().len();
    review(&world, run);
    found_first(&world, run, "a", "review", ("review-comments-read", "b"));
    let reads: Vec<Value> = host_calls(&world, from)
        .into_iter()
        .filter(|call| is_standings_read(call))
        .collect();
    assert!(
        query_of(&reads[0]).contains("reviewThreads("),
        "the pass's first read is not the one discovery reads from: {reads:#?}"
    );
    assert!(host_calls(&world, from)
        .iter()
        .all(|call| !query_of(call).contains("pullRequest(number: $number)")));
    assert_eq!(settled(&world, run, "b")["base"], "main");
}

/// A restack reads its stack parents' standings before it starts: `b`'s rework reads
/// before its republication and its push is held there; a person merges `z` meanwhile;
/// and the restack `b`'s push owes `c` reads `z` landed — trigger `placement` — before
/// `c` starts.
#[test]
fn a_restack_readjusts_before_it_starts() {
    let world = World::new("readjust-trigger-restack").with_env("ONEVCS_BRANCH_PREFIX", "me/");
    let go = world.fakes.join("push.go");
    let held = crate::harness::held_publication(&world, &go);
    world.repository("change-open", &held.argv());
    crate::draft_lifecycle::rules(&world, "change-open", "required", None);
    world.script("gh.checks", crate::draft_lifecycle::GREEN);
    for node in ["b", "z", "c"] {
        world.script(&format!("{node}.work"), &format!("{node}'s change\n"));
    }
    world.script("b-rework-1.work", "b's change, as the review asked\n");
    world.script("b-rework-1.work-file", "b");
    held.release();
    let run = "restack";
    launched(
        &world,
        run,
        vec![
            lifecycle("b", &[]),
            lifecycle("z", &[]),
            lifecycle("c", &["b", "z"]),
        ],
    );
    for marker in [go.clone(), go.with_extension("go.holding")] {
        std::fs::remove_file(marker).expect("the launch's publications released the gate");
    }
    reply(&world, run, json!([rework("b")])).exited(0);
    let driver = world
        .cmd(&["adopt", run])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the adopting driver starts");
    world.until("the rework's push to be held", |_| held.holding());
    merge(&world, &number_of(&world, run, "z"), "squash", "kept");
    held.release();
    let ended = driver.wait_with_output().expect("the driver ends");
    assert!(ended.status.success(), "{ended:?}");
    found_first(&world, run, "z", "placement", ("node-restack-started", "c"));
    assert_eq!(
        settled(&world, run, "c")["base"],
        branch_of(&world, run, "b").as_str()
    );
}

/// A placement at dispatch reads its stack parents' standings first: on a live driver,
/// a person merges `a` and then a node is added on `b`. The pass that places it reads
/// `a` landed — trigger `placement` — before it dispatches the node.
#[test]
fn a_placement_at_dispatch_readjusts_before_it_dispatches() {
    let world = team_world("readjust-trigger-placement");
    for node in ["a", "b", "e", "x"] {
        world.script(&format!("{node}.work"), &format!("{node}'s change\n"));
    }
    world.script("x.wait", "");
    let run = "placement";
    launched_at(
        &world,
        run,
        vec![lifecycle("a", &[]), lifecycle("b", &["a"])],
        3,
    );
    reply(
        &world,
        run,
        json!([{"op": "add", "node": lifecycle("x", &[])}]),
    )
    .exited(0);
    let driver = world
        .cmd(&["adopt", run])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the adopting driver starts");
    world.until("x to be held", |world| {
        world
            .events_of(run, "node-dispatched")
            .iter()
            .any(|event| event["labels"]["node"] == "x")
    });
    merge(&world, &number_of(&world, run, "a"), "squash", "kept");
    reply(
        &world,
        run,
        json!([{"op": "add", "node": lifecycle("e", &["b"])}]),
    )
    .exited(0);
    world.until("e to be dispatched", |world| {
        world
            .events_of(run, "node-dispatched")
            .iter()
            .any(|event| event["labels"]["node"] == "e")
    });
    found_first(&world, run, "a", "placement", ("node-dispatched", "e"));
    std::fs::write(world.fakes.join("x.go"), "").expect("the hold is released");
    let ended = driver.wait_with_output().expect("the driver ends");
    assert!(ended.status.success(), "{ended:?}");
}

/// A continuing launch on a cold host, from the local Markdown store, readjusts before
/// its first dispatch: `a`'s landing, made before it, is found by its own read and `b`
/// is moved onto the root.
#[test]
fn a_continuing_launch_readjusts_before_its_first_dispatch() {
    let world = team_world("readjust-trigger-continuation");
    for node in ["a", "b"] {
        world.script(&format!("{node}.work"), &format!("{node}'s change\n"));
    }
    let run = "continued";
    let project = launched(
        &world,
        run,
        vec![lifecycle("a", &[]), lifecycle("b", &["a"])],
    );
    world.until_store("b to reach the store", |world| {
        projected_settlement(world, &project, "b")["status"] == "done"
    });
    merge(&world, &number_of(&world, run, "a"), "squash", "kept");
    let cold = world.on_host("readjust-continued-cold");
    cold.run_from(&cold.project, &["start", &project, "--attach"])
        .exited(0);
    found_first(
        &cold,
        run,
        "a",
        "continuation",
        ("node-restack-started", "b"),
    );
    assert!(cold.events_of(run, "node-dispatched").is_empty());
    assert_eq!(settled(&cold, run, "b")["base"], "main");
}

/// An adoption readjusts before its first dispatch: `a`'s landing, made while nothing
/// drove the run, is found by the adopting driver's own read and `b` is moved.
#[test]
fn an_adoption_readjusts_before_its_first_dispatch() {
    let world = team_world("readjust-trigger-adoption");
    for node in ["a", "b"] {
        world.script(&format!("{node}.work"), &format!("{node}'s change\n"));
    }
    let run = "adopted";
    launched(
        &world,
        run,
        vec![lifecycle("a", &[]), lifecycle("b", &["a"])],
    );
    merge(&world, &number_of(&world, run, "a"), "squash", "kept");
    world.run(&["adopt", run]).exited(0);
    found_first(
        &world,
        run,
        "a",
        "continuation",
        ("node-restack-started", "b"),
    );
    assert_eq!(settled(&world, run, "b")["base"], "main");
}

/// Nothing polls: a live driver with drafts outstanding and nothing to do, left over
/// three of its idle intervals — shortened through the test seam entry 119 names —
/// asks the host nothing at all; and `start` takes no readjustment-interval flag.
#[test]
fn an_idle_driver_asks_the_host_nothing() {
    let world = team_world("readjust-idle").with_env("ONEPIPELINE_IDLE_CLAIM_CHECK_SECONDS", "1");
    for node in ["a", "b", "x"] {
        world.script(&format!("{node}.work"), &format!("{node}'s change\n"));
    }
    world.script("x.wait", "");
    let run = "idle";
    launched_at(
        &world,
        run,
        vec![lifecycle("a", &[]), lifecycle("b", &["a"])],
        3,
    );
    reply(
        &world,
        run,
        json!([{"op": "add", "node": lifecycle("x", &[])}]),
    )
    .exited(0);
    let driver = world
        .cmd(&["adopt", run])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the adopting driver starts");
    world.until("x's session to be open and its worker held", |world| {
        world
            .journal(run)
            .iter()
            .any(|event| event["kind"] == "session-opened" && event["labels"]["node"] == "x")
    });
    // Past the instant the worker reached its hold, so the window below is the driver's
    // own idle and nothing of x's dispatch.
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let from = world.invocations().len();
    std::thread::sleep(std::time::Duration::from_millis(3500));
    let asked = host_calls(&world, from);
    assert!(
        asked.is_empty(),
        "an idle driver asked the host: {asked:#?}"
    );
    std::fs::write(world.fakes.join("x.go"), "").expect("the hold is released");
    let ended = driver.wait_with_output().expect("the driver ends");
    assert!(ended.status.success(), "{ended:?}");

    let refused = world.run(&[
        "start",
        "--readjust-interval",
        "5",
        "plans:idle",
        "--attach",
    ]);
    assert_ne!(refused.code, 0, "{}", refused.stdout);
    assert!(
        refused.stderr.contains("--readjust-interval"),
        "{}",
        refused.stderr
    );
}
