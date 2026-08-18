//! `onepipeline` owns a task DAG, executes it over
//! [`oneagentgraph`](https://github.com/nickderobertis/oneagentgraph) and
//! [`onevcs`](https://github.com/nickderobertis/onevcs), and merges the three
//! libraries' event streams into one.
//!
//! Dependency direction is one-way: `onepipeline` → `{oneagentgraph, onevcs}`.
//! Agent, harness, and model selection stay in the first; repository identities,
//! sessions, and publication stay in the second. This crate composes them and
//! owns the DAG, its continuous execution, the planner channel, and the
//! [executor seam](executor) that decides *where* a dispatch runs.
//!
//! # The surface, and what is behind it
//!
//! Every public item here is named by
//! [`docs/contract.md`](../../../docs/contract.md) — the approved contract,
//! committed verbatim — and the engine implementing it is private, so a
//! consumer can only reach what the contract promised.
//! `tests/contract.rs` drives the fixtures out of that document through these
//! types, so the two cannot drift.
//!
//! A run's durable state is one directory: the plan it was launched with, the
//! merged event store every view reads, the run's own result, and the channel's
//! transport. The process driving the run is that ledger's **single writer**,
//! guarded by the run's ownership lock; everything else reads.
//!
//! Composition is by subprocess. The agents come from `oneagentgraph` and the
//! clones, gates, and change requests from `onevcs`, each reached through its
//! own CLI — so a build of either that still refuses will make the dispatches
//! this crate starts refuse too.
//!
//! Where the contract could not be compiled exactly as written, the code
//! compiles against what does exist and the divergence is recorded in
//! [`docs/contract-divergences.md`](../../../docs/contract-divergences.md) for
//! the planner who owns the contract. Every entry there has now been ruled on
//! and the contract amended to carry the ruling.

#![warn(missing_docs)]

pub mod channel;
pub mod cli;
pub mod controls;
pub mod error;
pub mod event;
pub mod executor;
pub mod filter;
pub mod plan;
pub mod report;
pub mod rules;
pub mod views;

// The engine behind the contract's surface. These modules are private on
// purpose: `docs/contract.md` names the plan schema, the channel, the executor
// seam, the rules grammar, the views, and the report retention path, and a
// public item it does not name is a promise this crate did not make. The binary
// reaches them through [`run`](crate::run).
mod agentgraph;
mod concurrency;
mod crossdag;
mod driver;
mod edits;
mod engine;
mod graph;
mod journal;
mod ledger;
mod lifecycle;
mod projection;
mod sys;
mod telemetry;
mod vcs;

pub use error::{Error, Result};

/// The release of this crate a consumer is linking.
///
/// A host that pins this engine and separately pins a reader of the run store it
/// writes has nothing else to hold the two to one another: the retention path
/// and the resolution path are the same promise only where both sides are the
/// same release, and this is how each side says which one it is.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Execute one parsed command line.
///
/// The binary is this function plus argument parsing and an exit code, so every
/// journey a user can reach is reachable from a test that drives the same
/// entry point the binary does.
pub fn run(cli: cli::Cli) -> Result<i32> {
    driver::dispatch(cli)
}
