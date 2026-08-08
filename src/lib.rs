//! `onepipeline` owns a task DAG, executes it over
//! [`oneagentgraph`](https://github.com/nickderobertis/oneagentgraph) and
//! [`onevcs`](https://github.com/nickderobertis/onevcs), and merges the three
//! libraries' event streams into one.
//!
//! Dependency direction is one-way: `onepipeline` → `{oneagentgraph, onevcs}`.
//! Agent, harness, and model selection stay in the first; repository identities,
//! sessions, and publication stay in the second. This crate composes them and
//! owns the DAG, the rounds, the planner channel, and the
//! [executor seam](executor) that decides *where* a dispatch runs.
//!
//! # Interface-only
//!
//! This crate is at the **interface-only** stage of its build-out. Every item
//! below is the surface named by [`docs/contract.md`](../../../docs/contract.md)
//! — the approved contract, committed verbatim — and **none of it is
//! implemented**. There are no method bodies beyond derives, trivial field
//! constructors, and serde defaults, and the binary's subcommands parse per the
//! contract and then refuse with a `NOT IMPLEMENTED` error and exit code
//! [`error::EXIT_NOT_IMPLEMENTED`].
//!
//! Consequently these types are useful for one thing today: reading and writing
//! the contract's wire shapes. `tests/contract.rs` drives the fixtures out of
//! `docs/contract.md` through them, so the document and this surface cannot
//! drift.
//!
//! Where the contract names a sibling type that sibling does not export, the
//! code compiles against the type that does exist and the divergence is recorded
//! in [`docs/contract-divergences.md`](../../../docs/contract-divergences.md).

#![warn(missing_docs)]

pub mod channel;
pub mod cli;
pub mod error;
pub mod event;
pub mod executor;
pub mod plan;
pub mod rules;
pub mod views;

pub use error::{Error, Result};
