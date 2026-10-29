//! Pool admission and slot hand-back over the compiled CLI and real onevcs.

#[path = "../e2e/harness.rs"]
mod harness;
#[path = "../e2e/pool_support.rs"]
mod support;

use harness::{lifecycle, plan_of, World};
use serde_json::json;
use std::path::PathBuf;
use support::*;

/// A pooled identity that admits one session holds the second node that would
/// open one, and lets it go when the first's session closes.
///
/// Against the real linked `onevcs` under a scratch state root with a
/// `workspaces.yml`: the read is the sibling's own, the slot is a real warm
/// worktree, and the second node runs on it once the first hands it back. Five
/// things at once, because they are one pass's decisions. Two nodes of the one
/// identity are both ready in the first pass with one slot between them, and the
/// second is held on a reading taken **before** the first's session existed —
/// which is the pass counting its own dispatch rather than the identity's
/// records. A node of a second, unconfigured identity behind the held one starts
/// in that same pass. A node whose capacity read fails — its identity directory
/// carries a plain file where the pool directory would be, which the survey
/// refuses and the open, placing an unconfigured identity without one, never
/// looks at — is dispatched as it always was. A third node of the full identity
/// stating `pool: 0` and `overflow: unlimited` is admitted past the hold and
/// placed under `runs/`, which only the sibling reading those overrides off the
/// request can do. And the hold is on the record, in `status`, and surfaced non-blocking
/// with the identity and the numbers — again on the cadence a release wait is.
#[test]
fn a_pooled_identity_holds_the_second_node_until_the_first_hands_its_slot_back() {
    let world = World::new("lifecycle-pool-held")
        .with_env("ONEPIPELINE_WORKSPACE_POLL_SECONDS", "1")
        .with_env("ONEPIPELINE_RELEASE_SURFACE_SECONDS", "1");
    let repo = world.repository("local-direct", &[]);
    let other = world.extra_repository("other");
    let unread = world.extra_repository("unread");
    pool_one_slot_no_overflow(&world);
    // Each repository's visibility is declared, so a publication's write boundary
    // takes the rule's word rather than probing the fake host, which answers no
    // visibility: an unanswered probe screens the write, and the `unread` node's
    // commit message names its own repository, a term of a private one.
    std::fs::write(
        world.onevcs_home().join("rules.yml"),
        "version: 4\n\
         rules:\n\
         \x20 - match: {host: github.com, owner: owner, name: service}\n\
         \x20   visibility: private\n\
         \x20 - match: {host: github.com, owner: owner, name: other}\n\
         \x20   visibility: private\n\
         \x20 - match: {host: github.com, owner: owner, name: unread}\n\
         \x20   visibility: private\n\
         default:\n\
         \x20 publication: local-direct\n\
         \x20 approvals: none\n",
    )
    .expect("the rules file is written");
    // The identity directory exists once a session has been placed under it, so
    // one is opened and closed in this process to find it — and then a file is
    // put where the survey expects a directory.
    let identity_root = world.on_onevcs(|| {
        let vcs = onevcs::Providers::real().vcs;
        let session = vcs
            .open_session(onevcs::SessionRequest {
                repo: unread.checkout.to_string_lossy().into_owned(),
                branch: None,
                branch_name: None,
                branch_prefix: None,
                base: None,
                execution_checkout: None,
                pool: None,
                overflow: None,
                labels: Default::default(),
                refuse_conflicts: false,
                caller_merges_base: false,
            })
            .expect("a session opens on the unread identity");
        let root = session
            .worktree
            .ancestors()
            .nth(3)
            .expect("<identity>/runs/<token>/worktree")
            .to_path_buf();
        vcs.close_session(&session.token)
            .expect("the session closes");
        root
    });
    // llmlint: ignore-block[tests_mirror_real_usage] no interface produces a capacity
    // read that fails while the open succeeds — both resolve the identity through one
    // path — except a pool directory the survey cannot list and an unconfigured open
    // never consults; the file is that condition, and the behaviour it proves is the
    // contract's own "a read that fails holds nothing".
    std::fs::write(identity_root.join("pool"), "not a directory\n")
        .expect("the file standing where the pool directory would be is written");
    // llmlint: ignore-end[tests_mirror_real_usage]
    let _ = other;

    // The first holds until this journey has read the hold it causes.
    world.script("first.work", "the first wrote this\n");
    world.script("first.wait", "");
    world.script("second.work", "the second wrote this\n");
    world.script("other.work", "the other wrote this\n");
    world.script("unread.work", "the unread wrote this\n");
    world.script("unbounded.work", "the unbounded wrote this\n");
    let mut on_other = lifecycle("other", &[]);
    on_other["repo"] = json!("other");
    let mut on_unread = lifecycle("unread", &[]);
    on_unread["repo"] = json!("unread");
    // Both overrides, as the sibling reads them: `pool: 0` places this one
    // fresh under `runs/` rather than letting it race the first for the slot,
    // and `overflow: unlimited` is what admits it there past the rule's `0`.
    let mut unbounded = lifecycle("unbounded", &[]);
    unbounded["pool"] = json!(0);
    unbounded["overflow"] = json!("unlimited");
    let mut plan = plan_of(
        "poolheld",
        vec![
            lifecycle("first", &[]),
            lifecycle("second", &[]),
            on_other,
            on_unread,
            unbounded,
        ],
    );
    plan["concurrency"] = json!(5);
    let path = world.plan("poolheld", &plan);
    world.run(&["start", &path, "--detach"]).exited(0);
    let run = "poolheld".to_string();

    world.until("the second node to be held for its workspace", |world| {
        assert!(
            dispatches_of(world, &run, "second").is_empty(),
            "the second node was dispatched before the first handed its slot back\n{}",
            why(world, &run)
        );
        first_workspace_hold_of(world, &run, "second").is_some()
    });
    let hold = first_workspace_hold_of(&world, &run, "second").expect("held");
    assert_eq!(hold["identity"], SERVICE_IDENTITY, "{hold}");
    assert_eq!(hold["pool"], 1, "{hold}");
    assert_eq!(hold["overflow"], 0, "{hold}");
    assert_eq!(hold["overflow_in_use"], 0, "{hold}");
    for key in ["slots", "idle", "maintaining"] {
        assert!(hold[key].is_u64(), "the hold carries no {key}: {hold}");
    }
    // The first was dispatched and the second was not, on the one pass both
    // were ready in; nothing about the second settled.
    assert_eq!(
        dispatches_of(&world, &run, "first").len(),
        1,
        "{}",
        why(&world, &run)
    );
    assert!(
        dispatches_of(&world, &run, "second").is_empty(),
        "the second node was dispatched into a full identity\n{}",
        why(&world, &run)
    );
    // The two other identities started behind the held node, in that same pass:
    // before the first's session had even opened, since that is what the read
    // that held the second predates. So did the node of the full identity that
    // opts out of the cap.
    for node in ["other", "unread", "unbounded"] {
        assert_eq!(
            dispatches_of(&world, &run, node).len(),
            1,
            "{node} did not start behind the held node\n{}",
            why(&world, &run)
        );
        assert!(
            workspace_hold_of(&world, &run, node).is_none(),
            "{node} was held for a workspace it has room in\n{}",
            why(&world, &run)
        );
    }
    let held_at = first_seq(&world, &run, "node-held", "second");
    for node in ["first", "other", "unread", "unbounded"] {
        let dispatched_at = first_seq(&world, &run, "node-dispatched", node);
        assert!(
            dispatched_at < held_at + 5,
            "{node} was dispatched at {dispatched_at}, long after the hold at {held_at}\n{}",
            why(&world, &run)
        );
    }

    // The wait is surfaced: non-blocking, the node's own workstream, naming the
    // identity, the numbers, and the sibling's verb that names the holders — and
    // surfaced again on the cadence, so a wait nobody has ended cannot go silent.
    let workspace_waits = |world: &World| -> Vec<serde_json::Value> {
        world
            .events_of(&run, "planner-surface-queued")
            .into_iter()
            .filter(|event| event["payload"]["kind"] == "workspace-wait")
            .collect()
    };
    world.until("the wait to be surfaced again on its cadence", |world| {
        workspace_waits(world).len() >= 2
    });
    let surfaced = workspace_waits(&world);
    for surface in &surfaced {
        assert_eq!(surface["payload"]["blocking"], false, "{surface}");
        assert_eq!(surface["payload"]["source"], "proposal", "{surface}");
        assert_eq!(surface["labels"]["node"], "second", "{surface}");
    }
    let said = surfaced[0]["payload"]["message"]
        .as_str()
        .expect("the surface says something");
    for names in [
        "node 'second' is held",
        &format!("'{SERVICE_IDENTITY}' workspace admits no more sessions"),
        "pool 1 (",
        "overflow 0 with 0 in use",
        &format!("onevcs pool status {SERVICE_IDENTITY}"),
        "Nothing times this out",
    ] {
        assert!(
            said.contains(names),
            "the surface does not say {names:?}: {said}"
        );
    }
    // And a planner reads it off the queue without it holding anything.
    world
        .run(&["next", &run])
        .exited(0)
        .out_has("admits no more sessions");
    // `status` renders the hold the way it renders the other holds.
    world
        .run(&["status", &run])
        .exited(0)
        .out_has(&format!(
            "second: held — the '{SERVICE_IDENTITY}' workspace admits no more sessions now"
        ))
        .out_has(&format!("onevcs pool status {SERVICE_IDENTITY}"));

    // The overflow session proves placement while the slot is occupied, but
    // must finish publishing before the slot's owner publishes. onevcs's
    // publish_locally checks checkout readiness before queue::turn; a check
    // during another publication's fast-forward can see a transient dirty tree.
    // This journey tests pool admission, independently of that publication race.
    world.until("the overflow node to finish publishing", |world| {
        world
            .events_of(&run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "unbounded")
    });
    assert!(
        dispatches_of(&world, &run, "second").is_empty(),
        "the second node was dispatched before the first handed its slot back\n{}",
        why(&world, &run)
    );

    // The first hands its slot back, and the second takes it. Wait for the
    // session's close on the record, rather than allowing time for its return.
    world.release("first.go");
    world.until("the first session to return its pool slot", |world| {
        world.events_of(&run, "session-closed").iter().any(|event| {
            event["labels"]["node"] == "first" && event["payload"]["returned"]["slot"].is_u64()
        })
    });
    world.until("the run to settle", |world| {
        world.run_file(&run, "result.json").is_file()
    });
    let result = world.run_json(&run, "result.json");
    assert_eq!(
        result["state"],
        "complete",
        "{result}\n{}",
        why(&world, &run)
    );
    for node in result["nodes"].as_array().expect("nodes") {
        assert_eq!(node["status"], "done", "{node}\n{}", why(&world, &run));
    }
    // The second was dispatched onto the **slot** the first handed back — the
    // one warm worktree the identity keeps, which is where both sessions say
    // they worked — and the hold cleared on the record. Usually once; not
    // always. The read that let it go is advisory and `open` is authoritative:
    // the sibling counts a slot idle from the moment the first's publication
    // closes the session record and takes it only once the tree is returned.
    // Two different things reach the second inside that window, and this
    // journey has met both on Windows, where it is wide enough for the poll to
    // land in. A read inside it admits the second and its open is refused
    // `PoolExhausted` — the race the contract says costs the node nothing, and
    // which this crate answers with a `node-requeued` under
    // `workspace-exhausted` and a fresh attempt. Or the open is admitted and
    // its own `git fetch` collides in that one checkout with the fetch the
    // first's return is still running there — which is not a pool refusal, so
    // `src/pool.rs`'s exemption does not reach it and the boundary answers it
    // the way it answers any dispatch that failed: it asks again, naming what
    // it is asking after.
    //
    // So what is held is the accounting rather than the count. Read in order,
    // every dispatch of the second is exactly one of those two — a fresh
    // attempt, which is attempt 1 and names no reason, or the boundary asking
    // again, which names what it follows and counts one past the dispatch
    // before it — there is one fresh attempt per refusal plus the one that took
    // the slot, and however many times the host got in the way, the node
    // settled once.
    let dispatched = dispatches_of(&world, &run, "second");
    let requeued: Vec<serde_json::Value> = world
        .events_of(&run, "node-requeued")
        .into_iter()
        .filter(|event| event["labels"]["node"] == "second")
        .collect();
    let mut fresh = 0usize;
    let mut previous = 0u64;
    for dispatch in &dispatched {
        let attempt = dispatch["payload"]["attempt"]
            .as_u64()
            .unwrap_or_else(|| panic!("a dispatch naming no attempt: {dispatch}"));
        if dispatch["payload"]["reason"].is_null() {
            assert_eq!(
                attempt, 1,
                "a fresh dispatch is not a first attempt\n{dispatch}"
            );
            fresh += 1;
        } else {
            assert!(
                dispatch["payload"]["reason"]
                    .as_str()
                    .is_some_and(|reason| !reason.trim().is_empty()),
                "a re-asked dispatch says nothing about what it follows\n{dispatch}"
            );
            assert_eq!(
                attempt,
                previous + 1,
                "a re-asked dispatch does not count one past the one before it\n{dispatch}\n{}",
                why(&world, &run)
            );
        }
        previous = attempt;
    }
    assert_eq!(
        fresh,
        1 + requeued.len(),
        "a fresh dispatch of the second is not accounted for by a refusal\n{dispatched:#?}\n\
         {requeued:#?}\n{}",
        why(&world, &run)
    );
    for requeue in &requeued {
        assert_eq!(
            requeue["payload"]["reason"], "workspace-exhausted",
            "{requeue}"
        );
    }
    let settled_second = world
        .events_of(&run, "node-settled")
        .into_iter()
        .filter(|event| event["labels"]["node"] == "second")
        .count();
    assert_eq!(settled_second, 1, "{}", why(&world, &run));
    let worktree_of = |node: &str| {
        world
            .events_of(&run, "session-opened")
            .into_iter()
            .find(|event| event["labels"]["node"] == node)
            .and_then(|event| event["payload"]["worktree"].as_str().map(str::to_owned))
            .unwrap_or_else(|| panic!("{node} opened no session\n{}", why(&world, &run)))
    };
    assert_eq!(
        worktree_of("first"),
        worktree_of("second"),
        "the second did not take the slot the first handed back\n{}",
        why(&world, &run)
    );
    // Placement is read off the path's components, not a `/`-joined substring:
    // the sibling writes the host's separator, and this journey runs on Windows.
    let placed_under = |node: &str, directory: &str| {
        PathBuf::from(worktree_of(node))
            .components()
            .any(|component| component.as_os_str() == directory)
    };
    assert!(
        placed_under("first", "pool"),
        "the sessions were not placed on a pool slot: {}",
        worktree_of("first")
    );
    // And the one that opted out of the cap was placed **past** the pool, under
    // `runs/`, while the slot was held: the override reached the sibling.
    assert!(
        placed_under("unbounded", "runs"),
        "the unbounded node was not placed under runs/: {}",
        worktree_of("unbounded")
    );
    let unheld: Vec<serde_json::Value> = world
        .events_of(&run, "node-unheld")
        .into_iter()
        .filter(|event| event["labels"]["node"] == "second")
        .collect();
    // Released once per time it was held: once for the slot the first handed
    // back, and once more for each open the identity refused after admitting it
    // that the next pass still held. Not for every refusal: a refusal holds the
    // node only until the paced re-read comes due, so a pass that reaches it
    // after that reads the identity afresh and — the first's slot returned by
    // then — dispatches it without holding it again. On Windows the loop can be
    // that late, and was.
    let (held_spans, released_spans) = hold_spans(&world, &run, "second");
    assert_eq!(
        released_spans,
        held_spans,
        "a hold of the second was not released exactly once\n{unheld:#?}\n{}",
        why(&world, &run)
    );
    assert!(
        (1..=1 + requeued.len()).contains(&held_spans),
        "the second was held {held_spans} time(s) over {} refusal(s): each hold past \
         the first is a refusal's\n{unheld:#?}\n{}",
        requeued.len(),
        why(&world, &run)
    );
    for release in &unheld {
        assert!(
            release["payload"]["released"]
                .as_array()
                .expect("released")
                .iter()
                .any(|reason| reason["kind"] == "workspace"),
            "the release does not name the workspace hold: {release:#}"
        );
    }
    let landed = repo.base_commits(&world);
    assert!(
        landed.iter().any(|subject| subject.contains("first"))
            && landed.iter().any(|subject| subject.contains("second")),
        "both nodes' work did not land: {landed:?}"
    );
}

/// Every hold `node` was put under and later let go of, counted from its own
/// `node-held` and `node-unheld` records in stream order: a `node-held` opens
/// one only where `node` was not already held, since a change of reason or
/// reading is recorded as another `node-held` on the same hold.
fn hold_spans(world: &World, run: &str, node: &str) -> (usize, usize) {
    let (mut opened, mut released, mut holding) = (0usize, 0usize, false);
    for event in world.journal(run) {
        if event["labels"]["node"] != node {
            continue;
        }
        match event["kind"].as_str() {
            Some("node-held") if !holding => {
                opened += 1;
                holding = true;
            }
            Some("node-unheld") => {
                released += 1;
                holding = false;
            }
            _ => {}
        }
    }
    (opened, released)
}
