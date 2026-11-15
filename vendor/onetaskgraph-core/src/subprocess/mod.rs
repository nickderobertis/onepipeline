//! Sources that are other processes, on both sides of the pipe.
//!
//! `docs/plugin-protocol.md` is the normative text; this module is its Rust
//! implementation. [`SubprocessSource`] is the engine's half — a [`TaskSource`] that is a
//! spawned program — and [`serve`] is the plugin's half, hosting any plugin of this build
//! behind the same protocol so the two can be exercised against each other.
//!
//! Nothing here compensates for a capability. A subprocess-hosted source declares what it
//! can do in the handshake exactly as a compiled-in one does, and the engine's one
//! compensation layer reads that declaration and does the rest — which is what keeps a
//! source's answers the same whichever side of a pipe it is on.
//!
//! # What this source declares, field by field
//!
//! One verdict per field of `Capabilities`. Every one of them is **supported and proven,
//! and is the hosted source's own**: [`SubprocessSource`] reports the value the program
//! behind the pipe sent in its handshake, unchanged, and holds no opinion of its own about
//! any of them. Nothing here could be unsupported, because nothing here decides — a
//! hosted source with no documents declares that itself, and this source forwards the
//! declaration rather than holding one.
//!
//! | Field | Verdict |
//! | --- | --- |
//! | `projects` | **Supported and proven** — the hosted source's own, forwarded. |
//! | `documents` | **Supported and proven** — the hosted source's own, forwarded. |
//! | `comments` | **Supported and proven** — the hosted source's own, forwarded, and the four comment methods carried over the pipe as §4.15 and §4.16 specify. |
//! | `assets` | **Supported and proven** — the hosted source's own, forwarded; absent from a handshake written before assets, and then read as unsupported, so such a plugin is never handed an asset. A record's assets cross beside `write_task` and `write_document` as §4.9 specifies; a source over the pipe reports no asset of its own, because the protocol carries no asset read. |
//! | `priority` | **Supported and proven** — the hosted source's own, forwarded; absent from a handshake written before priorities, and then read as unsupported, so such a plugin is never handed a priority it would drop. `set_task_priority` is carried over the pipe as §4.19 specifies. |
//! | `assignees` | **Supported** — the hosted source's own, forwarded; absent from a handshake written before assignees, and then read as unsupported, so such a plugin is never handed an assignee write. |
//! | `filter_by_priority` | **Supported and proven** — the hosted source's own, forwarded; absent from an older handshake, and then read as unsupported, so the engine narrows. |
//! | `filter_by_comment_activity` | **Supported and proven** — the hosted source's own, forwarded, and a query's `commented_since` carried over the pipe as §4.5 specifies; absent from an older handshake, and then read as unsupported, so the engine narrows over the plugin's comments. |
//! | `filter_by_metadata` | **Supported and proven** — the hosted source's own, forwarded, and a query's `metadata` matches carried over the pipe as §4.5 specifies; absent from an older handshake, and then read as unsupported, so the engine narrows over each task's metadata. |
//! | `filter_by_origin` | **Supported and proven** — the hosted source's own, forwarded, and a query's `origin` carried over the pipe as §4.5 specifies, on the same terms. |
//! | `reconcile_status` | **Supported,** as the hosted source declares it — forwarded at the handshake, and `status_contradictions` carried over the pipe as §4.23 specifies; a plugin that does not declare it is sent nothing. |
//! | `complete_by_issue` | **Supported,** as the hosted source declares it — forwarded at the handshake, and `issue_holdings` carried over the pipe as §4.24 specifies; a plugin that does not declare it is sent nothing. |
//! | `orphan_tasks` | **Supported and proven** — the hosted source's own, forwarded. |
//! | `filter_by_label` | **Supported and proven** — the hosted source's own, forwarded. |
//! | `filter_by_status` | **Supported and proven** — the hosted source's own, forwarded. |
//! | `search_title` | **Supported and proven** — the hosted source's own, forwarded. |
//! | `search_content` | **Supported and proven** — the hosted source's own, forwarded. |
//! | `task_dependencies` | **Supported and proven** — the hosted source's own, forwarded. |
//! | `project_dependencies` | **Supported and proven** — the hosted source's own, forwarded. |
//! | `max_page_size` | **Supported and proven** — the hosted source's own, forwarded. |
//!
//! That is proven rather than asserted: the journey table's `subprocess` row hosts the
//! in-memory source over a real pipe and answers every shared journey — every capability
//! field included — with the same rows and the same plan the in-process row does.
//!
//! [`TaskSource`]: onetaskgraph_plugin_api::TaskSource

mod connection;
mod plugin;
mod serve;
mod source;
mod wire;

pub use connection::MAX_LINE;
pub(crate) use plugin::SETTINGS_FIELD;
pub use plugin::{Plugin as SubprocessPlugin, Program, SubprocessConfig};
pub use serve::{serve, serve_plugin};
pub use source::{RequestDeadline, SubprocessSource};
pub(crate) use wire::DocumentDir;
