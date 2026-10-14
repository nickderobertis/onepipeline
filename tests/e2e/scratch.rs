//! `ONEPIPELINE_NODE_SCRATCH_DIR`: the directory each node dispatch is given.
//!
//! The promise these journeys hold is in the module documentation of
//! [`onepipeline::executor`] and recorded as divergence 48: an absolute path to a
//! directory that exists and is writable before the dispatch's first turn, unique
//! to that dispatch, and not removed while it runs. The spelling of the path is
//! not promised, so nothing here asserts one.

// llmlint: ignore-file[e2e_not_mocked] the dispatched agent is what reads this variable,
// and the double is what stands in for one: it takes the value out of its own environment
// exactly as an agent would. `harness.rs` carries the same suppression and the rationale.

use crate::harness::{agent, lifecycle, plan_of, World};

/// The scratch directory each of a node's dispatches was given, in dispatch
/// order, read out of the run's own store.
///
/// Every dispatch publishes the value it took out of its environment on the turn
/// it opens, so this is what the dispatches themselves acted on rather than
/// anything a double reported back on the side.
fn given(world: &World, run: &str, node: &str) -> Vec<std::path::PathBuf> {
    world
        .events_of(run, "turn-activity")
        .into_iter()
        .filter(|event| event["labels"]["onepipeline.node"] == node)
        .map(|event| {
            let named = event["payload"]["scratch_dir"].as_str().unwrap_or_else(|| {
                panic!("a dispatch's environment carried no scratch directory: {event}")
            });
            std::path::PathBuf::from(named)
        })
        .collect()
}

/// Assert of one directory everything the promise says about it, except the one
/// thing it does not say: what it is called.
fn kept(world: &World, at: &std::path::Path) {
    assert!(at.is_absolute(), "{} is not an absolute path", at.display());
    assert!(
        at.is_dir(),
        "{} is not a directory that exists\n{}",
        at.display(),
        world.dump()
    );
    // Still holding what its dispatch wrote into it, which is both halves at
    // once: it was writable while that dispatch ran, and nothing took it away
    // afterwards.
    let mut held = std::fs::read_dir(at)
        .unwrap_or_else(|error| panic!("{}: {error}", at.display()))
        .flatten();
    assert!(
        held.next().is_some(),
        "{} no longer holds what its dispatch wrote into it",
        at.display()
    );
}

/// The asker each of a node's dispatches was given, in dispatch order, read out
/// of the run's own store.
///
/// The same evidence as [`given`] and for the same reason: what a journey states
/// is what the dispatch itself took out of its environment, rather than anything
/// a double reported back on the side.
// llmlint: ignore-block[tests_mirror_real_usage] the run's journal is a product
// surface rather than a shortcut around one — it is what `onepipeline` writes for an
// operator to read and what this suite states every dispatch-side promise off, `given`
// three lines above included. What is being asserted is a property of the *dispatch*,
// so the evidence has to be the value that dispatch itself acted on; the only route
// through a user-facing command would be a surface the dispatch raised, and the verb
// that raises one deliberately names no asker.
fn asked_as(world: &World, run: &str, node: &str) -> Vec<String> {
    world
        .events_of(run, "turn-activity")
        .into_iter()
        .filter(|event| event["labels"]["onepipeline.node"] == node)
        .map(|event| {
            event["payload"]["asker"]
                .as_str()
                .unwrap_or_else(|| panic!("a dispatch's environment carried no asker: {event}"))
                .to_string()
        })
        .collect()
} // llmlint: ignore-end[tests_mirror_real_usage]

fn settle(world: &World, name: &str, nodes: Vec<serde_json::Value>) -> String {
    let path = world.plan(name, &plan_of(name, nodes));
    world.run(&["start", &path, "--attach"]).settled();
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });
    name.to_string()
}

/// A dispatch is handed an absolute path to a directory of its own that already
/// exists and can be written to.
///
/// A dispatch that has nowhere of its own invents one, and what it invents
/// collides with what every other dispatch on the host invents. Read off the
/// run's store rather than off the double's files: the promise is a property of
/// the *dispatch*, so the evidence is the value the dispatch itself took out of
/// its environment and published, and the directory that value names.
#[test]
fn a_dispatch_is_given_an_absolute_writable_directory_of_its_own() {
    let world = World::new("scratch-given");
    let run = settle(&world, "given", vec![agent("build", &[])]);

    let named = given(&world, &run, "build");
    assert_eq!(named.len(), 1, "{named:?}\n{}", world.dump());
    assert!(!named[0].exists());
    assert!(!world.run_file(&run, "scratch").exists());
    let removed = world.events_of(&run, "scratch-removed");
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0]["payload"]["trigger"], "success");
    assert_eq!(removed[0]["payload"]["dirs"], 1);
    assert!(removed[0]["payload"]["bytes"].as_u64().unwrap() > 0);
}

/// Every dispatch gets its own directory, kept at the failed ending and removed
/// when the manager settles the node done.
///
/// The requeue is the case the promise is *for*: a node dispatched again is the
/// one set of dispatches that agree on every name a path could be derived from —
/// same run, same node, same step — so a scratch directory keyed on any of them
/// would hand each attempt the last one's half-written files.
///
/// The requeue here is a real one: the host reports this node's checks red and
/// keeps reporting them red, so its publication fails in a way that leaves the
/// work on its branch and the whole node is dispatched again, to its budget.
#[test]
fn every_dispatch_of_one_node_is_given_its_own_directory_kept_until_success() {
    let world = World::new("scratch-requeued");
    // `change-auto` is the policy that watches the host's own checks to their
    // conclusion, which is where a red one is observed at all.
    world.repository("change-auto", &[]);
    world.script("service.work", "the worker wrote this\n");
    world.script("gh.checks", "llmlint completed failure required");
    let run = settle(&world, "requeued", vec![lifecycle("service", &[])]);

    let dispatched = world.events_of(&run, "node-dispatched").len();
    assert!(
        dispatched > 1,
        "this journey's node was dispatched once, so it compares nothing\n{}",
        world.dump()
    );
    let named = given(&world, &run, "service");
    assert_eq!(
        named.len(),
        dispatched,
        "a dispatch published no scratch directory: {named:?}\n{}",
        world.dump()
    );

    let distinct: std::collections::BTreeSet<&std::path::PathBuf> = named.iter().collect();
    assert_eq!(
        distinct.len(),
        named.len(),
        "a node dispatched again was handed a directory an earlier attempt had \
         been writing into: {named:?}"
    );
    // And every one of them is still there holding what its own dispatch wrote,
    // so no attempt's directory was cleared for the next.
    for at in &named {
        kept(&world, at);
    }
    reply(
        &world,
        &run,
        serde_json::json!([{"op":"settle", "id":"service", "outcome":"done", "evidence":"the manager verified the completed work"}]),
    );
    for at in &named {
        assert!(!at.exists(), "successful run kept {}", at.display());
    }
    assert!(!world.run_file(&run, "scratch").exists());
}

/// Every dispatch is given an asker of its own, and no two dispatches share one.
///
/// This is the half of the re-arm repair that belongs to the *executor*, and the
/// half a `onemessagebus serve` cannot state for itself. A dispatched agent asks its
/// manager through a succession of serving sessions and waits for its verdict
/// through all of them; what makes those sessions one side rather than a series
/// of sides that have each gone is the word they carry, and nothing the agent
/// does supplies it. So the dispatch is given one, and it has to be **the same
/// throughout that dispatch** — every session it starts inherits this one value —
/// and **not shared with any other dispatch**, or one agent's listener would take
/// over another's open question and be handed its answer.
///
/// The other half — that two sessions carrying one asker recover a question the
/// first of them left open — is
/// `channel::a_question_survives_its_listener_being_replaced_and_the_verdict_reaches_the_asker`.
///
/// Read off the run's store rather than off the double's files: the promise is a
/// property of the *dispatch*, so the evidence is the value the dispatch itself
/// took out of its environment and published.
#[test]
fn every_dispatch_is_given_an_asker_of_its_own_that_no_other_dispatch_carries() {
    let world = World::new("scratch-asker");
    let run = settle(
        &world,
        "askers",
        vec![agent("build", &[]), agent("ship", &["build"])],
    );

    let mut every: Vec<String> = Vec::new();
    for node in ["build", "ship"] {
        let named = asked_as(&world, &run, node);
        assert_eq!(named.len(), 1, "{named:?}\n{}", world.dump());
        assert!(
            !named[0].trim().is_empty(),
            "{node} was dispatched with an asker that names nobody: {named:?}"
        );
        // What the value *is*, gated rather than restated: the divergence record
        // says a dispatch's asker is the scratch directory it was given, because
        // that directory is already the one handle minted per dispatch. Composing
        // a second one, or naming it after something two dispatches can share,
        // fails here.
        let dirs = given(&world, &run, node);
        assert_eq!(
            named,
            dirs.iter()
                .map(|at| at.display().to_string())
                .collect::<Vec<_>>(),
            "{node}'s asker is no longer the scratch directory the same dispatch was given\n{}",
            world.dump()
        );
        every.extend(named);
    }
    let distinct: std::collections::BTreeSet<&String> = every.iter().collect();
    assert_eq!(
        distinct.len(),
        every.len(),
        "two dispatches were given the same asker, so either one's listener would take over \
         the other's open question: {every:?}"
    );
}

