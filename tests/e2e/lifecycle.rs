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

use std::path::PathBuf;

use crate::harness::{agent, gate_script, lifecycle, plan_of, Repository, World, REFUSED};
use onevcs::provenance::SUBJECT_LIMIT;
use serde_json::json;

/// The subject `onevcs` derives for a publication that states none.
///
/// The sibling's own, composed from the branch it is publishing rather than from
/// anything this crate said — which is the point: a node that states no title
/// lands under a subject only the repository side could have written. Spelled
/// here so a journey can hold the whole subject rather than a prefix, and so a
/// sibling that changes what it derives fails a test that says why instead of
/// one that reads differently.
/// The branch a run before this one preserved its work on.
///
/// Named once because two journeys pin it and both then assert on the name: a
/// branch spelled twice is a journey that passes when the pin never took.
const KEPT: &str = "feature/kept";

fn derived_subject(branch: &str) -> String {
    format!("chore: preserve work on {branch}")
}

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

/// One node, with the title its change request opens under.
///
/// A lifecycle node needs one from schema version 3 on, and the journeys here
/// are about the *body*, so the title is stated once here rather than in each.
fn titled(node: serde_json::Value, title: &str) -> serde_json::Value {
    let mut node = node;
    node["title"] = json!(title);
    node
}

