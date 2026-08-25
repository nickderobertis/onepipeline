//! The hook that checks a node introduced by a live edit, driven end to end
//! against a real validator command.
//!
//! A plan file is checked before launch by a validator of the consuming host's
//! own. A node introduced by `add` or `retry` was dispatched identically and
//! checked by nothing — so the guard that exists to stop a node being failed for
//! something other than its work was bypassed by exactly the path a manager
//! reaches for under pressure.
//!
//! What runs here is a **real** validator: a compiled program at the seam,
//! reading the node off its stdin and answering with an exit status and its own
//! words, exactly as a host's would. `crates/testfakes/src/bin/node-validator.rs`
//! says why it is not a double.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. The validator is not a substitution either: it is the host's own
// command, and this suite supplies a real one. `harness.rs` carries the same suppression
// and the full rationale.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::harness::{agent, double, plan_of, repo_file, World, REFUSED};

/// What entry 41 of the divergence record proposes, which is where the three
/// spellings of this launch-level setting are written down.
///
/// Read rather than restated: the contract is committed as approved and names
/// none of this, so that entry is the only source — and a journey that spelled
/// the flag itself would go on passing after the record and the code disagreed.
fn proposed() -> Value {
    let record = std::fs::read_to_string(repo_file("docs/contract-divergences.md"))
        .expect("the divergence record ships");
    let entry = record
        .split("\n## ")
        .find(|entry| entry.starts_with("41."))
        .expect("the record still carries entry 41");
    let block = entry
        .split("```json")
        .nth(1)
        .and_then(|rest| rest.split("```").next())
        .expect("entry 41 carries the json block these journeys drive");
    serde_json::from_str::<Value>(block).expect("entry 41's block is JSON")["validator"].clone()
}

/// One spelling out of that block, refused loudly when the entry stops naming
/// it: a journey that fell back to a literal would prove the literal.
fn spelling(named: &str) -> String {
    proposed()[named]
        .as_str()
        .unwrap_or_else(|| panic!("entry 41 no longer names the validator's {named}"))
        .to_string()
}

/// One copy of the real validator, under a name of this journey's choosing.
///
/// Three names for one program is how the precedence journey tells which
/// validator a launch resolved: the program records the name it was invoked as,
/// so the answer comes off the process that actually ran rather than off
/// anything this crate wrote down about which one it picked.
fn validator_named(world: &World, name: &str) -> String {
    let path = world
        .root
        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(double("node-validator"), &path).expect("the validator is placed");
    path.to_string_lossy().into_owned()
}

/// Every node the validator was offered, in order, each with the name the
/// validator was invoked as.
fn offered(world: &World) -> Vec<(String, Value)> {
    let path = world.fakes.join("validator.jsonl");
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let record: Value = serde_json::from_str(line).expect("the validator records JSON");
            (
                record["as"].as_str().expect("it names itself").to_string(),
                record["node"].clone(),
            )
        })
        .collect()
}

fn envelope(commands: Value) -> String {
    json!({"version": 1, "commands": commands}).to_string()
}

/// Start a run whose one node is held open, so the graph is live while edits
/// arrive.
fn live_run(world: &World, name: &str, extra: &[&str]) -> String {
    world.script("slow.wait", "hold");
    let path = world.plan(name, &plan_of(name, vec![agent("slow", &[])]));
    let mut args = vec!["start".to_string(), path.to_string_lossy().into_owned()];
    args.extend(extra.iter().map(|arg| (*arg).to_string()));
    args.push("--detach".to_string());
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    world.run(&borrowed).exited(0);
    world.until("the held node to be dispatched", |world| {
        !world.events_of(name, "node-dispatched").is_empty()
    });
    name.to_string()
}

