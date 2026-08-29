//! `onepipeline plan check`: the engine's own loader, and whatever checks the
//! consumer registered, behind one entry point.
//!
//! Every journey here drives the compiled binary against a real `onetaskgraph`
//! store, exactly as a launch does, and registers **real executables** as
//! checks — which is the whole claim: what refuses a plan here is the loader
//! `onepipeline start` runs, and what a consumer's check is handed is a document
//! this suite reads back out of the file that check wrote.

// llmlint: ignore-file[e2e_not_mocked] nothing is substituted here at all: the binary is
// the compiled one, the store is the real `onetaskgraph` reading a folder of Markdown, and
// each registered check is a shell script this journey wrote and the binary spawned.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::harness::{plan_of, World, REFUSED, STORE_SOURCE};

/// What a refusal from either source exits with.
const HAS_REFUSALS: i32 = 1;

/// Write one executable check into this world and answer with its path.
///
/// A real program, spawned by the binary under test: a journey that handed the
/// verb a closure would be proving this suite rather than the seam.
fn check_script(world: &World, name: &str, body: &str) -> PathBuf {
    let path = world.root.join(format!("{name}.sh"));
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("the check is written");
    make_executable(&path);
    path
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .expect("the check is executable");
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

/// A check that accepts, and records what it was handed.
fn recording_check(world: &World, name: &str, capture: &Path) -> PathBuf {
    check_script(
        world,
        name,
        &format!(
            "cat > '{}'\nprintf '%s' \"$ONEPIPELINE_PLAN_CHECK_SCHEMA\" > '{}.schema'\n\
             printf '%s' \"$PWD\" > '{}.pwd'\nprintf '{{\"refusals\": []}}'",
            capture.display(),
            capture.display(),
            capture.display()
        ),
    )
}

/// A check that refuses once, naming a node and a field.
fn refusing_check(world: &World, name: &str, node: &str, field: &str, reason: &str) -> PathBuf {
    check_script(
        world,
        name,
        &format!(
            "cat > /dev/null\nprintf '%s' '{}'",
            json!({"refusals": [{"node": node, "field": field, "reason": reason}]})
        ),
    )
}

fn as_str(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The one JSON object `--json` prints.
fn answer(run: &crate::harness::Run) -> Value {
    serde_json::from_str(run.stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "`onepipeline {}` did not print one JSON object ({error}):\n{}",
            run.args, run.stdout
        )
    })
}

/// Every key the contract says the answer always carries, whatever it holds.
fn assert_shape(answer: &Value) {
    assert!(answer["project"].is_string(), "{answer}");
    assert!(answer["accepted"].is_boolean(), "{answer}");
    assert!(answer["refusals"].is_array(), "{answer}");
    assert!(answer["unrunnable"].is_array(), "{answer}");
}

fn refusals(answer: &Value) -> Vec<Value> {
    answer["refusals"]
        .as_array()
        .unwrap_or_else(|| panic!("the answer has no refusals list: {answer}"))
        .clone()
}

fn unrunnable(answer: &Value) -> Vec<Value> {
    answer["unrunnable"]
        .as_array()
        .unwrap_or_else(|| panic!("the answer has no unrunnable list: {answer}"))
        .clone()
}