fn scratch_dirs(world: &World, run: &str) -> Vec<std::path::PathBuf> {
    let mut dirs: Vec<_> = std::fs::read_dir(world.run_file(run, "scratch"))
        .expect("dispatch scratch exists")
        .map(|entry| entry.unwrap().path())
        .collect();
    dirs.sort();
    dirs
}

fn allocated_tree(path: &std::path::Path) -> u64 {
    let metadata = std::fs::symlink_metadata(path).unwrap();
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::fs::MetadataExt;
        metadata.blocks() * 512
    };
    #[cfg(windows)]
    let bytes = {
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            FileStandardInfo, GetFileInformationByHandleEx, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_STANDARD_INFO,
        };
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .unwrap();
        let mut info = std::mem::MaybeUninit::<FILE_STANDARD_INFO>::uninit();
        // SAFETY: the file handle is live and opened on the link itself; the buffer
        // is writable storage of the requested class with its exact size passed.
        assert_ne!(
            unsafe {
                GetFileInformationByHandleEx(
                    file.as_raw_handle(),
                    FileStandardInfo,
                    info.as_mut_ptr().cast(),
                    std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
                )
            },
            0
        );
        // SAFETY: success above promises that the whole buffer was filled.
        u64::try_from(unsafe { info.assume_init() }.AllocationSize).unwrap()
    };
    bytes
        + if metadata.is_dir() {
            std::fs::read_dir(path)
                .unwrap()
                .map(|entry| allocated_tree(&entry.unwrap().path()))
                .sum()
        } else {
            0
        }
}

fn snapshot(path: &std::path::Path) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    let mut files = std::collections::BTreeMap::new();
    for entry in std::fs::read_dir(path).unwrap() {
        let at = entry.unwrap().path();
        if std::fs::symlink_metadata(&at).unwrap().is_dir() {
            files.extend(snapshot(&at));
        } else if !std::fs::symlink_metadata(&at).unwrap().is_symlink() {
            files.insert(at.clone(), std::fs::read(at).unwrap());
        }
    }
    files
}

fn reply(world: &World, run: &str, commands: serde_json::Value) {
    world
        .run_with_stdin(
            &["reply", run],
            &serde_json::json!({"version": 3, "commands": commands}).to_string(),
        )
        .exited(0);
}

fn read_views(world: &World, run: &str) -> (String, String) {
    let status = world.run(&["status", run]);
    status.exited(0);
    let results = world.run(&["results", run]);
    results.exited(0);
    let machine = world.run(&["status", run, "--json"]);
    machine.exited(0);
    let document = machine.json();
    let mut fields: Vec<_> = document
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    fields.sort();
    assert_eq!(
        fields,
        [
            "driven",
            "ending",
            "liveness",
            "paused",
            "run_id",
            "schema_version",
            "word"
        ]
    );
    (status.stdout, results.stdout)
}

/// The dispatch double writes before meeting the journey; an ending without hooks
/// removes only scratch, appending its account to the intact journal.
#[test]
fn success_reclaims_dispatch_scratch_without_hooks_and_preserves_run_evidence() {
    let world = World::new("scratch-success-evidence");
    world.script("build.wait", "hold");
    world.script("build.turn-open", "");
    let path = world.plan("success", &plan_of("success", vec![agent("build", &[])]));
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the dispatch's writable scratch", |world| {
        !world.events_of("success", "turn-started").is_empty()
    });
    let dirs = scratch_dirs(&world, "success");
    for dir in &dirs {
        kept(&world, dir);
    }
    let journal = std::fs::read(world.run_file("success", "events.jsonl")).unwrap();
    let launch = std::fs::read(world.run_file("success", "launch.json")).unwrap();
    let sentinel = world.run_file("success", "unrelated");
    std::fs::write(&sentinel, "keep me").unwrap();
    std::fs::create_dir_all(world.run_file("success", "reports")).unwrap();
    std::fs::write(
        world.run_file("success", "reports/prior.txt"),
        "prior report bytes",
    )
    .unwrap();
    let reports = snapshot(&world.run_file("success", "reports"));
    let bytes: u64 = dirs.iter().map(|dir| allocated_tree(dir)).sum();
    world.release("build.go");
    world.until("scratch removal at the success ending", |world| {
        !world.events_of("success", "scratch-removed").is_empty()
    });
    assert!(!world.run_file("success", "scratch").exists());
    assert!(std::fs::read(world.run_file("success", "events.jsonl"))
        .unwrap()
        .starts_with(&journal));
    assert_eq!(
        std::fs::read(world.run_file("success", "launch.json")).unwrap(),
        launch
    );
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"keep me");
    for (path, contents) in reports {
        assert_eq!(std::fs::read(path).unwrap(), contents);
    }
    let records = world.events_of("success", "scratch-removed");
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0]["payload"],
        serde_json::json!({"trigger": "success", "dirs": dirs.len(), "bytes": bytes, "failed": []})
    );
    assert!(world.events_of("success", "scratch-retained").is_empty());
    let (status, results) = read_views(&world, "success");
    assert!(!status.contains("scratch kept:"));
    assert!(!results.contains("scratch kept:"));
}