/// The directory each dispatch of one run ran in, by the persona it ran under.
///
/// Read off the dispatch's own relayed envelopes, which carry the directory the
/// member was given — so "the drafting dispatch ran where the work was done" is
/// two recorded directories compared, rather than a name this suite recognised.
fn dispatch_directories(world: &World, run: &str) -> Vec<(String, String)> {
    world
        .journal(run)
        .iter()
        .filter(|event| event["source"] == "agentgraph")
        .filter_map(|event| {
            Some((
                event["labels"]["persona"].as_str()?.to_string(),
                event["payload"]["dir"].as_str()?.to_string(),
            ))
        })
        .fold(Vec::new(), |mut seen, (persona, dir)| {
            if !seen.iter().any(|(known, _)| known == &persona) {
                seen.push((persona, dir));
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

    // This launch named no drafting graph, which is the shipped default and not
    // a failure: nothing was drafted and nothing is reported about it. The
    // change request that opens with no body here is the one this kind must not
    // be read off.
    let reported = world.events_of(&run, "body-not-drafted");
    assert!(
        reported.is_empty(),
        "a launch that named no pr-author graph reported a drafting failure: {reported:?}"
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
        "title": "feat: land the workstream",
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
///
/// It is also the one world where the **drafting** dispatch has no worktree to
/// run in — the node's own is what it reads the diff from, and there is no
/// record left to name one — so the launch names a graph to draft with and this
/// journey holds that ending too: the change request keeps its title and gets no
/// body, said out loud rather than dispatching an agent to read an empty diff.
#[test]
fn a_session_record_that_cannot_be_read_falls_back_to_opening_a_session() {
    use std::process::Stdio;

    let world = World::new("lifecycle-norecord");
    published_locally(&world);
    let drafting = world.pr_author_graph();
    world.script("service.implement.wait", "hold");
    world.script("driver.wait", "hold");
    let node = json!({
        "id": "service",
        "repo": "service",
        // The title its change request opens under, which a lifecycle node
        // states from plan schema 3 on.
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
        .cmd(&[
            "start",
            &path.to_string_lossy(),
            "--attach",
            "--pr-author-graph",
            &drafting,
        ])
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
    // And the drafting dispatch, which had nowhere to read the diff from, said
    // so rather than running somewhere it could not read one.
    assert!(
        said.contains("no worktree to draft its change request in"),
        "a launch that named a drafting graph drafted nothing and said nothing:\n{said}"
    );
    assert!(
        !world.was_invoked(
            "oneagentgraph",
            &["--label", "onepipeline.persona=pr-author"]
        ),
        "a drafting dispatch ran with no worktree to read a diff in"
    );
    // And it is in the run's own record as well as on stderr, under the ending
    // that says the dispatch is what could not happen.
    let undrafted = world.events_of("norecord", "body-not-drafted");
    assert_eq!(undrafted.len(), 1, "{undrafted:?}");
    assert_eq!(undrafted[0]["payload"]["ending"], "dispatch-failed");
    assert_eq!(undrafted[0]["labels"]["node"], "service");
    let detail = undrafted[0]["payload"]["detail"]
        .as_str()
        .unwrap_or_default();
    assert!(
        detail.contains("no worktree to read this branch's diff in"),
        "the recorded ending does not say what could not happen: {detail}"
    );
    let settled = world.events_of("norecord", "node-settled");
    // After the publication's own words, where it had any: what settled the node
    // leads, and the drafting failure is added to it — the same order
    // `publication_failed` composes the two in.
    let settled_detail = settled[0]["payload"]["detail"].as_str().unwrap_or_default();
    assert!(
        settled_detail.ends_with(detail),
        "the settlement of a node with nowhere to draft in did not name it:          {settled_detail}"
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
        "title": "feat: land the workstream",
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

/// A change request the drafting dispatch wrote the body of.
///
/// `change-open` rather than `local-direct`, because a body is prose on a change
/// request and a direct merge opens none: the far side of this publication is
/// the host, and what it was asked to open the change request with is where a
/// drafted body is a fact rather than an argument this crate passed.
#[test]
fn the_pr_author_dispatch_drafts_the_body_the_change_request_opens_with() {
    let world = World::new("lifecycle-pr-author");
    world.repository("change-open", &["true"]);
    world.script("service.work", "the worker wrote this\n");
    world.script("pr-author.body", "## What\nRead off the branch's diff.\n");
    let drafting = world.pr_author_graph();
    let node = titled(lifecycle("service", &[]), "feat: land what the worker made");
    let path = world.plan("authored", &plan_of("authored", vec![node]));
    world
        .run(&[
            "start",
            &path.to_string_lossy(),
            "--attach",
            "--pr-author-graph",
            &drafting,
        ])
        .settled();

    // It ran the graph the launch named, rather than the node-scope one every
    // other dispatch runs.
    let drafts: Vec<serde_json::Value> = world
        .invocations()
        .into_iter()
        .filter(|call| {
            call["tool"] == "oneagentgraph"
                && call["args"].as_array().is_some_and(|args| {
                    args.iter()
                        .any(|arg| arg == "onepipeline.persona=pr-author")
                })
        })
        .collect();
    assert_eq!(
        drafts.len(),
        1,
        "the drafting dispatch did not run exactly once: {:?}",
        world.invocations()
    );
    assert_eq!(
        drafts[0]["args"][1], drafting,
        "the drafting dispatch ran a graph the launch did not name: {}",
        drafts[0]
    );

    // And it ran in the node's **own** worktree, which is the only place the
    // diff it was asked to read exists: the same directory the worker wrote in,
    // proven by comparing the two rather than by reading the name.
    let dirs = dispatch_directories(&world, "authored");
    let whose = |persona: &str| {
        dirs.iter()
            .find(|(who, _)| who == persona)
            .map(|(_, dir)| dir.clone())
            .unwrap_or_else(|| panic!("no {persona} dispatch recorded a directory: {dirs:?}"))
    };
    assert_eq!(
        whose("pr-author"),
        whose("engineer"),
        "the drafting dispatch did not run where the work was done: {dirs:?}"
    );
    assert!(
        whose("pr-author").contains("worktree"),
        "the drafting dispatch did not run in the session's worktree: {dirs:?}"
    );

    // And the body it drafted is what the change request opened with.
    let opened = world.changes_opened();
    assert_eq!(opened.len(), 1, "{opened:?}");
    assert_eq!(
        opened[0]["body"],
        "## What\nRead off the branch's diff.",
        "the drafted body did not reach the change request: {opened:?}\n{}",
        why(&world, "authored")
    );
    assert_eq!(opened[0]["title"], "feat: land what the worker made");
    assert_eq!(
        world.run_json("authored", "result.json")["state"],
        "complete"
    );
    // A drafting dispatch that produced a body is not a failure, so nothing
    // reports it: the kind exists for the three endings that produce none.
    assert!(
        world.events_of("authored", "body-not-drafted").is_empty(),
        "a body that was drafted was reported as one that was not"
    );
}

/// A launch that names no drafting graph is the shipped default, and it drafts
/// nothing: the change request opens with the plan's own body, or with none.
#[test]
fn a_node_that_states_its_own_body_publishes_it_and_spends_no_dispatch() {
    let world = World::new("lifecycle-body");
    world.repository("change-open", &["true"]);
    world.script("service.work", "the worker wrote this\n");
    let mut node = titled(
        lifecycle("service", &[]),
        "feat: land what the planner asked for",
    );
    node["body"] = json!("## What\nThe planner wrote this.");
    let path = world.plan("bodied", &plan_of("bodied", vec![node]));
    // The graph *is* named, so the only reason no dispatch runs is the body the
    // node already carries.
    let drafting = world.pr_author_graph();
    world
        .run(&[
            "start",
            &path.to_string_lossy(),
            "--attach",
            "--pr-author-graph",
            &drafting,
        ])
        .settled();

    let opened = world.changes_opened();
    assert_eq!(opened.len(), 1, "{opened:?}");
    assert_eq!(
        opened[0]["body"], "## What\nThe planner wrote this.",
        "the node's own body was not what the change request opened with: {opened:?}"
    );
    assert!(
        !world.was_invoked(
            "oneagentgraph",
            &["--label", "onepipeline.persona=pr-author"]
        ),
        "a body the planner wrote still spent a drafting dispatch"
    );
    // And nothing was reported about the drafting that never happened: a node
    // that wrote its own body spent no dispatch, which is not a failure.
    let reported = world.events_of("bodied", "body-not-drafted");
    assert!(
        reported.is_empty(),
        "a node that wrote its own body was reported as an undrafted one: {reported:?}"
    );
}

/// Every way the drafting dispatch can end badly: the publication proceeds, and
/// the run says which ending it was.
///
/// It runs after the branch is verified and is not on the publication path, so
/// each of these is a change request that opens with no body and a node that
/// settles on its publication as before. What each of them is *not* is silent:
/// the three endings need three different fixes — a drafter that will not
/// finish, one whose answer the schema refuses, and one that answers inside the
/// schema with nothing in it — so each is named twice, on `body-not-drafted` and
/// on the node's own settlement, where `results` renders it.
#[test]
fn a_drafting_dispatch_that_ends_badly_leaves_the_publication_untouched() {
    for (name, scenario, ending, says) in [
        (
            "failed",
            "service.pr-author.fail",
            "dispatch-failed",
            "settled without succeeding",
        ),
        (
            "unschematic",
            "pr-author.unschematic",
            "schema-refused",
            "answered nothing the schema it was validated against accepted",
        ),
        // A chain the schema refused once and then accepted, with nothing in
        // the answer it accepted: the schema is working, so the ending is the
        // drafter's and a reader is not sent to correct a schema instead.
        (
            "bodyless",
            "pr-author.bodyless",
            "no-body",
            "succeeded and there was no body in what it answered with",
        ),
    ] {
        let world = World::new(&format!("lifecycle-draft-{name}"));
        world.repository("change-open", &["true"]);
        world.script("service.work", "the worker wrote this\n");
        world.script(scenario, "1");
        let drafting = world.pr_author_graph();
        let node = titled(lifecycle("service", &[]), "feat: land it anyway");
        let path = world.plan(name, &plan_of(name, vec![node]));
        let launched = world.run(&[
            "start",
            &path.to_string_lossy(),
            "--attach",
            "--pr-author-graph",
            &drafting,
        ]);
        launched.settled();

        let result = world.run_json(name, "result.json");
        assert_eq!(
            result["state"],
            "complete",
            "a drafting dispatch that {name} blocked publication: {result}\n{}",
            why(&world, name)
        );
        let opened = world.changes_opened();
        assert_eq!(opened.len(), 1, "{opened:?}");
        assert_eq!(
            opened[0]["body"], "",
            "a drafting dispatch that {name} still put a body on the change request: {opened:?}"
        );
        assert_eq!(opened[0]["title"], "feat: land it anyway");

        // Once, under the node it happened to, naming which of the three
        // endings it was — the whole reason they are not collapsed into one.
        let reported = world.events_of(name, "body-not-drafted");
        assert_eq!(
            reported.len(),
            1,
            "a drafting dispatch that {name} was not reported: {reported:?}"
        );
        assert_eq!(reported[0]["labels"]["node"], "service", "{}", reported[0]);
        assert_eq!(
            reported[0]["payload"]["ending"], ending,
            "a drafting dispatch that {name} was reported under the wrong ending: {}",
            reported[0]
        );
        let detail = reported[0]["payload"]["detail"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            detail.contains(says),
            "the {name} ending did not say what it was: {detail}"
        );

        // And beside the event, on the settlement — which is what a planner
        // reading `results` is shown without opening the run's store.
        let settled = world.events_of(name, "node-settled");
        let settlement = settled[0]["payload"]["detail"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            settlement, detail,
            "the settlement of a drafting dispatch that {name} did not name the ending"
        );
        world
            .run(&["results", name])
            .exited(0)
            .out_has("was not drafted");
    }
}

/// The launch config declares the drafting graph, the flag overrides it, and
/// both are resolved against the directory the launch was made from.
///
/// The same pair every other launch-level decision has: a team writes the graph
/// down beside its plan, and one launch says otherwise on the command line
/// without restating the rest of the document. Both references are **relative**,
/// which is how a document beside a plan names a document beside a plan — and
/// what the launch records is the resolved path, because every later driver
/// replays that record from wherever it happens to be started.
#[test]
fn a_launch_config_names_the_drafting_graph_and_the_flag_overrides_it() {
    let world = World::new("lifecycle-draft-config");
    world.repository("change-open", &["true"]);
    world.script("service.work", "the worker wrote this\n");
    let declared = world.pr_author_graph();
    // A second document, so "which graph ran" is a question with two answers.
    let overriding = world.graphs().join("pr-author-override.yaml");
    std::fs::copy(&declared, &overriding).expect("the overriding graph is written");
    let config = world.root.join("launch.yaml");
    // At the version that declares the key: `pr_author_graph` is what schema 2
    // added, and a document below it naming one is refused by that key's name.
    std::fs::write(
        &config,
        "schema_version: 2\npr_author_graph: graphs/pr-author.yaml\n",
    )
    .expect("the launch config is written");

    let drafted = |run: &str, extra: &[&str]| -> String {
        let node = titled(lifecycle("service", &[]), "feat: land it");
        let path = world.plan(run, &plan_of(run, vec![node]));
        let mut args = vec![
            "start".to_string(),
            path.to_string_lossy().into_owned(),
            "--attach".to_string(),
            "--launch-config".to_string(),
            config.to_string_lossy().into_owned(),
        ];
        args.extend(extra.iter().map(|arg| (*arg).to_string()));
        let mut command = world.cmd(&args.iter().map(String::as_str).collect::<Vec<_>>());
        // Launched from the directory both references are written against, which
        // is what makes them relative to anything at all.
        command.current_dir(&world.root);
        world
            .run_on(command, "start with a declared drafting graph")
            .settled();
        world.run_json(run, "launch.json")["pr_author_graph"]
            .as_str()
            .unwrap_or_else(|| panic!("{run} recorded no drafting graph"))
            .to_string()
    };

    // Each run records the *resolved* reference: relative in the document,
    // absolute in the record, against the directory the launch was made from.
    assert_eq!(
        std::fs::canonicalize(drafted("declared", &[])).expect("the recorded graph resolves"),
        std::fs::canonicalize(&declared).expect("the declared graph is there"),
        "the launch config's own graph did not reach the run"
    );
    assert_eq!(
        std::fs::canonicalize(drafted(
            "overridden",
            &["--pr-author-graph", "graphs/pr-author-override.yaml"]
        ))
        .expect("the recorded graph resolves"),
        std::fs::canonicalize(&overriding).expect("the overriding graph is there"),
        "the flag did not override the config it names"
    );
    // And it is the graph each run *dispatched*, rather than only what each
    // recorded: the second run drafted through the document the flag named.
    let graphs: Vec<PathBuf> = world
        .invocations()
        .iter()
        .filter(|call| {
            call["tool"] == "oneagentgraph"
                && call["args"].as_array().is_some_and(|args| {
                    args.iter()
                        .any(|arg| arg == "onepipeline.persona=pr-author")
                })
        })
        .filter_map(|call| call["args"][1].as_str())
        .map(|graph| std::fs::canonicalize(graph).expect("the dispatched graph resolves"))
        .collect();
    assert_eq!(
        graphs,
        vec![
            std::fs::canonicalize(&declared).expect("the declared graph is there"),
            std::fs::canonicalize(&overriding).expect("the overriding graph is there"),
        ],
        "the drafting dispatches did not run the graphs their launches decided"
    );
}

/// The drafted body is read out of the copy **this run kept**, never out of the
/// path the producer named.
///
/// `report_path` is a stranger's path on a journal line: a reader that opened
/// whatever it named would be an arbitrary-file reader driven by whatever wrote
/// there, and what it read would be published on a change request. So the
/// producer here names a **symlink** wearing the report's own file name, and the
/// file behind it is a valid report carrying a body of its own. Retention
/// refuses it — a report is a plain file the producer wrote — and the change
/// request opens with no body at all: the planted words reach nothing this run
/// wrote, published or recorded.
#[test]
fn a_drafted_body_is_read_only_from_the_copy_this_run_retained() {
    let world = World::new("lifecycle-planted-body");
    world.repository("change-open", &["true"]);
    world.script("service.work", "the worker wrote this\n");
    // Every settlement in this run names one, which costs the transcript its
    // words and must cost the publication its body.
    world.script("report.symlink", "");
    let drafting = world.pr_author_graph();
    let node = titled(lifecycle("service", &[]), "feat: land it with no body");
    let path = world.plan("planted", &plan_of("planted", vec![node]));
    world
        .run(&[
            "start",
            &path.to_string_lossy(),
            "--attach",
            "--pr-author-graph",
            &drafting,
        ])
        .settled();

    let opened = world.changes_opened();
    assert_eq!(opened.len(), 1, "{opened:?}");
    assert_eq!(
        opened[0]["body"], "",
        "the change request carries a body this run never retained: {opened:?}"
    );
    // And nowhere else either: not in the merged store, and not in the run's own
    // record of what it published.
    for artifact in ["events.jsonl", "result.json"] {
        let written = std::fs::read_to_string(world.run_file("planted", artifact))
            .unwrap_or_else(|error| panic!("cannot read {artifact}: {error}"));
        assert!(
            !written.contains("planted-and-never-read"),
            "{artifact} carries what only the producer-named path held"
        );
    }
}

/// A plan written at an **earlier** version still runs, and its untitled
/// lifecycle node publishes under the subject `onevcs` derives.
///
/// Every version this build reads below the one it writes, because "an earlier
/// plan still runs" is a promise to every plan already written on a host and a
/// journey that drove only the newest of them would not hold it. A node written
/// there states no title — the field is required from schema 3 on — so this
/// crate passes none, and the subject the change lands under is the sibling's
/// own reading of the branch's conventional commits rather than anything
/// composed here.
#[test]
fn an_earlier_plan_still_publishes_under_the_subject_the_sibling_derives() {
    for version in [1, 2] {
        let world = World::new(&format!("lifecycle-v{version}"));
        let repo = published_locally(&world);
        world.script("service.work", "the worker wrote this\n");
        let run = format!("v{version}");
        // Untitled on purpose: that is the whole shape an earlier version has,
        // and it is what a plan on a host was written as.
        let mut node = lifecycle("service", &[]);
        node.as_object_mut().expect("a node").remove("title");
        let mut plan = plan_of(&run, vec![node]);
        plan["schema_version"] = json!(version);
        let path = world.plan(&run, &plan);
        world
            .run(&["start", &path.to_string_lossy(), "--attach"])
            .settled();

        let result = world.run_json(&run, "result.json");
        assert_eq!(
            result["state"],
            "complete",
            "a version {version} plan no longer runs: {result}\n{}",
            why(&world, &run)
        );
        let landed = repo.base_commits(&world);
        assert!(
            landed.len() > 1,
            "the workstream published nothing: {landed:?}\n{}",
            why(&world, &run)
        );
        // Nothing this crate composed: the `chore: <node id>` fallback it used
        // to publish under is gone, and the subject the base landed under is the
        // one the sibling derived from the branch this node published — read off
        // the node's own settlement rather than written down here.
        let branch = result["nodes"][0]["branch"]
            .as_str()
            .unwrap_or_else(|| panic!("the node recorded no branch: {result}"));
        assert!(
            !landed.iter().any(|subject| subject.contains("service")),
            "this crate composed a subject for a node that stated none: {landed:?}"
        );
        assert_eq!(
            landed.first().map(String::as_str),
            Some(derived_subject(branch).as_str()),
            "the base did not land under the subject the sibling derives: {landed:?}"
        );
    }
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

/// A drafting graph the launch directory cannot produce is refused before a run
/// exists.
///
/// The reference is external input and it is *relative*, so this crate owns the
/// base it resolves against and is the only thing that can say it does not
/// resolve. Refused at launch rather than at the first publication: a run minted
/// against a graph nothing can read would dispatch every node, do the work, and
/// discover at the change request that the drafting it was launched for was
/// never going to happen — so the refusal is worth nothing unless no run was
/// minted, which is the second half of what this asserts.
#[test]
fn a_drafting_graph_the_launch_directory_cannot_produce_is_refused_before_a_run_starts() {
    let world = World::new("lifecycle-nodrafting");
    world.repository("local-direct", &["true"]);
    let path = world.plan(
        "nodrafting",
        &plan_of("nodrafting", vec![lifecycle("service", &[])]),
    );
    world
        .run(&[
            "start",
            &path.to_string_lossy(),
            "--attach",
            "--pr-author-graph",
            "graphs/no-such-pr-author.yaml",
        ])
        .exited(2)
        // Naming the reference as it was given and the directory it was resolved
        // against: a launch refused for a relative path says which path and
        // relative to what, or the operator cannot tell a typo from a launch run
        // from the wrong directory.
        .err_has("cannot read graph 'graphs/no-such-pr-author.yaml'")
        .err_has("resolved against launch directory");
    assert!(!world.run_file("nodrafting", "launch.json").exists());
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

/// A publication its gate rejected, with a drafting dispatch that also failed.
///
/// Both endings in one journey because the settlement carries both, in one
/// order: the publication's own reason is what settled the node, and the
/// drafting ending follows it because it is true either way. A reader looking
/// for the drafter must not have to know which of the two failed first.
#[test]
fn a_publication_that_its_gate_rejects_settles_the_node_failed_by_name() {
    let world = World::new("lifecycle-gate");
    world.repository("local-direct", &["false"]);
    world.script("service.work", "the worker wrote this\n");
    world.script("service.pr-author.fail", "1");
    let drafting = world.pr_author_graph();
    let path = world.plan(
        "rejected",
        &plan_of("rejected", vec![lifecycle("service", &[])]),
    );
    world
        .run(&[
            "start",
            &path.to_string_lossy(),
            "--attach",
            "--pr-author-graph",
            &drafting,
        ])
        .settled();
    let run = "rejected".to_string();

    let result = world.run_json(&run, "result.json");
    assert_eq!(
        result["nodes"][0]["status"],
        "failed",
        "{result}\n{}",
        why(&world, &run)
    );
    assert_eq!(result["nodes"][0]["outcome"], "publication-failed");
    let reported = world.run(&["results", &run]);
    reported.exited(0).out_has("publication-failed");

    // The sibling's own reason leads, and the drafting ending follows it.
    let settled = world.events_of(&run, "node-settled");
    let detail = settled[0]["payload"]["detail"]
        .as_str()
        .expect("the settlement says why");
    let publication = detail
        .find("onevcs:")
        .expect("the sibling's own reason is what settled the node");
    let drafted = detail
        .find("the change request's body was not drafted")
        .unwrap_or_else(|| panic!("the drafting ending is missing from: {detail}"));
    assert!(
        publication < drafted,
        "the drafting ending displaced the reason the node failed: {detail}"
    );
    assert!(
        reported.stdout.contains("was not drafted"),
        "`results` did not show the drafting ending:\n{}",
        reported.stdout
    );

    // And it is reported on its own kind as well, under the node it happened
    // to: a publication that failed does not swallow the drafting failure.
    let undrafted = world.events_of(&run, "body-not-drafted");
    assert_eq!(undrafted.len(), 1, "{undrafted:?}");
    assert_eq!(undrafted[0]["payload"]["ending"], "dispatch-failed");
    assert_eq!(undrafted[0]["labels"]["node"], "service");

    // The terminal half. The gate is the repository's own verification of the
    // tree as it stands, and nothing this crate can do from here changes what it
    // will say — so the node settles on the residual word and is **not** asked
    // again. A second dispatch here would spend a whole workstream reproducing a
    // verdict that is already in hand.
    assert_eq!(
        dispatches_of(&world, &run, "service").len(),
        1,
        "a failure no further attempt can answer was retried anyway\n{}",
        why(&world, &run)
    );
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

/// A re-dispatch is another `node-dispatched`, so counting these in order is how
/// a journey tells one attempt from several.
fn dispatches_of(world: &World, run: &str, node: &str) -> Vec<serde_json::Value> {
    world
        .events_of(run, "node-dispatched")
        .into_iter()
        .filter(|event| event["labels"]["node"] == node)
        .collect()
}

/// The task prose each of one node's dispatches was given, in order.
///
/// Read off the `turn-activity` the dispatch's own worker emitted, which echoes
/// the prose it received — so this is what the agent was actually handed rather
/// than what this crate believes it composed.
fn tasks_dispatched_to(world: &World, run: &str, node: &str) -> Vec<String> {
    world
        .journal(run)
        .iter()
        .filter(|event| event["labels"]["node"] == node && event["kind"] == "turn-activity")
        .filter_map(|event| event["payload"]["task"].as_str().map(str::to_string))
        .collect()
}

/// A required check the host reports as red, which is what CI failing looks like
/// to a change request the host was asked to land.
const RED: &str = "llmlint completed failure required";

/// The journey the whole change exists for: the host's checks reject a change
/// the node published, and the node goes back to work on the branch it left
/// behind instead of failing with nobody to fix it.
///
/// This is the incident. A change request opened, auto-merge was armed, the node
/// settled, and then a required check failed in CI — with no node left to fail,
/// nothing reported back, and a person eventually noticing a blocked pull
/// request. What has to happen instead is all of it: the failure gets its own
/// word, the node is dispatched again **on the branch that carries the rejected
/// tree**, and the diagnosis travels with it.
///
/// The run is detached so the world can move while it is going, which is what
/// makes this a recovery rather than a loop: the host reports the check red, and
/// once it has, this test makes it green the way a re-run of CI would. The flip
/// happens while the *first* publication is still failing over it — `onevcs`
/// gives up on the reading it has already made — so the attempt that follows
/// meets a host with a different answer.
#[test]
fn a_publication_its_checks_reject_is_redispatched_on_the_branch_it_preserved() {
    let world = World::new("lifecycle-checksfailed");
    // `change-auto` is the policy that watches the host's own checks to their
    // conclusion, which is where a red one is observed at all.
    let repo = world.repository("change-auto", &["true"]);
    world.script("service.work", "the worker wrote this\n");
    world.script("gh.checks", RED);

    let path = world.plan(
        "checksfailed",
        &plan_of("checksfailed", vec![lifecycle("service", &[])]),
    );
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    let run = "checksfailed".to_string();

    // The host has reported the check red, which is the reading the publication
    // is failing on. Everything after this is a world that has moved on.
    world.until("the host to report its check red", |world| {
        world
            .events_of(&run, "change-check")
            .iter()
            .any(|event| event["payload"]["conclusion"] == "failure")
    });
    // CI ran again and this time nothing blocks the merge, so the host lands what
    // it was handed. A publication already in flight does not see this: it has
    // its answer.
    std::fs::remove_file(world.fakes.join("gh.checks")).expect("the red check is cleared");
    world.script("gh.merged", "");

    world.until("the run to settle", |world| {
        world.run_file(&run, "result.json").is_file()
    });
    let result = world.run_json(&run, "result.json");
    let node = result["nodes"][0].clone();
    // It recovered: a later attempt published and the host landed it. A node that
    // had settled on the first failure could never have reached this.
    assert_eq!(node["status"], "done", "{result}\n{}", why(&world, &run));
    assert_eq!(node["outcome"], "merged", "{result}");
    assert_eq!(node["landing"], "landed", "{result}");

    // It was dispatched again, and the second dispatch says why it happened: a
    // reader counting them sees the recovery without a kind of its own to learn.
    //
    // A floor rather than an exact count, deliberately. What flips the host is
    // this test rather than the run, so how many attempts were made before the
    // run observed the change is the machine's business — an exact count here
    // would be an assertion about scheduling. That the budget is a *bound* is
    // `a_node_that_spends_its_publication_budget_settles_naming_every_attempt`,
    // where nothing flips and the count is exact.
    let dispatched = dispatches_of(&world, &run, "service");
    assert!(
        dispatched.len() >= 2,
        "the node was never dispatched again: {dispatched:?}\n{}",
        why(&world, &run)
    );
    let again = &dispatched[1];
    assert_eq!(again["payload"]["attempt"], 2, "{again}");
    assert_eq!(again["payload"]["attempts"], 3, "{again}");
    let reason = again["payload"]["reason"]
        .as_str()
        .expect("the re-dispatch says what the last attempt ended with");
    assert!(
        reason.starts_with("checks-failed:"),
        "the re-dispatch does not name the failure it answers: {reason}"
    );
    assert!(
        reason.contains("llmlint"),
        "the re-dispatch does not name the check that failed: {reason}"
    );

    // The diagnosis reached the worker, in the prose it was dispatched with.
    let tasks = tasks_dispatched_to(&world, &run, "service");
    assert!(tasks.len() >= 2, "{tasks:?}");
    assert!(
        !tasks[0].contains("## Planner context"),
        "the first attempt was told about a failure that had not happened: {}",
        tasks[0]
    );
    let second = &tasks[1];
    for said in [
        "## Planner context",
        "checks-failed",
        "llmlint",
        "onevcs artifact cat",
    ] {
        assert!(
            second.contains(said),
            "the re-dispatch was not told {said:?}:\n{second}"
        );
    }
    // And the evidence it points at is an artifact this publication really
    // recorded, rather than an id composed here.
    let recorded: Vec<String> = world
        .journal(&run)
        .iter()
        .filter(|event| event["source"] == "vcs")
        .filter_map(|event| event["artifacts"].as_array().cloned())
        .flatten()
        .filter_map(|artifact| artifact["id"].as_str().map(str::to_string))
        .collect();
    assert!(
        recorded.iter().any(|id| second.contains(id)),
        "the re-dispatch names no artifact the publication recorded; it recorded \
         {recorded:?}:\n{second}"
    );

    // One branch, continued. Every session this node opened worked on it, so the
    // attempt that recovered met the tree the host had rejected rather than a
    // fresh one cut beside it.
    let branch = node["branch"].as_str().expect("the node names its branch");
    let branches: Vec<String> = world
        .journal(&run)
        .iter()
        .filter(|event| event["source"] == "vcs" && event["kind"] == "session-opened")
        .filter(|event| event["payload"]["clone"].is_string())
        .filter_map(|event| event["payload"]["branch"].as_str().map(str::to_string))
        .collect();
    assert!(
        branches.len() >= 2 && branches.iter().all(|opened| opened == branch),
        "the re-dispatch cut a second branch beside the committed work: {branches:?}"
    );
    // And **one** change request, adopted rather than opened again: the branch
    // already had one, which is what the host answers when it is asked.
    let opened = world.changes_opened();
    assert_eq!(
        opened.len(),
        1,
        "the re-dispatch opened a second change request for one branch: {opened:?}"
    );
    assert_eq!(opened[0]["head"], json!(branch), "{:?}", opened[0]);
    // The branch is still in the checkout the failed attempt handed it back to,
    // which is where the attempt that recovered found it.
    assert!(
        repo.has_branch(&world, branch),
        "the branch the attempts shared was not handed back to the checkout"
    );
}

/// A publishing push the merge path refuses is the second preserving failure, and
/// it is preserving for a different reason: nothing was published at all.
///
/// The `pre-receive` hook below is the repository's own — a real one, on the real
/// bare origin, refusing the branch the way a hook that finds a secret or a
/// forbidden path does. `onevcs` reports it as `push-rejected`, the branch is
/// handed back because it is the only record of the work, and the node is
/// dispatched again on it carrying what the remote wrote.
///
/// Two attempts rather than three, because what this proves is the routing and
/// the evidence rather than the size of the budget.
#[test]
fn a_push_the_merge_path_refuses_is_redispatched_carrying_what_the_remote_wrote() {
    let world =
        World::new("lifecycle-pushrejected").with_env("ONEPIPELINE_PUBLICATION_ATTEMPTS", "2");
    let repo = world.repository("change-auto", &["true"]);
    world.script("service.work", "the worker wrote this\n");
    refuse_pushed_branches(&world, &repo);

    let run = settle(&world, "pushrejected", vec![lifecycle("service", &[])]);
    let result = world.run_json(&run, "result.json");
    let node = result["nodes"][0].clone();
    assert_eq!(node["status"], "failed", "{result}\n{}", why(&world, &run));
    assert_eq!(node["outcome"], "push-rejected", "{result}");
    // Nothing was published, so there is nothing to have landed and no change
    // request to point anybody at.
    assert_eq!(node["landing"], json!(null), "{result}");
    assert!(node["change_url"].is_null(), "{result}");

    let dispatched = dispatches_of(&world, &run, "service");
    assert_eq!(
        dispatched.len(),
        2,
        "the node was not dispatched again on its preserved branch\n{}",
        why(&world, &run)
    );
    assert!(
        dispatched[1]["payload"]["reason"]
            .as_str()
            .is_some_and(|reason| reason.starts_with("push-rejected:")),
        "{}",
        dispatched[1]
    );

    // What the remote wrote is the whole diagnosis of a refused push, and it
    // travels as the artifact the `push` record carried rather than inline.
    let pushes = world.events_of(&run, "push");
    let artifacts: Vec<String> = pushes
        .iter()
        .filter_map(|event| event["artifacts"].as_array().cloned())
        .flatten()
        .filter_map(|artifact| artifact["id"].as_str().map(str::to_string))
        .collect();
    assert!(
        !artifacts.is_empty(),
        "the refused push recorded nothing to diagnose it with: {pushes:?}"
    );
    let second = &tasks_dispatched_to(&world, &run, "service")[1];
    assert!(
        artifacts.iter().any(|id| second.contains(id)),
        "the re-dispatch names none of the push's evidence ({artifacts:?}):\n{second}"
    );
    // Git's own per-ref summary leads, because that is what `onevcs` reports and
    // what tells a worker the push never landed; the hook's own words are in the
    // artifact above, where a whole run of a repository's verification belongs.
    assert!(
        second.contains("pre-receive hook declined"),
        "the re-dispatch was not told what git said about the ref:\n{second}"
    );

    // And the work survived the refusal: the branch is in the checkout, which is
    // the only place it exists once the session's clone is gone.
    let branch = node["branch"].as_str().expect("the node names its branch");
    assert!(
        repo.has_branch(&world, branch),
        "the branch a refused push left behind was not handed back"
    );
}

/// A run cancelled between a preserving failure and the attempt that would
/// answer it settles on the failure, not on the cancellation.
///
/// The window is held open rather than raced for: the repository's gate blocks
/// until this test releases it, so the cancel lands while the publication is
/// still running and everything after the release — the push, its refusal, the
/// preserving classification — happens unconditionally. Dispatched again into a
/// run whose teardown is on its way to reap it, the node would settle as the
/// cancellation and lose the publication failure, which is the useful half of
/// what happened.
#[test]
fn a_cancel_that_lands_before_the_next_attempt_settles_on_the_publication_failure() {
    let world =
        World::new("lifecycle-cancelledretry").with_env("ONEPIPELINE_PUBLICATION_ATTEMPTS", "2");
    let go = world.fakes.join("gate.go");
    let held = gate_script(&world, &["wait-for", &go.to_string_lossy()]);
    let repo = world.repository(
        "change-auto",
        &held.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    world.script("service.work", "the worker wrote this\n");
    refuse_pushed_branches(&world, &repo);

    let path = world.plan(
        "cancelledretry",
        &plan_of("cancelledretry", vec![lifecycle("service", &[])]),
    );
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    let run = "cancelledretry".to_string();

    world.until("the publication to reach its gate", |world| {
        world
            .journal(&run)
            .iter()
            .any(|event| event["source"] == "vcs" && event["kind"] == "gate-started")
    });
    world
        .run_with_stdin(
            &["reply", &run],
            &json!({"version": 1, "commands": [{"op": "cancel", "id": "service"}]}).to_string(),
        )
        .exited(0);
    world.until("the cancel to be committed", |world| {
        !world.events_of(&run, "edit-committed").is_empty()
    });
    std::fs::write(&go, "go").expect("the gate is released");

    world.until("the run to settle", |world| {
        world.run_file(&run, "result.json").is_file()
    });
    // The settlement the loop reached is the publication's, under its own word:
    // that is what an operator has to act on, and it is the half a node
    // dispatched again into a cancelled run would have lost.
    let settled = &world.events_of(&run, "node-settled")[0]["payload"];
    assert_eq!(
        settled["status"],
        "failed",
        "{settled}\n{}",
        why(&world, &run)
    );
    assert_eq!(settled["outcome"], "push-rejected", "{settled}");
    let detail = settled["detail"]
        .as_str()
        .expect("the settlement says why")
        .to_string();
    assert!(
        detail.contains("1 publication attempt on"),
        "the settlement does not name the one attempt that was made: {detail}"
    );
    // And no second attempt, though the budget had one left.
    assert_eq!(
        dispatches_of(&world, &run, "service").len(),
        1,
        "a cancelled run was dispatched again\n{}",
        why(&world, &run)
    );

    // The run's own document reports it `parked`, because that is what the
    // cancel made it and a parked node is one a planner can requeue. The failure
    // is not lost to that: the outcome beside it is the publication's.
    let node = world.run_json(&run, "result.json")["nodes"][0].clone();
    assert_eq!(node["status"], "parked", "{node}\n{}", why(&world, &run));
    assert_eq!(node["outcome"], "push-rejected", "{node}");
    // And the work is on the branch the one attempt was made on, which is what
    // a requeue would continue.
    let branch = node["branch"].as_str().expect("the node names its branch");
    assert!(
        repo.has_branch(&world, branch),
        "the branch the cancelled attempt was made on was not handed back"
    );
}

/// Make the bare origin refuse every branch a publication pushes.
///
/// A real `pre-receive` hook, installed after the seed push so the base branch is
/// already there: what a repository's own hook does on the merge path is the
/// thing `push-rejected` is about, and scripting the refusal anywhere else would
/// be this suite deciding what git said.
fn refuse_pushed_branches(world: &World, repo: &Repository) {
    let hook = repo.origin.join("hooks").join("pre-receive");
    std::fs::create_dir_all(hook.parent().expect("hooks has a directory"))
        .expect("the hooks directory");
    std::fs::write(
        &hook,
        "#!/bin/sh\necho \"this origin does not take onevcs branches\" >&2\nexit 1\n",
    )
    .expect("the hook is written");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))
            .expect("the hook is executable");
    }
    let _ = world;
}

/// A `pre-push` gate, whose verdict is the publishing push's own output.
///
/// `git clone` copies no hooks, so `onevcs` carries the checkout's configured
/// `core.hooksPath` onto the session's clone — which is where the publishing push
/// is made from and therefore the only place a merge-path hook runs. The
/// directory sits outside the working tree so the hook is never a file the
/// session has to have committed.
fn gate_on_a_refusing_pre_push_hook(world: &World, repo: &Repository) {
    std::fs::write(
        world.onevcs_home().join("rules.yml"),
        "version: 2\nrules: []\ndefault:\n  publication: change-auto\n  approvals: none\n  \
         gate:\n    kind: pre-push\n",
    )
    .expect("the rules file is written");
    let hooks = world.root.join("merge-path-hooks");
    std::fs::create_dir_all(&hooks).expect("the hooks directory");
    let hook = hooks.join("pre-push");
    std::fs::write(
        &hook,
        "#!/bin/sh\necho \"the merge-path gate says no\" >&2\nexit 1\n",
    )
    .expect("the hook is written");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))
            .expect("the hook is executable");
    }
    crate::harness::git(
        world,
        &repo.checkout,
        &["config", "core.hooksPath", &hooks.to_string_lossy()],
    );
}

/// One stored artifact two records point at is one piece of evidence.
///
/// A `pre-push` gate runs no command of its own: its verdict *is* what the
/// publishing push wrote, so `onevcs` stores that output once and references it
/// from both the `push` record and the `gate-verdict` beside it. A diagnosis
/// composed straight off the stream would hand the worker the same
/// `onevcs artifact cat` twice, which reads as two runs of a gate that ran once
/// — and sends somebody looking for the difference between them.
///
/// Unix-only for the hook bit, which is where a `pre-push` gate lives at all.
#[cfg(unix)]
#[test]
fn one_artifact_two_records_point_at_reaches_the_worker_once() {
    let world =
        World::new("lifecycle-evidencetwice").with_env("ONEPIPELINE_PUBLICATION_ATTEMPTS", "2");
    let repo = world.repository("change-auto", &["true"]);
    gate_on_a_refusing_pre_push_hook(&world, &repo);
    world.script("service.work", "the worker wrote this\n");

    let run = settle(&world, "evidencetwice", vec![lifecycle("service", &[])]);
    let node = world.run_json(&run, "result.json")["nodes"][0].clone();
    assert_eq!(
        node["outcome"],
        "push-rejected",
        "{node}\n{}",
        why(&world, &run)
    );

    // The fixture really did produce the shape this is about: one artifact id,
    // on two of the publication's own records.
    let mut records: Vec<(String, String)> = Vec::new();
    for event in world.journal(&run) {
        if event["source"] != "vcs" {
            continue;
        }
        let kind = event["kind"].as_str().unwrap_or_default().to_owned();
        for artifact in event["artifacts"].as_array().into_iter().flatten() {
            if let Some(id) = artifact["id"].as_str() {
                records.push((kind.clone(), id.to_owned()));
            }
        }
    }
    let shared = records
        .iter()
        .find(|(_, id)| records.iter().filter(|(_, other)| other == id).count() > 1)
        .map(|(_, id)| id.clone())
        .unwrap_or_else(|| {
            panic!(
                "no artifact was recorded twice, so there is nothing to deduplicate: {records:?}"
            )
        });

    // And the worker was told to read it once.
    let tasks = tasks_dispatched_to(&world, &run, "service");
    assert!(
        tasks.len() >= 2,
        "the node was not dispatched again: {tasks:?}"
    );
    let second = &tasks[1];
    assert_eq!(
        second.matches(shared.as_str()).count(),
        1,
        "the re-dispatch names {shared} more than once:\n{second}"
    );
    assert!(
        second.contains("onevcs artifact cat"),
        "the re-dispatch does not say how to read its evidence:\n{second}"
    );
}

/// A base that moves under a publication is the third preserving failure, and it
/// is the one whose continuation currently cannot get started.
///
/// The conflict is real and it is made the way one happens: the base takes a
/// change to the same file while the node's worker is still working, and the
/// publication's bounded resolve-and-requeue cannot merge the two. `onevcs`
/// reports `sync-conflict`, hands the branch back, and this crate dispatches the
/// node again on it — which is what this journey is here to pin.
///
/// What that second dispatch then meets is pinned too, and deliberately: opening
/// a session on a branch that conflicts with its integration target is a refusal
/// `onevcs` makes at session open, so the continuation never reaches a worker and
/// the node settles `infrastructure-failure` carrying the sibling's own sentence.
/// That is the behaviour today rather than the behaviour anybody wants, and a
/// journey that asserted a happier ending would be describing a stack this one is
/// not. When the sibling learns to open a session into a conflict for a worker to
/// resolve, this is the test that says so by failing.
#[test]
fn a_base_that_moved_under_a_publication_is_redispatched_on_the_branch_it_preserved() {
    let world = World::new("lifecycle-syncconflict")
        .with_env("ONEPIPELINE_PUBLICATION_ATTEMPTS", "2")
        // The continuation's session refuses to open, which is a dispatch that
        // produced nothing — and the *dispatch* boundary re-asks one of those
        // three times over. That retry is not what this journey is about, and
        // three of it would make the count below say nothing about the loop that
        // is.
        .with_env("ONEPIPELINE_BOUNDARY_ATTEMPTS", "1");
    let repo = world.repository("local-direct", &["true"]);
    // The worker holds until this test releases it, which is the window the base
    // moves in. What it writes is the file the base is about to take a different
    // version of.
    world.script("service.work", "the worker wrote this\n");
    world.script("service.wait", "");

    let path = world.plan(
        "syncconflict",
        &plan_of("syncconflict", vec![lifecycle("service", &[])]),
    );
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    let run = "syncconflict".to_string();

    // The **session** rather than the dispatch: the branch is cut when the
    // session opens, and a base that moved before that is a base the branch was
    // cut from — which is no conflict at all, and a journey that waited on the
    // dispatch would prove that instead about half the time. This crate's own
    // `session-opened` and not the sibling's, because the sibling's arrives on
    // the session stream this run only starts following once the dispatch has
    // drained — and the dispatch is what is being held here.
    world.until("the node's session to cut its branch", |world| {
        !world.events_of(&run, "session-opened").is_empty()
    });
    // Somebody else lands a change to the same file, while the node's worker is
    // still holding. The publication that follows syncs the base into the branch
    // before it verifies anything, and these two versions of one file do not
    // merge.
    let work = repo.checkout.join("service.md");
    std::fs::write(&work, "somebody else wrote this instead\n").expect("the base change");
    crate::harness::git(&world, &repo.checkout, &["add", "-A"]);
    crate::harness::git(
        &world,
        &repo.checkout,
        &["commit", "-m", "feat: take the file another way"],
    );
    crate::harness::git(&world, &repo.checkout, &["push", "origin", "main"]);
    world.release("service.go");

    world.until("the run to settle", |world| {
        world.run_file(&run, "result.json").is_file()
    });
    let result = world.run_json(&run, "result.json");
    let node = result["nodes"][0].clone();
    assert_eq!(node["status"], "failed", "{result}\n{}", why(&world, &run));

    // The routing, which is what this crate owns: the conflict was named, and
    // the node was asked again on the branch that carries the work.
    let dispatched = dispatches_of(&world, &run, "service");
    assert_eq!(
        dispatched.len(),
        2,
        "a base that moved settled the node instead of sending it back to the branch\n{}",
        why(&world, &run)
    );
    let reason = dispatched[1]["payload"]["reason"]
        .as_str()
        .expect("the re-dispatch says what the last attempt ended with");
    assert!(
        reason.starts_with("sync-conflict:"),
        "the re-dispatch does not name the failure it answers: {reason}"
    );
    // The branch is in the checkout, which is the whole reason the failure is
    // one a further attempt could answer at all.
    let branch = node["branch"].as_str().expect("the node names its branch");
    assert!(
        repo.has_branch(&world, branch),
        "the branch the conflict was on was not handed back"
    );
    // And the sibling recorded the conflict, with the hunks beside it.
    let conflicts = world.events_of(&run, "sync-conflict");
    assert!(
        !conflicts.is_empty(),
        "the conflict reached no record a reader can find it in\n{}",
        why(&world, &run)
    );
}

/// A node that publishes into a host whose required check stays red.
///
/// Every change request this host is handed and not just the first: the check is
/// red and stays red, which is the loop the budget exists to bound.
fn publishing_into_checks_that_stay_red(world: &World, name: &str) -> (String, Repository) {
    let repo = world.repository("change-auto", &["true"]);
    world.script("service.work", "the worker wrote this\n");
    world.script("gh.checks", RED);
    let run = settle(world, name, vec![lifecycle("service", &[])]);
    (run, repo)
}

/// A check that is never going to pass is a worse failure than the one it
/// replaces, so the loop is bounded and says so when it stops.
///
/// The budget is set to `0` here, which is not a budget: read literally it would
/// settle this node having never dispatched it at all. An unusable value falls
/// back to the default instead — the same direction every other bound this crate
/// reads falls in, because a knob that *disabled* the recovery it configures is
/// the one setting nobody means. So three attempts is what a `0` gets, and three
/// is what this journey counts.
#[test]
fn a_node_that_spends_its_publication_budget_settles_naming_every_attempt() {
    let world = World::new("lifecycle-budget").with_env("ONEPIPELINE_PUBLICATION_ATTEMPTS", "0");
    let (run, repo) = publishing_into_checks_that_stay_red(&world, "budget");
    let result = world.run_json(&run, "result.json");
    let node = result["nodes"][0].clone();
    assert_eq!(node["status"], "failed", "{result}\n{}", why(&world, &run));
    assert_eq!(node["outcome"], "checks-failed", "{result}");
    assert_eq!(
        dispatches_of(&world, &run, "service").len(),
        3,
        "the node was not dispatched exactly three times\n{}",
        why(&world, &run)
    );

    let detail = world.events_of(&run, "node-settled")[0]["payload"]["detail"]
        .as_str()
        .expect("the settlement says why")
        .to_string();
    let branch = node["branch"].as_str().expect("the node names its branch");
    for said in [
        "3 publication attempts",
        "1 checks-failed, 2 checks-failed, 3 checks-failed",
        branch,
        "llmlint",
    ] {
        assert!(
            detail.contains(said),
            "the spent budget does not say {said:?}: {detail}"
        );
    }
    // `results` is where an operator reads it, so the word reaches them without
    // opening the store.
    world
        .run(&["results", &run])
        .exited(0)
        .out_has("checks-failed");
    // And the work is still there to pick up by hand: the branch a person
    // retries from is the one every attempt was made on.
    assert!(
        repo.has_branch(&world, branch),
        "the branch {branch} every attempt was made on was not handed back"
    );
}

/// The other way an operator gets that bound wrong: a value that is not a number
/// at all.
///
/// `0` above is a number this crate refuses and this is a parse that never
/// produces one, so they reach the fallback down two different paths — and an
/// operator who wrote `three` where a digit goes must not silently get a
/// different run from one who wrote `0`. Its own journey rather than a second
/// case inside the one above, because what is held is that the whole run is the
/// same run.
#[test]
fn a_publication_budget_that_is_not_a_number_spends_the_same_default() {
    let world =
        World::new("lifecycle-budgetword").with_env("ONEPIPELINE_PUBLICATION_ATTEMPTS", "three");
    let (run, _repo) = publishing_into_checks_that_stay_red(&world, "budgetword");

    let node = world.run_json(&run, "result.json")["nodes"][0].clone();
    assert_eq!(node["status"], "failed", "{node}\n{}", why(&world, &run));
    assert_eq!(node["outcome"], "checks-failed", "{node}");
    assert_eq!(
        dispatches_of(&world, &run, "service").len(),
        3,
        "a budget that is not a number was not the default three\n{}",
        why(&world, &run)
    );
    let detail = world.events_of(&run, "node-settled")[0]["payload"]["detail"]
        .as_str()
        .expect("the settlement says why")
        .to_string();
    assert!(
        detail.contains("3 publication attempts"),
        "the spent budget does not name the attempts it made: {detail}"
    );
}

/// A node that lost its body and then its publication settles saying both, in
/// that order.
///
/// The two endings are unrelated — drafting runs after the branch is verified
/// and off the publication path — so a node can have both, and the settlement
/// that stops the retry loop is the one place a reader meets them together. Its
/// detail composes the drafting ending **after** the roll-up of every attempt,
/// so a planner reads the failure standing in the way first and the missing body
/// as the aside it is; a settlement that dropped either half would send that
/// reader to fix one thing when there are two.
#[test]
fn a_node_that_spends_its_budget_undrafted_settles_saying_both_endings() {
    let world = World::new("lifecycle-budget-undrafted");
    world.repository("change-auto", &["true"]);
    world.script("service.work", "the worker wrote this\n");
    world.script("gh.checks", RED);
    // The drafter answers inside its schema with nothing in it, which is the
    // ending that needs no second double to arrange.
    world.script("pr-author.bodyless", "1");
    let drafting = world.pr_author_graph();
    let path = world.plan(
        "budgetdraft",
        &plan_of("budgetdraft", vec![lifecycle("service", &[])]),
    );
    world
        .run(&[
            "start",
            &path.to_string_lossy(),
            "--attach",
            "--pr-author-graph",
            &drafting,
        ])
        .settled();
    world.until("the run to settle", |world| {
        world.run_file("budgetdraft", "result.json").is_file()
    });

    let node = world.run_json("budgetdraft", "result.json")["nodes"][0].clone();
    assert_eq!(
        node["status"],
        "failed",
        "{node}\n{}",
        why(&world, "budgetdraft")
    );
    // The publication's ending and not the drafting one: the check is what a
    // person has to answer, and the body is what they will also want to write.
    assert_eq!(node["outcome"], "checks-failed", "{node}");
    let detail = world.events_of("budgetdraft", "node-settled")[0]["payload"]["detail"]
        .as_str()
        .expect("the settlement says why")
        .to_string();
    let undrafted = "succeeded and there was no body in what it answered with";
    let roll_up = "3 publication attempts";
    for said in [
        roll_up,
        "1 checks-failed, 2 checks-failed, 3 checks-failed",
        undrafted,
    ] {
        assert!(
            detail.contains(said),
            "the spent budget of an undrafted node does not say {said:?}: {detail}"
        );
    }
    assert!(
        detail.find(roll_up) < detail.find(undrafted),
        "the drafting ending did not come after the attempts it is an aside to: {detail}"
    );
    // And `results` is where an operator meets the pair without opening the
    // store, which is the only place either word does them any good.
    world
        .run(&["results", "budgetdraft"])
        .exited(0)
        .out_has("checks-failed");
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
    // llmlint: ignore-block[tests_mirror_real_usage] the argv is the *host* boundary here,
    // not an internal seam: `gh pr create --body` is the whole of what GitHub is told the
    // description is, and this suite's host is `gh` at `onevcs`'s own override. There is no
    // reply-side surface to read it back from — `onevcs::Publication` carries the URL and no
    // body — so what a reviewer would open is observable only as what the host was sent.
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
    // llmlint: ignore-end[tests_mirror_real_usage]

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
    // And it says what it compared against, which `no-changes` alone does not:
    // the same word covers a worker that wrote nothing and a branch measured
    // against itself, and only the ref tells them apart.
    let results = world.run(&["results", &run]);
    results
        .exited(0)
        .out_has("no-changes")
        .out_has("compared against main");
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

/// A `change-auto` change the host never lands is the second silent failure this
/// change exists to end, and the bound on waiting for it is the one that ends it.
///
/// `change-auto` asks the host to land the change once its checks pass, and
/// `onevcs` watches until it does. A host that never does is a change nobody is
/// going to merge, and the watch stopping on its own bound used to be exactly the
/// place a run went quiet: the node settled on a snapshot and nothing looked
/// again. It settles `checks-unsettled` instead — its own word, naming the bound
/// and what the host had said — and the branch is preserved, so the node is
/// re-dispatched on it like any other preserving failure.
///
/// The budget is set to one here, so what this journey is about is the **ending**
/// rather than the loop — and setting it proves the bound is read from where it
/// is documented to be read.
#[test]
fn a_change_auto_publication_the_host_never_lands_settles_checks_unsettled() {
    // Spelled as the operator spells it, which is how every other bound this
    // suite moves is spelled: the name is the surface, and `src/engine.rs`'s own
    // test holds it to the contract that publishes it.
    let world = World::new("lifecycle-unsettled").with_env("ONEPIPELINE_PUBLICATION_ATTEMPTS", "1");
    let repo = world.repository("change-auto", &["true"]);
    world.script("service.work", "the worker wrote this\n");
    // No `gh.merged`: this host takes the change and never lands it.
    let run = settle(&world, "unsettled", vec![lifecycle("service", &[])]);

    let node = world.run_json(&run, "result.json")["nodes"][0].clone();
    assert_eq!(node["status"], "failed", "{node}\n{}", why(&world, &run));
    assert_eq!(node["outcome"], "checks-unsettled", "{node}");
    // It claims no landing: the change did not reach the base and nothing here
    // observed it doing so.
    assert_eq!(node["landing"], json!(null), "{node}");

    // One attempt, because that is the budget this run was given — the bound is
    // a number something reads, not a constant nothing can move.
    assert_eq!(
        dispatches_of(&world, &run, "service").len(),
        1,
        "a budget of one was not honoured\n{}",
        why(&world, &run)
    );
    let detail = world.events_of(&run, "node-settled")[0]["payload"]["detail"]
        .as_str()
        .expect("the settlement says why")
        .to_string();
    for said in ["1 publication attempt on", "1 checks-unsettled"] {
        assert!(
            detail.contains(said),
            "the settlement does not say {said:?}: {detail}"
        );
    }
    // And the work is where a person picks it up: on the branch the attempt was
    // made on, which the change request they have to decide about points at.
    let branch = node["branch"].as_str().expect("the node names its branch");
    assert!(
        repo.has_branch(&world, branch),
        "the branch the unlanded change is on was not handed back"
    );
}

/// And the same ending is preserving, which is only visible where the budget
/// leaves room for the attempt that proves it.
///
/// The journey above states the ending under a budget of one, so nothing there
/// distinguishes `checks-unsettled` from a failure that settles where it stands.
/// This one gives it two: the host still never lands anything, so the second
/// attempt meets the same bound — and that it *ran at all*, on the branch the
/// first attempt handed back, is the whole of what "preserving" means here.
#[test]
fn a_change_the_host_never_lands_is_redispatched_on_the_branch_it_preserved() {
    let world =
        World::new("lifecycle-unsettledagain").with_env("ONEPIPELINE_PUBLICATION_ATTEMPTS", "2");
    let repo = world.repository("change-auto", &["true"]);
    world.script("service.work", "the worker wrote this\n");
    // No `gh.merged`, on either attempt: this host takes the change and holds it.
    let run = settle(&world, "unsettledagain", vec![lifecycle("service", &[])]);

    let node = world.run_json(&run, "result.json")["nodes"][0].clone();
    assert_eq!(node["status"], "failed", "{node}\n{}", why(&world, &run));
    assert_eq!(node["outcome"], "checks-unsettled", "{node}");

    let dispatched = dispatches_of(&world, &run, "service");
    assert_eq!(
        dispatched.len(),
        2,
        "an unsettled change was not re-dispatched on its preserved branch\n{}",
        why(&world, &run)
    );
    assert!(
        dispatched[1]["payload"]["reason"]
            .as_str()
            .is_some_and(|reason| reason.starts_with("checks-unsettled:")),
        "the re-dispatch does not name the failure it answers: {}",
        dispatched[1]
    );

    // One branch, continued: the second attempt met the change the host was
    // already holding rather than a fresh one cut beside it.
    let branch = node["branch"].as_str().expect("the node names its branch");
    let branches: Vec<String> = world
        .journal(&run)
        .iter()
        .filter(|event| event["source"] == "vcs" && event["kind"] == "session-opened")
        .filter(|event| event["payload"]["clone"].is_string())
        .filter_map(|event| event["payload"]["branch"].as_str().map(str::to_string))
        .collect();
    assert!(
        branches.iter().all(|opened| opened == branch),
        "the attempts did not share one branch: {branches:?}"
    );
    assert!(
        repo.has_branch(&world, branch),
        "the branch both attempts were made on was not handed back"
    );
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

    // A change request left open for somebody to decide about, which is the
    // publication that settles `done` with its change unlanded: `change-open` is
    // the one policy that does not watch the host, because a person does.
    let world = World::new("lifecycle-result-contract");
    world.repository("change-open", &["true"]);
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

/// A settled node and a landed node are different facts, and a node settles
/// `done` either way.
///
/// Both halves publish and both settle `done` — publishing is the whole of what
/// the plan asked of them — and only one of them put anything on `main`. What
/// tells them apart is **what the host did**, read off the publication's own
/// answer and never off the policy that asked for it.
///
/// The two policies are the two shapes a change request can be handed over in.
/// `change-open` leaves it for a person, so the node settles with it unlanded and
/// nothing waits: a change request somebody owns is not something a run may block
/// or poll on. `change-auto` asks the host to land it, and *is* watched to its
/// end — so the second half settles only once the host has actually merged, which
/// is the observation `landed` rests on.
///
/// Everything a planner reads is checked, because closing work on a settled node
/// is a decision made from any of them: the ledger record, the round result the
/// read API serves, and every view that renders a node's status.
#[test]
fn a_settled_node_and_a_landed_node_are_told_apart_by_what_the_host_did_not_by_the_policy() {
    let world = World::new("lifecycle-landing");
    // The repository asks the host to land its changes; the first node below
    // narrows that to `change-open` for itself, which is a person's decision and
    // nothing this run waits on.
    world.repository("change-auto", &["true"]);

    // The change request is open and nobody has merged it.
    world.script("service.work", "the change nobody merged\n");
    // Named for the scenario and not for the answer: a run id is printed on every
    // view line, so `heldopen` cannot satisfy an assertion looking for the word
    // this journey is about.
    let held = {
        let mut node = lifecycle("service", &[]);
        node["merge_policy"] = json!("change-open");
        node
    };
    let open = settle(&world, "heldopen", vec![held]);

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
        "title": "feat: land the workstream",
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
    // `change-auto` is watched to its end, so this host lands what it is handed
    // rather than leaving the publication waiting out its bound: what this
    // journey is about is the pins reaching the session, not what CI said.
    world.script("gh.merged", "");
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
        "title": "feat: land the workstream",
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

/// A dispatch that opened a change request and *then* failed its own verdict
/// settles as its own outcome, carrying the change.
///
/// The engine's publication step deliberately does not run for a step that did
/// not settle `done`, so nothing this crate did opened this change: the worker
/// ran `onevcs publish` in its own final turn, which is the incident. Reported
/// as a plain `task-failed`, it sent a planner to re-run work that had already
/// merged and released — the two call for opposite actions, so they are two
/// outcomes.
#[test]
fn a_dispatch_that_failed_after_opening_a_change_settles_carrying_that_change() {
    let world = World::new("lifecycle-failed-open");
    // A policy that opens a change request and leaves it open, which is what the
    // worker's own publication reaches.
    world.repository("change-open", &["true"]);
    world.script("service.work", "the work its judge would not pass\n");
    world.script(
        "service.publishes",
        "chore: the change the dispatch opened itself",
    );
    world.script("service.fail", "1");
    let run = settle(&world, "failedopen", vec![lifecycle("service", &[])]);

    // The worker really reached that sibling: this is the real `onevcs`, over
    // real git, opening a real change request through the host stand-in.
    assert!(
        world.was_invoked("onevcs", &["publish"]),
        "the dispatch never published its own branch: {:?}",
        world.invocations()
    );
    let opened = world
        .journal(&run)
        .into_iter()
        .find(|event| event["kind"] == "change-opened")
        .unwrap_or_else(|| panic!("no change request was opened\n{}", why(&world, &run)));
    let url = opened["payload"]["url"]
        .as_str()
        .expect("the change request names where it is read")
        .to_owned();

    let settled = world
        .events_of(&run, "node-settled")
        .into_iter()
        .find(|event| event["labels"]["node"] == "service")
        .unwrap_or_else(|| panic!("the node never settled\n{}", why(&world, &run)));
    assert_eq!(settled["payload"]["status"], "failed", "{settled}");
    assert_eq!(
        settled["payload"]["outcome"], "task-failed-change-open",
        "a failure with an open change request settled as a plain task failure: {settled}"
    );
    assert_eq!(
        settled["payload"]["change_url"], url,
        "the settlement does not carry the change a reviewer opens: {settled}"
    );

    // And every view a planner decides from carries it, so "review the change"
    // is reachable without reading the merged stream by hand.
    let node = world.run_json(&run, "result.json")["nodes"][0].clone();
    assert_eq!(node["status"], "failed", "{node}");
    assert_eq!(node["change_url"], url, "{node}");
    world
        .run(&["results", &run])
        .exited(0)
        .out_has("task-failed-change-open")
        .out_has(&url);
}

/// The same failure with nothing published settles exactly as it always did.
///
/// The pair is the point: an outcome that is *distinct* is only distinct if the
/// ordinary case still reads the way it did, and a lookup that answered the same
/// way for both would have qualified every failure in the run.
#[test]
fn a_dispatch_that_failed_with_nothing_published_settles_as_a_plain_task_failure() {
    let world = World::new("lifecycle-failed-plain");
    world.repository("change-open", &["true"]);
    world.script("service.work", "the work its judge would not pass\n");
    world.script("service.fail", "1");
    let run = settle(&world, "failedplain", vec![lifecycle("service", &[])]);

    assert!(
        !world.was_invoked("onevcs", &["publish"]),
        "this journey's worker published something: {:?}",
        world.invocations()
    );
    let settled = world
        .events_of(&run, "node-settled")
        .into_iter()
        .find(|event| event["labels"]["node"] == "service")
        .unwrap_or_else(|| panic!("the node never settled\n{}", why(&world, &run)));
    assert_eq!(settled["payload"]["status"], "failed", "{settled}");
    assert_eq!(
        settled["payload"]["outcome"], "task-failed",
        "a failure with nothing published was qualified anyway: {settled}"
    );
    assert!(
        settled["payload"]["change_url"].is_null(),
        "a failure with nothing published carries a change request: {settled}"
    );
}

/// A change that has merged since the node settled is not reported as work
/// nobody landed.
///
/// The settlement is an observation of a moment: the host was holding this
/// change when the node settled, and the run neither blocks nor polls for a
/// merge somebody else owns. Hours later that snapshot was still being rendered
/// as the state of things now, and `just runs` reported a change that had merged
/// and released as one that had reached nobody.
///
/// So every line that carries the count says *when* it was true and points at
/// the change. What it does not say is that the change is still open, because
/// nothing here has looked: a change request lives on the repository's host,
/// `onevcs` owns every route to one, and the read that would answer this is not
/// on that library's surface — the proposal is recorded in
/// `docs/contract-divergences.md`.
#[test]
fn a_change_that_merged_after_settlement_is_reported_as_of_settlement_not_as_now() {
    let world = World::new("lifecycle-landing-stale");
    // A change request left for a person to merge, so the node settles with its
    // change unlanded — the state the incident started from.
    let repository = world.repository("change-open", &["true"]);
    world.script("service.work", "the change that merged later\n");
    let run = settle(&world, "mergedlater", vec![lifecycle("service", &[])]);

    let node = world.run_json(&run, "result.json")["nodes"][0].clone();
    assert_eq!(node["landing"], "unlanded", "{node}\n{}", why(&world, &run));
    let branch = node["branch"]
        .as_str()
        .expect("the node names the branch its work is on")
        .to_owned();

    // And then the world moves on: the change reaches the base, and nothing
    // tells the settled run about it. This is the state every assertion below
    // is about.
    crate::harness::git(&world, &repository.checkout, &["fetch", "origin"]);
    crate::harness::git(
        &world,
        &repository.checkout,
        &["merge", "--no-ff", "-m", "chore: land the change", &branch],
    );
    crate::harness::git(&world, &repository.checkout, &["push", "origin", "main"]);
    assert!(
        repository
            .base_commits(&world)
            .iter()
            .any(|subject| subject == "chore: land the change"),
        "the change never reached the base, so there is nothing stale to report"
    );

    // The per-node view: dated, and pointing at the change rather than claiming
    // to know where it is now.
    let results = world.run(&["results", &run]);
    results.exited(0).out_has("NOT landed");
    results.out_has("when this settled");
    results.out_has("nothing has re-read it since");
    results.out_has("open the change for where it is now");

    // The counting views, which are the ones a planner closes work from.
    world
        .run(&["runs"])
        .exited(0)
        .out_has("1 not landed as of settlement");
    world
        .run(&["goals", &run])
        .exited(0)
        .out_has("1 not landed as of settlement");
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("as each settled, not as of now");
}

/// A change request this crate cannot read the record of settles the failure it
/// always did.
///
/// The lookup that names an open change on a failed node reads the session's own
/// stream, and that stream is a file another build of `onevcs` may have written
/// a line of. `EventStream` refuses a whole read over one line it cannot parse —
/// so the change request really is there and the record of it really is
/// unreadable, which is the case the settlement must degrade on. It reports
/// `task-failed`, exactly as it did before there was a lookup at all: an
/// unreadable record is not evidence of a change nobody opened, and a settlement
/// that failed here would fail every node whose stream a newer sibling had
/// touched.
#[test]
fn a_change_this_crate_cannot_read_the_record_of_settles_as_a_plain_task_failure() {
    let world = World::new("lifecycle-failed-unreadable");
    // llmlint: ignore-block[tests_mirror_real_usage] the same extension point the two
    // journeys above use, and the same reason: a repository's gate is a command an
    // operator wrote, `Stream::emit` is the only writer of a stream and it appends whole
    // envelopes, and the stream exists only once the session has opened. The gate runs on
    // the publication path, so the line it appends is on the stream *before* the change
    // request is opened — which is what makes the whole read refuse. Everything asserted
    // is through the binary.
    let gate = gate(&world, &["append-future-event"]);
    world.repository(
        "change-open",
        &gate.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    // llmlint: ignore-end[tests_mirror_real_usage]
    world.script("service.work", "the work its judge would not pass\n");
    world.script("service.publishes", "chore: a change nobody can read about");
    world.script("service.fail", "1");
    let run = settle(&world, "failedunread", vec![lifecycle("service", &[])]);

    // The change request was opened — this is not a journey about a publication
    // that did not happen.
    assert!(
        world.was_invoked("onevcs", &["publish"]),
        "the dispatch never published its own branch: {:?}",
        world.invocations()
    );
    let settled = world
        .events_of(&run, "node-settled")
        .into_iter()
        .find(|event| event["labels"]["node"] == "service")
        .unwrap_or_else(|| panic!("the node never settled\n{}", why(&world, &run)));
    assert_eq!(settled["payload"]["status"], "failed", "{settled}");
    assert_eq!(
        settled["payload"]["outcome"], "task-failed",
        "a settlement that could not read the record claimed one anyway: {settled}"
    );
    assert!(
        settled["payload"]["change_url"].is_null(),
        "a settlement carries a change request it could not read: {settled}"
    );
}

/// A node that writes its own branch as its integration target is refused, and
/// told the spelling that continues one.
///
/// `base_branch` equal to `branch` is the only way a plan can *look* like it is
/// asking to continue an existing branch, and it never was one: the node would be
/// asked at publication what its branch adds to itself. `onevcs` 0.8.0 stops it
/// where a planner can act on it — before the dispatch is spent, rather than in a
/// `no-changes` that reads exactly like a node whose worker wrote nothing — and
/// names `branch` on its own as the spelling that does continue a branch. What
/// [`onepipeline::plan::Node::base_branch`] documents is held here: the run fails
/// rather than reporting on work it never integrated, and the reason reaches the
/// settlement.
#[test]
fn a_node_whose_base_branch_is_its_branch_is_refused_and_told_what_continues_a_branch() {
    let world = World::new("lifecycle-selfbase");
    let repo = world.repository("change-direct", &["true"]);
    // The preserved branch a planner is trying to continue: it carries work the
    // repository's integration target does not, which is the whole reason to
    // point a node at it.
    let kept = world.root.join("kept");
    crate::harness::git(
        &world,
        &repo.checkout,
        &[
            "worktree",
            "add",
            "-b",
            KEPT,
            &kept.to_string_lossy(),
            "main",
        ],
    );
    std::fs::write(kept.join("service.md"), "the work already done\n").expect("the kept work");
    crate::harness::git(&world, &kept, &["add", "-A"]);
    crate::harness::git(
        &world,
        &kept,
        &["commit", "-m", "feat: what the last run did"],
    );
    crate::harness::git(&world, &repo.checkout, &["push", "origin", KEPT]);
    crate::harness::git(
        &world,
        &repo.checkout,
        &["worktree", "remove", &kept.to_string_lossy()],
    );

    let mut node = lifecycle("service", &[]);
    node["branch"] = json!(KEPT);
    node["base_branch"] = json!(KEPT);
    let run = settle(&world, "selfbase", vec![node]);

    let settled = world.run_json(&run, "result.json")["nodes"][0].clone();
    assert_eq!(
        settled["status"],
        "failed",
        "a node pinned with `base_branch` equal to its `branch` is no longer \
         refused; `Node::base_branch` documents that it is, and the field's \
         documentation is what needs to change with it: {settled}\n{}",
        why(&world, &run)
    );
    assert_eq!(
        settled["outcome"],
        "infrastructure-failure",
        "the refusal reached the settlement as something other than a dispatch \
         that never began: {settled}\n{}",
        why(&world, &run)
    );
    // And the branch was never merged anywhere: the work the node was pointed at
    // is still only on that branch, which is what a planner is being told to go
    // and re-pin rather than being handed a report about.
    assert_eq!(
        repo.base_file("service.md"),
        None,
        "the preserved branch reached the repository's base, so this journey is no \
         longer about work that went nowhere"
    );

    // The reason, in the run's own record and in what an operator reads — it names
    // the branch and the spelling that continues one, so the fix is in the message
    // rather than in folklore about which fields may be equal.
    let detail = world.events_of(&run, "node-settled")[0]["payload"]["detail"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    for claim in [KEPT, "is also this session's base", "on its own"] {
        assert!(
            detail.contains(claim),
            "the settlement does not say '{claim}', so the refusal it carries is not \
             the one this journey is about: {detail}"
        );
    }
    world
        .run(&["results", &run])
        .exited(0)
        .out_has("infrastructure-failure")
        .out_has("is also this session's base");
}
