//! The hook that reviews a whole reply envelope, driven end to end against a
//! real reviewer program — a compiled one at the seam, which
//! `crates/testfakes/src/bin/envelope-reviewer.rs` says why it is not a double.
//! Entry 45 of the divergence record states the seam it exists for.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. The reviewer is not a substitution either: it is the host's own
// command, and this suite supplies a real one. `harness.rs` carries the same suppression
// and the full rationale.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::harness::{agent, double, plan_of, repo_file, World, REFUSED};

/// What entry 45 of the divergence record proposes, which is where the three
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
        .find(|entry| entry.starts_with("45."))
        .expect("the record still carries entry 45");
    let block = entry
        .split("```json")
        .nth(1)
        .and_then(|rest| rest.split("```").next())
        .expect("entry 45 carries the json block these journeys drive");
    serde_json::from_str::<Value>(block).expect("entry 45's block is JSON")["reviewer"].clone()
}

/// One spelling out of that block, refused loudly when the entry stops naming
/// it: a journey that fell back to a literal would prove the literal.
fn spelling(named: &str) -> String {
    proposed()[named]
        .as_str()
        .unwrap_or_else(|| panic!("entry 45 no longer names the reviewer's {named}"))
        .to_string()
}

/// One copy of the real reviewer, under a name of this journey's choosing.
///
/// Three names for one program is how the precedence journey tells which
/// reviewer a launch resolved: the program records the name it was invoked as
/// and says it in every refusal, so the answer comes off the process that
/// actually ran.
fn reviewer_named(world: &World, name: &str) -> String {
    let path = world
        .root
        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(double("envelope-reviewer"), &path).expect("the reviewer is placed");
    path.to_string_lossy().into_owned()
}

/// Every envelope the reviewer was offered, in order, each with the name the
/// reviewer was invoked as.
fn offered(world: &World) -> Vec<(String, Value)> {
    let path = world.fakes.join("reviewer.jsonl");
    // Absent is a reviewer that has not been invoked yet, and nothing else is:
    // this file is the only witness to what crossed the stdin, so a journey that
    // could not read it would report "nothing was offered" — which is exactly
    // what several of the assertions below take as a pass.
    let recorded = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => panic!(
            "the reviewer's record at {} cannot be read ({error}), so what it was offered is \
             unknown rather than nothing",
            path.display()
        ),
    };
    recorded
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let record: Value = serde_json::from_str(line).expect("the reviewer records JSON");
            (
                record["as"].as_str().expect("it names itself").to_string(),
                record["envelope"].clone(),
            )
        })
        .collect()
}

/// What the host's review says, in the sentence a manager reads. The reviewer
/// prefixes it with the name it was invoked as, and declares the node it
/// objected to on the line before it.
const RULES: &str =
    "its acceptance criterion contradicts a rule the target repository states in its own suite";

fn envelope(commands: Value) -> String {
    json!({"version": 2, "commands": commands}).to_string()
}

/// The two-node edit these journeys submit: a node, and a second one that
/// depends on it. One envelope, two nodes, and an edge between them — which is
/// exactly the shape no per-node check can see.
fn two_related_nodes() -> Value {
    json!([
        {"op": "add", "node": agent("cover", &[])},
        {"op": "add", "node": agent("verify", &["cover"])},
    ])
}

/// Start a run whose one node is held open, so the graph is live while edits
/// arrive.
fn live_run(world: &World, name: &str, extra: &[&str]) -> String {
    // A fresh hold each time: the rendezvous a previous run released is a file,
    // and left in place it satisfies this run's hold the instant the dispatch
    // reaches it — which is a run that settles rather than one an edit can reach.
    let _ = std::fs::remove_file(world.fakes.join("slow.go"));
    world.script("slow.wait", "hold");
    let path = world.plan(name, &plan_of(name, vec![agent("slow", &[])]));
    let mut args = vec!["start".to_string(), path.clone()];
    args.extend(extra.iter().map(|arg| (*arg).to_string()));
    args.push("--detach".to_string());
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    world.run(&borrowed).exited(0);
    world.until("the held node to be running", |world| {
        world
            .run(&["status", name])
            .stdout
            .contains("slow: running")
    });
    name.to_string()
}