/// Seven real dispatches produce the capped status and complete results notices;
/// both stay tied to recorded usage after the filesystem changes.
#[test]
fn failed_scratch_is_recorded_once_and_views_use_the_record_with_a_five_directory_cap() {
    let world = World::new("scratch-seven");
    let nodes: Vec<_> = (0..7)
        .map(|index| agent(&format!("job{index}"), &[]))
        .collect();
    for index in 0..7 {
        world.script(&format!("job{index}.wait"), "hold");
        world.script(&format!("job{index}.turn-open"), "");
        if index == 0 {
            world.script("job0.fail", "1");
        }
    }
    let mut plan = plan_of("seven", nodes);
    plan["concurrency"] = serde_json::json!(7);
    let path = world.plan("seven", &plan);
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("all seven dispatches writing scratch", |world| {
        world.events_of("seven", "turn-started").len() >= 7
    });
    let dirs = scratch_dirs(&world, "seven");
    assert_eq!(dirs.len(), 7);
    for (index, dir) in dirs.iter().enumerate() {
        std::fs::write(
            dir.join("distinct-size"),
            vec![index as u8; (index + 1) * 16384],
        )
        .unwrap();
    }
    for index in 0..7 {
        world.release(&format!("job{index}.go"));
    }
    world.until("the failed or unfinished ending records scratch", |world| {
        !world.events_of("seven", "scratch-retained").is_empty()
    });
    let records = world.events_of("seven", "scratch-retained");
    assert_eq!(records.len(), 1);
    let record = &records[0]["payload"];
    assert_eq!(record["ending"], "failed");
    assert_eq!(record["dirs"].as_array().unwrap().len(), 7);
    let mut previous = u64::MAX;
    for dir in record["dirs"].as_array().unwrap() {
        let path = std::path::Path::new(dir["path"].as_str().unwrap());
        let bytes = dir["bytes"].as_u64().unwrap();
        assert_eq!(bytes, allocated_tree(path));
        assert!(bytes < previous);
        previous = bytes;
    }
    let (before_status, before_results) = read_views(&world, "seven");
    assert!(before_status.contains(
        "scratch kept: 7 dir(s), 0.0 GiB — remove with: onepipeline scratch discard seven"
    ));
    assert!(before_status.contains("… and 2 more, 0.0 GiB — full list: onepipeline results seven"));
    let recorded = record["dirs"].as_array().unwrap();
    for (index, dir) in recorded.iter().enumerate() {
        let line = format!("    {}  0.0 GiB", dir["path"].as_str().unwrap());
        assert_eq!(before_status.contains(&line), index < 5);
        assert!(before_results.contains(&line));
    }
    std::fs::write(dirs[0].join("grown"), vec![0; 32768]).unwrap();
    std::fs::remove_dir_all(&dirs[1]).unwrap();
    let (after_status, after_results) = read_views(&world, "seven");
    let notice = |text: String| {
        text.lines()
            .filter(|line| {
                line.contains("scratch kept:")
                    || recorded
                        .iter()
                        .any(|dir| line.contains(dir["path"].as_str().unwrap()))
                    || line.contains("… and 2 more")
            })
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    assert_eq!(notice(before_status), notice(after_status));
    assert_eq!(notice(before_results), notice(after_results));
    reply(
        &world,
        "seven",
        serde_json::json!([{"op":"retry", "id":"job0", "node": agent("fixed", &[])}]),
    );
    world.run(&["adopt", "seven"]).settled();
    assert!(!world.run_file("seven", "scratch").exists());
    let (status, results) = read_views(&world, "seven");
    assert!(!status.contains("scratch kept:") && !results.contains("scratch kept:"));
}

#[test]
fn discard_reports_its_own_walk_and_preserves_everything_else() {
    let world = World::new("scratch-discard");
    let run = "discard".to_string();
    for node in ["one", "two", "three"] {
        world.script(&format!("{node}.wait"), "hold");
        world.script(&format!("{node}.turn-open"), "");
    }
    world.script("one.fail", "1");
    let plan = world.plan(
        &run,
        &plan_of(
            &run,
            vec![agent("one", &[]), agent("two", &[]), agent("three", &[])],
        ),
    );
    world.run(&["start", &plan, "--detach"]).exited(0);
    world.until("three dispatches writing scratch", |world| {
        world.events_of(&run, "turn-started").len() >= 3
    });
    let dirs = scratch_dirs(&world, &run);
    assert_eq!(dirs.len(), 3);
    for (index, dir) in dirs.iter().enumerate() {
        std::fs::write(dir.join("data"), vec![index as u8; (index + 1) * 16384]).unwrap();
    }
    for node in ["one", "two", "three"] {
        world.release(&format!("{node}.go"));
    }
    world.until("the failed ending", |world| {
        !world.events_of(&run, "scratch-retained").is_empty()
    });
    assert_retained(&world, &run, "failed", 1);
    let mut expected: Vec<_> = dirs
        .iter()
        .map(|dir| (allocated_tree(dir), dir.clone()))
        .collect();
    expected.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let journal = std::fs::read(world.run_file(&run, "events.jsonl")).unwrap();
    let before = snapshot(&world.run_file(&run, "."));
    let removed = world.run(&["scratch", "discard", &run]);
    removed.exited(0);
    let report = format!(
        "removed {} scratch dir(s) of run {run}, 0.0 GiB freed:\n{}",
        dirs.len(),
        expected
            .iter()
            .map(|(_, dir)| format!("  {}  0.0 GiB\n", dir.display()))
            .collect::<String>()
    );
    assert_eq!(removed.stdout, report);
    assert!(removed.stderr.is_empty());
    assert!(!world.run_file(&run, "scratch").exists());
    let after = std::fs::read(world.run_file(&run, "events.jsonl")).unwrap();
    assert!(after.starts_with(&journal));
    let appended: Vec<serde_json::Value> = after[journal.len()..]
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    assert_eq!(appended.len(), 1);
    assert_eq!(appended[0]["kind"], "scratch-removed");
    assert_eq!(
        appended[0]["payload"],
        serde_json::json!({"trigger": "discard", "dirs": dirs.len(), "bytes": expected.iter().map(|(bytes,_)| bytes).sum::<u64>(), "failed": []})
    );
    for (path, contents) in before {
        // The journal is held to its prefix above; only the run's own derived
        // caches at its root may be rewritten, as any append or view does.
        if !path.starts_with(world.run_file(&run, "scratch"))
            && !["events.jsonl", "summary.json", "checkpoint.json"]
                .iter()
                .any(|cache| path == world.run_file(&run, cache))
        {
            assert_eq!(
                std::fs::read(&path).unwrap(),
                contents,
                "{}",
                path.display()
            );
        }
    }
    world
        .run(&["scratch", "discard", &run])
        .exited(0)
        .out_has("removed 0 scratch dir(s) of run discard, 0.0 GiB freed");
}

fn assert_retained(world: &World, run: &str, ending: &str, count: usize) -> serde_json::Value {
    let records = world.events_of(run, "scratch-retained");
    assert_eq!(records.len(), count, "{}", world.dump());
    let record = records.last().unwrap()["payload"].clone();
    assert_eq!(record["ending"], ending);
    let dirs = scratch_dirs(world, run);
    assert_eq!(dirs.len(), record["dirs"].as_array().unwrap().len());
    let mut total = 0;
    for dir in record["dirs"].as_array().unwrap() {
        let path = std::path::Path::new(dir["path"].as_str().unwrap());
        assert!(dirs.contains(&path.to_path_buf()));
        let bytes = allocated_tree(path);
        assert_eq!(dir["bytes"], bytes);
        total += bytes;
    }
    assert_eq!(record["total_bytes"], total);
    let (status, results) = read_views(world, run);
    let suffix = if ending == "paused" {
        format!(
            "kept while paused; discard with onepipeline scratch discard {run} once the run ends"
        )
    } else {
        format!("remove with: onepipeline scratch discard {run}")
    };
    for text in [status, results] {
        assert!(
            text.contains(&format!(
                "scratch kept: {} dir(s), 0.0 GiB — {suffix}",
                dirs.len()
            )),
            "{text}"
        );
        let listed: Vec<_> = text
            .lines()
            .filter(|line| {
                line.starts_with("    ")
                    && record["dirs"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|dir| line.contains(dir["path"].as_str().unwrap()))
            })
            .collect();
        let expected: Vec<_> = record["dirs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|dir| format!("    {}  0.0 GiB", dir["path"].as_str().unwrap()))
            .collect();
        assert_eq!(listed, expected);
    }
    record
}

#[test]
fn retry_retains_a_new_ending_then_undriven_success_removes_the_tree() {
    let world = World::new("scratch-refailed");
    world.script("build.fail", "1");
    world.script("again.fail", "1");
    let run = settle(&world, "retry", vec![agent("build", &[])]);
    let original = snapshot(&world.run_file(&run, "scratch"));
    assert_retained(&world, &run, "failed", 1);
    reply(
        &world,
        &run,
        serde_json::json!([{"op":"retry", "id":"build", "node": agent("again", &[])}]),
    );
    world.run(&["adopt", &run]).settled();
    let latest = assert_retained(&world, &run, "failed", 2);
    assert_eq!(latest["dirs"].as_array().unwrap().len(), 2);
    for (path, bytes) in original {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    world.run(&["adopt", &run]).settled();
    assert_eq!(
        world.events_of(&run, "scratch-retained").len(),
        2,
        "an inert adoption is no new ending"
    );
    let launch = std::fs::read(world.run_file(&run, "launch.json")).unwrap();
    let reports = snapshot(&world.run_file(&run, "reports"));
    let journal = std::fs::read(world.run_file(&run, "events.jsonl")).unwrap();
    reply(
        &world,
        &run,
        serde_json::json!([{"op":"settle", "id":"again", "outcome":"done", "evidence":"the manager verified completion"}]),
    );
    assert!(!world.run_file(&run, "scratch").exists());
    assert_eq!(world.events_of(&run, "scratch-removed").len(), 1);
    assert!(std::fs::read(world.run_file(&run, "events.jsonl"))
        .unwrap()
        .starts_with(&journal));
    assert_eq!(
        std::fs::read(world.run_file(&run, "launch.json")).unwrap(),
        launch
    );
    assert_eq!(snapshot(&world.run_file(&run, "reports")), reports);
    let (status, results) = read_views(&world, &run);
    assert!(!status.contains("scratch kept:"));
    assert!(!results.contains("scratch kept:"));
}

/// A run made live again reads as what it is now: the kept notice and its discard
/// instruction, which the verb would refuse, wait for the run's next ending.
#[test]
fn a_run_made_live_again_shows_no_scratch_notice_until_it_ends_again() {
    for partly_discarded in [false, true] {
        made_live_again_after(partly_discarded);
    }
}

fn made_live_again_after(partly_discarded: bool) {
    use std::os::unix::fs::PermissionsExt;
    let world = World::new(if partly_discarded {
        "scratch-live-again-discarded"
    } else {
        "scratch-live-again"
    });
    world.script("build.fail", "1");
    let run = settle(&world, "relive", vec![agent("build", &[])]);
    assert_retained(&world, &run, "failed", 1);
    struct RestorePermissions(std::path::PathBuf);
    impl Drop for RestorePermissions {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
        }
    }
    let blocked = world.run_file(&run, "scratch/blocked");
    let _restore = RestorePermissions(blocked.clone());
    if partly_discarded {
        // A discard the filesystem partly refuses leaves its failure line as the
        // latest record, which a run made live again must not print either.
        partly_discard(&world, &run, &blocked);
    }

    world.script("again.wait", "hold");
    world.script("again.turn-open", "");
    reply(
        &world,
        &run,
        serde_json::json!([{"op":"retry", "id":"build", "node": agent("again", &[])}]),
    );
    let silent = |world: &World, word: &str| {
        let (status, results) = read_views(world, &run);
        assert!(status.starts_with(&format!("{run}  {word}")), "{status}");
        for text in [status, results] {
            assert!(
                !text.contains("scratch kept:")
                    && !text.contains("  scratch:")
                    && !text.contains("scratch discard"),
                "{text}"
            );
        }
    };
    silent(&world, "DRIVER DEAD");
    world.run(&["adopt", &run, "--detach"]).exited(0);
    world.until("the retried dispatch to start", |world| {
        !world.events_of(&run, "turn-started").is_empty()
    });
    silent(&world, "ACTIVE");
    assert_eq!(world.events_of(&run, "scratch-retained").len(), 1);
    world.script("again.fail", "1");
    world.release("again.go");
    world.until("the run to end again", |world| {
        world.events_of(&run, "scratch-retained").len() == 2
    });
    world.until("the driver to let go", |world| {
        world.run(&["status", &run, "--json"]).json()["driven"] == false
    });
    let latest = assert_retained(&world, &run, "failed", 2);
    assert_eq!(latest["dirs"].as_array().unwrap().len(), 2);
}

fn partly_discard(world: &World, run: &str, blocked: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(blocked).unwrap();
    std::fs::write(blocked.join("data"), "written before the discard").unwrap();
    std::fs::set_permissions(blocked, std::fs::Permissions::from_mode(0o555)).unwrap();
    world.run(&["scratch", "discard", run]).exited(1);
    let (status, results) = read_views(world, run);
    let failure = format!(
        "  scratch: 1 dir(s) could not be removed: {}: ",
        blocked.display()
    );
    assert!(status.contains(&failure), "{status}");
    assert!(results.contains(&failure), "{results}");
}

#[test]
fn a_paused_run_keeps_scratch_and_an_undriven_reply_records_its_failed_ending() {
    let world = World::new("scratch-paused-reply");
    world.script("build.fail", "1");
    let run = settle(
        &world,
        "pause",
        vec![agent("build", &[]), crate::harness::human("approve", &[])],
    );
    let before = snapshot(&world.run_file(&run, "scratch"));
    assert_retained(&world, &run, "paused", 1);
    let journal = std::fs::read(world.run_file(&run, "events.jsonl")).unwrap();
    world.run(&["scratch", "discard", &run]).exited(2).err_has("onepipeline scratch discard: run pause is PAUSED, not ended; its scratch is kept and nothing was deleted");
    assert_eq!(
        std::fs::read(world.run_file(&run, "events.jsonl")).unwrap(),
        journal
    );
    assert_eq!(snapshot(&world.run_file(&run, "scratch")), before);
    reply(
        &world,
        &run,
        serde_json::json!([{"op":"drop", "id":"approve", "dependents":"detach"}]),
    );
    assert_retained(&world, &run, "failed", 2);
    assert_eq!(snapshot(&world.run_file(&run, "scratch")), before);
}

#[test]
fn stop_records_retention_after_the_stop_and_a_driver_cancellation_keeps_scratch() {
    for stop in [true, false] {
        let world = World::new(if stop {
            "scratch-stop"
        } else {
            "scratch-cancel"
        });
        world.script("build.wait", "hold");
        world.script("build.turn-open", "");
        world.script("build.turn-open", "");
        world.script("build.stops-when-interrupted", "");
        let path = world.plan("ending", &plan_of("ending", vec![agent("build", &[])]));
        world.run(&["start", &path, "--detach"]).exited(0);
        world.until("the held dispatch's scratch", |world| {
            !world.events_of("ending", "turn-started").is_empty()
        });
        let before = snapshot(&world.run_file("ending", "scratch"));
        if stop {
            world.run(&["stop", "ending"]).exited(0);
        } else {
            reply(
                &world,
                "ending",
                serde_json::json!([{"op":"cancel", "id":"build"}]),
            );
        }
        world.until("the retention record", |world| {
            !world.events_of("ending", "scratch-retained").is_empty()
        });
        assert_retained(
            &world,
            "ending",
            if stop { "stopped" } else { "unfinished" },
            1,
        );
        assert_eq!(snapshot(&world.run_file("ending", "scratch")), before);
        if stop {
            let kinds = world.kinds("ending");
            assert!(
                kinds.iter().position(|kind| kind == "run-stopped").unwrap()
                    < kinds
                        .iter()
                        .position(|kind| kind == "scratch-retained")
                        .unwrap()
            );
        }
        world.until("the driver has released ownership", |world| {
            !world.run_file("ending", "owner.lock").exists()
        });
        world.run(&["scratch", "discard", "ending"]).exited(0);
        assert!(!world.run_file("ending", "scratch").exists());
        assert_eq!(
            world.events_of("ending", "scratch-removed")[0]["payload"]["trigger"],
            "discard"
        );
    }
}

/// Shutdown and a live quiet driver are distinct from endings, and reading
/// every view cannot measure, remove or journal scratch in either state.
#[cfg(unix)]
#[test]
fn shutdown_and_parked_keep_scratch_without_records_or_view_side_effects() {
    for shutdown in [true, false] {
        let world = World::new(if shutdown {
            "scratch-shutdown"
        } else {
            "scratch-parked"
        })
        .with_env("ONEPIPELINE_PARKED_AFTER_SECONDS", "1");
        world.script("build.wait", "hold");
        world.script("build.turn-open", "");
        world.script("build.turn-open", "");
        world.script("build.stops-when-interrupted", "");
        if shutdown {
            world.script("build.fail", "1");
        }
        let path = world.plan("quiet", &plan_of("quiet", vec![agent("build", &[])]));
        world.run(&["start", &path, "--detach"]).exited(0);
        world.until("the held dispatch's scratch", |world| {
            !world.events_of("quiet", "turn-started").is_empty()
        });
        let before = snapshot(&world.run_file("quiet", "scratch"));
        if shutdown {
            world.run(&["shutdown", "quiet", "--grace", "1"]).exited(0);
            // The word is journalled before the driver has left, and a driver
            // still leaving holds the handover gate the verb takes.
            world.until_store("the shutdown driver to exit", |world| {
                world
                    .run(&["status", "quiet"])
                    .stdout
                    .contains("HOST SHUTDOWN")
                    && world.run(&["status", "quiet", "--json"]).json()["driven"] == false
            });
        } else {
            world.until_store("a live driver gone quiet", |world| {
                world
                    .run(&["status", "quiet"])
                    .stdout
                    .starts_with("quiet  PARKED")
            });
        }
        for _ in 0..2 {
            assert!(world.events_of("quiet", "scratch-retained").is_empty());
            assert!(world.events_of("quiet", "scratch-removed").is_empty());
            let (status, results) = read_views(&world, "quiet");
            for text in [status, results] {
                assert!(
                    !text.contains("scratch kept:") && !text.contains("  scratch:"),
                    "{text}"
                );
            }
            assert_eq!(snapshot(&world.run_file("quiet", "scratch")), before);
        }
        let journal = std::fs::read(world.run_file("quiet", "events.jsonl")).unwrap();
        let refused = world.run(&["scratch", "discard", "quiet"]);
        refused.exited(2).err_has(if shutdown {
            "is HOST SHUTDOWN, not ended"
        } else {
            "is PARKED, not ended"
        });
        assert_eq!(
            std::fs::read(world.run_file("quiet", "events.jsonl")).unwrap(),
            journal
        );
        if shutdown {
            // A new driver clears shutdown; its failed dispatch finally keeps scratch.
            world.script("build.fail", "1");
            world.release("build.go");
            world.run(&["adopt", "quiet"]).settled();
            assert_retained(&world, "quiet", "failed", 1);
        } else {
            world.run(&["stop", "quiet"]).exited(0);
        }
    }
}

/// A shut-down driver journals `host-shutdown` and only then lets go of the run
/// inside its handover gate. Discard in that interval still answers with the run's
/// word, since a refusal deletes nothing and needs no gate.
#[test]
fn discard_refuses_a_shut_down_run_whose_departing_driver_holds_the_gate() {
    let world =
        World::new("scratch-shutdown-gate").with_env("ONEPIPELINE_PARKED_AFTER_SECONDS", "1");
    world.script("build.wait", "hold");
    world.script("build.turn-open", "");
    world.script("build.turn-open", "");
    world.script("build.stops-when-interrupted", "");
    world.script("build.fail", "1");
    let path = world.plan("leaving", &plan_of("leaving", vec![agent("build", &[])]));
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the held dispatch's scratch", |world| {
        !world.events_of("leaving", "turn-started").is_empty()
    });
    world
        .run(&["shutdown", "leaving", "--grace", "1"])
        .exited(0);
    world.until_store("the shutdown driver to exit", |world| {
        world
            .run(&["status", "leaving"])
            .stdout
            .contains("HOST SHUTDOWN")
            && world.run(&["status", "leaving", "--json"]).json()["driven"] == false
    });
    // llmlint: ignore-block[tests_mirror_real_usage] which party is inside the gate
    // is decided in microseconds inside the departing driver, and no CLI input holds
    // it there. This entry is the one that driver writes while it lets go: a live
    // process on this host, which every gate taker must wait behind.
    let gate = world.run_file("leaving", "channel/handover");
    std::fs::create_dir_all(&gate).unwrap();
    // The departed driver's ownership lock is gone; its launch record still names
    // the host its pid is meaningful on.
    let host = world.run_json("leaving", "launch.json")["host"]
        .as_str()
        .unwrap()
        .to_owned();
    let holder = gate.join(format!("{:020}", 1));
    std::fs::write(&holder, format!("{} {host}", std::process::id())).unwrap();
    // llmlint: ignore-end[tests_mirror_real_usage]
    let journal = std::fs::read(world.run_file("leaving", "events.jsonl")).unwrap();
    let before = snapshot(&world.run_file("leaving", "scratch"));
    assert!(!before.is_empty());

    let refused = world.run(&["scratch", "discard", "leaving"]);
    refused.exited(2);
    assert_eq!(
        refused.stderr,
        "onepipeline scratch discard: run leaving is HOST SHUTDOWN, not ended; its scratch is \
         kept and nothing was deleted\n"
    );
    assert!(refused.stdout.is_empty(), "{}", refused.stdout);
    assert_eq!(
        std::fs::read(world.run_file("leaving", "events.jsonl")).unwrap(),
        journal
    );
    assert_eq!(snapshot(&world.run_file("leaving", "scratch")), before);

    std::fs::remove_file(holder).unwrap();
    world.script("build.fail", "1");
    world.release("build.go");
    world.run(&["adopt", "leaving"]).settled();
    assert_retained(&world, "leaving", "failed", 1);
}

