//! What the recorded-channel journeys share: the recorded run root a channel
//! is read inside, a world that reads it as the host that recorded it, and the
//! normalization two binaries' answers over the same steps are compared under.
//!
//! Its own file, with no tests of its own, because two test binaries include
//! it: `e2e`, whose `recorded_channel` holds this build to the answers 0.28.2
//! gave, and `release_channel`, which drives the 0.28.2 wheel itself.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::harness::World;

/// The run every recorded channel is read inside: the recorded run root's own id.
pub(crate) const RUN: &str = "onemessagebus-repair-2";

/// The files of the channel layout, in the order an answer records them.
///
/// The one list of them: the write-side journey fails on a channel file either
/// binary writes that this does not name, and the recorded directories' README
/// points here rather than restating it.
pub(crate) const LAYOUT: [&str; 7] = [
    "surfaces.jsonl",
    "queue.json",
    "replies.jsonl",
    "replies-cursor.json",
    "commands.jsonl",
    "commands-cursor.json",
    "command-outcomes.jsonl",
];

pub(crate) fn recorded(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/recorded")
        .join(relative)
}

fn copied(from: &Path, into: &Path) {
    std::fs::create_dir_all(into).expect("a directory to seed");
    for entry in std::fs::read_dir(from).expect("a recorded directory") {
        let entry = entry.expect("a recorded file");
        // A transport may keep a directory of its own beside the layout's files;
        // only the files are the channel.
        if entry.file_name() == "README.md" || !entry.path().is_file() {
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
pub(crate) fn recorded_world(name: &str) -> World {
    World::new(name).with_env("HOSTNAME", &recording_host())
}

/// The world's copy of the recorded run root, its driver proved gone on any
/// host, with `channel`'s files as its `channel/` — or an empty `channel/` when
/// there is none.
pub(crate) fn seeded_with(world: &World, channel: Option<&Path>) -> PathBuf {
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
    let into = root.join("channel");
    match channel {
        Some(from) => copied(from, &into),
        None => std::fs::create_dir_all(&into).expect("an empty channel directory"),
    }
    into
}

fn recorded_driver_pid(record: &str) -> u64 {
    serde_json::from_str::<Value>(record).expect("the recorded launch record is JSON")["pid"]
        .as_u64()
        .expect("the recorded launch record names a driver pid")
}

/// The line 0.28.2 wrote on `next`'s stderr for a run that launched no observer
/// graph: it sent a check-in reset to a graph run the record did not name, and
/// said so. This build sends none for such a run and says nothing — a launch
/// with no observer graph has no clock to restart, and entry 79 of
/// `docs/contract-divergences.md` records the ruling — so the report is taken
/// out of both answers before they are compared. It is the whole of that line
/// and nothing else on stderr: any other word either binary writes there is
/// still held.
const RESET_REPORT_OF_0_28_2: &str = "onepipeline: could not reset the check-in pacemaker: \
                                     invalid: run 'onemessagebus-repair-2' records no \
                                     agent-graph run to address it by\n";

pub(crate) fn without_the_reset_report(text: &str) -> String {
    text.replace(RESET_REPORT_OF_0_28_2, "")
}

/// `text` with how long each update has been unread taken out: a view counts
/// what is unread and says for how long, and the second half is wall-clock.
pub(crate) fn aged(text: &str) -> String {
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
pub(crate) fn journalled_here(text: &str, recorded: &BTreeSet<String>) -> String {
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
pub(crate) fn answered(world: &World, program: &Path, args: &[&str]) -> Value {
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
        "stderr": without_the_reset_report(&scrubbed(&output.stderr)),
    })
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
pub(crate) fn normalized(text: &str) -> String {
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
