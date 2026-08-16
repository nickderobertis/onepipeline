//! A lifecycle node is this crate composing a `onevcs` session with the
//! dispatches that work in it. The branch, the worktree, the gate, and the
//! publication are all the sibling's; the DAG, the loop, and the pr-author
//! composition are this one's.
//!
//! Every journey here drives the **real** repository side: `onevcs` is a library
//! this crate calls, so there is nothing to substitute at a subprocess boundary
//! and nothing scripts what a publication did. What a journey states instead is
//! the world that library reads — the repository's rules, the command that gates
//! it, and, at `onevcs`'s own `ONEVCS_GH` seam, what GitHub does with the change
//! request it is handed.
//!
//! Ported from the lifecycle-node composition halves of `test_lifecycle_e2e`.

// llmlint: ignore-file[e2e_not_mocked] the crate under test is driven as a real compiled
// binary and the sibling these journeys are about — `onevcs` — is the real library, over
// real git and a real origin on disk. `oneagentgraph` is substituted at its subprocess
// boundary so a journey states a dispatch outcome rather than paying for a model turn, and
// GitHub is substituted at `onevcs`'s own `ONEVCS_GH` override so a change request can be
// opened and merged offline. `harness.rs` carries the same suppression and the full
// rationale.

use crate::harness::{agent, gate_script, lifecycle, plan_of, Repository, World, REFUSED};
use onevcs::provenance::SUBJECT_LIMIT;
use serde_json::json;

fn settle(world: &World, name: &str, nodes: Vec<serde_json::Value>) -> String {
    let path = world.plan(name, &plan_of(name, nodes));
    world
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .settled();
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });
    name.to_string()
}

/// Drive one run from this test, attached, and keep what it said.
///
/// A detached driver's diagnostics go to a log rather than to a descriptor this
/// test holds. Attached, the loop runs in the process this command started, so
/// what it says lands on the stderr the assertion reads.
fn driven(
    world: &World,
    name: &str,
    nodes: Vec<serde_json::Value>,
) -> (String, crate::harness::Run) {
    let path = world.plan(name, &plan_of(name, nodes));
    let launched = world.run(&["start", &path.to_string_lossy(), "--attach"]);
    (name.to_string(), launched)
}

/// A repository whose gate passes, publishing straight onto its base.
///
/// `local-direct` reaches the base with git alone, so a journey that only needs
/// *a* publication asks no host for anything.
fn published_locally(world: &World) -> Repository {
    world.repository("local-direct", &["true"])
}

/// A command a gate can be given, for a journey that needs the gate to do
/// something other than pass.
///
/// The gate runs in the session's worktree, which sits at
/// `$ONEVCS_HOME/<identity>/runs/<token>/worktree` — so the session's own token
/// is the name of the directory above it, and a gate can address the stream that
/// session is writing without this crate telling it one.
fn gate(world: &World, args: &[&str]) -> Vec<String> {
    gate_script(world, args)
}

/// Every `onevcs`-produced event one run recorded, by kind.
fn vcs_kinds(world: &World, run: &str) -> Vec<String> {
    world
        .journal(run)
        .iter()
        .filter(|event| event["source"] == "vcs")
        .filter_map(|event| event["kind"].as_str().map(str::to_string))
        .collect()
}

/// The session tokens **the sibling itself** opened, in order.
///
/// Told apart from this crate's own `session-opened` — which it writes beside
/// the sibling's for the merged stream — by the fields only the repository side
/// knows: it names the clone it cut and the checkout it cut it from.
fn sibling_sessions(world: &World, run: &str) -> Vec<String> {
    world
        .journal(run)
        .iter()
        .filter(|event| event["source"] == "vcs" && event["kind"] == "session-opened")
        .filter(|event| event["payload"]["clone"].is_string())
        .filter_map(|event| event["payload"]["token"].as_str().map(str::to_string))
        .collect()
}

/// Every session token a run has recorded an opening for, in order, once each.
///
/// This crate writes its own `session-opened` as the dispatch starts, so a token
/// is readable here while the step that opened it is still running — long before
/// the sibling's own record of the same session is relayed off its stream.
fn opened_tokens(world: &World, run: &str) -> Vec<String> {
    world
        .journal(run)
        .iter()
        .filter(|event| event["source"] == "vcs" && event["kind"] == "session-opened")
        .filter_map(|event| event["payload"]["token"].as_str().map(str::to_string))
        .fold(Vec::new(), |mut seen, token| {
            if !seen.contains(&token) {
                seen.push(token);
            }
            seen
        })
}

/// The directory each dispatched step ran in, in the order they settled.
fn step_directories(world: &World, run: &str) -> Vec<(String, String)> {
    world
        .journal(run)
        .iter()
        .filter(|event| event["source"] == "agentgraph")
        .filter_map(|event| {
            Some((
                event["labels"]["step"].as_str()?.to_string(),
                event["payload"]["dir"].as_str()?.to_string(),
            ))
        })
        .fold(Vec::new(), |mut seen, (step, dir)| {
            if !seen.iter().any(|(known, _)| known == &step) {
                seen.push((step, dir));
            }
            seen
        })
}

/// The branch each session a run opened was cut on.
fn session_branches(world: &World, run: &str) -> Vec<String> {
    world
        .journal(run)
        .iter()
        .filter(|event| event["source"] == "vcs" && event["kind"] == "session-opened")
        .filter_map(|event| event["payload"]["branch"].as_str().map(str::to_string))
        .collect()
}

/// Why a run settled the way it did, as the sibling itself said it.
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
    format!(
        "what the nodes settled on:\n  {}\n  the sibling recorded: {:?}",
        settled.join("\n  "),
        vcs_kinds(world, run)
    )
}

#[test]
fn a_lifecycle_node_opens_a_session_works_in_it_and_publishes_through_onevcs() {
    let world = World::new("lifecycle-publish");
    published_locally(&world);
    world.script("service.work", "the worker wrote this\n");
    let run = settle(&world, "shipped", vec![lifecycle("service", &[])]);

    // What the composition did, read off the sibling's own records rather than
    // off an argument vector: a session was opened, its branch was gated and
    // pushed, and the session was released. Against a spawned double the
    // equivalent assertion was "these arguments were passed", which stayed true
    // of a command that then failed.
    let kinds = vcs_kinds(&world, &run);
    for kind in ["session-opened", "gate-verdict", "push", "session-closed"] {
        assert!(
            kinds.iter().any(|seen| seen == kind),
            "the publication recorded no {kind}: {kinds:?}\n{}",
            why(&world, &run)
        );
    }

    // The dispatch ran *in the worktree the session handed back*, which is what
    // `WorkspaceSpec::VcsSession` means: the machine running the dispatch opens
    // the session there.
    let dispatched = world
        .journal(&run)
        .into_iter()
        .find(|event| {
            event["source"] == "agentgraph"
                && event["kind"] == "turn-activity"
                && event["labels"]["node"] == "service"
        })
        .expect("the lifecycle node dispatched");
    let dir = dispatched["payload"]["dir"].as_str().expect("a directory");
    assert!(
        dir.contains("worktree"),
        "the dispatch did not run in the session's worktree: {dir}"
    );

    // The session's own stream joins the merged one, and this crate's record of
    // the publication joins it beside them.
    assert!(
        world
            .journal(&run)
            .iter()
            .any(|event| event["source"] == "vcs" && event["kind"] == "published"),
        "the publication is missing from the merged store"
    );

    let result = world.run_json(&run, "result.json");
    assert_eq!(
        result["nodes"][0]["status"],
        "done",
        "{}",
        why(&world, &run)
    );
    assert_eq!(result["state"], "complete");
}