/// The journey the hook exists for: an envelope the host's review refuses is
/// refused **whole**, and the same envelope goes through once the review is
/// satisfied — carrying every node it changes, the plan, and the goal.
#[test]
fn a_refused_envelope_applies_none_of_its_commands_and_an_accepted_one_is_reviewed_once() {
    let world = World::new("reviewer-refuses");
    let reviewer = reviewer_named(&world, "review-edit");
    let run = live_run(&world, "reviewerrefuses", &[&spelling("flag"), &reviewer]);

    world.script("reviewer.refuse", RULES);
    let refused = world.run_with_stdin(&["reply", &run], &envelope(two_related_nodes()));
    refused
        .exited(REFUSED)
        .err_has(RULES)
        // The node the reviewer objected to, told apart from the other node the
        // same envelope carried: an envelope is no longer one command, so a
        // refusal a reader cannot locate is one nobody can act on.
        .err_has("refused this envelope over node 'cover',")
        // And everything the envelope carried beside that, which is not the
        // same set: it is what a reader looks over, rather than what the
        // reviewer turned down.
        .err_has("add 'cover'")
        .err_has("add 'verify'");

    // Refused **whole**: neither node joined the graph, so no command of the
    // envelope half-applied.
    for node in ["cover", "verify"] {
        world.run(&["results", &run]).exited(0).out_lacks(node);
        world.run(&["status", &run]).exited(0).out_lacks(node);
    }

    // The document the reviewer read: both nodes it introduces with the op that
    // produced each, the plan they are being edited into, and the run's goal.
    let seen = offered(&world);
    assert_eq!(
        seen.len(),
        1,
        "the envelope was not reviewed once: {seen:?}"
    );
    let (invoked_as, document) = &seen[0];
    assert_eq!(invoked_as, "review-edit");
    assert_eq!(
        document["goal"],
        json!("Deliver reviewerrefuses"),
        "{document}"
    );
    assert_eq!(
        document["changes"]
            .as_array()
            .expect("the changes are a list")
            .iter()
            .map(|change| (
                change["op"].as_str().expect("an op").to_string(),
                change["node"]["id"].as_str().expect("a node").to_string()
            ))
            .collect::<Vec<_>>(),
        vec![
            ("add".to_string(), "cover".to_string()),
            ("add".to_string(), "verify".to_string())
        ],
        "{document}"
    );
    // The prose and the edge, which is the half a per-node check cannot see.
    assert!(
        document["changes"][0]["node"]["task"]
            .as_str()
            .expect("the task crossed")
            .contains("Acceptance criteria"),
        "{document}"
    );
    assert_eq!(document["changes"][1]["node"]["deps"], json!(["cover"]));
    // And the plan as the envelope leaves it: the node already running, plus
    // both the envelope adds.
    let planned: Vec<String> = document["plan"]["tasks"]
        .as_array()
        .expect("the plan carries its tasks")
        .iter()
        .map(|task| task["id"].as_str().expect("an id").to_string())
        .collect();
    for node in ["slow", "cover", "verify"] {
        assert!(planned.contains(&node.to_string()), "{document}");
    }

    // With the review satisfied, the same envelope goes through and both nodes
    // run. The reviewer narrates on stdout while it does, the way a review that
    // reports what it checked does — and none of it reaches `reply`'s own
    // stdout, which is a machine-readable verdict its caller parses.
    std::fs::remove_file(world.fakes.join("reviewer.refuse")).expect("the review is satisfied");
    let narration = "read the goal, 3 nodes, and 1 new edge";
    world.script("reviewer.chatter", narration);
    let applied = world.run_with_stdin(&["reply", &run], &envelope(two_related_nodes()));
    applied
        .exited(0)
        .out_has("\"applied\"")
        .out_lacks(narration);
    let verdict: Value = serde_json::from_str(applied.stdout.trim())
        .unwrap_or_else(|e| panic!("`reply` printed something other than its verdict: {e}"));
    assert_eq!(verdict["state"], json!("applied"), "{verdict}");
    world.until("the accepted nodes to settle", |world| {
        let results = world.run(&["results", &run]).stdout;
        ["cover", "verify"].iter().all(|node| {
            results
                .lines()
                .any(|line| line.trim_start().starts_with(node) && line.contains("done"))
        })
    });

    // Offered **once** for the accepted envelope, and deliberately: unlike the
    // per-node validator, which the submission check and the reconciler both
    // run, a review a host plausibly answers with an agent is not asked the same
    // question twice — and the submission check is the only place a refusal is
    // still whole.
    let seen = offered(&world);
    assert_eq!(
        seen.len(),
        2,
        "an accepted envelope was reviewed more than once: {seen:?}"
    );

    // Two commands about one node are two entries under the op each carried,
    // and both show the node as the envelope leaves it rather than as the
    // command carried it — the amendment is on the added node in both, because
    // that is the node this edit would dispatch.
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([
                {"op": "add", "node": agent("rework", &[])},
                {"op": "amend", "id": "rework", "text": "the ruling"},
            ])),
        )
        .exited(0)
        .out_has("\"applied\"");
    let seen = offered(&world);
    let document = &seen.last().expect("the envelope was reviewed").1;
    assert_eq!(
        document["changes"]
            .as_array()
            .expect("the changes are a list")
            .iter()
            .map(|change| (
                change["op"].as_str().expect("an op").to_string(),
                change["node"]["id"].as_str().expect("a node").to_string(),
                change["node"]["amendment"].clone()
            ))
            .collect::<Vec<_>>(),
        vec![
            ("add".to_string(), "rework".to_string(), json!("the ruling")),
            (
                "amend".to_string(),
                "rework".to_string(),
                json!("the ruling")
            )
        ],
        "{document}"
    );
    world.release("slow.go");
}

