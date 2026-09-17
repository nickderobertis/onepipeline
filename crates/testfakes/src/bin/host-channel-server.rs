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
use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SurfaceFrame {
    kind: HostKind,
    message: String,
    #[serde(default = "blocking")]
    blocking: bool,
    #[serde(default)]
    node: Option<String>,
}

struct HostKind(String);

impl<'de> Deserialize<'de> for HostKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let word = String::deserialize(deserializer)?;
        let valid = !word.is_empty()
            && word.len() <= 64
            && word.bytes().enumerate().all(|(index, byte)| {
                byte.is_ascii_lowercase() || (index > 0 && (byte.is_ascii_digit() || byte == b'-'))
            });
        if !valid {
            return Err(serde::de::Error::custom(
                "a kind matches ^[a-z][a-z0-9-]{0,63}$",
            ));
        }
        Ok(Self(word))
    }
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
            "kind": frame.kind.0,
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
    let reply_window = match std::env::var("ONEPIPELINE_REPLY_TIMEOUT_SECONDS") {
        Ok(word) => word.parse::<u64>()?,
        Err(std::env::VarError::NotPresent) => 60,
        Err(failure) => return Err(failure.into()),
    };
    let options = ServeOptions {
        asker: std::env::var("ONEPIPELINE_CHANNEL_ASKER")
            .ok()
            .map(|word| Asker::new(&word, "ONEPIPELINE_CHANNEL_ASKER"))
            .transpose()?,
        about: None,
        session: std::env::var("ONEPIPELINE_SERVE_SESSION_SECONDS")
            .ok()
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
