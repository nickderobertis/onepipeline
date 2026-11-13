//! Option (b)'s prototype suite e2e-watch: written by scripts/crate-split/prototype-b.mjs.
#![allow(dead_code, unused_imports, unused_macros)]
#[path = "../e2e/budget_result.rs"]
mod budget_result;
#[path = "../e2e/dispatch.rs"]
mod dispatch;
#[path = "../e2e/harness.rs"]
mod harness;
#[path = "../e2e/loopcost.rs"]
mod loopcost;
#[path = "../e2e/stop_guard.rs"]
mod stop_guard;
#[path = "../e2e/unpublished.rs"]
mod unpublished;
#[path = "../e2e/unwatched.rs"]
mod unwatched;
#[path = "../e2e/wake_budget.rs"]
mod wake_budget;
#[path = "../e2e/watch.rs"]
mod watch;
