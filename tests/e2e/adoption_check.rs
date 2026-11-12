//! Whether a release adoption can ever complete is **onevcs's** answer, asked where a
//! plan is loaded and wherever a live edit changes one.
//!
//! Every journey drives the real binary over scratch repositories registered in this
//! world's own `ONEVCS_HOME`: a producer, `engine`, declaring release targets, and a
//! consumer, `service`, publishing `change-auto`. Each refusal is held to name the
//! node, the dependency, `source: "onevcs"`, onevcs's rule id and the reason onevcs's
//! own `release check-adoption` prints for the same pair — so the engine is proven to
//! carry onevcs's words rather than a copy of its rules.

// llmlint: ignore-file[e2e_not_mocked] the crate under test is driven as a real compiled
// binary, and `onevcs` — whose answer these journeys are about — is the real library and
// its real executable over real repositories on disk. `oneagentgraph` is substituted at its
// subprocess boundary so a journey states a dispatch outcome rather than paying for a model
// turn. `harness.rs` carries the same suppression and the full rationale.

use serde_json::{json, Value};

use crate::harness::{
    agent, end_driver, lifecycle, plan_of, posix_script, shell_path, Repository, World, REFUSED,
};

/// `plan check`'s status for a plan something refused.
const HAS_REFUSALS: i32 = 1;

/// The host's release-targets document: `engine` releases `crate`, and names it as its
/// default target only where `default_target` says so.
fn releases(script: &str, default_target: bool) -> String {
    releasing(script, "crate", default_target)
}

/// The same document with `engine` releasing `target` instead.
fn releasing(script: &str, target: &str, default_target: bool) -> String {
    let default = if default_target {
        format!("\x20   default_target: {target}\n")
    } else {
        String::new()
    };
    format!(
        "version: 1\n\
         repositories:\n\
         \x20 - match: {{host: github.com, owner: owner, name: engine}}\n\
         {default}\
         \x20   targets:\n\
         \x20   - name: {target}\n\
         \x20     style: automated\n\
         \x20     probe: {{script: {script}, timeout_seconds: 30}}\n\
         default:\n\x20 adoption: fast\n"
    )
}

/// A world with the consumer `service` publishing `change-auto` and the producer
/// `engine` registered beside it, its probe committed and answering no release.
fn world(name: &str, default_target: bool) -> (World, String, Repository) {
    let world = World::new(name);
    world.write_graphs();
    world.repository("change-auto", &[]);
    let engine = world.extra_repository("engine");
    let (script, _no_release) = world.probe_in(&engine, "engine");
    world.releases(&releases(&script, default_target));
    (world, script, engine)
}

/// The producer node, landing in `engine`.
fn engine() -> Value {
    let mut node = lifecycle("engine", &[]);
    node["repo"] = json!("engine");
    node
}

/// A consumer in `service` depending on `engine`, stating `fields` over the defaults.
fn consumer(id: &str, fields: Value) -> Value {
    let mut node = lifecycle(id, &["engine"]);
    for (key, value) in fields.as_object().expect("fields are an object") {
        node[key] = value.clone();
    }
    node
}

/// What onevcs's own executable says about one consumer of `engine`: its rule ids and
/// reasons, in order.
fn onevcs_says(world: &World, flags: &[&str]) -> Vec<(String, String)> {
    let output = std::process::Command::new(crate::harness::onevcs_binary())
        .args([
            "release",
            "check-adoption",
            "engine",
            "--consumer",
            "service",
            "--json",
        ])
        .args(flags)
        .env("ONEVCS_HOME", world.onevcs_home())
        .env("GIT_CONFIG_GLOBAL", world.gitconfig())
        .stdin(std::process::Stdio::null())
        .output()
        .expect("the onevcs binary runs");
    let verdict: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "`onevcs release check-adoption` printed no verdict ({error}): {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    verdict["refusals"]
        .as_array()
        .unwrap_or_else(|| panic!("a verdict carries refusals: {verdict}"))
        .iter()
        .map(|refusal| {
            (
                refusal["rule"].as_str().expect("a rule id").to_owned(),
                refusal["reason"].as_str().expect("a reason").to_owned(),
            )
        })
        .collect()
}

