//! Where a live driver's claim on its project stands: the `claim.json` sidecar in the run's
//! directory, and the `claim` object `status --json` carries.
//!
//! The driver reads its project's claim on a thread of its own, once per claim interval,
//! and after every read writes down what that read found — whole, through a temporary file
//! renamed over the last — because the views are other processes that read the run's
//! directory and never the driver. Like `owner.lock` and `channel/deliveries.json` it is
//! driver state, not anything a fold of the journal could rebuild, and no journal record is
//! written per read.
//!
//! **One declaration.** [`ClaimReading`] is both the body of the sidecar and the `claim`
//! object of the run reading, so the writer and every reader import this type rather than
//! restating it, and `schemas/run-reading.schema.json` is generated from it.
//!
//! The file is additive: a build that does not know it never reads it, and a missing or
//! unreadable file means nothing is known about the claim — how a run of an older engine
//! reads — never that the claim is unconfirmed.

use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The sidecar's file name, inside the run's directory.
pub(crate) const CLAIM_FILE: &str = "claim.json";

/// The version of the sidecar document.
pub(crate) const CLAIM_SCHEMA_VERSION: u32 = 1;

/// The keys the sidecar carries, and the only ones.
const CLAIM_KEYS: [&str; 7] = [
    "schema_version",
    "state",
    "last_confirmed_at",
    "unconfirmed_since",
    "reason",
    "detail",
    "lease_interval_s",
];

/// Where the driver's claim on its project stands, as its newest read left it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ClaimState {
    /// A read within the lease window found the project this driver's.
    Confirmed,
    /// No read has confirmed the claim within the lease window: the driver's dispatches in
    /// flight keep running, and it starts no new dispatch and opens no new publication
    /// until a read confirms the claim again.
    Unconfirmed,
    /// A read found another driver's claim: this driver ends its dispatches and lets go.
    Superseded,
}

/// Why no read has confirmed the claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum UnconfirmedReason {
    /// The read due did not answer within its deadline.
    Late,
    /// The read was refused, or answered with something that confirms nothing.
    Failed,
    /// The read was declined for the store's rate limit: refused for it, or not asked
    /// inside a wait the store asked for.
    RateLimited,
}

impl UnconfirmedReason {
    /// The word the document carries.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Late => "late",
            Self::Failed => "failed",
            Self::RateLimited => "rate-limited",
        }
    }

    /// What a person reads for it, in a sentence about the newest read.
    pub(crate) fn phrase(self) -> &'static str {
        match self {
            Self::Late => "the claim read was late",
            Self::Failed => "the claim read failed",
            Self::RateLimited => "the claim read was declined for the store's rate limit",
        }
    }
}

/// The driver's claim, as `status --json` reads it and as `claim.json` carries it.
///
/// Instants are carried as epoch milliseconds and written as RFC 3339, as the document
/// states, so one read back that is not an instant is refused where it is read.
// llmlint: ignore-block[invalid_states_unrepresentable] the five fields are the plan's
// contracted wire shape for the `claim` reading and `claim.json`, field for field, with
// `null` where a field does not apply; the driver's own state is an enum (`Standing` in
// `src/continuation.rs`) that builds this in one place, and `ClaimFile::parse` refuses a
// document whose fields disagree with its `state`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct ClaimReading {
    /// Where the claim stands.
    pub state: ClaimState,
    /// When the newest read that confirmed the claim began — `null` where none has.
    #[serde(with = "crate::channel::deliveries::optional_instant")]
    #[schemars(with = "Option<String>")]
    pub last_confirmed_at: Option<u64>,
    /// When the claim became unconfirmed — `null` unless it is.
    #[serde(with = "crate::channel::deliveries::optional_instant")]
    #[schemars(with = "Option<String>")]
    pub unconfirmed_since: Option<u64>,
    /// Why it is unconfirmed — `null` unless it is.
    pub reason: Option<UnconfirmedReason>,
    /// The newest read's own words: why it confirmed nothing, or the claim that
    /// superseded this driver — `null` where the claim is confirmed.
    pub detail: Option<String>,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

impl ClaimReading {
    /// A claim the newest read confirmed, from `at`.
    pub(crate) fn confirmed(at: u64) -> Self {
        Self {
            state: ClaimState::Confirmed,
            last_confirmed_at: Some(at),
            unconfirmed_since: None,
            reason: None,
            detail: None,
        }
    }

    /// A claim no read has confirmed since `last_confirmed_at`, unconfirmed since `since`.
    pub(crate) fn unconfirmed(
        last_confirmed_at: Option<u64>,
        since: u64,
        reason: UnconfirmedReason,
        detail: String,
    ) -> Self {
        Self {
            state: ClaimState::Unconfirmed,
            last_confirmed_at,
            unconfirmed_since: Some(since),
            reason: Some(reason),
            detail: Some(detail),
        }
    }