/// The three structural refusals a consumer's own re-implementation passed.
///
/// Each is a plan `onepipeline start` refuses before it dispatches anything, so
/// each is one `plan check` has to make: the whole point of the verb is that the
/// two are one implementation. Each answer names the node it is about and the
/// field that has to change, because a consumer acts on the field rather than on
/// the sentence.
#[test]
fn the_three_structural_refusals_name_the_node_and_the_field() {
    let world = World::new("plancheck-structural");
    let cases: &[(&str, Value, &str, &str)] = &[
        // A node naming its repository in two places.
        (
            "tworepos",
            json!({"schema_version": 3, "tasks": [
                {"id": "a", "persona": "e", "task": "t", "title": "feat: x",
                 "repo": "github.com/owner/service",
                 "onepipeline-repo": "/var/checkouts/service"}]}),
            "repo",
            "names a repository in both `repositories` and `onepipeline.repo`",
        ),
        // `onepipeline.deps` used for an edge between two nodes of this plan.
        (
            "recordeddep",
            json!({"schema_version": 3, "tasks": [
                {"id": "a", "persona": "e", "task": "t"},
                {"id": "b", "persona": "e", "task": "t", "onepipeline-deps": ["a"]}]}),
            "deps",
            "is a dependency edge between the two tasks",
        ),
        // A stepped node that also carries a task.
        (
            "stepsandtask",
            json!({"schema_version": 3, "tasks": [
                {"id": "a", "repo": "o/r", "title": "feat: x", "task": "t",
                 "steps": [{"id": "s", "persona": "e", "task": "t"}]}]}),
            "task",
            "a node with steps takes its persona, task, and turn budget from them",
        ),
    ];

    for (name, plan, field, sentence) in cases {
        // The reserved keys a journey states as store keys rather than as plan
        // fields, spelled with a dash and swapped to the dot the store holds —
        // the same two-step `plan.rs`'s refusal journey uses, and for the same
        // reason.
        let stated = serde_json::to_string(plan)
            .expect("a plan serialises")
            .replace("onepipeline-", "onepipeline.");
        let plan: Value = serde_json::from_str(&stated).expect("it re-reads");
        let project = world.plan(name, &plan);

        let checked = world.run(&["plan", "check", &project, "--json"]);
        checked.exited(HAS_REFUSALS);
        let answered = answer(&checked);
        assert_shape(&answered);
        assert_eq!(answered["accepted"], json!(false), "{answered}");
        assert_eq!(answered["project"], json!(project), "{answered}");
        let refused = refusals(&answered);
        assert_eq!(refused.len(), 1, "{answered}");
        assert_eq!(refused[0]["source"], json!("engine"), "{answered}");
        assert_eq!(refused[0]["field"], json!(*field), "{answered}");
        assert!(
            refused[0]["node"].is_string(),
            "the refusal names no node: {answered}"
        );
        assert!(
            refused[0]["reason"]
                .as_str()
                .expect("a reason")
                .contains(sentence),
            "{answered}"
        );

        // The same plan, refused by the launch in the same words — which is what
        // makes one implementation of the loader true rather than claimed.
        world
            .run(&["start", &project])
            .exited(REFUSED)
            .err_has(sentence);
        // And the human output says the same thing, naming the source.
        world
            .run(&["plan", "check", &project])
            .exited(HAS_REFUSALS)
            .out_has("engine:")
            .out_has(sentence);
    }
}

/// A plan the loader accepts, with no check and with one that accepts.
#[test]
fn a_plan_the_loader_takes_and_no_check_refuses_exits_zero() {
    let world = World::new("plancheck-accepted");
    let project = world.plan(
        "sound",
        &plan_of("sound", vec![crate::harness::agent("build", &[])]),
    );
    world.run(&["plan", "check", &project]).exited(0);

    let capture = world.root.join("handed.json");
    let check = recording_check(&world, "accepts", &capture);
    let run = world.run(&[
        "plan",
        "check",
        &project,
        "--check",
        &as_str(&check),
        "--json",
    ]);
    run.exited(0);
    let answered = answer(&run);
    assert_shape(&answered);
    assert_eq!(answered["accepted"], json!(true), "{answered}");
    assert_eq!(refusals(&answered), Vec::<Value>::new(), "{answered}");
    assert_eq!(unrunnable(&answered), Vec::<Value>::new(), "{answered}");
    // Checking a plan starts nothing.
    assert!(
        !world.runs.join("sound").exists(),
        "`plan check` left a run directory behind"
    );
}

