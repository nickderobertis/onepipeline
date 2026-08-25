//! A node validator, as `onepipeline start --node-validator` names one.
//!
//! **Not a double for anything in this stack.** The hook's whole promise is that
//! a command the *host* names is run, and the host's own is hundreds of lines of
//! rules over documents this crate has never seen. So what stands here is a real
//! validator: it reads the node off its stdin the way the contract says one
//! does, decides, and answers with an exit status and its own words on stderr.
//! A stub inside the crate under test would prove none of that.
//!
//! It is scripted from the same directory the sibling doubles are, so a journey
//! states what the host's rules say the way it states everything else:
//!
//!   `validator.refuse`   present → refuse every node, naming itself and then
//!                        this file's text on stderr; absent → accept
//!   `validator.silent`   present → refuse without reading stdin and without
//!                        saying anything, which is the answer a caller still
//!                        has to be able to act on
//!   `validator.chatter`  present → write this file's text to **stdout** before
//!                        answering, the way a host's rules engine narrates what
//!                        it checked
//!   `validator.flood`    present → after refusing, write this many bytes more to
//!                        stderr, the way a rules engine that dumps its whole
//!                        trace does; the caller's refusal must not grow with it
//!   `validator.signal`   present → end on a signal rather than an exit status,
//!                        which is a validator that crashed or was killed and is
//!                        still an answer the caller has to act on (Unix only)
//!
//! It names itself in every refusal, so a journey about which of three names a
//! launch resolved reads the answer off `onepipeline reply`'s own stderr — the
//! surface a manager reads a refusal from. Every invocation is also recorded to
//! `validator.jsonl`, carrying the name and the node, which is the only witness
//! there is to *what crossed the stdin* the contract describes.

use std::io::Read;

use onepipeline_testfakes as fake;
use serde::Deserialize;

/// A node's id, as this boundary accepts one.
///
/// Blank is not a node id, so it is not a value this type can hold: the check
/// happens where the document is read, and every later line has an id or the
/// program never got here.
#[derive(Debug)]
struct NodeId(String);

impl<'de> Deserialize<'de> for NodeId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let written = String::deserialize(deserializer)?;
        match written.trim().is_empty() {
            true => Err(serde::de::Error::custom(
                "a node crossed with a blank id, which names nothing this validator could check",
            )),
            false => Ok(Self(written)),
        }
    }
}

/// The node as it crosses the validator's stdin.
///
/// A **shape** rather than a bare JSON value: the contract says a plan node
/// arrives here, so a document that is not one — a list, a scalar, an object
/// with no `id` or a blank one — is a seam that broke, and a validator that read
/// it anyway would let a journey pass on a node nothing checked. The three
/// fields the journeys assert on are named; everything else a node carries is
/// kept as written, because this is a host's validator and a host reads the
/// whole node.
#[derive(Debug, Deserialize)]
struct OfferedNode {
    id: NodeId,
    #[serde(default)]
    task: Option<String>,
    #[serde(default)]
    amendment: Option<String>,
    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

impl OfferedNode {
    /// The node as this validator records it, which is the node as it arrived.
    fn recorded(&self) -> serde_json::Value {
        let mut document = serde_json::Map::new();
        document.insert("id".into(), serde_json::Value::from(self.id.0.clone()));
        for (key, value) in [("task", &self.task), ("amendment", &self.amendment)] {
            if let Some(value) = value {
                document.insert(key.into(), serde_json::Value::from(value.clone()));
            }
        }
        document.extend(self.rest.clone());
        serde_json::Value::Object(document)
    }
}

fn main() -> std::process::ExitCode {
    let dir = fake::script_dir();

    // The name this copy of the program was invoked as, which is the command
    // the launch resolved. Three names for one program is how a journey tells
    // the flag's validator from the environment's and from the config's.
    let invoked_as = std::env::args()
        .next()
        .map(|argv0| {
            std::path::Path::new(&argv0)
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or(argv0)
        })
        .unwrap_or_else(|| "unknown".to_string());

    // A validator that refuses without reading its input is answering rather
    // than failing, and what decides the edit is the status below — so this one
    // exits with stdin still unread, deliberately.
    // A validator that ends on a signal has no exit status at all — a crash, or
    // an operator's `kill` — and what the caller must not do is read that as a
    // verdict.
    #[cfg(unix)]
    if dir.join("validator.signal").is_file() {
        fake::append(
            &dir.join("validator.jsonl"),
            &serde_json::json!({"as": invoked_as, "node": serde_json::Value::Null}).to_string(),
        );
        // SAFETY: `raise` delivers a signal to this process and nothing else; it
        // is the only way to end without an exit status, which is the answer
        // being acted out.
        let raised = unsafe { libc::raise(libc::SIGKILL) };
        // Unreachable where the signal landed, because it ends this process. So
        // arriving here at all means the delivery failed, and a validator that
        // fell through into ordinary handling would answer the journey with a
        // verdict where the scenario asked for no status at all.
        fake::fail(&format!(
            "could not end on a signal: raise(SIGKILL) answered {raised}, so this validator \
             cannot act out a run that ends without an exit status"
        ));
    }

    if dir.join("validator.silent").is_file() {
        fake::append(
            &dir.join("validator.jsonl"),
            &serde_json::json!({"as": invoked_as, "node": serde_json::Value::Null}).to_string(),
        );
        return std::process::ExitCode::from(3);
    }

    let mut document = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut document) {
        fake::fail(&format!("cannot read the node on stdin: {error}"));
    }
    let node: OfferedNode = match serde_json::from_str(&document) {
        Ok(node) => node,
        Err(error) => {
            eprintln!("the node did not cross as a plan node: {error}: {document}");
            return std::process::ExitCode::from(1);
        }
    };
    fake::append(
        &dir.join("validator.jsonl"),
        &serde_json::json!({"as": invoked_as, "node": node.recorded()}).to_string(),
    );

    // A validator that narrates on stdout is ordinary — a host's rules engine
    // prints what it checked — and none of it is the caller's answer.
    if let Ok(chatter) = std::fs::read_to_string(dir.join("validator.chatter")) {
        println!("{}", chatter.trim());
    }

    match std::fs::read_to_string(dir.join("validator.refuse")) {
        Ok(reason) => {
            eprintln!("{invoked_as}: {}", reason.trim());
            flood(&dir);
            std::process::ExitCode::from(1)
        }
        Err(_) => std::process::ExitCode::SUCCESS,
    }
}

/// Write as much further stderr as the scenario asks for.
///
/// A rules engine that dumps its whole trace after the sentence that matters is
/// ordinary, and what the caller must not do is hold all of it or put all of it
/// in front of a manager. A count this file cannot read as a number of bytes is
/// a misconfigured scenario rather than a run of zero, so it is reported.
fn flood(dir: &std::path::Path) {
    let Ok(asked) = std::fs::read_to_string(dir.join("validator.flood")) else {
        return;
    };
    let Ok(bytes) = asked.trim().parse::<usize>() else {
        fake::fail(&format!(
            "validator.flood holds {:?}, which is not a number of bytes",
            asked.trim()
        ));
    };
    eprintln!("{}", "x".repeat(bytes));
}
