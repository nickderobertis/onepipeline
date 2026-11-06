//! The trusted approval authority a design-review task completes through.
//!
//! The adapter is a command the host configured, chosen by the phase the task
//! was raised from — never by anything the item or the person completing it
//! supplies. It is handed the task's stored `{board_copy,key}` and who is
//! acting, revalidates the version itself, and answers the version it holds:
//! the stored one when it recorded the approval, another when the document
//! changed and it recorded nothing. Any other answer leaves the task open.

use std::io::Write;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use super::config::{ApprovalAdapter, EffectiveConfig};
use super::live::Actor;
use super::phases::phase_of;
use super::{HumanItem, HumanItemApproval, HumanItemInput, HumanItemRefusal};

/// What a completion refuses with when the document changed since the task was
/// raised.
pub const REREAD: &str = "Re-read the current document";

// llmlint: ignore-block[invalid_states_unrepresentable] the adapter protocol the task fixes is `{board_copy,key,actor,on_behalf_of}` in and `{board_copy,key}` out, as JSON strings; the request copies the stored HumanItemApproval and the resolution's actor, and the answer's decoder refuses a blank version before it is compared with the stored board copy and key.
#[derive(Serialize)]
struct Request<'a> {
    board_copy: &'a str,
    key: &'a str,
    actor: &'a str,
    on_behalf_of: Option<&'a str>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    board_copy: String,
    #[serde(deserialize_with = "version")]
    key: String,
}

// llmlint: ignore-end[invalid_states_unrepresentable]

fn version<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let key = String::deserialize(d)?;
    if key.trim().is_empty() {
        return Err(serde::de::Error::custom("a blank document version"));
    }
    Ok(key)
}

fn refused(reason: impl Into<String>) -> HumanItemRefusal {
    HumanItemRefusal::Conflict {
        reason: reason.into(),
    }
}

/// The adapter the phase `item` was raised from names, if it names a configured one.
fn adapter_of<'a>(config: &'a EffectiveConfig, item: &HumanItem) -> Option<&'a ApprovalAdapter> {
    phase_of(&item.dedup_key)
        .and_then(|phase| config.hook(phase))
        .and_then(|hook| hook.approval.as_ref())
        .and_then(|approval| config.human_items.approval_adapters.get(&approval.adapter))
}

/// Run a configured adapter program over `request` on its standard input.
fn invoke(argv: (&String, &[String]), request: &[u8]) -> std::io::Result<std::process::Output> {
    let mut child = Command::new(argv.0)
        .args(argv.1)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(request);
    }
    child.wait_with_output()
}

fn request(stored: &HumanItemApproval, actor: &Actor) -> Result<Vec<u8>, HumanItemRefusal> {
    serde_json::to_vec(&Request {
        board_copy: &stored.board_copy,
        key: &stored.key,
        actor: &actor.actor,
        on_behalf_of: actor.on_behalf_of.as_deref(),
    })
    .map_err(|e| refused(e.to_string()))
}

/// Whether reopening `item` could take its approval back: its adapter declares an undo.
pub(crate) fn undoable(config: &EffectiveConfig, item: &HumanItem) -> bool {
    adapter_of(config, item).is_some_and(|adapter| adapter.undo.is_some())
}

/// Run the configured undo of a completed task's approval. Only `{board_copy,key}`
/// naming the stored version means undone; anything else refuses with what the script
/// said on standard error, and the task stays completed.
pub(crate) fn undo(
    config: &EffectiveConfig,
    item: &HumanItem,
    actor: &Actor,
) -> Result<(), HumanItemRefusal> {
    let (Some(stored), Some(argv)) = (
        &item.approval,
        adapter_of(config, item).and_then(|adapter| adapter.undo.as_ref()),
    ) else {
        return Err(refused(
            "its approval adapter declares no undo, so the recorded approval cannot be taken back",
        ));
    };
    let Some((program, arguments)) = argv.split_first() else {
        return Err(refused(
            "the approval adapter's undo names no program; the task stays completed",
        ));
    };
    let output = invoke((program, arguments), &request(stored, actor)?).map_err(|e| {
        refused(format!(
            "the undo script {program} could not be started: {e}; the task stays completed"
        ))
    })?;
    let said = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let refusal = |otherwise: String| {
        refused(if said.is_empty() {
            otherwise
        } else {
            said.clone()
        })
    };
    if !output.status.success() {
        return Err(refusal(format!(
            "the undo script refused ({}); the task stays completed",
            output.status
        )));
    }
    match serde_json::from_slice::<Answer>(&output.stdout) {
        Ok(answer) if answer.board_copy == stored.board_copy && answer.key == stored.key => Ok(()),
        _ => Err(refusal(
            "the undo script did not answer {board_copy,key} naming the stored version; the \
             task stays completed"
                .to_owned(),
        )),
    }
}

/// Run the configured authority for a task carrying an approval reference.
///
/// Answers `Ok(None)` when it recorded the approval of the stored version, and
/// `Ok(Some(replacement))` when the document moved on: the replacement is the
/// same task against the version the authority reported.
pub(crate) fn complete(
    config: &EffectiveConfig,
    item: &HumanItem,
    actor: &Actor,
) -> Result<Option<HumanItemInput>, HumanItemRefusal> {
    let Some(stored) = &item.approval else {
        return Ok(None);
    };
    let adapter = adapter_of(config, item).ok_or_else(|| {
        refused(format!(
            "{} carries a document version, and no configured phase names an approval \
             adapter for it; nothing was approved",
            item.id
        ))
    })?;
    let Some((program, arguments)) = adapter.command.split_first() else {
        return Err(refused(
            "the approval adapter names no program; the task stays open",
        ));
    };
    let output = invoke((program, arguments), &request(stored, actor)?).map_err(|e| {
        refused(format!(
            "the approval adapter {program} could not be started: {e}; the task stays open"
        ))
    })?;
    if !output.status.success() {
        return Err(refused(format!(
            "the approval authority refused ({}): {}; the task stays open",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let answer: Answer = serde_json::from_slice(&output.stdout).map_err(|e| {
        refused(format!(
            "the approval authority answered something other than {{board_copy,key}} ({e}); the \
             task stays open"
        ))
    })?;
    if answer.board_copy != stored.board_copy {
        return Err(refused(format!(
            "the approval authority answered for board copy {}, not {}; the task stays open",
            answer.board_copy, stored.board_copy
        )));
    }
    if answer.key == stored.key {
        return Ok(None);
    }
    Ok(Some(HumanItemInput {
        schema_version: 1,
        dedup_key: format!("{}:{}", item.dedup_key, answer.key),
        kind: item.kind,
        title: item.title.clone(),
        body: item.body.clone(),
        priority: item.priority,
        owner: item.owner.clone(),
        block: item.block.clone(),
        approval: Some(HumanItemApproval {
            board_copy: answer.board_copy,
            key: answer.key,
        }),
    }))
}
