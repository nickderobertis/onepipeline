//! Option (b)'s prototype suite e2e-dispatch: written by scripts/crate-split/prototype-b.mjs.
#![allow(dead_code, unused_imports, unused_macros)]
#[path = "../e2e/agents.rs"]
mod agents;
#[path = "../e2e/bus_config.rs"]
mod bus_config;
#[path = "../e2e/dispatch.rs"]
mod dispatch;
#[path = "../e2e/dispatch_env_hook.rs"]
mod dispatch_env_hook;
#[path = "../e2e/draft_lifecycle.rs"]
mod draft_lifecycle;
#[path = "../e2e/driver.rs"]
mod driver;
#[path = "../e2e/envelope_reviewer.rs"]
mod envelope_reviewer;
#[path = "../e2e/harness.rs"]
mod harness;
#[path = "../e2e/malformed_envelopes.rs"]
mod malformed_envelopes;
#[path = "../e2e/node_validator.rs"]
mod node_validator;
#[path = "../e2e/review.rs"]
mod review;
#[path = "../e2e/review_reply.rs"]
mod review_reply;
#[path = "../e2e/run_end_hooks.rs"]
mod run_end_hooks;
#[path = "../e2e/stacked_drafts.rs"]
mod stacked_drafts;
#[path = "../e2e/turns.rs"]
mod turns;