#[test]
fn several_steps_share_one_branch_and_run_serially_in_topological_order() {
    let world = World::new("lifecycle-steps");
    published_locally(&world);
    let node = json!({
        "id": "service",
        "repo": "service",
        "steps": [
            {"id": "review", "persona": "reviewer", "task": "## What\nreview", "deps": ["implement"]},
            {"id": "implement", "persona": "engineer", "task": "## What\nimplement"},
        ],
    });
    let run = settle(&world, "workstream", vec![node]);

    // One settlement per dispatched step, in the order they settled: a
    // dispatch records several envelopes and counting all of them would say a
    // step ran as many times as it spoke.
    let steps: Vec<String> = world
        .journal(&run)
        .iter()
        .filter(|event| event["source"] == "agentgraph" && event["kind"] == "member-settled")
        .filter_map(|event| event["labels"]["step"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        steps,
        vec!["implement".to_string(), "review".to_string()],
        "the steps did not run in topological order"
    );

    // **One** session for the whole workstream. A step that opened its own
    // would be a fresh clone cut from the base, carrying none of the earlier
    // steps' work — and opening it reclaims the first session's workspace,
    // uncommitted work and all. Counted off the sibling's own record rather
    // than this crate's, which writes one of its own beside it.
    let opened = sibling_sessions(&world, &run);
    assert_eq!(
        opened.len(),
        1,
        "the workstream opened {} sessions, not one: {opened:?}\n{}",
        opened.len(),
        why(&world, &run)
    );

    // And both steps worked in that session's worktree — the same directory,
    // which is what "several steps share one branch" has always meant and what
    // a second session would quietly break.
    let dirs = step_directories(&world, &run);
    assert_eq!(
        dirs.len(),
        2,
        "a step recorded no directory it ran in: {dirs:?}"
    );
    assert_eq!(
        dirs[0].1, dirs[1].1,
        "the steps ran in different worktrees: {dirs:?}"
    );
    assert!(
        dirs[0].1.contains("worktree"),
        "the steps did not run in the session's worktree: {dirs:?}"
    );
    // The branch that one session was cut on is the branch the node published.
    let branches = session_branches(&world, &run);
    assert!(
        branches.windows(2).all(|pair| pair[0] == pair[1]),
        "the steps did not share one branch: {branches:?}"
    );
    assert_eq!(world.run_json(&run, "result.json")["state"], "complete");
}

/// A workstream whose session record goes missing between two steps.
///
/// Every step after the first runs in the worktree the first step's session
/// opened, and this crate asks the sibling where that is. When the record it
/// asks for cannot be read there is no worktree to name — so the step opens a
/// session of its own, which is what it would have done before, rather than
/// running nowhere or taking the node down. The fallback is said out loud,
/// because a workstream that quietly started over is a step's work silently
/// left behind.
///
/// The record is removed while the first step is held at a rendezvous, so the
/// window is the test's rather than the host's.
#[test]
fn a_session_record_that_cannot_be_read_falls_back_to_opening_a_session() {
    use std::process::Stdio;

    let world = World::new("lifecycle-norecord");
    published_locally(&world);
    world.script("service.implement.wait", "hold");
    world.script("driver.wait", "hold");
    let node = json!({
        "id": "service",
        "repo": "service",
        // Its own title, so the run spends no `pr-author` dispatch: that one
        // reads the node's worktree too and would fall back alongside the step,
        // and this journey is about the step.
        "title": "feat: land what the steps made",
        "steps": [
            {"id": "implement", "persona": "engineer", "task": "## What\nimplement"},
            {"id": "review", "persona": "reviewer", "task": "## What\nreview", "deps": ["implement"]},
        ],
    });
    let path = world.plan("norecord", &plan_of("norecord", vec![node]));
    // Attached, so the fallback's own words land on a descriptor this test
    // holds: the loop runs in the process this command started.
    let driving = world
        .cmd(&["start", &path.to_string_lossy(), "--attach"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary starts");

    // The first step is held, so its session is open and recorded and nothing
    // has asked where its worktree is yet. The token comes off *this crate's*
    // record of the opening, which the executor emits as the dispatch starts —
    // the sibling's own reaches the merged store only once the session's stream
    // is relayed, which is after the step settles and therefore too late.
    world.until("the first step's session to be recorded", |world| {
        !opened_tokens(world, "norecord").is_empty()
    });
    let token = opened_tokens(&world, "norecord")
        .into_iter()
        .next()
        .expect("the session was just seen");
    // llmlint: ignore-block[tests_mirror_real_usage] the *arrangement* below removes the
    // session's record on purpose, because that is the condition: a state root a cleanup
    // swept, or a process that died between writing the record and closing the session.
    // It is a fault rather than an operation, and no command produces one — every
    // deletion `onevcs` performs is a run root under `workspaces/`, an integrate or
    // publish scratch, or a rotated gate log, and `session close` keeps the record
    // deliberately, because a closed session is still addressable. So there is nothing
    // else to reach it with. It fails loudly rather than silently if the sibling
    // relocates its records, and everything asserted afterwards is through the binary:
    // the run is `onepipeline start --attach`, and the claim is what it said and what
    // sessions it recorded.
    let record = world
        .onevcs_home()
        .join("sessions")
        .join(format!("{token}.json"));
    std::fs::remove_file(&record)
        .unwrap_or_else(|error| panic!("cannot remove {}: {error}", record.display()));
    // llmlint: ignore-end[tests_mirror_real_usage]

    world.release("service.implement.go");
    let settled = driving.wait_with_output().expect("the run drives");
    let said = String::from_utf8_lossy(&settled.stderr).into_owned();

    assert!(
        said.contains("cannot read session") && said.contains("record"),
        "the fallback was silent about the record it could not read:\n{said}"
    );
    // It fell back rather than running nowhere: the second step asked for a
    // session of its own, which is what it would have done had the first never
    // opened one. Two distinct tokens, where a workstream whose record *is*
    // readable records exactly one.
    let opened = opened_tokens(&world, "norecord");
    assert_eq!(
        opened.len(),
        2,
        "the step whose worktree could not be found did not open one: {opened:?}\n{}",
        why(&world, "norecord")
    );
    assert_eq!(opened[0], token, "{opened:?}");
    // And the run settled rather than the driver going down with it. It
    // settles *failed*: the sibling reclaims a run root nothing holds a lease
    // on, so opening the second session took the first one's workspace with it
    // — divergence 14 in `docs/contract-divergences.md`, and the reason a node
    // opens one session when it can.
    assert!(
        world.run_file("norecord", "result.json").is_file(),
        "the run never settled:\n{said}"
    );
}

#[test]
fn a_human_step_holds_the_workstream_rather_than_being_inferred() {
    let world = World::new("lifecycle-human-step");
    published_locally(&world);
    let node = json!({
        "id": "service",
        "repo": "service",
        "steps": [
            {"id": "implement", "persona": "engineer", "task": "## What\nimplement"},
            {"id": "staging-approval", "kind": "human", "task": "Exercise the staged service.", "deps": ["implement"]},
        ],
    });
    let run = settle(&world, "gatedstream", vec![node]);

    let result = world.run_json(&run, "result.json");
    assert_eq!(result["nodes"][0]["status"], "waiting", "{result}");
    // Nothing was published: the workstream is held at its human step, so the
    // sibling never ran a gate and never pushed.
    let kinds = vcs_kinds(&world, &run);
    for kind in ["gate-started", "push"] {
        assert!(
            !kinds.iter().any(|seen| seen == kind),
            "a workstream published past its human step: {kinds:?}"
        );
    }

    // It is a decision point like any other: a workstream stopped at a human
    // step waits on a person exactly as a `kind: human` node does, and the same
    // `attest` clears it. Reported as one, and released as one.
    let pending = world.events_of(&run, "decision-pending");
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0]["payload"]["reference"], "service");
    assert_eq!(pending[0]["payload"]["kind"], "attestation");

    world.run(&["attest", &run, "service"]).exited(0);
    // The run had settled on the decision, so a fresh driver picks it up — and
    // finds the action recorded, which is what releases it.
    world.run(&["adopt", &run]).exited(0);
    let cleared = world.events_of(&run, "decision-cleared");
    assert_eq!(cleared.len(), 1, "{cleared:?}");
    assert_eq!(cleared[0]["payload"]["reference"], "service");
    assert_eq!(
        world.run_json(&run, "result.json")["state"],
        "complete",
        "the attested workstream did not settle:\n{}",
        why(&world, &run)
    );
}

#[test]
fn the_pr_author_dispatch_drafts_the_title_and_never_blocks_publication() {
    let world = World::new("lifecycle-pr-author");
    let repo = published_locally(&world);
    world.script("service.work", "the worker wrote this\n");
    let run = settle(&world, "authored", vec![lifecycle("service", &[])]);

    assert!(
        world.was_invoked(
            "oneagentgraph",
            &["--label", "onepipeline.persona=pr-author"]
        ),
        "no pr-author dispatch: {:?}",
        world.invocations()
    );

    // The drafted title is the subject the change landed under — read off the
    // base branch, which is the only place a title is a fact rather than an
    // argument somebody passed.
    assert!(
        repo.base_commits(&world)
            .iter()
            .any(|subject| subject == "feat: drafted from the diff"),
        "the drafted title did not reach publication: {:?}\n{}",
        repo.base_commits(&world),
        why(&world, &run)
    );
    assert_eq!(world.run_json(&run, "result.json")["state"], "complete");
}

#[test]
fn a_planner_supplied_title_wins_over_the_drafting_dispatch() {
    let world = World::new("lifecycle-title");
    let repo = published_locally(&world);
    world.script("service.work", "the worker wrote this\n");
    let mut node = lifecycle("service", &[]);
    node["title"] = json!("fix: the planner named this");
    let run = settle(&world, "titled", vec![node]);

    assert!(
        repo.base_commits(&world)
            .iter()
            .any(|subject| subject == "fix: the planner named this"),
        "the planner's title was overwritten: {:?}\n{}",
        repo.base_commits(&world),
        why(&world, &run)
    );
    assert!(
        !world.was_invoked(
            "oneagentgraph",
            &["--label", "onepipeline.persona=pr-author"]
        ),
        "a title the planner set still spent a drafting dispatch"
    );
}

#[test]
fn a_drafting_failure_falls_back_deterministic_and_still_publishes() {
    let world = World::new("lifecycle-fallback");
    let repo = published_locally(&world);
    world.script("service.work", "the worker wrote this\n");
    // Only the drafting dispatch fails. It runs after the branch is already
    // verified and is not on the publication path, so the change still lands.
    world.script("service.pr-author.fail", "1");
    let run = settle(&world, "fallback", vec![lifecycle("service", &[])]);

    let result = world.run_json(&run, "result.json");
    assert_eq!(
        result["state"],
        "complete",
        "a drafting failure blocked publication: {result}\n{}",
        why(&world, &run)
    );
    assert!(
        repo.base_commits(&world)
            .iter()
            .any(|subject| subject == "chore: service"),
        "the deterministic title was not used: {:?}",
        repo.base_commits(&world)
    );
}

/// Publication, watched while it happens.
///
/// It is the longest wall-clock segment a lifecycle node has — the gate run,
/// the push, the change request, the check polling, the merge — and read once
/// at settlement every record of it appears at once, when it is over. The claim
/// here is the opposite one: a record written *during* the publication is
/// readable out of the merged store while the node is still in flight.
///
/// The gate is what is held, because the gate is the one stretch of a real
/// publication a journey can hold from outside it: `onevcs` runs the
/// repository's own command, and this one waits for a file.
#[test]
fn a_publications_own_records_reach_the_journal_while_it_is_still_publishing() {
    let world = World::new("lifecycle-livepublish");
    let go = world.fakes.join("gate.go");
    world.repository(
        "local-direct",
        &gate(&world, &["wait-for", &go.to_string_lossy()])
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    );
    world.script("service.work", "the worker wrote this\n");
    let path = world.plan(
        "watched",
        &plan_of("watched", vec![lifecycle("service", &[])]),
    );
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);

    world.until("the publication to reach its gate", |world| {
        world
            .run(&["monitor", "watched", "--all"])
            .stdout
            .contains("gate-started")
    });
    // Mid-publication, and readable: `monitor` renders the record and `status`
    // still calls the node running. Both are what an operator has open.
    let watching = world.run(&["status", "watched"]);
    watching.exited(0).out_has("service: running");
    assert!(
        !world
            .run(&["results", "watched"])
            .stdout
            .contains("service                  done"),
        "the node settled before the publication was even watched: {}",
        world.dump()
    );
    // Under the node it belongs to. A session does not know it is a graph node,
    // so every per-node reader would otherwise take a whole publication for work
    // that happened to nobody — and no view renders a relayed envelope's node,
    // so the merged store the contract defines is where that is readable.
    let started = &world.events_of("watched", "gate-started")[0];
    assert_eq!(started["labels"]["node"], "service", "{started}");
    assert_eq!(started["source"], "vcs", "{started}");

    world.release("gate.go");
    world.until("the run to settle", |world| {
        world.run_file("watched", "result.json").is_file()
    });
    // And the rest of the publication landed too, exactly once each: `monitor`
    // renders one line per event, so a record relayed twice — by a follow and by
    // the read that covers for one — shows up as two.
    let stream = world.run(&["monitor", "watched", "--all"]);
    stream.exited(0);
    for kind in [
        "lock-acquired",
        "gate-started",
        "gate-verdict",
        "push",
        "merge-completed",
        "session-closed",
    ] {
        let seen = stream
            .stdout
            .lines()
            .filter(|line| line.contains(kind))
            .count();
        assert_eq!(
            seen, 1,
            "{kind} reached the merged store {seen} time(s):\n{}",
            stream.stdout
        );
    }
    world
        .run(&["results", "watched"])
        .exited(0)
        .out_has("service")
        .out_has("done");
}

