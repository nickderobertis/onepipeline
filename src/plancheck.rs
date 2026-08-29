//! `onepipeline plan check`: the engine's own loader, and whatever checks the
//! consumer registered, behind one entry point.
//!
//! A consumer that wanted to know whether a plan would launch used to
//! re-implement this crate's loader in its own language, and a
//! re-implementation drifts: it passes plans the launch then refuses, and
//! refuses plans the launch would have taken. So the loader that runs here is
//! the launch's own — [`Store::read_plan`] and [`crate::graph::check`], which is
//! every refusal `start` makes before it dispatches anything and no other rule —
//! and a consumer's own rules become **checks this verb runs** rather than a
//! second implementation of that loader.
//!
//! The two kinds of refusal stay apart in the answer: the engine's carry
//! `source: "engine"` and come first, and each registered check's follow in the
//! order its `--check` flags were given, carrying the path as it was given. A
//! check that could not be **run** is reported separately again, because reading
//! it as an accept is the one answer that stops anybody looking.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cli::PlanCheckArgs;
use crate::error::{Result, EXIT_QUEUED, EXIT_REFUSED, EXIT_SUCCESS};
use crate::refusal::Refusal;
use crate::taskgraph::{Load, QualifiedId, Store};

/// The variable every registered check is spawned with.
///
/// It says which document shape is on the check's stdin, so a check written for
/// a later one can tell what it was handed rather than guessing from the keys.
pub const SCHEMA_ENV: &str = "ONEPIPELINE_PLAN_CHECK_SCHEMA";

/// The schema the document on a check's stdin is written at.
pub const SCHEMA_VERSION: &str = "1";

/// The loader and every check accepted.
///
/// The three codes are the ones this crate already spends — a fourth would be a
/// code the contract does not name — and they are chosen to match the consuming
/// wrapper's own convention, so it forwards this status rather than translating
/// it.
const ACCEPTED: i32 = EXIT_SUCCESS;

/// At least one refusal, from either source.
const REFUSED: i32 = EXIT_QUEUED;

/// The project could not be read at all, or a registered check could not be run.
const NOT_ANSWERED: i32 = EXIT_REFUSED;

/// What `source` an engine refusal carries. A registered check's own is the
/// path the `--check` flag named, verbatim.
pub const ENGINE: &str = "engine";

/// Which side made one refusal.
///
/// Two cases rather than a string that is one of them by convention: the wire
/// value `engine` is reserved, and a `String` there would let a check registered
/// at a path spelled `engine` be indistinguishable inside this process from the
/// loader itself. What the two serialise to is the contract's, and it is written
/// at the boundary rather than carried around.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Source {
    /// The plan loader this crate runs.
    Engine,
    /// A registered check, named by the path its `--check` flag gave.
    Check(String),
}

impl Serialize for Source {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Engine => serializer.serialize_str(ENGINE),
            Self::Check(path) => serializer.serialize_str(path),
        }
    }
}

impl std::fmt::Display for Source {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Engine => formatter.write_str(ENGINE),
            Self::Check(path) => formatter.write_str(path),
        }
    }
}

/// One refusal, from either side, as the answer carries it.
#[derive(Debug, Clone, PartialEq, Serialize)]
struct Reported {
    /// Which side made it.
    source: Source,
    /// Always present, and null where the refusal is about no one node.
    node: Option<String>,
    /// Always present, and null where it is about no one field.
    field: Option<String>,
    /// Why. Never empty: an engine refusal's is the sentence `start` prints,
    /// composed here, and a check's is [`Reason`], which refuses a blank one
    /// where it arrives.
    reason: Reason,
}

/// One registered check that could not be run.
#[derive(Debug, Clone, PartialEq, Serialize)]
struct Unrunnable {
    /// The path as it was given.
    check: String,
    /// Its exit status, or null where there was no process to have one.
    exit_code: Option<i32>,
    /// What it said for itself.
    stderr: String,
    /// Whether this verb ever tried to start it, which decides the exit status
    /// and is not part of the answer's own shape.
    #[serde(skip)]
    ran: Ran,
}

/// How far this verb got with one registered check.
///
/// The distinction the exit status turns on, in the type rather than in the
/// wording of a message: a check that was **attempted** and could not be run
/// leaves what it would have said unknown, and a check the loader's own refusal
/// stopped was never asked, so the refusal is what the status reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ran {
    /// This verb tried to start it.
    Attempted,
    /// The loader refused first, so there was no loaded plan to hand it.
    StoppedByTheLoader,
}

