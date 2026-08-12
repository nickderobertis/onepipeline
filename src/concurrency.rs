//! The repository-identity launch interlock, delegated to `onevcs`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;
use std::process::Command;

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::plan::Plan;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Liveness {
    Live,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum State {
    Open,
    Closed,
}

/// One record returned by `onevcs session holders --json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Holder {
    pub(crate) token: SessionToken,
    pub(crate) identity: RepositoryIdentity,
    pub(crate) branch: Branch,
    pub(crate) worktree: PathBuf,
    pub(crate) owner_pid: u32,
    pub(crate) state: State,
    pub(crate) liveness: Liveness,
}

macro_rules! boundary_string {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
        #[serde(transparent)]
        pub(crate) struct $name(String);

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

boundary_string!(SessionToken);
boundary_string!(RepositoryIdentity);
boundary_string!(Branch);

/// Ask `onevcs` about every distinct repository named by the plan.
pub fn holders(plan: &Plan) -> Result<Vec<Holder>> {
    let repos: BTreeSet<_> = plan
        .tasks
        .iter()
        .filter_map(|node| node.repo.as_deref())
        .collect();
    let mut by_identity_and_token = BTreeMap::new();
    for repo in repos {
        let output = Command::new("onevcs")
            .args(["session", "holders", repo, "--json"])
            .output()
            // llmlint: ignore-block[changed_behavior_has_e2e] no real-interface journey
            // can make the pinned executable fail to start or make its typed `--json`
            // output malformed. Replacing `onevcs` is exactly mocking the layer this
            // interlock composes. `tests/e2e/lifecycle.rs` proves a real holders command
            // refusal reaches the user, and `tests/e2e/concurrency.rs` proves valid holder
            // output drives every live, acknowledged, and stale decision.
            .map_err(|error| {
                sibling(format!(
                    "cannot start `onevcs session holders {repo} --json`: {error}"
                ))
            })?; // llmlint: ignore-end[changed_behavior_has_e2e]
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
        // llmlint: ignore-block[changed_behavior_has_e2e] the pinned sibling serializes
        // this same typed contract, so malformed output requires substituting that sibling.
        // The real-boundary journeys named above cover its failure and valid-output paths;
        // this remains a defensive refusal for incompatible executables found on PATH.
        let found: Vec<Holder> = serde_json::from_slice(&output.stdout).map_err(|error| {
            sibling(format!(
                "invalid JSON from `session holders {repo} --json`: {error}"
            ))
        })?;
        // llmlint: ignore-end[changed_behavior_has_e2e]
        for holder in found {
            by_identity_and_token.insert((holder.identity.clone(), holder.token.clone()), holder);
        }
    }
    Ok(by_identity_and_token.into_values().collect())
}

fn sibling(message: String) -> Error {
    Error::Sibling {
        tool: "onevcs",
        message,
    }
}
