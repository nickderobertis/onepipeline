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
// their subprocess boundary, never anything inside the crate under test. Both siblings
// are fully implemented, and both have a journey here that drives the real one:
// `dispatch.rs` drives the real `oneagentgraph` binary and substitutes only the paid
// model turn, and `real_vcs.rs` drives the real `onevcs` binary over a real git origin
// through a whole publication. What the doubles buy the journeys in between is a
// scenario stated directly — a rejected gate, a held publication, a session that will
// not open — where the real sibling would need a repository arranged into that state and
// the real agent a paid turn. The same rationale, at more length, is in `harness.rs`.

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
mod real_vcs;
mod shipped;
mod surface;
mod views;
