//! The lever that binds a node's judge, driven through the real reply path.
//!
//! `amend` exists because `context` does not do this. A note steers the worker
//! for one dispatch and says of itself that it adds no acceptance criteria; a
//! manager who ruled mid-dispatch had nothing that reached the **judge**, and
//! that node's own judge overturned the ruling from a task that never mentioned
//! it. So the journeys here are about the one thing an accepted edit cannot tell
//! you on its own: what the dispatch was actually handed, on the dispatch after
//! the amendment and on every later one.
//!
//! What a node's dispatch is handed is one `--task`, and `oneagentgraph` gives
//! that one task to the worker and to the judge supervising it alike — which is
//! what `dispatch.rs`'s
//! `every_dag_scope_member_is_given_the_runs_description_and_its_own_job` proves
//! against the real sibling. So the task each dispatch recorded is where these
//! journeys read the bar from.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. What each dispatch was handed is read off the launch the double
// actually received, never off anything this crate wrote down about it. `harness.rs`
// carries the same suppression and the full rationale.

// llmlint: ignore-file[tests_mirror_real_usage] every assertion below about *what a
// dispatch was asked to do* reads the `--task` of the `oneagentgraph run` this crate
// really issued, through `tasks_dispatched`. That argv is the product boundary the whole
// feature crosses — a dispatch **is** that command line, and the one `--task` on it is
// what the worker and the judge supervising it are both handed — so it is the thing under
// test rather than an internal of the crate under test, and it is the same evidence
// `World::was_invoked` gives every other journey here. No view renders it: `monitor` caps
// a line at 96 characters, `transcript` needs a member that has settled, and neither can
// answer what a **held** dispatch was asked to do, which is what the running-node journey
// turns on. Everything else in this file — the verdicts, the refusals, the amendment a
// manager reads before replacing it — is asserted through the CLI.

use crate::harness::{agent, plan_of, World, REFUSED};
use onepipeline::plan::{AMENDMENT_HEADING, AMENDMENT_PRECEDENCE};
use serde_json::{json, Value};

/// The ruling a manager issues mid-dispatch, and the correction that replaces it.
const RULING: &str = "The four redundant comment lines are out of scope for this node: leave them.";
const CORRECTION: &str = "Restore the four comment lines after all; the reviewer asked for them.";

fn envelope(commands: Value) -> String {
    json!({"version": 2, "commands": commands}).to_string()
}

/// The task prose each of a node's dispatches was given, in dispatch order.
///
/// Read off the `--task` the sibling's own launch really carried. That argv is
/// the **product boundary** this crate composes across — a dispatch *is*
/// `oneagentgraph run GRAPH --task T`, and that one value is what the worker and
/// the judge supervising it are both handed — so it is the thing under test
/// rather than an internal of the crate under test. No view renders a
/// dispatch's prose: `monitor` caps a line at 96 characters and `transcript`
/// renders a settled member's conversation, so neither can answer what a
/// **held** dispatch was asked to do, which is what the running-node journey
/// below turns on.
fn tasks_dispatched(world: &World, node: &str) -> Vec<String> {
    let mine = format!("## What\nDo {node}.");
    world
        .invocations()
        .into_iter()
        .filter(|call| call["tool"] == "oneagentgraph")
        .filter_map(|call| {
            let args = call["args"].as_array()?.clone();
            let at = args.iter().position(|arg| arg == "--task")?;
            args.get(at + 1)?.as_str().map(str::to_owned)
        })
        .filter(|task| task.starts_with(&mine))
        .collect()
}

/// Start a run with a node held open, so an edit arrives while work is in
/// flight, beside a node that has not been dispatched at all.
fn held_beside_a_pending_node(world: &World, name: &str, extra: Vec<Value>) -> String {
    world.script("slow.turn-open", "");
    world.script("slow.wait", "hold");
    let mut nodes = vec![agent("slow", &[]), agent("later", &["slow"])];
    nodes.extend(extra);
    let path = world.plan(name, &plan_of(name, nodes));
    world.run(&["start", &path, "--detach"]).exited(0);
    // Waited out on the view a supervisor watches a run through, rather than on
    // the store behind it: `status` reports a node that is running and how long
    // it has been.
    world.until("the held node to be running", |world| {
        world
            .run(&["status", name])
            .stdout
            .contains("slow: running")
    });
    name.to_string()
}

