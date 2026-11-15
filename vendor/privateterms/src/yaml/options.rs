//! The one set of reader options every YAML read in this crate uses.
//!
//! Its own file so there is exactly one place the hardening is stated, and so a test can
//! hold every YAML read in the crate to it.

/// The deepest a YAML declaration may nest. A declaration's deepest value is a string
/// inside an entry inside a list inside the document — four levels — so this leaves
/// generous room and still refuses a document built to exhaust the stack.
pub(crate) const MAX_DEPTH: usize = 16;

/// The most bytes a declaration may be, in either encoding.
pub(crate) const MAX_BYTES: usize = 1024 * 1024;

/// How many alias (`*name`) references a document may make, and how many events replaying
/// them may produce in total: enough for a hand-written file, far short of what an
/// alias-expansion ("billion laughs") document asks for.
const MAX_ALIASES: usize = 64;
const MAX_REPLAYED_EVENTS: usize = 4096;

/// Plain `yes`, `no`, `on` and `off` stay strings; `<<` is an ordinary key; an error
/// carries no rendered snippet of the document (which could quote a term); a duplicate key
/// and an application-specific (custom) tag are errors; and an explicit budget bounds
/// nesting depth, alias expansion and document size.
pub(crate) fn options() -> serde_saphyr::Options {
    serde_saphyr::options! {
        strict_booleans: true,
        merge_keys: serde_saphyr::MergeKeyPolicy::AsOrdinary,
        with_snippet: false,
        duplicate_keys: serde_saphyr::options::DuplicateKeyPolicy::Error,
        reject_unsupported_tags: true,
        alias_limits: serde_saphyr::alias_limits! {
            max_total_replayed_events: MAX_REPLAYED_EVENTS,
            max_alias_expansions_per_anchor: MAX_ALIASES,
        },
        budget: serde_saphyr::budget! {
            max_depth: MAX_DEPTH,
            max_documents: 1,
            max_aliases: MAX_ALIASES,
            max_total_scalar_bytes: MAX_BYTES,
            max_reader_input_bytes: Some(MAX_BYTES),
        },
    }
}
