//! The repository-identity launch interlock, delegated to `onevcs`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::plan::Plan;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Liveness {
    Live,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Open,
    Closed,
}

/// One record returned by `onevcs session holders --json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Holder {
    pub token: String,
    pub identity: String,
    pub branch: String,
    pub worktree: PathBuf,
    pub owner_pid: u32,
    pub state: State,
    pub liveness: Liveness,
}

/// Ask `onevcs` about every distinct repository named by the plan.
pub fn holders(plan: &Plan) -> Result<Vec<Holder>> {
    let mut by_identity = BTreeMap::new();
    for repo in plan.tasks.iter().filter_map(|node| node.repo.as_deref()) {
        let output = Command::new("onevcs")
            .args(["session", "holders", repo, "--json"])
            .output()
            .map_err(|error| {
                sibling(format!(
                    "cannot start `onevcs session holders {repo} --json`: {error}"
                ))
            })?;
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(sibling(format!(
                "`session holders {repo} --json` exited {}: {detail}",
                output
                    .status
                    .code()
                    .map_or_else(|| "from a signal".into(), |code| code.to_string())
            )));
        }
        let found: Vec<Holder> = serde_json::from_slice(&output.stdout).map_err(|error| {
            sibling(format!(
                "invalid JSON from `session holders {repo} --json`: {error}"
            ))
        })?;
        for holder in found {
            by_identity.insert((holder.identity.clone(), holder.token.clone()), holder);
        }
    }
    Ok(by_identity.into_values().collect())
}

fn sibling(message: String) -> Error {
    Error::Sibling {
        tool: "onevcs",
        message,
    }
}
