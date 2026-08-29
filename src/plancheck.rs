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

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cli::PlanCheckArgs;
use crate::error::{Error, Result, EXIT_QUEUED, EXIT_REFUSED, EXIT_SUCCESS};
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

/// One refusal, from either side, as the answer carries it.
#[derive(Debug, Clone, PartialEq, Serialize)]
struct Reported {
    /// `engine`, or the check's path as it was given.
    source: String,
    /// Always present, and null where the refusal is about no one node.
    node: Option<String>,
    /// Always present, and null where it is about no one field.
    field: Option<String>,
    /// Why. Never empty.
    reason: String,
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
}

/// What one registered check answered with.
///
/// External input, so an answer this build cannot read is a check that could not
/// be run rather than one that accepted: `deny_unknown_fields` is what makes a
/// misspelled key say so instead of being dropped into an empty accept.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    #[serde(default)]
    refusals: Vec<AnswerRefusal>,
}

/// One refusal a registered check made.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AnswerRefusal {
    #[serde(default)]
    node: Option<String>,
    #[serde(default)]
    field: Option<String>,
    reason: String,
}

/// Read one project, run every registered check over it, and report.
pub(crate) fn check(args: &PlanCheckArgs) -> Result<i32> {
    let store = Store::resolve()?;
    let project: QualifiedId = args.project.parse()?;

    let mut refusals: Vec<Reported> = Vec::new();
    let mut unrunnable: Vec<Unrunnable> = Vec::new();
    // A check is handed the *loaded* plan, so a loader refusal leaves nothing to
    // hand it. Reporting each as not run is the whole point: a check that never
    // ran has said nothing, and reading its silence as an accept is what a
    // drifting re-implementation already did once.
    match store.read_plan(&project) {
        Err(Load::Unreadable(error)) => return Err(error),
        Err(Load::Refused(refusal)) => {
            refusals.push(engine_refusal(refusal));
            for path in &args.checks {
                unrunnable.push(Unrunnable {
                    check: path.display().to_string(),
                    exit_code: None,
                    stderr: "not run: the plan loader refused the project, so there was no \
                             loaded plan to hand it"
                        .to_owned(),
                });
            }
        }
        Ok(read) => {
            if let Err(refusal) = crate::graph::check(&read.plan) {
                refusals.push(engine_refusal(refusal));
                for path in &args.checks {
                    unrunnable.push(Unrunnable {
                        check: path.display().to_string(),
                        exit_code: None,
                        stderr: "not run: the plan loader refused the project, so there was no \
                                 loaded plan to hand it"
                            .to_owned(),
                    });
                }
            } else {
                let document = document(&read);
                for path in &args.checks {
                    match offer(path, &document) {
                        Ok(answered) => refusals.extend(answered),
                        Err(why) => unrunnable.push(why),
                    }
                }
            }
        }
    }

    // A check that was *attempted* and could not be run is the exit-2 case: what
    // it would have said is unknown, and nothing else in the answer stands in
    // for it. A check the loader's own refusal stopped is not attempted, and the
    // refusal it was stopped by is what the exit code reports.
    let attempted = unrunnable
        .iter()
        .any(|report| !report.stderr.starts_with("not run:"));
    let accepted = refusals.is_empty() && unrunnable.is_empty();
    let code = if attempted {
        NOT_ANSWERED
    } else if refusals.is_empty() {
        ACCEPTED
    } else {
        REFUSED
    };

    if args.json {
        let answer = json!({
            "project": project.as_str(),
            "accepted": accepted,
            "refusals": refusals,
            "unrunnable": unrunnable,
        });
        println!(
            "{}",
            serde_json::to_string(&answer).map_err(|error| Error::Invalid(format!(
                "the answer will not serialise: {error}"
            )))?
        );
        return Ok(code);
    }

    for refusal in &refusals {
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
            refusal.reason
        );
    }
    for report in &unrunnable {
        println!(
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
        println!("{}: accepted", project.as_str());
    }
    Ok(code)
}

fn engine_refusal(refusal: Refusal) -> Reported {
    Reported {
        source: ENGINE.to_owned(),
        node: refusal.node,
        field: refusal.field,
        reason: refusal.message,
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
    };
    // Against the working directory this command was run from, which is also the
    // one the check itself runs in: a consumer registers a check beside the plan
    // it is checking, and a path that resolved against anything else would name
    // a different file to the two sides.
    let resolved = resolve(path);
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
    let output = child
        .wait_with_output()
        .map_err(|error| cannot(None, format!("{named} could not be waited for: {error}")))?;
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !output.status.success() {
        return Err(cannot(output.status.code(), stderr));
    }
    let answer: Answer = serde_json::from_slice(&output.stdout).map_err(|error| {
        cannot(
            output.status.code(),
            format!(
                "answered with something this build cannot read: {error}; it said {:?}{}",
                String::from_utf8_lossy(&output.stdout).trim(),
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(" (stderr: {stderr})")
                }
            ),
        )
    })?;
    for refusal in &answer.refusals {
        if refusal.reason.trim().is_empty() {
            return Err(cannot(
                output.status.code(),
                "answered with a refusal carrying no reason".to_owned(),
            ));
        }
    }
    Ok(answer
        .refusals
        .into_iter()
        .map(|refusal| Reported {
            source: named.clone(),
            node: refusal.node,
            field: refusal.field,
            reason: refusal.reason,
        })
        .collect())
}

/// A relative path against the working directory; anything else as it was given.
fn resolve(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir().map_or_else(|_| path.to_path_buf(), |dir| dir.join(path))
}