#[test]
fn discard_refuses_unknown_and_path_like_names_and_live_or_dead_drivers() {
    let world = World::new("scratch-discard-refusals");
    world.script("build.wait", "hold");
    world.script("build.turn-open", "");
    let path = world.plan(
        "live",
        &plan_of("live", vec![agent("build", &[]), agent("later", &[])]),
    );
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the dispatch's scratch", |world| {
        !world.events_of("live", "turn-started").is_empty()
    });
    let before = snapshot(&world.run_file("live", "scratch"));
    for run in ["unknown", "../live", "/live", "live/", "live\\outside"] {
        world.run(&["scratch", "discard", run]).exited(2).err_has(&format!("onepipeline scratch discard: no run named '{run}' under the runs root; deleted nothing"));
    }
    for word in ["ACTIVE", "DRIVER DEAD"] {
        if word == "DRIVER DEAD" {
            crate::harness::end_driver(&world, "live");
        }
        let journal = std::fs::read(world.run_file("live", "events.jsonl")).unwrap();
        world.run(&["scratch", "discard", "live"]).exited(2).err_has(&format!("onepipeline scratch discard: run live is {word}, not ended; its scratch is kept and nothing was deleted"));
        assert_eq!(
            std::fs::read(world.run_file("live", "events.jsonl")).unwrap(),
            journal
        );
        assert_eq!(snapshot(&world.run_file("live", "scratch")), before);
    }
    world.run(&["stop", "live"]).exited(0);
}

