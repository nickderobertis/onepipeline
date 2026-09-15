//! A channel directory `onepipeline` 0.28.2 wrote, read by this build.
//!
//! The planner channel's promise is that nothing queued is lost, nothing pending
//! is handed out twice, and a reply reaches the reader it is for. Holding that
//! across a change of implementation means holding it over what a real run left
//! on disk, not over directories this build wrote for itself. So each journey
//! seeds a world with one of the recorded directories under
//! `tests/recorded/channel/` (their README names the run each came from), drives
//! the verbs a supervisor reads a run with, and holds every answer, the
//! re-derived `queue.json`, and every channel file those verbs leave behind to
//! what the 0.28.2 binary answered over the same world. One journey runs the
//! other way: it writes a channel from empty with the verbs that write one, and
//! holds the files this build leaves to the files 0.28.2 leaves.
//!
//! Those answers are checked in under `tests/recorded/answers/`, and
//! `scripts/record-channel-answers.sh` captured them. It runs these same
//! journeys with `ONEPIPELINE_RECORD_CHANNEL_ANSWERS_WITH` naming the 0.28.2
//! executable, which makes each journey write what that binary answered rather
//! than compare against it. The world, the doubles and the steps are identical
//! either way, so the two answers differ in the binary alone.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes the two *siblings* at their
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary — as is the release it is compared with. `harness.rs` carries the same
// suppression and the full rationale.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::harness::{agent, binary, ended, plan_of, World};

/// The run every recorded channel is read inside: the recorded run root's own id.
const RUN: &str = "onemessagebus-repair-2";

/// The variable that turns these journeys from comparing into capturing.
const CAPTURE_ENV: &str = "ONEPIPELINE_RECORD_CHANNEL_ANSWERS_WITH";

/// The files of the channel layout, in the order an answer records them.
///
/// The one list of them: the write-side journey fails on a channel file either
/// binary writes that this does not name, and the recorded directories' README
/// points here rather than restating it.
const LAYOUT: [&str; 7] = [
    "surfaces.jsonl",
    "queue.json",
    "replies.jsonl",
    "replies-cursor.json",
    "commands.jsonl",
    "commands-cursor.json",
    "command-outcomes.jsonl",
];

fn recorded(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/recorded")
        .join(relative)
}

fn copied(from: &Path, into: &Path) {
    std::fs::create_dir_all(into).expect("a directory to seed");
    for entry in std::fs::read_dir(from).expect("a recorded directory") {
        let entry = entry.expect("a recorded file");
        if entry.file_name() == "README.md" {
            continue;
        }
        std::fs::copy(entry.path(), into.join(entry.file_name())).expect("a recorded file copies");
    }
}

/// A pid no platform issues: above Linux's `PID_MAX_LIMIT` and macOS's
/// `PID_MAX`, and not a multiple of four, which every Windows pid is. Still an
/// `i32`, because a Unix probe answers "may be live" for a pid it cannot pass to
/// `kill`.
const NO_PROCESS: u32 = 2_147_483_647;

/// The host the recorded launch record names, which every command in a world
/// reading it is told is its own.
fn recording_host() -> String {
    let launch: Value = serde_json::from_str(
        &std::fs::read_to_string(recorded(&format!("run-root/{RUN}/launch.json")))
            .expect("the recorded launch record reads"),
    )
    .expect("the recorded launch record is JSON");
    launch["host"]
        .as_str()
        .expect("the recorded launch record names a host")
        .to_owned()
}

/// A world that reads the recorded run root as the host that recorded it does.
///
/// Whether that run's driver is gone is proved only on the host its launch
/// record names, and only by asking that host's process table about its pid. So
/// the world's commands are told they run on the recording host, and the world's
/// copy of the launch record names a pid no process holds: every host then proves
/// the driver over by the same answer, and neither the reading host's name nor
/// what it happens to be running decides a word of what the verbs answer.
fn recorded_world(name: &str) -> World {
    World::new(name).with_env("HOSTNAME", &recording_host())
}