/// What one registered check answered with.
///
/// External input, so an answer this build cannot read is a check that could not
/// be run rather than one that accepted: `deny_unknown_fields` is what makes a
/// misspelled key say so instead of being dropped into an empty accept, and
/// **no key here carries a default** — the contract states each one as always
/// present, and a missing `refusals` read as an empty list is exactly the false
/// accept this verb exists to stop.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    refusals: Vec<AnswerRefusal>,
}

/// One refusal a registered check made.
///
/// `node` and `field` are `Option` because their **value** may be null, not
/// because the key may be absent: [`absent_key`] holds every one of the three to
/// being *there*, which is what the contract says of each.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AnswerRefusal {
    node: Option<String>,
    field: Option<String>,
    reason: Reason,
}

/// A refusal's own words, which are never empty.
///
/// The invariant is in the type rather than in a pass afterwards: a blank reason
/// is a refusal that says nothing, and reading one is how a consumer ends up
/// with a plan refused for no stated cause. Deserialising is where it is
/// enforced, because that is where the value arrives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
struct Reason(String);

impl<'de> Deserialize<'de> for Reason {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let said = String::deserialize(deserializer)?;
        if said.trim().is_empty() {
            return Err(serde::de::Error::custom(
                "a refusal's reason is the whole of what it says, and this one is blank",
            ));
        }
        Ok(Self(said))
    }
}

/// Read one project, run every registered check over it, and report.
pub(crate) fn check(args: &PlanCheckArgs) -> Result<i32> {
    match read(args) {
        Ok((refusals, unrunnable)) => Ok(report(args, &refusals, &unrunnable)),
        // The project could not be read at all — no binary, a store that
        // answered badly, an id naming nothing. `--json` still prints exactly
        // one object, because a consumer parses this verb's stdout without first
        // asking which failure it met; the diagnosis goes to stderr, where every
        // other refusal this binary makes goes.
        Err(error) => {
            eprintln!("onepipeline: {error}");
            // Not accepted: a project nothing could read is the one answer that
            // must never look like a plan that passed.
            print(args, false, &[], &[]);
            Ok(NOT_ANSWERED)
        }
    }
}

/// The loader, and every registered check it leaves something to hand.
///
/// `Err` is the project not being readable at all, which is a different answer
/// from a plan the schema refuses: see [`Load`].
fn read(args: &PlanCheckArgs) -> Result<(Vec<Reported>, Vec<Unrunnable>)> {
    let store = Store::resolve()?;
    let project: QualifiedId = args.project.parse()?;

    // A check is handed the *loaded* plan, so a loader refusal leaves nothing to
    // hand it. Reporting each as not run is the whole point: a check that never
    // ran has said nothing, and reading its silence as an accept is what a
    // drifting re-implementation already did once.
    let refusal = match store.read_plan(&project) {
        Err(Load::Unreadable(error)) => return Err(error),
        Err(Load::Refused(refusal)) => Some(refusal),
        Ok(read) => match crate::graph::check(&read.plan) {
            Err(refusal) => Some(refusal),
            Ok(()) => {
                let document = document(&read);
                let mut refusals = Vec::new();
                let mut unrunnable = Vec::new();
                for path in &args.checks {
                    match offer(path, &document) {
                        Ok(answered) => refusals.extend(answered),
                        Err(why) => unrunnable.push(why),
                    }
                }
                return Ok((refusals, unrunnable));
            }
        },
    };
    let refusal = refusal.expect("this arm is only reached where the loader refused");
    Ok((
        vec![engine_refusal(refusal)],
        args.checks
            .iter()
            .map(|path| Unrunnable {
                check: path.display().to_string(),
                exit_code: None,
                stderr: "the plan loader refused the project, so there was no loaded plan to \
                         hand this check; it did not run"
                    .to_owned(),
                ran: Ran::StoppedByTheLoader,
            })
            .collect(),
    ))
}

/// Print the answer and say what the status is.
fn report(args: &PlanCheckArgs, refusals: &[Reported], unrunnable: &[Unrunnable]) -> i32 {
    print(
        args,
        refusals.is_empty() && unrunnable.is_empty(),
        refusals,
        unrunnable,
    );
    // A check that was *attempted* and could not be run is the exit-2 case: what
    // it would have said is unknown, and nothing else in the answer stands in
    // for it. A check the loader's own refusal stopped was never asked, and the
    // refusal it was stopped by is what the status reports.
    if unrunnable.iter().any(|report| report.ran == Ran::Attempted) {
        NOT_ANSWERED
    } else if refusals.is_empty() {
        ACCEPTED
    } else {
        REFUSED
    }
}

