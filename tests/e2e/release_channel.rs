//! The planner channel held byte for byte to the `onepipeline-cli` 0.28.2 wheel,
//! driven live.
//!
//! `recorded_channel.rs` holds this build to answers 0.28.2 gave once and that
//! are checked in. These journeys run the release itself — the `onepipeline`
//! binary of the pinned wheel, resolved through `uv tool run` — beside this
//! build, each over its own copy of the recorded run root
//! `onemessagebus-repair-2`, and hold the seven files of the `planner-channel`
//! layout both ways:
//!
//! - a directory **this build writes**, through its own layout, is read by the
//!   release exactly as this build reads it, and the release's claims and reply
//!   leave the same bytes this build's leave — and this build reads back what
//!   the release appended;
//! - a directory **the release writes**, through its `surface` and `next`, is the directory
//!   this build writes through the same verbs, and each binary reads the other's
//!   exactly as it reads its own.
//!
//! This journey was `onemessagebus`'s (`crates/onemessagebus-e2e/tests/e2e/onepipeline.rs`)
//! while the bus carried the layout; onemessagebus#126 moved the layout here, and
//! the proof came with it.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes the two *siblings* at their
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary — as is the release it is compared with. `harness.rs` carries the same
// suppression and the full rationale.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, OnceLock};

use onemessagebus::{Asker, Author, LocalTransport, Transport};
use onepipeline::channel::layout::{
    source, Channel, CommandOutcome, CommandResult, CommandVerdict, ReplyEnvelope, Surface,
};
use serde_json::{json, Map, Value};

use crate::harness::{binary, World};
use crate::recorded_channel::{answered, normalized, recorded_world, seeded_with, LAYOUT, RUN};

/// The release whose channel this build is held to.
const RELEASE: &str = "0.28.2";

/// The pinned wheel's `onepipeline`, resolved once through `uv tool run` and
/// refused unless it reports [`RELEASE`].
fn release() -> &'static Path {
    static PROGRAM: OnceLock<PathBuf> = OnceLock::new();
    PROGRAM.get_or_init(|| {
        let from = format!("onepipeline-cli=={RELEASE}");
        let resolved = std::process::Command::new("uv")
            .args([
                "tool",
                "run",
                "--from",
                &from,
                "sh",
                "-c",
                "command -v onepipeline",
            ])
            .output()
            .unwrap_or_else(|failure| {
                panic!(
                    "this journey runs the {from} wheel through `uv tool run`, and uv could not \
                     be started ({failure}); install uv (https://docs.astral.sh/uv/) — CI sets \
                     it up for every job that runs this suite"
                )
            });
        assert!(
            resolved.status.success(),
            "uv could not provide {from}: {}",
            String::from_utf8_lossy(&resolved.stderr)
        );
        let program = PathBuf::from(String::from_utf8_lossy(&resolved.stdout).trim());
        let reported = std::process::Command::new(&program)
            .arg("--version")
            .output()
            .expect("the release's binary runs");
        assert_eq!(
            String::from_utf8_lossy(&reported.stdout).trim(),
            format!("onepipeline {RELEASE}"),
            "{} is not the {RELEASE} release; clear uv's cached copy with `uv cache clean \
             onepipeline-cli`",
            program.display()
        );
        program
    })
}