fn seeded(world: &World, fixture: &str) -> PathBuf {
    let root = world.runs.join(RUN);
    copied(&recorded(&format!("run-root/{RUN}")), &root);
    let launch = root.join("launch.json");
    let record = std::fs::read_to_string(&launch).expect("the seeded launch record reads");
    let driver = format!("\"pid\": {},", recorded_driver_pid(&record));
    assert_eq!(
        record.matches(&driver).count(),
        1,
        "the recorded launch record names its driver once"
    );
    std::fs::write(
        &launch,
        record.replace(&driver, &format!("\"pid\": {NO_PROCESS},")),
    )
    .expect("the seeded launch record is written");
    let channel = root.join("channel");
    copied(&recorded(&format!("channel/{fixture}")), &channel);
    channel
}

fn recorded_driver_pid(record: &str) -> u64 {
    serde_json::from_str::<Value>(record).expect("the recorded launch record is JSON")["pid"]
        .as_u64()
        .expect("the recorded launch record names a driver pid")
}

/// `text` with how long each update has been unread taken out: a view counts
/// what is unread and says for how long, and the second half is wall-clock.
fn aged(text: &str) -> String {
    const UNREAD_FOR: &str = "unread for ";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(UNREAD_FOR) {
        let (head, tail) = rest.split_at(at + UNREAD_FOR.len());
        out.push_str(head);
        out.push_str("<age>");
        rest = &tail[tail.find([',', ';', ' ', '\n']).unwrap_or(tail.len())..];
    }
    out.push_str(rest);
    out
}

/// `text` with the instant and the stream of every event this world's own
/// invocations journalled blanked, where `recorded` holds the streams the
/// recorded run root's journal already carries.
///
/// A `next` that hands a surface out records that it did, stamped with the clock
/// and the process that did it, and the next read returns that record: those two
/// fields are the only ones two runs of the same steps cannot share. An event
/// the recorded journal already holds is compared exactly as it stands.
fn journalled_here(text: &str, recorded: &BTreeSet<String>) -> String {
    const TS: &str = "\"ts\":\"";
    const STREAM: &str = "\",\"stream\":\"";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(TS) {
        let (head, tail) = rest.split_at(at + TS.len());
        out.push_str(head);
        let Some(ts_end) = tail.find('"') else {
            rest = tail;
            break;
        };
        let after_ts = &tail[ts_end..];
        let stream = after_ts
            .strip_prefix(STREAM)
            .and_then(|rest| rest.find('"').map(|end| (&rest[..end], &rest[end..])));
        match stream {
            Some((stream, after)) if !recorded.contains(stream) => {
                out.push_str("<instant>");
                out.push_str(STREAM);
                out.push_str("<this-process>");
                rest = after;
            }
            _ => {
                out.push_str(&tail[..ts_end]);
                rest = after_ts;
            }
        }
    }
    out.push_str(rest);
    out
}

fn recorded_streams() -> BTreeSet<String> {
    std::fs::read_to_string(recorded(&format!("run-root/{RUN}/events.jsonl")))
        .expect("the recorded journal reads")
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).unwrap_or_else(|error| {
                panic!("a recorded journal line is not JSON: {error}: {line}")
            })
        })
        .filter_map(|event| event["stream"].as_str().map(str::to_owned))
        .collect()
}

/// What one invocation answered, with the world's own scratch path taken out —
/// the one thing two worlds running the same steps cannot share — along with how
/// long anything has been unread, which is the clock's, and the stamp of any
/// event this world's own invocations journalled.
fn answered(world: &World, program: &Path, args: &[&str]) -> Value {
    let output = world
        .cmd_on(program, args)
        .output()
        .expect("the binary runs");
    let root = world.root.display().to_string();
    let streams = recorded_streams();
    let scrubbed = |bytes: &[u8]| {
        aged(&journalled_here(
            &String::from_utf8_lossy(bytes).replace(&root, "<world>"),
            &streams,
        ))
    };
    json!({
        "args": args,
        "exit": output.status.code(),
        "stdout": scrubbed(&output.stdout),
        "stderr": scrubbed(&output.stderr),
    })
}

