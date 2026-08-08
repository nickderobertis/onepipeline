//! End-to-end journeys against the compiled binary.
//!
//! Every test here spawns the real `onepipeline` executable as a subprocess and
//! asserts on its exit code, stdout, and stderr — the way a user reaches it. The
//! two sibling CLIs it composes are real executables too, scripted per test, so
//! nothing the code under test does is stubbed inside it.
//!
//! The journeys are ported from `ai-orchestrator`'s own e2e suite, adapted to
//! the command vocabulary `docs/contract.md` fixes.

mod harness;

mod boundary;
mod channel;
mod driver;
mod journal;
mod lifecycle;
mod live_edit;
mod plan;
mod shipped;
mod surface;
mod views;