/// The one refusal onevcs makes of a pair, which every journey here states exactly one.
fn one_rule(world: &World, flags: &[&str], rule: &str) -> String {
    let said = onevcs_says(world, flags);
    assert_eq!(
        said.len(),
        1,
        "onevcs made {} refusals: {said:?}",
        said.len()
    );
    assert_eq!(said[0].0, rule, "onevcs refused by another rule: {said:?}");
    said[0].1.clone()
}

/// The line the engine prints for one of onevcs's refusals.
fn line(node: &str, field: &str, rule: &str, reason: &str) -> String {
    format!("onevcs: node '{node}': `{field}`: dependency 'engine': {rule}: {reason}")
}

fn runs(world: &World) -> Vec<String> {
    let root = world.run_file("", "");
    let mut found: Vec<String> = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    found.sort();
    found
}

/// One load case: a consumer of `engine` breaking one onevcs rule, and its repair.
struct LoadCase {
    /// The rule it breaks, which also names the case.
    rule: &'static str,
    /// Whether `engine` names a default target.
    default_target: bool,
    /// The consumer's fields as refused.
    refused: Value,
    /// The flags that put the same pair to onevcs's own executable.
    flags: &'static [&'static str],
    /// The plan field the refusal names.
    field: &'static str,
    /// The consumer's fields once repaired.
    repaired: Value,
}

/// One plan per rule onevcs still states is refused by `plan check` and by `start` —
/// naming the node, the dependency, `source: "onevcs"`, the rule and onevcs's own reason
/// — and `start` mints no run for it; the same plan with the adoption repaired is
/// accepted and launches.
#[test]
fn a_plan_onevcs_says_can_never_complete_is_refused_at_load_and_loads_once_repaired() {
    let cases = [
        LoadCase {
            rule: "published-no-target",
            default_target: false,
            refused: json!({"adoption": "published"}),
            flags: &["--adoption", "published"],
            field: "adoption",
            repaired: json!({"adoption": "published", "consumes": {"engine": "crate"}}),
        },
        LoadCase {
            rule: "consumes-undeclared-target",
            default_target: true,
            refused: json!({"consumes": {"engine": "wheel"}}),
            flags: &["--consumes", "wheel"],
            field: "consumes",
            repaired: json!({"consumes": {"engine": "crate"}}),
        },
    ];
    for LoadCase {
        rule,
        default_target,
        refused,
        flags,
        field,
        repaired,
    } in cases
    {
        let case = rule;
        let (world, _, _) = world(&format!("adoption-check-load-{case}"), default_target);
        let reason = one_rule(&world, flags, rule);

        let project = world.plan(
            case,
            &plan_of(case, vec![engine(), consumer("consumer", refused)]),
        );
        let checked = world.run(&["plan", "check", &project, "--json"]);
        checked.exited(HAS_REFUSALS);
        let answer: Value = serde_json::from_str(checked.stdout.trim()).expect("one object");
        assert_eq!(answer["accepted"], false, "{case}: {answer}");
        assert_eq!(
            answer["refusals"],
            json!([{
                "source": "onevcs",
                "node": "consumer",
                "field": field,
                "reason": format!("dependency 'engine': {rule}: {reason}"),
            }]),
            "{case}: `plan check` did not carry onevcs's refusal"
        );
        world
            .run(&["plan", "check", &project])
            .exited(HAS_REFUSALS)
            .out_has(&line("consumer", field, rule, &reason));

        let before = runs(&world);
        world
            .run(&["start", &project, "--detach"])
            .exited(REFUSED)
            .err_has(&line("consumer", field, rule, &reason));
        assert_eq!(runs(&world), before, "{case}: a refused start minted a run");

        let fixed = format!("{case}-repaired");
        let project = world.plan(
            &fixed,
            &plan_of(&fixed, vec![engine(), consumer("consumer", repaired)]),
        );
        world
            .run(&["plan", "check", &project])
            .exited(0)
            .out_lacks("onevcs:");
        world
            .run(&["start", &project, "--detach"])
            .exited(0)
            .err_lacks("onevcs");
        world.until("the repaired run to dispatch its producer", |world| {
            !world.events_of(&fixed, "node-dispatched").is_empty()
        });
    }
}