/// A directory link and a file link, each to a target outside the run. Windows CI
/// runs this through the platform's own link kinds; the Linux worker runs Unix's.
#[cfg(any(unix, windows))]
#[test]
fn links_are_unlinked_without_following_them_and_a_link_scratch_root_is_refused() {
    #[cfg(unix)]
    use std::os::unix::fs::{symlink as symlink_dir, symlink as symlink_file};
    #[cfg(windows)]
    use std::os::windows::fs::{symlink_dir, symlink_file};
    for success in [true, false] {
        let world = World::new(if success {
            "scratch-success-links"
        } else {
            "scratch-discard-links"
        });
        world.script("build.wait", "hold");
        world.script("build.turn-open", "");
        if !success {
            world.script("build.fail", "1");
        }
        let path = world.plan("links", &plan_of("links", vec![agent("build", &[])]));
        world.run(&["start", &path, "--detach"]).exited(0);
        world.until("the dispatch's scratch", |world| {
            !world.events_of("links", "turn-started").is_empty()
        });
        let outside = world.root.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("precious"), "keep the target").unwrap();
        let dir = scratch_dirs(&world, "links").remove(0);
        symlink_dir(&outside, dir.join("directory-link")).unwrap();
        symlink_file(outside.join("precious"), dir.join("file-link")).unwrap();
        world.release("build.go");
        world.until("the ending scratch record", |world| {
            !world
                .events_of(
                    "links",
                    if success {
                        "scratch-removed"
                    } else {
                        "scratch-retained"
                    },
                )
                .is_empty()
        });
        if !success {
            world.run(&["scratch", "discard", "links"]).exited(0);
        }
        assert!(!world.run_file("links", "scratch").exists());
        assert_eq!(
            std::fs::read(outside.join("precious")).unwrap(),
            b"keep the target"
        );
        if !success {
            symlink_dir(&outside, world.run_file("links", "scratch")).unwrap();
            let journal = std::fs::read(world.run_file("links", "events.jsonl")).unwrap();
            world
                .run(&["scratch", "discard", "links"])
                .exited(2)
                .err_has(&format!(
                    "onepipeline scratch discard: {} is a symlink; nothing was deleted",
                    world.run_file("links", "scratch").display()
                ));
            assert_eq!(
                std::fs::read(world.run_file("links", "events.jsonl")).unwrap(),
                journal
            );
            assert_eq!(
                std::fs::read(outside.join("precious")).unwrap(),
                b"keep the target"
            );
        }
    }
}