/// What a registered check is actually handed.
///
/// The document is read back out of the file the check wrote, so what this
/// asserts on is what crossed the seam. The task carries a metadata key outside
/// this crate's reserved namespace — a consumer's own review record is one — and
/// the check has to be able to read it, because a checker that cannot see the
/// keys its rules are about is a checker that has to re-read the store itself.
#[test]
fn a_registered_check_is_handed_the_loaded_plan_and_each_tasks_own_metadata() {
    let world = World::new("plancheck-handed");
    // Written as the store's own documents, because the key this journey is
    // about is one no plan field answers to.
    world.write_store_item(
        "projects/carried.md",
        "---\ntitle: \"carried\"\nmetadata: {\"onepipeline.schema_version\": 3, \
         \"onepipeline.concurrency\": 2, \"onepipeline.goal\": {\"text\": \"ship it\"}}\n---\n\n",
    );
    world.write_store_item(
        "tasks/carried/000-build.md",
        "---\ntitle: \"feat: build it\"\nproject: \"carried\"\nrepositories: \
         [\"github.com/owner/service\"]\nmetadata: {\"onepipeline.id\": \"build\", \
         \"onepipeline.persona\": \"engineer\", \"review.record\": \"REV-7\"}\n---\n\nDo the work.\n",
    );
    world.write_store_item(
        "tasks/carried/001-verify.md",
        "---\ntitle: \"verify\"\nproject: \"carried\"\ndepends_on: [\"carried/000-build\"]\n\
         metadata: {\"onepipeline.id\": \"verify\", \"onepipeline.persona\": \"engineer\"}\n\
         ---\n\nCheck the work.\n",
    );

    let capture = world.root.join("handed.json");
    let check = recording_check(&world, "records", &capture);
    let project = format!("{STORE_SOURCE}:carried");
    world
        .run(&["plan", "check", &project, "--check", &as_str(&check)])
        .exited(0);

    let handed: Value = serde_json::from_str(
        &std::fs::read_to_string(&capture).expect("the check wrote what it was handed"),
    )
    .expect("the document on the check's stdin is JSON");
    assert_eq!(handed["schema_version"], json!(3), "{handed}");
    assert_eq!(handed["concurrency"], json!(2), "{handed}");
    assert_eq!(handed["name"], json!("carried"), "{handed}");
    assert_eq!(handed["goal"]["text"], json!("ship it"), "{handed}");
    let tasks = handed["tasks"].as_array().expect("the tasks").clone();
    assert_eq!(tasks.len(), 2, "one entry per task: {handed}");

    let build = tasks
        .iter()
        .find(|task| task["id"] == json!("build"))
        .unwrap_or_else(|| panic!("{handed}"));
    // The engine's own loaded node: the repository identity taken off the
    // store's `repositories`, the persona off its reserved key, and the title
    // and prose off the task itself.
    assert_eq!(build["repo"], json!("github.com/owner/service"), "{handed}");
    assert_eq!(build["persona"], json!("engineer"), "{handed}");
    assert_eq!(build["title"], json!("feat: build it"), "{handed}");
    assert_eq!(build["task"], json!("Do the work."), "{handed}");
    // The store's own metadata map for that task, verbatim — the reserved keys
    // and the one outside the namespace alike.
    assert_eq!(
        build["metadata"]["review.record"],
        json!("REV-7"),
        "{handed}"
    );
    assert_eq!(
        build["metadata"]["onepipeline.id"],
        json!("build"),
        "{handed}"
    );

    // And the dependency edge, resolved to the node id the far task carries
    // rather than left as the store's own.
    let verify = tasks
        .iter()
        .find(|task| task["id"] == json!("verify"))
        .unwrap_or_else(|| panic!("{handed}"));
    assert_eq!(verify["deps"], json!(["build"]), "{handed}");

    // The environment says which document shape is on the check's stdin.
    assert_eq!(
        std::fs::read_to_string(format!("{}.schema", capture.display()))
            .expect("the check recorded its environment"),
        "1"
    );
}

/// A check's refusals reach the output whole, and a null stays a key.
#[test]
fn a_check_that_refuses_makes_the_verb_exit_one_and_carries_every_field() {
    let world = World::new("plancheck-refusing");
    let project = world.plan(
        "sound",
        &plan_of("sound", vec![crate::harness::agent("build", &[])]),
    );
    // Two refusals: one about a node and a field, one about neither — whose
    // `node` and `field` are still keys, because a consumer reads a key that is
    // there and null rather than branching on its absence.
    let check = check_script(
        &world,
        "two",
        &format!(
            "cat > /dev/null\nprintf '%s' '{}'",
            json!({"refusals": [
                {"node": "build", "field": "task", "reason": "the bar omits the appendix"},
                {"reason": "the plan states no review record"}
            ]})
        ),
    );
    let run = world.run(&[
        "plan",
        "check",
        &project,
        "--check",
        &as_str(&check),
        "--json",
    ]);
    run.exited(HAS_REFUSALS);
    let answered = answer(&run);
    assert_shape(&answered);
    assert_eq!(answered["accepted"], json!(false), "{answered}");
    let refused = refusals(&answered);
    assert_eq!(refused.len(), 2, "{answered}");
    assert_eq!(refused[0]["source"], json!(as_str(&check)), "{answered}");
    assert_eq!(refused[0]["node"], json!("build"), "{answered}");
    assert_eq!(refused[0]["field"], json!("task"), "{answered}");
    assert_eq!(
        refused[0]["reason"],
        json!("the bar omits the appendix"),
        "{answered}"
    );
    for key in ["node", "field"] {
        assert!(
            refused[1].get(key).is_some_and(Value::is_null),
            "`{key}` is missing rather than null: {answered}"
        );
    }

    // The same two on the human output, each naming its source.
    world
        .run(&["plan", "check", &project, "--check", &as_str(&check)])
        .exited(HAS_REFUSALS)
        .out_has("the bar omits the appendix")
        .out_has("the plan states no review record")
        .out_has(&as_str(&check));
}

