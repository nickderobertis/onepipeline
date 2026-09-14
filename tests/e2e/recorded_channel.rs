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
//! what the 0.28.2 binary answered over the same world.
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
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::harness::{binary, World};

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

/// Capture 0.28.2's answers, or hold this build's to them.
fn held_to_the_release(fixture: &str) {
    let path = recorded(&format!("answers/{fixture}.json"));
    if let Some(release) = std::env::var_os(CAPTURE_ENV) {
        let answers = answers_over(fixture, Path::new(&release));
        std::fs::create_dir_all(path.parent().expect("an answers directory"))
            .expect("the answers directory is created");
        let mut written = serde_json::to_string_pretty(&answers).expect("the answers serialize");
        written.push('\n');
        std::fs::write(&path, written).expect("the answers are written");
        return;
    }
    let expected: Value = serde_json::from_str(
        &std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{} cannot be read: {error}", path.display())),
    )
    .expect("the recorded answers are JSON");
    let actual = answers_over(fixture, &binary());

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
