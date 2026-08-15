//! The failure modes the contract names, and the process exit codes it assigns.

// llmlint: ignore-file[invalid_states_unrepresentable] the run and node a failure names
// are `String`s because `RunId`/`NodeId` newtypes are public items `docs/contract.md`
// does not name. The engine validates a node reference against the live graph before it
// reaches a failure, so a newtype here would restate that check rather than add one.

use std::path::PathBuf;

/// What can go wrong driving a run.
///
/// The variants are the failures `docs/contract.md` distinguishes, and they
/// carry the exit codes it assigns: [`EXIT_QUEUED`], [`EXIT_REFUSED`], and
/// [`EXIT_NOTHING_DRIVING`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A reply was malformed, or an edit in it was refused at submission, or the
    /// reconciler rejected it. Exits [`EXIT_REFUSED`].
    #[error("refused: {0}")]
    Refused(String),
    /// A reply's edits are accepted and durable but were not reconciled within
    /// the timeout; they remain queued. Exits [`EXIT_QUEUED`].
    #[error("queued: {0}")]
    Queued(String),
    /// Nothing is driving the run: no orchestrator process, no surface, no
    /// ledger write. Exits [`EXIT_NOTHING_DRIVING`].
    #[error("nothing is driving run '{run}'")]
    NothingDriving {
        /// The run id.
        run: String,
    },
    /// External input the schema does not accept — a plan file, a rules file, a
    /// reply envelope, or a command line naming something that does not exist.
    /// Exits [`EXIT_REFUSED`].
    #[error("invalid: {0}")]
    Invalid(String),
    /// The run does not exist under the runs root. Exits [`EXIT_REFUSED`].
    #[error("no such run '{run}' under {root}")]
    NoSuchRun {
        /// The run id as it was named.
        run: String,
        /// The runs root that was searched.
        root: PathBuf,
    },
    /// The command needs the run's ownership, and this session does not have it.
    /// Exits [`EXIT_REFUSED`].
    #[error("run '{run}' belongs to {owner}, not to this session")]
    NotOwned {
        /// The run id.
        run: String,
        /// How the owner is named to a caller that is not it.
        owner: String,
    },
    /// Another writer holds the run's single-writer lock.
    /// Exits [`EXIT_REFUSED`].
    #[error("run '{run}' is being written by pid {pid} on {host} ({verb})")]
    Locked {
        /// The run id.
        run: String,
        /// The holding process.
        pid: u32,
        /// The host that pid is meaningful on.
        host: String,
        /// What the holder is doing.
        verb: String,
    },
    /// A sibling refused: `onevcs` answering a library call with an error, or
    /// `oneagentgraph`'s CLI refusing or failing to run.
    /// Exits [`EXIT_REFUSED`].
    #[error("{tool}: {message}")]
    Sibling {
        /// Which sibling — `oneagentgraph` or `onevcs`.
        tool: &'static str,
        /// What it said.
        message: String,
    },
    /// The ledger could not be read or written. Exits [`EXIT_REFUSED`].
    #[error("{path}: {source}")]
    Ledger {
        /// The file involved.
        path: PathBuf,
        /// What the filesystem said.
        #[source]
        source: std::io::Error,
    },
}

impl Error {
    /// The process exit code this failure carries.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Queued(_) => EXIT_QUEUED,
            Self::NothingDriving { .. } => EXIT_NOTHING_DRIVING,
            _ => EXIT_REFUSED,
        }
    }
}

/// The result of anything in this crate that can fail.
pub type Result<T> = std::result::Result<T, Error>;

/// The run settled, or the reply's every edit was applied.
pub const EXIT_SUCCESS: i32 = 0;

/// The reply's edits are accepted and durable but not yet reconciled.
///
/// It is also a run's own "unfinished": a graph that is waiting or has a
/// failed node has not settled, and the two readings agree — neither is an
/// error, and neither is completion.
pub const EXIT_QUEUED: i32 = 1;

/// The reply was malformed, or an edit was refused.
pub const EXIT_REFUSED: i32 = 2;

/// Nothing is driving the run — the state to intervene in.
pub const EXIT_NOTHING_DRIVING: i32 = 3;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_failure_carries_the_code_the_contract_assigns() {
        assert_eq!(Error::Queued("edits".into()).exit_code(), EXIT_QUEUED);
        assert_eq!(Error::Refused("bad op".into()).exit_code(), EXIT_REFUSED);
        assert_eq!(Error::Invalid("bad plan".into()).exit_code(), EXIT_REFUSED);
        assert_eq!(
            Error::NothingDriving { run: "r".into() }.exit_code(),
            EXIT_NOTHING_DRIVING
        );
        assert_eq!(
            Error::Sibling {
                tool: "onevcs",
                message: "refused".into()
            }
            .exit_code(),
            EXIT_REFUSED
        );
    }

    #[test]
    fn a_refusal_says_which_run_and_who_owns_it() {
        let error = Error::NotOwned {
            run: "demo".into(),
            owner: "[claude-code:3f9a1c2e]".into(),
        };
        let rendered = error.to_string();
        assert!(rendered.contains("demo"), "{rendered}");
        assert!(rendered.contains("claude-code"), "{rendered}");
    }

    #[test]
    fn a_locked_run_names_the_writer_that_holds_it() {
        let rendered = Error::Locked {
            run: "demo".into(),
            pid: 4321,
            host: "builder".into(),
            verb: "drive".into(),
        }
        .to_string();
        assert!(rendered.contains("4321"), "{rendered}");
        assert!(rendered.contains("builder"), "{rendered}");
        assert!(rendered.contains("drive"), "{rendered}");
    }
}
