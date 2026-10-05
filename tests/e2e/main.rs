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
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] `ask` is a verb of this
// binary over `src/ask.rs`, `src/channel.rs` and `src/driver.rs`, each of which any change
// under `src/` can move, so the narrowest edge it can honestly sit behind is the crate's.
// Its waits are the behaviour under test — a reply arriving after a shorter window would
// have elapsed — and are a few seconds in all.
mod ask;
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
mod boundary;
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] the reason is at the
// head of `tests/e2e/branch_template.rs` and is not restated here; this declaration is the
// other site the rule reads, and what it adds is only that the module belongs to this
// binary for the same reason `mod listing` and `mod shutdown` below do.
mod branch_template;
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
mod bus_config;
mod cancellation;
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] nine journeys, about
// 25s together, whose waits are the measurement under test — a scripted turn and a
// `pre-push` hook of known lengths, against which the view's segments are checked. What
// they exercise is the fold over a whole run's store — `changes`, `projection`, `engine`,
// `lifecycle` and the linked `onevcs` together — so the narrowest edge they can honestly
// sit behind is the crate itself, which is this target's; same grounds as `mod checkpoint`.
mod change_telemetry;
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
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
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] C6b is checked in `plan`,
// `graph`, `edits` and `driver` at once, so the crate is the narrowest edge these journeys can
// honestly sit behind; same grounds as `mod delivers` below.
mod criteria_rule;
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
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
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] what these journeys
// exercise is `lifecycle`, `vcs`, `land` and `telemetry` together against the linked
// `onevcs`'s draft lifecycle over a real origin, so the narrowest edge they can honestly
// sit behind is the crate itself, which is this target's — the same grounds as `mod
// lifecycle` below, whose journeys these extend.
mod draft_lifecycle;
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
mod driver;
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] what these journeys
// exercise is how a driver lets go of a run — `engine`, `driver`, `channel`, `projection`,
// `views` and `hooks` together, through the compiled binary — so the narrowest edge they can
// honestly sit behind is the crate itself, which is this target's; a project edged narrower
// would drop them out of `nx affected` for the very changes they exist to catch. The unrelated
// dependency the target carries is the shared target's, and `mod driver` and `mod
// run_end_hooks`, whose ground these extend, already run under it.
mod driver_exit;
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
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
// llmlint: ignore[expensive_tests_stay_behind_their_own_edge] two journeys, about ten
// seconds together — one detached run and a handful of `onemessagebus` invocations, the
// shape and cost of `channel` and `boundary` in this same target — and what they hold is
// the document this crate publishes against the binary it builds, so the narrowest edge
// they can honestly sit behind is the crate itself, which is this target's.
mod layout_document;
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
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] the reason is at the
// head of the leaving-driver journey in `tests/e2e/malformed_envelopes.rs` and is not
// restated here; this declaration is the other site the rule reads, and what it adds is
// only that the module belongs to this binary for the same reason `mod listing` does: its
// journeys are of the crate's own command queue and handover gate, which any change under
// `src/` can move.
mod malformed_envelopes;
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
// llmlint: ignore[expensive_tests_stay_behind_their_own_edge] these journeys read a plan out of two sources, run it and write each settlement back where its task lives, exercising `taskgraph`, `writeback`, `edits` and `driver` together, so the crate is the narrowest edge they can honestly sit behind; same grounds as `mod delivers` above.
mod multi_source;
mod node_validator;
mod older_launch_record;
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] what these journeys
// exercise is `land`, `lifecycle`'s drafter and `destination`'s resolution together, against
// the linked `onevcs` over a real origin — so the narrowest edge they can honestly sit
// behind is the crate itself, which is this target's. Same grounds as `mod lifecycle` above.
mod out_of_band;
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
mod plan;
mod plan_check;
mod real_vcs;
mod recorded_channel;
mod recorded_support;
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] what these journeys
// exercise is `supersession`, `maintenance`'s idle sweep, `engine`'s landing paths and the
// `supersessions` verb in `driver` together, against the linked `onevcs` over real git and a
// bare origin — so the narrowest edge they can honestly sit behind is the crate itself,
// which is this target's; a project edged narrower would drop them out of `nx affected` for
// the very changes they exist to catch. Same grounds as `mod maintenance` above.
mod retirement;
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
mod run_end_hooks;
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] what these journeys
// exercise is one reading across `views`, `hooks`, `summary`, `unwatched` and the `status`
// verb in `driver` together, through the compiled binary and the run-end hook fixture
// `run_end_hooks` places — so the narrowest edge they can honestly sit behind is the crate
// itself, which is this target's, and the note-journey dependency is this binary's own,
// shared by every module in it. Same grounds as `mod retirement` above.
mod run_ending;
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
mod scratch;
mod session;
mod session_reuse;
mod shipped;
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] the reason is at the head of
// `tests/e2e/shutdown.rs`, and is carried here too because this declaration is the other
// site the rule reads: twenty-seven journeys, 90 to 195 seconds summed and 25 to 50 on the wall,
// each waiting out a real grace against real dispatches and a real origin, over `driver`'s
// teardown, `engine`'s interrupt, `views` and the linked `onevcs` together — so the
// narrowest edge they can honestly sit behind is the crate itself, which is this target's.
// The block form because a line-scoped directive reaches only the line right below it.
mod shutdown;
// And `stop-guard`, under the same block: it is a verb of
// this binary over `src/stopguard.rs` and `src/unwatched.rs`, and it answers off the run
// store every launch writes, so any change under `src/` can move what it decides: the crate
// is the narrowest edge it can honestly sit behind, as for `mod unwatched` beside it.
mod stop_guard;
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
mod store;
mod summary;
mod surface;
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] C4, C6a and C7 are
// checked in `templates`, `taskgraph`, `plancheck`, `driver` and `filter` at once, and the
// seam journeys drive the released `onetaskgraph` against the same store the engine links,
// so the crate is the narrowest edge they can honestly sit behind; same grounds as
// `mod criteria_rule` above.
mod templates;
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
mod turns;
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] these journeys reach
// every caller of the workspace-listing operations across `src/`, so the crate's edge is
// the narrowest they fit behind.
mod unreadable_sessions;
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] the reason is at the
// head of `tests/e2e/unwatched.rs` and is not restated here; this declaration is the other
// site the rule reads, and what it adds is only that the module belongs to this binary for
// the same reason `mod listing` above does — what it exercises is `watchers`, `unwatched`,
// `summary` and `views`, which any change under `src/` can move.
mod unwatched;
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
mod views;
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] the wake budget is
// decided by `watchers`, `unwatched`, `stopguard`, `summary`, `projection` and `driver`'s
// reply path at once, over the run store every launch writes, so any change under `src/`
// can move what it decides: the crate is the narrowest edge it can honestly sit behind, as
// for `mod unwatched` and `mod stop_guard` above.
mod wake_budget;
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] twelve journeys,
// 15.1s together, driving the binary they test — cheaper than single journeys already in
// this file that nextest marks SLOW past 120s. The `onepipeline-note-journeys` edge the
// rule points at belongs to this project already and is unaffected by where this module
// sits; that project is edged on conversational cost, which this module has none of.
mod watch;
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] four of its journeys
// wait past the write-back's sixty-second floor by construction, for the reason at the
// head of `tests/e2e/writeback_budget.rs`; this declaration is the other site the rule
// reads, and the module belongs to this binary for the reason the minute-long schedule
// journeys in `mod store` above do.
mod writeback_budget;
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] one whole run of the
// comparable plan, measured at the store: it exercises writeback, engine and taskgraph
// together, so a narrower project edge would omit it from nx affected when the projection
// changes — the reason `mod writeback_projections` below records.
mod writeback_cost;
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] These journeys exercise
// writeback, engine, and taskgraph together; a narrower project edge would omit them
// from nx affected when the projection changes.
mod writeback_projections;
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