/// The journey the op exists for: a ruling reaches the task the node is judged
/// against, it is rendered where a ruling has to be, and a later ruling replaces
/// it rather than sitting beside it.
#[test]
fn an_amendment_binds_every_later_dispatch_and_a_second_one_replaces_it() {
    let world = World::new("amend-binds");
    // `later` has not been dispatched, so the amendment lands before its first
    // dispatch; `keep` stays in flight throughout, so the loop is still running
    // when the requeue below asks for a second dispatch of `later`.
    world.script("keep.wait", "hold");
    let run = held_beside_a_pending_node(&world, "amendbinds", vec![agent("keep", &[])]);
    world.script("later.wait", "hold");

    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "amend", "id": "later", "text": RULING}])),
        )
        .exited(0)
        .out_has("\"applied\"");
    world.until("the amendment to be committed", |world| {
        world
            .events_of(&run, "edit-committed")
            .iter()
            .any(|event| event["payload"]["command"]["op"] == "amend")
    });

    // A manager about to replace it can read what they are replacing, from
    // either view — each a fresh process that re-folds the run's journal from
    // nothing, which is what "replay reconstructs the amended task" means where
    // a reader stands.
    for verb in ["status", "results"] {
        world
            .run(&[verb, &run])
            .exited(0)
            .out_has(RULING)
            .out_has("amend");
    }

    // The dependency settles, so the amended node dispatches — carrying the
    // ruling, in the one task its worker and its judge are both handed.
    world.release("slow.go");
    world.until("the amended node to be dispatched", |world| {
        world
            .events_of(&run, "node-dispatched")
            .iter()
            .any(|event| event["labels"]["node"] == "later")
    });

    // Parked and brought back, so the node is dispatched a second time: what a
    // note would have been consumed by, and what an amendment must survive.
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "cancel", "id": "later"}])),
        )
        .exited(0);
    world.release("later.go");
    world.until("the parked node to settle", |world| {
        world
            .events_of(&run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "later")
    });
    let first = tasks_dispatched(&world, "later");
    assert!(
        first[0].contains(RULING),
        "the ruling did not reach the node's dispatch: {}",
        first[0]
    );
    assert!(
        first[0].contains(AMENDMENT_HEADING) && first[0].contains(AMENDMENT_PRECEDENCE),
        "the ruling reached the task without its authority: {}",
        first[0]
    );
    // And it is not disclaiming itself the way a carried note does — which is
    // the whole difference between the two levers.
    assert!(
        !first[0].contains("adds no acceptance criteria"),
        "the ruling was rendered as an observation: {}",
        first[0]
    );

    // The correction goes out before the node runs again, so what the second
    // dispatch is judged against is the ruling that replaced the first.
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "amend", "id": "later", "text": CORRECTION}])),
        )
        .exited(0)
        .out_has("\"applied\"");
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "requeue", "id": "later"}])),
        )
        .exited(0);
    world.until("the requeued node to be dispatched again", |world| {
        tasks_dispatched(world, "later").len() >= 2
    });

    let both = tasks_dispatched(&world, "later");
    assert!(
        both[1].contains(CORRECTION),
        "the replacing ruling did not reach the later dispatch: {}",
        both[1]
    );
    assert!(
        !both[1].contains(RULING),
        "the replaced ruling is still binding the judge beside its own correction: {}",
        both[1]
    );
    // And the view a manager reads says the same thing the dispatch was given.
    world
        .run(&["results", &run])
        .exited(0)
        .out_has(CORRECTION)
        .out_lacks(RULING);
    world.release("keep.go");
}

