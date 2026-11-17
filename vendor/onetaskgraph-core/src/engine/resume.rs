//! Where a walk resumes, per source and per stream.
//!
//! This is the inside of the engine's own [`PageToken`](crate::PageToken), and nothing
//! outside this crate names it. A source's [`Cursor`] passes through untouched — the
//! engine stores one and hands it back, exactly as a plugin stores nothing of the
//! engine's page token.
//!
//! The `skip` beside the cursor is the engine counting **its own** rows, never
//! interpreting the source's. It exists because compensation narrows a source page in
//! memory: a page of 50 that yields 7 surviving rows, of which the caller's page had
//! room for 3, has to resume at "the page beginning at this cursor, four surviving rows
//! in". The alternative would be holding the other four somewhere between calls, which
//! is exactly the caching this product does not do.

use onetaskgraph_plugin_api::{Cursor, SourceName};
use serde::{Deserialize, Serialize};

/// Which of a source's streams a state addresses.
///
/// A verb reads one stream per source, except `search --kind both`, which reads two —
/// so the key is the pair rather than the source alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum StreamKind {
    /// The one stream a single-entity verb reads.
    Items,
    /// The task half of a search over both entities.
    Tasks,
    /// The project half of a search over both entities.
    Projects,
    /// The dependents a task of a source declared private records on itself, which a reverse
    /// dependency read of it merges beside its source's own answer.
    Recorded,
}

impl StreamKind {
    /// What this stream is, for a message a user has to act on.
    pub(crate) fn describe(self) -> &'static str {
        match self {
            Self::Items => "a single-entity listing",
            Self::Tasks => "the task half of a search",
            Self::Projects => "the project half of a search",
            Self::Recorded => "the dependents a private task records",
        }
    }
}

/// The place one walk of one stream picks up from.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub(crate) struct Resume {
    /// The source cursor whose page the next row is in, or `None` for the beginning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// How many surviving rows of that page were already delivered.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub skip: u32,
}

/// A page token's whole document: the query it was minted for, and where each of that
/// query's streams picks up.
///
/// The query is here because every cursor below is an offset into *one* result set. Hand
/// them to a different query — the same verb with a different `--label`, a different
/// `--search`, a different `--direction` — and each source resumes at a position that
/// meant something in a walk the caller is no longer doing. The rows that come back are
/// real rows and the exit code is zero, so nothing about the answer says it is arbitrary.
/// That is the same failure the stream-kind check refuses, one layer in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Resumption {
    /// A fingerprint of the query that minted this token — see `Engine`'s `shape`.
    pub query: String,
    /// The one stream owed the next row, or `None` when the last round ended evenly.
    ///
    /// Rows come back one stream at a time, so exactly one stream can be next — and here
    /// rather than as a flag on each state precisely so that "two streams are next" has
    /// no spelling. Omitted when there is none, which leaves a token resuming an evenly
    /// ended round exactly the token it would have been.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owed: Option<Owed>,
    /// Where each of that query's streams picks up.
    pub streams: Vec<StreamState>,
}

/// Which stream hands back the next row when a page boundary falls mid-round.
///
/// A page of three rows over two streams ends having taken two from the first and one
/// from the second, with the second's turn still owed. Without this the next page would
/// begin its rounds at the first stream again, and the walk as a whole would stop
/// alternating — every row would still come back exactly once, but not in the order this
/// product documents, and the order would depend on the page size the caller chose.
///
/// Named by stream rather than by position, because a stream the previous page exhausted
/// is not in the next one and every position after it would shift.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Owed {
    /// The configured source whose turn it is.
    pub source: SourceName,
    /// Which of that source's streams.
    pub stream: StreamKind,
}

/// One stream's place, as the page token carries it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StreamState {
    /// The configured source this stream belongs to.
    pub source: SourceName,
    /// Which of that source's streams it is.
    pub stream: StreamKind,
    /// Where to pick up.
    #[serde(flatten)]
    pub resume: Resume,
}

/// Keeps a token that resumes at the beginning of a stream down to its source and kind.
fn is_zero(value: &u32) -> bool {
    *value == 0
}