/// The last record of a session, and the recovery that covers for a follow that
/// ended before it was written.
///
/// Not a hypothetical: closing a session flips its record to `Closed` and *then*
/// emits `session-closed`, while the follow relays what the stream holds and only
/// then asks whether the session closed. Between those two the follow returns —
/// cleanly, successfully, having relayed everything but the tail. This crate read
/// that clean end as "everything was relayed" and skipped the recovery read, so
/// the merged store carried a whole publication with no close on it, and a later
/// reader had no way to tell a session that was released from one that was
/// abandoned.
///
/// The window is microseconds wide and **cannot be widened from outside**: it is
/// inside one library call now, where the `onevcs` double used to be a process a
/// journey could delay. So what this asserts is the invariant either path has to
/// satisfy — the tail arrives, once — rather than which path delivered it. The
/// arithmetic that makes "once" true when the follow *did* end early is held
/// deterministically by `relays_only_what_the_follow_did_not` in
/// `src/lifecycle.rs`.
#[test]
fn a_record_written_after_the_follow_ended_still_reaches_the_merged_store_once() {
    let world = World::new("lifecycle-latetail");
    published_locally(&world);
    world.script("service.work", "the worker wrote this\n");
    let run = settle(&world, "tail", vec![lifecycle("service", &[])]);

    // Once. Recovering the tail by re-reading the whole stream would put every
    // other record in twice, which is the same defect from the other side —
    // `monitor` renders one line per event, so a duplicate is visible as one.
    let stream = world.run(&["monitor", &run, "--all"]);
    stream.exited(0);
    // Not `session-opened`: this crate writes one of its own beside the
    // sibling's, so two is the right answer there and says nothing about relay.
    for kind in [
        "lock-wait",
        "gate-verdict",
        "push",
        "merge-completed",
        "session-closed",
    ] {
        let seen = stream
            .stdout
            .lines()
            .filter(|line| line.contains(kind))
            .count();
        assert_eq!(
            seen, 1,
            "{kind} reached the merged store {seen} time(s):\n{}",
            stream.stdout
        );
    }
    // And it belongs to the node, like every other record the session wrote:
    // the recovery read is the same relay, not a second path that forgets to
    // say whose the record is.
    let closed = &world.events_of(&run, "session-closed")[0];
    assert_eq!(closed["labels"]["node"], "service", "{closed}");
    assert_eq!(closed["source"], "vcs", "{closed}");
}

