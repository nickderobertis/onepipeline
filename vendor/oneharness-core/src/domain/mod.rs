//! Pure core: no process, filesystem, env, clock, or terminal I/O lives here.
//! These modules build commands, shape the report, and parse output; all real
//! I/O stays behind the `io` and `commands` boundaries.

// The shared Codex app-server's environment, pool key and frame ownership.
pub mod app_server;
pub mod batch;
// The declared capability surface the parity gate reconciles against.
pub mod capability;
pub mod config;
pub mod control;
pub mod dialogue;
pub mod events;
pub mod fallback;
pub mod gate;
pub mod harness;
pub mod history;
pub mod history_index;
pub mod hooks;
pub mod http;
pub mod mock;
pub mod mode;
// The shared Codex model catalog's pure half; only `io::model_catalog` reads
// it, so it is no surface of this crate.
pub(crate) mod model_catalog;
pub mod normalize;
pub mod render;
pub mod report;
pub mod sdk;
pub mod select;
pub mod session;
pub mod shim;
pub mod signals;
pub mod structured;
pub mod sync;
pub mod usage;
