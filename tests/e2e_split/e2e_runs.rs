//! Option (b)'s prototype suite e2e-runs: written by scripts/crate-split/prototype-b.mjs.
#![allow(dead_code, unused_imports, unused_macros)]
#[path = "../e2e/cancellation.rs"]
mod cancellation;
#[path = "../e2e/concurrency.rs"]
mod concurrency;
#[path = "../e2e/dispatch.rs"]
mod dispatch;
#[path = "../e2e/driver.rs"]
mod driver;
#[path = "../e2e/driver_exit.rs"]
mod driver_exit;
#[path = "../e2e/flow.rs"]
mod flow;
#[path = "../e2e/harness.rs"]
mod harness;
#[path = "../e2e/journal_scaling.rs"]
mod journal_scaling;
#[path = "../e2e/malformed_envelopes.rs"]
mod malformed_envelopes;
#[path = "../e2e/observer_follow.rs"]
mod observer_follow;
#[path = "../e2e/older_launch_record.rs"]
mod older_launch_record;
#[path = "../e2e/out_of_band.rs"]
mod out_of_band;
#[path = "../e2e/recorded_support.rs"]
mod recorded_support;
#[path = "../e2e/run_end_hooks.rs"]
mod run_end_hooks;
#[path = "../e2e/run_ending.rs"]
mod run_ending;
#[path = "../e2e/shutdown.rs"]
mod shutdown;
#[path = "../e2e/unreadable_sessions.rs"]
mod unreadable_sessions;
