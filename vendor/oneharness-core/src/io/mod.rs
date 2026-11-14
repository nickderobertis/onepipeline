//! I/O boundary: PATH resolution, version probing, and subprocess execution.
//! Everything that touches the process, filesystem, env, or clock lives here or
//! in `commands`, never in `domain`.

// The shared Codex app-server: its pool lease, sweeper, and pooled turns.
pub mod app_server;
pub mod cancel;
pub mod config;
pub mod control;
pub mod detect;
pub mod history;
pub mod hooks;
// Public on purpose, unlike the shared machinery below it: an embedder that
// selects a `<id>:<variant>` itself needs the same environment a run would
// resolve for it, and the CLI re-exports exactly these three items rather than
// resolving an identity a second way.
pub mod identity;
// The starter-config scaffold `oneharness init` writes.
pub mod init;
// The raw HTTP client is an implementation detail of the controlled-turn
// driver, not a surface a consumer of this crate should depend on: it exists
// only because these two control servers speak HTTP.
pub(crate) mod http;
// The driver above it *is* a public surface, and deliberately so: a turn
// submitted to a control server is one of this crate's three execution models,
// so the binary — a separate crate — drives it here exactly as it drives the
// other two through `runner` and `server_pool`.
pub mod http_turn;
// Finding and stopping the processes whose environment carries a marker
// variable: the one handle on a command a shared server started, which no
// process tree this crate owns can reach.
pub mod marked_processes;
// The shared Codex model catalog a Codex launch reads instead of fetching its
// own, and the one refresh a burst of launches shares. Internal: only `run`
// and `usage` launch through it, so no embedder depends on its shape.
pub(crate) mod model_catalog;
mod process;
// The registry description `oneharness list` prints, as a call that returns it.
pub mod registry;
// The run verb's whole orchestration, behind an API that returns a report
// instead of printing one — so the binary is a shell over it and a library
// caller reaches the same engine without a subprocess hop.
pub mod run;
pub mod runner;
// A self-removing temp directory. Public because the three builds that need it
// — this crate's unit tests, the binary crate's unit tests, and the
// integration-test binaries — are three separate compilation units, and a
// `#[cfg(test)]` item reaches only the first.
pub mod scratch;
pub mod server_pool;
pub mod session;
pub mod sync;
pub mod usage;
// The WebSocket client a shared `codex app-server --listen unix://` is spoken
// to over, on an AF_UNIX socket on every platform.
pub mod websocket;