/// A ruling issued while the node is **running** binds its next dispatch, and
/// leaves the turn already in flight alone.
///
/// This is the asymmetry between the two levers, and the reason the pair exists.
/// A `context` note aimed at a running node is pushed into that turn through the
/// agent's control socket — it steers the worker that is working *now* and is
/// gone afterwards. An amendment cannot reach that turn: its task was composed
/// before the ruling existed and the judge that reviews it reads that same task.
/// So an amendment does the other thing, and does it permanently — every
/// dispatch of the node from here on is measured against it.
#[test]
fn a_ruling_issued_while_the_node_runs_binds_its_next_dispatch_and_not_its_turn() {
    let world = World::new("amend-running");
    // The amended node is the one being held, so the ruling arrives while its
    // dispatch is genuinely in flight and its turn is one an interrupt *could*
    // have reached.
    world.script("slow.turn-open", "");
    let run = held_beside_a_pending_node(&world, "amendrunning", Vec::new());

    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "amend", "id": "slow", "text": RULING}])),
        )
        .exited(0)
        .out_has("\"applied\"");
    world.until("the amendment to be committed", |world| {
        world
            .events_of(&run, "edit-committed")
            .iter()
            .any(|event| event["payload"]["command"]["op"] == "amend")
    });

    // The turn already in flight was not touched: no lever was pulled at it,
    // and the task it is working from is the one it was dispatched with.
    assert!(
        world.events_of(&run, "turn-interrupted").is_empty(),
        "an amendment reached for the running turn's control socket: {:?}",
        world.events_of(&run, "turn-interrupted")
    );
    let held = tasks_dispatched(&world, "slow");
    assert_eq!(held.len(), 1, "{held:?}");
    assert!(
        !held[0].contains(RULING),
        "the running dispatch's own task changed under it: {}",
        held[0]
    );

    // And the bar it is now measured against is readable, so a manager watching
    // the dispatch it just ruled on can see the ruling land.
    world.run(&["status", &run]).exited(0).out_has(RULING);

    // The node that has not run yet takes its own ruling, before the dependency
    // that is holding it settles.
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "amend", "id": "later", "text": CORRECTION}])),
        )
        .exited(0);

    // The held dispatch settles, so the amended dependent runs — carrying the
    // ruling, in the one task its worker and its judge are both handed.
    world.release("slow.go");
    world.until("the dependent node to be dispatched", |world| {
        !tasks_dispatched(world, "later").is_empty()
    });
    let next = tasks_dispatched(&world, "later");
    assert!(
        next[0].contains(CORRECTION),
        "the ruling did not reach the dispatch that followed it: {}",
        next[0]
    );

    // And the node that has now settled done takes neither lever: a `retry`
    // refuses it, and so does a second amendment — the same fact from two
    // sides, that what an amendment binds is a dispatch still to come.
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "retry", "id": "slow", "node": {
                "id": "slow-2",
                "persona": "engineer",
                "task": "## What\nDo slow.\n\n## Why\nSo the run can settle.\n\n\
                         ## Acceptance criteria\n- slow is done.",
            }}])),
        )
        .exited(REFUSED)
        .err_has("not running, failed, or cancelled");
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "amend", "id": "slow", "text": CORRECTION}])),
        )
        .exited(REFUSED)
        .err_has("settled done");
}

/// The three ways an amendment reaches nobody, each refused by the one it was —
/// and the graph left exactly as it was.
#[test]
fn an_amendment_nothing_will_read_is_refused_by_the_reason_it_was() {
    let world = World::new("amend-refused");
    let run = held_beside_a_pending_node(&world, "amendrefused", Vec::new());

    // A node the graph does not hold.
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "amend", "id": "nowhere", "text": RULING}])),
        )
        .exited(REFUSED)
        .err_has("no node 'nowhere'");

    // Blank text: a bar nobody can clear is refused rather than recorded as one.
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "amend", "id": "later", "text": "   \n"}])),
        )
        .exited(REFUSED)
        .err_has("cannot be blank");

    // A node that has settled done, which is the one nothing will ever read.
    world.release("slow.go");
    world.until("the first node to settle done", |world| {
        world
            .events_of(&run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "slow" && event["payload"]["status"] == "done")
    });
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "amend", "id": "slow", "text": RULING}])),
        )
        .exited(REFUSED)
        .err_has("settled done");

    // Nothing was committed by any of the three, so no node is quietly carrying
    // a bar the reply said it refused.
    assert!(
        world
            .events_of(&run, "edit-committed")
            .iter()
            .all(|event| event["payload"]["command"]["op"] != "amend"),
        "a refused amendment reached the graph: {:?}",
        world.events_of(&run, "edit-committed")
    );
    world.run(&["results", &run]).exited(0).out_lacks(RULING);
}

