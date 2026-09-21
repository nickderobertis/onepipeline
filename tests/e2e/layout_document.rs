//! The published planner-channel document, linked by a host's own bus.
//!
//! A host runs the `onemessagebus` CLI beside this engine, and that CLI links no
//! code of this crate's: it reads the channel through the layout document this
//! repository commits, named by path in its configuration's `schemas` key. These
//! journeys drive the CLI the workspace pins — the release `Cargo.lock` links —
//! over a channel directory the compiled binary wrote, and hold what it reads
//! and writes to what the engine reads and writes there.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

use crate::harness::{agent, plan_of, World};

/// The repository's own checkout, where the committed document and the pinned
/// CLI both live.
fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The `onemessagebus` CLI `package.json` pins, which `npm ci` installs.
///
/// Refused rather than skipped when it is absent: a journey that passed without
/// the CLI would prove nothing about what a host's bus reads.
fn bus_cli() -> PathBuf {
    let shims = repository().join("node_modules").join(".bin");
    // npm writes both shims everywhere; only the `.cmd` one is a program Windows
    // runs, and only the extensionless one is a program a unix shell does.
    #[cfg(windows)]
    let names = ["onemessagebus.cmd", "onemessagebus"];
    #[cfg(not(windows))]
    let names = ["onemessagebus", "onemessagebus.cmd"];
    names
        .into_iter()
        .map(|name| shims.join(name))
        .find(|shim| shim.exists())
        .unwrap_or_else(|| {
            panic!(
                "the pinned onemessagebus CLI is not installed under {}. ACTION: run `npm ci` \
                 (or `just bootstrap`) in the repository root",
                shims.display()
            )
        })
}

/// A bus configuration linking the committed document by path, pinned to the
/// major it declares — the configuration a host writes, with a path where the
/// host names the tagged URL.
fn configuration(world: &World) -> PathBuf {
    let document = repository().join(onepipeline::channel::layout::DOCUMENT_PATH);
    let major = onepipeline::channel::layout::DOCUMENT_VERSION
        .split('.')
        .next()
        .expect("a version has a major");
    let path = world.root.join("onemessagebus.yaml");
    std::fs::write(
        &path,
        format!(
            "version: 1\ntransport:\n  kind: local\nprofile: {}\nschemas:\n  - {}\n",
            onepipeline::channel::layout::PLANNER_CHANNEL,
            serde_json::to_string(&format!("{}@{major}", document.display()))
                .expect("a link is text"),
        ),
    )
    .expect("the configuration is written");
    path
}

/// One CLI invocation over `channel`, answering its stdout; any other exit is
/// a failure naming what it said.
fn bus(world: &World, channel: &Path, args: &[&str], stdin: Option<&Value>) -> String {
    let mut command = Command::new(bus_cli());
    command
        .args(args)
        .arg("--config")
        .arg(configuration(world))
        .arg("--transport-dir")
        .arg(channel)
        // A cache of this world's own, so no host-wide state is read or left.
        .env(
            "ONEMESSAGEBUS_SCHEMA_CACHE_DIR",
            world.root.join("bus-cache"),
        )
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().expect("the bus CLI runs");
    {
        use std::io::Write as _;
        let mut input = child.stdin.take().expect("a stdin");
        if let Some(record) = stdin {
            input
                .write_all(record.to_string().as_bytes())
                .expect("the record is written");
        }
    }
    let output = child.wait_with_output().expect("the bus CLI ends");
    assert!(
        output.status.success(),
        "`onemessagebus {}` exited {:?}\nstdout: {}\nstderr: {}",
        args.join(" "),
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("the CLI writes text")
}

/// Every file of the channel layout `dir` holds, by name.
fn files(dir: &Path) -> Vec<(&'static str, Option<Vec<u8>>)> {
    onepipeline::channel::layout::FILES
        .iter()
        .map(|file| (*file, std::fs::read(dir.join(file)).ok()))
        .collect()
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("a copy directory");
    for (file, bytes) in files(from) {
        if let Some(bytes) = bytes {
            std::fs::write(to.join(file), bytes).expect("a channel file copies");
        }
    }
}

/// The last `queued` surface the engine's log holds, as it was offered: the
/// record less the event the log stamps it with and the id the queue allocates.
fn last_queued(channel: &Path) -> Value {
    let log = std::fs::read_to_string(channel.join("surfaces.jsonl")).expect("a surface log");
    let mut record: Value = log
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|line| line["event"] == json!("queued"))
        .expect("the engine queued a surface");
    let fields = record.as_object_mut().expect("a surface is an object");
    fields.shift_remove("event");
    fields.shift_remove("id");
    record
}