/// Which node the reviewer objected to, told apart from the other nodes the same
/// envelope carried — and said outright when it declared none.
///
/// The set an envelope offered for review is not the set the reviewer turned
/// down, and a reader handed only the first still cannot tell which node to go
/// and change. So the reviewer declares the node, on the line entry 45 states,
/// and the three answers it can leave are three different facts a reader acts
/// differently on: a node this envelope changes, a name it does not carry, and no
/// declaration at all. Reporting the last as the first, by listing everything the
/// envelope offered, is the failure the declaration exists to end.
#[test]
fn a_refusal_names_the_node_objected_to_and_says_so_when_the_reviewer_declared_none() {
    let world = World::new("reviewer-objection");
    let reviewer = reviewer_named(&world, "review-edit");
    let run = live_run(&world, "reviewerobjection", &[&spelling("flag"), &reviewer]);
    // The declaration is composed from the prefix the record states rather than
    // from a literal here, so a build that stopped reading that line fails this
    // journey instead of quietly reporting every refusal as unnamed.
    let prefix = spelling("objection_prefix");
    world.script("reviewer.refuse", RULES);

    for (which, declares, names, absent) in [
        (
            "one of the two nodes it was offered",
            format!("{prefix} cover"),
            "refused this envelope over node 'cover', so none of its edits were applied",
            vec!["over node 'verify'", "over nodes"],
        ),
        (
            "both of them, for a seam between the two",
            format!("{prefix} cover\n{prefix} verify"),
            "refused this envelope over nodes 'cover', 'verify', so none of its edits were \
             applied",
            vec!["over node 'cover',"],
        ),
        (
            "a name no node in the envelope goes by",
            format!("{prefix} ghost"),
            "over the name 'ghost', which no node this envelope changes goes by",
            vec!["over node"],
        ),
        (
            "nothing at all",
            String::new(),
            "refused this envelope without declaring the node it objected to",
            vec!["over node", "over the name"],
        ),
    ] {
        world.script("reviewer.objection", &declares);
        let refused = world.run_with_stdin(&["reply", &run], &envelope(two_related_nodes()));
        refused.exited(REFUSED).err_has(names);
        for other in &absent {
            refused.err_lacks(other);
        }
        // The reviewer's own sentence still reaches the manager beside it, and
        // the declaration is lifted out rather than read back in front of it.
        refused.err_has(RULES).err_lacks(&prefix);
        // And every one of them is still a refusal of the whole envelope: a
        // reviewer that declared nothing identifiable is not a reviewer that
        // accepted anything.
        for node in ["cover", "verify"] {
            world.run(&["results", &run]).exited(0).out_lacks(node);
        }
        assert!(
            refused.stderr.lines().count() == 1,
            "a reviewer declaring {which} left a refusal of more than one line: {:?}",
            refused.stderr
        );
    }
    world.release("slow.go");
}