/// The journey the hook exists for: a node the host's rules refuse never reaches
/// a dispatch, and the manager is told in the host's own words.
#[test]
fn a_node_the_validator_refuses_is_refused_with_its_own_words_and_never_joins_the_graph() {
    let world = World::new("validator-refuses");
    let validator = validator_named(&world, "check-node");
    let run = live_run(
        &world,
        "validatorrefuses",
        &["--node-validator", &validator],
    );

    // The rules this host applies. The refusal a manager reads has to be this
    // sentence: only the host knows which of its rules a node broke.
    let refusal = "acceptance criterion 2 names a procedure — `run just gate` — rather than a \
                   property of the finished tree";
    world.script("validator.refuse", refusal);

    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "add", "node": agent("fresh", &[])}])),
        )
        .exited(REFUSED)
        .err_has(refusal);

    // The node was offered whole, as JSON, so a host's rules read the criteria
    // they are about to judge.
    let seen = offered(&world);
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].1["id"], "fresh");
    assert!(
        seen[0].1["task"]
            .as_str()
            .expect("the task crossed")
            .contains("Acceptance criteria"),
        "{seen:?}"
    );

    // And the graph is unchanged: nothing was committed and nothing dispatched.
    world.run(&["results", &run]).exited(0).out_lacks("fresh");
    assert!(
        world
            .events_of(&run, "edit-committed")
            .iter()
            .all(|event| event["payload"]["command"]["op"] != "add"),
        "a refused node reached the graph: {:?}",
        world.events_of(&run, "edit-committed")
    );

    // With the rules satisfied, the same edit goes through and the node runs.
    std::fs::remove_file(world.fakes.join("validator.refuse")).expect("the rule is lifted");
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "add", "node": agent("fresh", &[])}])),
        )
        .exited(0)
        .out_has("\"applied\"");
    world.until("the accepted node to settle", |world| {
        world
            .events_of(&run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "fresh")
    });
    world.release("slow.go");
}

/// Every op that puts task prose in front of a dispatch is offered to the
/// validator, and no other op is.
///
/// Both directions: an op that reaches no validator is the hole this hook exists
/// to close, and an op it has no opinion about — a `requeue` raising a turn
/// budget — spends a subprocess to be told nothing.
#[test]
fn every_op_that_introduces_or_changes_a_task_is_offered_and_nothing_else_is() {
    let world = World::new("validator-offered");
    let validator = validator_named(&world, "check-node");
    world.script("build.fail", "");
    world.script("slow.wait", "hold");
    // `spare` waits on the held node throughout, so it is a node an edit can
    // still reach: parked and requeued without a dispatch in flight for it.
    let path = world.plan(
        "validatoroffered",
        &plan_of(
            "validatoroffered",
            vec![
                agent("slow", &[]),
                agent("build", &[]),
                agent("spare", &["slow"]),
            ],
        ),
    );
    world
        .run(&[
            "start",
            &path.to_string_lossy(),
            "--node-validator",
            &validator,
            "--detach",
        ])
        .exited(0);
    let run = "validatoroffered".to_string();
    world.until("the node that fails to settle", |world| {
        world
            .events_of(&run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "build")
    });

    for command in [
        json!({"op": "add", "node": agent("fresh", &[])}),
        json!({"op": "retry", "id": "build", "node": agent("build-2", &[])}),
        json!({"op": "amend", "id": "spare", "text": "the ruling"}),
        // Offered: its amendment rewrites the task.
        json!({"op": "cancel", "id": "spare"}),
        json!({"op": "requeue", "id": "spare", "amend": {"task": "## What\nsomething else"}}),
        // Not offered: neither changes what a dispatch is asked to do.
        json!({"op": "cancel", "id": "spare"}),
        json!({"op": "requeue", "id": "spare", "amend": {"max_turns": 9}}),
        json!({"op": "context", "id": "spare", "note": "the fixture moved"}),
    ] {
        world
            .run_with_stdin(&["reply", &run], &envelope(json!([command])))
            .exited(0);
    }

    let seen: Vec<String> = offered(&world)
        .into_iter()
        .map(|(_, node)| node["id"].as_str().expect("a node id").to_string())
        .collect();
    // Each accepted edit is offered **twice**, and deliberately: `compile` is
    // the one validator the submission check and the reconciler both run, which
    // is what makes "applied or rejected with a reason" true — an envelope
    // reaching the loop may have been written by a build or a caller that did
    // not check, and the reconciler is the last place a refusal still means
    // something. A refused edit is offered once, because the submission check
    // turns it away before anything is queued.
    assert!(
        seen.chunks(2)
            .all(|pair| pair.len() == 2 && pair[0] == pair[1]),
        "an edit was not offered to the validator at both the submission check \
         and the reconcile: {seen:?}"
    );
    let each: Vec<&String> = seen.iter().step_by(2).collect();
    assert_eq!(
        each,
        vec!["fresh", "build-2", "spare", "spare"],
        "the validator was offered the wrong edits"
    );
    world.release("slow.go");
}

