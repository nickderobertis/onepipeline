//! Option (b)'s prototype suite e2e-harness: written by scripts/crate-split/prototype-b.mjs.
#![allow(dead_code, unused_imports, unused_macros)]
#[path = "../e2e/gh_session_bus.rs"]
mod gh_session_bus;
#[path = "../e2e/harness.rs"]
mod harness;
#[path = "../e2e/profiling.rs"]
mod profiling;