#[test]
fn an_unresolvable_repository_is_refused_before_a_run_starts() {
    let world = World::new("lifecycle-nosession");
    // No repository registered, so the node names one `onevcs` has never heard
    // of. The holders preflight is now the first `onevcs` boundary, so the
    // launcher refuses before creating a run or dispatching an agent.
    let path = world.plan(
        "nosession",
        &plan_of("nosession", vec![lifecycle("service", &[])]),
    );
    world
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .exited(2)
        // The sibling's own refusal, reaching the operator whole: the interlock
        // calls `onevcs::session_holders` rather than spawning the verb, so what
        // comes back is that library's message — which names the repository, why
        // it cannot be resolved, and the command that would fix it — instead of
        // an argv this crate composed.
        .err_has("cannot read the session holders of service")
        .err_has("not a registered repository")
        .err_has("onevcs register PATH");
    assert!(!world.run_file("nosession", "launch.json").exists());
}

#[test]
fn a_publication_that_its_gate_rejects_settles_the_node_failed_by_name() {
    let world = World::new("lifecycle-gate");
    world.repository("local-direct", &["false"]);
    world.script("service.work", "the worker wrote this\n");
    let run = settle(&world, "rejected", vec![lifecycle("service", &[])]);

    let result = world.run_json(&run, "result.json");
    assert_eq!(
        result["nodes"][0]["status"],
        "failed",
        "{result}\n{}",
        why(&world, &run)
    );
    assert_eq!(result["nodes"][0]["outcome"], "publication-failed");
    world
        .run(&["results", &run])
        .exited(0)
        .out_has("publication-failed");
}

/// A title the sibling will not commit under never reaches a dispatch.
///
/// The plan file states the title and
/// [`SUBJECT_LIMIT`](onevcs::provenance::SUBJECT_LIMIT) bounds it, so the launch
/// refuses it — naming the node, the length, and the limit — rather than the
/// publication refusing it after the node's whole dispatch and its gate. Each
/// title that is legal publishes on the same repository, because a bound is only
/// proven by the side of it that commits.
#[test]
fn a_title_the_sibling_will_not_commit_under_is_refused_before_any_dispatch() {
    let world = World::new("lifecycle-longtitle");
    let repo = world.repository("local-direct", &["true"]);
    world.script("service.work", "the worker wrote this\n");
    // A plausible planner title, padded to exactly the length this journey is
    // about: nothing else about it is wrong.
    let titled = |length: usize| {
        let mut title = "feat: land the change the worker made".to_string();
        while title.len() < length {
            title.push_str(" and then some");
        }
        title.truncate(length);
        let mut node = lifecycle("service", &[]);
        node["title"] = json!(title);
        node
    };
    let over = world.plan(
        "longtitle",
        &plan_of("longtitle", vec![titled(SUBJECT_LIMIT + 1)]),
    );
    world
        .run(&["start", &over.to_string_lossy(), "--attach"])
        .exited(REFUSED)
        .err_has("node 'service'")
        .err_has(&format!("{} characters", SUBJECT_LIMIT + 1))
        .err_has(&format!("{SUBJECT_LIMIT}-character limit"));

    assert!(
        !world.runs.join("longtitle").exists(),
        "a refused plan left a run directory behind"
    );
    assert!(
        !world.was_invoked("oneagentgraph", &["run"]),
        "a plan refused at its boundary still spent a dispatch"
    );
    assert_eq!(
        repo.base_commits(&world),
        vec!["chore: seed the repository".to_string()],
        "a title the sibling would refuse still reached the base"
    );

    // The subject inside is the last one that fits, and the spacing around it is
    // spacing the sibling trims before it measures — so the launch measures it
    // the same way, and what publishes is the subject.
    let mut fits = titled(SUBJECT_LIMIT);
    let subject = fits["title"].as_str().expect("the title").to_string();
    fits["title"] = json!(format!("  {subject}  "));
    let run = settle(&world, "fitstitle", vec![fits]);
    assert_eq!(
        world.run_json(&run, "result.json")["state"],
        "complete",
        "{}",
        why(&world, &run)
    );
    assert!(
        repo.base_commits(&world).contains(&subject),
        "the title at the limit did not reach the base: {:?}\n{}",
        repo.base_commits(&world),
        why(&world, &run)
    );

    // 100 characters: a real subject rather than one padded out of the bound, so
    // it stays an ordinary title however the bound moves.
    let ordinary = "feat(plan): refuse a node title that the publication would not commit under, \
                    before it is dispatched";
    assert!(
        ordinary.len() < SUBJECT_LIMIT,
        "this is only an ordinary title while it is inside the bound: {} characters",
        ordinary.len()
    );
    let mut plain = lifecycle("service", &[]);
    plain["title"] = json!(ordinary);
    // The run above already landed the worker's file on the base, so this one
    // needs its own content: with nothing to commit the node settles
    // `no-changes` and publishes no subject, which would leave the assertion
    // below passing for a reason that has nothing to do with the title.
    world.script("service.work", "the worker wrote this too\n");
    let run = settle(&world, "plaintitle", vec![plain]);
    assert_eq!(
        world.run_json(&run, "result.json")["state"],
        "complete",
        "{}",
        why(&world, &run)
    );
    assert!(
        repo.base_commits(&world).contains(&ordinary.to_string()),
        "an ordinary title did not reach the base: {:?}\n{}",
        repo.base_commits(&world),
        why(&world, &run)
    );
}

#[test]
fn a_node_whose_publication_failed_continues_the_branch_it_preserved() {
    let world = World::new("lifecycle-preserved");
    // The steps ran and only the merge path refused, so the work is real, it is
    // on that branch, and nothing else points at it.
    let repo = world.repository("local-direct", &["false"]);
    world.script("service.work", "the worker wrote this\n");
    let run = settle(&world, "preserved", vec![lifecycle("service", &[])]);

    let result = world.run_json(&run, "result.json");
    assert_eq!(
        result["nodes"][0]["status"],
        "failed",
        "{result}\n{}",
        why(&world, &run)
    );
    // And it claims no landing. A publication that ran and did not land settles
    // `failed` under its own name, which no reader mistakes for success — so
    // qualifying it would put a second word on a fact already stated, and
    // `unlanded` in particular would send a planner looking for a change request
    // that was never opened.
    assert_eq!(result["nodes"][0]["landing"], json!(null), "{result}");
    let failed = world.run(&["results", &run]);
    failed.exited(0).out_has("publication-failed");
    assert!(
        !failed.stdout.contains("landed"),
        "a node whose publication failed is reported as one whose change did or did not \
         land:\n{}",
        failed.stdout
    );

    let preserved = result["nodes"][0]["branch"]
        .as_str()
        .expect("the failed node named the branch it left behind")
        .to_string();

    // A `retry` that names no branch of its own continues the one the failed
    // attempt left behind. A replacement that cut a fresh branch would redo work
    // that is already committed and leave the preserved branch for a person to
    // find — the failure a planner otherwise catches by hand-writing the pin, or
    // pays for twice by missing it.
    world
        .run_with_stdin(
            &["reply", &run],
            &json!({
                "version": 1,
                "commands": [{
                    "op": "retry",
                    "id": "service",
                    "node": {"id": "service-2", "repo": "service", "persona": "engineer",
                             "task": "## What\nPublish again.\n\n## Why\nIt failed.\n\n\
                                      ## Acceptance criteria\n- published."},
                }],
            })
            .to_string(),
        )
        .exited(0);
    let committed = world
        .events_of(&run, "edit-committed")
        .into_iter()
        .find(|event| event["payload"]["command"]["op"] == "retry")
        .expect("the retry was committed");
    let node = committed["payload"]["operations"]
        .as_array()
        .expect("operations")
        .iter()
        .find(|operation| operation["kind"] == "node-added")
        .expect("the replacement was added")["node"]
        .clone();
    assert_eq!(node["id"], "service-2", "{node}");
    assert_eq!(node["resume"]["branch"], json!(preserved), "{node}");
    // The pin and the resume agree, which is the same invariant `retry` holds.
    assert_eq!(node["branch"], json!(preserved), "{node}");
    // And the branch the plan points at is one the repository actually holds:
    // a pin naming a branch nobody kept is a continuation that starts from
    // nothing while reporting that it resumed.
    assert!(
        repo.has_branch(&world, &preserved),
        "the preserved branch {preserved} was not handed back to the checkout"
    );
}

