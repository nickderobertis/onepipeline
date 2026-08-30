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
    check_in(&world.root, name, body)
}

/// One check written into `directory` — which is the world's own root for every
/// journey but the one that registers a check by a path relative to the
/// directory the verb runs in.
#[cfg(unix)]
fn check_in(directory: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = directory.join(format!("{name}.sh"));
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("the check is written");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("the check is executable");
    path
}

/// One check written into `directory`, in this platform's spelling.
///
/// Windows starts no `#!` script, so what the `--check` flag names here is a
/// batch file and the POSIX body sits beside it: the batch hands it to the
/// `bash` git for Windows installs, which is the same shell every `just` recipe
/// on this platform already runs through, and the check's stdin, stdout, stderr
/// and exit status all reach it through `cmd` unchanged. One body per check
/// rather than two, so a journey states what its check does once and neither
/// spelling can drift from the other.
///
/// Written with **CRLF**, for the reason `harness::write_hook_script` records:
/// `cmd.exe` seeks by byte offset between commands and the arithmetic assumes
/// two bytes end a line, so an LF batch file is executed from the middles of
/// words.
#[cfg(windows)]
fn check_in(directory: &Path, name: &str, body: &str) -> PathBuf {
    let posix = directory.join(format!("{name}.sh"));
    std::fs::write(&posix, format!("#!/bin/sh\n{body}\n")).expect("the check is written");
    let path = directory.join(format!("{name}.bat"));
    std::fs::write(
        &path,
        format!(
            "@echo off\r\n\"{}\" \"{}\"\r\nexit /b %ERRORLEVEL%\r\n",
            bash().display(),
            shell_path(&posix)
        ),
    )
    .expect("the check is written");
    path
}

/// The `bash` this platform's checks are read by, as an absolute path.
///
/// Resolved here rather than left as a bare name in the batch file, so a host
/// without one fails naming what was looked for instead of leaving every check
/// reported as one that could not be run.
#[cfg(windows)]
fn bash() -> PathBuf {
    let on_path = std::env::var_os("PATH").unwrap_or_default();
    let installed = PathBuf::from(
        std::env::var_os("ProgramFiles").unwrap_or_else(|| r"C:\Program Files".into()),
    )
    .join(r"Git\bin");
    std::env::split_paths(&on_path)
        .chain([installed])
        .map(|dir| dir.join("bash.exe"))
        .find(|candidate| candidate.is_file())
        .expect("git for windows installs the bash these checks are written in")
}

