//! Harness selection: turn a caller's `--all` / include / exclude selectors into
//! the ordered registry entries a run drives.
//!
//! Pure: it only reads the static registry. It lives beside the registry rather
//! than in the CLI because every entry point that names harnesses — `run`,
//! `detect`, `sync`, `usage`, and any library caller of [`crate::io::run`] —
//! must resolve the same selectors the same way.

use crate::domain::harness::{self, HarnessSpec};
use crate::errors::OneharnessError;

/// Preserve selector order while removing exactly repeated ids.
// llmlint: ignore[invalid_states_unrepresentable] This boundary intentionally normalizes raw CLI/config selector text before select_specs validates it; a validated selector type could not carry malformed input into the precise usage diagnostic.
#[must_use]
pub fn dedupe_exact_ids(ids: &[String]) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    ids.iter()
        .filter(|id| seen.insert((*id).clone()))
        .cloned()
        .collect()
}

/// Resolve `all` in registry order or an explicit selection in caller order.
///
/// `all` selects every harness (minus `exclude`); otherwise `include` lists the
/// ids to run. An empty selection, or any unknown id, is a usage error.
// llmlint: ignore[invalid_states_unrepresentable] Raw external selector strings are the input contract here so unknown/malformed ids can be reported verbatim; every id is registry-validated before any HarnessSpec is returned, and callers retain the same ordered selectors only for variant resolution.
pub fn select_specs(
    all: bool,
    include: &[String],
    exclude: &[String],
) -> Result<Vec<&'static HarnessSpec>, OneharnessError> {
    // Validate every named id up front so a typo fails loudly, not silently.
    for id in include.iter().chain(exclude.iter()) {
        let base = id.split_once(':').map_or(id.as_str(), |(base, _)| base);
        if harness::by_id(base).is_none() {
            return Err(OneharnessError::UnknownHarness {
                id: id.clone(),
                valid: harness::valid_ids(),
            });
        }
    }

    if all {
        let excluded: Vec<&str> = exclude.iter().map(String::as_str).collect();
        let specs: Vec<_> = harness::all()
            .iter()
            .filter(|s| !excluded.contains(&s.id))
            .collect();
        if specs.is_empty() {
            return Err(OneharnessError::NoSelection);
        }
        return Ok(specs);
    }

    if include.is_empty() {
        return Err(OneharnessError::NoSelection);
    }

    // Preserve caller order: variants of one base harness are distinct
    // candidates, while an exactly repeated composed id is de-duplicated.
    Ok(dedupe_exact_ids(include)
        .iter()
        .map(|id| {
            let base = id.split_once(':').map_or(id.as_str(), |(base, _)| base);
            harness::by_id(base).expect("validated above")
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_minus_exclude() {
        let specs = select_specs(true, &[], &["codex".into(), "goose".into()]).unwrap();
        assert_eq!(specs.len(), harness::all().len() - 2);
        assert!(!specs.iter().any(|s| s.id == "codex" || s.id == "goose"));
    }

    #[test]
    fn include_preserves_caller_order_and_dedupes_exact_ids() {
        let specs = select_specs(
            false,
            &["cursor".into(), "claude-code".into(), "cursor".into()],
            &[],
        )
        .unwrap();
        let ids: Vec<_> = specs.iter().map(|s| s.id).collect();
        assert_eq!(ids, vec!["cursor", "claude-code"]);
    }

    #[test]
    fn empty_selection_is_an_error() {
        assert!(matches!(
            select_specs(false, &[], &[]),
            Err(OneharnessError::NoSelection)
        ));
    }

    #[test]
    fn unknown_id_is_an_error() {
        assert!(matches!(
            select_specs(false, &["nope".into()], &[]),
            Err(OneharnessError::UnknownHarness { .. })
        ));
    }
}
