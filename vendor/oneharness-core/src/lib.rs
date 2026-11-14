//! oneharness-core: the reusable engine behind the `oneharness` CLI, and a
//! depend-able library for tools that install hooks into — or sync configs for
//! — the supported agentic coding harnesses (Claude Code, Codex, OpenCode,
//! Goose, Qwen Code, Crush, Copilot CLI, Cursor).
//!
//! - [`domain`] is pure: it builds argv, parses output, shapes the report,
//!   holds the harness registry, renders hooks, layers config, and computes the
//!   sync merge — with no process / filesystem / env / clock I/O.
//! - [`io`] performs the real I/O: PATH resolution, version probing, spawning,
//!   reading config, and writing harness config files and hooks.
//!
//! Two capabilities a consumer most often wants:
//!
//! - **Drive a run.** [`io::run::run`] is the whole `oneharness run` verb as a
//!   call: it takes an [`io::run::RunRequest`] (the flag surface as plain data)
//!   and *returns* the [`domain::report::RunReport`], publishing normalized
//!   events to a caller-supplied [`io::run::EventSink`] as they occur and
//!   tearing the harness tree down on a caller-owned
//!   [`io::cancel::CancelToken`]. Nothing on that path writes to the process's
//!   stdout, so an embedder whose own stdout is a contract can use it, and
//!   [`io::run::run_supervised`] puts each harness child into the process group
//!   / job object that embedder supervises.
//! - **Install a hook.** Build a [`domain::hooks::HookSpec`] and call
//!   [`io::hooks::install`] to write a pre-tool hook into any harness in that
//!   harness's native shape (a shared config file, a dedicated hooks file, or a
//!   plugin). It is generic — the command and plugin identity are
//!   caller-supplied, never hardcoded.

pub mod domain;
pub mod errors;
pub mod io;
#[cfg(feature = "sdk-schema")]
pub mod sdk_schema;
