//! The repository-identity launch interlock, delegated to `onevcs`.
//!
//! Reached by calling that library, like every other operation this crate
//! performs against it: [`onevcs::session_holders`] is the enumeration the
//! `session holders` verb prints, so the launcher reads the sibling's own typed
//! records rather than re-parsing the JSON a process printed for a person.

use std::collections::{BTreeMap, BTreeSet};

use crate::error::{Error, Result};
use crate::plan::Plan;

/// Whether a holder's owner is still there — the sibling's own verdict.
///
/// A pid alone cannot answer it, which is why the sibling reports it rather
/// than leaving a caller to derive one from [`Holder::owner_pid`]; re-deriving
/// it here would be a second liveness rule for the same question.
pub(crate) use onevcs::Liveness;

/// Where a holder's session is in its life. Named `State` here because that is
/// what this crate's interlock calls the distinction it makes.
pub(crate) use onevcs::Lifecycle as State;

/// One session holding a repository's workspace.
pub(crate) use onevcs::SessionHolder as Holder;

/// Ask `onevcs` about every distinct repository named by the plan.
pub fn holders(plan: &Plan) -> Result<Vec<Holder>> {
    let repos: BTreeSet<_> = plan
        .tasks
        .iter()
        .filter_map(|node| node.repo.as_deref())
        .collect();
    let mut by_identity_and_token = BTreeMap::new();
    for repo in repos {
        let found = onevcs::session_holders(repo).map_err(|error| {
            sibling(format!(
                "cannot read the session holders of {repo}: {error}"
            ))
        })?;
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