/// A path as the shell that reads these bodies spells it.
///
/// The bodies are POSIX on both platforms, and that shell reads `\` as an escape
/// rather than as a separator, so a native path reaches it spelled with the one
/// separator both sides accept.
#[cfg(windows)]
fn shell_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// A path as the shell that reads these bodies spells it.
#[cfg(not(windows))]
fn shell_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// A check that accepts, and records what it was handed.
fn recording_check(world: &World, name: &str, capture: &Path) -> PathBuf {
    check_script(
        world,
        name,
        &format!(
            "cat > '{capture}'\nprintf '%s' \"$ONEPIPELINE_PLAN_CHECK_SCHEMA\" > \
             '{capture}.schema'\nprintf '{{\"refusals\": []}}'",
            capture = shell_path(capture)
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
    // A check that answers without reading its stdin at all is an accept: what
    // this verb offers, a check is free not to take.
    let deaf = check_script(&world, "deaf", "printf '{\"refusals\": []}'");
    world
        .run(&[
            "plan",
            "check",
            &project,
            "--check",
            &as_str(&deaf),
            "--json",
        ])
        .exited(0);

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
                {"node": null, "field": null, "reason": "the plan states no review record"}
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
    // An answer carrying a key this build does not know. Read leniently it would
    // be dropped and the check would read as an accept, which is the false pass
    // this verb exists to stop.
    let inventive = check_script(
        &world,
        "inventive",
        &format!(
            "cat > /dev/null\nprintf '%s' '{}'",
            json!({"refusals": [], "verdict": "fine"})
        ),
    );
    // An answer omitting a key the contract states as always present.
    let terse = check_script(
        &world,
        "terse",
        &format!(
            "cat > /dev/null\nprintf '%s' '{}'",
            json!({"refusals": [{"reason": "the bar omits the appendix"}]})
        ),
    );
    // A check whose diagnosis runs past the bound. What it said is truncated and
    // *said to be*, because a sentence cut off at a byte count reads as the whole
    // of it.
    let voluble = check_script(
        &world,
        "voluble",
        "cat > /dev/null\nyes 'the check broke and would not stop saying so'          | head -c 1200000 >&2\nexit 5",
    );
    // An answer past the bound this build reads. A check is somebody else's
    // program, and one that answers with a megabyte is one nothing can act on.
    let endless = check_script(
        &world,
        "endless",
        "cat > /dev/null\nprintf '{\"refusals\": ['\nyes '{\"node\": null, \"field\": null,          \"reason\": \"padding\"},' | head -c 1200000",
    );
    // An answer shaped right and saying nothing: `reason` is the whole of what a
    // refusal is, so a blank one is an answer this build cannot read rather than
    // a refusal with no words.
    let wordless = check_script(
        &world,
        "wordless",
        &format!(
            "cat > /dev/null\nprintf '%s' '{}'",
            json!({"refusals": [{"node": null, "field": null, "reason": "   "}]})
        ),
    );
    // Written, and deliberately not made a program: no executable bit where a
    // host asks for one, and an extension no host starts a file by where the
    // extension is what decides.
    let unopenable = world.root.join("unopenable.sh");
    std::fs::write(&unopenable, "#!/bin/sh\nprintf '{\"refusals\": []}'\n")
        .expect("the check is written");
    // A check the host killed, *after* it wrote a well-formed accept: it died by
    // a signal, so it exited with no status at all and never said it had
    // finished. A well-formed answer on the stdout of a process that was killed
    // is not an accept — a check that ran answers with exit status 0 — and the
    // report carries a null status rather than a number nothing produced.
    #[cfg(unix)]
    let signalled = check_script(
        &world,
        "signalled",
        "cat > /dev/null\nprintf '{\"refusals\": []}'\necho 'killed before it finished' >&2\n\
         kill -TERM $$",
    );

    // The signalled case is the only one added to this list, so on a host
    // without it the list is already whole.
    #[cfg_attr(
        not(unix),
        allow(unused_mut, reason = "nothing is pushed without signals")
    )]
    let mut cases = vec![
        (&failing, true, "the check broke"),
        (&babbling, true, "not-json"),
        (&wordless, true, "blank"),
        (&inventive, true, "verdict"),
        (&endless, true, "bytes this build reads"),
        (&voluble, true, "truncated at the"),
        (&terse, true, "refusals[].node"),
        (&unopenable, false, "cannot be run"),
    ];
    #[cfg(unix)]
    cases.push((&signalled, false, "killed before it finished"));

    for (check, exit_named, said) in cases {
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
        // The human diagnosis names the path and what it said, on stderr —
        // which is where this binary's diagnoses go, and it leaves stdout to
        // the answer about the plan.
        world
            .run(&["plan", "check", &project, "--check", &as_str(check)])
            .exited(REFUSED)
            .err_has(&as_str(check))
            .err_has("could not be run");
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

/// A relative `--check` is resolved against the directory the verb ran in, and
/// the check itself runs in that same directory.
///
/// Both halves are asserted by **relative names alone**: the flag names the
/// check by one, and the check writes and reads by ones. Nothing here spells an
/// absolute path, so neither half can pass against a directory that merely
/// happens to hold the same files — which is the whole of what a consumer
/// registering a check beside its plan depends on.
#[test]
fn a_check_path_is_resolved_against_the_directory_the_verb_ran_from() {
    let world = World::new("plancheck-cwd");
    let project = world.plan(
        "sound",
        &plan_of("sound", vec![crate::harness::agent("build", &[])]),
    );
    let here = world.root.join("checks");
    std::fs::create_dir_all(&here).expect("a directory for the check");
    // The file a check reads beside its plan, and which only this directory has.
    std::fs::write(here.join("beside.txt"), "beside the plan").expect("the file is written");
    let check = check_in(
        &here,
        "relative",
        "cat > 'handed.json'\ncat 'beside.txt' > 'beside.seen'\nprintf '{\"refusals\": []}'",
    );
    let named = format!(
        "./{}",
        check
            .file_name()
            .expect("the check has a name")
            .to_string_lossy()
    );

    world
        .run_from(&here, &["plan", "check", &project, "--check", &named])
        .exited(0);
    let handed = here.join("handed.json");
    assert!(handed.is_file(), "the check beside the plan never ran");
    let document: Value = serde_json::from_str(
        &std::fs::read_to_string(&handed).expect("the check wrote what it was handed"),
    )
    .expect("the document on the check's stdin is JSON");
    assert!(document["schema_version"].is_i64(), "{document}");
    // And it ran in that same directory, which is what a check reading a file
    // beside the plan depends on.
    assert_eq!(
        std::fs::read_to_string(here.join("beside.seen"))
            .expect("the check read the file beside the plan"),
        "beside the plan"
    );
}

/// A project this build cannot read at all is exit 2, and `--json` still prints
/// exactly one object.
///
/// A consumer parses this verb's stdout without first asking which failure it
/// met, so the object is unconditional and the diagnosis is on stderr — which is
/// where every other refusal this binary makes goes.
#[test]
fn a_project_that_cannot_be_read_is_exit_two_and_still_answers_with_one_object() {
    let world = World::new("plancheck-unreadable");
    // A source this world has and a project it does not.
    let missing = format!("{STORE_SOURCE}:nothing-here");
    let run = world.run(&["plan", "check", &missing, "--json"]);
    run.exited(REFUSED);
    let answered = answer(&run);
    assert_shape(&answered);
    assert_eq!(answered["project"], json!(missing), "{answered}");
    assert_eq!(answered["accepted"], json!(false), "{answered}");
    assert!(
        !run.stderr.trim().is_empty(),
        "nothing said what went wrong"
    );

    // And an id that is not qualified at all, which this build cannot even parse.
    let run = world.run(&["plan", "check", "unqualified", "--json"]);
    run.exited(REFUSED);
    assert_shape(&answer(&run));
    run.err_has("qualified onetaskgraph id");
}

/// A project this build cannot read leaves its registered checks unspawned.
///
/// There is no loaded plan to hand them, so the same thing happens to them as a
/// loader refusal does — and this answer says so the only way it can: nothing
/// accepted the project, and the two lists a consumer reads a *verdict* out of
/// are both empty. An empty `unrunnable` beside exit 2 is what tells the two
/// exit-2 causes apart: a check that could not be run names itself there, and
/// this one, where nothing got as far as a check, names nobody.
#[test]
fn an_unreadable_project_runs_no_registered_check_and_accepts_nothing() {
    let world = World::new("plancheck-unreadable-checks");
    let first = world.root.join("first.json");
    let second = world.root.join("second.json");
    let one = recording_check(&world, "one", &first);
    let two = recording_check(&world, "two", &second);

    let missing = format!("{STORE_SOURCE}:nothing-here");
    let run = world.run(&[
        "plan",
        "check",
        &missing,
        "--check",
        &as_str(&one),
        "--check",
        &as_str(&two),
        "--json",
    ]);
    run.exited(REFUSED);
    let answered = answer(&run);
    assert_shape(&answered);
    assert_eq!(answered["project"], json!(missing), "{answered}");
    assert_eq!(answered["accepted"], json!(false), "{answered}");
    // Neither was spawned: neither wrote the file it writes the moment it is.
    assert!(!first.exists() && !second.exists(), "{answered}");
    assert!(refusals(&answered).is_empty(), "{answered}");
    assert!(unrunnable(&answered).is_empty(), "{answered}");
    // The diagnosis is on stderr, where every other refusal this binary makes
    // goes, and it is about the project rather than about either check.
    assert!(
        !run.stderr.trim().is_empty(),
        "nothing said what went wrong"
    );
    assert!(
        !run.stderr.contains(&as_str(&one)) && !run.stderr.contains(&as_str(&two)),
        "a check nothing spawned was reported: {}",
        run.stderr
    );
}

/// A refusing check beside one that could not be run: exit 2 wins, and both are
/// still in the answer.
///
/// The two are different facts and a status can only carry one, so the one that
/// carries less information wins — a refusal is a thing the consumer knows, and
/// a check that could not be run is a thing nobody knows.
#[test]
fn a_refusal_beside_an_unrunnable_check_exits_two_and_reports_both() {
    let world = World::new("plancheck-both");
    let project = world.plan(
        "sound",
        &plan_of("sound", vec![crate::harness::agent("build", &[])]),
    );
    let refuses = refusing_check(
        &world,
        "refuses",
        "build",
        "task",
        "the appendix is missing",
    );
    let breaks = check_script(
        &world,
        "breaks",
        "cat > /dev/null\necho 'the check broke' >&2\nexit 4",
    );

    let run = world.run(&[
        "plan",
        "check",
        &project,
        "--check",
        &as_str(&refuses),
        "--check",
        &as_str(&breaks),
        "--json",
    ]);
    run.exited(REFUSED);
    let answered = answer(&run);
    assert_shape(&answered);
    assert_eq!(answered["accepted"], json!(false), "{answered}");
    let refused = refusals(&answered);
    assert_eq!(refused.len(), 1, "{answered}");
    assert_eq!(
        refused[0]["reason"],
        json!("the appendix is missing"),
        "{answered}"
    );
    let could_not = unrunnable(&answered);
    assert_eq!(could_not.len(), 1, "{answered}");
    assert_eq!(could_not[0]["check"], json!(as_str(&breaks)), "{answered}");
    assert_eq!(could_not[0]["exit_code"], json!(4), "{answered}");
}
