//! The shared Codex model catalog, as data: which file a launch reads, when it
//! is fresh, what a refresh must see in Codex's own cache before it copies it,
//! and how a failed refresh is remembered.
//!
//! Codex fetches its whole model catalog (a few hundred KB) on every process
//! start, ignoring the cache it writes. oneharness instead keeps one catalog
//! file per Codex home and Codex version, refreshed by one short-lived
//! `codex app-server` when it grows older than the configured interval, and
//! hands every launch `-c model_catalog_json=<that file>`. The spawning,
//! locking and file I/O are [`crate::io::model_catalog`]'s; everything here is
//! pure.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::domain::usage::epoch_millis_from_rfc3339;

/// The longest one refresh waits for Codex to write a fresh catalog into its
/// cache before it gives up and records a failure.
pub const MODEL_CATALOG_REFRESH_DEADLINE: Duration = Duration::from_secs(10);
/// The longest a launch waits for another process's refresh: the refresh
/// deadline plus the refresh helper's start.
pub const MODEL_CATALOG_WAIT: Duration = Duration::from_secs(12);
/// How long after a failed refresh every launch goes without a fresh catalog —
/// with the stale file when one exists, else with Codex's own fetch — before
/// another refresh is tried.
pub const MODEL_CATALOG_FAILURE_BACKOFF: Duration = Duration::from_secs(300);

/// How long `<codex> --version` may take before a launch goes without a
/// shared catalog: the version is half the catalog's key.
pub const CODEX_VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// The name of the cache Codex writes in its home after each catalog fetch.
pub const CODEX_MODELS_CACHE: &str = "models_cache.json";

/// The version a Codex executable reports, from its `--version` output
/// (`codex-cli 0.159.2` → `0.159.2`): the first non-empty line must be exactly
/// `codex-cli <version>`. Anything else — another program's version line, a
/// wrapper's banner — is no Codex this module can key a catalog for.
#[must_use]
pub fn codex_version(version_output: &str) -> Option<&str> {
    let line = version_output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let word = line.strip_prefix(CODEX_VERSION_PREFIX)?.trim();
    let looks_like_a_version = word.starts_with(|c: char| c.is_ascii_digit())
        && word.contains('.')
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'));
    looks_like_a_version.then_some(word)
}

const CODEX_VERSION_PREFIX: &str = "codex-cli ";

/// The catalog file's stem for one Codex home and one executable: a digest of
/// the resolved `CODEX_HOME` and the executable's `--version` output, so two
/// homes, or two versions, never share a catalog.
#[must_use]
pub fn catalog_key(codex_home: &str, version_output: &str) -> String {
    let material = format!("{codex_home}\n{}", version_output.trim());
    Uuid::new_v5(&Uuid::NAMESPACE_OID, material.as_bytes())
        .simple()
        .to_string()
}

/// Whether a catalog written `age` ago is still fresh under `interval`.
#[must_use]
pub fn is_fresh(age: Duration, interval: Duration) -> bool {
    age < interval
}

// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] The cache's fields (`fetched_at`, `client_version`, `models`) are real Codex's `models_cache.json`, which only an installed Codex can confirm; the live phase `oh_usage_enforce codex` (scripts/e2e-lib.sh) runs a usage probe from a cold catalog against real Codex and fails, naming this reader, when the refresh that reads them does. The mock's cache (tests/support/mock_harness.rs) is held to the same fields by the refresh journeys.
/// A catalog file's contents, `{"models": [...]}`, each model an object with
/// a `slug`: only [`read_codex_cache`] makes one, from a cache it validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog(String);

