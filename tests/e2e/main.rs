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
mod amend;
mod boundary;
mod cancellation;
mod channel;
mod concurrency;
mod context_delivery;
mod criteria;
mod crossdag;
mod dispatch;
mod driver;
mod envelope_reviewer;
mod filter;
mod holds;
mod journal;
mod lifecycle;
mod live_edit;
mod loopcost;
mod node_validator;
mod plan;
mod plan_check;
mod real_vcs;
mod scratch;
mod session;
mod session_reuse;
mod shipped;
mod store;
mod summary;
mod surface;
mod turns;
mod views;
// llmlint: ignore[expensive_tests_stay_behind_their_own_edge] this module's eleven
// journeys cost 20.7s together — cheaper than several single journeys already in this
// binary, two of which nextest marks SLOW past 120s — and their edges reach only what
// they test: they drive the `onepipeline` binary against `src/watch.rs`, `src/cli.rs`,
// `src/journal.rs` and `src/views.rs`, all of the project this suite belongs to. The
// implicit dependency on `onepipeline-note-journeys` is a pre-existing edge of that
// project rather than anything this module adds, and it points the other way: it makes a
// note change run the crate's suite, which no placement of this module alters. That
// separately-edged project is edged on *conversational* cost — each of its journeys holds
// a two-party turn open — and it had to grow its own instrumented recipe and a
// `coverage-clean` edge to stay inside the 95% floor; giving one cheap module that shape
// would buy nothing and take it out of the run that measures it.
mod watch;