/// A filesystem permission refusal in one directory must not stop removal of
/// its peers, for either trigger. The driver still settles on successful work.
#[cfg(unix)]
#[test]
fn partial_filesystem_failure_is_journalled_and_visible_for_both_removal_triggers() {
    use std::os::unix::fs::PermissionsExt;
    for (success, failure_count) in [(true, 1), (false, 1), (true, 2), (false, 2)] {
        let world = World::new(if success {
            "scratch-success-partial"
        } else {
            "scratch-discard-partial"
        });
        world.script("build.wait", "hold");
        world.script("build.turn-open", "");
        if !success {
            world.script("build.fail", "1");
        }
        let path = world.plan("partial", &plan_of("partial", vec![agent("build", &[])]));
        world.run(&["start", &path, "--detach"]).exited(0);
        world.until("the dispatch's scratch", |world| {
            !world.events_of("partial", "turn-started").is_empty()
        });
        let blocked = world.run_file("partial", "scratch/blocked");
        let another_blocked = world.run_file("partial", "scratch/another-blocked");
        let removable = world.run_file("partial", "scratch/removable");
        for dir in [&blocked, &removable]
            .into_iter()
            .chain((failure_count == 2).then_some(&another_blocked))
        {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("data"), "written before the ending").unwrap();
        }
        struct RestorePermissions(std::path::PathBuf);
        impl Drop for RestorePermissions {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
            }
        }
        let _restore = RestorePermissions(blocked.clone());
        let _restore_another =
            (failure_count == 2).then(|| RestorePermissions(another_blocked.clone()));
        let good_dirs: Vec<_> = scratch_dirs(&world, "partial")
            .into_iter()
            .filter(|path| path != &blocked && path != &another_blocked)
            .collect();
        let freed: u64 = good_dirs.iter().map(|path| allocated_tree(path)).sum();
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o555)).unwrap();
        if failure_count == 2 {
            std::fs::set_permissions(&another_blocked, std::fs::Permissions::from_mode(0o555))
                .unwrap();
        }
        world.release("build.go");
        world.until("the automatic scratch record", |world| {
            !world
                .events_of(
                    "partial",
                    if success {
                        "scratch-removed"
                    } else {
                        "scratch-retained"
                    },
                )
                .is_empty()
        });
        let discarded = if success {
            None
        } else {
            Some(world.run(&["scratch", "discard", "partial"]))
        };
        let records = world.events_of("partial", "scratch-removed");
        assert_eq!(records.len(), 1);
        let record = &records[0]["payload"];
        assert_eq!(
            record["trigger"],
            if success { "success" } else { "discard" }
        );
        let failed = record["failed"].as_array().unwrap();
        assert_eq!(failed.len(), failure_count);
        assert_eq!(record["dirs"], good_dirs.len());
        assert_eq!(record["bytes"], freed);
        let mut failed_paths: Vec<_> = failed
            .iter()
            .map(|failure| failure["path"].as_str().unwrap())
            .collect();
        failed_paths.sort();
        let mut expected_paths = vec![blocked.to_str().unwrap()];
        if failure_count == 2 {
            expected_paths.push(another_blocked.to_str().unwrap());
        }
        expected_paths.sort();
        assert_eq!(failed_paths, expected_paths);
        let error = failed[0]["error"].as_str().unwrap();
        assert!(!error.is_empty());
        assert!(!removable.exists());
        let (status, results) = read_views(&world, "partial");
        if success {
            assert!(status.starts_with("partial  SETTLED"));
        }
        let notice = format!(
            "  scratch: {} dir(s) could not be removed: {}: {error}",
            failure_count,
            failed[0]["path"].as_str().unwrap()
        );
        assert!(status.contains(&notice));
        assert!(results.contains(&notice));
        if let Some(discarded) = discarded {
            assert!(discarded.stdout.starts_with(&format!(
                "removed {} scratch dir(s) of run partial, 0.0 GiB freed:\n",
                good_dirs.len()
            )));
            discarded.exited(1);
            for failure in failed {
                discarded.err_has(&format!(
                    "onepipeline scratch discard: could not remove {}: {}",
                    failure["path"].as_str().unwrap(),
                    failure["error"].as_str().unwrap()
                ));
            }
            assert!(!discarded
                .stdout
                .contains(&format!("  {}  ", blocked.display())));
        }
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o755)).unwrap();
        if failure_count == 2 {
            std::fs::set_permissions(&another_blocked, std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        world.run(&["scratch", "discard", "partial"]).exited(0);
        assert!(!world.run_file("partial", "scratch").exists());
    }
}

/// The historical backlog has no scratch event to read: the verb measures what
/// exists itself. Removing scratch remains a library operation with CLI parity.
#[test]
fn historical_settled_scratch_and_public_verb_parity() {
    let world = World::new("scratch-historical");
    let run = settle(&world, "old", vec![agent("build", &[])]);
    let journal_path = world.run_file(&run, "events.jsonl");
    // llmlint: ignore-block[tests_mirror_real_usage] This build always writes a scratch
    // ending account, so it cannot emit the pre-release input this criterion requires.
    // The fixture keeps every real compiled-run record except the new scratch account;
    // the compiled discard consumer then measures real files and preserves that prefix.
    let historic = std::fs::read_to_string(&journal_path)
        .unwrap()
        .lines()
        .filter(|line| {
            let event: serde_json::Value = serde_json::from_str(line).unwrap();
            event["kind"] != "scratch-removed" && event["kind"] != "scratch-retained"
        })
        .map(|line| format!("{line}\n"))
        .collect::<String>();
    std::fs::write(&journal_path, &historic).unwrap();
    // llmlint: ignore-end[tests_mirror_real_usage]
    let dir = world.run_file(&run, "scratch/old-dispatch");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("data"), vec![42; 16384]).unwrap();
    let bytes = allocated_tree(&dir);
    let launch = std::fs::read(world.run_file(&run, "launch.json")).unwrap();
    let reports = snapshot(&world.run_file(&run, "reports"));
    world
        .run(&["scratch", "discard", &run])
        .exited(0)
        .out_has("removed 1 scratch dir(s) of run old, 0.0 GiB freed:");
    let journal = std::fs::read_to_string(&journal_path).unwrap();
    assert!(journal.starts_with(&historic));
    let record = world.events_of(&run, "scratch-removed");
    assert_eq!(record.len(), 1);
    assert_eq!(record[0]["payload"]["bytes"], bytes);
    assert_eq!(
        std::fs::read(world.run_file(&run, "launch.json")).unwrap(),
        launch
    );
    assert_eq!(snapshot(&world.run_file(&run, "reports")), reports);
    let sdk = onepipeline::verbs::scratch_discard(&world.runs, &run).unwrap();
    let cli = world.run(&["scratch", "discard", &run]);
    cli.exited(sdk.exit_code());
    assert_eq!(cli.stdout, onepipeline::verbs::render_scratch_discard(&sdk));
    assert_eq!(
        cli.stderr,
        onepipeline::verbs::render_scratch_discard_errors(&sdk)
    );
    for unknown in ["unknown", "../old"] {
        let sdk = onepipeline::verbs::scratch_discard(&world.runs, unknown).unwrap();
        let cli = world.run(&["scratch", "discard", unknown]);
        cli.exited(sdk.exit_code());
        assert_eq!(cli.stdout, onepipeline::verbs::render_scratch_discard(&sdk));
        assert_eq!(
            cli.stderr,
            onepipeline::verbs::render_scratch_discard_errors(&sdk)
        );
    }
}

#[test]
fn attestation_after_a_pause_reaches_success_and_reclaims_the_retained_scratch() {
    let world = World::new("scratch-attested");
    let run = settle(
        &world,
        "attested",
        vec![
            agent("build", &[]),
            crate::harness::human("approve", &["build"]),
        ],
    );
    assert_retained(&world, &run, "paused", 1);
    world.run(&["attest", &run, "approve"]).exited(0);
    world.run(&["adopt", &run]).settled();
    assert!(!world.run_file(&run, "scratch").exists());
    assert_eq!(world.events_of(&run, "scratch-removed").len(), 1);
}