impl Catalog {
    /// The file's text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What Codex's own cache says to a refresh that started at `started_ms`.
#[derive(Debug, PartialEq, Eq)]
pub enum CacheReading {
    /// The cache holds a catalog fetched no earlier than the millisecond the
    /// refresh started, by this executable's version.
    Fresh(Catalog),
    /// Not yet: no cache, one fetched in a millisecond before the refresh's, or
    /// one another version of Codex wrote.
    NotYet,
}

/// Read Codex's `models_cache.json` text for a refresh that started at
/// `started_ms` (milliseconds since the UNIX epoch) against executable version
/// `version`. A cache that is not JSON, or whose `models` is not a non-empty
/// array of model objects (each with a string `slug`), reads as not yet: Codex
/// may be mid-write, and the deadline bounds it.
#[must_use]
pub fn read_codex_cache(text: &str, started_ms: i64, version: &str) -> CacheReading {
    let Ok(cache) = serde_json::from_str::<Value>(text) else {
        return CacheReading::NotYet;
    };
    // Both instants are read to the millisecond, so a fetch inside the
    // refresh's own starting millisecond counts as after it: a helper can
    // start and fetch within one.
    let fetched_after_start = cache
        .get("fetched_at")
        .and_then(Value::as_str)
        .and_then(epoch_millis_from_rfc3339)
        .is_some_and(|fetched| fetched >= started_ms);
    let same_version = cache.get("client_version").and_then(Value::as_str) == Some(version);
    match cache.get("models") {
        Some(Value::Array(models)) if fetched_after_start && same_version && are_models(models) => {
            CacheReading::Fresh(Catalog(serde_json::json!({ "models": models }).to_string()))
        }
        _ => CacheReading::NotYet,
    }
}
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

/// A non-empty list of model objects, each naming its `slug`: the least a
/// catalog Codex reads must hold.
fn are_models(models: &[Value]) -> bool {
    !models.is_empty()
        && models
            .iter()
            .all(|model| model.get("slug").is_some_and(Value::is_string))
}

/// Whether `bytes` are a catalog file Codex can read: JSON whose `models` is a
/// non-empty array of model objects, which is what [`read_codex_cache`] makes.
#[must_use]
pub fn is_usable_catalog(bytes: &[u8]) -> bool {
    serde_json::from_slice::<Value>(bytes).is_ok_and(|catalog| {
        catalog
            .get("models")
            .and_then(Value::as_array)
            .is_some_and(|models| are_models(models))
    })
}

/// The record a failed refresh leaves beside the catalog file, which holds
/// every launch off refreshing again until [`MODEL_CATALOG_FAILURE_BACKOFF`]
/// has passed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureRecord {
    /// When the refresh failed, in milliseconds since the UNIX epoch.
    pub failed_at_ms: i64,
    /// Why, as the warning a launch prints names it.
    pub reason: String,
}

impl FailureRecord {
    /// Whether the backoff this record starts is still running at `now_ms`.
    #[must_use]
    pub fn backing_off(&self, now_ms: i64) -> bool {
        let backoff = i64::try_from(MODEL_CATALOG_FAILURE_BACKOFF.as_millis()).unwrap_or(i64::MAX);
        now_ms.saturating_sub(self.failed_at_ms) < backoff
    }

    /// The warning a launch prints while this record holds refreshes off:
    /// what failed, and what the launch uses instead.
    #[must_use]
    pub fn warning(&self, stale_catalog: bool) -> String {
        let instead = if stale_catalog {
            "the existing stale catalog"
        } else {
            "Codex's own catalog fetch"
        };
        let backoff = MODEL_CATALOG_FAILURE_BACKOFF.as_secs();
        format!(
            "the shared Codex model catalog could not be refreshed ({}); this launch uses \
             {instead}, and no refresh is tried until {backoff} s after that failure",
            self.reason
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_codex_version_is_the_last_word_of_its_first_line() {
        assert_eq!(codex_version("codex-cli 0.159.2\n"), Some("0.159.2"));
        assert_eq!(
            codex_version("\n  codex-cli 1.0.0-alpha.3 \n"),
            Some("1.0.0-alpha.3")
        );
        for other in [
            "",
            "{\"result\":\"mock ok\"}",
            "codex",
            "codex-cli dev",
            "v1.2",
            "other-cli 1.2.3",
            "codex-cli 1.2.3 extra",
        ] {
            assert_eq!(codex_version(other), None, "{other:?}");
        }
    }

    #[test]
    fn the_key_separates_homes_and_versions() {
        let key = catalog_key("/home/a/.codex", "codex-cli 0.1.0");
        assert_eq!(key, catalog_key("/home/a/.codex", "codex-cli 0.1.0\n"));
        assert_ne!(key, catalog_key("/home/b/.codex", "codex-cli 0.1.0"));
        assert_ne!(key, catalog_key("/home/a/.codex", "codex-cli 0.1.1"));
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()), "{key}");
    }

    #[test]
    fn a_catalog_is_fresh_only_inside_its_interval() {
        let hour = Duration::from_secs(3_600);
        assert!(is_fresh(Duration::ZERO, hour));
        assert!(is_fresh(Duration::from_secs(3_599), hour));
        assert!(!is_fresh(hour, hour));
    }