/// `program`'s exit and stdout for `args` in `world`, with `stdin` written to it.
fn fed(world: &World, program: &Path, args: &[&str], stdin: &str) -> (Option<i32>, String) {
    let mut child = world
        .cmd_on(program, args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary starts");
    child
        .stdin
        .take()
        .expect("a stdin")
        .write_all(stdin.as_bytes())
        .expect("stdin is written");
    let output = child.wait_with_output().expect("the binary exits");
    assert!(
        output.status.success(),
        "`{} {args:?}` refused: {}",
        program.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

/// Every file of the layout in `channel`, `None` for one it does not hold.
fn files(channel: &Path) -> Vec<(&'static str, Option<String>)> {
    LAYOUT
        .into_iter()
        .map(|file| (file, std::fs::read_to_string(channel.join(file)).ok()))
        .collect()
}

/// Hold every file of the layout alike in two channels, after `step`.
fn alike(release: &Path, ours: &Path, step: &str, normalize: fn(&str) -> String) {
    for ((file, theirs), (_, mine)) in files(release).into_iter().zip(files(ours)) {
        assert_eq!(
            mine.as_deref().map(normalize),
            theirs.as_deref().map(normalize),
            "after {step}, {file} differs between {RELEASE}'s channel and this build's"
        );
    }
}

fn exact(text: &str) -> String {
    text.to_owned()
}

/// Both binaries' answers to `args`, each over its own world, held equal.
fn answered_alike(release_world: &World, ours_world: &World, args: &[&str]) -> Value {
    let theirs = answered(release_world, release(), args);
    let mine = answered(ours_world, &binary(), args);
    assert_eq!(
        mine, theirs,
        "`onepipeline {args:?}` answered differently from {RELEASE} over the same channel"
    );
    mine
}

fn surface(kind: &str, message: &str, from: &str, blocking: bool, asker: Option<&str>) -> Surface {
    Surface {
        id: 0,
        kind: kind.to_owned(),
        message: message.to_owned(),
        source: from.to_owned(),
        blocking,
        queued_at: 1_789_300_000_000,
        workstream: None,
        abandoned: false,
        asker: asker.map(|name| Asker::new(name, "the journey").expect("an asker")),
        correlation: None,
    }
}

fn command(fields: Value) -> Map<String, Value> {
    fields.as_object().expect("a command object").clone()
}

/// Write a channel directory from empty through this crate's layout, across a
/// history that writes every one of its seven files.
fn write_channel(dir: &Path) {
    let transport: Arc<dyn Transport> = Arc::new(LocalTransport::open(dir).expect("opens"));
    let channel = Channel::open(&transport).expect("the channel opens");

    // A check-in superseded by the next, and a finding.
    for (kind, message, from) in [
        ("check-in", "first update", source::CHECK_IN),
        ("check-in", "second update", source::CHECK_IN),
        ("finding", "the gate went red", source::PROPOSAL),
    ] {
        channel
            .push(&surface(kind, message, from, false, None))
            .expect("queued");
    }

    // A question claimed, abandoned by its listener, attended by a later one of
    // the same asker, and answered by a reply.
    let asked = channel
        .push(&surface(
            "planner-question",
            "asked by a listener that left and came back",
            source::PROPOSAL,
            true,
            Some("listener-a"),
        ))
        .expect("queued");
    let claimed = channel.claim().expect("a claim").expect("the question");
    assert_eq!(Some(claimed.id), asked.id);
    assert_eq!(
        channel
            .abandon(&[asked.id.expect("an id")])
            .expect("abandoned")
            .len(),
        1
    );
    assert_eq!(
        channel
            .attend(&Asker::new("listener-a", "the journey").expect("an asker"))
            .expect("attended")
            .len(),
        1
    );
    let answered = channel
        .answer(
            &ReplyEnvelope {
                completion: Some(false),
                message: Some("carry on".to_owned()),
                ..ReplyEnvelope::default()
            },
            1_789_300_000_100,
        )
        .expect("answered");
    assert_eq!(
        channel
            .claim_reply()
            .expect("a claim")
            .map(|reply| reply.id),
        Some(answered)
    );

    // A commands-only envelope from the sentinel and one from the planner, both
    // claimed and answered.
    channel
        .submit(
            Author::from("sentinel"),
            vec![command(
                json!({"op": "finding", "message": "look at the gate"}),
            )],
        )
        .expect("submitted");
    channel
        .submit(
            Author::from("planner"),
            vec![command(
                json!({"op": "note", "id": "plan", "addressee": "worker", "text": "a smaller diff"}),
            )],
        )
        .expect("submitted");
    assert_eq!(channel.claim_commands().expect("claimed").len(), 2);
    for (id, op, applied) in [(0, "finding", true), (1, "note", false)] {
        channel
            .answer_commands(&CommandOutcome {
                id,
                applied,
                reason: (!applied).then(|| "the node has settled".to_owned()),
                results: vec![CommandResult {
                    index: 0,
                    op: op.to_owned(),
                    outcome: if applied {
                        CommandVerdict::Applied
                    } else {
                        CommandVerdict::Refused
                    },
                    reason: (!applied).then(|| "the node has settled".to_owned()),
                }],
            })
            .expect("answered");
    }

    // A question whose listener never came back, and one still pending.
    let gone = channel
        .push(&surface(
            "planner-question",
            "asked by a listener that never came back",
            source::PROPOSAL,
            true,
            Some("listener-b"),
        ))
        .expect("queued");
    channel
        .abandon(&[gone.id.expect("an id")])
        .expect("abandoned");
    channel
        .push(&surface(
            "planner-question",
            "still pending",
            source::PROPOSAL,
            true,
            None,
        ))
        .expect("queued");
    let pending = channel.claim().expect("a claim").expect("the question");
    assert_eq!(pending.message, "still pending");

    for file in LAYOUT {
        assert!(dir.join(file).is_file(), "the layout wrote no {file}");
    }
}

#[test]
fn a_channel_this_build_writes_is_read_by_the_0_28_2_wheel_and_what_it_appends_is_read_back() {
    let scratch = World::new("release-channel-written");
    let written = scratch.root.join("written");
    write_channel(&written);

    let release_world = recorded_world("release-channel-written-theirs");
    let ours_world = recorded_world("release-channel-written-ours");
    let theirs = seeded_with(&release_world, Some(&written));
    let ours = seeded_with(&ours_world, Some(&written));

    // Reads first, then the two claims a supervisor makes: the release and this
    // build answer each alike, and leave all seven files alike.
    let status = answered_alike(&release_world, &ours_world, &["status", RUN]);
    let stdout = status["stdout"].as_str().expect("a status");
    for line in [
        "  waiting for planner decision: planner-question — still pending",
        "  2 planner update(s) waiting (1 check-in, 1 finding), unread for <age>",
        "  1 planner update(s) nobody is waiting on: ",
    ] {
        assert!(
            stdout.contains(line),
            "{RELEASE} does not say `{line}` of the channel this build wrote:\n{stdout}"
        );
    }
    alike(&theirs, &ours, "`status`", exact);
    answered_alike(&release_world, &ours_world, &["results", RUN]);
    let next = answered_alike(&release_world, &ours_world, &["next", RUN]);
    assert!(
        next["stdout"]
            .as_str()
            .is_some_and(|text| text.contains("\"message\":\"second update\"")),
        "the first `next` did not hand out the check-in that replaced the first: {next}"
    );
    answered_alike(&release_world, &ours_world, &["next", RUN]);
    alike(&theirs, &ours, "both `next`s", exact);

    // The release answers the pending question, and so does this build; the
    // replies differ only in the instant each was sent.
    let reply = r#"{"completion": false, "message": "answered by the release"}"#;
    let (code, receipt) = fed(&release_world, release(), &["reply", RUN], reply);
    assert_eq!(
        (
            code,
            serde_json::from_str::<Value>(&receipt).expect("a receipt")
        ),
        (
            Some(0),
            json!({"reply": 1, "state": "delivered", "verdict": "delivered"})
        )
    );
    let (code, ours_receipt) = fed(&ours_world, &binary(), &["reply", RUN], reply);
    assert_eq!(
        (
            code,
            serde_json::from_str::<Value>(&ours_receipt).expect("a receipt")
        ),
        (Some(0), serde_json::from_str(&receipt).expect("a receipt"))
    );
    alike(&theirs, &ours, "both `reply`s", normalized);

    // This build reads the reply the release appended, through its layout and
    // through its own `status`, exactly as it reads its own.
    let transport: Arc<dyn Transport> = Arc::new(LocalTransport::open(&theirs).expect("opens"));
    let channel = Channel::open(&transport).expect("the channel opens");
    assert_eq!(channel.pending().expect("a read"), None);
    let read_back = channel
        .claim_reply()
        .expect("a claim")
        .expect("the release's reply is read back");
    assert_eq!(
        (read_back.id, read_back.reply.message.as_deref()),
        (1, Some("answered by the release"))
    );
    let transport: Arc<dyn Transport> = Arc::new(LocalTransport::open(&ours).expect("opens"));
    Channel::open(&transport)
        .expect("the channel opens")
        .claim_reply()
        .expect("a claim")
        .expect("this build's reply is read back");
    alike(&theirs, &ours, "reading both replies back", normalized);
    assert_eq!(
        answered(&release_world, &binary(), &["status", RUN]),
        answered(&ours_world, &binary(), &["status", RUN]),
        "this build reads the channel {RELEASE} replied on differently from its own"
    );
}

#[test]
fn a_channel_the_0_28_2_wheel_writes_is_the_channel_this_build_writes_and_each_reads_the_other() {
    let release_world = recorded_world("release-channel-verbs-theirs");
    let ours_world = recorded_world("release-channel-verbs-ours");
    let theirs = seeded_with(&release_world, None);
    let ours = seeded_with(&ours_world, None);

    // The verbs that write a channel from empty, each binary over its own copy.
    for args in [
        &["surface", RUN, "--kind", "check-in", "--message", "first"][..],
        &["surface", RUN, "--kind", "check-in", "--message", "second"],
        &[
            "surface",
            RUN,
            "--kind",
            "finding",
            "--message",
            "a finding",
        ],
        &["next", RUN],
        &["next", RUN],
    ] {
        let theirs = answered(&release_world, release(), args);
        let mine = answered(&ours_world, &binary(), args);
        let said = |answer: &Value| {
            (
                answer["exit"].clone(),
                normalized(answer["stdout"].as_str().expect("a stdout")),
                answer["stderr"].clone(),
            )
        };
        assert_eq!(
            said(&mine),
            said(&theirs),
            "`onepipeline {args:?}` answered differently from {RELEASE}"
        );
        assert_eq!(mine["exit"], json!(0), "{mine}");
    }
    alike(&theirs, &ours, "the surfaces and both `next`s", normalized);

    // Each binary reads the other's directory exactly as it reads its own.
    for (program, name) in [(release(), RELEASE), (binary().as_path(), "this build")] {
        for args in [&["status", RUN][..], &["results", RUN]] {
            assert_eq!(
                answered(&ours_world, program, args),
                answered(&release_world, program, args),
                "{name} reads the other binary's channel differently from its own for {args:?}"
            );
        }
    }
}
