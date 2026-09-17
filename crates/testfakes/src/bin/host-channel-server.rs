//! A host-owned, generic channel server used by the compiled-binary journeys.
//!
//! This deliberately knows only the bus's serving contract: a JSON frame is a
//! surface to raise or ask. Kinds and authors remain data chosen by each test.

use std::{io, path::PathBuf, sync::Arc, time::Duration};

use onemessagebus::{
    Answer, Asker, Codec, CodecFailure, CodecName, Config, Correlation, Layouts, Lifetime,
    QueueName, ServeOptions, ServeSession, TransportKinds,
};
use onemessagebus_agent::channel::{PlannerChannel, PLANNER_CHANNEL, SURFACES};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SurfaceFrame {
    /// The word as the test wrote it. The kind grammar is the engine's
    /// (`SurfaceKind`, Contract K) and is enforced where the engine reads the
    /// queue; a host that also spelled it would be a second copy to drift.
    kind: String,
    message: String,
    #[serde(default = "blocking")]
    blocking: bool,
    #[serde(default)]
    node: Option<String>,
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
        .map(|word| word.parse::<u64>())
        .transpose()?
        .unwrap_or(60);
    let options = ServeOptions {
        asker: optional("ONEPIPELINE_CHANNEL_ASKER")?
            .map(|word| Asker::new(&word, "ONEPIPELINE_CHANNEL_ASKER"))
            .transpose()?,
        about: None,
        session: optional("ONEPIPELINE_SERVE_SESSION_SECONDS")?
            .map(|word| word.parse::<u64>().map(Duration::from_secs))
            .transpose()?,
        reply_window: Duration::from_secs(reply_window),
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
