//! `onepipeline unpublished` and `stop-guard --unpublished` — whether a manager
//! session still owes a preserved branch, decided in process over the linked
//! `onevcs`.
//!
//! Every journey drives the compiled binary over a real `ONEVCS_HOME`, a real
//! origin on disk and real git: branches are cut, worked, closed, landed,
//! preserved and swept by the real `onevcs` verbs, and every session carries the
//! labels a run's sessions carry. Nothing inside either crate is substituted. The
//! one stand-in is `World`'s `oneagentgraph` double, for the journey whose claim
//! is about a printed landing command running this host's drafter.
//!
//! The stop-guard journeys run in an environment whose
//! `ONEPIPELINE_LAUNCHER_SESSION` names *another* session, as `stop_guard.rs`'s
//! do, so an answer about the payload's session is one the environment could not
//! have given.

// llmlint: ignore-file[e2e_not_mocked] the crate under test is driven as a real compiled
// binary over the real `onevcs` library and binary, real git and a real origin on disk.
// `oneagentgraph` is substituted at its subprocess boundary by `World`, so the landing
// journey states a drafting outcome rather than paying for a model turn; `harness.rs`
// carries the same suppression and the full rationale.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::harness::{git, onevcs_binary, Repository, Run, World};

/// The manager session whose runs' sessions these journeys label.
const MANAGER: &str = "manager-under-test";

/// Another manager's session on the same host.
const OTHER: &str = "another-manager";

/// The session a stop hook's environment carries: never the one asked about.
const INHERITED: &str = "the-managers-session";

/// The listing's exit statuses, as entry 113 states them.
const NOTHING_COUNTED: i32 = 0;
const COUNTED: i32 = 7;
const UNANSWERED: i32 = 1;
const REFUSED: i32 = 2;

/// A host with one registered repository whose publication lands with git alone.
fn host(name: &str) -> (World, Repository) {
    let world = World::new(name);
    let repository = world.repository("local-direct", &[]);
    (world, repository)
}

/// Run the real `onevcs` binary against this world and answer its stdout.
fn onevcs(world: &World, args: &[&str]) -> String {
    let output = world
        .cmd_on(&onevcs_binary(), args)
        .output()
        .expect("onevcs runs");
    assert!(
        output.status.success(),
        "onevcs {args:?} failed: {}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Open a session on `branch` labelled as a run of `launcher` opens one, and
/// answer its token and worktree.
fn open(world: &World, branch: &str, launcher: &str) -> (String, PathBuf) {
    let opened = onevcs(
        world,
        &[
            "session",
            "open",
            "service",
            "--branch",
            branch,
            "--label",
            &format!("launcher={launcher}"),
            "--label",
            "run=fixture-run",
            "--label",
            &format!("node={}", branch.replace('/', "-")),
        ],
    );
    let opened: Value = serde_json::from_str(opened.trim()).expect("session open answers JSON");
    (
        opened["token"].as_str().expect("a token").to_owned(),
        PathBuf::from(opened["worktree"].as_str().expect("a worktree")),
    )
}

/// Commit one file in `worktree` and answer the new tip.
fn commit(world: &World, worktree: &Path, file: &str, contents: &str, subject: &str) -> String {
    let path = worktree.join(file);
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
    std::fs::write(&path, contents).expect("the work is written");
    git(world, worktree, &["add", "-A"]);
    git(world, worktree, &["commit", "-q", "-m", subject]);
    git(world, worktree, &["rev-parse", "HEAD"])
        .trim()
        .to_owned()
}

/// Open, commit one file, close: a session that left its work preserved.
fn worked(
    world: &World,
    branch: &str,
    launcher: &str,
    file: &str,
    contents: &str,
    subject: &str,
) -> (String, String) {
    let (token, worktree) = open(world, branch, launcher);
    let tip = commit(world, &worktree, file, contents, subject);
    onevcs(world, &["session", "close", &token]);
    (token, tip)
}

/// Land `branch` onto the base through the real `onevcs publish-branch`.
fn land(world: &World, branch: &str) {
    onevcs(world, &["publish-branch", branch, "--repo", "service"]);
}

/// A session opened by **this process** through the linked library, and kept: its
/// owner is alive, so `onevcs` reports the branch held.
fn held_session(world: &World, repository: &Repository, branch: &str) -> onevcs::Session {
    let session = world.on_onevcs(|| {
        onevcs::Providers::real()
            .vcs
            .open_session(onevcs::SessionRequest {
                repo: repository.checkout.to_string_lossy().into_owned(),
                branch: Some(branch.to_owned()),
                branch_name: None,
                branch_prefix: None,
                base: None,
                execution_checkout: None,
                pool: None,
                overflow: None,
                labels: [
                    ("launcher".to_owned(), MANAGER.to_owned()),
                    ("run".to_owned(), "fixture-run".to_owned()),
                ]
                .into_iter()
                .collect(),
                refuse_conflicts: false,
            })
            .expect("a live session opens")
    });
    commit(
        world,
        &session.worktree,
        "live.txt",
        "live\n",
        "feat: live work",
    );
    session
}

/// `unpublished … --format json`, read back as the document.
fn listed(world: &World, args: &[&str]) -> (Run, Value) {
    let mut argv = vec!["unpublished", "--format", "json"];
    argv.extend_from_slice(args);
    let run = world.run(&argv);
    let document = serde_json::from_str(run.stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "`onepipeline {}` wrote no document ({error}):\n{}\n{}",
            argv.join(" "),
            run.stdout,
            run.stderr
        )
    });
    (run, document)
}

/// The row for `branch`, or a failure naming what was there.
fn row<'a>(document: &'a Value, branch: &str) -> &'a Value {
    document["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["branch"] == branch)
        .unwrap_or_else(|| panic!("no row for {branch}: {document:#}"))
}