/// A plan breaking several of onevcs's rules is refused once per rule, each in the
/// loader's place: every refusal is reported, and a check registered beside them is
/// reported as not run and is never spawned, since no plan loaded to hand it.
#[test]
fn every_rule_a_plan_breaks_is_reported_and_stops_the_registered_checks() {
    let (world, _, _) = world("adoption-check-several", false);
    let published = one_rule(&world, &["--adoption", "published"], "published-no-target");
    let undeclared = one_rule(
        &world,
        &["--consumes", "wheel"],
        "consumes-undeclared-target",
    );
    let project = world.plan(
        "several",
        &plan_of(
            "several",
            vec![
                engine(),
                consumer("held", json!({"adoption": "published"})),
                consumer("misnamed", json!({"consumes": {"engine": "wheel"}})),
            ],
        ),
    );
    let ran = world.root.join("check-ran");
    let check = posix_script(
        &world.root,
        "never-asked",
        &format!(
            "touch '{}'\nprintf '{{\"refusals\": []}}'",
            shell_path(&ran)
        ),
    );
    let check = check.to_string_lossy().into_owned();

    let checked = world.run(&["plan", "check", &project, "--check", &check, "--json"]);
    checked.exited(HAS_REFUSALS);
    let answer: Value = serde_json::from_str(checked.stdout.trim()).expect("one object");
    assert_eq!(
        answer["refusals"],
        json!([
            {
                "source": "onevcs",
                "node": "held",
                "field": "adoption",
                "reason": format!("dependency 'engine': published-no-target: {published}"),
            },
            {
                "source": "onevcs",
                "node": "misnamed",
                "field": "consumes",
                "reason": format!("dependency 'engine': consumes-undeclared-target: {undeclared}"),
            },
        ]),
        "{answer}"
    );
    let unrunnable = answer["unrunnable"].as_array().expect("a list");
    assert_eq!(unrunnable.len(), 1, "{answer}");
    assert_eq!(unrunnable[0]["check"], json!(check), "{answer}");
    assert_eq!(unrunnable[0]["exit_code"], Value::Null, "{answer}");
    assert!(
        unrunnable[0]["stderr"]
            .as_str()
            .is_some_and(|said| said.contains("did not run")),
        "{answer}"
    );
    assert!(!ran.exists(), "a check ran with no loaded plan to hand it");

    world
        .run(&["start", &project, "--detach"])
        .exited(REFUSED)
        .err_has(&line("held", "adoption", "published-no-target", &published))
        .err_has(&line(
            "misnamed",
            "consumes",
            "consumes-undeclared-target",
            &undeclared,
        ));
}

