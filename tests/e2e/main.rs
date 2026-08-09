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
// their subprocess boundary, never anything inside the crate under test, and there is
// no alternative today: both crates are at their own interface-only stage, so the real
// `oneagentgraph run` and `onevcs session open` refuse every invocation with exit 70. A
// suite built on them would prove only that this crate can start a process that says no.
// The same rationale, at more length, is in `harness.rs`; revisit each seam as its
// sibling implements it.

mod harness;

mod boundary;
mod channel;
mod crossdag;
mod driver;
mod journal;
mod lifecycle;
mod live_edit;
mod plan;
mod shipped;
mod surface;
mod views;
