//! YAML read and written as the engine reads and writes it: the same `serde-saphyr` options
//! as its own crate-private reader, so a document a test or a double writes, and one it reads
//! back out of a run, reads here exactly as it reads in the code under test. The engine's
//! `src/yaml.rs` holds this copy to its own in
//! `the_test_support_reader_and_writer_are_the_engines`.

use serde::de::DeserializeOwned;
use serde::Serialize;

/// Read one YAML document as `T`, or the parser's one-line refusal ending in its line and
/// column.
pub fn from_str<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    let options = serde_saphyr::options! {
        strict_booleans: true,
        merge_keys: serde_saphyr::MergeKeyPolicy::AsOrdinary,
        with_snippet: false,
    };
    serde_saphyr::from_str_with_options(text, options).map_err(|error| error.to_string())
}

/// Write `value` as one block-style YAML document, a long string kept on its one line.
pub fn to_string<T: Serialize>(value: &T) -> Result<String, String> {
    let options = serde_saphyr::ser_options! {
        folded_wrap_chars: usize::MAX,
    };
    serde_saphyr::to_string_with_options(value, options).map_err(|error| error.to_string())
}