/// An adoption onevcs cannot answer about refuses nothing, and says why it went
/// unchecked.
///
/// A producer registered nowhere is one such: `plan check` accepts its plan with the
/// notice. (`start` refuses that plan before this check matters, as it always has,
/// because the session-holder interlock cannot read an unregistered repository.) A
/// registered producer whose own `release-targets.toml` does not read is another: that
/// plan launches with the notice, a live edit adding another such consumer is applied
/// with it, and adopting the run raises no surface and drives it on.
#[test]
fn an_adoption_onevcs_cannot_answer_is_a_notice_and_the_plan_launches() {
    let (world, _, engine_repo) = world("adoption-check-unanswered", true);
    let notice_of = |node: &str, dependency: &str| {
        format!(
            "onepipeline: node '{node}': dependency '{dependency}': onevcs could not say \
             whether this release adoption can complete, so it was not checked: the producer"
        )
    };
    let notice = |dependency: &str| notice_of("consumer", dependency);

    let mut producer = lifecycle("elsewhere", &[]);
    producer["repo"] = json!("https://github.com/owner/unregistered.git");
    let project = world.plan(
        "unregistered",
        &plan_of(
            "unregistered",
            vec![producer, lifecycle("consumer", &["elsewhere"])],
        ),
    );
    world
        .run(&["plan", "check", &project])
        .exited(0)
        .err_has(&notice("elsewhere"))
        .err_has("is not a registered repository")
        .out_lacks("onevcs:");

    // The producer's own declaration is not one onevcs can read, so what it releases
    // is unknown — even with a `consumes` it does not otherwise declare.
    std::fs::write(
        engine_repo.checkout.join("release-targets.toml"),
        "this is = not [ toml\n",
    )
    .expect("the declaration is written");
    let project = world.plan(
        "unreadable",
        &plan_of(
            "unreadable",
            vec![
                engine(),
                consumer("consumer", json!({"consumes": {"engine": "wheel"}})),
            ],
        ),
    );
    world
        .run(&["plan", "check", &project])
        .exited(0)
        .err_has(&notice("engine"))
        .out_lacks("onevcs:");
    let run = "unreadable";
    world.script("engine.wait", "hold");
    world
        .run(&["start", &project, "--detach"])
        .exited(0)
        .err_has(&notice("engine"));
    world.until(
        "the run whose adoption went unchecked to dispatch",
        |world| !world.events_of(run, "node-dispatched").is_empty(),
    );

    // The driver judges a live edit, so its notice is in the driver's log.
    let committed = |world: &World| world.events_of(run, "edit-committed").len();
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [{"op": "add",
                "node": consumer("added", json!({"consumes": {"engine": "wheel"}}))}]})
            .to_string(),
        )
        .exited(0)
        .err_lacks("onevcs:");
    world.until("the unchecked add to commit", |world| committed(world) == 1);
    world.until_run_file_holds(run, "driver.log", &notice_of("added", "engine"));

    // Adopting asks of every unsettled node: `consumer`, whose notice so far went only to
    // `start`'s stderr, is now noticed too.
    end_driver(&world, run);
    let adopted = world.run(&["adopt", run, "--detach"]);
    adopted.exited(0);
    let adopt_notice = notice("engine");
    world.until(
        "the adopted run to notice its unchecked consumer",
        |world| {
            adopted.stderr.contains(&adopt_notice)
                || std::fs::read_to_string(world.run_file(run, "driver.log"))
                    .is_ok_and(|log| log.contains(&adopt_notice))
        },
    );
    world.until("the adopted run to re-dispatch its producer", |world| {
        world
            .events_of(run, "node-dispatched")
            .iter()
            .filter(|event| event["labels"]["node"] == "engine")
            .count()
            >= 2
    });
    assert!(
        world
            .events_of(run, "planner-surface-queued")
            .iter()
            .all(|event| event["payload"]["kind"] != "adoption-never-completes"),
        "an adoption onevcs could not answer was surfaced as one that can never complete"
    );
}