/// Every answer one binary gives over one recorded channel, in the order a
/// supervisor reaching the run asks for them.
///
/// The projection is removed first, so the first read has to fold it back out
/// of the log alone and the bytes it writes are the re-derivation. `next` is
/// asked twice: the second is a reopen of the same channel, and whatever the
/// first handed out is not handed out again.
fn answers_over(fixture: &str, program: &Path) -> Value {
    let world = recorded_world("recorded-channel");
    let channel = seeded(&world, fixture);
    std::fs::remove_file(channel.join("queue.json")).expect("the recorded projection is removed");

    let mut steps = vec![answered(&world, program, &["status", RUN])];
    let rederived =
        std::fs::read_to_string(channel.join("queue.json")).expect("the read re-derived the queue");
    for args in [
        &["runs"][..],
        &["results", RUN],
        &["next", RUN],
        &["next", RUN],
        &["status", RUN],
    ] {
        steps.push(answered(&world, program, args));
    }
    // A file the reads left byte for byte as recorded is said to be so rather
    // than copied into the answer a second time; one they changed, or wrote, is
    // held whole.
    let seeded_from = recorded(&format!("channel/{fixture}"));
    let after: BTreeMap<&str, Value> = LAYOUT
        .into_iter()
        .filter_map(|file| {
            let bytes = std::fs::read(channel.join(file)).ok()?;
            let held = if std::fs::read(seeded_from.join(file)).ok().as_ref() == Some(&bytes) {
                json!({"as_recorded": true})
            } else {
                Value::String(String::from_utf8_lossy(&bytes).into_owned())
            };
            Some((file, held))
        })
        .collect();
    json!({
        "fixture": fixture,
        "steps": steps,
        "rederived_queue": rederived,
        "channel_after": after,
    })
}

/// The version every answer under `tests/recorded/answers/` is the answer of.
const RELEASE: &str = "onepipeline 0.28.2";

/// The executable [`CAPTURE_ENV`] names, refused unless it runs and reports
/// [`RELEASE`] — so an answer file is never overwritten with what some other
/// program, or another release, answered.
fn the_release_named(program: &Path) -> PathBuf {
    let reported = std::process::Command::new(program)
        .arg("--version")
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "{CAPTURE_ENV} names {}, which cannot be run: {error}",
                program.display()
            )
        });
    let version = String::from_utf8_lossy(&reported.stdout);
    assert!(
        reported.status.success() && version.trim() == RELEASE,
        "{CAPTURE_ENV} names {}, which reports {:?} rather than {RELEASE}; nothing was captured",
        program.display(),
        version.trim()
    );
    program.to_path_buf()
}

/// Capture what the release does into `answers/<name>.json` and answer `None`,
/// or answer what the release did beside what this build does.
fn against_the_release(name: &str, produce: impl Fn(&Path) -> Value) -> Option<(Value, Value)> {
    let path = recorded(&format!("answers/{name}.json"));
    if let Some(release) = std::env::var_os(CAPTURE_ENV) {
        let answers = produce(&the_release_named(Path::new(&release)));
        std::fs::create_dir_all(path.parent().expect("an answers directory"))
            .expect("the answers directory is created");
        let mut written = serde_json::to_string_pretty(&answers).expect("the answers serialize");
        written.push('\n');
        std::fs::write(&path, written).expect("the answers are written");
        return None;
    }
    let expected: Value = serde_json::from_str(
        &std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{} cannot be read: {error}", path.display())),
    )
    .expect("the recorded answers are JSON");
    Some((expected, produce(&binary())))
}