/// The three names, in the order the record states, proven by driving them
/// rather than by asserting the order in prose.
///
/// Each rung is added on top of the one below it and the answer is read off the
/// program that actually ran, so what is proven is which validator the launch
/// resolved rather than which one this crate believes it picked.
#[test]
fn the_flag_beats_the_environment_which_beats_the_config_and_naming_none_runs_nothing() {
    let precedence: Vec<String> = serde_json::from_value(proposed()["precedence"].clone())
        .expect("entry 41 states the precedence it proposes");
    assert_eq!(
        precedence,
        vec!["flag", "environment", "config_key"],
        "entry 41 proposes a different order than this journey drives"
    );

    let world = World::new("validator-precedence");
    let by_flag = validator_named(&world, "by-flag");
    let by_environment = validator_named(&world, "by-environment");
    let by_config = validator_named(&world, "by-config");
    let config = world.root.join("launch.yaml");
    std::fs::write(
        &config,
        format!(
            "schema_version: {}\n{}: {by_config}\n",
            proposed()["config_schema_version"]
                .as_u64()
                .expect("entry 41 states the version the key arrived at"),
            spelling("config_key"),
        ),
    )
    .expect("the launch config is written");

    // Each rung, from the bottom up: the config alone, then the environment
    // over it, then the flag over both. A fresh run each time, because a
    // validator is resolved once, at the launch.
    for (which, extra, environment) in [
        ("by-config", vec![], None),
        ("by-environment", vec![], Some(by_environment.clone())),
        (
            "by-flag",
            vec![spelling("flag"), by_flag.clone()],
            Some(by_environment.clone()),
        ),
    ] {
        let name = format!("precedence-{which}");
        let path = world.plan(&name, &plan_of(&name, vec![agent("slow", &[])]));
        world.script("slow.wait", "hold");
        let mut args = vec![
            "start".to_string(),
            path.to_string_lossy().into_owned(),
            "--launch-config".to_string(),
            config.to_string_lossy().into_owned(),
        ];
        args.extend(extra);
        args.push("--detach".to_string());
        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut command = world.cmd(&borrowed);
        match &environment {
            Some(value) => command.env(spelling("environment"), value),
            None => command.env_remove(spelling("environment")),
        };
        world.run_on(command, "start").exited(0);
        world.until("the held node to be dispatched", |world| {
            !world.events_of(&name, "node-dispatched").is_empty()
        });

        // A `reply` typed with *no* environment at all is judged by the rules
        // the run was launched under, because the resolved validator is in the
        // launch record rather than re-read here.
        let mut reply = world.cmd(&["reply", &name]);
        reply.env_remove(spelling("environment"));
        world
            .run_with_stdin_on(
                reply,
                &envelope(json!([{"op": "add", "node": agent("fresh", &[])}])),
            )
            .exited(0);
        assert_eq!(
            offered(&world).last().map(|(named, _)| named.clone()),
            Some(which.to_string()),
            "the launch resolved a validator other than the one that should have won"
        );
        world.release("slow.go");
    }

    // And a launch naming none of the three runs no validator at all: the edit
    // is judged exactly as it was before this hook existed.
    let before = offered(&world).len();
    let name = "precedence-none".to_string();
    let path = world.plan(&name, &plan_of(&name, vec![agent("slow", &[])]));
    world.script("slow.wait", "hold");
    let mut command = world.cmd(&["start", &path.to_string_lossy(), "--detach"]);
    command.env_remove(spelling("environment"));
    world.run_on(command, "start").exited(0);
    world.until("the held node to be dispatched", |world| {
        !world.events_of(&name, "node-dispatched").is_empty()
    });
    let mut reply = world.cmd(&["reply", &name]);
    reply.env_remove(spelling("environment"));
    world
        .run_with_stdin_on(
            reply,
            &envelope(json!([{"op": "add", "node": agent("fresh", &[])}])),
        )
        .exited(0);
    assert_eq!(
        offered(&world).len(),
        before,
        "a launch that named no validator ran one"
    );
    world.release("slow.go");
}

