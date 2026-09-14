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

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::harness::{binary, ended, World};

/// The run every recorded channel is read inside: the recorded run root's own id.
const RUN: &str = "onemessagebus-repair-2";

/// The variable that turns these journeys from comparing into capturing.
const CAPTURE_ENV: &str = "ONEPIPELINE_RECORD_CHANNEL_ANSWERS_WITH";

/// The files of the channel layout, in the order an answer records them.
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

/// Copy every file of a recorded directory into `into`.
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

/// A world holding the recorded run root, with `fixture` as its channel.
fn seeded(world: &World, fixture: &str) -> PathBuf {
    let root = world.runs.join(RUN);
    copied(&recorded(&format!("run-root/{RUN}")), &root);
    let channel = root.join("channel");
    copied(&recorded(&format!("channel/{fixture}")), &channel);
    channel
}

/// What one invocation answered, with the world's own scratch path taken out:
/// it is the one thing two worlds running the same steps cannot share.
fn answered(world: &World, program: &Path, args: &[&str]) -> Value {
    let output = world
        .cmd_on(program, args)
        .output()
        .expect("the binary runs");
    let root = world.root.display().to_string();
    let scrubbed = |bytes: &[u8]| String::from_utf8_lossy(bytes).replace(&root, "<world>");
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
    let world = World::new("recorded-channel");
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

/// Capture what the release does into `answers/<name>.json` and answer `None`,
/// or answer what the release did beside what this build does.
fn against_the_release(name: &str, produce: impl Fn(&Path) -> Value) -> Option<(Value, Value)> {
    let path = recorded(&format!("answers/{name}.json"));
    if let Some(release) = std::env::var_os(CAPTURE_ENV) {
        let answers = produce(Path::new(&release));
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

/// What one binary writes into a fresh channel, over the recorded run root,
/// across every verb that writes one: two check-ins, the second replacing the
/// first; a finding; a blocking question `channel serve` raises under a named
/// asker; the `next`s that claim all three; a verdict that answers the question
/// and reaches the server holding it; and a monitor's commands-only reply, which
/// both binaries refuse on this settled run, so nothing of it may be written.
///
/// Each step's exit is kept beside the files, so a step one binary refused and
/// the other took cannot leave the same bytes by accident. The server's own
/// stdout is not compared: what it answers is `channel serve`'s protocol, which
/// the channel journeys hold, and not what the channel directory holds.
fn written_by(program: &Path) -> Value {
    let world = World::new("written-channel");
    let root = world.runs.join(RUN);
    copied(&recorded(&format!("run-root/{RUN}")), &root);
    let channel = root.join("channel");
    std::fs::create_dir_all(&channel).expect("an empty channel directory");

    let mut steps = Vec::new();
    let mut step = |args: &[&str], stdin: &str| {
        let mut child = world
            .cmd_on(program, args)
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
    };
    step(
        &[
            "surface",
            RUN,
            "--kind",
            "check-in",
            "--message",
            "the first check-in",
        ],
        "",
    );
    step(
        &[
            "surface",
            RUN,
            "--kind",
            "check-in",
            "--message",
            "the check-in that replaces it",
        ],
        "",
    );
    step(
        &[
            "surface",
            RUN,
            "--kind",
            "finding",
            "--message",
            "a finding nobody read yet",
        ],
        "",
    );

    let question = "a blocking question the planner answers";
    let mut serving = world
        .cmd_on(program, &["channel", "serve", RUN])
        .env("ONEPIPELINE_CHANNEL_ASKER", "written-listener")
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "120")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the channel server starts");
    let mut frames = serving.stdin.take().expect("stdin is piped");
    writeln!(
        frames,
        r#"{{"kind":"planner-question","message":"{question}","blocking":true}}"#
    )
    .expect("the frame is written");
    frames.flush().expect("flushed");
    world.until("the question to be queued", |_| {
        std::fs::read_to_string(channel.join("surfaces.jsonl"))
            .is_ok_and(|log| log.contains(question))
    });
    for _ in 0..3 {
        step(&["next", RUN], "");
    }
    step(
        &["reply", RUN],
        r#"{"completion":false,"message":"keep going"}"#,
    );
    let answered = BufReader::new(serving.stdout.take().expect("stdout is piped"))
        .lines()
        .map_while(Result::ok)
        .any(|line| line.contains("keep going"));
    drop(frames);
    ended(serving);
    step(
        &["reply", RUN],
        r#"{"version":3,"author":"monitor","commands":[{"op":"cancel","id":"plan"}]}"#,
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
        "verdict_reached_the_server": answered,
        "channel": files,
    })
}

/// Every channel file this build writes across the channel's writing verbs is,
/// byte for byte, what 0.28.2 writes across the same steps — with only the
/// instants, what is derived from them, and the correlation normalized.
#[test]
fn a_channel_this_build_writes_is_byte_for_byte_the_channel_the_release_writes() {
    let Some((expected, actual)) = against_the_release("written-channel", written_by) else {
        return;
    };
    assert_eq!(
        actual["steps"], expected["steps"],
        "a writing verb exited differently than 0.28.2's did"
    );
    assert_eq!(
        actual["verdict_reached_the_server"],
        expected["verdict_reached_the_server"]
    );
    for file in LAYOUT {
        assert_eq!(
            actual["channel"][file].as_str(),
            expected["channel"][file].as_str(),
            "`{file}` is not the bytes 0.28.2 wrote across the same steps"
        );
    }
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