    #[test]
    fn a_refresh_takes_only_a_cache_fetched_after_it_started_by_its_version() {
        let started = epoch_millis_from_rfc3339("2026-10-13T05:54:41.500Z").unwrap();
        let cache = |fetched_at: &str, version: &str, models: &str| {
            format!(
                r#"{{"fetched_at":"{fetched_at}","etag":"W/\"x\"","client_version":"{version}","models":{models}}}"#
            )
        };
        let fresh = read_codex_cache(
            &cache(
                "2026-10-13T05:54:41.501234567Z",
                "0.159.2",
                r#"[{"slug":"m"}]"#,
            ),
            started,
            "0.159.2",
        );
        assert_eq!(
            fresh,
            CacheReading::Fresh(Catalog(r#"{"models":[{"slug":"m"}]}"#.to_string()))
        );
        for (text, why) in [
            (
                cache("2026-10-13T05:54:41.499Z", "0.159.2", r#"[{"slug":"m"}]"#),
                "before the start",
            ),
            (
                cache("2026-10-13T05:54:41.900Z", "0.159.1", r#"[{"slug":"m"}]"#),
                "another version",
            ),
            (
                cache("2026-10-13T05:54:41.900Z", "0.159.2", "[]"),
                "no models",
            ),
            (
                cache("2026-10-13T05:54:41.900Z", "0.159.2", "[null]"),
                "a model that is no object",
            ),
            (
                cache("2026-10-13T05:54:41.900Z", "0.159.2", r#"[{"name":"m"}]"#),
                "a model naming no slug",
            ),
            (
                cache("not a time", "0.159.2", r#"[{"slug":"m"}]"#),
                "no fetch time",
            ),
            ("{\"models\":".to_string(), "mid-write"),
        ] {
            assert_eq!(
                read_codex_cache(&text, started, "0.159.2"),
                CacheReading::NotYet,
                "{why}"
            );
        }
    }

    /// A helper can start and fetch inside the refresh's own millisecond (the
    /// mock codex's fetch lands 0 ms after its start), and both instants are
    /// read to the millisecond: that fetch is the refresh's, not an older one.
    #[test]
    fn a_cache_fetched_in_the_refreshs_starting_millisecond_is_fresh() {
        let started = epoch_millis_from_rfc3339("2026-10-13T05:54:41.500Z").unwrap();
        let models = r#"[{"slug":"m"}]"#;
        for fetched_at in ["2026-10-13T05:54:41.500Z", "2026-10-13T05:54:41.500999999Z"] {
            let text = format!(
                r#"{{"fetched_at":"{fetched_at}","client_version":"0.159.2","models":{models}}}"#
            );
            assert_eq!(
                read_codex_cache(&text, started, "0.159.2"),
                CacheReading::Fresh(Catalog(format!(r#"{{"models":{models}}}"#))),
                "{fetched_at}"
            );
        }
    }

    /// The README is where a user learns the catalog's interval, its waits
    /// and its backoff, and which requests a launch makes; prose that drifted
    /// from these constants would be confidently wrong.
    #[test]
    fn the_readme_states_the_constants_and_every_request_row() {
        let readme = include_str!("../../../../README.md")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let stated = [
            format!(
                "waits — at most {} s — for Codex",
                MODEL_CATALOG_REFRESH_DEADLINE.as_secs()
            ),
            format!(
                "at most {} s, the refresh deadline plus the helper's start",
                MODEL_CATALOG_WAIT.as_secs()
            ),
            format!(
                "for {} minutes from it every launch",
                MODEL_CATALOG_FAILURE_BACKOFF.as_secs() / 60
            ),
            format!(
                "(default `{}`, an hour; at least `1`)",
                crate::domain::config::DEFAULT_MODEL_CATALOG_REFRESH_SECONDS
            ),
        ];
        for phrase in &stated {
            assert!(
                readme.contains(phrase.as_str()),
                "README must state: {phrase}"
            );
        }
        for row in [
            "| Model catalog |",
            "| Analytics |",
            "| ChatGPT apps (connectors) MCP |",
            "| Responses websocket: prewarm and turn |",
            "| Git-attribution read |",
            "| In-turn read |",
            "| Cloud config bundle |",
            "| OAuth refresh |",
            "| Usage probe's reads |",
        ] {
            assert!(
                readme.contains(row),
                "README's request table must have the row {row}"
            );
        }
    }

    #[test]
    fn only_a_catalog_with_models_is_usable() {
        assert!(is_usable_catalog(br#"{"models":[{"slug":"m"}]}"#));
        for bad in [
            &b""[..],
            b"{",
            b"{}",
            br#"{"models":[]}"#,
            br#"{"models":{}}"#,
            b"[]",
            br#"{"models":[null]}"#,
            br#"{"models":[{"slug":1}]}"#,
        ] {
            assert!(!is_usable_catalog(bad), "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn a_failure_holds_refreshes_off_for_the_backoff_and_says_what_a_launch_uses() {
        let record = FailureRecord {
            failed_at_ms: 1_000_000,
            reason: "Codex wrote no catalog within 10 s".to_string(),
        };
        let backoff = i64::try_from(MODEL_CATALOG_FAILURE_BACKOFF.as_millis()).unwrap();
        assert!(record.backing_off(1_000_000));
        assert!(record.backing_off(1_000_000 + backoff - 1));
        assert!(!record.backing_off(1_000_000 + backoff));
        assert!(record.warning(true).contains("the existing stale catalog"));
        assert!(record.warning(false).contains("Codex's own catalog fetch"));
        assert!(record.warning(false).contains("within 10 s"));
    }
}