/// The three ways a check can fail to run, and none of them is an accept.
#[test]
fn a_check_that_could_not_be_run_is_reported_as_such_and_exits_two() {
    let world = World::new("plancheck-unrunnable");
    let project = world.plan(
        "sound",
        &plan_of("sound", vec![crate::harness::agent("build", &[])]),
    );

    let failing = check_script(
        &world,
        "failing",
        "cat > /dev/null\necho 'the check broke' >&2\nexit 3",
    );
    let babbling = check_script(&world, "babbling", "cat > /dev/null\necho not-json");
    // An answer shaped right and saying nothing: `reason` is the whole of what a
    // refusal is, so a blank one is an answer this build cannot read rather than
    // a refusal with no words.
    let wordless = check_script(
        &world,
        "wordless",
        &format!(
            "cat > /dev/null\nprintf '%s' '{}'",
            json!({"refusals": [{"reason": "   "}]})
        ),
    );
    // Written, and deliberately not made executable.
    let unopenable = world.root.join("unopenable.sh");
    std::fs::write(&unopenable, "#!/bin/sh\nprintf '{\"refusals\": []}'\n")
        .expect("the check is written");

    for (check, exit_named, said) in [
        (&failing, true, "the check broke"),
        (&babbling, true, "not-json"),
        (&wordless, true, "carrying no reason"),
        (&unopenable, false, "cannot be run"),
    ] {
        let run = world.run(&[
            "plan",
            "check",
            &project,
            "--check",
            &as_str(check),
            "--json",
        ]);
        // Never 0, and never 1: what the check would have said is unknown.
        run.exited(REFUSED);
        let answered = answer(&run);
        assert_shape(&answered);
        assert_eq!(answered["accepted"], json!(false), "{answered}");
        assert_eq!(refusals(&answered), Vec::<Value>::new(), "{answered}");
        let could_not = unrunnable(&answered);
        assert_eq!(could_not.len(), 1, "{answered}");
        assert_eq!(could_not[0]["check"], json!(as_str(check)), "{answered}");
        assert!(
            could_not[0]["stderr"]
                .as_str()
                .expect("a stderr")
                .contains(said),
            "{answered}"
        );
        if exit_named {
            assert!(
                could_not[0]["exit_code"].is_number(),
                "a check that exited names its status: {answered}"
            );
        } else {
            assert!(
                could_not[0].get("exit_code").is_some_and(Value::is_null),
                "a check no process ran for still carries the key: {answered}"
            );
        }
        // The human output names the path and what it said.
        world
            .run(&["plan", "check", &project, "--check", &as_str(check)])
            .exited(REFUSED)
            .out_has(&as_str(check))
            .out_has("could not be run");
    }
}

