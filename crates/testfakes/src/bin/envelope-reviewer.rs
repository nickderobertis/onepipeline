//! An envelope reviewer, as `onepipeline start --envelope-reviewer` names one.
//!
//! **Not a double for anything in this stack**, for the reason the sibling
//! `node-validator` is not: the hook's whole promise is that a command the
//! *host* names is run over a document this crate has never seen, and the host's
//! own reviewer is a plan-quality review of the edit against the run's goal. So
//! what stands here is a real reviewer — it reads the envelope off its stdin the
//! way the contract says one does, decides, and answers with an exit status and
//! its own words on stderr.
//!
//! It is scripted from the same directory the sibling doubles are:
//!
//!   `reviewer.refuse`    present → refuse every envelope, naming itself, the
//!                        node it objected to, and this file's text on stderr;
//!                        absent → accept
//!   `reviewer.chatter`   present → write this file's text to **stdout** before
//!                        answering, the way a review that narrates what it
//!                        checked does
//!
//! It names itself in every refusal, so a journey about which of three names a
//! launch resolved reads the answer off `onepipeline reply`'s own stderr. Every
//! invocation is also recorded to `reviewer.jsonl`, carrying the name and the
//! envelope as it arrived, which is the only witness there is to *what crossed
//! the stdin*.

use std::io::Read;

use onepipeline_testfakes as fake;
use serde::Deserialize;

/// A word this boundary accepts as a name — a node id, or an op.
///
/// Blank names nothing a reviewer could be about, so it is not a value these
/// types can hold: the check happens where the document is read, and every later
/// line has a name or the program never got here.
#[derive(Debug)]
struct Named(String);

impl<'de> Deserialize<'de> for Named {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let written = String::deserialize(deserializer)?;
        match written.trim().is_empty() {
            true => Err(serde::de::Error::custom(
                "an envelope crossed naming something blank, which is nothing this reviewer \
                 could review",
            )),
            false => Ok(Self(written)),
        }
    }
}

/// One node the envelope introduces or changes, with the op that produced it.
#[derive(Debug, Deserialize)]
struct ChangedNode {
    op: Named,
    node: OfferedNode,
}

/// A node as it crosses the reviewer's stdin: the whole plan node, because a
/// plan-quality review reads the prose it is about to judge.
#[derive(Debug, Deserialize)]
struct OfferedNode {
    id: Named,
    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

impl OfferedNode {
    /// The node as this reviewer records it, which is the node as it arrived.
    fn recorded(&self) -> serde_json::Value {
        let mut document = serde_json::Map::new();
        document.insert("id".into(), serde_json::Value::from(self.id.0.clone()));
        document.extend(self.rest.clone());
        serde_json::Value::Object(document)
    }
}

/// The plan the edits are being made into, as it crosses.
///
/// `tasks` is required rather than defaulted: a review that could not see the
/// plan is the review this hook exists to make possible, and a document arriving
/// without one is a seam that broke rather than a plan with no nodes.
#[derive(Debug, Deserialize)]
struct ReviewedPlan {
    tasks: Vec<OfferedNode>,
    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

/// The envelope under review, as the contract states the document.
#[derive(Debug, Deserialize)]
struct EnvelopeUnderReview {
    #[serde(default)]
    goal: Option<Named>,
    changes: Vec<ChangedNode>,
    plan: ReviewedPlan,
}

impl EnvelopeUnderReview {
    /// The envelope as this reviewer records it.
    fn recorded(&self) -> serde_json::Value {
        serde_json::json!({
            "goal": self.goal.as_ref().map(|goal| goal.0.clone()),
            "changes": self
                .changes
                .iter()
                .map(|change| serde_json::json!({"op": change.op.0, "node": change.node.recorded()}))
                .collect::<Vec<_>>(),
            "plan": {
                "tasks": self.plan.tasks.iter().map(OfferedNode::recorded).collect::<Vec<_>>(),
                "rest": serde_json::Value::Object(self.plan.rest.clone()),
            },
        })
    }

    /// What a refusal is about: the first node this envelope changes, or the
    /// plan itself for an envelope that changes none.
    fn objection(&self) -> String {
        match self.changes.first() {
            Some(change) => format!("node '{}'", change.node.id.0),
            None => "this envelope's edits to the plan".to_string(),
        }
    }
}

fn main() -> std::process::ExitCode {
    let dir = fake::script_dir();

    // The name this copy of the program was invoked as, which is the command the
    // launch resolved. Three names for one program is how a journey tells the
    // flag's reviewer from the environment's and from the config's.
    let invoked_as = std::env::args()
        .next()
        .map(|argv0| {
            std::path::Path::new(&argv0)
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or(argv0)
        })
        .unwrap_or_else(|| "unknown".to_string());

    let mut document = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut document) {
        fake::fail(&format!("cannot read the envelope on stdin: {error}"));
    }
    let envelope: EnvelopeUnderReview = match serde_json::from_str(&document) {
        Ok(envelope) => envelope,
        Err(error) => {
            eprintln!("the envelope did not cross as one document: {error}: {document}");
            return std::process::ExitCode::from(1);
        }
    };
    fake::append(
        &dir.join("reviewer.jsonl"),
        &serde_json::json!({"as": invoked_as, "envelope": envelope.recorded()}).to_string(),
    );

    // A reviewer that narrates on stdout is ordinary — a review prints what it
    // checked — and none of it is the caller's answer.
    if let Some(chatter) = scenario(&dir.join("reviewer.chatter")) {
        println!("{}", chatter.trim());
    }

    match scenario(&dir.join("reviewer.refuse")) {
        Some(reason) => {
            // The node it objected to, in its own sentence: an envelope is no
            // longer one command, so a reason that named none would leave a
            // manager reading a refusal with nothing to look at.
            eprintln!("{invoked_as}: {}: {}", envelope.objection(), reason.trim());
            std::process::ExitCode::from(1)
        }
        None => std::process::ExitCode::SUCCESS,
    }
}

/// What one scenario file states, or `None` when that scenario is simply not
/// set.
///
/// Read exactly as the sibling validator reads its own, and for the same reason:
/// the file not being there is the scenario being off, and every *other* way a
/// read can fail is a scenario set to something unreadable, which is not the
/// same answer. Folded together they would be, and the fold is fail-open — an
/// unreadable `reviewer.refuse` would fall through to accepting the envelope, so
/// a journey about refusal would pass having proved the opposite.
fn scenario(path: &std::path::Path) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => fake::fail(&format!(
            "{} is there and this reviewer cannot read it ({error}), so what it scripts \
             is unknown rather than unset",
            path.display()
        )),
    }
}