#[test]
fn a_published_node_reports_where_a_human_reads_the_change_it_opened() {
    let world = World::new("lifecycle-evidence");
    // A change request left open for review, which is what `change-open` means.
    world.repository("change-open", &["true"]);
    world.script("service.work", "the worker wrote this\n");
    let run = settle(&world, "evidence", vec![lifecycle("service", &[])]);

    // The URL the sibling handed back is the one piece of evidence a person
    // actually opens, and until it reaches the run's own record it lives only
    // inside a journal payload nobody reads by hand.
    let node = world.run_json(&run, "result.json")["nodes"][0].clone();
    let published = node["change_url"]
        .as_str()
        .unwrap_or_else(|| panic!("{node}\n{}", why(&world, &run)))
        .to_string();
    assert!(published.contains("/pull/"), "{published}");
    // A change request left open for review is an outcome of its own — the node
    // is done and the change has *not* reached its base — so it is named rather
    // than reported as a bare "published".
    assert_eq!(node["outcome"], "change-open", "{node}");
    // And it did not land: the change is open for a person to review, so the
    // status `done` is qualified rather than left to read as work that arrived.
    assert_eq!(node["landing"], "unlanded", "{node}");

    // And the host's own identifier for it, which is what a later command
    // addresses the change by. Read off the sibling's own `change-opened`
    // record: `onevcs::Publication` carries the URL and no id, so the stream is
    // where the id is a fact — see the proposal in
    // `docs/contract-divergences.md`.
    let opened = &world.events_of(&run, "change-opened")[0];
    assert!(
        opened["payload"]["id"].is_string(),
        "the publication recorded no change id: {opened}"
    );
    assert_eq!(opened["payload"]["url"], json!(published), "{opened}");

    // And it carries **no body of this crate's writing**. `onevcs` used to
    // compose one — the branch's own subject echoed back — for a request naming
    // none, and this crate would have been shipping that to reviewers as its
    // description of the change. `PublishRequest::body` is where a body would
    // come from and this crate names none, so what the host is given is empty.
    // Read off the argv the host was actually invoked with.
    let created = world
        .invocations()
        .into_iter()
        .find(|call| {
            call["tool"] == "gh"
                && call["args"]
                    .as_array()
                    .is_some_and(|args| args.first().is_some_and(|arg| arg == "pr"))
                && call["args"][1] == "create"
        })
        .unwrap_or_else(|| panic!("no change request was opened: {}", why(&world, &run)));
    let args: Vec<String> = serde_json::from_value(created["args"].clone()).expect("the argv");
    let at = args
        .iter()
        .position(|arg| arg == "--body")
        .unwrap_or_else(|| panic!("the host was given no --body at all: {args:?}"));
    assert_eq!(
        args[at + 1],
        "",
        "a publication this crate gave no body opened a change request carrying one: {args:?}"
    );

    world.run(&["results", &run]).exited(0).out_has(&published);
}

/// A publication that had nothing to publish.
///
/// `onevcs` reports `PublishOutcome::NothingToPublish` on a branch its base
/// already carries: it writes no push, no change request, and no merge. Read as
/// a success with no outcome, this crate settled it as a bare "published" — so a
/// node whose worker wrote nothing reported as one that landed work, and the
/// only way to tell was to notice that the merged store held no publication at
/// all. The real-everything smoke is where that turned up, on the first run that
/// reached a real `onevcs` with a clean tree.
#[test]
fn a_publication_that_had_nothing_to_publish_says_so_rather_than_claiming_it_landed() {
    let world = World::new("lifecycle-nothing");
    published_locally(&world);
    // Nothing is scripted for the worker to write, so the session's branch
    // carries exactly what its base does.
    let run = settle(&world, "empty", vec![lifecycle("service", &[])]);

    let node = world.run_json(&run, "result.json")["nodes"][0].clone();
    // Done: nothing failed, and there was nothing to do.
    assert_eq!(node["status"], "done", "{node}\n{}", why(&world, &run));
    assert_eq!(node["outcome"], "no-changes", "{node}");
    assert_eq!(node["change_url"], json!(null), "{node}");
    // And it claims no landing either way. There was no change of this node's,
    // so "landed" would say work reached the base that never existed and "not
    // landed" would send a planner looking for a change request nobody opened.
    assert_eq!(node["landing"], json!(null), "{node}");
    // The sibling's own record of the publication claims nothing either.
    let published = world.events_of(&run, "published");
    assert_eq!(published.len(), 1, "{}", why(&world, &run));
    assert_eq!(
        published[0]["payload"]["landing"],
        json!(null),
        "{}",
        published[0]
    );
    let results = world.run(&["results", &run]);
    results.exited(0).out_has("no-changes");
    assert!(
        !results.stdout.contains("landed"),
        "a node with nothing to publish is reported as one whose change did or did not land:\n{}",
        results.stdout
    );
}

#[test]
fn a_change_the_host_merged_settles_the_node_on_the_merge_rather_than_the_request() {
    let world = World::new("lifecycle-merged");
    world.repository("change-direct", &["true"]);
    // The host lands the change it was handed.
    world.script("gh.merged", "");
    world.script("service.work", "the worker wrote this\n");
    let run = settle(&world, "landed", vec![lifecycle("service", &[])]);

    let node = world.run_json(&run, "result.json")["nodes"][0].clone();
    assert_eq!(node["status"], "done", "{node}\n{}", why(&world, &run));
    assert_eq!(node["outcome"], "merged", "{node}");
    // The host was observed landing it, which is the one thing that makes a node
    // landed. The policy asked for the same thing in the queued journey above and
    // did not get it.
    assert_eq!(node["landing"], "landed", "{node}");
    // And the run's own record names nowhere to read it. That is what
    // `PublishOutcome::Merged` carries — the commit, not the change request — so
    // the operator-facing `change_url` a queued or open change would have is
    // empty here, and `results` renders none. It is the proposal recorded as
    // divergence 15 in `docs/contract-divergences.md`, and this assertion is
    // what fails when the sibling starts carrying the change request on a merge.
    assert_eq!(
        node["change_url"],
        json!(null),
        "a merged change now names where a human reads it; carry it into the \
         settlement and hold this journey to it: {node}"
    );
    let results = world.run(&["results", &run]);
    results.exited(0).out_has("merged");
    assert!(
        !results.stdout.contains("/pull/"),
        "`results` renders a change request the settlement does not carry:\n{}",
        results.stdout
    );

    // The change request it merged is still where a person reads what landed —
    // on the sibling's own record of opening it, which is the only place it
    // survives.
    let opened = &world.events_of(&run, "change-opened")[0];
    assert!(
        opened["payload"]["url"]
            .as_str()
            .is_some_and(|url| url.contains("/pull/")),
        "{opened}"
    );
}