    /// A claim another driver's replaced, as `detail` says.
    pub(crate) fn superseded(last_confirmed_at: Option<u64>, detail: String) -> Self {
        Self {
            state: ClaimState::Superseded,
            last_confirmed_at,
            unconfirmed_since: None,
            reason: None,
            detail: Some(detail),
        }
    }

    /// Whether the fields agree with the state: an unconfirmed claim says since when and
    /// why, and no other says either.
    fn consistent(&self) -> bool {
        match self.state {
            ClaimState::Unconfirmed => {
                self.unconfirmed_since.is_some() && self.reason.is_some() && self.detail.is_some()
            }
            ClaimState::Confirmed => {
                self.unconfirmed_since.is_none()
                    && self.reason.is_none()
                    && self.detail.is_none()
                    && self.last_confirmed_at.is_some()
            }
            ClaimState::Superseded => {
                self.unconfirmed_since.is_none() && self.reason.is_none() && self.detail.is_some()
            }
        }
    }

    /// How long, in whole seconds at `now` (epoch milliseconds), since the newest read that
    /// confirmed the claim — or, where none has, since it became unconfirmed.
    #[must_use]
    pub fn unconfirmed_for(&self, now: u64) -> Option<u64> {
        self.last_confirmed_at
            .or(self.unconfirmed_since)
            .map(|from| now.saturating_sub(from) / 1_000)
    }
}

/// The whole `claim.json` document: the reading, the version, and the lease it is read
/// against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct ClaimFile {
    /// [`CLAIM_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Where the claim stands.
    #[serde(flatten)]
    pub reading: ClaimReading,
    /// The claim interval the driver reads on, in whole seconds: the lease window an
    /// unconfirmed claim is measured against.
    pub lease_interval_s: u64,
}

impl ClaimFile {
    /// Read one document: refused where it is not JSON, not this shape, not this version,
    /// or its fields disagree with its state.
    ///
    /// # Errors
    ///
    /// What was wrong with it, in words.
    pub fn parse(text: &str) -> Result<Self, String> {
        // The document's keys exactly: a flattened reading cannot refuse unknown fields
        // itself, so a misspelled or unexpected one is refused here rather than dropped.
        let keys: std::collections::BTreeSet<String> =
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(text)
                .map_err(|error| error.to_string())?
                .into_iter()
                .map(|(key, _)| key)
                .collect();
        if let Some(unknown) = keys.iter().find(|key| !CLAIM_KEYS.contains(&key.as_str())) {
            return Err(format!("an unknown key `{unknown}`"));
        }
        // Every key is written, `null` where it does not apply: one missing is no document
        // this build wrote.
        if let Some(missing) = CLAIM_KEYS.iter().find(|key| !keys.contains(**key)) {
            return Err(format!("no `{missing}`"));
        }
        let file: Self = serde_json::from_str(text).map_err(|error| error.to_string())?;
        if !(1..=crate::continuation::MAX_CLAIM_INTERVAL.as_secs()).contains(&file.lease_interval_s)
        {
            return Err(format!(
                "a lease interval of {}s, outside 1 to {}",
                file.lease_interval_s,
                crate::continuation::MAX_CLAIM_INTERVAL.as_secs()
            ));
        }
        if file.schema_version != CLAIM_SCHEMA_VERSION {
            return Err(format!(
                "schema_version {} is not {CLAIM_SCHEMA_VERSION}",
                file.schema_version
            ));
        }
        if !file.reading.consistent() {
            return Err(format!(
                "a `{}` claim whose fields disagree with it",
                serde_json::to_value(file.reading.state)
                    .map_or_else(|_| String::new(), |state| state.to_string())
            ));
        }
        Ok(file)
    }

    /// The run's sidecar, or `None` where there is none or it cannot be read: nothing is
    /// known.
    pub(crate) fn read(run_dir: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(run_dir.join(CLAIM_FILE)).ok()?;
        Self::parse(&text).ok()
    }

    /// Write the run's sidecar whole, replacing the last. Best effort: what could not be
    /// written is said on standard error, and the driver goes on.
    pub(crate) fn write(&self, run_dir: &Path) {
        let path = run_dir.join(CLAIM_FILE);
        // llmlint: ignore-block[changed_behavior_has_e2e] writing a file in the run's own
        // directory fails only on a host whose run directory has been made unwritable, which no
        // CLI journey arranges; every claim journey reads the write that lands.
        let written = serde_json::to_vec(self)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                crate::ledger::write_atomic(&path, &bytes, crate::ledger::Durability::Projection)
                    .map_err(|error| error.to_string())
            });
        if let Err(error) = written {
            eprintln!(
                "onepipeline: could not write the claim's standing to {}: {error}",
                path.display()
            );
        }
        // llmlint: ignore-end[changed_behavior_has_e2e]
    }
}