/// A reviewer that cannot be started refuses the envelope rather than letting it
/// through unreviewed, and a launch that configures none behaves exactly as it
/// did before this hook existed.
///
/// The first is the whole reason this fails closed: accepting an envelope
/// because the review could not be run would be the crate deciding that an
/// unenforced rule is no rule, silently, on the path a manager reaches for under
/// pressure.
#[test]
fn an_unstartable_reviewer_fails_closed_and_a_launch_naming_none_is_unchanged() {
    let world = World::new("reviewer-closed");
    let missing = world.root.join("no-such-reviewer");
    let run = live_run(
        &world,
        "reviewerclosed",
        &[&spelling("flag"), &missing.to_string_lossy()],
    );

    world
        .run_with_stdin(&["reply", &run], &envelope(two_related_nodes()))
        .exited(REFUSED)
        .err_has("reviewed by nothing")
        .err_has("could not be started");
    world.run(&["results", &run]).exited(0).out_lacks("cover");
    world.release("slow.go");

    // The same envelope, into a run whose launch named no reviewer at all: the
    // edits are applied exactly as they were before this hook, and nothing was
    // asked about them.
    let plain = live_run(&world, "reviewernone", &[]);
    world
        .run_with_stdin(&["reply", &plain], &envelope(two_related_nodes()))
        .exited(0)
        .out_has("\"applied\"");
    assert!(
        offered(&world).is_empty(),
        "a launch that named no reviewer ran one"
    );
    world.release("slow.go");
}

