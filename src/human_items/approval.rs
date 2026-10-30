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

use super::config::EffectiveConfig;
use super::live::Actor;
use super::phases::phase_of;
use super::{HumanItem, HumanItemApproval, HumanItemInput, HumanItemRefusal};

/// What a completion refuses with when the document changed since the task was
/// raised.
pub const REREAD: &str = "Re-read the current document";

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
    let adapter = phase_of(&item.dedup_key)
        .and_then(|phase| config.hook(phase))
        .and_then(|hook| hook.approval.as_ref())
        .and_then(|approval| config.human_items.approval_adapters.get(&approval.adapter))
        .ok_or_else(|| {
            refused(format!(
                "{} carries a document version, and no configured phase names an approval \
                 adapter for it; nothing was approved",
                item.id
            ))
        })?;
    let request = serde_json::to_vec(&Request {
        board_copy: &stored.board_copy,
        key: &stored.key,
        actor: &actor.actor,
        on_behalf_of: actor.on_behalf_of.as_deref(),
    })
    .map_err(|e| refused(e.to_string()))?;
    let mut command = Command::new(&adapter.command[0]);
    command
        .args(&adapter.command[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|e| {
        refused(format!(
            "the approval adapter {} could not be started: {e}; the task stays open",
            adapter.command[0]
        ))
    })?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(&request);
    }
    let output = child
        .wait_with_output()
        .map_err(|e| refused(format!("the approval adapter did not finish: {e}")))?;
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