/// Every branch the document lists.
fn branches(document: &Value) -> Vec<String> {
    document["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["branch"].as_str().expect("a branch").to_owned())
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Where `session`'s acknowledgements are kept by default under this world.
fn acknowledgement_file(world: &World, session: &str) -> PathBuf {
    world
        .state_home()
        .join("onepipeline/unpublished/acknowledged")
        .join(format!("{}.json", hex(&Sha256::digest(session.as_bytes()))))
}

/// Acknowledge `branch` for [`MANAGER`].
fn acknowledge(world: &World, branch: &str, reason: &str) -> Run {
    world.run(&[
        "unpublished",
        "--acknowledge",
        branch,
        "--reason",
        reason,
        "--session",
        MANAGER,
    ])
}

/// Every own branch landed, retirable, in flight or acknowledged at its tip owes
/// nothing — and another launcher's unlanded branch is never this manager's.
#[test]
fn a_session_whose_every_branch_is_landed_retirable_in_flight_or_acknowledged_owes_nothing() {
    let (world, repository) = host("unpub-none");
    worked(
        &world,
        "work/landed",
        MANAGER,
        "landed.txt",
        "landed\n",
        "feat: landed",
    );
    land(&world, "work/landed");
    worked(
        &world,
        "work/retirable",
        MANAGER,
        "same.txt",
        "same\n",
        "feat: same",
    );
    worked(
        &world,
        "work/retirable-twin",
        MANAGER,
        "same.txt",
        "same\n",
        "feat: same again",
    );
    land(&world, "work/retirable-twin");
    worked(
        &world,
        "work/acknowledged",
        MANAGER,
        "ack.txt",
        "ack\n",
        "feat: kept spike",
    );
    worked(
        &world,
        "work/theirs",
        OTHER,
        "theirs.txt",
        "theirs\n",
        "feat: theirs",
    );
    let _live = held_session(&world, &repository, "work/live");

    // Before the acknowledgement, the one unlanded branch is owed.
    let (run, before) = listed(&world, &["--session", MANAGER]);
    run.exited(COUNTED);
    assert_eq!(before["verdict"], "owed");
    assert_eq!(row(&before, "work/acknowledged")["counted"], true);

    acknowledge(&world, "work/acknowledged", "kept as the spike's evidence")
        .exited(NOTHING_COUNTED);

    let (run, document) = listed(&world, &["--session", MANAGER]);
    run.exited(NOTHING_COUNTED);
    assert_eq!(document["verdict"], "none", "{document:#}");
    assert_eq!(
        document["target"],
        json!({"kind": "session", "session": MANAGER})
    );
    let names = branches(&document);
    for absent in [
        "work/landed",
        "work/retirable",
        "work/retirable-twin",
        "work/theirs",
    ] {
        assert!(
            !names.contains(&absent.to_owned()),
            "{absent} is listed: {document:#}"
        );
    }
    let live = row(&document, "work/live");
    assert_eq!(live["in_flight"], true);
    assert_eq!(live["counted"], false);
    let acknowledged = row(&document, "work/acknowledged");
    assert_eq!(acknowledged["counted"], false);
    assert_eq!(
        acknowledged["acknowledgement"]["reason"],
        "kept as the spike's evidence"
    );
    assert_eq!(acknowledged["acknowledgement"]["tip"], acknowledged["tip"]);
    assert_eq!(acknowledged["manager_session"], MANAGER);
    assert_eq!(acknowledged["run"], "fixture-run");
    assert_eq!(acknowledged["disk"], Value::Null);

    // The other manager's branch is owed — to the other manager.
    let (run, theirs) = listed(&world, &["--session", OTHER]);
    run.exited(COUNTED);
    assert_eq!(branches(&theirs), vec!["work/theirs".to_owned()]);

    // And the text form says the same, with a summary line last.
    let text = world.run(&["unpublished", "--session", MANAGER]);
    text.exited(NOTHING_COUNTED);
    let last = text.stdout.lines().last().expect("a summary line");
    assert!(last.starts_with("none: 0 counted of 2"), "{}", text.stdout);
    assert!(text.stdout.contains("in flight"), "{}", text.stdout);
    assert!(text
        .stdout
        .contains("acknowledged: kept as the spike's evidence"));
}

/// Unlanded, `unknown` and `in-part` branches are each owed, and so is one
/// acknowledged at a tip it has since moved past.
#[test]
fn unlanded_unknown_in_part_and_moved_branches_are_owed() {
    let (world, _repository) = host("unpub-owed");
    worked(&world, "work/no", MANAGER, "no.txt", "no\n", "feat: no");
    worked(
        &world,
        "work/unknown",
        MANAGER,
        "u.txt",
        "mine\n",
        "feat: retry u",
    );
    worked(
        &world,
        "work/unknown-other",
        OTHER,
        "u.txt",
        "theirs\n",
        "feat: retry u",
    );
    land(&world, "work/unknown-other");
    worked(
        &world,
        "work/in-part",
        MANAGER,
        "p.txt",
        "one\n",
        "feat: part",
    );
    land(&world, "work/in-part");
    let (token, worktree) = open(&world, "work/in-part", MANAGER);
    commit(&world, &worktree, "p-more.txt", "two\n", "feat: part more");
    onevcs(&world, &["session", "close", &token]);
    worked(&world, "work/moved", MANAGER, "m.txt", "m\n", "feat: moved");
    acknowledge(&world, "work/moved", "left for later").exited(COUNTED);
    let (_, acknowledged) = listed(&world, &["--session", MANAGER]);
    assert_eq!(row(&acknowledged, "work/moved")["counted"], false);
    let (token, worktree) = open(&world, "work/moved", MANAGER);
    commit(&world, &worktree, "m2.txt", "more\n", "feat: moved on");
    onevcs(&world, &["session", "close", &token]);

    let (run, document) = listed(&world, &["--session", MANAGER]);
    run.exited(COUNTED);
    assert_eq!(document["verdict"], "owed", "{document:#}");
    for (branch, state) in [
        ("work/no", "no"),
        ("work/unknown", "unknown"),
        ("work/in-part", "in-part"),
        ("work/moved", "no"),
    ] {
        let row = row(&document, branch);
        assert_eq!(row["landed"]["state"], state, "{branch}: {row:#}");
        assert_eq!(row["counted"], true, "{branch}");
        assert_eq!(row["in_flight"], false, "{branch}");
        assert_eq!(row["acknowledgement"], Value::Null, "{branch}");
        assert_eq!(row["land_command"][0], "onepipeline", "{branch}");
    }
    // The acknowledgement is still on file; it simply no longer stands.
    let file: Value = serde_json::from_str(
        &std::fs::read_to_string(acknowledgement_file(&world, MANAGER)).expect("the file"),
    )
    .expect("JSON");
    assert_eq!(file["acknowledged"][0]["branch"], "work/moved");
    assert_ne!(
        file["acknowledged"][0]["tip"],
        row(&document, "work/moved")["tip"]
    );

    // The text form lists each counted branch with its landing and acknowledging
    // commands, and says no drafter is configured.
    let text = world.run(&["unpublished", "--session", MANAGER]);
    text.exited(COUNTED);
    for branch in ["work/no", "work/unknown", "work/in-part", "work/moved"] {
        assert!(
            text.stdout
                .contains(&format!("onepipeline unpublished --acknowledge {branch} ")),
            "{branch}:\n{}",
            text.stdout
        );
    }
    assert!(text.stdout.contains("land it:"), "{}", text.stdout);
    assert!(
        text.stdout.contains("no drafter is configured"),
        "{}",
        text.stdout
    );
    assert!(text
        .stdout
        .lines()
        .last()
        .expect("summary")
        .starts_with("owed: 4 counted"));
}

/// A branch a landed retry superseded is listed with its evidence and the
/// `onevcs reclaim` line first, and still counts.
#[test]
fn a_superseded_branch_is_listed_with_its_evidence_and_reclaim_line() {
    let (world, repository) = host("unpub-superseded");
    worked(&world, "work/first", MANAGER, "s.txt", "first\n", "feat: s");
    worked(
        &world,
        "work/first-retry",
        MANAGER,
        "s.txt",
        "second\n",
        "feat: s retried",
    );
    land(&world, "work/first-retry");
    let landing = git(&world, &repository.origin, &["rev-parse", "main"])
        .trim()
        .to_owned();
    onevcs(
        &world,
        &[
            "supersede",
            "work/first",
            "--repo",
            "service",
            "--by",
            "work/first-retry",
            "--landing",
            &landing,
            "--label",
            "node=n-first",
        ],
    );
    let (_, document) = listed(&world, &["--session", MANAGER]);
    let superseded = row(&document, "work/first");
    assert_eq!(superseded["retirement"]["class"], "superseded-with-changes");
    assert_eq!(superseded["counted"], true);
    let text = world.run(&["unpublished", "--session", MANAGER]);
    text.exited(COUNTED);
    let lines: Vec<&str> = text.stdout.lines().collect();
    let at = lines
        .iter()
        .position(|line| line.starts_with("work/first ["))
        .expect("the counted row's heading");
    assert!(lines[at + 1].contains("superseded by:   work/first-retry (node n-first)"));
    assert!(lines[at + 2].contains("differs in:      s.txt"));
    assert!(
        lines[at + 3].contains("reclaim it:      onevcs reclaim work/first --repo"),
        "{}",
        text.stdout
    );
}

/// A recovery read that fails, and a row returned without the session's
/// launcher label, are each `unanswered` — exit 1, never `none`.
#[test]
fn a_failed_read_and_a_filter_that_did_not_filter_are_unanswered() {
    let (world, _repository) = host("unpub-unanswered");
    // Another manager continued this manager's branch: the newest record naming
    // it is theirs, so the row the label filter returns carries their label.
    worked(&world, "work/shared", MANAGER, "a.txt", "a\n", "feat: a");
    worked(&world, "work/shared", OTHER, "b.txt", "b\n", "feat: b");
    let (run, document) = listed(&world, &["--session", MANAGER]);
    run.exited(UNANSWERED);
    assert_eq!(document["verdict"], "unanswered", "{document:#}");
    let said = document["unresolved"].to_string();
    assert!(said.contains("the filter did not filter"), "{said}");
    let text = world.run(&["unpublished", "--session", MANAGER]);
    text.exited(UNANSWERED).err_has("the filter did not filter");
    assert!(text
        .stdout
        .lines()
        .last()
        .expect("summary")
        .starts_with("unanswered:"));

    // The registry the read starts from cannot be read.
    std::fs::write(world.onevcs_home().join("registry.json"), "not json").expect("written");
    let (run, document) = listed(&world, &["--session", MANAGER]);
    run.exited(UNANSWERED);
    assert_eq!(document["verdict"], "unanswered");
    assert_eq!(document["rows"], json!([]));
    assert!(
        document["unresolved"][0]
            .as_str()
            .expect("a line")
            .starts_with("the recovery read failed"),
        "{document:#}"
    );
}

/// `--host` lists every identity; `--token` exactly the named sessions'; an
/// unknown token is refused as onevcs refuses it.
#[test]
fn the_host_and_token_targets_list_what_they_name() {
    let (world, _repository) = host("unpub-targets");
    world.extra_repository("engine");
    let (mine, _) = worked(&world, "work/mine", MANAGER, "a.txt", "a\n", "feat: a");
    worked(&world, "work/theirs", OTHER, "b.txt", "b\n", "feat: b");
    let opened = onevcs(
        &world,
        &[
            "session",
            "open",
            "engine",
            "--branch",
            "work/engine",
            "--label",
            "launcher=x",
        ],
    );
    let opened: Value = serde_json::from_str(opened.trim()).expect("JSON");
    commit(
        &world,
        Path::new(opened["worktree"].as_str().expect("worktree")),
        "e.txt",
        "e\n",
        "feat: e",
    );
    onevcs(
        &world,
        &["session", "close", opened["token"].as_str().expect("token")],
    );

    let (run, everything) = listed(&world, &["--host"]);
    run.exited(COUNTED);
    assert_eq!(everything["target"], json!({"kind": "host"}));
    let mut all = branches(&everything);
    all.sort();
    assert_eq!(all, ["work/engine", "work/mine", "work/theirs"]);
    let identities: std::collections::BTreeSet<&str> = everything["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["identity"].as_str().expect("identity"))
        .collect();
    assert_eq!(identities.len(), 2, "{everything:#}");

    let (run, named) = listed(&world, &["--token", &mine]);
    run.exited(COUNTED);
    assert_eq!(named["target"], json!({"kind": "tokens", "tokens": [mine]}));
    assert_eq!(branches(&named), ["work/mine"]);

    world
        .run(&["unpublished", "--token", "s-000000000000"])
        .exited(REFUSED)
        .err_has("no session record on this host names s-000000000000");
    // And alongside a token that is there, still refused rather than narrowed.
    world
        .run(&["unpublished", "--token", &mine, "--token", "s-000000000000"])
        .exited(REFUSED);
}

/// Allocated bytes of a fresh file of `size` bytes, as the filesystem reports them.
#[cfg(unix)]
fn allocated(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(path).expect("metadata").blocks() * 512
}

/// `--disk` reads a real worktree's run root and build output in allocated bytes,
/// and never follows a symbolic link out of it.
#[cfg(unix)]
#[test]
fn disk_reads_the_run_root_and_build_output_without_following_a_link() {
    let (world, _repository) = host("unpub-disk");
    let (_, worktree) = open(&world, "work/built", MANAGER);
    commit(&world, &worktree, "src.txt", "src\n", "feat: built");
    std::fs::create_dir_all(worktree.join("target/debug")).expect("target");
    std::fs::write(worktree.join("target/debug/artifact"), vec![7_u8; 200_000]).expect("a build");
    std::fs::create_dir_all(worktree.join("node_modules")).expect("node_modules");
    std::fs::write(worktree.join("node_modules/pkg.js"), vec![1_u8; 50_000]).expect("a module");
    // A link to a much larger tree outside the run root, under the worktree and
    // as a build-output name: neither is followed.
    let outside = world.root.join("outside");
    std::fs::create_dir_all(&outside).expect("outside");
    std::fs::write(outside.join("huge"), vec![9_u8; 4_000_000]).expect("a large file");
    std::os::unix::fs::symlink(&outside, worktree.join("linked")).expect("a link");
    std::os::unix::fs::symlink(&outside, worktree.join("dist")).expect("a linked dist");

    let (run, document) = listed(&world, &["--session", MANAGER, "--disk"]);
    run.exited(COUNTED);
    let disk = &row(&document, "work/built")["disk"];
    let run_root = worktree.parent().expect("the run root");
    assert_eq!(disk["run_root"], run_root.display().to_string());
    let target = allocated(&worktree.join("target/debug/artifact"));
    let modules = allocated(&worktree.join("node_modules/pkg.js"));
    assert_eq!(disk["build_output"]["target"], target);
    assert_eq!(disk["build_output"]["node_modules"], modules);
    assert!(disk["build_output"].get("dist").is_none(), "{disk}");
    let bytes = disk["run_root_bytes"].as_u64().expect("a reading");
    assert!(bytes >= target + modules, "{disk}");
    assert!(
        bytes < allocated(&outside.join("huge")),
        "the reading followed the link out of the run root: {disk}"
    );
    // Without `--disk`, no reading at all.
    let (_, plain) = listed(&world, &["--session", MANAGER]);
    assert_eq!(row(&plain, "work/built")["disk"], Value::Null);
}

/// The file's bytes, or `None` where it does not exist.
fn bytes_of(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

/// Every refusal `--acknowledge` makes is exit 2 with the file untouched — absent
/// when it did not exist, byte-for-byte when it did — and an acknowledgement
/// answers as the session listing does: 7 while another branch counts, 0 once none
/// does.
#[test]
fn acknowledge_refuses_before_writing_and_answers_as_the_session_listing() {
    let (world, _repository) = host("unpub-ack");
    world.extra_repository("engine");
    worked(&world, "work/a", MANAGER, "a.txt", "a\n", "feat: a");
    worked(&world, "work/b", MANAGER, "b.txt", "b\n", "feat: b");
    worked(&world, "work/twice", OTHER, "t.txt", "t\n", "feat: t");
    let opened = onevcs(
        &world,
        &["session", "open", "engine", "--branch", "work/twice"],
    );
    let opened: Value = serde_json::from_str(opened.trim()).expect("JSON");
    commit(
        &world,
        Path::new(opened["worktree"].as_str().expect("worktree")),
        "t.txt",
        "t2\n",
        "feat: t2",
    );
    onevcs(
        &world,
        &["session", "close", opened["token"].as_str().expect("token")],
    );
    let file = acknowledgement_file(&world, MANAGER);

    let refusals = |world: &World, expected: Option<&[u8]>| {
        for (args, said) in [
            (
                vec![
                    "--acknowledge",
                    "work/a",
                    "--reason",
                    "",
                    "--session",
                    MANAGER,
                ],
                "no visible",
            ),
            (
                vec![
                    "--acknowledge",
                    "work/a",
                    "--reason",
                    "  ",
                    "--session",
                    MANAGER,
                ],
                "no visible",
            ),
            (
                vec![
                    "--acknowledge",
                    "work/a",
                    "--reason",
                    "two\nlines",
                    "--session",
                    MANAGER,
                ],
                "unprintable",
            ),
            (
                vec![
                    "--acknowledge",
                    "work/nowhere",
                    "--reason",
                    "r",
                    "--session",
                    MANAGER,
                ],
                "no row",
            ),
            (
                vec![
                    "--acknowledge",
                    "work/twice",
                    "--reason",
                    "r",
                    "--session",
                    MANAGER,
                ],
                "--repo",
            ),
            (
                vec![
                    "--acknowledge",
                    "work/a",
                    "--reason",
                    "r",
                    "--session",
                    "  ",
                ],
                "no manager session",
            ),
        ] {
            let mut argv = vec!["unpublished"];
            argv.extend(args.iter().copied());
            world.run(&argv).exited(REFUSED).err_has(said);
            assert_eq!(
                bytes_of(&file).as_deref(),
                expected,
                "{argv:?} touched the file"
            );
        }
        // A session nothing identifies: no `--session`, and none in the environment.
        world
            .cmd(&["unpublished", "--acknowledge", "work/a", "--reason", "r"])
            .env("ONEPIPELINE_LAUNCHER_SESSION", "")
            .output()
            .map(|output| {
                assert_eq!(output.status.code(), Some(REFUSED));
                assert!(String::from_utf8_lossy(&output.stderr).contains("no manager session"));
            })
            .expect("the binary runs");
        assert_eq!(bytes_of(&file).as_deref(), expected);
    };
    refusals(&world, None);

    acknowledge(&world, "work/a", "kept on purpose").exited(COUNTED);
    let written = bytes_of(&file).expect("the file is written");
    refusals(&world, Some(&written));

    let last = acknowledge(&world, "work/b", "also kept");
    last.exited(NOTHING_COUNTED)
        .out_has("acknowledged work/b [")
        .out_has("none: 0 counted of 2");
    let document: Value =
        serde_json::from_slice(&bytes_of(&file).expect("the file")).expect("JSON");
    assert_eq!(document["version"], 1);
    let entries = document["acknowledged"].as_array().expect("entries");
    assert_eq!(entries.len(), 2, "{document:#}");
    for entry in entries {
        let keys: Vec<&String> = entry.as_object().expect("an entry").keys().collect();
        assert_eq!(keys, ["branch", "identity", "tip", "reason", "at"]);
    }

    // `--repo` names one of the two identities holding `work/twice`.
    let (_, host) = listed(&world, &["--host"]);
    let identity = host["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["branch"] == "work/twice" && row["manager_session"] == OTHER)
        .expect("the service copy")["identity"]
        .as_str()
        .expect("identity")
        .to_owned();
    world
        .run(&[
            "unpublished",
            "--acknowledge",
            "work/twice",
            "--reason",
            "theirs, seen",
            "--repo",
            &identity,
            "--session",
            MANAGER,
        ])
        .exited(NOTHING_COUNTED)
        .out_has(&format!("acknowledged work/twice [{identity}]"));
}

/// An acknowledgement file ai-orchestrator wrote — its version-1 format, its
/// two-space indentation — is honoured exactly as written.
#[test]
fn an_acknowledgement_file_the_consumer_wrote_is_honoured_as_written() {
    let (world, _repository) = host("unpub-adopted");
    let (_, tip) = worked(&world, "work/kept", MANAGER, "k.txt", "k\n", "feat: k");
    let (_, document) = listed(&world, &["--session", MANAGER]);
    let identity = row(&document, "work/kept")["identity"]
        .as_str()
        .expect("identity")
        .to_owned();
    // Byte for byte what `json.dumps(document, indent=2) + "\n"` writes.
    let written = format!(
        "{{\n  \"version\": 1,\n  \"acknowledged\": [\n    {{\n      \"branch\": \"work/kept\",\n      \
         \"identity\": \"{identity}\",\n      \"tip\": \"{tip}\",\n      \"reason\": \"kept by \
         the plan\",\n      \"at\": \"2026-09-30T08:15:00Z\"\n    }}\n  ]\n}}\n"
    );
    let file = acknowledgement_file(&world, MANAGER);
    std::fs::create_dir_all(file.parent().expect("parent")).expect("dir");
    std::fs::write(&file, &written).expect("written");

    let (run, document) = listed(&world, &["--session", MANAGER]);
    run.exited(NOTHING_COUNTED);
    assert_eq!(document["unresolved"], json!([]));
    assert_eq!(
        row(&document, "work/kept")["acknowledgement"],
        json!({"branch": "work/kept", "identity": identity, "tip": tip,
               "reason": "kept by the plan", "at": "2026-09-30T08:15:00Z"})
    );
    assert_eq!(std::fs::read_to_string(&file).expect("unchanged"), written);
}

/// A malformed entry, a duplicated pair and a file of another version each apply
/// no acknowledgement — the branch counts — and each says so.
#[test]
fn a_malformed_entry_a_duplicate_and_another_version_apply_nothing_and_say_so() {
    let (world, _repository) = host("unpub-faults");
    let (_, tip) = worked(&world, "work/kept", MANAGER, "k.txt", "k\n", "feat: k");
    let (_, document) = listed(&world, &["--session", MANAGER]);
    let identity = row(&document, "work/kept")["identity"].clone();
    let entry = |reason: &str, at: &str, tip: &str| json!({"branch": "work/kept", "identity": identity, "tip": tip, "reason": reason, "at": at});
    let file = acknowledgement_file(&world, MANAGER);
    std::fs::create_dir_all(file.parent().expect("parent")).expect("dir");
    for (document, said) in [
        (
            json!({"version": 1, "acknowledged": [entry("ok", "2026-09-30 08:15:00", &tip)]}),
            "was left out",
        ),
        (
            json!({"version": 1, "acknowledged": [entry("two\nlines", "2026-09-30T08:15:00Z", &tip)]}),
            "was left out",
        ),
        (
            json!({"version": 1, "acknowledged": [entry("ok", "2026-09-30T08:15:00Z", "XYZ")]}),
            "was left out",
        ),
        (
            json!({"version": 1, "acknowledged": [{"branch": "work/kept"}]}),
            "is not an acknowledgement",
        ),
        (
            json!({"version": 1, "acknowledged": [
                entry("one", "2026-09-30T08:15:00Z", &tip),
                entry("two", "2026-09-30T08:16:00Z", &tip)]}),
            "none of them applies",
        ),
        (
            json!({"version": 2, "acknowledged": [entry("ok", "2026-09-30T08:15:00Z", &tip)]}),
            "not a version 1 acknowledgement file",
        ),
    ] {
        std::fs::write(&file, document.to_string()).expect("written");
        let (run, listing) = listed(&world, &["--session", MANAGER]);
        run.exited(COUNTED);
        assert_eq!(row(&listing, "work/kept")["counted"], true, "{document}");
        assert!(
            listing["unresolved"].to_string().contains(said),
            "{document} did not say {said:?}: {listing:#}"
        );
        world
            .run(&["unpublished", "--session", MANAGER])
            .exited(COUNTED)
            .err_has(said);
    }
}

/// Set the modification time of everything under `root` to `ago` in the past,
/// without following a link.
#[cfg(unix)]
fn aged(root: &Path, ago: std::time::Duration) {
    let when = std::time::SystemTime::now() - ago;
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let meta = std::fs::symlink_metadata(&path).expect("metadata");
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            for entry in std::fs::read_dir(&path).expect("a directory") {
                pending.push(entry.expect("an entry").path());
            }
        }
        if let Ok(handle) = std::fs::File::open(&path) {
            let _ = handle.set_modified(when);
        }
    }
}

/// A claude-code `Stop` payload for `session`.
fn payload(session: &str, continuation: bool) -> String {
    json!({
        "session_id": session,
        "stop_hook_active": continuation,
        "hook_event_name": "Stop",
        "transcript_path": "/dev/null",
        "cwd": "/",
    })
    .to_string()
}

/// The guard's world: the host seen under the session a hook's environment
/// carries.
fn guard_world(owner: &World) -> World {
    owner.as_session(INHERITED)
}

/// `stop-guard --format claude-code` with `extra` flags over `session`'s payload.
fn stop(world: &World, extra: &[&str], session: &str, continuation: bool) -> Run {
    let mut argv = vec!["stop-guard", "--format", "claude-code"];
    argv.extend_from_slice(extra);
    world.run_with_stdin(&argv, &payload(session, continuation))
}

/// The block reason a claude-code rendering carries, or `None` for silence.
fn blocked(run: &Run) -> Option<String> {
    let trimmed = run.stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    let decision: Value = serde_json::from_str(trimmed).expect("one decision object");
    assert_eq!(decision["decision"], "block", "{decision}");
    Some(decision["reason"].as_str().expect("a reason").to_owned())
}

/// A closed, launcher-labelled branch preserved on its origin and swept past the
/// age floor is still this manager's: the listing owes it and the native Stop hook
/// blocks naming it, with the real session and its labels still on record.
#[cfg(unix)]
#[test]
fn a_swept_preserved_branch_still_blocks_its_manager() {
    let (world, _repository) = host("unpub-swept");
    let (token, _) = worked(&world, "work/preserved", MANAGER, "p.txt", "p\n", "feat: p");
    onevcs(&world, &["preserve", "work/preserved", "--repo", "service"]);
    aged(
        &world.onevcs_home(),
        std::time::Duration::from_secs(5 * 3600),
    );
    onevcs(&world, &["sweep", "--min-age-hours", "4"]);

    let holders: Value =
        serde_json::from_str(onevcs(&world, &["session", "holders", "service", "--json"]).trim())
            .expect("holders JSON");
    let record = holders
        .as_array()
        .expect("holders")
        .iter()
        .find(|holder| holder["token"] == token.as_str())
        .unwrap_or_else(|| panic!("the swept session's record is gone: {holders:#}"));
    assert_eq!(record["labels"]["launcher"], MANAGER);

    let (run, document) = listed(&world, &["--session", MANAGER]);
    run.exited(COUNTED);
    assert_eq!(row(&document, "work/preserved")["counted"], true);
    let guard = guard_world(&world);
    let reason = blocked(
        &stop(&guard, &["--unpublished"], MANAGER, false)
            .exited(0)
            .clone_run(),
    )
    .expect("the guard blocks");
    assert!(reason.contains("work/preserved"), "{reason}");
}

/// A borrowed run's fields, so a journey can keep one after asserting on it.
trait CloneRun {
    fn clone_run(&self) -> Run;
}

impl CloneRun for Run {
    fn clone_run(&self) -> Run {
        Run {
            code: self.code,
            stdout: self.stdout.clone(),
            stderr: self.stderr.clone(),
            args: self.args.clone(),
            world: self.world.clone(),
        }
    }
}

/// The native Stop hook: a block naming the owed branch; silence once nothing is
/// owed; a block — never a warning or silence — when the read is unanswered; a
/// continuation over an unchanged listing standing aside; and without
/// `--unpublished`, exactly the guard it was before.
#[test]
fn the_stop_hook_blocks_on_owed_and_unanswered_and_is_silent_on_none() {
    let (world, _repository) = host("unpub-guard");
    let guard = guard_world(&world);
    // Nothing owed yet.
    let quiet = stop(&guard, &["--unpublished"], MANAGER, false);
    quiet.exited(0);
    assert_eq!(quiet.stdout, "", "{}", quiet.stderr);

    worked(&world, "work/owed", MANAGER, "o.txt", "o\n", "feat: o");
    let first = stop(&guard, &["--unpublished"], MANAGER, false);
    first.exited(0);
    let reason = blocked(&first).expect("an owed branch blocks");
    assert!(reason.contains("work/owed"), "{reason}");
    assert!(reason.contains("owed: 1 counted"), "{reason}");
    let listing = world.run(&["unpublished", "--session", MANAGER]);
    assert_eq!(reason, listing.stdout, "the reason is the text listing");
    let memory = world
        .state_home()
        .join("onepipeline/stop-guard")
        .join(format!(
            "{}.unpublished",
            hex(&Sha256::digest(MANAGER.as_bytes()))
        ));
    assert!(memory.is_file(), "the block was not remembered");

    // A continuation over the same listing stands aside.
    let again = stop(&guard, &["--unpublished"], MANAGER, true);
    again.exited(0);
    assert_eq!(again.stdout, "");

    // Without `--unpublished`, the guard is what it always was: nothing this
    // session owns is unwatched, so it is silent — byte for byte the empty output
    // of a host with no preserved branch at all.
    let (empty, _repository) = host("unpub-guard-empty");
    let empty_guard = guard_world(&empty);
    for guard in [&guard, &empty_guard] {
        let plain = stop(guard, &[], MANAGER, false);
        plain.exited(0);
        assert_eq!(plain.stdout, "");
        assert_eq!(plain.stderr, "");
    }

    // Acknowledged, nothing is owed: silence, and the memory is removed.
    acknowledge(&world, "work/owed", "kept").exited(NOTHING_COUNTED);
    let none = stop(&guard, &["--unpublished"], MANAGER, false);
    none.exited(0);
    assert_eq!(none.stdout, "");
    assert!(!memory.exists());

    // An unreadable registry is unanswered, which blocks naming why.
    std::fs::write(world.onevcs_home().join("registry.json"), "not json").expect("written");
    let unanswered = stop(&guard, &["--unpublished"], MANAGER, false);
    unanswered.exited(0);
    let reason = blocked(&unanswered).expect("an unanswered read blocks");
    assert!(reason.contains("is unanswered"), "{reason}");
    assert!(reason.contains("the recovery read failed"), "{reason}");
    assert!(!unanswered.stdout.contains("systemMessage"));
    let continued = stop(&guard, &["--unpublished"], MANAGER, true);
    assert_eq!(
        continued.stdout, "",
        "an unchanged unanswered reason stands aside"
    );
}

/// A run nothing watches, held open by its dispatch.
fn held_run(world: &World, name: &str) -> String {
    crate::stop_guard::held(world, name)
}

/// The two halves combine strongest-wins: either one's block refuses the stop,
/// and with both, the reason names both.
#[test]
fn the_unpublished_decision_and_unwatched_combine_strongest_wins() {
    let (world, _repository) = host("unpub-combine");
    world.script("build.wait", "hold");
    let session = world.session.clone();
    let guard = guard_world(&world);

    // Owed, nothing unwatched: the branch.
    worked(&world, "work/owed", &session, "o.txt", "o\n", "feat: o");
    let reason = blocked(
        &stop(&guard, &["--unpublished"], &session, false)
            .exited(0)
            .clone_run(),
    )
    .expect("blocks");
    assert!(reason.contains("work/owed"), "{reason}");

    // Both: the run and the branch.
    let run = held_run(&world, "unpubcombine");
    let reason = blocked(
        &stop(&guard, &["--unpublished"], &session, false)
            .exited(0)
            .clone_run(),
    )
    .expect("blocks");
    assert!(
        reason.contains("work/owed") && reason.contains(&run),
        "{reason}"
    );

    // Nothing owed, the run unwatched: still the run.
    acknowledge_for(&world, &session, "work/owed");
    let reason = blocked(
        &stop(&guard, &["--unpublished"], &session, false)
            .exited(0)
            .clone_run(),
    )
    .expect("the unwatched run still blocks");
    assert!(
        reason.contains(&run) && !reason.contains("work/owed"),
        "{reason}"
    );
    world.release("build.go");
}

fn acknowledge_for(world: &World, session: &str, branch: &str) {
    world
        .run(&[
            "unpublished",
            "--acknowledge",
            branch,
            "--reason",
            "seen",
            "--session",
            session,
        ])
        .exited(NOTHING_COUNTED);
}

/// What one traced process did: the threads and processes it created, and the
/// programs it executed.
#[cfg(target_os = "linux")]
#[derive(Default)]
struct Traced {
    threads: Vec<u32>,
    children: Vec<u32>,
    executed: Vec<String>,
}

/// Run `argv` under `strace -ff`, one trace file per thread or process, and answer
/// the guard's program name and the programs its **direct** children executed.
///
/// Parentage is the point: `git` starts helpers of its own — a shell for a local
/// transport, `git-upload-pack` — and those are `git`'s, not the guard's.
#[cfg(target_os = "linux")]
fn spawned_by_the_guard(
    world: &World,
    argv: &[&str],
    stdin: &str,
    into: &Path,
) -> (String, Vec<String>) {
    use std::io::Write;
    std::fs::create_dir_all(into).expect("a trace directory");
    let prefix = into.join("trace");
    let inner = world.cmd(argv);
    let mut traced = std::process::Command::new("strace");
    traced
        .args([
            "-ff",
            "-qq",
            "-e",
            "trace=execve,clone,clone3,fork,vfork",
            "-o",
        ])
        .arg(&prefix)
        .arg(inner.get_program())
        .args(inner.get_args())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for (key, value) in inner.get_envs() {
        match value {
            Some(value) => traced.env(key, value),
            None => traced.env_remove(key),
        };
    }
    let mut child = traced.spawn().unwrap_or_else(|error| {
        panic!(
            "this journey's claim is what the guard spawned, and the tracer would not run: \
             strace: {error}. Install strace, or run the suite where ptrace is permitted."
        )
    });
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("the payload is written");
    let output = child.wait_with_output().expect("the tracer runs");
    assert!(output.status.success(), "{output:?}");
    let mut processes: std::collections::BTreeMap<u32, Traced> = Default::default();
    for entry in std::fs::read_dir(into).expect("the traces") {
        let path = entry.expect("a trace").path();
        let Some(pid) = path
            .extension()
            .and_then(|pid| pid.to_str())
            .and_then(|pid| pid.parse::<u32>().ok())
        else {
            continue;
        };
        let mut traced = Traced::default();
        for line in std::fs::read_to_string(&path).expect("a trace").lines() {
            let result = line
                .rsplit_once("= ")
                .and_then(|(_, n)| n.trim().parse::<u32>().ok());
            if line.starts_with("execve(\"") && line.trim_end().ends_with("= 0") {
                let program = line.split('"').nth(1).expect("a path");
                traced.executed.push(
                    Path::new(program)
                        .file_name()
                        .expect("a file name")
                        .to_string_lossy()
                        .into_owned(),
                );
            } else if ["clone(", "clone3(", "fork(", "vfork("]
                .iter()
                .any(|call| line.starts_with(call))
            {
                if let Some(created) = result {
                    if line.contains("CLONE_THREAD") {
                        traced.threads.push(created);
                    } else {
                        traced.children.push(created);
                    }
                }
            }
        }
        processes.insert(pid, traced);
    }
    let guard_program = Path::new(inner.get_program())
        .file_name()
        .expect("a file name")
        .to_string_lossy()
        .into_owned();
    let root = processes
        .iter()
        .find(|(_, traced)| traced.executed.first() == Some(&guard_program))
        .map(|(pid, _)| *pid)
        .unwrap_or_else(|| panic!("no traced process executed {guard_program}"));
    let mut group = vec![root];
    let mut at = 0;
    while at < group.len() {
        let threads = processes[&group[at]].threads.clone();
        group.extend(threads);
        at += 1;
    }
    let mut programs = Vec::new();
    for member in &group {
        for child in &processes[member].children {
            if let Some(child) = processes.get(child) {
                programs.extend(child.executed.iter().cloned());
            }
        }
    }
    (guard_program, programs)
}

/// Through the real guard the unpublished decision runs in the guard's own
/// process: the only programs it starts are the `git` the linked onevcs read runs —
/// no shell, no Python, no `onepipeline`, no helper.
#[cfg(target_os = "linux")]
#[test]
fn the_guard_decides_in_its_own_process_starting_only_git() {
    let (world, _repository) = host("unpub-traced");
    worked(&world, "work/owed", MANAGER, "o.txt", "o\n", "feat: o");
    let guard = guard_world(&world);
    let _ = std::fs::remove_dir_all(world.onevcs_home().join("cache"));
    let argv = ["stop-guard", "--format", "claude-code", "--unpublished"];
    let (_, programs) = spawned_by_the_guard(
        &guard,
        &argv,
        &payload(MANAGER, false),
        &world.root.join("guard-trace"),
    );
    assert!(
        programs.iter().any(|program| program == "git"),
        "the positive control: the onevcs read runs git, and the trace saw none: {programs:?}"
    );
    let others: Vec<&String> = programs.iter().filter(|p| *p != "git").collect();
    assert!(
        others.is_empty(),
        "the guard started something other than git: {others:?}"
    );
    println!("  the guard's direct children executed: {programs:?}");
}

/// Make `path` a FIFO whose one reader is answered `contents` only `delay` after it
/// opened it — a read that is measurably slow because the reader waits on it, with
/// nothing substituted. Answers the thread that writes it.
#[cfg(unix)]
fn slow_file(
    path: &Path,
    contents: String,
    delay: std::time::Duration,
) -> std::thread::JoinHandle<()> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    let _ = std::fs::remove_file(path);
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).expect("a C path");
    // SAFETY: `mkfifo` reads the NUL-terminated path and nothing else.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0, "mkfifo");
    let path = path.to_path_buf();
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        let mut file = loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&path)
            {
                Ok(file) => break file,
                Err(_) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(error) => panic!("nothing opened {} to read: {error}", path.display()),
            }
        };
        std::thread::sleep(delay);
        use std::io::Write;
        file.write_all(contents.as_bytes())
            .expect("the answer is written");
    })
}

