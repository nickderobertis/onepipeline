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
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::harness::{binary, repo_file, World};
use crate::recorded_support::{
    aged, answered, journalled_here, normalized, recorded, recorded_world, seeded_with,
    without_the_reset_report, LAYOUT, RELEASE, RUN,
};

/// The variable that turns these journeys from comparing into capturing.
const CAPTURE_ENV: &str = "ONEPIPELINE_RECORD_CHANNEL_ANSWERS_WITH";

fn seeded(world: &World, fixture: &str) -> PathBuf {
    seeded_with(world, Some(&recorded(&format!("channel/{fixture}"))))
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
    // The one listing 0.28.2 had is this build's flat one: `runs` groups its rows
    // by project now, under a header line each, and the rows beneath a header are
    // the rows the release printed. So this build is asked for them flat, and the
    // release — which knows no such flag — for its listing.
    let listing: &[&str] = if program == binary() {
        &["runs", "--flat"]
    } else {
        &["runs"]
    };
    for args in [
        listing,
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
        reported.status.success() && version.trim() == format!("onepipeline {RELEASE}"),
        "{CAPTURE_ENV} names {}, which reports {:?} rather than onepipeline {RELEASE}; nothing \
         was captured",
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
            // The recorded answer is 0.28.2's as captured; the one report this
            // build deliberately no longer writes is taken out of it here, as
            // it is out of every answer produced now.
            let want_part = match (part, want[part].as_str()) {
                ("stderr", Some(recorded)) => json!(without_the_reset_report(recorded)),
                _ => want[part].clone(),
            };
            assert_eq!(
                got[part], want_part,
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

/// The same pending state as the bus recorded it: the directory `onemessagebus`
/// held the `planner-channel` layout to before the layout moved here, read
/// exactly as the release read it.
#[test]
fn the_pending_surface_the_bus_recorded_is_neither_lost_nor_handed_out_twice_after_a_reopen() {
    held_to_the_release("onemessagebus-repair-2-pending-bus");
}

/// Updates nobody has read — a check-in a later one replaced, and a finding
/// beside it — counted as unread by every view, and handed out by `next` as the
/// release handed them out.
#[test]
fn updates_nobody_has_read_are_counted_and_handed_out_as_the_release_did() {
    held_to_the_release("waiting-updates");
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

/// The release the channel comparisons are held to is the one the contract says
/// this build's channel stays byte-compatible with — so the pin the recorded
/// answers and the wheel journeys share cannot move without the contract moving.
#[test]
fn the_release_the_channel_is_held_to_is_the_one_the_contract_names() {
    let contract =
        std::fs::read_to_string(repo_file("docs/contract.md")).expect("the contract ships");
    let passage = contract
        .lines()
        .find(|line| line.starts_with("**The planner channel runs on `onemessagebus`"))
        .expect("the contract states who owns the planner-channel layout");
    assert!(
        passage.contains(&format!("the `onepipeline-cli` {RELEASE} wheel wrote")),
        "the contract does not name onepipeline-cli {RELEASE} as the release the channel is \
         byte-compatible with"
    );
    let script = std::fs::read_to_string(repo_file("scripts/record-channel-answers.sh"))
        .expect("the capture script ships");
    assert!(
        script.contains(&format!("release=\"${{1:-{RELEASE}}}\"")),
        "scripts/record-channel-answers.sh captures another release by default"
    );
}