/// Write the answer, as one JSON object or as a line per refusal.
fn print(args: &PlanCheckArgs, accepted: bool, refusals: &[Reported], unrunnable: &[Unrunnable]) {
    if args.json {
        // The project as it was named: an id this build could not even parse is
        // still the one the caller asked about.
        let answer = json!({
            "project": args.project,
            "accepted": accepted,
            "refusals": refusals,
            "unrunnable": unrunnable,
        });
        // Built from `json!` over types that serialise, so there is nothing here
        // that can fail to render — and a check that answered would rather be
        // reported than lost to a fallible print.
        println!("{answer}");
        return;
    }

    for refusal in refusals {
        println!(
            "{}: {}{}{}",
            refusal.source,
            refusal
                .node
                .as_ref()
                .map(|node| format!("node '{node}': "))
                .unwrap_or_default(),
            refusal
                .field
                .as_ref()
                .map(|field| format!("`{field}`: "))
                .unwrap_or_default(),
            refusal.reason.0
        );
    }
    // A check that could not be run is the exit-2 diagnosis rather than an
    // answer about the plan, so it goes where this binary's diagnoses go.
    for report in unrunnable {
        eprintln!(
            "{}: could not be run ({}): {}",
            report.check,
            report.exit_code.map_or_else(
                || "no exit status".to_owned(),
                |code| format!("exit {code}")
            ),
            report.stderr
        );
    }
    if accepted {
        println!("{}: accepted", args.project);
    }
}

fn engine_refusal(refusal: Refusal) -> Reported {
    Reported {
        source: Source::Engine,
        node: refusal.node,
        field: refusal.field,
        reason: Reason(refusal.message),
    }
}

/// The document a registered check is handed on its stdin.
///
/// `name` and `goal` are written even where the plan states neither, because a
/// check reads a key that is there and null rather than discovering that this
/// plan happens to omit it. Each task is the engine's own loaded node — every
/// default resolved, the repository identity taken off whichever spelling the
/// store held it in, and each dependency resolved to a node id — with the
/// store's own metadata map for that task beside it, verbatim: a consumer's
/// checks read keys outside this crate's reserved namespace, and dropping them
/// would leave those checks unable to run here at all.
fn document(read: &crate::taskgraph::Read) -> Value {
    let tasks: Vec<Value> = read
        .plan
        .tasks
        .iter()
        .map(|node| {
            let mut written = serde_json::to_value(node).unwrap_or_else(|_| json!({}));
            if let Some(map) = written.as_object_mut() {
                map.insert(
                    "metadata".to_owned(),
                    json!(read.metadata.get(&node.id).cloned().unwrap_or_default()),
                );
            }
            written
        })
        .collect();
    json!({
        "schema_version": read.plan.schema_version,
        "name": read.plan.name,
        "goal": read.plan.goal,
        "concurrency": read.plan.concurrency,
        "tasks": tasks,
    })
}