/// How long `seconds` is, as a person reads it beside a lease: seconds under two minutes,
/// minutes under two hours, and hours and minutes past that.
pub(crate) fn span(seconds: u64) -> String {
    match seconds {
        0..120 => format!("{seconds} s"),
        120..7_200 => format!("{} min", seconds / 60),
        _ => format!("{} h {} min", seconds / 3_600, seconds % 3_600 / 60),
    }
}

/// How long `seconds` is, compactly, for a watch line: one whole unit, at most six
/// characters — seconds under two minutes, minutes under a hundred, hours under a hundred,
/// and days past that, up to 9999 (some 27 years, where it stops). [`span`] says it in full
/// for `status` and the planner surface; a watch line only has room for its order.
pub(crate) fn watch_age(seconds: u64) -> String {
    match seconds {
        0..120 => format!("{seconds} s"),
        120..6_000 => format!("{} min", seconds / 60),
        6_000..360_000 => format!("{} h", seconds / 3_600),
        _ => format!("{} d", (seconds / 86_400).min(9_999)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(reading: ClaimReading) -> ClaimFile {
        ClaimFile {
            schema_version: CLAIM_SCHEMA_VERSION,
            reading,
            lease_interval_s: 300,
        }
    }

    #[test]
    fn each_state_is_written_in_the_contracted_shape_and_read_back() {
        let at = 1_760_000_000_000;
        let unconfirmed = file(ClaimReading::unconfirmed(
            Some(at),
            at + 300_000,
            UnconfirmedReason::RateLimited,
            "the board is down".into(),
        ));
        let written = serde_json::to_value(&unconfirmed).unwrap();
        assert_eq!(
            written,
            serde_json::json!({
                "schema_version": 1,
                "state": "unconfirmed",
                "last_confirmed_at": crate::sys::rfc3339_from_millis(at),
                "unconfirmed_since": crate::sys::rfc3339_from_millis(at + 300_000),
                "reason": "rate-limited",
                "detail": "the board is down",
                "lease_interval_s": 300,
            })
        );
        for each in [
            unconfirmed,
            file(ClaimReading::confirmed(at)),
            file(ClaimReading::superseded(Some(at), "run 'b'".into())),
            file(ClaimReading::superseded(None, "run 'b'".into())),
        ] {
            let text = serde_json::to_string(&each).unwrap();
            assert_eq!(ClaimFile::parse(&text), Ok(each));
        }
        let confirmed = serde_json::to_value(file(ClaimReading::confirmed(at))).unwrap();
        for key in ["unconfirmed_since", "reason", "detail"] {
            assert_eq!(confirmed[key], serde_json::Value::Null, "{key}");
        }
    }

    /// What a reader racing the writer would see were the file not replaced whole: an
    /// empty or a truncated document, which `parse` refuses rather than reading as
    /// something it is not — so a reader that never sees a refusal has never seen one.
    #[test]
    fn an_empty_truncated_or_inconsistent_document_is_refused() {
        let whole = serde_json::to_string(&file(ClaimReading::unconfirmed(
            None,
            1_760_000_000_000,
            UnconfirmedReason::Late,
            "no answer".into(),
        )))
        .unwrap();
        assert!(ClaimFile::parse("").is_err());
        for cut in 1..whole.len() {
            assert!(
                ClaimFile::parse(&whole[..cut]).is_err(),
                "{}",
                &whole[..cut]
            );
        }
        let mut wrong: serde_json::Value = serde_json::from_str(&whole).unwrap();
        wrong["reason"] = serde_json::Value::Null;
        assert!(ClaimFile::parse(&wrong.to_string()).is_err());
        wrong["state"] = serde_json::json!("confirmed");
        assert!(ClaimFile::parse(&wrong.to_string()).is_err());
        for key in CLAIM_KEYS {
            let mut short: serde_json::Value = serde_json::from_str(&whole).unwrap();
            short.as_object_mut().unwrap().remove(key);
            assert!(
                ClaimFile::parse(&short.to_string()).is_err(),
                "without {key}"
            );
        }
        let mut odd: serde_json::Value = serde_json::from_str(&whole).unwrap();
        odd["lease_interval"] = serde_json::json!(5);
        assert!(
            ClaimFile::parse(&odd.to_string()).is_err(),
            "an unknown key"
        );
        for interval in [0, 86_401] {
            let mut lease: serde_json::Value = serde_json::from_str(&whole).unwrap();
            lease["lease_interval_s"] = serde_json::json!(interval);
            assert!(ClaimFile::parse(&lease.to_string()).is_err(), "{interval}");
        }
        let at = 1_760_000_000_000;
        let mut chatty = serde_json::to_value(file(ClaimReading::confirmed(at))).unwrap();
        chatty["detail"] = serde_json::json!("why");
        assert!(ClaimFile::parse(&chatty.to_string()).is_err());
        let mut silent =
            serde_json::to_value(file(ClaimReading::superseded(Some(at), "b".into()))).unwrap();
        silent["detail"] = serde_json::Value::Null;
        assert!(ClaimFile::parse(&silent.to_string()).is_err());
        let mut later: serde_json::Value = serde_json::from_str(&whole).unwrap();
        later["schema_version"] = serde_json::json!(2);
        assert!(ClaimFile::parse(&later.to_string()).is_err());
        later["schema_version"] = serde_json::json!(1);
        later["state"] = serde_json::json!("lapsed");
        assert!(ClaimFile::parse(&later.to_string()).is_err());
    }

    /// Entry 137 of `docs/contract-divergences.md` states the sidecar, its words and the
    /// surface kind in one json block: each is held to the type and the constant it names.
    #[test]
    fn entry_137s_block_is_what_this_build_writes_and_reads() {
        let record = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/contract-divergences.md"),
        )
        .expect("the divergence record ships");
        let entry = record
            .split("\n## ")
            .find(|entry| entry.starts_with("137."))
            .expect("the record carries entry 137");
        let block: serde_json::Value = entry
            .split("```json")
            .nth(1)
            .and_then(|rest| rest.split("```").next())
            .and_then(|block| serde_json::from_str(block).ok())
            .expect("entry 137 carries a json block");
        assert_eq!(block["claim_file"], CLAIM_FILE);
        let example = ClaimFile::parse(&block["claim"].to_string()).expect("the example reads");
        assert_eq!(serde_json::to_value(&example).unwrap(), block["claim"]);
        let words = |of: Vec<serde_json::Value>| serde_json::Value::Array(of);
        assert_eq!(
            block["states"],
            words(
                [
                    ClaimState::Confirmed,
                    ClaimState::Unconfirmed,
                    ClaimState::Superseded
                ]
                .map(|state| serde_json::to_value(state).unwrap())
                .to_vec()
            )
        );
        assert_eq!(
            block["reasons"],
            words(
                [
                    UnconfirmedReason::Late,
                    UnconfirmedReason::Failed,
                    UnconfirmedReason::RateLimited
                ]
                .map(|reason| serde_json::json!(reason.as_str()))
                .to_vec()
            )
        );
        for reason in [
            UnconfirmedReason::Late,
            UnconfirmedReason::Failed,
            UnconfirmedReason::RateLimited,
        ] {
            assert_eq!(serde_json::to_value(reason).unwrap(), reason.as_str());
        }
        assert_eq!(block["surface_kind"], crate::engine::CLAIM_UNCONFIRMED);
        assert_eq!(
            block["watch_segment"],
            format!("CLAIM UNCONFIRMED {}", watch_age(720))
        );
    }

    #[test]
    fn a_watch_age_is_one_whole_unit_of_at_most_six_characters() {
        for (seconds, said) in [
            (0, "0 s"),
            (119, "119 s"),
            (120, "2 min"),
            (5_999, "99 min"),
            (6_000, "1 h"),
            (7_200, "2 h"),
            (10_740, "2 h"),
            (359_999, "99 h"),
            (360_000, "4 d"),
            (86_400 * 400, "400 d"),
            (u64::MAX, "9999 d"),
        ] {
            assert_eq!(watch_age(seconds), said, "{seconds}");
            assert!(said.len() <= 6, "{said}");
        }
    }

    #[test]
    fn a_span_is_read_in_the_unit_a_lease_is() {
        assert_eq!(span(0), "0 s");
        assert_eq!(span(119), "119 s");
        assert_eq!(span(120), "2 min");
        assert_eq!(span(720), "12 min");
        assert_eq!(span(7_199), "119 min");
        assert_eq!(span(7_200 + 300), "2 h 5 min");
    }

    #[test]
    fn unconfirmed_for_counts_from_the_last_confirming_read_or_else_the_episode() {
        let at = 1_760_000_000_000;
        let reading =
            ClaimReading::unconfirmed(Some(at), at + 10_000, UnconfirmedReason::Failed, "x".into());
        assert_eq!(reading.unconfirmed_for(at + 61_000), Some(61));
        let never = ClaimReading::unconfirmed(None, at, UnconfirmedReason::Failed, "x".into());
        assert_eq!(never.unconfirmed_for(at + 5_000), Some(5));
    }
}
