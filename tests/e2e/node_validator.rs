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
    // Absent is a validator that has not been invoked yet, and nothing else is:
    // this file is the only witness to what crossed the stdin, so a journey that
    // could not read it would report "nothing was offered" — which is exactly
    // what several of the assertions below take as a pass.
    let recorded = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => panic!(
            "the validator's record at {} cannot be read ({error}), so what it was offered is \
             unknown rather than nothing",
            path.display()
        ),
    };
    recorded
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

/// What the host's rules say, in the sentence a manager reads. Each validator
/// prefixes it with the name it was invoked as.
const RULES: &str = "this node's criteria name a procedure rather than a property";

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
    // Waited out on the view a supervisor watches a run through: `status`
    // reports a node that is running and how long it has been.
    world.until("the held node to be running", |world| {
        world
            .run(&["status", name])
            .stdout
            .contains("slow: running")
    });
    name.to_string()
}

/// Whether a node has settled, as the view a planner reads an outcome from says.
fn settled(world: &World, run: &str, node: &str, status: &str) -> bool {
    world
        .run(&["results", run])
        .stdout
        .lines()
        .any(|line| line.trim_start().starts_with(node) && line.contains(status))
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

    // And the graph is unchanged: the node the edit would have added is in
    // neither view a supervisor reads the graph from.
    world.run(&["results", &run]).exited(0).out_lacks("fresh");
    world.run(&["status", &run]).exited(0).out_lacks("fresh");

    // With the rules satisfied, the same edit goes through and the node runs.
    //
    // The validator narrates on stdout while it does, the way a host's rules
    // engine does — and none of it reaches `reply`'s own stdout, which is a
    // machine-readable verdict its caller parses. A validator that could write
    // into it could make an applied edit unreadable, or read as a different one.
    std::fs::remove_file(world.fakes.join("validator.refuse")).expect("the rule is lifted");
    let narration = "checked 14 rules against the resolved review bar";
    world.script("validator.chatter", narration);
    let applied = world.run_with_stdin(
        &["reply", &run],
        &envelope(json!([{"op": "add", "node": agent("fresh", &[])}])),
    );
    applied
        .exited(0)
        .out_has("\"applied\"")
        .out_lacks(narration);
    let verdict: Value = serde_json::from_str(applied.stdout.trim())
        .unwrap_or_else(|e| panic!("`reply` printed something other than its verdict: {e}"));
    assert_eq!(verdict["state"], json!("applied"), "{verdict}");
    world.until("the accepted node to settle", |world| {
        settled(world, &run, "fresh", "done")
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
        settled(world, &run, "build", "failed")
    });

    for command in [
        json!({"op": "add", "node": agent("fresh", &[])}),
        json!({"op": "retry", "id": "build", "node": agent("build-2", &[])}),
        json!({"op": "amend", "id": "spare", "text": "the ruling"}),
        json!({"op": "cancel", "id": "spare"}),
        // Offered: this requeue's amendment rewrites the task.
        json!({"op": "requeue", "id": "spare", "amend": {"task": "## What\nsomething else"}}),
        json!({"op": "cancel", "id": "spare"}),
        // Not offered: this one raises a turn budget, which changes nothing a
        // dispatch is asked to do — and neither does a cancel or a note.
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

    // Every validator refuses, naming itself, so which one a launch resolved is
    // readable off `reply`'s own stderr rather than out of any file.
    world.script("validator.refuse", RULES);

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
        // A fresh hold each time: the rendezvous the previous iteration released
        // is a file, and left in place it satisfies this run's hold the instant
        // the dispatch reaches it.
        let _ = std::fs::remove_file(world.fakes.join("slow.go"));
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
        world.until("the held node to be running", |world| {
            world
                .run(&["status", &name])
                .stdout
                .contains("slow: running")
        });

        // A `reply` typed with *no* environment at all is judged by the rules
        // the run was launched under, because the resolved validator is in the
        // launch record rather than re-read here. Which validator that was is
        // read off the refusal, which is where a manager reads one: each of the
        // three names itself in what it says.
        let mut reply = world.cmd(&["reply", &name]);
        reply.env_remove(spelling("environment"));
        let refused = world.run_with_stdin_on(
            reply,
            &envelope(json!([{"op": "add", "node": agent("fresh", &[])}])),
        );
        refused
            .exited(REFUSED)
            .err_has(&format!("{which}: {RULES}"));
        for other in ["by-flag", "by-environment", "by-config"] {
            if other != which {
                refused.err_lacks(other);
            }
        }
        world.release("slow.go");
    }

    // And three launches that name none: no rung at all, a blank flag, and a
    // blank variable. The last two are a rung that is *there* and names nothing,
    // which is a launch saying it has none — not a fall-through to the config,
    // which names one throughout this journey. A host that exported the variable
    // empty to turn the hook off would otherwise get the config's validator.
    for (at, (which, names_a_config, extra, environment)) in [
        ("no rung at all", false, vec![], None),
        (
            "a blank flag",
            true,
            vec![spelling("flag"), "   ".to_string()],
            None,
        ),
        ("a blank variable", true, vec![], Some(String::new())),
    ]
    .into_iter()
    .enumerate()
    {
        let before = offered(&world).len();
        let name = format!("precedence-none-{at}");
        let path = world.plan(&name, &plan_of(&name, vec![agent("slow", &[])]));
        let _ = std::fs::remove_file(world.fakes.join("slow.go"));
        world.script("slow.wait", "hold");
        let mut args = vec!["start".to_string(), path.to_string_lossy().into_owned()];
        if names_a_config {
            args.push("--launch-config".to_string());
            args.push(config.to_string_lossy().into_owned());
        }
        args.extend(extra);
        args.push("--detach".to_string());
        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut command = world.cmd(&borrowed);
        match &environment {
            Some(value) => command.env(spelling("environment"), value),
            None => command.env_remove(spelling("environment")),
        };
        world.run_on(command, "start").exited(0);
        world.until("the held node to be running", |world| {
            world
                .run(&["status", &name])
                .stdout
                .contains("slow: running")
        });
        // Every validator this journey placed refuses, so an edit that is
        // *applied* is one nothing was asked about.
        let mut reply = world.cmd(&["reply", &name]);
        match &environment {
            Some(value) => reply.env(spelling("environment"), value),
            None => reply.env_remove(spelling("environment")),
        };
        world
            .run_with_stdin_on(
                reply,
                &envelope(json!([{"op": "add", "node": agent("fresh", &[])}])),
            )
            .exited(0)
            .out_has("\"applied\"")
            .err_lacks(RULES);
        assert_eq!(
            offered(&world).len(),
            before,
            "a launch naming {which} ran a validator"
        );
        world.release("slow.go");
    }
}

