//! Option (b)'s prototype suite e2e-channel: written by scripts/crate-split/prototype-b.mjs.
#![allow(dead_code, unused_imports, unused_macros)]
#[path = "../e2e/ask.rs"]
mod ask;
#[path = "../e2e/boundary.rs"]
mod boundary;
#[path = "../e2e/channel.rs"]
mod channel;
#[path = "../e2e/harness.rs"]
mod harness;
#[path = "../e2e/recorded_channel.rs"]
mod recorded_channel;
#[path = "../e2e/recorded_support.rs"]
mod recorded_support;
#[path = "../e2e/rulings.rs"]
mod rulings;
#[path = "../e2e/stop_guard.rs"]
mod stop_guard;