/// Every live edit that adds or changes a node's adoption, its `consumes` or an edge —
/// `add`, `retry`, `requeue` and `reparent` — is refused whole where onevcs says the
/// result can never complete, naming the node, the dependency, the rule and onevcs's
/// reason, and applying nothing; the same edit repaired is applied.
///
/// Each edit is asked about the node it changes alone: once the host stops declaring
/// what a running node consumes, an edit to an unrelated node is still applied, and
/// only an edit to that node is refused.
#[test]
fn every_live_edit_onevcs_says_can_never_complete_is_refused_whole_and_applied_repaired() {
    let (world, script, _) = world("adoption-check-edits", false);
    let mut parked = lifecycle("requeued", &["engine"]);
    parked["parked"] = json!(true);
    parked["consumes"] = json!({"engine": "crate"});
    let mut rewired = lifecycle("rewired", &[]);
    rewired["parked"] = json!(true);
    rewired["adoption"] = json!("published");
    let mut rewired_fast = lifecycle("rewired-fast", &[]);
    rewired_fast["parked"] = json!(true);
    world.script("flaky.fail", "1");
    world.script("engine.wait", "hold");
    let run = "edited";
    let path = world.plan(
        run,
        &plan_of(
            run,
            vec![engine(), agent("flaky", &[]), parked, rewired, rewired_fast],
        ),
    );
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the node to retry to fail", |world| {
        world.events_of(run, "node-settled").iter().any(|event| {
            event["labels"]["node"] == "flaky" && event["payload"]["status"] == "failed"
        })
    });

    let published = one_rule(&world, &["--adoption", "published"], "published-no-target");
    let undeclared = one_rule(
        &world,
        &["--consumes", "wheel"],
        "consumes-undeclared-target",
    );
    let reply = |command: Value| {
        world.run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [command]}).to_string(),
        )
    };
    let committed = |world: &World| world.events_of(run, "edit-committed").len();

    // (the edit refused, the line it is refused with, the same edit repaired)
    let edits = [
        (
            json!({"op": "add", "node": consumer("added", json!({"adoption": "published"}))}),
            line("added", "adoption", "published-no-target", &published),
            json!({"op": "add", "node": consumer("added",
                json!({"adoption": "published", "consumes": {"engine": "crate"}}))}),
        ),
        (
            json!({"op": "retry", "id": "flaky", "node": consumer("retried",
                json!({"consumes": {"engine": "wheel"}}))}),
            line(
                "retried",
                "consumes",
                "consumes-undeclared-target",
                &undeclared,
            ),
            json!({"op": "retry", "id": "flaky", "node": consumer("retried",
                json!({"consumes": {"engine": "crate"}}))}),
        ),
        (
            json!({"op": "requeue", "id": "requeued", "amend": {"consumes": {"engine": "wheel"}}}),
            line(
                "requeued",
                "consumes",
                "consumes-undeclared-target",
                &undeclared,
            ),
            json!({"op": "requeue", "id": "requeued", "amend": {"adoption": "published"}}),
        ),
        (
            json!({"op": "reparent", "id": "rewired", "deps": ["engine"]}),
            line("rewired", "adoption", "published-no-target", &published),
            json!({"op": "reparent", "id": "rewired-fast", "deps": ["engine"]}),
        ),
    ];
    for (refused, said, repaired) in edits {
        let before = committed(&world);
        let status = world.run(&["status", run, "--json"]).json();
        reply(refused.clone()).exited(REFUSED).err_has(&said);
        assert_eq!(committed(&world), before, "a refused {refused} committed");
        assert_eq!(
            world.run(&["status", run, "--json"]).json()["nodes"],
            status["nodes"],
            "a refused {refused} changed the run"
        );
        reply(repaired.clone()).exited(0).err_lacks("onevcs:");
        world.until("the repaired edit to commit", |world| {
            committed(world) == before + 1
        });
    }

    // The host's declaration moves under the run: `engine` now releases `wheel`, so the
    // `added` node consuming `crate` can no longer complete.
    world.releases(&releasing(&script, "wheel", false));
    let stranded = one_rule(
        &world,
        &["--consumes", "crate"],
        "consumes-undeclared-target",
    );
    let before = committed(&world);
    reply(json!({"op": "add", "node": agent("unrelated", &[])}))
        .exited(0)
        .err_lacks("onevcs:");
    world.until("the unrelated add to commit", |world| {
        committed(world) == before + 1
    });
    let status = world.run(&["status", run, "--json"]).json();
    reply(json!({"op": "reparent", "id": "added", "deps": ["engine"]}))
        .exited(REFUSED)
        .err_has(&line(
            "added",
            "consumes",
            "consumes-undeclared-target",
            &stranded,
        ));
    assert_eq!(
        committed(&world),
        before + 1,
        "a refused reparent committed"
    );
    assert_eq!(
        world.run(&["status", run, "--json"]).json()["nodes"],
        status["nodes"],
        "a refused reparent changed the run"
    );
}

