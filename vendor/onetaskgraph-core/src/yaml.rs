//! Every YAML read the engine makes, through `serde-saphyr`: a configuration, an answers
//! file, a template's front matter and a typed `--var` value.
//!
//! serde-saphyr's own behaviour with three options: `yes`, `no`, `on` and `off` stay strings,
//! as the local Markdown source writes them plain and every document this engine reads was
//! written to be read that way; `<<` is an ordinary key; and a refusal is one line, which
//! every message here names its file or its variable beside. The local Markdown source reads
//! with the same three, in its own crate, because a plugin never depends on the engine;
//! `tests/yaml.rs` reads one document through both and fails the moment they part.

use serde::de::DeserializeOwned;

fn options() -> serde_saphyr::Options {
    serde_saphyr::options! {
        strict_booleans: true,
        merge_keys: serde_saphyr::MergeKeyPolicy::AsOrdinary,
        with_snippet: false,
    }
}

pub(crate) fn from_str<T: DeserializeOwned>(text: &str) -> Result<T, serde_saphyr::Error> {
    serde_saphyr::from_str_with_options(text, options())
}

/// `text` read as one YAML value, refusing any text after it: what a reader would otherwise
/// drop without a word, `title` after `[bracketed]`, is named in the refusal.
pub(crate) fn one_value<T: DeserializeOwned>(text: &str) -> Result<Option<T>, String> {
    let mut documents: Vec<T> =
        serde_saphyr::from_multiple_with_options(text, options()).map_err(|error| {
            // Text a lone value reads without is text after that value.
            let after = from_str::<serde::de::IgnoredAny>(text)
                .ok()
                .and(error.location())
                .and_then(|at| offset(text, at.line(), at.column()))
                .map(|offset| text[offset..].trim_end())
                .filter(|after| !after.is_empty());
            match after {
                Some(after) => format!("`{after}` follows the value ({error})"),
                None => error.to_string(),
            }
        })?;
    match documents.len() {
        0 | 1 => Ok(documents.pop()),
        count => Err(format!(
            "it holds {count} YAML documents where one value was expected"
        )),
    }
}

/// The byte offset of the 1-based `line` and `column` in `text`, when it is inside it.
fn offset(text: &str, line: u64, column: u64) -> Option<usize> {
    let line = usize::try_from(line.checked_sub(1)?).ok()?;
    let column = usize::try_from(column.checked_sub(1)?).ok()?;
    let start: usize = text.split_inclusive('\n').take(line).map(str::len).sum();
    let rest = text.get(start..)?;
    let within = rest
        .char_indices()
        .nth(column)
        .map_or(rest.len(), |(at, _)| at);
    text.is_char_boundary(start + within)
        .then_some(start + within)
}
