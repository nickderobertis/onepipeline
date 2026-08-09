//! End-to-end journeys against the compiled binary.
//!
//! Every test here spawns the real `onepipeline` executable as a subprocess and
//! asserts on its exit code, stdout, and stderr — the way a user reaches it. The
//! two sibling CLIs it composes are real executables too, scripted per test, so
//! nothing the code under test does is stubbed inside it.
//!
//! The journeys are ported from `ai-orchestrator`'s own e2e suite, adapted to
//! the command vocabulary `docs/contract.md` fixes.

// llmlint: ignore-file[e2e_not_mocked] the doubles substitute the two *siblings* at
// their subprocess boundary, never anything inside the crate under test. `dispatch.rs`
// drives the real `oneagentgraph` binary and substitutes only the paid model turn;
// every other journey scripts a scenario a real sibling would need paid turns to
// produce, and `onevcs` has no alternative at all — it is still at its interface-only
// stage and refuses every invocation with exit 70. The same rationale, at more length,
// is in `harness.rs`.

mod harness;

mod boundary;
mod channel;
mod crossdag;
mod dispatch;
mod driver;
mod journal;
mod lifecycle;
mod live_edit;
mod plan;
mod shipped;
mod surface;
mod views;