/// `du` counts allocated blocks once per inode, and a sparse file's logical
/// length is not its disk usage. Compare the ending record with the real tool.
#[cfg(target_os = "linux")]
#[test]
fn retained_disk_usage_matches_du_for_sparse_files_and_hard_links() {
    let world = World::new("scratch-disk-usage");
    world.script("build.wait", "hold");
    world.script("build.turn-open", "");
    world.script("build.fail", "1");
    let plan = world.plan("usage", &plan_of("usage", vec![agent("build", &[])]));
    world.run(&["start", &plan, "--detach"]).exited(0);
    world.until("the scratch-writing turn", |world| {
        !world.events_of("usage", "turn-started").is_empty()
    });
    let dir = scratch_dirs(&world, "usage").remove(0);
    std::fs::File::create(dir.join("sparse"))
        .unwrap()
        .set_len(512 * 1024 * 1024)
        .unwrap();
    std::fs::write(dir.join("original"), vec![0; 16384]).unwrap();
    std::fs::hard_link(dir.join("original"), dir.join("alias")).unwrap();
    let du = std::process::Command::new("du")
        .args(["-s", "-B1"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(du.status.success());
    let bytes: u64 = std::str::from_utf8(&du.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(bytes < 512 * 1024 * 1024);
    world.release("build.go");
    world.until("the retention record", |world| {
        !world.events_of("usage", "scratch-retained").is_empty()
    });
    let record = world.events_of("usage", "scratch-retained");
    assert_eq!(record[0]["payload"]["total_bytes"], bytes);
    assert_eq!(record[0]["payload"]["dirs"][0]["bytes"], bytes);
    world.run(&["scratch", "discard", "usage"]).exited(0);
    let removed = world.events_of("usage", "scratch-removed");
    assert_eq!(removed[0]["payload"]["bytes"], bytes);
    assert!(!world.run_file("usage", "scratch").exists());
}

/// A future producer's record is external journal input. The older reader must
/// not reclaim scratch from the partial graph it can still display as settled.
#[test]
fn unreadable_journal_keeps_scratch_at_success_and_refuses_discard() {
    use std::io::Write;
    for automatic in [true, false] {
        let world = World::new(if automatic {
            "scratch-unreadable-success"
        } else {
            "scratch-unreadable-discard"
        });
        let run = "unread";
        let plan = world.plan(run, &plan_of(run, vec![agent("build", &[])]));
        if automatic {
            world.script("build.wait", "hold");
            world.script("build.turn-open", "");
            world.run(&["start", &plan, "--detach"]).exited(0);
            world.until("the held dispatch", |world| {
                !world.events_of(run, "turn-started").is_empty()
            });
        } else {
            world.run(&["start", &plan, "--attach"]).settled();
            let dir = world.run_file(run, "scratch/historical");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("kept"), "unproven data").unwrap();
        }
        let before = snapshot(&world.run_file(run, "scratch"));
        let stamp = world.journal(run).last().unwrap()["ts"].clone();
        // llmlint: ignore-block[tests_mirror_real_usage] A current emitter cannot
        // produce an unreadable future schema. The manager explicitly requires this
        // external journal fixture; the real driver and compiled discard reader
        // consume it, without substituting their fold or filesystem operations.
        let record = serde_json::json!({"v":999, "ts":stamp, "stream":"future-producer", "seq":0, "source":"pipeline", "kind":"future-operation", "labels":{"run_id":run}, "payload":{}, "artifacts":[]});
        std::fs::OpenOptions::new()
            .append(true)
            .open(world.run_file(run, "events.jsonl"))
            .unwrap()
            .write_all(format!("{record}\n").as_bytes())
            .unwrap();
        // llmlint: ignore-end[tests_mirror_real_usage]
        if automatic {
            world.release("build.go");
            world.until("the driver to release its ending", |world| {
                world.run_file(run, "result.json").is_file()
                    && !world.run_file(run, "owner.lock").exists()
            });
            assert!(world.events_of(run, "scratch-removed").is_empty());
        }
        assert_eq!(snapshot(&world.run_file(run, "scratch")), before);
        let (status, _) = read_views(&world, run);
        assert!(status.starts_with("unread  SETTLED"), "{status}");
        let journal = std::fs::read(world.run_file(run, "events.jsonl")).unwrap();
        world.run(&["scratch", "discard", run]).exited(2).err_has("onepipeline scratch discard: run unread is SETTLED, not ended; its scratch is kept and nothing was deleted");
        assert_eq!(
            std::fs::read(world.run_file(run, "events.jsonl")).unwrap(),
            journal
        );
        assert_eq!(snapshot(&world.run_file(run, "scratch")), before);
    }
}

#[cfg(unix)]
#[test]
fn retention_walk_failure_keeps_bytes_and_reports_the_filesystem_error() {
    use std::os::unix::fs::PermissionsExt;
    let world = World::new("scratch-retention-error");
    world.script("build.wait", "hold");
    world.script("build.turn-open", "");
    world.script("build.fail", "1");
    let plan = world.plan("measure", &plan_of("measure", vec![agent("build", &[])]));
    world.run(&["start", &plan, "--detach"]).exited(0);
    world.until("the held dispatch", |world| {
        !world.events_of("measure", "turn-started").is_empty()
    });
    let unreadable = world.run_file("measure", "scratch/unreadable");
    std::fs::create_dir_all(&unreadable).unwrap();
    std::fs::write(unreadable.join("data"), "keep this data").unwrap();
    let before = snapshot(&world.run_file("measure", "scratch"));
    std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o0)).unwrap();
    world.release("build.go");
    world.until("the ending's ownership release", |world| {
        world.run_file("measure", "result.json").is_file()
            && !world.run_file("measure", "owner.lock").exists()
    });
    std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(snapshot(&world.run_file("measure", "scratch")), before);
    assert!(world.events_of("measure", "scratch-retained").is_empty());
    assert!(world.events_of("measure", "scratch-removed").is_empty());
    let diagnostic = std::fs::read_to_string(world.run_file("measure", "driver.log")).unwrap();
    assert!(
        diagnostic.contains("scratch of run measure could not be handled"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("Permission denied"), "{diagnostic}");
}

#[cfg(unix)]
#[test]
fn optional_telemetry_errors_preserve_endings_and_discard_outcomes() {
    use std::os::unix::fs::PermissionsExt;
    for success in [true, false] {
        let world = World::new(if success {
            "scratch-telemetry-success"
        } else {
            "scratch-telemetry-failed"
        });
        let destination = world.root.join("telemetry");
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o555)).unwrap();
        let world = world.with_env(
            "ONEPIPELINE_SCRATCH_TELEMETRY_DIR",
            destination.to_str().unwrap(),
        );
        if !success {
            world.script("build.fail", "1");
        }
        let plan = world.plan("metrics", &plan_of("metrics", vec![agent("build", &[])]));
        let ended = world.run(&["start", &plan, "--attach"]);
        ended.settled().err_has("onepipeline: scratch telemetry:");
        if success {
            ended.exited(0);
            assert!(!world.run_file("metrics", "scratch").exists());
            assert_eq!(world.events_of("metrics", "scratch-removed").len(), 1);
        } else {
            assert_retained(&world, "metrics", "failed", 1);
            let discarded = world.run(&["scratch", "discard", "metrics"]);
            discarded
                .exited(0)
                .err_has("onepipeline: scratch telemetry:");
            assert!(discarded
                .stdout
                .starts_with("removed 1 scratch dir(s) of run metrics, 0.0 GiB freed:\n"));
            assert!(!world.run_file("metrics", "scratch").exists());
            assert_eq!(world.events_of("metrics", "scratch-removed").len(), 1);
        }
        std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn a_symlink_run_directory_is_not_a_named_run_and_its_target_stays_unchanged() {
    let world = World::new("scratch-run-alias");
    world.script("build.fail", "1");
    let run = settle(&world, "real", vec![agent("build", &[])]);
    let before = snapshot(&world.run_file(&run, "."));
    std::os::unix::fs::symlink(world.run_file(&run, "."), world.runs.join("alias")).unwrap();
    world
        .run(&["scratch", "discard", "alias"])
        .exited(2)
        .err_has(
        "onepipeline scratch discard: no run named 'alias' under the runs root; deleted nothing",
    );
    assert_eq!(snapshot(&world.run_file(&run, ".")), before);
}

#[cfg(unix)]
#[test]
fn discard_refuses_an_unreadable_scratch_root_without_a_journal_write() {
    use std::os::unix::fs::PermissionsExt;
    let world = World::new("scratch-root-access");
    world.script("build.fail", "1");
    let plan = world.plan(
        "root-denied",
        &plan_of("root-denied", vec![agent("build", &[])]),
    );
    world.run(&["start", &plan]).exited(3);
    let scratch = world.run_file("root-denied", "scratch");
    let before = snapshot(&scratch);
    let journal = std::fs::read(world.run_file("root-denied", "events.jsonl")).unwrap();
    std::fs::set_permissions(&scratch, std::fs::Permissions::from_mode(0o0)).unwrap();
    let refusal = world.run(&["scratch", "discard", "root-denied"]);
    std::fs::set_permissions(&scratch, std::fs::Permissions::from_mode(0o755)).unwrap();
    refusal.exited(2).err_has("Permission denied");
    assert_eq!(snapshot(&scratch), before);
    assert_eq!(
        std::fs::read(world.run_file("root-denied", "events.jsonl")).unwrap(),
        journal
    );
}

/// Windows CI executes this real-binary allocation journey; the Linux worker
/// cross-typechecks the native accounting and runs the separate du oracle.
#[cfg(windows)]
#[test]
fn windows_hard_link_aliases_do_not_increase_retained_or_removed_bytes() {
    let mut totals = Vec::new();
    for linked in [false, true] {
        let world = World::new(if linked {
            "scratch-windows-linked"
        } else {
            "scratch-windows-single"
        });
        world.script("build.wait", "hold");
        world.script("build.turn-open", "");
        world.script("build.fail", "1");
        let plan = world.plan(
            "allocation",
            &plan_of("allocation", vec![agent("build", &[])]),
        );
        world.run(&["start", &plan, "--detach"]).exited(0);
        world.until("the writing dispatch", |world| {
            !world.events_of("allocation", "turn-started").is_empty()
        });
        let dir = scratch_dirs(&world, "allocation").remove(0);
        std::fs::write(dir.join("original"), vec![0; 16384]).unwrap();
        if linked {
            std::fs::hard_link(dir.join("original"), dir.join("alias")).unwrap();
        }
        world.release("build.go");
        world.until("the completed failed ending", |world| {
            !world.events_of("allocation", "scratch-retained").is_empty()
                && !world.run_file("allocation", "owner.lock").exists()
        });
        let bytes = world.events_of("allocation", "scratch-retained")[0]["payload"]["total_bytes"]
            .as_u64()
            .unwrap();
        assert!(bytes >= 16384);
        world.run(&["scratch", "discard", "allocation"]).exited(0);
        assert_eq!(
            world.events_of("allocation", "scratch-removed")[0]["payload"]["bytes"],
            bytes
        );
        assert!(!world.run_file("allocation", "scratch").exists());
        totals.push(bytes);
    }
    assert_eq!(totals[0], totals[1]);
}

#[cfg(unix)]
#[test]
fn a_success_ending_refuses_a_symlink_scratch_root_and_preserves_its_target() {
    use std::os::unix::fs::symlink;
    let world = World::new("scratch-success-root-link");
    world.script("build.wait", "hold");
    world.script("build.turn-open", "");
    let plan = world.plan(
        "root-link",
        &plan_of("root-link", vec![agent("build", &[])]),
    );
    world.run(&["start", &plan, "--detach"]).exited(0);
    world.until("the held dispatch", |world| {
        !world.events_of("root-link", "turn-started").is_empty()
    });
    let scratch = world.run_file("root-link", "scratch");
    let displaced = world.root.join("displaced-scratch");
    let outside = world.root.join("external-target");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("precious"), "keep these bytes").unwrap();
    std::fs::rename(&scratch, &displaced).unwrap();
    symlink(&outside, &scratch).unwrap();
    world.release("build.go");
    world.until("the settled driver exits", |world| {
        world.run_file("root-link", "result.json").is_file()
            && !world.run_file("root-link", "owner.lock").exists()
    });
    world.run(&["status", "root-link"]).out_has("SETTLED");
    assert!(std::fs::symlink_metadata(&scratch).unwrap().is_symlink());
    assert_eq!(
        std::fs::read(outside.join("precious")).unwrap(),
        b"keep these bytes"
    );
    assert!(world.events_of("root-link", "scratch-removed").is_empty());
    assert!(world.events_of("root-link", "scratch-retained").is_empty());
    let log = std::fs::read_to_string(world.run_file("root-link", "driver.log")).unwrap();
    assert!(log.contains("is a symlink; nothing was deleted"), "{log}");
}