/// The validator a launch resolved is in the launch record, and an `adopt`
/// replays it rather than re-reading an environment that has since moved.
///
/// It is resolved **once**, before the run exists, out of three names a later
/// process has no way to resolve the same way: a fresh driver started from
/// another shell — with another `ONEPIPELINE_NODE_VALIDATOR`, or none — would
/// otherwise judge the run's edits by rules its launch never chose.
#[test]
fn the_resolved_validator_is_in_the_launch_record_and_survives_an_adoption() {
    let world = World::new("validator-adopt");
    let chosen = validator_named(&world, "by-flag");
    let elsewhere = validator_named(&world, "somewhere-else");
    let name = "validatoradopt";
    let path = world.plan(name, &plan_of(name, vec![agent("only", &[])]));

    // Launched under an environment naming a *different* validator, so what the
    // record carries is what the launch resolved rather than what was ambient.
    let mut launch = world.cmd(&[
        "start",
        &path.to_string_lossy(),
        &spelling("flag"),
        &chosen,
        "--attach",
    ]);
    launch.env(spelling("environment"), &elsewhere);
    world.run_on(launch, "start").exited(0).settled();

    // A fresh driver takes up what its launch chose. Adopted from a shell whose
    // environment still names the other one, which is exactly the drift this
    // guards against.
    let mut adopt = world.cmd(&["adopt", name]);
    adopt.env(spelling("environment"), &elsewhere);
    world.run_on(adopt, "adopt").exited(0);

    // And the edit that follows is judged by the validator the *launch* chose,
    // said in the refusal a manager reads — from a third shell naming the other
    // one again.
    world.script("validator.refuse", RULES);
    let mut reply = world.cmd(&["reply", name]);
    reply.env(spelling("environment"), &elsewhere);
    let refused = world.run_with_stdin_on(
        reply,
        &envelope(json!([{"op": "add", "node": agent("fresh", &[])}])),
    );
    refused
        .exited(REFUSED)
        .err_has(&format!("by-flag: {RULES}"))
        .err_lacks("somewhere-else");
}