#[test]
fn a_change_the_host_is_holding_settles_the_node_as_queued() {
    let world = World::new("lifecycle-queued");
    // `change-auto` asks the host to land it once its checks pass, and this host
    // has not landed it.
    world.repository("change-auto", &["true"]);
    world.script("service.work", "the worker wrote this\n");
    let run = settle(&world, "queued", vec![lifecycle("service", &[])]);

    // The host has it and will land it once its checks pass. The node is done —
    // there is nothing more for the run to do with it — and it says so as queued
    // rather than as merged, which would claim the base already carries it.
    let node = world.run_json(&run, "result.json")["nodes"][0].clone();
    assert_eq!(node["status"], "done", "{node}\n{}", why(&world, &run));
    assert_eq!(node["outcome"], "queued", "{node}");
    assert!(
        node["change_url"]
            .as_str()
            .is_some_and(|url| url.contains("/pull/")),
        "a queued change named nowhere to read it: {node}"
    );
    // Done, and not landed. The host has accepted it and the base does not carry
    // it yet, so the settlement says both things rather than only the first.
    assert_eq!(node["landing"], "unlanded", "{node}");
}

/// The document a consumer reads carries the landing, at a stated version, and
/// says nothing where there was nothing to observe.
///
/// This is the **read interface**. Execution is continuous, so there is no round
/// to serve a result for: the ledger holds one `result.json` per run, rewritten
/// whenever the driver closes out, and that document is what a consumer driving
/// the engine parses. Its version is the only statement they get about what
/// changed in it.
///
/// All three of the landing's cases are read here. The first run carries two: a
/// change the host is holding, and a plain agent node that published nothing and
/// therefore carries **no `landing` key at all**. The third — a change observed
/// on its base — is the second half, under the same policy once the host lands
/// what it is handed, because a version that round-trips one value and drops the
/// other is the defect a golden exists to catch.
///
/// Each half gets its own world: the rendezvous holding a run's driver is
/// per-world, and two halves sharing one would have the second launch wait on the
/// first run's driver.
#[test]
fn the_run_result_a_consumer_reads_states_its_version_and_carries_the_landing() {
    let node = |recorded: &serde_json::Value, id: &str| {
        recorded["nodes"]
            .as_array()
            .expect("the result carries nodes")
            .iter()
            .find(|node| node["id"] == id)
            .unwrap_or_else(|| panic!("{id} is missing from {recorded}"))
            .clone()
    };

    // The host is holding the change it was handed.
    let world = World::new("lifecycle-result-contract");
    world.repository("change-auto", &["true"]);
    world.script("service.work", "the worker wrote this\n");
    let (open, launched) = driven(
        &world,
        "readapi",
        vec![lifecycle("service", &[]), agent("build", &[])],
    );
    launched.settled();
    let recorded = world.run_json(&open, "result.json");
    assert_eq!(
        recorded["schema_version"], 3,
        "the run result a consumer parses states no version, or not this one: {recorded}"
    );
    assert!(
        recorded.get("round").is_none(),
        "the run's own result document names a round: {recorded}"
    );
    assert_eq!(
        node(&recorded, "service")["landing"],
        "unlanded",
        "{recorded}"
    );
    // A node that published nothing carries no landing *key*: a `null` on every
    // node would have a consumer branching on a field that is almost always
    // meaningless, which is how the distinction stops being read at all.
    assert!(
        node(&recorded, "build").get("landing").is_none(),
        "a node with no change to land carries a landing key anyway: {}",
        node(&recorded, "build")
    );

    // The same policy, and this time the host lands what it is handed.
    let world = World::new("lifecycle-result-contract-landed");
    world.repository("change-auto", &["true"]);
    world.script("gh.merged", "");
    world.script("service.work", "the worker wrote this\n");
    let (landed, launched) = driven(&world, "readapi", vec![lifecycle("service", &[])]);
    launched.settled();
    let recorded = world.run_json(&landed, "result.json");
    assert_eq!(recorded["schema_version"], 3, "{recorded}");
    assert_eq!(
        node(&recorded, "service")["landing"],
        "landed",
        "{recorded}"
    );
}

/// A settled node and a landed node are different facts, and one publication
/// policy produces both.
///
/// The identity is `change-auto` for **both** halves — it asks the host to land
/// the change once its checks pass — so nothing about the ask distinguishes
/// them. What distinguishes them is the host: in the first half it holds the
/// change, and in the second it lands it. Both nodes settle `done`, because
/// publishing is the whole of what the round asked of them, and only one of them
/// put anything on `main`.
///
/// Everything a planner reads is checked, because closing work on a settled node
/// is a decision made from any of them: the ledger record, the round result the
/// read API serves, and every view that renders a node's status.
///
/// Nothing waits for the merge. The unlanded half settles and the round ends with
/// the change still open, because a change request a person owns is not something
/// a run may block or poll on.
#[test]
fn a_settled_node_and_a_landed_node_are_told_apart_by_what_the_host_did_not_by_the_policy() {
    let world = World::new("lifecycle-landing");
    // Asks the host to land it once its checks pass. One policy, both answers.
    world.repository("change-auto", &["true"]);

    // The host is holding the change and has not landed it.
    world.script("service.work", "the change nobody merged\n");
    // Named for the scenario and not for the answer: a run id is printed on every
    // view line, so `heldopen` cannot satisfy an assertion looking for the word
    // this journey is about.
    let open = settle(&world, "heldopen", vec![lifecycle("service", &[])]);

    // The sibling's own record of the publication says it too, so a reader
    // watching the merged stream sees where the change got to at the moment it
    // was published rather than only in the settlement folded from it.
    let published = world.events_of(&open, "published");
    assert_eq!(
        published.len(),
        1,
        "the publication is missing from the merged store\n{}",
        why(&world, &open)
    );
    assert_eq!(
        published[0]["payload"]["landing"], "unlanded",
        "{}",
        published[0]
    );

    let settled = world.events_of(&open, "node-settled");
    let record = settled
        .iter()
        .find(|event| event["labels"]["node"] == "service")
        .unwrap_or_else(|| panic!("the node never settled\n{}", why(&world, &open)));
    assert_eq!(record["payload"]["status"], "done", "{record}");
    assert_eq!(
        record["payload"]["landing"], "unlanded",
        "the ledger records a change that reached nobody as though it had landed: {record}"
    );

    let node = world.run_json(&open, "result.json")["nodes"][0].clone();
    assert_eq!(node["status"], "done", "{node}\n{}", why(&world, &open));
    assert_eq!(
        node["landing"], "unlanded",
        "the read API serves a settled node and a landed one as the same node: {node}"
    );

    // Every view a planner decides from. `results` is the per-node one; `status`
    // otherwise reports only what is in flight, so a run gone quiet reads as one
    // whose work landed; `goals` is the `n/n done` line that says a run is
    // finished; `monitor` is the stream.
    world
        .run(&["results", &open])
        .exited(0)
        .out_has("NOT landed");
    world
        .run(&["status", &open])
        .exited(0)
        .out_has("1 node(s) settled without landing: service");
    world
        .run(&["goals", &open])
        .exited(0)
        .out_has("1 not landed");
    world
        .run(&["monitor", &open, "--all"])
        .exited(0)
        .out_has("unlanded");

    // The same policy, and this time the host lands what it is handed.
    world.script("gh.merged", "");
    world.script("service.work", "the change the host merged\n");
    let landed = settle(&world, "hostmerged", vec![lifecycle("service", &[])]);

    let node = world.run_json(&landed, "result.json")["nodes"][0].clone();
    assert_eq!(node["status"], "done", "{node}\n{}", why(&world, &landed));
    assert_eq!(
        node["landing"],
        "landed",
        "a change the host was observed landing is not reported as landed: {node}\n{}",
        why(&world, &landed)
    );
    assert_eq!(
        world.events_of(&landed, "published")[0]["payload"]["landing"],
        "landed",
        "{}",
        why(&world, &landed)
    );
    let results = world.run(&["results", &landed]);
    results.exited(0).out_has("landed on its base");
    assert!(
        !results.stdout.contains("NOT landed"),
        "a landed change is reported as one that did not land:\n{}",
        results.stdout
    );
    let status = world.run(&["status", &landed]);
    status.exited(0);
    assert!(
        !status.stdout.contains("settled without landing"),
        "a run with nothing outstanding is reported as holding an unlanded change:\n{}",
        status.stdout
    );
    let goals = world.run(&["goals", &landed]);
    goals.exited(0).out_has("1/1 done");
    assert!(
        !goals.stdout.contains("not landed"),
        "a run whose only change landed still counts one against it:\n{}",
        goals.stdout
    );

    // And side by side, which is how `runs` presents them: the two runs did the
    // same work under the same policy, and only one of them still owes somebody
    // a merge.
    let listed = world.run(&["runs"]);
    listed.exited(0);
    let row = |run: &str| {
        listed
            .stdout
            .lines()
            .find(|line| line.contains(run))
            .unwrap_or_else(|| panic!("`runs` never listed {run}:\n{}", listed.stdout))
            .to_string()
    };
    assert!(
        row(&open).contains("1 not landed"),
        "`runs` reports a run holding an open change as finished work:\n{}",
        row(&open)
    );
    assert!(
        !row(&landed).contains("not landed"),
        "`runs` reports a run whose change landed as one that did not:\n{}",
        row(&landed)
    );
}