/// Capture 0.28.2's answers, or hold this build's to them.
fn held_to_the_release(fixture: &str) {
    let Some((expected, actual)) =
        against_the_release(fixture, |program| answers_over(fixture, program))
    else {
        return;
    };

    let (Some(want), Some(got)) = (expected["steps"].as_array(), actual["steps"].as_array()) else {
        panic!("an answer document holds no steps");
    };
    assert_eq!(
        want.len(),
        got.len(),
        "{fixture}: a different number of steps"
    );
    for (want, got) in want.iter().zip(got) {
        for part in ["exit", "stdout", "stderr"] {
            assert_eq!(
                got[part], want[part],
                "{fixture}: `onepipeline {}` answered a different {part} than 0.28.2 did",
                want["args"]
            );
        }
    }
    assert_eq!(
        actual["rederived_queue"], expected["rederived_queue"],
        "{fixture}: the queue.json re-derived from the log is not 0.28.2's"
    );
    assert_eq!(
        actual["channel_after"], expected["channel_after"],
        "{fixture}: the channel files the reads left behind are not 0.28.2's"
    );
}

/// A superseded check-in, surfaces answered, replies carrying a verdict and
/// edits, commands-only envelopes with their outcomes, surfaces abandoned and
/// attended by one asker, and a reply cursor short of the log's end.
#[test]
fn a_finished_runs_channel_reads_as_the_release_that_wrote_it_read_it() {
    held_to_the_release("domain-driven-modularity-2");
}

/// A small channel whose one reply answered the surface it was for.
#[test]
fn a_channel_with_no_command_queue_reads_as_the_release_that_wrote_it_read_it() {
    held_to_the_release("onemessagebus-repair-2");
}

/// A blocking surface claimed and still pending, which a reopen does not hand
/// out a second time.
#[test]
fn a_pending_blocking_surface_is_neither_lost_nor_handed_out_twice_after_a_reopen() {
    held_to_the_release("onemessagebus-repair-2-pending");
}

/// Updates nobody has read — a check-in a later one replaced, and a finding
/// beside it — counted as unread by every view, and handed out by `next` as the
/// release handed them out.
#[test]
fn updates_nobody_has_read_are_counted_and_handed_out_as_the_release_did() {
    held_to_the_release("waiting-updates");
}

/// The fields no two runs of the same steps share: when a surface was queued and
/// a reply was sent, and what the projection derives from the log's bytes, which
/// hold those instants. Each is blanked where it stands, so the rest is still
/// compared byte for byte and in the order it was written.
const UNSHARED: [&str; 4] = ["queued_at", "at", "accounted", "seal"];

/// A question's correlation, which 0.28.2 never wrote and this build writes on
/// every surface and reply record a question is bound through. Taken out
/// whole, with the separator before it, as the manager's ruling on the byte
/// comparison normalizes it.
const CORRELATION: &str = "correlation";

/// Where the value that starts at `from` ends: past its closing quote for a
/// string, or at the first separator for anything else.
fn value_end(text: &str, from: usize) -> usize {
    let bytes = text.as_bytes();
    if bytes.get(from) == Some(&b'"') {
        let mut at = from + 1;
        while at < bytes.len() && bytes[at] != b'"' {
            at += if bytes[at] == b'\\' { 2 } else { 1 };
        }
        return (at + 1).min(bytes.len());
    }
    text[from..]
        .find([',', '}', ']', '\n'])
        .map_or(text.len(), |offset| from + offset)
}

