//! End-to-end journeys against the compiled binary.
//!
//! Every test here spawns the real `onepipeline` executable as a subprocess and
//! asserts on its exit code, stdout, and stderr — the way a user reaches it.
//! `oneagentgraph` is a real executable too, scripted per test; `onevcs` is not
//! substituted at all, because this crate calls that library rather than spawning
//! it, so every lifecycle journey drives real git against a real origin on disk.
//!
//! The journeys are ported from `ai-orchestrator`'s own e2e suite, adapted to
//! the command vocabulary `docs/contract.md` fixes.

// llmlint: ignore-file[e2e_not_mocked] one double substitutes one *sibling* —
// `oneagentgraph` — at its subprocess boundary, never anything inside the crate under
// test, and `dispatch.rs` drives the real binary with only the paid model turn standing
// in. What it buys the journeys in between is a dispatch outcome stated directly, where
// the real agent would need a paid turn. The repository side is real everywhere: the
// lifecycle journeys register a git origin, open sessions, and publish through the linked
// `onevcs` — past whatever the repository's own merge path makes of the push. The one thing past it that is substituted is GitHub, at that
// library's own `ONEVCS_GH` override. The same rationale, at more length, is in
// `harness.rs`.

mod harness;

mod adoption;
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] two journeys, a few
// minutes together, and what they exercise is the environment every dispatch the
// engine starts is composed with — `executor`, `driver`, `lifecycle` and `agents`
// together, through the real `oneagentgraph` and the real linked `oneharness-core` —
// so the narrowest edge they can honestly sit behind is the crate itself, which is
// this target's. Same grounds as `mod dispatch` below; the block form because a
// line-scoped directive reaches only the line right below it, and this reason is
// longer than one line.
mod agents;
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
mod amend;
mod boundary;
mod bus_config;
mod cancellation;
mod channel;
mod compatibility;
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] four journeys, about
// 40s together, and what they exercise is the fold every view and the reconcile loop read
// through — `checkpoint`, `views` and `engine` — so a project edged narrower than the crate
// could not honestly run them, and any change under `src/` can put the cost back. Same
// grounds as `mod landing` and `mod watch` below.
mod checkpoint;
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
mod concurrency;
mod criteria;
mod crossdag;
// llmlint: ignore[expensive_tests_stay_behind_their_own_edge] these journeys drive the compiled
// binary over the real store and exercise `writeback`, `engine`, `graph`, `taskgraph` and
// `driver` together — the claim before a first dispatch, the release at closeout and at a stop,
// and the delivered-ticket report — so the narrowest edge they can honestly sit behind is the
// crate itself, which is this target's. A project edged narrower would drop them out of `nx
// affected` for the very changes they exist to catch. The unrelated dependency the target carries
// is the shared target's, and `store.rs`, `writeback_projections.rs` and `writeback_budget.rs`
// already run under it for the same reason; same grounds as `mod checkpoint` above.
mod delivers;
mod destination;
mod dispatch;
// llmlint: ignore[expensive_tests_stay_behind_their_own_edge] about 55 seconds, and what
// the journeys exercise is the executor's every dispatch — `executor`, `dispatchenv`,
// `driver`, `filter` and `ledger` together, one through the real `oneagentgraph` and one
// through the linked `onevcs` — so the narrowest edge they can honestly sit behind is the
// crate itself, which is this target's; a project edged narrower would drop them out of
// `nx affected` for the very changes they exist to catch. Same grounds as `mod dispatch`
// above and `mod run_end_hooks` below.
mod dispatch_env_hook;
mod driver;
mod envelope_reviewer;
mod filter;
mod holds;
mod journal;
// llmlint: ignore[expensive_tests_stay_behind_their_own_edge] the reason is at the head of
// `tests/e2e/landing.rs`, and is carried here too because this declaration is the other
// site the rule reads: the module is measured at about 45 seconds on an idle host, and
// about 110 under a loaded one, against a binary that already holds three deliberately
// minute-long journeys — and what it exercises is `views`, `vcs` and `rendercost`, so a
// project edged narrower than the crate would drop it out of `nx affected` for the very
// changes it exists to catch.
mod landing;
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] what these journeys
// exercise is `lifecycle`, `vcs`, `pool` and `engine` together against the linked `onevcs`
// over a real origin — the session, the publication, the pool's hold and its refusal — so
// the narrowest edge they can honestly sit behind is the crate itself, which is this
// target's; a project edged narrower would drop them out of `nx affected` for the very
// changes they exist to catch. Same grounds as `mod dispatch_env_hook` above. A block
// rather than a line, because the reason runs past the one line a line-scoped directive
// covers and the declaration it is for is below it.
mod lifecycle;
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] the reason is at the
// head of `tests/e2e/listing.rs` and is not restated here; this declaration is the other
// site the rule reads, and what it adds is only that the module belongs to this binary for
// the same reason `mod landing` and `mod checkpoint` above do.
mod listing;
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
mod live_edit;
mod loopcost;
// llmlint: ignore[expensive_tests_stay_behind_their_own_edge] about 35 seconds, and what
// the journeys exercise is the reconcile loop's idle branch, the launch's flag-and-key
// resolution, the launch record and the views together — `engine`, `maintenance`,
// `driver`, `filter`, `ledger` and `views`, over the linked `onevcs` — so the narrowest
// edge they can honestly sit behind is the crate itself, which is this target's; a
// project edged narrower would drop them out of `nx affected` for the very changes they
// exist to catch. Same grounds as `mod dispatch_env_hook` above.
mod maintenance;
mod node_validator;
mod plan;
mod plan_check;
mod real_vcs;
mod recorded_channel;
mod run_end_hooks;
mod scratch;
mod session;
mod session_reuse;
mod shipped;
mod store;
mod summary;
mod surface;
mod turns;
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] the reason is at the
// head of `tests/e2e/unwatched.rs` and is not restated here; this declaration is the other
// site the rule reads, and what it adds is only that the module belongs to this binary for
// the same reason `mod listing` above does — what it exercises is `watchers`, `unwatched`,
// `summary` and `views`, which any change under `src/` can move.
mod unwatched;
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
mod views;
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] twelve journeys,
// 15.1s together, driving the binary they test — cheaper than single journeys already in
// this file that nextest marks SLOW past 120s. The `onepipeline-note-journeys` edge the
// rule points at belongs to this project already and is unaffected by where this module
// sits; that project is edged on conversational cost, which this module has none of.
mod watch;
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] three of its journeys
// wait past the write-back's sixty-second floor by construction, for the reason at the
// head of `tests/e2e/writeback_budget.rs`; this declaration is the other site the rule
// reads, and the module belongs to this binary for the reason the minute-long schedule
// journeys in `mod store` above do.
mod writeback_budget;
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
// llmlint: ignore[expensive_tests_stay_behind_their_own_edge] four journeys, measured at under
// ten seconds together on this host, driving the compiled binary against its own write-back
// worker and the real store exactly as `mod store` above does. What they exercise is
// `writeback`, `engine` and `taskgraph`, so a project edged narrower than the crate would drop
// them out of `nx affected` for the very changes they exist to catch.
mod writeback_projections;