/// `stop-guard --format claude-code --unpublished` fed `stdin`, as exit code,
/// stdout and stderr, with nothing read from the world afterwards.
#[cfg(unix)]
fn undumped(world: &World, stdin: &str) -> (i32, String, String) {
    use std::io::Write;
    let mut child = world
        .cmd(&["stop-guard", "--format", "claude-code", "--unpublished"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the guard starts");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("the payload is written");
    let output = child.wait_with_output().expect("the guard runs");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// Through the real guard the unpublished decision and the `unwatched` check run
/// concurrently: with each half made measurably slow, the guard's wall time follows
/// the slower half rather than their sum.
#[cfg(unix)]
#[test]
fn the_two_halves_run_concurrently() {
    const DELAY: std::time::Duration = std::time::Duration::from_secs(2);
    let (world, _repository) = host("unpub-concurrent");
    worked(&world, "work/owed", MANAGER, "o.txt", "o\n", "feat: o");
    let guard = guard_world(&world);
    let launch = world.runs.join("slowrun").join("launch.json");
    let acknowledgements = acknowledgement_file(&world, MANAGER);
    let empty = r#"{"version":1,"acknowledged":[]}"#.to_owned();
    let timed = |unwatched: bool, unpublished: bool| {
        let writers: Vec<_> = [
            unwatched.then(|| slow_file(&launch, "{}".to_owned(), DELAY)),
            unpublished.then(|| slow_file(&acknowledgements, empty.clone(), DELAY)),
        ]
        .into_iter()
        .flatten()
        .collect();
        let started = std::time::Instant::now();
        // Run directly rather than through `World::run`, whose record of the world
        // afterwards would read the FIFO a second time with nobody left to answer.
        let output = undumped(&guard, &payload(MANAGER, false));
        let took = started.elapsed();
        assert_eq!(output.0, 0);
        let run = Run {
            code: output.0,
            stdout: output.1,
            stderr: output.2,
            args: "stop-guard --format claude-code --unpublished".to_owned(),
            world: String::new(),
        };
        assert!(blocked(&run).is_some_and(|reason| reason.contains("work/owed")));
        for writer in writers {
            writer.join().expect("the writer finished");
        }
        let _ = std::fs::remove_file(&launch);
        let _ = std::fs::remove_file(&acknowledgements);
        took
    };
    let base = timed(false, false);
    let unwatched = timed(true, false);
    let unpublished = timed(false, true);
    let both = timed(true, true);
    println!(
        "  base {base:?}, slow unwatched {unwatched:?}, slow unpublished {unpublished:?}, both \
         {both:?}"
    );
    assert!(
        unwatched >= DELAY && unpublished >= DELAY,
        "each half was made slow"
    );
    assert!(both >= DELAY, "{both:?}");
    assert!(
        both < unwatched + unpublished - base - DELAY / 2,
        "the guard took {both:?}, the sum of its halves rather than the slower one: unwatched \
         {unwatched:?}, unpublished {unpublished:?}"
    );
}

/// Split a line a shell would read into its words: bare words, and words in
/// single quotes with `'\''` for a quote — the two forms the listing prints.
fn shell_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut started = false;
    for c in line.chars() {
        match (quoted, c) {
            (false, '\'') | (true, '\'') => {
                quoted = !quoted;
                started = true;
            }
            (false, '\\') => {}
            (false, ' ') => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (_, c) => {
                word.push(c);
                started = true;
            }
        }
    }
    if started {
        words.push(word);
    }
    words
}

/// The `land it:` command printed for `branch` in a text listing.
fn land_line(text: &str, branch: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let at = lines
        .iter()
        .position(|line| line.starts_with(&format!("{branch} [")))
        .unwrap_or_else(|| panic!("no counted heading for {branch}:\n{text}"));
    let line = lines[at + 1..]
        .iter()
        .find_map(|line| line.trim_start().strip_prefix("land it:"))
        .unwrap_or_else(|| panic!("no landing command for {branch}:\n{text}"));
    shell_words(line.trim())
}

/// Every printed landing command runs this host's drafter or declines it out loud:
/// none is a bare onevcs `publish-branch`/`recover`, and none is a onepipeline
/// publication verb without `--pr-author-graph` or `--no-draft`.
fn assert_every_landing_drafts_or_declines(text: &str) {
    for line in text.lines() {
        let Some(command) = line.trim_start().strip_prefix("land it:") else {
            continue;
        };
        let words = shell_words(command.trim());
        assert!(
            !(words[0] == "onevcs" && ["publish-branch", "recover"].contains(&words[1].as_str())),
            "a bare onevcs landing: {line}"
        );
        if words[0] == "onepipeline" {
            assert!(
                words
                    .iter()
                    .any(|w| w == "--pr-author-graph" || w == "--no-draft"),
                "a publication verb with neither flag: {line}"
            );
        }
    }
}

/// A printed landing command, run exactly as printed from another directory, opens
/// the change request with the drafter's body — through `unpublished
/// --pr-author-graph` and through `stop-guard --unpublished-pr-author-graph`. Without a
/// graph it declines the draft and the listing says so.
#[test]
fn a_printed_landing_command_runs_this_hosts_drafter() {
    let world = World::new("unpub-drafted");
    world.repository("change-open", &[]);
    worked(&world, "work/first", MANAGER, "f.txt", "f\n", "feat: first");
    worked(
        &world,
        "work/second",
        MANAGER,
        "s.txt",
        "s\n",
        "feat: second",
    );
    world.script(
        "pr-author.body",
        "## What\nDrafted for the unpublished branch.\n",
    );
    let graph = PathBuf::from(world.pr_author_graph());
    let graphs = graph.parent().expect("the graph's directory").to_path_buf();
    let elsewhere = world.root.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).expect("another directory");

    // Without a graph: `--no-draft`, said out loud.
    let plain = world.run(&["unpublished", "--session", MANAGER]);
    plain.exited(COUNTED).out_has("no drafter is configured");
    assert!(land_line(&plain.stdout, "work/first").contains(&"--no-draft".to_owned()));
    assert_every_landing_drafts_or_declines(&plain.stdout);

    // A graph that is not a readable file is refused before anything is read.
    world
        .run(&[
            "unpublished",
            "--session",
            MANAGER,
            "--pr-author-graph",
            "no-such.yaml",
        ])
        .exited(REFUSED)
        .err_has("not a readable file");

    // Named relatively from the graph's own directory, printed absolute.
    let listing = world.run_from(
        &graphs,
        &[
            "unpublished",
            "--session",
            MANAGER,
            "--pr-author-graph",
            "pr-author.yaml",
        ],
    );
    listing.exited(COUNTED);
    assert_every_landing_drafts_or_declines(&listing.stdout);
    let printed = land_line(&listing.stdout, "work/first");
    assert_eq!(
        &printed[..2],
        ["onepipeline", "publish-branch"],
        "{printed:?}"
    );
    assert_eq!(
        printed[printed.len() - 2..],
        ["--pr-author-graph".to_owned(), graph.display().to_string()]
    );
    let ran = world.run_from(
        &elsewhere,
        &printed[1..].iter().map(String::as_str).collect::<Vec<_>>(),
    );
    ran.exited(0);

    // And through the guard.
    let guard = guard_world(&world);
    let graph_flag = graph.display().to_string();
    let reason = blocked(
        &stop(
            &guard,
            &[
                "--unpublished",
                "--unpublished-pr-author-graph",
                &graph_flag,
            ],
            MANAGER,
            false,
        )
        .exited(0)
        .clone_run(),
    )
    .expect("the second branch is still owed");
    assert_every_landing_drafts_or_declines(&reason);
    let printed = land_line(&reason, "work/second");
    assert!(
        printed.contains(&"--pr-author-graph".to_owned()),
        "{printed:?}"
    );
    world
        .run_from(
            &elsewhere,
            &printed[1..].iter().map(String::as_str).collect::<Vec<_>>(),
        )
        .exited(0);

    let opened = world.changes_opened();
    assert_eq!(opened.len(), 2, "{opened:#?}");
    for change in &opened {
        assert_eq!(
            change["body"], "## What\nDrafted for the unpublished branch.",
            "{change:#}"
        );
    }
}

/// The synopsis entry 113 and `docs/stop-guard.md` state is the verb's own.
#[test]
fn the_documented_synopsis_is_the_verbs_own() {
    let world = World::new("unpub-synopsis");
    let help = world.run(&["unpublished", "--help"]);
    help.exited(0);
    let page =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/stop-guard.md"))
            .expect("the page");
    let stated: String = page
        .lines()
        .filter(|line| line.starts_with("onepipeline unpublished "))
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(
        crate::stop_guard::flags_of(&stated),
        crate::stop_guard::flags_of(&help.stdout),
        "{stated}\n{}",
        help.stdout
    );
    let register = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/contract-divergences.md"),
    )
    .expect("the register");
    let entry = register.split("\n## 113.").nth(1).expect("entry 113");
    let proposed: String = entry
        .split("**Proposal")
        .nth(1)
        .and_then(|rest| rest.split("and amend entry 85").next())
        .expect("the proposal")
        .to_owned();
    assert_eq!(
        crate::stop_guard::flags_of(&proposed),
        crate::stop_guard::flags_of(&help.stdout),
        "{proposed}"
    );
}