/// `text` with every [`UNSHARED`] value blanked and every [`CORRELATION`] taken
/// out.
fn normalized(text: &str) -> String {
    let mut out = text.to_owned();
    for key in UNSHARED {
        let needle = format!("\"{key}\":");
        let mut searched = 0;
        while let Some(found) = out[searched..].find(&needle) {
            let key_end = searched + found + needle.len();
            let start =
                key_end + out[key_end..].len() - out[key_end..].trim_start_matches(' ').len();
            let end = value_end(&out, start);
            out.replace_range(start..end, "\"<unshared>\"");
            searched = start;
        }
    }
    let needle = format!("\"{CORRELATION}\":");
    while let Some(found) = out.find(&needle) {
        let key_end = found + needle.len();
        let start = key_end + out[key_end..].len() - out[key_end..].trim_start_matches(' ').len();
        let end = value_end(&out, start);
        let separator = out[..found].trim_end().len();
        match out[..separator].ends_with(',') {
            true => out.replace_range(separator - 1..end, ""),
            false => {
                let trailing = end + out[end..].len() - out[end..].trim_start_matches(',').len();
                out.replace_range(found..trailing, "");
            }
        }
    }
    out
}

const WRITTEN: &str = "written";

fn unread_lines(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter(|line| line.contains("unread for"))
        .map(|line| aged(line.trim()))
        .collect()
}