#[cfg(unix)]
#[test]
fn non_utf8_dispatch_names_are_refused_before_any_scratch_is_deleted() {
    use std::os::unix::ffi::OsStringExt;
    for success in [true, false] {
        let world = World::new(if success {
            "scratch-invalid-name-success"
        } else {
            "scratch-invalid-name-discard"
        });
        world.script("build.wait", "hold");
        world.script("build.turn-open", "");
        if !success {
            world.script("build.fail", "1");
        }
        let plan = world.plan(
            "invalid-name",
            &plan_of("invalid-name", vec![agent("build", &[])]),
        );
        world.run(&["start", &plan, "--detach"]).exited(0);
        world.until("the writing dispatch", |world| {
            !world.events_of("invalid-name", "turn-started").is_empty()
        });
        let invalid = world
            .run_file("invalid-name", "scratch")
            .join(std::ffi::OsString::from_vec(vec![b'x', 0xff]));
        std::fs::create_dir(&invalid).unwrap();
        std::fs::write(invalid.join("data"), "keep invalid directory").unwrap();
        let ordinary = scratch_dirs(&world, "invalid-name")
            .into_iter()
            .find(|path| path != &invalid)
            .unwrap();
        let before = snapshot(&ordinary);
        world.release("build.go");
        world.until("the driver exits", |world| {
            world.run_file("invalid-name", "result.json").is_file()
                && !world.run_file("invalid-name", "owner.lock").exists()
        });
        let journal = std::fs::read(world.run_file("invalid-name", "events.jsonl")).unwrap();
        world
            .run(&["scratch", "discard", "invalid-name"])
            .exited(2)
            .err_has("scratch path is not UTF-8");
        assert_eq!(
            std::fs::read(world.run_file("invalid-name", "events.jsonl")).unwrap(),
            journal
        );
        assert_eq!(snapshot(&ordinary), before);
        assert_eq!(
            std::fs::read(invalid.join("data")).unwrap(),
            b"keep invalid directory"
        );
        assert!(world
            .events_of("invalid-name", "scratch-removed")
            .is_empty());
    }
}

#[test]
fn a_paused_run_dispatches_added_work_and_records_its_next_pause_freshly() {
    let world = World::new("scratch-paused-again");
    let run = settle(
        &world,
        "pause-again",
        vec![
            agent("build", &[]),
            crate::harness::human("approve", &["build"]),
        ],
    );
    let first = assert_retained(&world, &run, "paused", 1);
    let before = snapshot(&world.run_file(&run, "scratch"));
    reply(
        &world,
        &run,
        serde_json::json!([{"op":"add", "node":agent("extra", &[])}]),
    );
    assert_eq!(world.events_of(&run, "scratch-retained").len(), 1);
    world.run(&["adopt", &run]).settled();
    assert_eq!(world.events_of(&run, "scratch-retained").len(), 2);
    let latest = assert_retained(&world, &run, "paused", 2);
    assert_eq!(first["ending"], "paused");
    assert!(latest["dirs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|dir| dir["path"] == given(&world, &run, "extra")[0].to_str().unwrap()));
    for (path, bytes) in before {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    world.run(&["adopt", &run]).settled();
    assert_eq!(
        world.events_of(&run, "scratch-retained").len(),
        2,
        "an inert adoption cannot repeat an ending"
    );
}

/// Append a copy of the run's latest `scratch-retained` record naming another
/// ending, as the next record of its journal's stream.
fn retain_as(world: &World, run: &str, ending: &str) {
    let journal = world.journal(run);
    let mut record = world
        .events_of(run, "scratch-retained")
        .pop()
        .expect("a retained record to copy");
    record["seq"] = serde_json::json!(journal.last().unwrap()["seq"].as_u64().unwrap() + 1);
    record["payload"]["ending"] = serde_json::json!(ending);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(world.run_file(run, "events.jsonl"))
        .unwrap();
    std::io::Write::write_all(&mut file, format!("{record}\n").as_bytes()).unwrap();
}

/// A kept record speaks only for the ending it was written at: a `paused` record
/// on a run that now reads `ENDED failed` gives no paused wording, and a `failed`
/// record on a `PAUSED` run gives no discard instruction the verb would refuse.
#[test]
fn a_retained_record_for_another_ending_than_the_runs_reading_prints_no_notice() {
    let world = World::new("scratch-mismatched-ending");
    world.script("failed.fail", "1");
    world.script("paused.fail", "1");
    let failed = settle(&world, "failed", vec![agent("failed", &[])]);
    let paused = settle(
        &world,
        "paused",
        vec![agent("paused", &[]), crate::harness::human("approve", &[])],
    );
    for (run, word, recorded, other) in [
        (&failed, "ENDED failed", "failed", "paused"),
        (&paused, "PAUSED", "paused", "failed"),
    ] {
        assert_retained(&world, run, recorded, 1);
        retain_as(&world, run, other);
        let status = world.run(&["status", run]);
        status.exited(0).out_has(word);
        let (status, results) = read_views(&world, run);
        for text in [status, results] {
            assert!(!text.contains("scratch kept:"), "{text}");
            assert!(!text.contains("scratch discard"), "{text}");
        }
    }
    // The appended record is one the journal admits strictly: the verb, which
    // refuses a run whose fold is not, removes the failed run's scratch.
    world.run(&["scratch", "discard", &failed]).exited(0);
}
