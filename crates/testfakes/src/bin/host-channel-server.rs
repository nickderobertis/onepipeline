//! A host-owned, generic channel server used by the compiled-binary journeys.
//!
//! This deliberately knows only the bus's serving contract: it is one codec the
//! bus's library server drives, and what it reads off its stdin is a surface to
//! raise or a correlation to listen for again. Kinds and authors remain data
//! chosen by each test.

use std::{io, path::PathBuf, sync::Arc, time::Duration};

use onemessagebus::{
    Answer, Asker, Codec, CodecFailure, CodecName, Config, Correlation, Layouts, Lifetime,
    QueueName, ServeOptions, ServeSession, TransportKinds,
};
use onemessagebus_agent::channel::{PlannerChannel, PLANNER_CHANNEL, SURFACES};
use serde::Deserialize;
use serde_json::{json, Value};

// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] The frames
// below are this double's own stdin protocol, and this file is their one source:
// the bus's `Codec` trait hands a codec an opaque string and leaves the frame's
// shape to the codec, so there is no bus-side schema for these to drift from,
// and the journeys that write them are the only other party.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SurfaceFrame {
    /// The word as the test wrote it, and nothing checked about it here. The
    /// kind grammar is the engine's (`SurfaceKind`, Contract K), enforced where
    /// an operator raises a kind — `surface --kind` — while whatever any writer
    /// queues is relayed as written; so this double writes the word the test
    /// chose, as `onemessagebus send` would, and a second spelling of the
    /// grammar here would be one more copy to drift.
    // llmlint: ignore-block[boundary_inputs_validated] the kind is deliberately
    // unchecked: Contract K relays a queued kind as written, and the journeys that
    // hand this double a kind are the ones proving that relay, malformed words included.
    // llmlint: ignore-block[invalid_states_unrepresentable] and for the same reason it
    // is a bare word rather than the engine's `SurfaceKind`: a double that could only
    // hold a well-formed kind could not hand the engine an ill-formed one to relay.
    kind: String,
    // llmlint: ignore-end[invalid_states_unrepresentable]
    // llmlint: ignore-end[boundary_inputs_validated]
    message: String,
    #[serde(default = "blocking")]
    blocking: bool,
    // llmlint: ignore-block[boundary_inputs_validated] the node a surface names is
    // the test's word, written through as the record's `workstream` — which the
    // engine itself holds as a plain string, and reads as the root of the subtree
    // a blocking surface holds: a name no node has holds nothing, by design.
    // llmlint: ignore-block[invalid_states_unrepresentable] so a bare word here, for
    // the same reason `kind` is one: what the engine does with a workstream it
    // cannot place is the engine's to decide, and a double that could only name a
    // node the run has could not hand it one to decide about.
    #[serde(default)]
    node: Option<String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
    // llmlint: ignore-end[boundary_inputs_validated]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RelistenFrame {
    correlation: Correlation,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Frame {
    Relisten(RelistenFrame),
    Surface(SurfaceFrame),
}
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

fn blocking() -> bool {
    true
}

struct SurfaceCodec(CodecName);

impl Codec for SurfaceCodec {
    fn name(&self) -> &CodecName {
        &self.0
    }

    fn answer(
        &mut self,
        frame: &str,
        session: &mut ServeSession<'_>,
    ) -> Result<Value, CodecFailure> {
        let frame: Frame = serde_json::from_str(frame).map_err(|failure| {
            CodecFailure::Refused(format!("the host received a bad frame: {failure}"))
        })?;
        if let Frame::Relisten(frame) = frame {
            let lifetime = session
                .options()
                .asker
                .clone()
                .map_or(Lifetime::Session, Lifetime::Durable);
            let pending = session
                .bus()
                .listen::<Value>(session.queue(), &frame.correlation, &lifetime)
                .map_err(|failure| CodecFailure::Refused(failure.to_string()))?;
            let answer = pending.wait(session.options().reply_window);
            return Ok(match answer {
                Answer::Reply(reply) => reply.get("reply").cloned().unwrap_or(reply),
                other => json!({"answer": other.word(), "correlation": frame.correlation}),
            });
        }
        let Frame::Surface(frame) = frame else {
            unreachable!("the relisten frame returned above")
        };
        let mut surface = json!({
            "kind": frame.kind,
            "message": frame.message,
            "source": "proposal",
            "blocking": frame.blocking,
        });
        if let Some(node) = frame.node {
            surface["workstream"] = Value::String(node);
        }
        let (correlation, answer) = session
            .ask(surface, frame.blocking)
            .map_err(|failure| CodecFailure::Failed(failure.to_string()))?;
        Ok(match answer {
            Answer::Reply(reply) => reply.get("reply").cloned().unwrap_or(reply),
            other => json!({"answer": other.word(), "correlation": correlation}),
        })
    }
}

/// An environment value that may be absent, but not unreadable: a variable that is
/// set to something this process cannot read is refused rather than served as if
/// nobody had set it.
fn optional(name: &str) -> Result<Option<String>, String> {
    match std::env::var(name) {
        Ok(word) => Ok(Some(word)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(failure) => Err(format!(
            "{name} is set to a value this host cannot read: {failure}"
        )),
    }
}

/// A bound spelled in whole seconds. Zero is refused with the rest of what is not
/// a duration: a wait or a session of no length is a value nobody meant.
fn seconds(name: &str, word: &str) -> Result<Duration, String> {
    word.parse::<std::num::NonZeroU64>()
        .map(|seconds| Duration::from_secs(seconds.get()))
        .map_err(|failure| {
            format!("{name} is set to {word:?}, which is not a whole number of seconds above zero: {failure}")
        })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let channel = PathBuf::from(
        args.next()
            .ok_or("usage: host-channel-server CHANNEL_DIR")?,
    );
    if args.next().is_some() {
        return Err("usage: host-channel-server CHANNEL_DIR".into());
    }
    let config = Config::local(channel, Some(PLANNER_CHANNEL));
    let layouts = Layouts::new().with(Arc::new(PlannerChannel));
    let bus = config.resolve(&layouts, &TransportKinds::builtin())?;
    let mut codec = SurfaceCodec("host-surface".parse()?);
    // llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] These names are
    // compatibility inputs of this test-only host, deliberately exercised by ported
    // historical journeys. The production engine owns only ASKER_ENV; the other two
    // are not product contracts and disappear with those fixtures.
    let reply_window = optional("ONEPIPELINE_REPLY_TIMEOUT_SECONDS")?
        .map(|word| seconds("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", &word))
        .transpose()?
        .unwrap_or(Duration::from_secs(60));
    let options = ServeOptions {
        asker: optional("ONEPIPELINE_CHANNEL_ASKER")?
            .map(|word| Asker::new(&word, "ONEPIPELINE_CHANNEL_ASKER"))
            .transpose()?,
        about: None,
        session: optional("ONEPIPELINE_SERVE_SESSION_SECONDS")?
            .map(|word| seconds("ONEPIPELINE_SERVE_SESSION_SECONDS", &word))
            .transpose()?,
        reply_window,
    };
    // llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]
    let queue: QueueName = SURFACES.parse()?;
    bus.serve(
        &queue,
        &mut codec,
        &options,
        Box::new(io::BufReader::new(io::stdin())),
        &mut io::stdout(),
    )?;
    Ok(())
}
