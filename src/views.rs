//! What the read-only views report.
//!
//! The views are the CLI's `runs`, `status`, `host`, `monitor`, `results`,
//! `goals`, and `telemetry`. Their semantics are ported one-to-one from
//! `ai-orchestrator`, and the one distinction that has cost real supervision
//! time is named here as a type rather than left to prose: whether a run is
//! being driven, and if not, how it stopped being driven.
//!
//! Nothing here reads a ledger, probes a process, or renders anything.

// llmlint: ignore-block[names_match_behavior] `Parked` reads as a deliberate idle, and
// for a *node* it is one — but this is the *run* liveness verdict, and `PARKED` is the
// word `docs/contract.md` fixes for it ("DRIVER DEAD vs PARKED vs UNDRIVEN"). Renaming it
// would make this crate's views disagree with the contract and with the operators who
// already read that word off them; the collision is exactly what the doc comment below
// exists to disarm. Raise it with the planner who owns the contract, not here.

/// Whether a run is being driven, and if not, why not.
///
/// The three are deliberately distinct. A run whose *driver* died is not lost —
/// its ledger is intact and `onepipeline adopt` attaches a fresh driver to it —
/// while a parked node is a planner's own deliberate idle and nothing to
/// intervene in. Reading one as the other is what this distinction exists to
/// prevent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DriverLiveness {
    /// A driver holds the run and this host has observed it working.
    Driving,
    /// This host has proved the recorded driver process is gone. Nothing is
    /// driving the run; `adopt` is the way back.
    DriverDead,
    /// The launch still holds its recorded pid, but nothing is happening — no
    /// child process, no surface, no ledger write. Alive and not working, so
    /// treat it as stopped and intervene.
    Parked,
    /// A *node* the ledger records as started that nothing is driving.
    /// Deliberately not [`Parked`](Self::Parked), which is the state a planner's
    /// own `cancel` produces.
    Undriven,
}
// llmlint: ignore-end[names_match_behavior]
