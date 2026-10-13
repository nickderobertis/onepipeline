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
    let mut plan = plan_of(run, nodes);
    plan["concurrency"] = json!(1);
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