/// The pinned bus CLI, linking the committed document by path, reads the
/// channel the engine wrote without changing a byte of it; a surface it writes
/// is byte for byte the one the engine wrote for the same record; and one it
/// raises is read by the engine as any other surface.
#[test]
fn the_bus_cli_linking_the_published_document_reads_and_writes_the_channel_the_engine_wrote() {
    let world = World::new("layout-document");
    world.script("build.wait", "hold");
    let run = "linked";
    let path = world.plan(run, &plan_of(run, vec![agent("build", &[])]));
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the run to dispatch something", |world| {
        !world.events_of(run, "node-dispatched").is_empty()
    });
    let channel = world.run_file(run, "channel");
    let version = bus(&world, &channel, &["--version"], None);
    let locked = std::fs::read_to_string(repository().join("Cargo.lock"))
        .expect("the lock")
        .split("[[package]]")
        .find(|package| {
            package
                .lines()
                .any(|line| line == "name = \"onemessagebus\"")
        })
        .and_then(|package| {
            package
                .lines()
                .find_map(|line| line.strip_prefix("version = \""))
                .map(|version| version.trim_end_matches('"').to_owned())
        })
        .expect("the lock resolves the bus");
    assert_eq!(
        version.trim(),
        format!("onemessagebus {locked}"),
        "the pinned CLI is not the bus release this build links"
    );

    world
        .run(&[
            "surface",
            run,
            "--kind",
            "finding",
            "--message",
            "from the engine",
        ])
        .exited(0);

    // Read: the CLI answers what the engine's projection holds, and the read
    // leaves every file as the engine wrote it.
    let before = files(&channel);
    let status: Value = serde_json::from_str(&bus(
        &world,
        &channel,
        &["status", onepipeline::channel::layout::SURFACES],
        None,
    ))
    .expect("the status is JSON");
    let projection: Value = serde_json::from_slice(
        &std::fs::read(channel.join(onepipeline::channel::layout::PROJECTION))
            .expect("the engine wrote its projection"),
    )
    .expect("the projection is JSON");
    let status = &status[0];
    assert_eq!(
        status["queue"],
        json!(onepipeline::channel::layout::SURFACES),
        "{status}"
    );
    assert_eq!(status["waiting"], projection["waiting"], "{status}");
    assert_eq!(status["unread"], json!(1), "{status}");
    assert_eq!(
        files(&channel),
        before,
        "reading through the CLI changed the channel"
    );

    // Write: the engine and the CLI each append the same surface to the same
    // channel, and the two directories come out byte for byte the same.
    let host = world.root.join("host-channel");
    copy(&channel, &host);
    world
        .run(&[
            "surface",
            run,
            "--kind",
            "finding",
            "--message",
            "the second",
        ])
        .exited(0);
    let offered = last_queued(&channel);
    bus(
        &world,
        &host,
        &["send", onepipeline::channel::layout::SURFACES],
        Some(&offered),
    );
    for ((file, engine), (_, cli)) in files(&channel).into_iter().zip(files(&host)) {
        assert!(
            engine == cli,
            "{file} differs:\n--- the engine wrote\n{}\n--- the CLI wrote\n{}",
            String::from_utf8_lossy(engine.as_deref().unwrap_or_default()),
            String::from_utf8_lossy(cli.as_deref().unwrap_or_default())
        );
    }

    // And what the CLI raises on the engine's own channel, the engine reads.
    bus(
        &world,
        &channel,
        &["send", onepipeline::channel::layout::SURFACES],
        Some(&json!({
            "kind": "finding",
            "message": "from the host",
            "source": "proposal",
            "blocking": false,
            "about": "build",
        })),
    );
    let mut read = Vec::new();
    for _ in 0..3 {
        let next = world.run(&["next", run]);
        next.exited(0);
        read.push(next.json()["surface"].clone());
    }
    let from_host = read
        .iter()
        .find(|surface| surface["message"] == json!("from the host"))
        .unwrap_or_else(|| panic!("the engine never read the CLI's surface: {read:?}"));
    assert_eq!(from_host["workstream"], json!("build"), "{from_host}");
    assert!(from_host["queued_at"].is_u64(), "{from_host}");

    world.release("build.go");
}

/// A configuration pinned to a major the document does not declare is refused
/// by the CLI naming the pin, so a host that pins its link reads only the
/// document it pinned.
#[test]
fn a_link_pinned_past_the_documents_version_is_refused_by_the_bus_cli() {
    let world = World::new("layout-document-pin");
    let channel = world.root.join("channel");
    std::fs::create_dir_all(&channel).expect("a channel directory");
    let document = repository().join(onepipeline::channel::layout::DOCUMENT_PATH);
    let major: u64 = onepipeline::channel::layout::DOCUMENT_VERSION
        .split('.')
        .next()
        .and_then(|major| major.parse().ok())
        .expect("a numeric major");
    let config = world.root.join("pinned-past.yaml");
    std::fs::write(
        &config,
        format!(
            "version: 1\ntransport:\n  kind: local\nprofile: planner-channel\nschemas:\n  - {}\n",
            serde_json::to_string(&format!("{}@{}", document.display(), major + 1))
                .expect("a link is text"),
        ),
    )
    .expect("the configuration is written");
    let output = Command::new(bus_cli())
        .args(["status", "--config"])
        .arg(&config)
        .arg("--transport-dir")
        .arg(&channel)
        .env(
            "ONEMESSAGEBUS_SCHEMA_CACHE_DIR",
            world.root.join("bus-cache"),
        )
        .output()
        .expect("the bus CLI runs");
    assert!(
        !output.status.success(),
        "a pin the document fails was read"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(onepipeline::channel::layout::DOCUMENT_VERSION),
        "the refusal does not name the version the document declares: {stderr}"
    );
}