/// Offer the plan to one registered check.
///
/// `Ok` is a check that **ran** — whatever its refusals list holds. Everything
/// else is a check that could not be run: a path that is not there or not
/// executable, a non-zero exit, or an answer this build cannot read.
fn offer(path: &Path, document: &Value) -> std::result::Result<Vec<Reported>, Unrunnable> {
    let named = path.display().to_string();
    let cannot = |exit_code: Option<i32>, stderr: String| Unrunnable {
        check: named.clone(),
        exit_code,
        stderr,
        ran: Ran::Attempted,
    };
    // Against the working directory this command was run from, which is also the
    // one the check itself runs in: a consumer registers a check beside the plan
    // it is checking, and a path that resolved against anything else would name
    // a different file to the two sides. A directory this process cannot read is
    // a boundary that could not be established, so the check is one that could
    // not be run rather than one resolved against something else.
    let resolved = resolve(path).map_err(|why| cannot(None, why))?;
    let mut child = Command::new(&resolved)
        .env(SCHEMA_ENV, SCHEMA_VERSION)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            cannot(
                None,
                format!("{} cannot be run: {error}", resolved.display()),
            )
        })?;
    let written = serde_json::to_vec(document).unwrap_or_default();
    if let Some(stdin) = child.stdin.as_mut() {
        // A check that read what it wanted and closed its stdin is answering,
        // not failing, so a broken pipe here is left to the answer to settle.
        let _ = stdin.write_all(&written);
    }
    drop(child.stdin.take());
    // Both streams, bounded and read at once. A check is somebody else's
    // program: reading either without a bound lets it exhaust this process's
    // memory before a byte of it has been validated, and reading them one after
    // the other deadlocks against a check that fills the pipe this one is not
    // draining. Each handle is dropped at its bound, which is what stops a check
    // that keeps writing rather than leaving it blocked on a pipe nobody reads.
    let mut out = child.stdout.take();
    let reading = std::thread::spawn({
        let mut err = child.stderr.take();
        // Dropped inside the thread, at its bound, for the reason above.
        move || err.as_mut().map(bounded).unwrap_or_default()
    });
    let stdout = out.as_mut().map(bounded).unwrap_or_default();
    drop(out);
    let stderr_bytes = reading.join().unwrap_or_default();
    let status = child
        .wait()
        .map_err(|error| cannot(None, format!("{named} could not be waited for: {error}")))?;
    let stderr = String::from_utf8_lossy(&stderr_bytes.said)
        .trim()
        .to_owned();
    if !status.success() {
        return Err(cannot(status.code(), stderr));
    }
    if stdout.past_the_bound {
        return Err(cannot(
            status.code(),
            format!("answered with more than the {MAX_ANSWER_BYTES} bytes this build reads"),
        ));
    }
    // The keys the contract states as **always present**, checked before the
    // answer is typed: serde reads an absent `Option` field as null, so a check
    // omitting `node`, `field`, or `refusals` itself would otherwise be read as
    // having said something it did not.
    let answered: Value = serde_json::from_slice(&stdout.said).map_err(|error| {
        cannot(
            status.code(),
            format!(
                "answered with something this build cannot read: {error}; it said {:?}{}",
                String::from_utf8_lossy(&stdout.said).trim(),
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(" (stderr: {stderr})")
                }
            ),
        )
    })?;
    if let Some(key) = absent_key(&answered) {
        return Err(cannot(
            status.code(),
            format!("answered with no `{key}`, which a check's answer always carries"),
        ));
    }
    let answer: Answer = serde_json::from_value(answered).map_err(|error| {
        cannot(
            status.code(),
            format!(
                "answered with something this build cannot read: {error}; it said {:?}{}",
                String::from_utf8_lossy(&stdout.said).trim(),
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(" (stderr: {stderr})")
                }
            ),
        )
    })?;
    Ok(answer
        .refusals
        .into_iter()
        .map(|refusal| Reported {
            source: Source::Check(named.clone()),
            node: refusal.node,
            field: refusal.field,
            reason: refusal.reason,
        })
        .collect())
}

/// The first key the contract requires that this answer does not carry.
///
/// Presence only: what each one *is* is the schema's, which reads it next. An
/// answer that is not an object at all, or whose `refusals` is not a list, has
/// no key to name and is left to that reading to refuse by type.
fn absent_key(answered: &Value) -> Option<String> {
    let object = answered.as_object()?;
    if !object.contains_key("refusals") {
        return Some("refusals".to_owned());
    }
    let refusals = object.get("refusals")?.as_array()?;
    for refusal in refusals {
        let stated = refusal.as_object()?;
        for key in ["node", "field", "reason"] {
            if !stated.contains_key(key) {
                return Some(format!("refusals[].{key}"));
            }
        }
    }
    None
}

/// The most of one check's stdout or stderr this build reads.
///
/// A refusals list is a handful of sentences and a diagnosis is a few lines, so
/// this is past anything a check has to say by any margin — and it is the bound
/// that keeps somebody else's program from exhausting this process before a byte
/// of what it wrote has been validated.
const MAX_ANSWER_BYTES: u64 = 1 << 20;

/// One bounded read of a check's stream.
#[derive(Default)]
struct Bounded {
    /// What it said, up to [`MAX_ANSWER_BYTES`].
    said: Vec<u8>,
    /// Whether there was more, which makes the answer one this build cannot
    /// read rather than a truncated one it acts on.
    past_the_bound: bool,
}

/// Read one stream to the bound, and say whether it reached it.
fn bounded(stream: &mut impl std::io::Read) -> Bounded {
    let mut said = Vec::new();
    // One past the bound, so a stream that is exactly it is not reported as
    // having overrun.
    let read = stream.take(MAX_ANSWER_BYTES + 1).read_to_end(&mut said);
    let past_the_bound = read.is_ok() && said.len() as u64 > MAX_ANSWER_BYTES;
    said.truncate(usize::try_from(MAX_ANSWER_BYTES).unwrap_or(usize::MAX));
    Bounded {
        said,
        past_the_bound,
    }
}

/// A relative path against the working directory; anything else as it was given.
fn resolve(path: &Path) -> std::result::Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    std::env::current_dir()
        .map(|dir| dir.join(path))
        .map_err(|error| {
            format!(
                "{} is relative and this process cannot read its own working directory, so there \
             is nothing to resolve it against: {error}",
                path.display()
            )
        })
}