/// A plan that states an amendment on a node is a plan whose dispatch is
/// measured against it — and so is every step of a lifecycle node that carries
/// one.
///
/// The field is on the node rather than only on the op, the way `context` is, so
/// a planner who already knows the ruling writes it once. And the ruling belongs
/// to the **node**: every step of one workstream shares one branch and one bar,
/// so a step dispatched under an amended node is handed it too.
#[test]
fn a_plan_may_state_an_amendment_and_every_step_of_an_amended_node_is_handed_it() {
    let world = World::new("amend-in-plan");
    world.repository("local-direct", &[]);
    // The first step writes, so the workstream publishes rather than failing on
    // a branch its steps left level with the base.
    world.script("service.implement.work", "the engineer wrote this\n");

    let node = json!({
        "id": "service",
        "repo": "service",
        "title": "feat: land the workstream",
        "amendment": RULING,
        "steps": [
            {"id": "implement", "persona": "engineer",
             "task": "## What\nimplement\n\n## Additional info\n\nrun the gate.\n"},
            {"id": "review", "persona": "reviewer", "task": "## What\nreview",
             "deps": ["implement"]},
        ],
    });
    let path = world.plan("amendplan", &plan_of("amendplan", vec![node]));
    world.run(&["start", &path, "--attach"]).exited(0).settled();

    // Every step's own dispatch, read off the `--task` the sibling's launch
    // carried. A step that never saw the ruling is a step judged against a bar
    // its node does not have.
    let dispatched: Vec<String> = world
        .invocations()
        .into_iter()
        .filter(|call| call["tool"] == "oneagentgraph")
        .filter_map(|call| {
            let args = call["args"].as_array()?.clone();
            let at = args.iter().position(|arg| arg == "--task")?;
            args.get(at + 1)?.as_str().map(str::to_owned)
        })
        .filter(|task| {
            task.starts_with("## What\nimplement") || task.starts_with("## What\nreview")
        })
        .collect();
    assert_eq!(dispatched.len(), 2, "{dispatched:?}");
    for task in &dispatched {
        assert!(
            task.contains(RULING) && task.contains(AMENDMENT_PRECEDENCE),
            "a step of an amended node was dispatched without its bar: {task}"
        );
    }
    // And where the step states operational notes, the ruling sits above them.
    let implement = dispatched
        .iter()
        .find(|task| task.starts_with("## What\nimplement"))
        .expect("the implementing step ran");
    assert!(
        implement.find(AMENDMENT_HEADING) < implement.find("## Additional info"),
        "{implement}"
    );

    // The view a manager reads says the same thing the dispatches were given.
    world
        .run(&["results", "amendplan"])
        .exited(0)
        .out_has(RULING);
}

/// However a manager spells an amendment, the dispatch is handed it as clauses
/// of the one acceptance section: every bullet marker and numbered-item spelling
/// is one clause, a paragraph wrapped over lines is one clause, and a heading is
/// kept as a label, since a heading inside the section would end it.
#[test]
fn every_spelling_of_an_amendment_is_handed_over_as_clauses_of_the_criteria() {
    let world = World::new("amend-spellings");
    let mut node = agent("shape", &[]);
    node["task"] = json!(
        "## What\nDo shape.\n\n## Acceptance criteria\n\n- shape is done.\n\n\
         ## Additional info\n\nRun the gate once.\n"
    );
    node["amendment"] = json!(
        "## Scope\n\n- dash\n* star\n+ plus\n1. dotted\n2) parenthesised\n\n\
         A ruling wrapped\nover two lines.\n"
    );
    let path = world.plan("amendspell", &plan_of("amendspell", vec![node]));
    world.run(&["start", &path, "--attach"]).exited(0).settled();

    let dispatched = tasks_dispatched(&world, "shape");
    assert_eq!(dispatched.len(), 1, "{dispatched:?}");
    assert_eq!(
        dispatched[0],
        format!(
            "## What\nDo shape.\n\n## Acceptance criteria\n\n- shape is done.\n\n\
             {AMENDMENT_HEADING}\n{AMENDMENT_PRECEDENCE}\n\n\
             **Scope**\n\n- dash\n- star\n- plus\n- dotted\n- parenthesised\n\
             - A ruling wrapped over two lines.\n\n\
             ## Additional info\n\nRun the gate once."
        ),
        "the amendment was not handed over as clauses of the one criteria section"
    );
}

// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] what this journey proves is
// what the real `oneagentgraph` and `onejudge` hand a node's judge from the task this crate
// renders, so its edge is this crate's rendering plus those two linked engines; the one
// separately edged test project's input begins at `{workspaceRoot}/src/**/*`, so a project of
// its own would declare the same dependency and skip nothing. It is one supervised turn, and it
// sits beside the other amend journeys, which is where a reader looks for one.
/// The verdict regression: a node whose original criteria carry a file allowlist
/// and whose binding amendment relaxes it is judged against **one** acceptance
/// section, in which the relaxed rule is stated as a criterion taking precedence
/// over the whole task — and the judge is never handed the amendment as a peer
/// section beside the allowlist it relaxes.
///
/// The failure this closes: rendered as its own `## Amendment` claiming
/// precedence over the operational notes alone, an amendment admitting a script
/// the original criteria excluded was read by the node's judge beside that
/// criterion, and a gate-green node was failed for touching the script the
/// in-force ruling admitted. So this drives the whole path the judge's reading
/// is composed on — this crate's `start`, the real `oneagentgraph`, and the real
/// `onejudge` composing the supervisor's prompt from the one `--task` both
/// parties are handed — with the harness double below onejudge recording what
/// the judge was actually asked, and reads the section off that prompt.
#[test]
fn a_judge_is_handed_one_acceptance_section_in_which_the_relaxing_amendment_wins() {
    let world = World::new("amend-judge-allowlist");
    world.write_graphs();
    world.write_supervised_node_graph();

    let allowlist = "Only `src/lib.rs` changes; nothing under `scripts/` is touched.";
    let relaxed =
        "`scripts/release-probe.sh` may change as well: the fix needs it, and touching it is \
         within this node's scope.";
    let mut node = agent("build", &[]);
    node["task"] = json!(format!(
        "## What\nDo build.\n\n## Why\nSo the run can settle.\n\n## Acceptance criteria\n\n\
         - build is done.\n- {allowlist}\n\n## Additional info\n\nRun the gate once.\n"
    ));
    node["amendment"] = json!(relaxed);
    let path = world.plan("amendjudge", &plan_of("amendjudge", vec![node]));
    // This crate's own `onepipeline start`, dispatching through the real
    // `oneagentgraph` rather than the double every other journey here puts on
    // its path: the harness below onejudge is the only thing stood in for.
    world
        .run_on_agentgraph(&["start", &path, "--attach"])
        .exited(0)
        .settled();

    let prompts: Vec<String> = world
        .invocations()
        .into_iter()
        .filter(|call| call["tool"] == "oneharness-config")
        .filter_map(|call| call["args"][0].as_str().map(str::to_string))
        .filter(|prompt| prompt.contains(SUPERVISOR_OPENING))
        .collect();
    assert!(
        !prompts.is_empty(),
        "the node's judge was never asked, so nothing here is proven: {:?}",
        world.invocations()
    );
    for whole in &prompts {
        // The task as onejudge states it to the judge. The transcript below it
        // quotes the worker's turn, which opens on the same task, so the section
        // is read off the judge's own statement of it rather than counted twice.
        let prompt = whole
            .split_once(TASK_OPENING)
            .and_then(|(_, rest)| rest.split_once(PERSONA_OPENING))
            .map(|(task, _)| task)
            .unwrap_or_else(|| panic!("the judge was not handed the task:\n{whole}"));
        let at = |needle: &str| {
            prompt
                .find(needle)
                .unwrap_or_else(|| panic!("the judge was not handed {needle:?}:\n{prompt}"))
        };
        // One acceptance section, and the amendment inside it — after the
        // original criteria, above the operational notes — never a `## Amendment`
        // of its own beside them.
        assert_eq!(
            prompt.matches("## Acceptance criteria").count(),
            1,
            "the judge was handed more than one acceptance section:\n{prompt}"
        );
        assert!(
            !prompt.contains("\n## Amendment\n"),
            "the judge was handed the amendment as a section beside the criteria:\n{prompt}"
        );
        assert!(
            at("## Acceptance criteria") < at(&format!("- {allowlist}"))
                && at(&format!("- {allowlist}")) < at(AMENDMENT_HEADING)
                && at(AMENDMENT_HEADING) < at("## Additional info"),
            "the amendment is not inside the criteria and above the notes:\n{prompt}"
        );
        // The relaxed rule is a criterion, under the sentence that says it takes
        // precedence over the whole task — the allowlist above it included.
        assert!(
            prompt.contains(&format!(
                "{AMENDMENT_HEADING}\n{AMENDMENT_PRECEDENCE}\n\n- {relaxed}\n"
            )),
            "the relaxed rule is not the criterion stated under the precedence sentence:\n{prompt}"
        );
        assert!(
            AMENDMENT_PRECEDENCE.contains("takes precedence over the whole task")
                && AMENDMENT_PRECEDENCE.contains("contradicts a criterion above it")
                && AMENDMENT_PRECEDENCE.contains("the contradicted one no longer binds"),
            "the precedence sentence does not resolve the allowlist against the ruling: \
             {AMENDMENT_PRECEDENCE}"
        );
        // And it claims precedence over the task, not over the notes alone.
        assert!(
            !prompt.contains("operational notes below disagree, this section wins"),
            "the amendment still claims precedence over the notes alone:\n{prompt}"
        );
    }
}