/// A loader refusal leaves no plan to hand a check, so no check runs.
#[test]
fn a_loader_refusal_stops_the_registered_checks_and_neither_reads_as_an_accept() {
    let world = World::new("plancheck-shortcircuit");
    let project = world.plan(
        "cyclic",
        &json!({"schema_version": 3, "name": "cyclic", "tasks": [
            {"id": "a", "persona": "e", "task": "t", "deps": ["b"]},
            {"id": "b", "persona": "e", "task": "t", "deps": ["a"]}]}),
    );
    let first = world.root.join("first.json");
    let second = world.root.join("second.json");
    let one = recording_check(&world, "one", &first);
    let two = recording_check(&world, "two", &second);

    let run = world.run(&[
        "plan",
        "check",
        &project,
        "--check",
        &as_str(&one),
        "--check",
        &as_str(&two),
        "--json",
    ]);
    let answered = answer(&run);
    assert_shape(&answered);
    assert_eq!(answered["accepted"], json!(false), "{answered}");
    // Neither ran: neither wrote the file it writes the moment it is spawned.
    assert!(!first.exists() && !second.exists(), "{answered}");
    // And neither is reported as accepting: each is a check that did not run.
    let could_not = unrunnable(&answered);
    assert_eq!(could_not.len(), 2, "{answered}");
    for (report, check) in could_not.iter().zip([&one, &two]) {
        assert_eq!(report["check"], json!(as_str(check)), "{answered}");
        assert!(
            report["stderr"]
                .as_str()
                .expect("a stderr")
                .contains("not run"),
            "{answered}"
        );
    }
    // The only refusal is the loader's own, and that is what the status reports.
    let refused = refusals(&answered);
    assert_eq!(refused.len(), 1, "{answered}");
    assert_eq!(refused[0]["source"], json!("engine"), "{answered}");
    run.exited(HAS_REFUSALS);
}

/// Registered checks answer in the order their flags were given.
#[test]
fn check_refusals_follow_the_order_of_their_flags() {
    let world = World::new("plancheck-order");
    let project = world.plan(
        "sound",
        &plan_of("sound", vec![crate::harness::agent("build", &[])]),
    );
    let appendix = refusing_check(
        &world,
        "appendix",
        "build",
        "task",
        "the appendix is missing",
    );
    let record = refusing_check(
        &world,
        "record",
        "build",
        "context",
        "the review record is absent",
    );

    let ordered = |first: &Path, second: &Path| {
        let run = world.run(&[
            "plan",
            "check",
            &project,
            "--check",
            &as_str(first),
            "--check",
            &as_str(second),
            "--json",
        ]);
        run.exited(HAS_REFUSALS);
        let answered = answer(&run);
        let sources: Vec<String> = refusals(&answered)
            .iter()
            .map(|refusal| refusal["source"].as_str().expect("a source").to_owned())
            .collect();
        assert_eq!(sources, vec![as_str(first), as_str(second)], "{answered}");
        // And the human output prints them in that same order.
        let printed = world
            .run(&[
                "plan",
                "check",
                &project,
                "--check",
                &as_str(first),
                "--check",
                &as_str(second),
            ])
            .stdout
            .clone();
        let at = |needle: &str| {
            printed
                .find(needle)
                .unwrap_or_else(|| panic!("{needle:?} is missing from:\n{printed}"))
        };
        assert!(
            at(&as_str(first)) < at(&as_str(second)),
            "the human output reordered the checks:\n{printed}"
        );
    };
    ordered(&appendix, &record);
    // The other way round, so the assertion cannot pass on the paths' own order.
    ordered(&record, &appendix);
}

/// A relative `--check` is resolved against the directory the verb ran in.
#[test]
fn a_check_path_is_resolved_against_the_directory_the_verb_ran_from() {
    let world = World::new("plancheck-cwd");
    let project = world.plan(
        "sound",
        &plan_of("sound", vec![crate::harness::agent("build", &[])]),
    );
    let here = world.root.join("checks");
    std::fs::create_dir_all(&here).expect("a directory for the check");
    let capture = here.join("handed.json");
    let check = check_script(
        &world,
        "relative",
        &format!(
            "cat > '{}'\nprintf '%s' \"$PWD\" > '{}.pwd'\nprintf '{{\"refusals\": []}}'",
            capture.display(),
            capture.display()
        ),
    );
    std::fs::rename(&check, here.join("relative.sh")).expect("the check moves beside the plan");

    world
        .run_from(
            &here,
            &["plan", "check", &project, "--check", "./relative.sh"],
        )
        .exited(0);
    assert!(capture.exists(), "the check beside the plan never ran");
    // And it ran in that same directory, which is what a check reading a file
    // beside the plan depends on.
    assert_eq!(
        std::fs::read_to_string(format!("{}.pwd", capture.display()))
            .expect("the check recorded its directory"),
        crate::harness::resolved(&here).to_string_lossy()
    );
}