/// A validator that refuses without saying anything is still not silent, and one
/// that cannot be started refuses the edit rather than letting the node through.
///
/// The second is the whole reason this fails closed: accepting an edit because
/// the check could not be run would be the crate deciding that an unenforced
/// rule is no rule, silently, on the path a manager reaches for under pressure.
#[test]
fn a_validator_that_says_nothing_and_one_that_cannot_be_started_both_refuse_loudly() {
    let world = World::new("validator-silent");
    let validator = validator_named(&world, "check-node");
    let run = live_run(&world, "validatorsilent", &["--node-validator", &validator]);

    world.script("validator.silent", "");
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "add", "node": agent("fresh", &[])}])),
        )
        .exited(REFUSED)
        .err_has("exited 3");
    world.run(&["results", &run]).exited(0).out_lacks("fresh");

    // A launch whose validator is not there at all. A separate run, because a
    // validator is resolved once, at the launch.
    let missing: PathBuf = world.root.join("no-such-validator");
    assert!(!Path::new(&missing).exists());
    let path = world.plan(
        "validatormissing",
        &plan_of("validatormissing", vec![agent("slow", &[])]),
    );
    world
        .run(&[
            "start",
            &path.to_string_lossy(),
            "--node-validator",
            &missing.to_string_lossy(),
            "--detach",
        ])
        .exited(0);
    world.until("the held node to be dispatched", |world| {
        !world
            .events_of("validatormissing", "node-dispatched")
            .is_empty()
    });
    world
        .run_with_stdin(
            &["reply", "validatormissing"],
            &envelope(json!([{"op": "add", "node": agent("fresh", &[])}])),
        )
        .exited(REFUSED)
        .err_has("could not be started")
        .err_has("checked by nothing");
    world
        .run(&["results", "validatormissing"])
        .exited(0)
        .out_lacks("fresh");
    world.release("slow.go");
}

/// A launch config carrying the key while declaring a version that never had it
/// is refused by that key's own name, and a config written at an earlier version
/// this build reads still loads.
#[test]
fn a_config_naming_the_key_at_a_version_that_never_had_it_is_refused_by_that_name() {
    let world = World::new("validator-config-version");
    let key = spelling("config_key");
    let arrived = proposed()["config_schema_version"]
        .as_u64()
        .expect("entry 41 states the version the key arrived at");

    let early = world.root.join("early.yaml");
    std::fs::write(
        &early,
        format!("schema_version: {}\n{key}: ./check\n", arrived - 1),
    )
    .expect("the config is written");
    let path = world.plan(
        "configearly",
        &plan_of("configearly", vec![agent("a", &[])]),
    );
    world
        .run(&[
            "start",
            &path.to_string_lossy(),
            "--launch-config",
            &early.to_string_lossy(),
            "--detach",
        ])
        .exited(REFUSED)
        .err_has(&format!("`{key}`"))
        .err_has(&format!("schema {arrived} key"));

    // The version before this one is still a whole document: it says nothing
    // about validating, which is what a launch naming no validator means, and it
    // launches a run.
    let earlier = world.root.join("earlier.yaml");
    std::fs::write(
        &earlier,
        format!(
            "schema_version: {}\npr_author_graph: ./graphs/dag-scope.yaml\n",
            arrived - 1
        ),
    )
    .expect("the config is written");
    world
        .run(&[
            "start",
            &path.to_string_lossy(),
            "--launch-config",
            &earlier.to_string_lossy(),
            "--attach",
        ])
        .exited(0)
        .settled();
}
