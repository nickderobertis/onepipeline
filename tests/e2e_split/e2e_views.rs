//! Option (b)'s prototype suite e2e-views: written by scripts/crate-split/prototype-b.mjs.
#![allow(dead_code, unused_imports, unused_macros)]
#[path = "../e2e/change_telemetry.rs"]
mod change_telemetry;
#[path = "../e2e/checkpoint.rs"]
mod checkpoint;
#[path = "../e2e/compatibility.rs"]
mod compatibility;
#[path = "../e2e/draft_lifecycle.rs"]
mod draft_lifecycle;
#[path = "../e2e/harness.rs"]
mod harness;
#[path = "../e2e/host_view.rs"]
mod host_view;
#[path = "../e2e/journal.rs"]
mod journal;
#[path = "../e2e/layout_document.rs"]
mod layout_document;
#[path = "../e2e/listing.rs"]
mod listing;
#[path = "../e2e/run_end_hooks.rs"]
mod run_end_hooks;
#[path = "../e2e/status_read.rs"]
mod status_read;
#[path = "../e2e/summary.rs"]
mod summary;
#[path = "../e2e/surface.rs"]
mod surface;
#[path = "../e2e/transcript_refs.rs"]
mod transcript_refs;
#[path = "../e2e/views.rs"]
mod views;