/// `adopt` refuses nothing: a run whose host configuration moved after launch, so a
/// waiting node's adoption can no longer complete, is adopted and driven on, and one
/// non-blocking surface names the node, the dependency and onevcs's rule and reason. A
/// node the run has finished with — here one skipped behind a failed dependency, whose
/// adoption the same move breaks — is not surfaced.
#[test]
fn adopting_a_run_onevcs_says_can_no_longer_complete_drives_it_on_and_surfaces_the_rule() {
    let (world, script, _) = world("adoption-check-adopt", true);
    world.script("engine.wait", "hold");
    world.script("flaky.fail", "1");
    let mut skipped = consumer("skipped", json!({"adoption": "published"}));
    skipped["deps"] = json!(["engine", "flaky"]);
    world.script("engine.work", "engine did its work once adopted\n");
    let run = "adopted";
    let path = world.plan(
        run,
        &plan_of(
            run,
            vec![
                engine(),
                consumer("consumer", json!({"adoption": "published"})),
                agent("flaky", &[]),
                skipped,
            ],
        ),
    );
    world
        .run(&["start", &path, "--detach"])
        .exited(0)
        .err_lacks("onevcs");
    world.until("the producer to be dispatched", |world| {
        world
            .events_of(run, "node-dispatched")
            .iter()
            .any(|event| event["labels"]["node"] == "engine")
    });
    world.until("the skipped node's dependency to fail", |world| {
        world.events_of(run, "node-settled").iter().any(|event| {
            event["labels"]["node"] == "flaky" && event["payload"]["status"] == "failed"
        })
    });
    let status = world.run(&["status", run, "--json"]).json();
    let skipped_status = status["nodes"]
        .as_array()
        .and_then(|nodes| nodes.iter().find(|node| node["id"] == "skipped"))
        .map(|node| node["status"].clone());
    assert_eq!(
        skipped_status,
        Some(json!("skipped")),
        "the node behind a failed dependency is not finished with: {status}"
    );
    end_driver(&world, run);

    // The host's override moves: `engine` no longer names a default target, so the
    // `published` consumer naming none has nothing to wait for.
    world.releases(&releases(&script, false));
    let reason = one_rule(&world, &["--adoption", "published"], "published-no-target");
    world.run(&["adopt", run, "--detach"]).exited(0);

    world.until("the adopted run to re-dispatch its producer", |world| {
        world
            .events_of(run, "node-dispatched")
            .iter()
            .filter(|event| event["labels"]["node"] == "engine")
            .count()
            >= 2
    });
    let surfaced: Vec<Value> = world
        .events_of(run, "planner-surface-queued")
        .into_iter()
        .filter(|event| event["payload"]["kind"] == "adoption-never-completes")
        .collect();
    // One: the unfinished consumer, and not the skipped node the same move breaks.
    assert_eq!(surfaced.len(), 1, "{surfaced:?}");
    let surface = &surfaced[0]["payload"];
    assert_eq!(surface["blocking"], false, "{surface}");
    let message = surface["message"]
        .as_str()
        .expect("a surface says something");
    for named in [
        "node 'consumer'",
        "dependency 'engine'",
        "(published-no-target)",
        reason.as_str(),
    ] {
        assert!(
            message.contains(named),
            "the surface lacks {named:?}: {message}"
        );
    }
    // Driven on: the adopted driver — not the one ended above — runs the producer's
    // turn to its end and settles it. How its publication then fares is this world's
    // host, which merges no change request, and nothing to do with the adoption.
    world.release("engine.go");
    let adopted = world.events_of(run, "driver-adopted");
    assert_eq!(adopted.len(), 1, "{adopted:?}");
    let driver = adopted[0]["stream"].clone();
    world.until("the adopted driver to settle the producer", |world| {
        world
            .events_of(run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "engine" && event["stream"] == driver)
    });
    assert!(
        world
            .events_of(run, "member-settled")
            .iter()
            .any(|event| event["labels"]["onepipeline.node"] == "engine"
                && event["payload"]["completed"] == true),
        "the adopted run never ran the producer's turn to its end"
    );
}