/// What a validator says is external input, and a manager reads it: the refusal
/// carries the sentence that matters, on one line, and does not grow with a
/// validator that dumped its whole trace after it.
///
/// A rules engine that prints escape sequences and then a megabyte of trace is
/// ordinary. What must not happen is that reaching a terminal, a planner's
/// queue, and the journal — where every payload text this crate writes is
/// already bounded.
#[test]
fn a_refusal_carries_what_the_validator_said_without_its_escape_codes_or_its_trace() {
    let world = World::new("validator-loud");
    let validator = validator_named(&world, "check-node");
    let run = live_run(&world, "validatorloud", &["--node-validator", &validator]);

    // The sentence that matters, wrapped in the colour a rules engine prints it
    // in, with a second line after it and a large trace to follow.
    let sentence = "rule 3 failed: criterion 2 names a procedure";
    let esc = '\u{1b}';
    world.script(
        "validator.refuse",
        &format!("{esc}[31m{sentence}{esc}[0m\nsee the trace below"),
    );
    let flood = 100_000;
    world.script("validator.flood", &flood.to_string());

    let refused = world.run_with_stdin(
        &["reply", &run],
        &envelope(json!([{"op": "add", "node": agent("fresh", &[])}])),
    );
    refused.exited(REFUSED).err_has(sentence);
    assert!(
        !refused.stderr.contains('\u{1b}'),
        "a validator's escape sequences reached the refusal: {:?}",
        refused.stderr
    );
    assert!(
        refused.stderr.lines().count() == 1,
        "the refusal is not one line: {:?}",
        refused.stderr
    );
    assert!(
        refused.stderr.len() < flood / 4,
        "the refusal grew with the validator's trace: {} bytes",
        refused.stderr.len()
    );
    // And the edit is still refused rather than lost in the noise.
    world.run(&["results", &run]).exited(0).out_lacks("fresh");
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
    std::fs::remove_file(world.fakes.join("validator.silent")).expect("the scenario is lifted");

    // A validator that ends on a **signal** has no exit status at all — it
    // crashed, or somebody killed it — and the one thing that must not happen is
    // that being read as a verdict. Unix-only for the provocation, not for the
    // rule: only this platform lets a process end without a status.
    #[cfg(unix)]
    {
        world.script("validator.signal", "");
        world
            .run_with_stdin(
                &["reply", &run],
                &envelope(json!([{"op": "add", "node": agent("fresh", &[])}])),
            )
            .exited(REFUSED)
            .err_has("without a status");
        world.run(&["results", &run]).exited(0).out_lacks("fresh");
        std::fs::remove_file(world.fakes.join("validator.signal")).expect("the scenario is lifted");
    }

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
    world.until("the held node to be running", |world| {
        world
            .run(&["status", "validatormissing"])
            .stdout
            .contains("slow: running")
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

/// A variable this build cannot read as text is a rung that is *there* and names
/// something unusable, and the launch is refused by that variable's name.
///
/// Discarded instead, it would read as an unset rung and hand the run whichever
/// validator the config file names — a launch judged by rules its operator did
/// not choose, with nothing said about why.
///
/// Unix-only for the provocation, not for the rule: an environment value that is
/// not text is bytes, and only this platform lets a caller hand one over.
#[test]
#[cfg(unix)]
fn a_validator_variable_this_build_cannot_read_refuses_the_launch_by_its_name() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let world = World::new("validator-not-text");
    let chosen = validator_named(&world, "check-node");
    let name = "validatornottext";
    let path = world.plan(name, &plan_of(name, vec![agent("only", &[])]));
    let variable = spelling("environment");
    let not_text = OsString::from_vec(vec![0x63, 0x68, 0xff, 0x6b]);

    let mut refused = world.cmd(&["start", &path.to_string_lossy(), "--detach"]);
    refused.env(&variable, &not_text);
    world
        .run_on(refused, "start")
        .exited(REFUSED)
        .err_has(&variable)
        .err_has("cannot read as text");

    // And a launch whose flag names one never consults the variable at all: it
    // was not going to use it, so an unreadable one is not its problem.
    let mut named = world.cmd(&[
        "start",
        &path.to_string_lossy(),
        &spelling("flag"),
        &chosen,
        "--attach",
    ]);
    named.env(&variable, &not_text);
    world.run_on(named, "start").exited(0).settled();
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

    // The validator key present and naming nothing is a decision half-written:
    // everything downstream would read it as a launch that named one, and
    // resolve it to a command nothing can start. Refused where the document is
    // read — and only for this key, which arrives with this version, so no
    // config on disk can be carrying a blank one.
    let blank = world.root.join("blank-validator.yaml");
    std::fs::write(
        &blank,
        format!("schema_version: {arrived}\n{key}: \"   \"\n"),
    )
    .expect("the config is written");
    world
        .run(&[
            "start",
            &path.to_string_lossy(),
            "--launch-config",
            &blank.to_string_lossy(),
            "--detach",
        ])
        .exited(REFUSED)
        .err_has(&format!("`{key}`"))
        .err_has("names nothing");

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

/// A launch config already on disk carrying a **blank** `pr_author_graph` starts
/// a run exactly as it did before this change.
///
/// The regression this exists for: the blank-value refusal that arrives with
/// `node_validator` was written for every key at once, and applied to
/// `pr_author_graph` it turns down a document an operator wrote against a build
/// that accepted it — a launch broken over a key its author never touched. Driven
/// through the CLI rather than through the loader alone, because what has to keep
/// working is `onepipeline start` reading the file.
#[test]
fn a_config_carrying_a_blank_drafting_graph_still_launches_a_run() {
    let world = World::new("validator-blank-drafting");
    let plan_path = world.plan(
        "blankdrafting",
        &plan_of("blankdrafting", vec![agent("only", &[])]),
    );

    // At the version that introduced the key, and at this one, because a config
    // on disk declares whichever it was written at.
    for (at, version) in [2, proposed()["config_schema_version"].as_u64().unwrap_or(3)]
        .into_iter()
        .enumerate()
    {
        let config = world.root.join(format!("blank-drafting-v{version}.yaml"));
        std::fs::write(
            &config,
            format!("schema_version: {version}\npr_author_graph: \"\"\n"),
        )
        .expect("the config is written");

        let name = format!("blankdrafting-{at}");
        let plan_for = world.plan(&name, &plan_of(&name, vec![agent("only", &[])]));
        world
            .run(&[
                "start",
                &plan_for.to_string_lossy(),
                "--launch-config",
                &config.to_string_lossy(),
                "--attach",
            ])
            .exited(0)
            .settled();
        // Read from the view a planner reads an outcome from, which is where a
        // run saying it is complete has to say so.
        world
            .run(&["results", &name])
            .exited(0)
            .out_has("complete")
            .out_has("only");
        assert!(
            settled(&world, &name, "only", "done"),
            "a config carrying a blank drafting graph no longer runs its plan"
        );
    }
    // And the plan itself was never the problem: the same one runs with no
    // config at all, which is what the assertions above are compared against.
    world
        .run(&["start", &plan_path.to_string_lossy(), "--attach"])
        .exited(0)
        .settled();
}

/// A blank `pr_author_graph` is the config that omits the key: same launch, and
/// the same record of what it drafts with.
///
/// The other half of the journey above, and the half that does not vary by
/// platform. "Does it launch?" a blank value can pass by accident: `""` resolved
/// against the launch directory *is* the launch directory, and opening a
/// directory is a read the host answers for itself — allowed on Linux and
/// refused on Windows — so one document launched on one platform, exited 2 on
/// another, and where it launched it recorded the launch directory as the graph
/// a change request's body is drafted by. What is asked here is the property no
/// file API is consulted for: a config carrying a blank key and a config that
/// omits it are indistinguishable in what the launch then does.
#[test]
fn a_blank_drafting_graph_records_what_omitting_the_key_records() {
    let world = World::new("validator-blank-drafting-omitted");
    let version = proposed()["config_schema_version"].as_u64().unwrap_or(3);

    let recorded = |run: &str, declared: &str| -> Value {
        let config = world.root.join(format!("{run}.yaml"));
        std::fs::write(&config, format!("schema_version: {version}\n{declared}"))
            .expect("the config is written");
        let path = world.plan(run, &plan_of(run, vec![agent("only", &[])]));
        world
            .run(&[
                "start",
                &path.to_string_lossy(),
                "--launch-config",
                &config.to_string_lossy(),
                "--attach",
            ])
            .exited(0)
            .settled();
        world.run_json(run, "launch.json")["pr_author_graph"].clone()
    };

    let blank = recorded("blankkey", "pr_author_graph: \"\"\n");
    let omitted = recorded("nokey", "");
    assert_eq!(
        blank, omitted,
        "a blank drafting graph did not launch the run the config omitting the key launches"
    );
    // Named, and not only compared: two runs that both recorded the launch
    // directory would agree with each other and still have wired a directory in
    // as the drafting graph. The record omits the field when the launch named no
    // graph, so absent is what "drafts nothing" reads as here.
    assert!(
        blank.is_null(),
        "a blank drafting graph reached the record as a graph this launch names: {blank}"
    );
}