/// The brief onejudge opens every supervisor prompt on, which is what tells a
/// judge's prompt from the worker's among everything the harness was handed.
const SUPERVISOR_OPENING: &str = "You are the simulated USER and completion supervisor";

/// The label onejudge states the judged task under, and the one that follows it.
const TASK_OPENING: &str = "Original task:\n";
const PERSONA_OPENING: &str = "Supervisor persona:\n";
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]

/// A plan stating an amendment that says nothing is refused by that field's name,
/// exactly as the op refuses a blank ruling.
///
/// The two are the same input by two routes, and the failure they share is a bar
/// nobody can clear: accepted, it would be left silently out of the very task it
/// was written to change.
#[test]
fn a_plan_amendment_that_says_nothing_is_refused_at_the_plan_boundary() {
    let world = World::new("amend-blank-plan");
    let mut node = agent("build", &[]);
    node["amendment"] = json!("   \n");
    let path = world.plan("amendblank", &plan_of("amendblank", vec![node]));
    world
        .run(&["start", &path, "--detach"])
        .exited(REFUSED)
        .err_has("`amendment`")
        .err_has("says nothing");
}

/// An author absent from the launch's channel configuration may not move a bar.
#[test]
fn an_undeclared_monitor_is_refused_amend_before_it_can_change_the_plan() {
    let world = World::new("amend-monitor");
    let run = held_beside_a_pending_node(&world, "amendmonitor", Vec::new());

    world
        .run_with_stdin(
            &["reply", &run],
            &json!({
                "version": 2,
                "author": "monitor",
                "commands": [{"op": "amend", "id": "later", "text": RULING}],
            })
            .to_string(),
        )
        .exited(REFUSED)
        .err_has("the envelope's author `monitor` is not declared")
        .err_has("the declared authors are: planner");
    world.run(&["results", &run]).exited(0).out_lacks(RULING);
    world.release("slow.go");
}