/// What one binary writes into a channel from empty, across every verb that
/// writes one, on a run that binary launched and is still driving — its one node
/// held open, so its loop is live and reconciles commands for the whole journey:
///
/// - two check-ins, the second replacing the first, and a finding, counted as
///   unread;
/// - a blocking question `channel serve` raises under a named asker, claimed by
///   `next` with the other two;
/// - a reply carrying a verdict and a command, whose verdict reaches the server
///   holding the question and whose command the driver applies;
/// - a commands-only reply the driver applies, writing its outcome;
/// - a question whose session ends unanswered, which abandons it, and a later
///   session of the same asker, which attends it and reaches its bound;
/// - a verdict no listener reads, which leaves the reply cursor short;
/// - and the run stopped.
///
/// Each step's exit is kept beside the files, and the unread lines `runs` prints
/// between them, so a step one binary refused and the other took cannot leave
/// the same bytes by accident. A server's own stdout is not compared: what it
/// answers is `channel serve`'s protocol, which the channel journeys hold, and
/// not what the channel directory holds.
fn written_by(program: &Path) -> Value {
    let world = World::new("written-channel");
    // An agent node the `oneagentgraph` double holds open, rather than a human
    // one: 0.28.2 settles a human node `waiting` on its first pass and its driver
    // lets go of the run, which leaves no loop to reconcile a command queue.
    world.script("slow.wait", "hold");
    let path = world.plan(WRITTEN, &plan_of(WRITTEN, vec![agent("slow", &[])]));
    let channel = world.runs.join(WRITTEN).join("channel");
    let asker = ("ONEPIPELINE_CHANNEL_ASKER", "written-listener");
    // Long enough that a reply's commands are answered by the driver before the
    // reply returns, so the next step never races the reconciler's write.
    let reply_wait = ("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "60");

    let mut steps = Vec::new();
    let mut unread = Vec::new();
    let mut step = |args: &[&str], stdin: &str, env: &[(&str, &str)]| -> String {
        let mut command = world.cmd_on(program, args);
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("the binary runs");
        child
            .stdin
            .take()
            .expect("stdin is piped")
            .write_all(stdin.as_bytes())
            .expect("stdin is written");
        let output = child.wait_with_output().expect("the binary exits");
        steps.push(json!({"args": args, "exit": output.status.code()}));
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    let serve = |env: &[(&str, &str)]| {
        let mut command = world.cmd_on(program, &["channel", "serve", WRITTEN]);
        for (key, value) in env {
            command.env(key, value);
        }
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("the channel server starts")
    };
    let queued = |text: &str| {
        std::fs::read_to_string(channel.join("surfaces.jsonl")).is_ok_and(|log| log.contains(text))
    };

    step(&["start", &path, "--detach"], "", &[]);
    world.until("the driver to hold its node open", |world| {
        !world.events_of(WRITTEN, "node-dispatched").is_empty()
    });
    for (kind, message) in [
        ("check-in", "the first check-in"),
        ("check-in", "the check-in that replaces it"),
        ("finding", "a finding nobody read yet"),
    ] {
        step(
            &["surface", WRITTEN, "--kind", kind, "--message", message],
            "",
            &[],
        );
    }
    unread.extend(unread_lines(&step(&["runs"], "", &[])));

    let question = "a blocking question the planner answers";
    let mut first = serve(&[asker, ("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "120")]);
    let mut frames = first.stdin.take().expect("stdin is piped");
    writeln!(
        frames,
        r#"{{"kind":"planner-question","message":"{question}","blocking":true}}"#
    )
    .expect("the frame is written");
    frames.flush().expect("flushed");
    world.until("the question to be queued", |_| queued(question));
    for _ in 0..3 {
        step(&["next", WRITTEN], "", &[]);
    }
    step(
        &["reply", WRITTEN],
        r#"{"version":3,"completion":false,"message":"keep going","commands":[{"op":"finding","message":"a finding beside the verdict"}]}"#,
        &[reply_wait],
    );
    let answered = BufReader::new(first.stdout.take().expect("stdout is piped"))
        .lines()
        .map_while(Result::ok)
        .any(|line| line.contains("keep going"));
    drop(frames);
    ended(first);
    step(
        &["reply", WRITTEN],
        r#"{"version":3,"commands":[{"op":"finding","message":"a finding on its own"}]}"#,
        &[reply_wait],
    );

    // A question whose session waits out its window and then ends: abandoned.
    let abandoned = "a question its session leaves behind";
    let mut second = serve(&[asker, ("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "1")]);
    let mut frames = second.stdin.take().expect("stdin is piped");
    writeln!(
        frames,
        r#"{{"kind":"planner-question","message":"{abandoned}","blocking":true}}"#
    )
    .expect("the frame is written");
    frames.flush().expect("flushed");
    let was_told = BufReader::new(second.stdout.take().expect("stdout is piped"))
        .lines()
        .next()
        .is_some();
    drop(frames);
    ended(second);

    // A later session of the same asker attends it, and ends on its own bound
    // with its stream still open, which marks nothing.
    let mut third = serve(&[
        asker,
        ("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "120"),
        ("ONEPIPELINE_SERVE_SESSION_SECONDS", "2"),
    ]);
    let held_open = third.stdin.take().expect("stdin is piped");
    let deadline = Instant::now() + Duration::from_secs(60);
    while third
        .try_wait()
        .expect("the session is waited on")
        .is_none()
    {
        if Instant::now() >= deadline {
            // This session's own process, identified by the handle that spawned it.
            let _ = third.kill();
            let _ = third.wait();
            panic!("the attending session never reached its bound");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(held_open);

    step(
        &["reply", WRITTEN],
        r#"{"completion":false,"message":"a ruling no listener reads"}"#,
        &[],
    );
    unread.extend(unread_lines(&step(&["runs"], "", &[])));
    step(&["stop", WRITTEN], "", &[]);
    world.release("slow.go");

    // A file either binary writes that the layout does not name would be compared
    // by nothing, so it fails here instead. Directories are not the layout's:
    // the transport's lock files and the handover gate each keep their own.
    let unnamed: Vec<String> = std::fs::read_dir(&channel)
        .expect("the channel directory reads")
        .map(|entry| entry.expect("a channel directory entry"))
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !LAYOUT.contains(&name.as_str()))
        .collect();
    assert!(
        unnamed.is_empty(),
        "{} wrote channel files `LAYOUT` does not name, so nothing compares them: {unnamed:?}",
        program.display()
    );

    let files: BTreeMap<&str, Value> = LAYOUT
        .into_iter()
        .filter_map(|file| {
            let text = std::fs::read_to_string(channel.join(file)).ok()?;
            Some((file, Value::String(normalized(&text))))
        })
        .collect();
    json!({
        "steps": steps,
        "unread": unread,
        "verdict_reached_the_server": answered,
        "abandoned_session_was_told": was_told,
        "channel": files,
    })
}

/// Every channel file this build writes across the channel's writing verbs is,
/// byte for byte, what 0.28.2 writes across the same steps — with only the
/// instants, what is derived from them, and the correlation normalized — and
/// every file either binary writes is compared, the command queue and its
/// outcomes included.
#[test]
// llmlint: ignore[expensive_tests_stay_behind_their_own_edge] this journey's cost is its steps — three `channel serve` sessions, one of which waits out its reply window, over a driver held live — and every byte it compares is written by the crate's own channel verbs, so any change under `src/` can move them. The one separately-edged project here, `onepipeline-note-journeys`, is edged on conversational cost and would put it where a change to `src/channel.rs` does not run it, which is the one change that must.
fn a_channel_this_build_writes_is_byte_for_byte_the_channel_the_release_writes() {
    let Some((expected, actual)) = against_the_release("written-channel", written_by) else {
        return;
    };
    assert_eq!(
        actual["steps"], expected["steps"],
        "a writing verb exited differently than 0.28.2's did"
    );
    assert_eq!(
        actual["unread"], expected["unread"],
        "the unread count differs from 0.28.2's"
    );
    for fact in ["verdict_reached_the_server", "abandoned_session_was_told"] {
        assert_eq!(actual[fact], expected[fact], "{fact}");
    }
    for file in [
        "commands.jsonl",
        "command-outcomes.jsonl",
        "replies-cursor.json",
    ] {
        assert!(
            expected["channel"][file].is_string(),
            "0.28.2 wrote no `{file}` across the steps, so nothing of it is compared"
        );
    }
    for file in LAYOUT {
        assert_eq!(
            actual["channel"][file].as_str(),
            expected["channel"][file].as_str(),
            "`{file}` is not the bytes 0.28.2 wrote across the same steps"
        );
    }
}

/// An answer's stamps are blanked only on events this world journalled, and an
/// age only where a view says how long something has been unread.
#[test]
fn normalizing_an_answer_blanks_only_this_worlds_stamps_and_the_unread_age() {
    let recorded: BTreeSet<String> = ["recorded-stream".to_owned()].into();
    assert_eq!(
        journalled_here(
            r#"[{"v":2,"ts":"2026-09-13T02:02:42.515Z","stream":"recorded-stream","seq":0},{"v":2,"ts":"2026-09-14T21:21:58.381Z","stream":"host-2128858","seq":0}]"#,
            &recorded
        ),
        r#"[{"v":2,"ts":"2026-09-13T02:02:42.515Z","stream":"recorded-stream","seq":0},{"v":2,"ts":"<instant>","stream":"<this-process>","seq":0}]"#
    );
    assert_eq!(
        aged("2 planner update(s) waiting (1 check-in), unread for 43h14m; read them"),
        "2 planner update(s) waiting (1 check-in), unread for <age>; read them"
    );
    assert_eq!(aged("nothing unread here"), "nothing unread here");
}

/// The normalization touches the fields it names and nothing else: a value is
/// blanked where it stands, and a correlation leaves no separator behind.
#[test]
fn normalizing_a_written_record_blanks_only_what_no_two_runs_share() {
    assert_eq!(
        normalized(r#"{"id":3,"queued_at":1789409111802,"asker":"a","correlation":"c-1"}"#),
        r#"{"id":3,"queued_at":"<unshared>","asker":"a"}"#
    );
    assert_eq!(
        normalized(
            "{\n  \"correlation\": \"c-2\",\n  \"accounted\": 18971,\n  \"seal\": \"716f\"\n}"
        ),
        "{\n  \n  \"accounted\": \"<unshared>\",\n  \"seal\": \"<unshared>\"\n}"
    );
    assert_eq!(
        normalized(r#"{"message":"queued_at is only a word here","id":1}"#),
        r#"{"message":"queued_at is only a word here","id":1}"#
    );
}