#[test]
fn an_explicit_pin_the_planner_wrote_wins_over_a_branch_a_dispatch_preserved() {
    let world = World::new("lifecycle-pin-wins");
    world.repository("local-direct", &["false"]);
    world.script("service.work", "the worker wrote this\n");
    let mut node = lifecycle("service", &[]);
    node["branch"] = json!("feature/the-planner-said-so");
    let run = settle(&world, "pinwins", vec![node]);

    // Naming a branch is a decision somebody made. What continues the work keeps
    // it, and carries no `resume` pointing somewhere else — the two disagreeing
    // is the state `retry` already refuses to construct.
    world
        .run_with_stdin(
            &["reply", &run],
            &json!({
                "version": 1,
                "commands": [{
                    "op": "retry",
                    "id": "service",
                    "node": {"id": "service-2", "repo": "service", "persona": "engineer",
                             "task": "## What\nPublish again.\n\n## Why\nIt failed.\n\n\
                                      ## Acceptance criteria\n- published."},
                }],
            })
            .to_string(),
        )
        .exited(0);
    let committed = world
        .events_of(&run, "edit-committed")
        .into_iter()
        .find(|event| event["payload"]["command"]["op"] == "retry")
        .expect("the retry was committed");
    let node = committed["payload"]["operations"]
        .as_array()
        .expect("operations")
        .iter()
        .find(|operation| operation["kind"] == "node-added")
        .expect("the replacement was added")["node"]
        .clone();
    assert_eq!(
        node["branch"],
        json!("feature/the-planner-said-so"),
        "{node}"
    );
    assert_eq!(
        node["resume"]["branch"],
        json!("feature/the-planner-said-so")
    );
}

#[test]
fn a_step_dispatches_under_its_own_agent_graph_before_its_nodes() {
    let world = World::new("lifecycle-graphs");
    published_locally(&world);
    let node_graph = world.root.join("workstream-graph.yaml");
    let step_graph = world.root.join("one-step-graph.yaml");
    for path in [&node_graph, &step_graph] {
        std::fs::copy(crate::harness::repo_file("graphs/node-scope.yaml"), path)
            .expect("the override config is written");
    }

    // The node names one for its whole workstream; one step overrides it. The
    // narrower statement wins, which is what makes a per-step override worth
    // stating at all.
    let node = json!({
        "id": "service",
        "repo": "service",
        "agent_graph": node_graph.to_string_lossy(),
        "steps": [
            {"id": "implement", "persona": "engineer", "task": "## What\nimplement"},
            {
                "id": "review",
                "persona": "reviewer",
                "task": "## What\nreview",
                "deps": ["implement"],
                "agent_graph": step_graph.to_string_lossy(),
            },
        ],
    });
    settle(&world, "stepgraphs", vec![node]);

    let by_step: Vec<(String, String)> = world
        .invocations()
        .iter()
        .filter(|call| call["tool"] == "oneagentgraph" && call["args"][0] == "run")
        .filter_map(|call| {
            let graph = call["args"][1].as_str()?.to_string();
            let step = call["args"]
                .as_array()?
                .iter()
                .filter_map(|arg| arg.as_str())
                .find_map(|arg| arg.strip_prefix("onepipeline.step="))?
                .to_string();
            Some((step, graph))
        })
        .collect();

    let graph_of = |step: &str| {
        by_step
            .iter()
            .find(|(id, _)| id == step)
            .unwrap_or_else(|| panic!("{step} never dispatched: {by_step:?}"))
            .1
            .clone()
    };
    assert_eq!(graph_of("implement"), node_graph.to_string_lossy());
    assert_eq!(graph_of("review"), step_graph.to_string_lossy());
}

#[test]
fn a_lifecycle_node_carries_the_pins_the_plan_states_into_its_session() {
    let world = World::new("lifecycle-pins");
    let repo = world.repository("change-auto", &["true"]);
    // A second checkout of the same identity, and a second base branch on the
    // origin: a pin naming either is only a pin if there is something for it to
    // name.
    crate::harness::git(&world, &repo.checkout, &["branch", "release", "main"]);
    crate::harness::git(&world, &repo.checkout, &["push", "origin", "release"]);
    let primary = world.root.join("primary");
    crate::harness::git(
        &world,
        &world.root,
        &["clone", &repo.origin.to_string_lossy(), "primary"],
    );
    // A session's clone is cut from the execution checkout with `--shared`,
    // which copies that checkout's *local* branches and not its remote-tracking
    // ones — so a base a session is asked to cut from has to be a branch the
    // execution checkout itself holds.
    crate::harness::git(&world, &primary, &["branch", "release", "origin/release"]);
    world.register(&primary, Some("https://github.com/owner/service.git"));

    world.script("service.work", "the worker wrote this\n");
    let mut node = lifecycle("service", &[]);
    node["branch"] = json!("feature/pinned");
    node["base_branch"] = json!("release");
    node["execution_checkout"] = json!("primary");
    node["merge_policy"] = json!("change-auto");
    let run = settle(&world, "pinned", vec![node]);

    // What the pins actually did, rather than which arguments carried them: the
    // session was cut on the branch and base the plan named, and the policy it
    // published under is the one the plan asked for.
    let opened = world
        .journal(&run)
        .into_iter()
        .find(|event| event["source"] == "vcs" && event["kind"] == "session-opened")
        .unwrap_or_else(|| panic!("no session was opened\n{}", why(&world, &run)));
    assert_eq!(opened["payload"]["branch"], "feature/pinned", "{opened}");
    assert_eq!(opened["payload"]["base"], "release", "{opened}");
    assert!(
        opened["payload"]["worktree"]
            .as_str()
            .is_some_and(|path| path.contains("worktree")),
        "{opened}"
    );

    let published = &world.events_of(&run, "published")[0];
    assert_eq!(
        published["payload"]["policy"], "change-auto",
        "the merge policy did not reach onevcs: {published}"
    );
    // And the execution checkout the plan pinned is the one the session was cut
    // from: the primary clone carries the branch, and the one it was not cut
    // from does not.
    assert!(
        world.invocations().iter().any(|call| call["tool"] == "gh"),
        "a change-auto publication asked the host for nothing"
    );
}

