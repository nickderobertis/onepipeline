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
mod journal;
mod lifecycle;
mod live_edit;
mod node_validator;
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] measured, and the
// measurement is why it stays here: `note` is 15.8s of this binary's 1102s of CPU —
// 1.4% — and the whole offline suite runs 268.0s with it and 258.8s without, a 9s
// delta that is the spread between two runs of the same suite. There is no expense
// here to put behind an edge, and one module of thirty-three addressed separately
// would be a structure this repository does not have: the e2e binary is deliberately
// one target, which `just test-e2e` addresses and `.config/nextest.toml` caps the
// concurrency of as a whole, because what these journeys really contend for is
// process trees rather than threads.
mod note;
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
mod plan;
mod real_vcs;
mod scratch;
mod session;
mod session_reuse;
mod shipped;
mod store;
mod surface;
mod turns;
mod views;