/// The three names, in the order the record states, proven by driving them
/// rather than by asserting the order in prose.
///
/// Each rung is added on top of the one below it and the answer is read off the
/// program that actually ran, so what is proven is which reviewer the launch
/// resolved rather than which one this crate believes it picked.
#[test]
fn the_flag_beats_the_environment_which_beats_the_config() {
    let precedence: Vec<String> = serde_json::from_value(proposed()["precedence"].clone())
        .expect("entry 45 states the precedence it proposes");
    assert_eq!(
        precedence,
        vec!["flag", "environment", "config_key"],
        "entry 45 proposes a different order than this journey drives"
    );

    let world = World::new("reviewer-precedence");
    let by_flag = reviewer_named(&world, "by-flag");
    let by_environment = reviewer_named(&world, "by-environment");
    let by_config = reviewer_named(&world, "by-config");
    let config = world.root.join("launch.yaml");
    std::fs::write(
        &config,
        format!(
            "schema_version: {}\n{}: {by_config}\n",
            proposed()["config_schema_version"]
                .as_u64()
                .expect("entry 45 states the version the key arrived at"),
            spelling("config_key"),
        ),
    )
    .expect("the launch config is written");

    // Every reviewer refuses, naming itself, so which one a launch resolved is
    // readable off `reply`'s own stderr rather than out of any file.
    world.script("reviewer.refuse", RULES);

    for (which, extra, environment) in [
        ("by-config", vec![], None),
        ("by-environment", vec![], Some(by_environment.clone())),
        (
            "by-flag",
            vec![spelling("flag"), by_flag.clone()],
            Some(by_environment.clone()),
        ),
    ] {
        let name = format!("precedence{which}").replace('-', "");
        let path = world.plan(&name, &plan_of(&name, vec![agent("slow", &[])]));
        // A fresh hold each time: the rendezvous the previous iteration released
        // is a file, and left in place it satisfies this run's hold the instant
        // the dispatch reaches it.
        let _ = std::fs::remove_file(world.fakes.join("slow.go"));
        world.script("slow.wait", "hold");
        let mut args = vec![
            "start".to_string(),
            path.clone(),
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

        // A `reply` typed with *no* environment at all is judged by the reviewer
        // the run was launched under, because it is resolved into the launch
        // record rather than re-read here.
        let mut reply = world.cmd(&["reply", &name]);
        reply.env_remove(spelling("environment"));
        let refused = world.run_with_stdin_on(reply, &envelope(two_related_nodes()));
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
}

/// Every op that introduces or changes a node is listed in the document, and no
/// other op is — held against the list entry 45 states, in both directions.
///
/// The drift gate for that list: the record names the ops it proposes and this
/// journey drives every op the protocol has through the real CLI, so a set the
/// code and the record disagree about fails here rather than reaching a host as
/// a document it was not told to expect. And an envelope that changes no node at
/// all is still reviewed — its edits are the plan's, which is where a review
/// sees them.
#[test]
fn the_ops_listed_as_changes_are_the_ones_the_record_names_and_no_others() {
    let listed: Vec<String> = serde_json::from_value(proposed()["ops_listed_as_changes"].clone())
        .expect("entry 45 names the ops it lists as changes");
    let world = World::new("reviewer-ops");
    let reviewer = reviewer_named(&world, "review-edit");
    world.script("build.fail", "1");
    // A second node that fails and is never retried, so there is a reference
    // `attest` takes: that op accepts a waiting human action or a node that
    // settled failed, and this run's held node is neither.
    world.script("flaky.fail", "1");
    world.script("slow.wait", "hold");
    // `spare` waits on the held node throughout, so it is a node an edit can
    // still reach: unstarted, so it can be reparented, parked, and requeued.
    let path = world.plan(
        "reviewerops",
        &plan_of(
            "reviewerops",
            vec![
                agent("slow", &[]),
                agent("build", &[]),
                agent("flaky", &[]),
                agent("spare", &["slow"]),
            ],
        ),
    );
    world
        .run(&["start", &path, &spelling("flag"), &reviewer, "--detach"])
        .exited(0);
    let run = "reviewerops".to_string();
    for failing in ["build", "flaky"] {
        world.until("the node that fails to settle", |world| {
            world
                .run(&["results", &run])
                .stdout
                .lines()
                .any(|line| line.trim_start().starts_with(failing) && line.contains("failed"))
        });
    }

    // One envelope per op, so what each contributes to the document is read off
    // that envelope's own review rather than untangled from a batch.
    let commands = [
        json!({"op": "add", "node": agent("fresh", &[])}),
        json!({"op": "retry", "id": "build", "node": agent("build-2", &[])}),
        json!({"op": "amend", "id": "spare", "text": "the ruling"}),
        json!({"op": "cancel", "id": "spare"}),
        // Listed: a requeue carrying any amendment changes the node a reviewer
        // reads, whether or not the amendment touches its task.
        json!({"op": "requeue", "id": "spare", "amend": {"max_turns": 9}}),
        // And not listed without one: a requeue that amends nothing returns the
        // node it parked exactly as it was.
        json!({"op": "cancel", "id": "spare"}),
        json!({"op": "requeue", "id": "spare"}),
        json!({"op": "note", "id": "spare", "addressee": "worker",
               "text": "the fixture moved", "deliver": "next"}),
        // Last of the ops about `spare`, because it moves that node onto a
        // dependency that has already settled and so lets it run.
        json!({"op": "reparent", "id": "spare", "deps": ["fresh"]}),
        // A node waiting on the held one, so there is something still droppable
        // once everything else has run.
        json!({"op": "add", "node": agent("extra", &["slow"])}),
        // Listed as no change at all, like the `cancel` and the `note` above:
        // these move the plan without changing any node's definition, and the
        // plan is where the review sees them.
        json!({"op": "drop", "id": "extra", "dependents": "detach"}),
        // The three ops that touch no node's definition at all: one clears a
        // reference, and two report to the planner.
        json!({"op": "attest", "ref": "flaky"}),
        json!({"op": "finding", "message": "the plan still owes a rollback", "blocking": false}),
        json!({"op": "complete", "reason": "the run has delivered its goal"}),
    ];
    // Every op the live-edit protocol has today is in that table, named here so
    // that an op added to the protocol and not to this journey reads as a gap
    // rather than as a pass.
    let driven: BTreeSet<String> = commands
        .iter()
        .map(|command| {
            command["op"]
                .as_str()
                .expect("each names its op")
                .to_string()
        })
        .collect();
    assert_eq!(
        driven,
        [
            "add", "amend", "attest", "cancel", "complete", "drop", "finding", "note",
            "reparent", "requeue", "retry",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<BTreeSet<String>>(),
        "this journey no longer drives every op the protocol has"
    );

    for command in &commands {
        world
            .run_with_stdin(&["reply", &run], &envelope(json!([command])))
            .exited(0);
    }

    let seen = offered(&world);
    // One review per envelope, including the envelopes that changed no node:
    // every accepted envelope is offered, and each exactly once.
    assert_eq!(
        seen.len(),
        commands.len(),
        "an envelope was reviewed a different number of times than once: {seen:?}"
    );
    let ops: Vec<Vec<String>> = seen
        .iter()
        .map(|(_, document)| {
            document["changes"]
                .as_array()
                .expect("the changes are a list")
                .iter()
                .map(|change| change["op"].as_str().expect("an op").to_string())
                .collect()
        })
        .collect();
    // Every op that changes no node contributes nothing, and every other
    // envelope contributes exactly the op it carried.
    assert_eq!(
        ops,
        vec![
            vec!["add".to_string()],
            vec!["retry".to_string()],
            vec!["amend".to_string()],
            vec![],
            vec!["requeue".to_string()],
            vec![],
            vec![],
            vec![],
            vec!["reparent".to_string()],
            vec!["add".to_string()],
            vec![],
            vec![],
            vec![],
            vec![],
        ],
        "the ops listed as changes are not the ones the envelopes carried"
    );
    // And the set of them is the record's, exactly: neither an op the code
    // lists and the record does not, nor one the record names and the code
    // never sends.
    let mut sent: Vec<String> = ops.into_iter().flatten().collect();
    sent.sort();
    sent.dedup();
    let mut named = listed.clone();
    named.sort();
    assert_eq!(
        sent, named,
        "the ops this build lists as changes are not the ops entry 45 names"
    );
    world.release("slow.go");
}

/// What a reviewer says is external input, and a manager reads it: the refusal
/// carries the sentence that matters, on one line, and does not grow with a
/// reviewer that dumped its whole trace after it. A reviewer that says nothing
/// at all is still not silent.
#[test]
fn a_refusal_carries_what_the_reviewer_said_without_its_escape_codes_or_its_trace() {
    let world = World::new("reviewer-loud");
    let reviewer = reviewer_named(&world, "review-edit");
    let run = live_run(&world, "reviewerloud", &[&spelling("flag"), &reviewer]);

    // The sentence that matters, wrapped in the colour a review prints it in,
    // with a second line after it and a large trace to follow.
    let sentence = "rule 3 failed: this envelope duplicates a seam the plan already owns";
    let esc = '\u{1b}';
    world.script(
        "reviewer.refuse",
        &format!("{esc}[31m{sentence}{esc}[0m\nsee the trace below"),
    );
    let flood = 100_000;
    world.script("reviewer.flood", &flood.to_string());

    let refused = world.run_with_stdin(&["reply", &run], &envelope(two_related_nodes()));
    refused.exited(REFUSED).err_has(sentence);
    assert!(
        !refused.stderr.contains('\u{1b}'),
        "a reviewer's escape sequences reached the refusal: {:?}",
        refused.stderr
    );
    assert_eq!(
        refused.stderr.lines().count(),
        1,
        "the refusal is not one line: {:?}",
        refused.stderr
    );
    assert!(
        refused.stderr.len() < flood / 4,
        "the refusal grew with the reviewer's trace: {} bytes",
        refused.stderr.len()
    );

    // A reviewer that refuses without saying anything: never silent, because a
    // refusal nobody can act on is the failure this hook exists to end.
    world.script("reviewer.silent", "");
    world
        .run_with_stdin(&["reply", &run], &envelope(two_related_nodes()))
        .exited(REFUSED)
        .err_has("exited 5")
        .err_has("said nothing on stderr");

    // And the edits are still refused rather than lost in the noise.
    world.run(&["results", &run]).exited(0).out_lacks("cover");
    world.release("slow.go");
}

/// A rung that is *there* and names nothing is a launch saying it has none,
/// rather than a fall-through to the rung below — and a variable this build
/// cannot read as text is refused at the launch rather than discarded.
#[test]
fn a_blank_rung_names_no_reviewer_and_an_unreadable_variable_is_refused() {
    let world = World::new("reviewer-none");
    let by_config = reviewer_named(&world, "by-config");
    let config = world.root.join("launch.yaml");
    std::fs::write(
        &config,
        format!(
            "schema_version: {}\n{}: {by_config}\n",
            proposed()["config_schema_version"]
                .as_u64()
                .expect("entry 45 states the version the key arrived at"),
            spelling("config_key"),
        ),
    )
    .expect("the launch config is written");
    // The reviewer this config names refuses everything, so an edit that is
    // *applied* below is one nothing was asked about.
    world.script("reviewer.refuse", RULES);

    // Three launches that name none: no rung at all, a blank flag over a config
    // that names one, and a blank variable over the same config. A host that
    // exported the variable empty to turn the hook off would otherwise get the
    // config's reviewer.
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
        let name = format!("reviewernone{at}");
        let _ = std::fs::remove_file(world.fakes.join("slow.go"));
        world.script("slow.wait", "hold");
        let path = world.plan(&name, &plan_of(&name, vec![agent("slow", &[])]));
        let mut args = vec!["start".to_string(), path.clone()];
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

        let mut reply = world.cmd(&["reply", &name]);
        match &environment {
            Some(value) => reply.env(spelling("environment"), value),
            None => reply.env_remove(spelling("environment")),
        };
        world
            .run_with_stdin_on(reply, &envelope(two_related_nodes()))
            .exited(0)
            .out_has("\"applied\"")
            .err_lacks(RULES);
        assert!(
            offered(&world).is_empty(),
            "a launch naming {which} ran a reviewer"
        );
        world.release("slow.go");
    }

    // And a variable that is there and holds something this build cannot read
    // as text: refused at the launch, naming the variable, rather than
    // discarded — which would hand the run whichever reviewer the config names.
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let name = "reviewergarbled";
        let _ = std::fs::remove_file(world.fakes.join("slow.go"));
        world.script("slow.wait", "hold");
        let path = world.plan(name, &plan_of(name, vec![agent("slow", &[])]));
        let mut command = world.cmd(&["start", &path, "--detach"]);
        command.env(
            spelling("environment"),
            std::ffi::OsStr::from_bytes(&[0x2e, 0xff, 0x2e]),
        );
        let refused = world.run_on(command, "start");
        refused.exited(REFUSED);
        assert!(
            refused.stderr.contains(&spelling("environment")),
            "the refusal does not name the variable: {}",
            refused.stderr
        );
    }
}

/// The reviewer a launch resolved is in the launch record, and an `adopt`
/// replays it rather than re-reading an environment that has since moved.
///
/// It is resolved **once**, before the run exists, out of three names a later
/// process has no way to resolve the same way: a fresh driver started from
/// another shell — with another `ONEPIPELINE_ENVELOPE_REVIEWER`, or none —
/// would otherwise review the run's envelopes by rules its launch never chose.
#[test]
fn the_resolved_reviewer_is_in_the_launch_record_and_survives_an_adoption() {
    let world = World::new("reviewer-adopt");
    let chosen = reviewer_named(&world, "by-flag");
    let elsewhere = reviewer_named(&world, "somewhere-else");
    let name = "revieweradopt";
    let path = world.plan(name, &plan_of(name, vec![agent("only", &[])]));

    // Launched under an environment naming a *different* reviewer, so what the
    // record carries is what the launch resolved rather than what was ambient.
    let mut launch = world.cmd(&["start", &path, &spelling("flag"), &chosen, "--attach"]);
    launch.env(spelling("environment"), &elsewhere);
    world.run_on(launch, "start").exited(0).settled();
    assert_eq!(
        world.run_json(name, "launch.json")["envelope_reviewer"],
        json!(chosen),
        "the launch record does not carry the reviewer the launch resolved"
    );

    // A fresh driver takes up what its launch chose, adopted from a shell whose
    // environment still names the other one.
    let mut adopt = world.cmd(&["adopt", name]);
    adopt.env(spelling("environment"), &elsewhere);
    world.run_on(adopt, "adopt").exited(0);

    // And the envelope that follows is reviewed by the reviewer the *launch*
    // chose, said in the refusal a manager reads — from a third shell naming the
    // other one again.
    world.script("reviewer.refuse", RULES);
    let mut reply = world.cmd(&["reply", name]);
    reply.env(spelling("environment"), &elsewhere);
    let refused = world.run_with_stdin_on(reply, &envelope(two_related_nodes()));
    refused
        .exited(REFUSED)
        .err_has(&format!("by-flag: {RULES}"))
        .err_lacks("somewhere-else");
}