#[test]
fn a_session_stream_that_cannot_be_read_is_reported_and_does_not_fail_the_node() {
    let world = World::new("lifecycle-noevents");
    // llmlint: ignore-block[tests_mirror_real_usage] the gate *is* the product's own
    // extension point — a repository's rules file names a command and `onevcs` runs it on
    // the merge path — so what this states is a repository whose own gate breaks the state
    // root under it, which is operator-supplied code doing what operator-supplied code
    // can. No command breaks a stream: every deletion `onevcs` performs is a run root, an
    // integrate or publish scratch, or a rotated gate log, and none touches `streams/`.
    // Nor can it be arranged before the run — a stream directory already broken fails
    // `Stream::open` inside `session open`, which is a session that never opened and a
    // different journey. So the gate is the point inside a run where the repository's own
    // code can reach it. Everything asserted is through the binary.
    //
    // A file where the streams directory was, so nothing can recreate it:
    // `EventStream::open` then refuses every session by name.
    let gate = gate(&world, &["break-streams"]);
    world.repository(
        "local-direct",
        &gate.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    // llmlint: ignore-end[tests_mirror_real_usage]
    world.script("service.work", "the worker wrote this\n");
    let run = driven(&world, "silentstream", vec![lifecycle("service", &[])]);

    // Said out loud: a silent gap in the merged store is what makes a later
    // reader think nothing happened.
    run.1.err_has("cannot read session").err_has("events");

    // The evidence is missing, not the result: the node published and settled.
    let run = run.0;
    let result = world.run_json(&run, "result.json");
    assert_eq!(result["state"], "complete", "{result}");
    // `gate-verdict` is the first record the session writes after the gate has
    // taken its own stream away, so its absence from the store is the
    // unreadable stream rather than a publication that did not happen.
    assert!(
        !world
            .journal(&run)
            .iter()
            .any(|event| event["kind"] == "gate-verdict"),
        "the unreadable stream still contributed events"
    );
}

/// A line on a session's stream that this build cannot read.
///
/// `onevcs::EventStream` is a **typed** reader: a line it cannot parse refuses
/// the whole read, and its cursor has already moved past that line — so the
/// whole records read alongside it are refused with it and never handed back.
/// That is the sibling's decision and it is recorded as a proposal in
/// `docs/contract-divergences.md`; what this journey holds is the part that is
/// this crate's: the node does not fail, and the loss is not silent.
#[test]
fn a_session_line_this_build_cannot_read_is_reported_and_does_not_fail_the_node() {
    let world = World::new("lifecycle-futureline");
    // llmlint: ignore-block[tests_mirror_real_usage] the same extension point as the
    // journey above, and the same reason: a repository's gate is a command an operator
    // wrote, and a stream carrying a record this build cannot read is what an
    // `ONEVCS_HOME` shared with another build of `onevcs` leaves behind. No command
    // writes one — `Stream::emit` is the only writer and it appends whole envelopes, so a
    // surface that could would be the defect — and the stream exists only once the
    // session has opened, which makes the gate the point inside a run where the
    // repository's own code can reach it. Everything asserted is through the binary.
    //
    // The token is the name of the directory above the worktree the gate runs in.
    let gate = gate(&world, &["append-future-event"]);
    world.repository(
        "local-direct",
        &gate.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    // llmlint: ignore-end[tests_mirror_real_usage]
    world.script("service.work", "the worker wrote this\n");
    let run = driven(&world, "futurestream", vec![lifecycle("service", &[])]);
    run.1.err_has("is not an event envelope");
    let run = run.0;

    // A sibling emitting a shape this build does not know must not stop the
    // node, and must not vanish silently either.
    assert_eq!(world.run_json(&run, "result.json")["state"], "complete");
    assert!(
        world
            .journal(&run)
            .iter()
            .any(|event| event["source"] == "vcs" && event["kind"] == "published"),
        "the publication still reached the merged store"
    );
}

/// Every `(node, step)` a dispatch was asked for, in the order they were asked.
fn steps_dispatched(world: &World) -> Vec<(String, String)> {
    world
        .invocations()
        .iter()
        .filter(|call| call["tool"] == "oneagentgraph" && call["args"][0] == "run")
        .filter_map(|call| {
            let args: Vec<&str> = call["args"]
                .as_array()?
                .iter()
                .filter_map(|arg| arg.as_str())
                .collect();
            let node = args
                .iter()
                .find_map(|a| a.strip_prefix("onepipeline.node="))?;
            let step = args
                .iter()
                .find_map(|a| a.strip_prefix("onepipeline.step="))?;
            Some((node.to_string(), step.to_string()))
        })
        .collect()
}

#[test]
fn a_continuation_skips_the_steps_the_preserved_branch_already_carries() {
    let world = World::new("lifecycle-resume-steps");
    published_locally(&world);
    // The second step fails, so the node settles failed with the first step's
    // work committed on the branch the session preserved.
    world.script("service.implement.work", "the worker wrote this\n");
    world.script("service.review.fail", "1");
    let node = json!({
        "id": "service",
        "repo": "service",
        "steps": [
            {"id": "implement", "persona": "engineer", "task": "## What\nimplement"},
            {"id": "review", "persona": "reviewer", "task": "## What\nreview", "deps": ["implement"]},
        ],
    });
    let run = settle(&world, "resumed", vec![node]);
    assert_eq!(
        world.run_json(&run, "result.json")["nodes"][0]["status"],
        "failed"
    );

    // The continuation names what the branch carries, and nothing it does not:
    // `review` never finished, so it is not on the list. A `retry` naming no
    // branch of its own inherits both from the attempt that preserved them.
    std::fs::remove_file(world.fakes.join("service.review.fail")).expect("the failure is cleared");
    world
        .run_with_stdin(
            &["reply", &run],
            &json!({
                "version": 1,
                "commands": [{
                    "op": "retry",
                    "id": "service",
                    "node": {"id": "service-2", "repo": "service", "steps": [
                        {"id": "implement", "persona": "engineer", "task": "## What\nimplement"},
                        {"id": "review", "persona": "reviewer", "task": "## What\nreview",
                         "deps": ["implement"]},
                    ]},
                }],
            })
            .to_string(),
        )
        .exited(0);
    let committed = world
        .events_of(&run, "edit-committed")
        .into_iter()
        .find(|event| event["payload"]["command"]["op"] == "retry")
        .expect("the retry was committed");
    let resume = committed["payload"]["operations"]
        .as_array()
        .expect("operations")
        .iter()
        .find(|operation| operation["kind"] == "node-added")
        .expect("the replacement was added")["node"]["resume"]
        .clone();
    assert_eq!(
        resume["completed_steps"],
        json!(["implement"]),
        "the continuation does not say what the branch carries: {resume}"
    );
    assert!(
        resume["branch"].is_string(),
        "a resume with completed steps but no branch: {resume}"
    );

    // The run had already settled on the failure, so nothing was driving it when
    // the retry landed: a fresh driver picks the edited graph up and runs it.
    // `implement` is on the branch already, so re-running it would redo work —
    // only `review` goes out.
    world.run(&["adopt", &run]).exited(0);
    assert!(
        world
            .events_of(&run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "service-2"),
        "the continuation never ran:\n{}",
        why(&world, &run)
    );
    let dispatched = steps_dispatched(&world);
    assert_eq!(
        dispatched
            .iter()
            .filter(|(node, step)| node == "service-2" && step == "implement")
            .count(),
        0,
        "the continuation re-ran a step the branch already carries: {dispatched:?}"
    );
    assert_eq!(
        dispatched
            .iter()
            .filter(|(node, step)| node == "service-2" && step == "review")
            .count(),
        1,
        "the continuation did not re-run the step that failed: {dispatched:?}"
    );
}
