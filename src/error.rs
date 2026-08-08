//! The failure modes the contract names, and the process exit codes it assigns.

// llmlint: ignore-file[invalid_states_unrepresentable] the run and node a failure names
// are `String`s because `RunId`/`NodeId` newtypes are public items `docs/contract.md`
// does not name, and minting one is the interface drift the interface-only stage forbids
// (see AGENTS.md). Revisit when the engine owns a run registry to validate against.

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
    /// The surface this build presents is the contract's, and none of it is
    /// implemented yet. Exits [`EXIT_NOT_IMPLEMENTED`].
    #[error("NOT IMPLEMENTED: {0}")]
    NotImplemented(&'static str),
}

/// The result of anything in this crate that can fail.
pub type Result<T> = std::result::Result<T, Error>;

/// The run settled, or the reply's every edit was applied.
pub const EXIT_SUCCESS: i32 = 0;

/// The reply's edits are accepted and durable but not yet reconciled.
pub const EXIT_QUEUED: i32 = 1;

/// The reply was malformed, or an edit was refused.
pub const EXIT_REFUSED: i32 = 2;

/// Nothing is driving the run — the state to intervene in.
pub const EXIT_NOTHING_DRIVING: i32 = 3;

/// The interface-only refusal.
///
/// `EX_SOFTWARE`, kept clear of every code the contract already spends: `0`,
/// `1`, and `2` are `reply`'s applied / queued / refused verdicts and `3` is
/// "nothing is driving the run". It goes away with the implementation.
pub const EXIT_NOT_IMPLEMENTED: i32 = 70;
