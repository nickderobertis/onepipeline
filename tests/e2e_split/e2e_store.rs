//! Option (b)'s prototype suite e2e-store: written by scripts/crate-split/prototype-b.mjs.
#![allow(dead_code, unused_imports, unused_macros)]
#[path = "../e2e/budget_result.rs"]
mod budget_result;
#[path = "../e2e/command_step.rs"]
mod command_step;
#[path = "../e2e/configured_host.rs"]
mod configured_host;
#[path = "../e2e/continuation.rs"]
mod continuation;
#[path = "../e2e/criteria.rs"]
mod criteria;
#[path = "../e2e/criteria_rule.rs"]
mod criteria_rule;
#[path = "../e2e/crossdag.rs"]
mod crossdag;
#[path = "../e2e/delivers.rs"]
mod delivers;
#[path = "../e2e/dispatch.rs"]
mod dispatch;
#[path = "../e2e/draft_lifecycle.rs"]
mod draft_lifecycle;
#[path = "../e2e/driver.rs"]
mod driver;
#[path = "../e2e/filter.rs"]
mod filter;
#[path = "../e2e/github_create_retry.rs"]
mod github_create_retry;
#[path = "../e2e/github_world_support.rs"]
mod github_world_support;
#[path = "../e2e/harness.rs"]
mod harness;
#[path = "../e2e/holds.rs"]
mod holds;
#[path = "../e2e/human_board_projection.rs"]
mod human_board_projection;
#[path = "../e2e/human_items.rs"]
mod human_items;
#[path = "../e2e/linear_launch.rs"]
mod linear_launch;
#[path = "../e2e/linear_loopback.rs"]
mod linear_loopback;
#[path = "../e2e/linear_writeback.rs"]
mod linear_writeback;
#[path = "../e2e/live_edit.rs"]
mod live_edit;
#[path = "../e2e/malformed_envelopes.rs"]
mod malformed_envelopes;
#[path = "../e2e/mixed_world_support.rs"]
mod mixed_world_support;
#[path = "../e2e/multi_source.rs"]
mod multi_source;
#[path = "../e2e/plan.rs"]
mod plan;
#[path = "../e2e/plan_check.rs"]
mod plan_check;
#[path = "../e2e/plan_edit_hook.rs"]
mod plan_edit_hook;
#[path = "../e2e/private_github_home.rs"]
mod private_github_home;
#[path = "../e2e/publication_boundary.rs"]
mod publication_boundary;
#[path = "../e2e/review.rs"]
mod review;
#[path = "../e2e/review_reply.rs"]
mod review_reply;
#[path = "../e2e/run_end_hooks.rs"]
mod run_end_hooks;
#[path = "../e2e/shipped.rs"]
mod shipped;
#[path = "../e2e/stacked_drafts.rs"]
mod stacked_drafts;
#[path = "../e2e/store.rs"]
mod store;
#[path = "../e2e/templates.rs"]
mod templates;
#[path = "../e2e/withdrawn.rs"]
mod withdrawn;
#[path = "../e2e/writeback_budget.rs"]
mod writeback_budget;
#[path = "../e2e/writeback_cost.rs"]
mod writeback_cost;
#[path = "../e2e/writeback_projections.rs"]
mod writeback_projections;
