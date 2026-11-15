// llmlint: ignore-block[comments_earn_their_place] The normalizations and unrepresentable states below are constraints on this module's own types, so the header states them; deferring them to `docs/harness-usage.md` and pointing at it is what `no_redundant_instruction_pointers` forbids.
// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] Each payload's gate lives with it: codex's schema is snapshotted and diffed (`scripts/check-codex-usage-schema.sh`), Claude's is `claude_usage_drift`, and Copilot's undocumented endpoint degrades to `Unknown`. See `docs/harness-usage.md`.
//! Normalized subscription **headroom**: the shape of a `oneharness usage`
//! report and one parser per harness payload. Pure — every parser takes an
//! already-captured payload and returns normalized values, and the observation
//! timestamp is passed in, never read from a clock. The probes that spawn a
//! harness (Claude Code's `get_usage` control request, codex's app-server
//! `account/rateLimits/read`, Copilot's `/copilot_internal/user` GET) live
//! outside this module.
//!
//! The report is its own output contract with its own [`SCHEMA_VERSION`],
//! independent of the run report's and the history's.
//!
//! Three normalizations exist because the harnesses disagree on every
//! representational detail, and the disagreements are silent unless made
//! explicit:
//!
//! - **Percent polarity.** [`WindowUsage::Metered::used_percent`] is *always*
//!   percent-**used**. Claude's `utilization` and codex's `usedPercent` are
//!   already percent-used; Copilot's `percent_remaining` and codex's
//!   `individualLimit.remainingPercent` are percent-**remaining** and are
//!   converted when promoted.
//! - **Reset units.** [`UsageWindow::resets_at`] is *always* absolute RFC 3339
//!   UTC. Codex reports epoch seconds, Claude an ISO 8601 instant with a numeric
//!   offset, Copilot an already-UTC RFC 3339 string.
//! - **Window length.** Codex reports `windowDurationMins`; Claude reports
//!   nothing and the length must be derived from the key name — and *cannot* be
//!   for the codename keys the payload also carries. [`WindowDuration`] keeps
//!   that asymmetry in the data instead of papering over it with a default.
//!
//! Three states are deliberately unrepresentable rather than merely discouraged,
//! because rendering any of them as "0% used / plenty of headroom" is the one way
//! this report can be actively harmful:
//!
//! - An **unavailable** identity has no percentage at all: windows live only
//!   inside [`UsageAvailability::Available`], which cannot be empty.
//! - An **unlimited** quota has no counters at all: [`WindowUsage::Unlimited`]
//!   carries none, so Copilot's `unlimited: true` snapshots (which report
//!   `entitlement: 0` / `remaining: 0` / `percent_remaining: 100.0`, meaningless
//!   as counters) can never render as a full bar.
//! - A **negative** entitlement or consumption: [`QuotaAmount`] rejects one, so
//!   an unreadable counter drops the whole set rather than being clamped into a
//!   plausible figure. `remaining` stays signed — a real deficit past the
//!   ceiling is the one negative that means something.
//!
//! Two more are unrepresentable because a report claiming them while carrying
//! something else hands a consumer a confidently wrong answer: a timestamp field
//! is a [`UtcInstant`], so it cannot hold text that is not an RFC 3339 UTC
//! instant, and a stated window length is a [`NonZeroU64`], so a window can
//! never claim to be zero seconds long. Both hold at *every* boundary —
//! construction, parsing, and deserialization.
// llmlint: ignore-end[comments_earn_their_place]

use std::fmt;
use std::num::NonZeroU64;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use thiserror::Error;

use crate::domain::config::{valid_env_name, VariantName};
use crate::domain::harness;
use crate::domain::history::{civil_from_epoch, format_rfc3339};

/// Bumped when the usage report shape changes in a way a consumer must notice.
/// Independent of [`crate::domain::report::SCHEMA_VERSION`] and
/// [`crate::domain::history::SCHEMA_VERSION`] — this is its own contract.
pub const SCHEMA_VERSION: &str = "0.1";

/// One usage report: every probed identity, stamped with a single observation
/// time supplied by the caller (this module reads no clock).
///
/// Deserializing is a consumer boundary — `oneharness-core` is published for
/// sibling tools — so the envelope is validated on the way in rather than
/// trusted: see the private `UsageReportWire` shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(try_from = "UsageReportWire")]
pub struct UsageReport {
    /// The shape version, as a type with one value — see [`SchemaVersion`].
    pub schema_version: SchemaVersion,
    /// The instant the identities were observed, minted by the io layer's single
    /// clock read. A [`UtcInstant`], so the field cannot be *set* to something
    /// that is not one — see [`UsageReport::new`].
    pub observed_at: UtcInstant,
    pub identities: Vec<UsageIdentity>,
}

impl UsageReport {
    /// Assemble a report at `observed_at`. The type of the argument is the
    /// enforcement: a caller of the published crate reaches an instant only
    /// through [`UtcInstant`], which no non-UTC or unparseable text survives.
    #[must_use]
    pub fn new(observed_at: UtcInstant, identities: Vec<UsageIdentity>) -> Self {
        Self {
            schema_version: SchemaVersion,
            observed_at,
            identities,
        }
    }
}

/// The report's shape version, as a type: [`SCHEMA_VERSION`] is the only value
/// it has. A report claiming a version this build does not implement is
/// therefore not something a caller can build — and one arriving on the wire is
/// refused rather than stored (see the private `UsageReportWire` shape), so the
/// two boundaries agree. Serializes as the version string a consumer reads.
///
/// The schema is written by hand because the derive describes the *Rust* shape
/// — a unit struct, so `null` — while [`Serialize`] writes the version string.
/// An SDK validating a real report against the derived schema would reject
/// every one of them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SchemaVersion;

impl schemars::JsonSchema for SchemaVersion {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("SchemaVersion")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "const": SCHEMA_VERSION,
        })
    }
}

impl SchemaVersion {
    /// The version text, always [`SCHEMA_VERSION`].
    #[must_use]
    pub fn as_str(self) -> &'static str {
        SCHEMA_VERSION
    }
}

impl fmt::Display for SchemaVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(SCHEMA_VERSION)
    }
}

impl Serialize for SchemaVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(SCHEMA_VERSION)
    }
}

/// The unvalidated wire shape a [`UsageReport`] is deserialized *through*, so a
/// report inconsistent with the contract never becomes a `UsageReport` at all.
///
/// Two envelope fields are external input the rest of this module's boundary
/// checks would otherwise miss:
///
/// - A `schema_version` this build does not implement is refused rather than
///   reinterpreted as the version it does. Silently reading a future report as
///   v0.1 is the one failure this whole report exists to avoid: a consumer would
///   get a confident headroom figure out of a shape that no longer means what it
///   used to.
/// - An `observed_at` that is not a complete RFC 3339 **UTC** instant is
///   refused, and an equivalent UTC spelling (`+00:00`, a sub-second fraction)
///   is canonicalized — so the field a consumer reads always means what it
///   documents, exactly as [`UsageWindow::resets_at`] does.
#[derive(Deserialize, schemars::JsonSchema)]
struct UsageReportWire {
    schema_version: String,
    observed_at: String,
    identities: Vec<UsageIdentity>,
}

impl TryFrom<UsageReportWire> for UsageReport {
    type Error = String;

    fn try_from(wire: UsageReportWire) -> Result<Self, Self::Error> {
        if wire.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "unsupported usage report schema_version `{}`; this build reads {SCHEMA_VERSION}",
                without_control_chars(&wire.schema_version)
            ));
        }
        // The same validator public construction goes through, so a deserialized
        // report and a built one cannot disagree about what the field means. Only
        // the field's *name* is added here, which serde cannot supply itself.
        let observed_at = wire
            .observed_at
            .parse::<UtcInstant>()
            .map_err(|error| format!("observed_at {error}"))?;
        Ok(Self {
            schema_version: SchemaVersion,
            observed_at,
            identities: wire.identities,
        })
    }
}

/// The error returned when text is not a [`UtcInstant`]. Carries the offending
/// text, flattened like every other external string this module quotes back
/// (the private `without_control_chars` filter), so a bad value cannot smuggle
/// escapes into whatever prints the refusal.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("must be an RFC 3339 UTC instant (`2026-07-29T12:00:00Z`), got `{0}`")]
pub struct UtcInstantError(String);

/// An absolute RFC 3339 **UTC** instant, in [`format_rfc3339`]'s spelling.
///
/// Every timestamp this report carries is one, and the type is what makes the
/// documented meaning true at all three boundaries a plain `String` left open:
/// deserializing a consumer's JSON, parsing a harness payload, and *constructing*
/// a report — `oneharness-core` is published, so a sibling tool builds reports
/// this crate never parsed. A field that promises UTC while holding
/// `2026-07-29T08:00:00-04:00` is wrong by hours with nothing to signal it.
///
/// Two ways in, both canonicalizing:
///
/// - [`UtcInstant::from_epoch`] for an instant already computed as epoch seconds
///   (the io layer's clock read, codex's numeric `resetsAt`) — canonical by
///   construction.
/// - [`FromStr`] for text that must *already* be UTC, which is the rule for a
///   field documented as UTC: an equivalent spelling (`+00:00`, a sub-second
///   fraction) is accepted and normalized, any other offset is refused. A harness
///   payload legitimately arrives in another offset, and
///   [`normalize_timestamp`] is the converting entry point for exactly that.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct UtcInstant(String);

// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] Deliberately a
// described string rather than a pattern: this type appears only in *output*,
// which the SDKs validate leniently so a reader never rejects a report the CLI
// just emitted. A pattern here would make every SDK re-implement `FromStr`'s
// normalization to agree with it.
impl schemars::JsonSchema for UtcInstant {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "UtcInstant".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        // A canonical RFC 3339 UTC string. Described rather than pattern-matched
        // so every SDK validator accepts exactly what `FromStr` does, which is
        // the one rule they all have to agree on.
        schemars::json_schema!({
            "type": "string",
            "description": "An RFC 3339 timestamp in UTC (`Z`), in oneharness's canonical spelling.",
        })
    }
}

impl UtcInstant {
    /// The instant `secs` seconds after the UNIX epoch.
    #[must_use]
    pub fn from_epoch(secs: i64) -> Self {
        Self(format_rfc3339(secs))
    }

    /// The canonical RFC 3339 UTC text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for UtcInstant {
    type Err = UtcInstantError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let offset = text
            .get(19..)
            .unwrap_or_default()
            .trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
        if !matches!(
            offset,
            "Z" | "z" | "+00:00" | "-00:00" | "+0000" | "-0000" | "+00" | "-00"
        ) {
            return Err(UtcInstantError(without_control_chars(text)));
        }
        normalize_timestamp(text).ok_or_else(|| UtcInstantError(without_control_chars(text)))
    }
}

impl fmt::Display for UtcInstant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<'de> Deserialize<'de> for UtcInstant {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// One harness identity's headroom.
///
/// Deserializing is a consumer boundary like the envelope's, so every string an
/// identity carries is flattened through the private `without_control_chars`
/// filter on the way in — see the private `UsageIdentityWire` shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(try_from = "UsageIdentityWire")]
pub struct UsageIdentity {
    // llmlint: ignore[invalid_states_unrepresentable] `RunResult::harness` and `HistoryRunRecord::harness` are the established representation for a registry id on the wire, and matching them is what lets a consumer join a usage identity to a run; a newtype here would make this one contract spell it differently from every sibling.
    /// Canonical harness id, matching a [`crate::domain::harness`] registry id.
    pub harness: String,
    /// The named variant this identity came from, when it was selected by a
    /// composed id (`claude-code:work`). Absent for a bare harness id, matching
    /// [`crate::domain::report::RunResult::variant`] — so a consumer joins a
    /// usage identity to the runs it describes on the same pair of fields.
    ///
    /// Two subscriptions of one harness therefore stay distinguishable even when
    /// their [`IdentitySelector`]s do not distinguish them (a variant that
    /// selects an identity by credential rather than by directory).
    ///
    /// A [`VariantName`], so the field cannot be *set* to a name no config could
    /// have declared — the same enforcement [`UsageReport::observed_at`] gets
    /// from [`UtcInstant`]. It serializes transparently, so the wire keeps the
    /// plain string [`crate::domain::report::RunResult::variant`] carries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<VariantName>,
    /// How this identity was selected — never the credential itself.
    pub selector: IdentitySelector,
    pub auth_mode: AuthMode,
    /// The plan as the harness spells it, **verbatim**: Claude's `max` and
    /// codex's `pro` are different vocabularies and are never unified into one
    /// enum. Absent when the harness reports no plan (an API-key session).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    pub availability: UsageAvailability,
    /// The banked reset credits this identity holds, always serialized so its
    /// absence is never read as "none". A report written before the field
    /// existed reads it as `unknown` / `no_reader`.
    pub reset_credits: ResetCredits,
}

impl UsageIdentity {
    /// Attach a parsed payload to the identity it was probed for.
    ///
    /// A harness whose probe reads no reset credits reports
    /// [`ResetCreditsUnknown::NoReader`] whatever the payload's outcome — a
    /// failed claude probe has no credits reader to have failed, so `no_answer`
    /// would wrongly imply one exists.
    // llmlint: ignore[invalid_states_unrepresentable] This established infallible public constructor is source-compatible for sibling probes; oneharness calls it only with a resolved registry spec id, while untrusted reports enter through `UsageIdentityWire` and are registry-validated before exposure.
    #[must_use]
    pub fn new(harness: &str, selector: IdentitySelector, parsed: ParsedUsage) -> Self {
        Self {
            harness: harness.to_string(),
            variant: None,
            selector,
            auth_mode: parsed.auth_mode,
            plan: parsed.plan,
            availability: parsed.availability,
            reset_credits: if reads_reset_credits(harness) {
                parsed.reset_credits
            } else {
                NO_READER
            },
        }
    }

    /// Attribute this identity to the named variant that selected it.
    ///
    /// The type of the argument is the enforcement, as it is for
    /// [`UsageReport::new`]: a caller reaches a variant only through
    /// [`VariantName`], whose [`FromStr`] is the very validator
    /// the private `UsageIdentityWire` shape runs — so an attribution this
    /// crate would refuse to read back is not one a caller of the published
    /// crate can build. It stays infallible because the probe path that calls
    /// it is: a probe records an outcome for every identity, including a
    /// crashed one, and has nowhere to report a second kind of failure.
    #[must_use]
    pub fn with_variant(mut self, variant: Option<VariantName>) -> Self {
        self.variant = variant;
        self
    }
}

/// The unsanitized wire shape a [`UsageIdentity`] is deserialized *through*.
///
/// A probe's own strings are flattened where they are first bounded (see
/// [`without_control_chars`]), but a report arriving as JSON never passed those
/// parsers: `oneharness-core` is published, so a sibling tool reads reports it
/// did not produce. Flattening here — the one point an identity can enter from
/// outside — keeps the two paths from drifting, so `usage --format text` cannot
/// be made to move the cursor or recolour its output by a report that carries
/// terminal escapes in a harness id, a plan name, or a failure message.
///
/// The readable text is kept: each control character becomes a space, so a
/// reader still sees what the report said.
#[derive(Deserialize, schemars::JsonSchema)]
struct UsageIdentityWire {
    harness: String,
    #[serde(default)]
    variant: Option<String>,
    selector: IdentitySelector,
    auth_mode: AuthMode,
    #[serde(default)]
    plan: Option<String>,
    availability: UsageAvailability,
    #[serde(default)]
    reset_credits: ResetCredits,
}

impl TryFrom<UsageIdentityWire> for UsageIdentity {
    type Error = String;

    fn try_from(wire: UsageIdentityWire) -> Result<Self, Self::Error> {
        // The variant is validated *into* its field rather than beside it, so
        // this path and [`UsageIdentity::with_variant`] cannot come to disagree
        // about what an attribution may say: both reach the field only through
        // [`VariantName`]'s one validator. Flattening first keeps the refusal's
        // quoted-back name free of escapes, like every other string here.
        let variant = wire
            .variant
            .map(|name| {
                without_control_chars(&name)
                    .parse::<VariantName>()
                    .map_err(|error| format!("invalid usage identity variant: {error}"))
            })
            .transpose()?;
        let mut identity = Self {
            harness: wire.harness,
            variant,
            selector: wire.selector,
            auth_mode: wire.auth_mode,
            plan: wire.plan,
            availability: wire.availability,
            reset_credits: wire.reset_credits,
        };
        flatten_identity(&mut identity);
        if harness::by_id(&identity.harness).is_none() {
            return Err(format!(
                "unknown usage identity harness `{}`",
                identity.harness
            ));
        }
        let env = match &identity.selector {
            IdentitySelector::EnvPath { env, .. } | IdentitySelector::EnvSecret { env } => {
                Some(env)
            }
            IdentitySelector::Ambient => None,
        };
        if let Some(env) = env {
            if !valid_env_name(env) {
                return Err(format!("invalid usage identity environment name `{env}`"));
            }
        }
        Ok(identity)
    }
}

/// Flatten the control characters out of every string an identity carries,
/// in place — mutating rather than rebuilding keeps [`Windows`]'s non-emptiness
/// invariant intact by construction.
fn flatten_identity(identity: &mut UsageIdentity) {
    flatten(&mut identity.harness);
    // `variant` needs no flattening: a [`VariantName`] cannot hold a control
    // character, and the wire's text is flattened before it becomes one.
    flatten_optional(&mut identity.plan);
    match &mut identity.selector {
        IdentitySelector::EnvPath { env, path } => {
            flatten(env);
            flatten(path);
        }
        IdentitySelector::EnvSecret { env } => flatten(env),
        IdentitySelector::Ambient => {}
    }
    match &mut identity.availability {
        UsageAvailability::Available { windows } => {
            for window in &mut windows.0 {
                flatten(&mut window.id);
                flatten_optional(&mut window.label);
                // `resets_at` needs no flattening: a [`UtcInstant`] cannot hold a
                // control character in the first place, so it is refused at the
                // boundary rather than sanitized after the fact.
                flatten_optional(&mut window.scope);
            }
        }
        UsageAvailability::Unavailable { .. } => {}
        UsageAvailability::Unknown { reason } => match reason {
            UnknownReason::ProbeFailed { message } => flatten(message),
            UnknownReason::BinaryMissing { bin } => flatten(bin),
            UnknownReason::Unprobed => {}
        },
    }
    match &mut identity.reset_credits {
        ResetCredits::Reported { credits, .. } => {
            for credit in credits.iter_mut().flatten() {
                flatten_credit(credit);
            }
        }
        ResetCredits::Unknown {
            reason: ResetCreditsUnknown::Unreadable { message },
        } => flatten(message),
        ResetCredits::Unknown { .. } => {}
    }
}

/// Flatten one credit's strings; its instants cannot hold a control character.
fn flatten_credit(credit: &mut ResetCredit) {
    // `status` and `reset_type` need no flattening: classifying a word
    // flattens it, so neither can hold a control character.
    flatten(&mut credit.id);
    flatten_optional(&mut credit.title);
    flatten_optional(&mut credit.description);
}

/// Decided from the registry rather than the payload, so an unknown or
/// non-probing harness id falls to `no_reader` instead of `no_answer`.
fn reads_reset_credits(harness: &str) -> bool {
    harness::by_id(harness)
        .and_then(|spec| spec.usage.probe())
        .is_some_and(|probe| probe.reads_reset_credits())
}

fn flatten(text: &mut String) {
    *text = without_control_chars(text);
}

fn flatten_optional(text: &mut Option<String>) {
    if let Some(text) = text {
        flatten(text);
    }
}

/// How one identity is selected for a probe. A secret-valued selector records
/// only the variable's *name*, so a credential can never reach the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IdentitySelector {
    /// An environment variable naming a credential *directory* — Claude Code's
    /// `CLAUDE_CONFIG_DIR`, codex's `CODEX_HOME`. The path is not a secret.
    // llmlint: ignore[invalid_states_unrepresentable] These public enum fields preserve the v0.1 constructor and wire shape; external selectors are checked with the repository's `valid_env_name` at `UsageIdentityWire`, and production constructors use registry-declared environment names.
    EnvPath { env: String, path: String },
    /// An environment variable carrying a credential — Copilot's GitHub token.
    /// The value is deliberately absent.
    // llmlint: ignore[invalid_states_unrepresentable] Same established selector contract as `EnvPath`: deserialization validates through `valid_env_name`, while in-process probes supply the registry-declared credential variable names.
    EnvSecret { env: String },
    /// The harness's ambient credential store, with nothing overridden.
    Ambient,
}

impl IdentitySelector {
    /// A stable, human-readable key for this identity, safe to print. The path
    /// is an environment value, so control characters are flattened here — the
    /// one place every caller goes through — rather than by each renderer.
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            Self::EnvPath { env, path } => without_control_chars(&format!("{env}={path}")),
            Self::EnvSecret { env } => format!("{env}=<secret>"),
            Self::Ambient => "ambient".to_string(),
        }
    }
}

/// How the identity authenticates, which is what decides whether plan headroom
/// exists at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    /// A first-party subscription (Claude.ai, ChatGPT, a GitHub Copilot seat).
    Subscription,
    /// An API key. Both claude-code and codex affirmatively report no plan
    /// headroom in this mode — see [`UnavailableReason::ApiKeyAuth`].
    ApiKey,
    /// Not established by the probe.
    Unknown,
}

/// Tri-state headroom availability.
///
/// The three states are genuinely different answers and are never collapsed:
/// `available` carries real data, `unavailable` is an affirmative "this identity
/// has no plan headroom to report" **with a reason**, and `unknown` means only
/// that nothing was learned. There is no percentage reachable from either
/// non-available state, so neither can be rendered as `0%` used.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UsageAvailability {
    /// Real headroom data: one entry per non-null window, never empty.
    Available { windows: Windows },
    /// The harness answered and affirmatively reported no plan headroom.
    Unavailable { reason: UnavailableReason },
    /// Nothing was learned — the identity was not probed, or the probe failed.
    Unknown { reason: UnknownReason },
}

impl UsageAvailability {
    /// Available when at least one window was read, else an affirmative
    /// [`UnavailableReason::NoWindowsReported`]. Never an empty `available`,
    /// which a renderer could mistake for full headroom.
    #[must_use]
    pub fn from_windows(windows: Vec<UsageWindow>) -> Self {
        match Windows::new(windows) {
            Some(windows) => Self::Available { windows },
            None => Self::Unavailable {
                reason: UnavailableReason::NoWindowsReported,
            },
        }
    }

    /// The windows, or an empty slice when not available. This is the only way
    /// to reach a percentage, so an unavailable or unknown identity has none.
    #[must_use]
    pub fn windows(&self) -> &[UsageWindow] {
        match self {
            Self::Available { windows } => windows.as_slice(),
            Self::Unavailable { .. } | Self::Unknown { .. } => &[],
        }
    }
}

/// Why an identity affirmatively has no plan headroom. Distinct from
/// [`UnknownReason`]: these are answers, not absences of one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UnavailableReason {
    /// API-key auth. Verified for both claude-code (`rate_limits_available:
    /// false`, `subscription_type: null`) and codex (`chatgpt authentication
    /// required to read rate limits`): the harness exposes no plan headroom in
    /// this mode. This is a finding, never a guess — and it is *not*
    /// [`UsageAvailability::Unknown`].
    ApiKeyAuth,
    /// No stored credential for the selected identity — codex's `codex account
    /// authentication required to read rate limits`, a different code branch
    /// from the API-key one above.
    NotLoggedIn,
    /// The harness answered, but carried no readable window at all.
    NoWindowsReported,
    /// The harness has **no first-party plan quota** for headroom to exist in:
    /// OpenCode Zen is pay-as-you-go with nothing that resets, and Goose ships
    /// no first-party inference plan at all. Nothing is missing here — the
    /// quantity itself is undefined, which is why this is not
    /// [`Self::NoHeadroomReader`].
    NoPlanQuota,
    /// A plan quota exists but the harness exposes **no non-interactive reader**
    /// for it: Cursor's dollar pools reach only its interactive TUI, Crush's
    /// Hyper credits have no balance command or API, and Qwen's Coding Plan
    /// weekly quota has no reader of any kind. A future CLI release could add
    /// one, which is exactly what separates this from [`Self::NoPlanQuota`].
    NoHeadroomReader,
}

/// Why nothing is known. Reserved **strictly** for unprobed or probe-failed —
/// API-key auth is [`UnavailableReason::ApiKeyAuth`], not this.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UnknownReason {
    /// No probe was run for this identity.
    Unprobed,
    /// A probe ran and failed, carrying the harness's own message.
    ProbeFailed { message: String },
    /// The harness's binary is not installed, so its probe could not run. The
    /// harness may well have headroom; this identity simply has no reader on
    /// this machine. Mirrors a run's `skipped` status: data, never a crash.
    /// Build it with [`UnknownReason::binary_missing`].
    // llmlint: ignore[invalid_states_unrepresentable] The public enum variant is retained for source compatibility; production construction uses `binary_missing`, and `UsageIdentityWire` flattens this field before any deserialized report is exposed or rendered.
    BinaryMissing { bin: String },
}

impl UnknownReason {
    /// The reason for an identity whose harness binary is absent, from the
    /// resolved binary name.
    ///
    /// The name comes from `--bin` or a config file, and `usage --format text`
    /// prints it verbatim, so it is flattened here — at the one point it becomes
    /// a display string — for the same reason every other external string this
    /// module bounds is (see the private `without_control_chars` filter).
    #[must_use]
    pub fn binary_missing(bin: &str) -> Self {
        Self::BinaryMissing {
            bin: without_control_chars(bin),
        }
    }
}

/// The banked rate-limit **reset credits** an identity holds — capacity a window
/// at 100% used does not show. Harness-neutral: codex fills it from its
/// `rateLimitResetCredits` summary, and a harness with no reader says so.
///
/// Like [`UsageAvailability`], `unknown` means only that nothing was learned and
/// is never zero: "no credits" is exactly `reported` with an `available_count`
/// of 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ResetCredits {
    /// The harness answered with a credits summary.
    Reported {
        /// The harness's own count, and the authoritative total — never derived
        /// from `credits`, which the harness may cap below it.
        available_count: u64,
        /// The credits the harness listed. Absent when it reported only the
        /// count; present — possibly empty, possibly shorter than
        /// `available_count` — when it listed them.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        credits: Option<Vec<ResetCredit>>,
    },
    /// Nothing was learned about this identity's credits.
    Unknown { reason: ResetCreditsUnknown },
}

const NO_READER: ResetCredits = ResetCredits::Unknown {
    reason: ResetCreditsUnknown::NoReader,
};

const NO_ANSWER: ResetCredits = ResetCredits::Unknown {
    reason: ResetCreditsUnknown::NoAnswer,
};

impl Default for ResetCredits {
    /// What a report written before the field existed says: nothing was read.
    fn default() -> Self {
        NO_READER
    }
}

/// Why an identity's reset credits are unknown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResetCreditsUnknown {
    /// This oneharness reads no reset credits for this harness.
    NoReader,
    /// The probe produced no rate-limit answer to read credits from — the
    /// identity's `availability` says why.
    NoAnswer,
    /// The harness answered, but its answer carried no credits summary.
    NotReported,
    /// A summary was present but structurally malformed.
    Unreadable { message: String },
}

/// One listed reset credit. `status` and `reset_type` are the harness's own
/// words, **verbatim** like [`UsageIdentity::plan`], so a value this release
/// has never seen is reported as itself rather than refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ResetCredit {
    /// The harness's opaque identifier for this credit.
    pub id: String,
    /// The credit's status as the harness spells it (codex: `available`,
    /// `redeeming`, `redeemed`, `unknown`).
    pub status: ResetCreditStatus,
    /// What the credit resets, as the harness spells it (codex:
    /// `codexRateLimits`).
    pub reset_type: ResetCreditType,
    pub granted_at: UtcInstant,
    /// When the credit expires. Absent when the harness said it does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<UtcInstant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl ResetCredit {
    /// Whether this row is an available credit of a reset type `harness`'s
    /// probe recognizes ([`UsageProbe::recognized_reset_types`]) — the only
    /// rows a renderer may count as evidence about when the identity's credits
    /// expire. Any other row is still reported, but says nothing a reader can
    /// rely on about the identity's spendable credits.
    #[must_use]
    pub fn is_recognized_available(&self, harness: &str) -> bool {
        self.status == ResetCreditStatus::Available
            && harness::by_id(harness)
                .and_then(|spec| spec.usage.probe())
                .is_some_and(|probe| {
                    probe
                        .recognized_reset_types()
                        .contains(&self.reset_type.as_str())
                })
    }
}

/// A status word this release does not recognize, kept verbatim with its
/// control characters flattened. Built only by classifying a status, so it
/// never holds `available`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnrecognizedStatus(String);

impl UnrecognizedStatus {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The schema of a verbatim harness word: a plain string, inlined so the wire
/// shape stays exactly the one the field always had.
fn verbatim_word_schema() -> schemars::Schema {
    schemars::json_schema!({ "type": "string" })
}

/// A credit's status. This release recognizes one word, `available` — the only
/// status the expiry rule reads; any other word (codex's `redeeming`,
/// `redeemed` and `unknown` among them) is kept as itself in
/// [`ResetCreditStatus::Unrecognized`]. Serialized as the harness's verbatim
/// word.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum ResetCreditStatus {
    /// `available`: the credit can be spent.
    Available,
    Unrecognized(UnrecognizedStatus),
}

impl ResetCreditStatus {
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Available => "available",
            Self::Unrecognized(word) => word.as_str(),
        }
    }
}

impl From<String> for ResetCreditStatus {
    /// Flattens control characters, then classifies the word.
    fn from(word: String) -> Self {
        let word = without_control_chars(&word);
        match word.as_str() {
            "available" => Self::Available,
            _ => Self::Unrecognized(UnrecognizedStatus(word)),
        }
    }
}

impl From<ResetCreditStatus> for String {
    fn from(status: ResetCreditStatus) -> Self {
        match status {
            ResetCreditStatus::Available => "available".to_string(),
            ResetCreditStatus::Unrecognized(word) => word.0,
        }
    }
}

impl schemars::JsonSchema for ResetCreditStatus {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ResetCreditStatus".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        verbatim_word_schema()
    }
}

/// What a credit resets: the harness's own word, kept verbatim with its
/// control characters flattened. Which words a reader may rely on is the
/// harness's to say, so recognition lives on its probe
/// ([`UsageProbe::recognized_reset_types`]) rather than in this shared type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub struct ResetCreditType(String);

impl ResetCreditType {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for ResetCreditType {
    /// Flattens control characters.
    fn from(word: String) -> Self {
        Self(without_control_chars(&word))
    }
}

impl From<ResetCreditType> for String {
    fn from(reset_type: ResetCreditType) -> Self {
        reset_type.0
    }
}

impl schemars::JsonSchema for ResetCreditType {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ResetCreditType".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        verbatim_word_schema()
    }
}

/// A non-empty list of windows. The non-emptiness is the invariant that keeps
/// "available" from ever meaning "no data" — construct it with [`Windows::new`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(try_from = "Vec<UsageWindow>", into = "Vec<UsageWindow>")]
pub struct Windows(Vec<UsageWindow>);

impl Windows {
    /// `Some` iff `windows` is non-empty.
    #[must_use]
    pub fn new(windows: Vec<UsageWindow>) -> Option<Self> {
        (!windows.is_empty()).then_some(Self(windows))
    }

    #[must_use]
    pub fn as_slice(&self) -> &[UsageWindow] {
        &self.0
    }
}

impl TryFrom<Vec<UsageWindow>> for Windows {
    type Error = &'static str;

    fn try_from(windows: Vec<UsageWindow>) -> Result<Self, Self::Error> {
        Self::new(windows).ok_or("an available identity must carry at least one window")
    }
}

impl From<Windows> for Vec<UsageWindow> {
    fn from(windows: Windows) -> Self {
        windows.0
    }
}

/// One rate-limit window. Emitted only for a window the harness actually
/// reported: a null window (Claude's `seven_day_opus: null`, codex's `secondary:
/// null`) means "not applicable to this plan", **not** "0% used", so it is
/// omitted rather than zero-filled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct UsageWindow {
    /// The harness-native window identifier, verbatim where the harness names
    /// one (`five_hour`, `tangelo`, `chat`) and `<limitId>/<slot>` where codex's
    /// two-level buckets are flattened.
    pub id: String,
    /// A human label the harness supplied (codex's `limitName`), when it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub usage: WindowUsage,
    /// The window's length, paired with where that length came from.
    #[serde(flatten)]
    pub duration: WindowDuration,
    /// When the window resets, always absolute RFC 3339 UTC — a [`UtcInstant`],
    /// so no other text is representable here. Absent when the harness reported
    /// no reset, or one that could not be normalized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<UtcInstant>,
    /// The model display name this window is scoped to, when it is scoped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Whether this is the limit currently binding, when the harness says so
    /// (Claude's `limits[].is_active`). Absent when it does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_binding: Option<bool>,
}

/// What a window's consumption looks like. An unlimited quota carries no
/// counters, so it can never be rendered as a metered bar at 0% used.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WindowUsage {
    Metered {
        /// **Always percent-used**, whatever polarity the source used, and
        /// validated at the boundary — see [`UsedPercent`].
        used_percent: UsedPercent,
        /// The raw counters behind the percentage, when the harness reported
        /// every one of them. Never partially fabricated, and absent rather than
        /// null when the payload did not carry the full set.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        counters: Option<QuotaCounters>,
    },
    /// The harness reported this quota as unlimited. No counter is read: an
    /// unlimited Copilot snapshot reports `entitlement: 0` / `remaining: 0`
    /// alongside `percent_remaining: 100.0`, which are meaningless as counters.
    Unlimited,
}

/// A validated percent-**used** figure, whatever polarity the harness reported.
///
/// Validated at the boundary rather than trusted: a harness's payload is
/// external input, and a `NaN`, an infinity, or a negative percentage would
/// render as a nonsense bar. A value *above* 100 is accepted and preserved
/// rather than clamped — it means the harness reported consumption past its own
/// ceiling (an overage), which is precisely when a consumer needs the figure.
///
/// `schemars` describes the newtype's private field rather than the `f64` the
/// serde attributes make it travel as, so the derived schema constrained
/// nothing at all — `with` plus the boundary's own bound is what makes the
/// generated validators check what [`UsedPercent::new`] checks. (JSON has no
/// `NaN` or infinity literal, so "finite" needs nothing said.)
#[derive(
    Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(try_from = "f64", into = "f64")]
#[schemars(with = "f64", extend("minimum" = 0.0))]
pub struct UsedPercent(f64);

impl UsedPercent {
    /// `Some` iff `value` is finite and non-negative.
    #[must_use]
    pub fn new(value: f64) -> Option<Self> {
        (value.is_finite() && value >= 0.0).then_some(Self(value))
    }

    #[must_use]
    pub fn get(self) -> f64 {
        self.0
    }
}

impl TryFrom<f64> for UsedPercent {
    type Error = &'static str;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value).ok_or("a used percentage must be finite and non-negative")
    }
}

impl From<UsedPercent> for f64 {
    fn from(percent: UsedPercent) -> Self {
        percent.0
    }
}

/// A counter that cannot legitimately be negative. An entitlement is a ceiling
/// and a consumption is an amount spent; neither has a meaning below zero, so a
/// negative one is a payload that failed to parse rather than an account state,
/// and it is rejected at the boundary like [`UsedPercent`].
///
/// Deliberately *not* used for [`QuotaCounters::remaining`], which is genuinely
/// negative for an account past its ceiling — the deficit is the signal there,
/// so constraining it would discard a real over-consumption reading.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(try_from = "i64", into = "u64")]
pub struct QuotaAmount(u64);

impl QuotaAmount {
    /// `Some` iff `value` is non-negative.
    #[must_use]
    pub fn new(value: i64) -> Option<Self> {
        u64::try_from(value).ok().map(Self)
    }

    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

impl TryFrom<i64> for QuotaAmount {
    type Error = &'static str;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        Self::new(value).ok_or("a quota counter must not be negative")
    }
}

impl From<QuotaAmount> for u64 {
    fn from(amount: QuotaAmount) -> Self {
        amount.0
    }
}

/// The raw counters behind a metered window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct QuotaCounters {
    pub entitlement: QuotaAmount,
    pub used: QuotaAmount,
    /// The server's own remaining figure, taken as authoritative rather than
    /// recomputed from `entitlement - used`: Copilot's observed values disagree
    /// by about 1 and the server's figure wins. Signed, and the one counter here
    /// that is: an account past its ceiling reports a real deficit.
    pub remaining: i64,
    /// Whether any quota remains on this plan.
    pub has_quota: bool,
    /// Whether spending past the entitlement is permitted. `has_quota: false`
    /// *and* `overage_permitted: false` together are the machine-readable
    /// "exhausted and blocked" signal — see [`QuotaCounters::blocked`].
    pub overage_permitted: bool,
    pub unit: QuotaUnit,
}

impl QuotaCounters {
    /// Exhausted **and** blocked from spending further: the joint-false state of
    /// `has_quota` and `overage_permitted`.
    #[must_use]
    pub fn blocked(&self) -> bool {
        !self.has_quota && !self.overage_permitted
    }
}

/// What the counters count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QuotaUnit {
    /// GitHub AI credits, $0.01 each — Copilot's unit when `token_based_billing`
    /// is true.
    AiCredits,
    /// The harness did not say what its counters count.
    Unspecified,
}

/// A window's length together with the provenance of that length. The
/// discriminator is the point: codex *reports* a duration, Claude's must be
/// *inferred* from the key name, and for Claude's codename keys it simply
/// cannot be derived. Serializes flat as `window_seconds_source` plus (except
/// when unknown) `window_seconds`.
///
/// A stated length is a [`NonZeroU64`]: a window zero seconds long is not a
/// window, so "no length" is [`Self::Unknown`] — the state a consumer already
/// handles — rather than a `window_seconds` of 0 that a renderer would divide by
/// or display as an instantaneous quota.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "window_seconds_source", rename_all = "snake_case")]
pub enum WindowDuration {
    /// The harness stated the length (codex's `windowDurationMins`).
    Reported { window_seconds: NonZeroU64 },
    /// Derived from the harness's own window key (Claude's `five_hour`).
    InferredFromId { window_seconds: NonZeroU64 },
    /// Not derivable — an opaque key, or a calendar window with no fixed length.
    Unknown,
}

impl WindowDuration {
    /// A length the harness stated, in seconds. `Unknown` for a non-positive
    /// one, which is not a length.
    #[must_use]
    pub fn reported(window_seconds: u64) -> Self {
        NonZeroU64::new(window_seconds).map_or(Self::Unknown, |window_seconds| Self::Reported {
            window_seconds,
        })
    }

    /// A length derived from the harness's own window key, in seconds. `Unknown`
    /// for a non-positive one, exactly as [`Self::reported`].
    #[must_use]
    pub fn inferred_from_id(window_seconds: u64) -> Self {
        NonZeroU64::new(window_seconds).map_or(Self::Unknown, |window_seconds| {
            Self::InferredFromId { window_seconds }
        })
    }

    /// The length in seconds, or `None` when it could not be established.
    #[must_use]
    pub fn seconds(&self) -> Option<u64> {
        match *self {
            Self::Reported { window_seconds } | Self::InferredFromId { window_seconds } => {
                Some(window_seconds.get())
            }
            Self::Unknown => None,
        }
    }

    /// A duration the harness reported, in minutes (codex). A missing or
    /// negative value stays [`WindowDuration::Unknown`] rather than becoming 0.
    fn from_reported_minutes(minutes: Option<i64>) -> Self {
        match minutes {
            Some(minutes) if minutes > 0 => Self::reported((minutes as u64).saturating_mul(60)),
            _ => Self::Unknown,
        }
    }
}

/// One parsed payload: everything a probe learns about a single identity, ready
/// to be attached to it with [`UsageIdentity::new`].
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedUsage {
    pub auth_mode: AuthMode,
    pub plan: Option<String>,
    pub availability: UsageAvailability,
    pub reset_credits: ResetCredits,
}

impl ParsedUsage {
    /// An identity nothing is known about — no probe ran, or one failed — so
    /// there was no answer to read credits from either.
    #[must_use]
    pub fn unknown(reason: UnknownReason) -> Self {
        Self {
            auth_mode: AuthMode::Unknown,
            plan: None,
            availability: UsageAvailability::Unknown { reason },
            reset_credits: NO_ANSWER,
        }
    }
}

/// What `oneharness usage` can learn about a harness. Registry data
/// ([`crate::domain::harness::HarnessSpec::usage`]) sourced from
/// `docs/harness-usage.md`, never guessed.
///
/// The two non-probing variants are the point of the enum: five of the eight
/// harnesses cannot report headroom, and *which kind* of cannot they are is a
/// real distinction — one has no quota, the other has no reader. Collapsing them
/// (or omitting those harnesses) would make `oneharness usage` quietly cover
/// three of eight while claiming to cover the fleet.
///
/// *How much* a probed harness reports is a property of the probe
/// ([`UsageProbe::reports`]), not a separate choice made here: pairing a tier
/// with a probe would let the registry claim headroom from a probe that reads
/// only a plan name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageSupport {
    /// Read by a zero-turn probe, which decides how much it can report.
    Probed(UsageProbe),
    /// No first-party plan quota exists to report ([`UnavailableReason::NoPlanQuota`]).
    NoPlanQuota,
    /// A plan quota exists with no non-interactive reader
    /// ([`UnavailableReason::NoHeadroomReader`]).
    NoHeadroomReader,
}

impl UsageSupport {
    /// The probe to run for this harness, or `None` when nothing is readable.
    #[must_use]
    pub fn probe(&self) -> Option<UsageProbe> {
        match *self {
            Self::Probed(probe) => Some(probe),
            Self::NoPlanQuota | Self::NoHeadroomReader => None,
        }
    }

    /// The affirmative "no headroom to report" verdict for a non-probing
    /// harness, or `None` when this harness is probed instead.
    #[must_use]
    pub fn unprobed_reason(&self) -> Option<UnavailableReason> {
        match *self {
            Self::NoPlanQuota => Some(UnavailableReason::NoPlanQuota),
            Self::NoHeadroomReader => Some(UnavailableReason::NoHeadroomReader),
            Self::Probed(_) => None,
        }
    }
}

/// How much a probe can report — a property of what the harness exposes, so it
/// cannot disagree with the probe that reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageReporting {
    /// Real remaining-headroom windows.
    Headroom,
    /// The plan tier only; the harness publishes no non-interactive headroom
    /// reader, so a successful read is still an affirmative
    /// [`UnavailableReason::NoHeadroomReader`].
    PlanTier,
}

/// One zero-turn probe. Every variant is chosen because it completes **without
/// the harness taking a model turn** — no user message is sent and no turn is
/// completed — which is what makes `oneharness usage` usable as a pre-flight
/// check rather than a thing that costs what it measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageProbe {
    /// Claude Code driven in stream-json input/output mode with an empty tool
    /// set, sent exactly one `get_usage` control request and read for the
    /// matching control response. No user message is ever written, which is what
    /// keeps it free (observed: `num_turns: 0`, `total_cost_usd: 0`).
    ClaudeGetUsage,
    /// `codex app-server --stdio`, driven `initialize` → `initialized` →
    /// `account/rateLimits/read` over JSON-RPC.
    CodexAppServer,
    /// An authenticated `GET /copilot_internal/user` against the GitHub API —
    /// out of band from the Copilot CLI entirely, so it needs no Copilot binary
    /// and answers before a run rather than after a turn is spent. The
    /// run-embedded JSONL quota surface is deliberately not used: oneharness
    /// drives Copilot in text mode, so it is unreachable as wired.
    CopilotUserEndpoint,
    /// `cursor-agent about --format json`, read for `subscriptionTier` only, and
    /// only from a **pre-existing** login. Cursor's `--api-key`/`CURSOR_API_KEY`
    /// path is not a per-process selector — it performs a token exchange and
    /// persists credentials to the shared store, observed clobbering a real user
    /// login — so the probe must never take it.
    CursorAbout,
}

impl UsageProbe {
    /// Whether this probe spawns the harness's own binary (and so needs it
    /// installed). Copilot's is an out-of-band HTTP GET whose entire credential
    /// requirement is a GitHub token, so it answers with no Copilot CLI present.
    #[must_use]
    pub fn spawns_harness(&self) -> bool {
        !matches!(self, Self::CopilotUserEndpoint)
    }

    /// Whether this probe reads the identity's [`ResetCredits`]. Only codex's
    /// answer carries them today.
    #[must_use]
    pub fn reads_reset_credits(&self) -> bool {
        matches!(self, Self::CodexAppServer)
    }

    /// The reset types this probe's harness names that this release
    /// recognizes — the only credits a renderer may count as evidence about
    /// when the identity's credits expire.
    #[must_use]
    pub fn recognized_reset_types(&self) -> &'static [&'static str] {
        match self {
            Self::CodexAppServer => &["codexRateLimits"],
            Self::ClaudeGetUsage | Self::CopilotUserEndpoint | Self::CursorAbout => &[],
        }
    }

    /// How much this probe can report. Cursor's is the lone
    /// [`UsageReporting::PlanTier`]: its dollar pools reach only the interactive
    /// TUI, so `about` yields a plan name and nothing more.
    #[must_use]
    pub fn reports(&self) -> UsageReporting {
        match self {
            Self::ClaudeGetUsage | Self::CodexAppServer | Self::CopilotUserEndpoint => {
                UsageReporting::Headroom
            }
            Self::CursorAbout => UsageReporting::PlanTier,
        }
    }
}

/// Claude's window lengths, derived from its own key names. The key set is
/// **open** — the observed payload also carries codenames (`tangelo`,
/// `iguana_necktie`, `nimbus_quill`, `cinder_cove`, `amber_ladder`) with no
/// derivable duration — so a key absent from this table becomes
/// [`WindowDuration::Unknown`], never a guessed length. Extend the table only
/// from a key whose window length is actually known.
// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] Claude Code publishes no schema for these key names, so there is nothing to generate from; the table is drift-*safe* instead — an unknown key degrades to `Unknown`, never a wrong duration.
const CLAUDE_WINDOW_SECONDS: &[(&str, u64)] = &[
    ("five_hour", 5 * 3_600),
    ("seven_day", 7 * 86_400),
    ("seven_day_cowork", 7 * 86_400),
    ("seven_day_oauth_apps", 7 * 86_400),
    ("seven_day_omelette", 7 * 86_400),
    ("seven_day_opus", 7 * 86_400),
    ("seven_day_overage_included", 7 * 86_400),
    ("seven_day_sonnet", 7 * 86_400),
];

/// Keys inside `rate_limits` that are *not* plan windows. `extra_usage` is
/// excluded by name because it carries a `utilization` field of its own (a
/// monthly credit axis, not a plan window) and would otherwise be mistaken for
/// one.
const CLAUDE_NON_WINDOW_KEYS: &[&str] = &[
    "extra_usage",
    "limits",
    "member_dashboard_available",
    "model_scoped",
    "spend",
];

/// Which named window each `limits[].kind` describes, so the array's `is_active`
/// flag can be attached to the window it belongs to. `weekly_scoped` maps to no
/// named key — it is emitted as its own window (see [`parse_claude_get_usage`]).
const CLAUDE_LIMIT_KIND_KEYS: &[(&str, &str)] =
    &[("session", "five_hour"), ("weekly_all", "seven_day")];

/// The seconds in Claude's `weekly_scoped` window: its `group` is literally
/// `weekly`, which is where the length comes from.
const CLAUDE_WEEKLY_SECONDS: u64 = 7 * 86_400;

/// Unwrap the `get_usage` payload from one `control_response` line, so a caller
/// reading Claude's JSONL stream does not have to know the envelope shape.
/// `None` for any other line.
#[must_use]
pub fn claude_control_response(line: &Value) -> Option<&Value> {
    if line.get("type").and_then(Value::as_str) != Some("control_response") {
        return None;
    }
    let response = line.get("response")?;
    if response.get("subtype").and_then(Value::as_str) != Some("success") {
        return None;
    }
    response.get("response")
}

/// The contract-drift guard for Claude's `get_usage` payload: `Some(reason)`
/// when the payload no longer looks like the shape [`parse_claude_get_usage`]
/// was written against. Private on purpose — [`parse_claude_get_usage`] runs it
/// itself, so no caller can obtain a confident answer without it.
///
/// This exists because Claude's structured usage surface is explicitly
/// experimental — the SDK method is literally named
/// `usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET()` — and, unlike
/// codex's app-server, it publishes no schema to snapshot and diff. The failure
/// mode to prevent is a renamed field silently becoming a confident verdict, so
/// every check here targets a branch whose *absent* input would otherwise read
/// as an affirmative negative:
///
/// - `subscription_type` is the auth-mode discriminator, and the parser reads
///   its absence as API-key auth — i.e. affirmatively "no plan headroom". A
///   rename or a wrong type must not inherit that answer, so the key must be
///   present and be either the plan string or null.
/// - `rate_limits_available` is the branch the parser takes on absence
///   (`unwrap_or(false)`), so its disappearance would read as "no headroom
///   reported" for every user at once.
/// - Under `rate_limits_available: true` the payload must still carry a
///   recognizable window surface: the `limits[]` array with at least one
///   expected [`CLAUDE_LIMIT_KIND_KEYS`]-style `kind`, or a named window key
///   with a numeric `utilization`. Neither present means the shape moved.
///
/// A *new* key or a new `kind` alongside a recognized one is not drift: the key
/// set is open by contract and unknown keys already degrade to an opaque window.
/// Nor is the transient no-snapshot answer ([`claude_usage_snapshot_missing`]):
/// every field it carries is the contracted one, and calling it drift sent a
/// reader chasing a parser break that did not exist.
fn claude_usage_drift(payload: &Value) -> Option<String> {
    match payload.get("subscription_type") {
        None => return Some("the payload carries no `subscription_type` field".to_string()),
        Some(plan) if !(plan.is_string() || plan.is_null()) => {
            return Some(
                "`subscription_type` is neither a plan string nor null (the API-key value)"
                    .to_string(),
            )
        }
        Some(_) => {}
    }

    let Some(available) = payload.get("rate_limits_available") else {
        return Some("the payload carries no `rate_limits_available` field".to_string());
    };
    let Some(available) = available.as_bool() else {
        return Some("`rate_limits_available` is not a boolean".to_string());
    };
    if !available {
        // An affirmative "no plan headroom" — the API-key answer, and the one
        // shape that needs no window surface at all.
        return None;
    }
    if claude_usage_snapshot_missing(payload) {
        // Nothing was renamed: the CLI is saying it could not fetch the
        // snapshot right now, which is a transient the parser names as one.
        return None;
    }
    let Some(rate_limits) = payload.get("rate_limits").filter(|value| value.is_object()) else {
        return Some(
            "`rate_limits_available` is true but `rate_limits` is not an object".to_string(),
        );
    };

    let known_kind = rate_limits
        .get("limits")
        .and_then(Value::as_array)
        .is_some_and(|entries| {
            entries.iter().any(|entry| {
                entry
                    .get("kind")
                    .and_then(Value::as_str)
                    .is_some_and(is_known_claude_limit_kind)
            })
        });
    let metered_window = rate_limits
        .as_object()
        .into_iter()
        .flatten()
        .any(|(key, value)| {
            !CLAUDE_NON_WINDOW_KEYS.contains(&key.as_str())
                && value.get("utilization").and_then(Value::as_f64).is_some()
        });
    if known_kind || metered_window {
        return None;
    }
    Some(format!(
        "`rate_limits_available` is true but no window surface was recognized: \
         `limits[]` carries none of the expected kinds ({}) and no window key carries a \
         numeric `utilization`",
        CLAUDE_LIMIT_KINDS.join(", ")
    ))
}

/// Whether a Claude `get_usage` payload is the **transient** no-snapshot answer:
/// a subscription account with windows (`subscription_type` a plan string,
/// `rate_limits_available` true) whose snapshot claude-code could not fetch
/// right now (`rate_limits` JSON `null`).
///
/// Observed against claude-code 2.1.273 on an identity whose OAuth token was
/// expired or refreshing and which had no cached snapshot; forty minutes later
/// the same identity answered normally, and three sibling identities answered
/// normally throughout on the same binary. So it is not drift — nothing moved —
/// and the private `claude_usage_drift` guard lets it through, while
/// [`parse_claude_get_usage`] reads it as [`UnknownReason::ProbeFailed`] with
/// [`CLAUDE_SNAPSHOT_MISSING_MESSAGE`]. It is public because the probe decides
/// whether to ask again from this predicate, never from that message. Any other
/// non-object `rate_limits` (an array, a string, a number, a boolean) is still
/// drift, and a null one under `rate_limits_available: false` keeps its
/// affirmative reading.
#[must_use]
pub fn claude_usage_snapshot_missing(payload: &Value) -> bool {
    payload
        .get("subscription_type")
        .is_some_and(Value::is_string)
        && payload
            .get("rate_limits_available")
            .and_then(Value::as_bool)
            == Some(true)
        && payload.get("rate_limits").is_some_and(Value::is_null)
}

/// The reason a payload [`claude_usage_snapshot_missing`] accepts is reported
/// under: what the payload carried, the likely causes, and that the probe is
/// worth repeating. A reader chasing a parser break here would be chasing a
/// release that did not happen; the answer is to ask again in a minute.
pub const CLAUDE_SNAPSHOT_MISSING_MESSAGE: &str = "claude-code returned no rate-limit snapshot \
    (`rate_limits_available` is true but `rate_limits` is null): the credential is likely \
    expired or refreshing, or the usage fetch failed; the probe is worth repeating";

/// Every `limits[].kind` the observed payload carries. `weekly_scoped` maps to
/// no named window key (see [`CLAUDE_LIMIT_KIND_KEYS`]) but is still an expected
/// member, so the drift guard recognizes it.
// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] These kinds ARE the drift gate: `get_usage` has no published schema, so the guard asserts on the observed kinds and degrades to `Unknown` — never to zero.
const CLAUDE_LIMIT_KINDS: &[&str] = &["session", "weekly_all", "weekly_scoped"];

fn is_known_claude_limit_kind(kind: &str) -> bool {
    CLAUDE_LIMIT_KINDS.contains(&kind)
}

/// Parse Claude Code's `get_usage` payload (the inner `response` object — see
/// [`claude_control_response`]).
///
/// `subscription_type` is the plan, verbatim, and its absence is the auth-mode
/// discriminator: the CLI documents it as null "for API key / 3P provider
/// sessions". `rate_limits_available` is the boolean to branch on, and under
/// API-key auth it is affirmatively `false` — an
/// [`UnavailableReason::ApiKeyAuth`], never an unknown.
///
/// Windows come from the open `rate_limits` key set (one per non-null key), with
/// `limits[].is_active` attached to the window each entry describes and each
/// `weekly_scoped` entry emitted as its own model-scoped window (it has no named
/// key of its own).
///
/// Every affirmative state below rests on a field's absence, so the payload is
/// checked for contract drift first (the private `claude_usage_drift` guard)
/// and a drifted one degrades to [`UsageAvailability::Unknown`]. That check is
/// not a step a caller can skip: there is no unguarded way in. The one
/// non-object `rate_limits` that is not drift is the transient no-snapshot
/// answer ([`claude_usage_snapshot_missing`]), which degrades to the same state
/// under a message that says what the payload means rather than that its shape
/// moved.
#[must_use]
pub fn parse_claude_get_usage(payload: &Value) -> ParsedUsage {
    if let Some(reason) = claude_usage_drift(payload) {
        return ParsedUsage::unknown(UnknownReason::ProbeFailed {
            message: format!("claude-code's `get_usage` payload changed shape: {reason}"),
        });
    }
    if claude_usage_snapshot_missing(payload) {
        return ParsedUsage::unknown(UnknownReason::ProbeFailed {
            message: CLAUDE_SNAPSHOT_MISSING_MESSAGE.to_string(),
        });
    }

    let plan = payload
        .get("subscription_type")
        .and_then(Value::as_str)
        .map(without_control_chars);
    // Documented by the CLI's own schema text: null for API-key / third-party
    // provider sessions, non-null for a Claude.ai subscription.
    let auth_mode = if plan.is_some() {
        AuthMode::Subscription
    } else {
        AuthMode::ApiKey
    };

    let available = payload
        .get("rate_limits_available")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let rate_limits = payload.get("rate_limits").filter(|value| value.is_object());

    let availability = match (available, rate_limits) {
        (true, Some(rate_limits)) => UsageAvailability::from_windows(claude_windows(rate_limits)),
        _ => UsageAvailability::Unavailable {
            reason: match auth_mode {
                AuthMode::ApiKey => UnavailableReason::ApiKeyAuth,
                _ => UnavailableReason::NoWindowsReported,
            },
        },
    };

    ParsedUsage {
        auth_mode,
        plan,
        availability,
        reset_credits: NO_READER,
    }
}

fn claude_windows(rate_limits: &Value) -> Vec<UsageWindow> {
    let limits = claude_limits(rate_limits);
    let mut windows: Vec<UsageWindow> = Vec::new();

    for (key, value) in rate_limits.as_object().into_iter().flatten() {
        if CLAUDE_NON_WINDOW_KEYS.contains(&key.as_str()) {
            continue;
        }
        // A null key means "not applicable to this plan" — omitted, never
        // zero-filled. Anything without a usable `utilization` is not a window.
        let Some(used_percent) = value
            .get("utilization")
            .and_then(Value::as_f64)
            .and_then(UsedPercent::new)
        else {
            continue;
        };
        let duration = CLAUDE_WINDOW_SECONDS
            .iter()
            .find(|(name, _)| *name == key)
            .map_or(WindowDuration::Unknown, |(_, seconds)| {
                WindowDuration::inferred_from_id(*seconds)
            });
        let is_binding = CLAUDE_LIMIT_KIND_KEYS
            .iter()
            .find(|(_, named)| *named == key)
            .and_then(|(kind, _)| limits.iter().find(|limit| limit.kind == *kind))
            .and_then(|limit| limit.is_active);

        windows.push(UsageWindow {
            id: without_control_chars(key),
            label: None,
            usage: WindowUsage::Metered {
                used_percent,
                counters: None,
            },
            duration,
            resets_at: value
                .get("resets_at")
                .and_then(Value::as_str)
                .and_then(normalize_timestamp),
            scope: None,
            is_binding,
        });
    }

    // Model-scoped weekly limits have no named `rate_limits` key of their own,
    // so the flat array is the only place they appear.
    for limit in limits.iter().filter(|limit| limit.kind == "weekly_scoped") {
        let Some(used_percent) = limit.percent.and_then(UsedPercent::new) else {
            continue;
        };
        let id = match &limit.scope {
            Some(scope) => format!("weekly_scoped/{scope}"),
            None => "weekly_scoped".to_string(),
        };
        windows.push(UsageWindow {
            id,
            label: limit.scope.clone(),
            usage: WindowUsage::Metered {
                used_percent,
                counters: None,
            },
            duration: WindowDuration::inferred_from_id(CLAUDE_WEEKLY_SECONDS),
            resets_at: limit.resets_at.as_deref().and_then(normalize_timestamp),
            scope: limit.scope.clone(),
            is_binding: limit.is_active,
        });
    }

    windows
}

/// One entry of Claude's flat `limits[]` array.
struct ClaudeLimit {
    kind: String,
    percent: Option<f64>,
    resets_at: Option<String>,
    scope: Option<String>,
    is_active: Option<bool>,
}

fn claude_limits(rate_limits: &Value) -> Vec<ClaudeLimit> {
    rate_limits
        .get("limits")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    Some(ClaudeLimit {
                        kind: entry.get("kind").and_then(Value::as_str)?.to_string(),
                        percent: entry.get("percent").and_then(Value::as_f64),
                        resets_at: entry
                            .get("resets_at")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        scope: entry
                            .pointer("/scope/model/display_name")
                            .and_then(Value::as_str)
                            .map(without_control_chars),
                        is_active: entry.get("is_active").and_then(Value::as_bool),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// codex's API-key branch: ChatGPT auth is required to read rate limits, so an
/// API-key session affirmatively has no plan headroom.
// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] A literal in the codex binary that appears in no emitted schema, so there is nothing to generate from; drift degrades safely — an unrecognized message becomes a probe failure, never an assumed absence of headroom.
const CODEX_API_KEY_ERROR: &str = "chatgpt authentication required to read rate limits";
/// codex's no-stored-credential branch. A genuinely separate code path from
/// [`CODEX_API_KEY_ERROR`] (both strings exist once each in the codex binary),
/// so collapsing the two to "error" would throw away a real distinction.
// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] Same as [`CODEX_API_KEY_ERROR`]: an unschematized binary literal, degrading safely to a probe failure rather than an assumed absence of headroom.
const CODEX_NOT_LOGGED_IN_ERROR: &str = "codex account authentication required to read rate limits";

/// Parse codex's app-server `account/rateLimits/read` response — the whole
/// JSON-RPC response object, either `{"result": …}` or `{"error": …}`.
///
/// Buckets come from `rateLimitsByLimitId`, keyed by `limitId`, in preference to
/// the top-level `rateLimits`, which the generated schema documents as a
/// "backward-compatible single-bucket view" mirroring one of them. Each bucket's
/// `primary` and optional `secondary` windows flatten to `<limitId>/<slot>` ids
/// that preserve both levels without inventing a matching hierarchy on the
/// Claude side.
#[must_use]
pub fn parse_codex_rate_limits(response: &Value) -> ParsedUsage {
    if let Some(message) = response
        .pointer("/error/message")
        .and_then(Value::as_str)
        .map(str::trim)
    {
        return codex_error(message);
    }

    // The result must be an *object* before anything is concluded from it: a
    // success verdict reached from a payload with no readable shape would be an
    // affirmative "subscription, no windows" built on nothing.
    let Some(result) = response.get("result").filter(|value| value.is_object()) else {
        return ParsedUsage::unknown(UnknownReason::ProbeFailed {
            message: "response carried neither a result object nor an error".to_string(),
        });
    };

    let by_limit_id = result
        .get("rateLimitsByLimitId")
        .and_then(Value::as_object)
        .filter(|buckets| !buckets.is_empty());
    let mirror = result.get("rateLimits").filter(|value| value.is_object());

    if let Some((limit_id, _)) =
        by_limit_id.and_then(|buckets| buckets.iter().find(|(_, bucket)| !bucket.is_object()))
    {
        return ParsedUsage::unknown(UnknownReason::ProbeFailed {
            message: format!(
                "`rateLimitsByLimitId.{}` is not a rate-limit snapshot object",
                without_control_chars(limit_id)
            ),
        });
    }

    // One of those two keys is the whole rate-limit surface, and the generated
    // schema makes `rateLimits` required — so a result carrying neither is drift
    // (a rename, a stripped payload), never an account with nothing to report.
    // Concluding `Subscription` here would publish an affirmative claim about
    // someone's plan derived from a payload that never mentioned one; the schema
    // diff in `scripts/check-codex-usage-schema.sh` guards the *generated*
    // contract at build time and cannot see what a live app-server actually sent.
    if by_limit_id.is_none() && mirror.is_none() {
        return ParsedUsage::unknown(UnknownReason::ProbeFailed {
            message: "the codex rate-limit result carries neither a `rateLimits` \
                      object nor a populated `rateLimitsByLimitId`"
                .to_string(),
        });
    }

    let mut windows = Vec::new();
    let mut plan = None;
    match by_limit_id {
        Some(buckets) => {
            for (limit_id, bucket) in buckets {
                let id = bucket
                    .get("limitId")
                    .and_then(Value::as_str)
                    .unwrap_or(limit_id.as_str());
                windows.extend(codex_bucket_windows(Some(id), bucket));
                plan = plan.or_else(|| codex_plan(bucket));
            }
            // The mirror is the authoritative plan when the buckets omit one.
            plan = mirror.and_then(codex_plan).or(plan);
        }
        None => {
            if let Some(bucket) = mirror {
                let id = bucket.get("limitId").and_then(Value::as_str);
                windows.extend(codex_bucket_windows(id, bucket));
                plan = codex_plan(bucket);
            }
        }
    }

    ParsedUsage {
        // Rate limits read only under ChatGPT auth — an API-key session errors
        // out above, so a successful read is a subscription by construction.
        auth_mode: AuthMode::Subscription,
        plan,
        availability: UsageAvailability::from_windows(windows),
        reset_credits: codex_reset_credits(result),
    }
}

fn codex_error(message: &str) -> ParsedUsage {
    let (auth_mode, reason) = match message {
        CODEX_API_KEY_ERROR => (AuthMode::ApiKey, UnavailableReason::ApiKeyAuth),
        CODEX_NOT_LOGGED_IN_ERROR => (AuthMode::Unknown, UnavailableReason::NotLoggedIn),
        // Any other failure is genuinely unknown — never an assumed absence.
        // The two arms above match codex's exact wording, so this text is
        // arbitrary upstream output: bound and flatten it like every other
        // external diagnostic, since `usage --format text` prints it verbatim.
        other => {
            return ParsedUsage::unknown(UnknownReason::ProbeFailed {
                message: snippet(other),
            })
        }
    };
    ParsedUsage {
        auth_mode,
        plan: None,
        availability: UsageAvailability::Unavailable { reason },
        // An auth refusal is no rate-limit answer at all.
        reset_credits: NO_ANSWER,
    }
}

/// Read codex's `rateLimitResetCredits` summary off a successful result.
///
/// The summary is optional and nullable in the generated schema, so absent and
/// `null` are both `not_reported` — an older codex, or an account the backend
/// had nothing to say about — never zero. A malformed summary is `unreadable`
/// and leaves the rest of the reading untouched: credits are a sibling of the
/// windows, not a precondition for them.
fn codex_reset_credits(result: &Value) -> ResetCredits {
    let summary = match result.get("rateLimitResetCredits") {
        None | Some(Value::Null) => {
            return ResetCredits::Unknown {
                reason: ResetCreditsUnknown::NotReported,
            }
        }
        Some(summary) => summary,
    };
    codex_reset_credit_summary(summary).unwrap_or_else(|message| ResetCredits::Unknown {
        reason: ResetCreditsUnknown::Unreadable { message },
    })
}

fn codex_reset_credit_summary(summary: &Value) -> Result<ResetCredits, String> {
    if !summary.is_object() {
        return Err(format!(
            "`rateLimitResetCredits` is {} rather than an object",
            json_type_name(summary)
        ));
    }
    let available_count = match summary.get("availableCount") {
        Some(count) => count.as_u64().ok_or_else(|| {
            format!(
                "`rateLimitResetCredits.availableCount` is {} rather than a \
                 non-negative integer",
                json_number_name(count)
            )
        })?,
        None => return Err("`rateLimitResetCredits` carries no `availableCount`".to_string()),
    };
    let credits = match summary.get("credits") {
        None | Some(Value::Null) => None,
        Some(Value::Array(rows)) => Some(
            rows.iter()
                .enumerate()
                .map(|(at, row)| {
                    codex_reset_credit(row)
                        .map_err(|why| format!("`rateLimitResetCredits.credits[{at}]` {why}"))
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Some(other) => {
            return Err(format!(
                "`rateLimitResetCredits.credits` is {} rather than an array or null",
                json_type_name(other)
            ))
        }
    };
    Ok(ResetCredits::Reported {
        available_count,
        credits,
    })
}

/// The error is a fragment the caller prefixes with the row's index, so a
/// malformed row is named in the `unreadable` message.
fn codex_reset_credit(row: &Value) -> Result<ResetCredit, String> {
    if !row.is_object() {
        return Err(format!("is {} rather than an object", json_type_name(row)));
    }
    let required_text = |field: &str| -> Result<String, String> {
        match row.get(field) {
            Some(Value::String(text)) => Ok(without_control_chars(text)),
            Some(other) => Err(format!(
                "carries a `{field}` that is {} rather than a string",
                json_type_name(other)
            )),
            None => Err(format!("carries no `{field}`")),
        }
    };
    let optional_text = |field: &str| -> Result<Option<String>, String> {
        match row.get(field) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) => Ok(Some(without_control_chars(text))),
            Some(other) => Err(format!(
                "carries a `{field}` that is {} rather than a string or null",
                json_type_name(other)
            )),
        }
    };
    let instant = |field: &str, value: &Value| -> Result<UtcInstant, String> {
        let what = match value.as_i64() {
            Some(secs) => match epoch_instant(secs) {
                Some(instant) => return Ok(instant),
                None => "outside the four-digit years",
            },
            None => json_number_name(value),
        };
        Err(format!(
            "carries a `{field}` that is {what} rather than epoch seconds"
        ))
    };
    let granted_at = match row.get("grantedAt") {
        Some(value) => instant("grantedAt", value)?,
        None => return Err("carries no `grantedAt`".to_string()),
    };
    // Absent and `null` alike are the schema's "does not expire".
    let expires_at = match row.get("expiresAt") {
        None | Some(Value::Null) => None,
        Some(value) => Some(instant("expiresAt", value)?),
    };
    Ok(ResetCredit {
        id: required_text("id")?,
        status: required_text("status")?.into(),
        reset_type: required_text("resetType")?.into(),
        granted_at,
        expires_at,
        title: optional_text("title")?,
        description: optional_text("description")?,
    })
}

/// An epoch-seconds instant, when its canonical spelling is one a reader can
/// parse back — a value whose year leaves four digits is not.
fn epoch_instant(secs: i64) -> Option<UtcInstant> {
    let instant = UtcInstant::from_epoch(secs);
    instant
        .as_str()
        .parse::<UtcInstant>()
        .is_ok_and(|parsed| parsed == instant)
        .then_some(instant)
}

/// [`json_type_name`], refined for a number that is not a usable integer.
fn json_number_name(value: &Value) -> &'static str {
    match value {
        Value::Number(number) if number.is_i64() => "a negative integer",
        Value::Number(number) if number.is_u64() => "an out-of-range integer",
        Value::Number(_) => "a fractional number",
        other => json_type_name(other),
    }
}

fn codex_plan(bucket: &Value) -> Option<String> {
    bucket
        .get("planType")
        .and_then(Value::as_str)
        .map(without_control_chars)
}

fn codex_bucket_windows(limit_id: Option<&str>, bucket: &Value) -> Vec<UsageWindow> {
    let label = bucket
        .get("limitName")
        .and_then(Value::as_str)
        .map(without_control_chars);
    ["primary", "secondary"]
        .into_iter()
        .filter_map(|slot| {
            // A null `secondary` means this plan surfaced one window, not a
            // second window at 0% used.
            let window = bucket.get(slot).filter(|value| value.is_object())?;
            let used_percent = window
                .get("usedPercent")
                .and_then(Value::as_f64)
                .and_then(UsedPercent::new)?;
            Some(UsageWindow {
                id: match limit_id {
                    Some(limit_id) => format!("{}/{slot}", without_control_chars(limit_id)),
                    None => slot.to_string(),
                },
                label: label.clone(),
                usage: WindowUsage::Metered {
                    used_percent,
                    counters: None,
                },
                duration: WindowDuration::from_reported_minutes(
                    window.get("windowDurationMins").and_then(Value::as_i64),
                ),
                resets_at: window
                    .get("resetsAt")
                    .and_then(Value::as_i64)
                    .map(UtcInstant::from_epoch),
                scope: None,
                is_binding: None,
            })
        })
        .collect()
}

/// Parse Copilot's `/copilot_internal/user` body.
///
/// Each `quota_snapshots.<id>` entry is gated on `unlimited` **before** any
/// counter is read: an unlimited snapshot reports `entitlement: 0`,
/// `remaining: 0` and `percent_remaining: 100.0`, which are meaningless as
/// counters and would render as a false full bar. `percent_remaining` is
/// percent-*remaining* and is converted to percent-used. `remaining` is taken as
/// the server reports it rather than recomputed from `entitlement -
/// credits_used` (the observed values disagree by about 1, and the server's
/// figure wins).
///
/// The reset is a calendar month, which has no fixed length in seconds, so every
/// window's duration is [`WindowDuration::Unknown`] — the honest answer, not a
/// 30-day default.
#[must_use]
pub fn parse_copilot_user(body: &Value) -> ParsedUsage {
    let plan = body
        .get("copilot_plan")
        .and_then(Value::as_str)
        .map(without_control_chars);
    let resets_at = body
        .get("quota_reset_date_utc")
        .and_then(Value::as_str)
        .and_then(normalize_timestamp);
    // AI credits are the unit only when the account is on token-based billing.
    let unit = if body
        .get("token_based_billing")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        QuotaUnit::AiCredits
    } else {
        QuotaUnit::Unspecified
    };

    // `quota_snapshots` is the whole quota surface, and this endpoint is
    // undocumented internal with no schema to diff — so its absence is drift,
    // not an answer. Without this the parser would report an affirmative "no
    // windows" for every user the moment the key is renamed, which is the same
    // silent-zero failure [`claude_usage_drift`] exists to prevent.
    let Some(snapshots) = body.get("quota_snapshots").and_then(Value::as_object) else {
        return ParsedUsage::unknown(UnknownReason::ProbeFailed {
            message: "the Copilot quota payload carries no `quota_snapshots` object".to_string(),
        });
    };
    let mut windows: Vec<UsageWindow> = Vec::new();
    for (id, snapshot) in snapshots {
        match copilot_window(id, snapshot, resets_at.clone(), unit) {
            Ok(Some(window)) => windows.push(window),
            // Not this shape — aggregated into the whole-payload drift check
            // below, which distinguishes "one odd entry" from "the shape moved".
            Ok(None) => {}
            Err(message) => return ParsedUsage::unknown(UnknownReason::ProbeFailed { message }),
        }
    }
    // The same drift rule one level down: entries that are present but carry
    // none of the expected fields mean the snapshot shape moved, not that the
    // account has no quota. An empty `quota_snapshots` is still an answer.
    if windows.is_empty() && !snapshots.is_empty() {
        return ParsedUsage::unknown(UnknownReason::ProbeFailed {
            message: format!(
                "the Copilot quota payload carries {} snapshot(s) with no readable \
                 `unlimited` or `percent_remaining` field",
                snapshots.len()
            ),
        });
    }

    ParsedUsage {
        auth_mode: AuthMode::Subscription,
        plan,
        availability: UsageAvailability::from_windows(windows),
        reset_credits: NO_READER,
    }
}

/// One `quota_snapshots` entry. `Ok(None)` is an entry that is simply not this
/// shape (handled one level up); `Err` is drift the whole payload must degrade
/// to, because the entry contradicts the contract rather than missing it.
fn copilot_window(
    id: &str,
    snapshot: &Value,
    resets_at: Option<UtcInstant>,
    unit: QuotaUnit,
) -> Result<Option<UsageWindow>, String> {
    // `unlimited` decides whether any counter in this entry means anything, so
    // a value of the wrong type cannot fall back to `false`: that would promote
    // a snapshot which failed to parse into an affirmative metered reading —
    // real-looking headroom derived from a payload nobody could read.
    let unlimited = match snapshot.get("unlimited") {
        Some(Value::Bool(unlimited)) => *unlimited,
        // Absent is the pre-existing "maybe not a quota entry" path: without the
        // gate the entry still needs a `percent_remaining` to say anything.
        None => false,
        Some(other) => {
            return Err(format!(
                "the Copilot quota snapshot `{id}` carries an `unlimited` that is {} \
                 rather than a boolean",
                json_type_name(other)
            ))
        }
    };
    let usage = if unlimited {
        WindowUsage::Unlimited
    } else {
        // percent_remaining is the inverse polarity of the normalized field, so
        // an out-of-range one converts to a negative used percentage and is
        // rejected by the validating constructor rather than rendered.
        let Some(remaining_percent) = snapshot.get("percent_remaining").and_then(Value::as_f64)
        else {
            return Ok(None);
        };
        let Some(used_percent) = UsedPercent::new(100.0 - remaining_percent) else {
            return Ok(None);
        };
        WindowUsage::Metered {
            used_percent,
            counters: copilot_counters(snapshot, unit),
        }
    };
    Ok(Some(UsageWindow {
        id: without_control_chars(id),
        label: None,
        usage,
        // A calendar month is not a fixed number of seconds.
        duration: WindowDuration::Unknown,
        resets_at,
        scope: None,
        is_binding: None,
    }))
}

/// What a JSON value *is*, for a drift message that has to say what arrived
/// where a documented type was expected.
fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

fn copilot_counters(snapshot: &Value, unit: QuotaUnit) -> Option<QuotaCounters> {
    // All five or none: a partial payload yields the percentage alone rather
    // than a counter set with fabricated members. A negative entitlement or
    // consumption is unreadable in the same way a missing one is, so it drops
    // the set instead of being clamped into a plausible-looking figure.
    Some(QuotaCounters {
        entitlement: snapshot
            .get("entitlement")
            .and_then(Value::as_i64)
            .and_then(QuotaAmount::new)?,
        used: snapshot
            .get("credits_used")
            .and_then(Value::as_i64)
            .and_then(QuotaAmount::new)?,
        remaining: snapshot.get("remaining").and_then(Value::as_i64)?,
        has_quota: snapshot.get("has_quota").and_then(Value::as_bool)?,
        overage_permitted: snapshot.get("overage_permitted").and_then(Value::as_bool)?,
        unit,
    })
}

/// How much of an external error payload is quoted back in a probe-failure
/// message. Such a payload is a server error document or a harness's own
/// diagnostic (never a credential), but an unbounded one would swamp the report.
const ERROR_BODY_CHARS: usize = 200;

/// Turn one `/copilot_internal/user` HTTP response into a parsed identity.
///
/// `401` is the token being absent, expired, or rejected — no usable credential,
/// which is [`UnavailableReason::NotLoggedIn`] rather than an unknown. Every
/// other non-200, and a 200 whose body is not JSON, stays
/// [`UnknownReason::ProbeFailed`]: the endpoint is undocumented internal, so an
/// unrecognized answer must degrade to "nothing learned", never to zero used.
#[must_use]
pub fn parse_copilot_http(status: u16, body: &str) -> ParsedUsage {
    if status == 401 {
        return ParsedUsage {
            auth_mode: AuthMode::Unknown,
            plan: None,
            availability: UsageAvailability::Unavailable {
                reason: UnavailableReason::NotLoggedIn,
            },
            reset_credits: NO_READER,
        };
    }
    if status != 200 {
        return ParsedUsage::unknown(UnknownReason::ProbeFailed {
            message: format!(
                "GET /copilot_internal/user returned HTTP {status}: {}",
                snippet(body)
            ),
        });
    }
    match serde_json::from_str::<Value>(body) {
        Ok(value) if value.is_object() => parse_copilot_user(&value),
        _ => ParsedUsage::unknown(UnknownReason::ProbeFailed {
            message: format!(
                "GET /copilot_internal/user returned a body that is not a JSON object: {}",
                snippet(body)
            ),
        }),
    }
}

/// Flatten every control character to a space, keeping the readable text around
/// it. Applied wherever an external payload is first bounded for a diagnostic:
/// those messages are printed verbatim by `usage --format text`, and a harness
/// that wrote ANSI escapes, a carriage return, or a bell into them could
/// otherwise move the cursor, recolour, or overwrite part of a report whose
/// whole purpose is to be trusted at a glance. Sanitizing at the bound means no
/// render site has to remember to.
pub(crate) fn without_control_chars(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// A single-line, character-bounded excerpt of an external payload, for a
/// diagnostic message. Bounded in **characters** so a multi-byte body can never
/// be split mid-code-point.
fn snippet(text: &str) -> String {
    let flat = without_control_chars(text);
    let flat = flat.trim();
    match flat.char_indices().nth(ERROR_BODY_CHARS) {
        Some((at, _)) => format!("{}…", &flat[..at]),
        None => flat.to_string(),
    }
}

/// Parse `cursor-agent about --format json`.
///
/// Cursor is a **plan-tier-only** harness: `subscriptionTier` is the sole
/// plan-level field the non-interactive surface carries, and its dollar pools
/// reach only the interactive TUI — so a successful read is still an
/// affirmative [`UnavailableReason::NoHeadroomReader`], never a percentage.
///
/// The tier is a display name (`Team`), not a lowercase enum like Claude's `max`
/// or codex's `pro`, and is kept verbatim like every other harness's plan.
///
/// A null tier means no **stored** login: the CLI populates it only when both an
/// access and a refresh token are on disk, and an API key does not satisfy that
/// gate. So null is [`UnavailableReason::NotLoggedIn`] — and the probe reports
/// it rather than resolving it, because Cursor's API-key path is a login that
/// overwrites the shared credential store.
#[must_use]
pub fn parse_cursor_about(payload: &Value) -> ParsedUsage {
    // `subscriptionTier` absent entirely is drift, not a logged-out account:
    // the field is always present in this document (null when there is no
    // stored login), so a rename would otherwise report a confident
    // "not logged in" for every Cursor user at once — the same silent-answer
    // failure [`claude_usage_drift`] and the Copilot quota guard prevent.
    let Some(tier) = payload.get("subscriptionTier") else {
        return ParsedUsage::unknown(UnknownReason::ProbeFailed {
            message: "cursor's `about` output carries no `subscriptionTier` field".to_string(),
        });
    };
    // Null is "no stored login" only because the field is contracted to carry a
    // string or null. A tier of any other type contradicts that contract, and
    // reading it as logged-out would state a fact about someone's account from
    // a document that no longer says it.
    let plan = match tier {
        Value::Null => None,
        Value::String(tier) => {
            Some(without_control_chars(tier)).filter(|tier| !tier.trim().is_empty())
        }
        other => {
            return ParsedUsage::unknown(UnknownReason::ProbeFailed {
                message: format!(
                    "cursor's `about` output carries a `subscriptionTier` that is {} \
                     rather than a string or null",
                    json_type_name(other)
                ),
            })
        }
    };
    match plan {
        Some(plan) => ParsedUsage {
            auth_mode: AuthMode::Subscription,
            plan: Some(plan),
            availability: UsageAvailability::Unavailable {
                reason: UnavailableReason::NoHeadroomReader,
            },
            reset_credits: NO_READER,
        },
        None => ParsedUsage {
            auth_mode: AuthMode::Unknown,
            plan: None,
            availability: UsageAvailability::Unavailable {
                reason: UnavailableReason::NotLoggedIn,
            },
            reset_credits: NO_READER,
        },
    }
}

/// Normalize an RFC 3339 / ISO 8601 instant to absolute RFC 3339 **UTC**,
/// whatever offset and sub-second precision it arrived with. `None` when the
/// text is not a complete instant, including when it carries no offset at all —
/// a local-time string is ambiguous, and guessing a zone would be fabrication.
///
/// This is the entry point for a **harness payload**, which legitimately reports
/// a reset in its own offset and is converted. Text that must *already* be UTC —
/// a field of this contract read back from JSON, or one a caller supplies —
/// parses through [`UtcInstant`]'s [`FromStr`] instead, which refuses a
/// non-UTC offset rather than shifting it.
#[must_use]
pub fn normalize_timestamp(text: &str) -> Option<UtcInstant> {
    epoch_from_rfc3339(text).map(UtcInstant::from_epoch)
}

/// Seconds since the UNIX epoch for an RFC 3339 instant. Sub-second precision is
/// parsed but truncated: window resets are minute-scale.
fn epoch_from_rfc3339(text: &str) -> Option<i64> {
    epoch_parts_from_rfc3339(text).map(|(secs, _)| secs)
}

/// Milliseconds since the UNIX epoch for an RFC 3339 instant, for a timestamp
/// that has to be ordered against another taken within the same second (a
/// cache Codex wrote against the moment a refresh started).
#[must_use]
pub fn epoch_millis_from_rfc3339(text: &str) -> Option<i64> {
    epoch_parts_from_rfc3339(text).map(|(secs, millis)| secs * 1_000 + i64::from(millis))
}

/// The instant as whole seconds since the UNIX epoch and the milliseconds past
/// it (digits beyond the third are truncated).
fn epoch_parts_from_rfc3339(text: &str) -> Option<(i64, u32)> {
    let bytes = text.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    // Every separator is checked, not just the date/time one: reading digits at
    // fixed offsets out of unvalidated text would otherwise accept shapes like
    // `2026x08x01T00.00.00Z` as a well-formed instant.
    if !matches!(bytes[10], b'T' | b't' | b' ')
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let year_text = text.get(0..4)?;
    if !year_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let year: i64 = year_text.parse().ok()?;
    let month: u32 = two_digits(text, 5)?;
    let day: u32 = two_digits(text, 8)?;
    let hour: u32 = two_digits(text, 11)?;
    let minute: u32 = two_digits(text, 14)?;
    let second: u32 = two_digits(text, 17)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // A leap second (`:60`) is a real RFC 3339 value; clamping it to :59 keeps
    // the instant within the minute rather than rejecting the whole timestamp.
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    let mut rest = text.get(19..)?;
    let mut millis = 0;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        millis = fraction
            .bytes()
            .take(digits.min(3))
            .chain(std::iter::repeat(b'0'))
            .take(3)
            .fold(0, |sum, digit| sum * 10 + u32::from(digit - b'0'));
        rest = fraction.get(digits..)?;
    }
    let offset_secs = parse_offset(rest)?;

    let days = days_from_civil(year, month, day);
    // `days_from_civil` is total: it happily rolls a date that does not exist
    // (2026-02-29) forward into the next month. Round-tripping the day back
    // through the inverse rejects it instead of silently reporting the wrong
    // instant.
    let (round_year, round_month, round_day, ..) = civil_from_epoch(days * 86_400);
    if (round_year, round_month, round_day) != (year, month, day) {
        return None;
    }

    Some((
        days * 86_400
            + i64::from(hour) * 3_600
            + i64::from(minute) * 60
            + i64::from(second.min(59))
            - offset_secs,
        millis,
    ))
}

/// The instant's offset from UTC, in seconds. `Z` is zero; a bare instant with
/// no offset is rejected by returning `None`.
fn parse_offset(text: &str) -> Option<i64> {
    if matches!(text, "Z" | "z") {
        return Some(0);
    }
    let (sign, rest) = match text.as_bytes().first()? {
        b'+' => (1, &text[1..]),
        b'-' => (-1, &text[1..]),
        _ => return None,
    };
    // Both `+hh:mm` and the compact `+hhmm` appear in the wild.
    let (hours, minutes) = match rest.len() {
        5 if rest.as_bytes()[2] == b':' => (two_digits(rest, 0)?, two_digits(rest, 3)?),
        4 => (two_digits(rest, 0)?, two_digits(rest, 2)?),
        2 => (two_digits(rest, 0)?, 0),
        _ => return None,
    };
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some(sign * (i64::from(hours) * 3_600 + i64::from(minutes) * 60))
}

/// Exactly two ASCII digits at `at`. The digit check is not redundant with the
/// parse: `"+1"` parses as `1`, so a signed pair would otherwise slip through a
/// field that must be two literal digits.
fn two_digits(text: &str, at: usize) -> Option<u32> {
    let pair = text.get(at..at + 2)?;
    pair.bytes()
        .all(|byte| byte.is_ascii_digit())
        .then(|| pair.parse().ok())
        .flatten()
}

/// Days since 1970-01-01 for a civil (proleptic Gregorian) date — Howard
/// Hinnant's `days_from_civil`, the exact inverse of
/// [`crate::domain::history::civil_from_epoch`]'s date half, so a reset
/// timestamp round-trips through [`format_rfc3339`] with no date library.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400; // [0, 399]
    let mp = i64::from(if month > 2 { month - 3 } else { month + 9 }); // [0, 11]
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A canonical observation instant for a report under test. The type is the
    /// invariant, so a test cannot stamp a report with anything else.
    fn observed_at() -> UtcInstant {
        "2026-07-29T12:00:00Z"
            .parse()
            .expect("a canonical RFC 3339 UTC instant")
    }

    fn used_percent(window: &UsageWindow) -> f64 {
        match window.usage {
            WindowUsage::Metered { used_percent, .. } => used_percent.get(),
            WindowUsage::Unlimited => panic!("{} is unlimited, not metered", window.id),
        }
    }

    fn metered(used_percent: f64) -> WindowUsage {
        WindowUsage::Metered {
            used_percent: UsedPercent::new(used_percent).expect("a valid percentage"),
            counters: None,
        }
    }

    fn window<'a>(parsed: &'a ParsedUsage, id: &str) -> &'a UsageWindow {
        parsed
            .availability
            .windows()
            .iter()
            .find(|window| window.id == id)
            .unwrap_or_else(|| panic!("no window {id}"))
    }

    fn ids(parsed: &ParsedUsage) -> Vec<&str> {
        parsed
            .availability
            .windows()
            .iter()
            .map(|window| window.id.as_str())
            .collect()
    }

    /// The observed subscription payload: two non-null plan windows, eleven
    /// null ones (five of them codenames), and the flat `limits[]` array.
    fn claude_subscription_payload() -> Value {
        json!({
            "session": {"total_cost_usd": 0, "model_usage": {}},
            "subscription_type": "max",
            "rate_limits_available": true,
            "rate_limits": {
                "five_hour": {
                    "utilization": 42,
                    "resets_at": "2026-07-29T18:30:00.123456+00:00",
                    "limit_dollars": null, "used_dollars": null, "remaining_dollars": null
                },
                "seven_day": {
                    "utilization": 61,
                    "resets_at": "2026-08-02T09:00:00.000000-04:00",
                    "limit_dollars": null, "used_dollars": null, "remaining_dollars": null
                },
                "seven_day_oauth_apps": null,
                "seven_day_opus": null,
                "seven_day_sonnet": null,
                "seven_day_cowork": null,
                "seven_day_omelette": null,
                "tangelo": null,
                "iguana_necktie": null,
                "omelette_promotional": null,
                "nimbus_quill": null,
                "cinder_cove": null,
                "amber_ladder": null,
                "extra_usage": {
                    "is_enabled": true, "monthly_limit": 5000, "used_credits": 1200,
                    "utilization": 24.0, "currency": "USD", "decimal_places": 2,
                    "disabled_reason": "out_of_credits", "user_disabled": false,
                    "spend_limit_reached": false, "credits_ever_enabled": true,
                    "daily": null, "weekly": null
                },
                "limits": [
                    {"kind": "session", "group": "session", "percent": 42,
                     "severity": "normal", "resets_at": "2026-07-29T18:30:00.123456+00:00",
                     "scope": null, "is_active": false},
                    {"kind": "weekly_all", "group": "weekly", "percent": 61,
                     "severity": "normal", "resets_at": "2026-08-02T13:00:00.000000+00:00",
                     "scope": null, "is_active": true},
                    {"kind": "weekly_scoped", "group": "weekly", "percent": 17,
                     "severity": "normal", "resets_at": "2026-08-02T13:00:00.000000+00:00",
                     "scope": {"model": {"id": null, "display_name": "Opus 5"}, "surface": null},
                     "is_active": false}
                ],
                "member_dashboard_available": true,
                "model_scoped": [
                    {"display_name": "Opus 5", "utilization": 17,
                     "resets_at": "2026-08-02T13:00:00.000000+00:00"}
                ]
            },
            "behaviors": {"day": {"request_count": 3, "session_count": 1}}
        })
    }

    #[test]
    fn claude_plan_is_kept_verbatim_and_windows_are_percent_used() {
        let parsed = parse_claude_get_usage(&claude_subscription_payload());

        assert_eq!(parsed.auth_mode, AuthMode::Subscription);
        assert_eq!(parsed.plan.as_deref(), Some("max"));
        let five_hour = window(&parsed, "five_hour");
        assert_eq!(used_percent(five_hour), 42.0);
        assert_eq!(five_hour.duration, WindowDuration::inferred_from_id(18_000));
        assert_eq!(
            five_hour.resets_at.as_ref().map(UtcInstant::as_str),
            Some("2026-07-29T18:30:00Z")
        );
        // A -04:00 offset must land four hours later in UTC.
        assert_eq!(
            window(&parsed, "seven_day")
                .resets_at
                .as_ref()
                .map(UtcInstant::as_str),
            Some("2026-08-02T13:00:00Z")
        );
    }

    #[test]
    fn claude_null_windows_are_omitted_never_zero_filled() {
        let parsed = parse_claude_get_usage(&claude_subscription_payload());

        assert_eq!(
            ids(&parsed),
            vec!["five_hour", "seven_day", "weekly_scoped/Opus 5"],
            "eleven null keys must be absent, not present at 0%"
        );
        for null_key in ["seven_day_opus", "tangelo", "amber_ladder"] {
            assert!(
                !ids(&parsed).contains(&null_key),
                "{null_key} was null and must not be emitted"
            );
        }
    }

    #[test]
    fn claude_unknown_codename_key_round_trips_as_an_opaque_window() {
        let mut payload = claude_subscription_payload();
        payload["rate_limits"]["tangelo"] = json!({
            "utilization": 7,
            "resets_at": "2026-07-30T00:00:00.000000+00:00",
            "limit_dollars": null, "used_dollars": null, "remaining_dollars": null
        });

        let parsed = parse_claude_get_usage(&payload);

        let tangelo = window(&parsed, "tangelo");
        assert_eq!(used_percent(tangelo), 7.0);
        assert_eq!(
            tangelo.duration,
            WindowDuration::Unknown,
            "a codename key implies no duration and must never have one guessed"
        );
        assert_eq!(tangelo.duration.seconds(), None);
        assert_eq!(
            tangelo.resets_at.as_ref().map(UtcInstant::as_str),
            Some("2026-07-30T00:00:00Z")
        );
        assert_eq!(
            window(&parsed, "five_hour").duration.seconds(),
            Some(18_000),
            "a derivable key still reports its length"
        );
    }

    #[test]
    fn claude_extra_usage_is_not_mistaken_for_a_window() {
        let parsed = parse_claude_get_usage(&claude_subscription_payload());

        assert!(
            !ids(&parsed).contains(&"extra_usage"),
            "extra_usage carries its own `utilization` but is a monthly credit axis"
        );
    }

    #[test]
    fn claude_limits_array_marks_the_binding_window_and_scoped_one() {
        let parsed = parse_claude_get_usage(&claude_subscription_payload());

        assert_eq!(window(&parsed, "five_hour").is_binding, Some(false));
        assert_eq!(window(&parsed, "seven_day").is_binding, Some(true));

        let scoped = window(&parsed, "weekly_scoped/Opus 5");
        assert_eq!(used_percent(scoped), 17.0);
        assert_eq!(scoped.scope.as_deref(), Some("Opus 5"));
        assert_eq!(scoped.duration, WindowDuration::inferred_from_id(604_800));
    }

    #[test]
    fn claude_scoped_limit_without_a_model_or_percent_degrades_honestly() {
        let mut payload = claude_subscription_payload();
        payload["rate_limits"]["limits"] = json!([
            {"kind": "weekly_scoped", "group": "weekly", "percent": 17,
             "severity": "normal", "resets_at": null, "scope": null, "is_active": false},
            {"kind": "weekly_scoped", "group": "weekly", "percent": null,
             "severity": "normal", "resets_at": null,
             "scope": {"model": {"id": null, "display_name": "Opus 5"}, "surface": null},
             "is_active": false},
            {"kind": "not-a-known-kind"}
        ]);

        let parsed = parse_claude_get_usage(&payload);

        assert_eq!(
            ids(&parsed),
            vec!["five_hour", "seven_day", "weekly_scoped"],
            "an unscoped entry keeps the bare kind; one with no percent is dropped"
        );
        let scoped = window(&parsed, "weekly_scoped");
        assert_eq!(scoped.scope, None);
        assert_eq!(scoped.resets_at, None);
        assert_eq!(
            window(&parsed, "five_hour").is_binding,
            None,
            "no session entry means no binding flag, never a fabricated false"
        );
    }

    #[test]
    fn claude_api_key_response_is_unavailable_not_unknown() {
        let payload = json!({
            "session": {"total_cost_usd": 0, "model_usage": {}},
            "subscription_type": null,
            "rate_limits_available": false,
            "rate_limits": null,
            "behaviors": null
        });

        let parsed = parse_claude_get_usage(&payload);

        assert_eq!(parsed.auth_mode, AuthMode::ApiKey);
        assert_eq!(parsed.plan, None);
        assert_eq!(
            parsed.availability,
            UsageAvailability::Unavailable {
                reason: UnavailableReason::ApiKeyAuth
            },
            "API-key auth is a verified finding, never `unknown`"
        );
        assert!(parsed.availability.windows().is_empty());
    }

    #[test]
    fn claude_subscription_without_rate_limits_is_not_an_api_key_verdict() {
        let payload = json!({
            "subscription_type": "team",
            "rate_limits_available": false,
            "rate_limits": null
        });

        let parsed = parse_claude_get_usage(&payload);

        assert_eq!(parsed.auth_mode, AuthMode::Subscription);
        assert_eq!(
            parsed.availability,
            UsageAvailability::Unavailable {
                reason: UnavailableReason::NoWindowsReported
            }
        );
    }

    /// The observed transient: a plan string and `rate_limits_available: true`
    /// with `rate_limits: null` — a subscription whose snapshot claude-code
    /// could not fetch right now.
    fn claude_snapshot_missing_payload() -> Value {
        json!({
            "session": {"total_cost_usd": 0, "model_usage": {}},
            "subscription_type": "max",
            "rate_limits_available": true,
            "rate_limits": null,
            "behaviors": null
        })
    }

    #[test]
    fn claude_missing_snapshot_is_a_transient_that_says_what_the_payload_means() {
        let parsed = parse_claude_get_usage(&claude_snapshot_missing_payload());

        let UsageAvailability::Unknown {
            reason: UnknownReason::ProbeFailed { message },
        } = parsed.availability
        else {
            panic!("a missing snapshot is unknown, never a number: {parsed:?}");
        };
        assert!(
            message.contains("returned no rate-limit snapshot")
                && message.contains("`rate_limits_available` is true")
                && message.contains("`rate_limits` is null"),
            "the message says what the payload carried: {message}"
        );
        assert!(
            message.contains("expired or refreshing") && message.contains("usage fetch failed"),
            "the message names the likely causes: {message}"
        );
        assert!(
            message.contains("worth repeating"),
            "the message says the probe is worth repeating: {message}"
        );
        assert!(
            !message.contains("changed shape"),
            "a transient is not a parser break, and saying so sent a reader after one: {message}"
        );
    }

    #[test]
    fn claude_snapshot_missing_recognizes_exactly_the_transient_shape() {
        assert!(claude_usage_snapshot_missing(
            &claude_snapshot_missing_payload()
        ));

        assert!(
            !claude_usage_snapshot_missing(&claude_subscription_payload()),
            "a full snapshot is an answer"
        );
        assert!(
            !claude_usage_snapshot_missing(&json!({
                "subscription_type": null,
                "rate_limits_available": false,
                "rate_limits": null
            })),
            "API-key auth is an affirmative answer, not a missing one"
        );
        for non_object in [json!([]), json!("none"), json!(0), json!(false)] {
            let mut payload = claude_snapshot_missing_payload();
            payload["rate_limits"] = non_object.clone();
            assert!(
                !claude_usage_snapshot_missing(&payload),
                "{non_object} is a shape the parser was not written against, not a transient"
            );
        }
    }

    #[test]
    fn claude_every_other_non_object_rate_limits_is_still_drift() {
        for non_object in [json!([]), json!("none"), json!(0), json!(false)] {
            let mut payload = claude_snapshot_missing_payload();
            payload["rate_limits"] = non_object.clone();

            let parsed = parse_claude_get_usage(&payload);

            let UsageAvailability::Unknown {
                reason: UnknownReason::ProbeFailed { message },
            } = parsed.availability
            else {
                panic!("{non_object} is drift, never an answer: {parsed:?}");
            };
            assert!(
                message.contains("changed shape") && message.contains("rate_limits"),
                "{non_object} keeps the drift verdict: {message}"
            );
        }
    }

    #[test]
    fn claude_control_response_unwraps_only_a_successful_usage_line() {
        let payload = claude_subscription_payload();
        let line = json!({
            "type": "control_response",
            "response": {"subtype": "success", "request_id": "req_1", "response": payload}
        });
        assert_eq!(
            claude_control_response(&line)
                .and_then(|payload| payload.get("subscription_type"))
                .and_then(Value::as_str),
            Some("max")
        );

        assert!(claude_control_response(&json!({"type": "assistant"})).is_none());
        assert!(
            claude_control_response(&json!({
                "type": "control_response",
                "response": {"subtype": "error", "error": "boom", "response": payload}
            }))
            .is_none(),
            "an error subtype carries no usable payload even when a body is present"
        );
    }

    fn codex_snapshot(limit_id: &str, limit_name: Value, used: i64, secondary: Value) -> Value {
        json!({
            "limitId": limit_id,
            "limitName": limit_name,
            "primary": {"usedPercent": used, "windowDurationMins": 10_080, "resetsAt": 1_785_000_000},
            "secondary": secondary,
            "credits": {"hasCredits": true, "unlimited": false, "balance": "12.34"},
            "individualLimit": null,
            "spendControlReached": false,
            "planType": "pro",
            "rateLimitReachedType": null
        })
    }

    fn codex_result() -> Value {
        json!({
            "id": 2,
            "result": {
                "rateLimits": codex_snapshot("codex", Value::Null, 31, Value::Null),
                "rateLimitsByLimitId": {
                    "codex": codex_snapshot("codex", Value::Null, 31, Value::Null),
                    "limit_model_x": codex_snapshot("limit_model_x", json!("GPT-5.3 Codex"), 12, Value::Null)
                },
                "rateLimitResetCredits": {"availableCount": 0, "credits": []}
            }
        })
    }

    #[test]
    fn codex_prefers_by_limit_id_and_reports_its_window_duration() {
        let parsed = parse_codex_rate_limits(&codex_result());

        assert_eq!(parsed.auth_mode, AuthMode::Subscription);
        assert_eq!(
            parsed.plan.as_deref(),
            Some("pro"),
            "codex's plan vocabulary stays verbatim alongside Claude's"
        );
        assert_eq!(
            ids(&parsed),
            vec!["codex/primary", "limit_model_x/primary"],
            "the top-level rateLimits mirror must not duplicate the bucket"
        );
        let primary = window(&parsed, "codex/primary");
        assert_eq!(used_percent(primary), 31.0);
        assert_eq!(
            primary.duration,
            WindowDuration::reported(604_800),
            "codex states its window length; it is never inferred"
        );
        assert_eq!(
            window(&parsed, "limit_model_x/primary").label.as_deref(),
            Some("GPT-5.3 Codex")
        );
    }

    #[test]
    fn codex_null_secondary_emits_no_second_window() {
        let parsed = parse_codex_rate_limits(&codex_result());

        assert!(
            !ids(&parsed).iter().any(|id| id.ends_with("/secondary")),
            "a null secondary means one window on this plan, not a second at 0%"
        );
    }

    #[test]
    fn codex_secondary_window_is_emitted_when_present() {
        let mut response = codex_result();
        response["result"]["rateLimitsByLimitId"]["codex"]["secondary"] =
            json!({"usedPercent": 88, "windowDurationMins": null, "resetsAt": null});

        let parsed = parse_codex_rate_limits(&response);

        let secondary = window(&parsed, "codex/secondary");
        assert_eq!(used_percent(secondary), 88.0);
        assert_eq!(
            secondary.duration,
            WindowDuration::Unknown,
            "a null windowDurationMins is unknown, not zero"
        );
        assert_eq!(secondary.resets_at, None);
    }

    #[test]
    fn codex_epoch_reset_becomes_absolute_utc() {
        let parsed = parse_codex_rate_limits(&codex_result());

        assert_eq!(
            window(&parsed, "codex/primary")
                .resets_at
                .as_ref()
                .map(UtcInstant::as_str),
            Some("2026-07-25T17:20:00Z")
        );
    }

    #[test]
    fn codex_falls_back_to_the_single_bucket_mirror() {
        let response = json!({
            "id": 2,
            "result": {"rateLimits": codex_snapshot("codex", Value::Null, 55, Value::Null)}
        });

        let parsed = parse_codex_rate_limits(&response);

        assert_eq!(ids(&parsed), vec!["codex/primary"]);
        assert_eq!(used_percent(window(&parsed, "codex/primary")), 55.0);
    }

    #[test]
    fn codex_mirror_without_a_limit_id_keeps_the_bare_slot_name() {
        let response = json!({
            "id": 2,
            "result": {"rateLimits": {
                "primary": {"usedPercent": 9, "windowDurationMins": 300, "resetsAt": null},
                "secondary": null,
                "planType": "plus"
            }}
        });

        let parsed = parse_codex_rate_limits(&response);

        assert_eq!(
            ids(&parsed),
            vec!["primary"],
            "with no limitId to prefix, the slot name stands alone rather than an invented id"
        );
        assert_eq!(parsed.plan.as_deref(), Some("plus"));
        assert_eq!(
            window(&parsed, "primary").duration,
            WindowDuration::reported(18_000)
        );
    }

    #[test]
    fn codex_api_key_error_is_distinct_from_the_not_logged_in_error() {
        let api_key = parse_codex_rate_limits(&json!({
            "id": 4,
            "error": {"code": -32600, "message": CODEX_API_KEY_ERROR}
        }));
        assert_eq!(api_key.auth_mode, AuthMode::ApiKey);
        assert_eq!(
            api_key.availability,
            UsageAvailability::Unavailable {
                reason: UnavailableReason::ApiKeyAuth
            }
        );

        let not_logged_in = parse_codex_rate_limits(&json!({
            "id": 4,
            "error": {"code": -32600, "message": CODEX_NOT_LOGGED_IN_ERROR}
        }));
        assert_eq!(not_logged_in.auth_mode, AuthMode::Unknown);
        assert_eq!(
            not_logged_in.availability,
            UsageAvailability::Unavailable {
                reason: UnavailableReason::NotLoggedIn
            },
            "no stored credential is a different branch from API-key auth"
        );

        assert_ne!(api_key.availability, not_logged_in.availability);
    }

    #[test]
    fn codex_unrecognized_error_is_unknown_not_unavailable() {
        let parsed = parse_codex_rate_limits(&json!({
            "id": 4,
            "error": {"code": -32603, "message": "internal error"}
        }));

        assert_eq!(
            parsed.availability,
            UsageAvailability::Unknown {
                reason: UnknownReason::ProbeFailed {
                    message: "internal error".to_string()
                }
            }
        );
    }

    #[test]
    fn codex_response_without_a_result_object_or_error_is_a_probe_failure() {
        // A missing result, and a result that is not an object: neither carries a
        // shape to conclude from, so neither may reach a verdict about the plan.
        for response in [
            json!({"id": 4}),
            json!({"id": 4, "result": null}),
            json!({"id": 4, "result": "ok"}),
            json!({"id": 4, "result": []}),
        ] {
            let parsed = parse_codex_rate_limits(&response);

            assert!(
                matches!(
                    parsed.availability,
                    UsageAvailability::Unknown {
                        reason: UnknownReason::ProbeFailed { .. }
                    }
                ),
                "{response} must be nothing learned, not an answer"
            );
            assert_eq!(
                parsed.auth_mode,
                AuthMode::Unknown,
                "{response} establishes no auth mode either"
            );
        }
    }

    #[test]
    fn codex_result_without_a_rate_limit_surface_is_a_probe_failure() {
        // An empty result, a result whose rate-limit keys were renamed, an empty
        // bucket map, and a `rateLimits` that is not an object: each answered
        // successfully while carrying no rate-limit surface, so none of them may
        // become "subscription, no windows reported".
        for result in [
            json!({}),
            json!({"rate_limits": {"primary": {"used_percent": 31}}}),
            json!({"rateLimitsByLimitId": {}}),
            json!({"rateLimits": "unlimited"}),
            json!({"rateLimits": null, "rateLimitsByLimitId": null}),
        ] {
            let parsed = parse_codex_rate_limits(&json!({"id": 2, "result": result}));

            assert!(
                matches!(
                    parsed.availability,
                    UsageAvailability::Unknown {
                        reason: UnknownReason::ProbeFailed { .. }
                    }
                ),
                "{result} carries no rate-limit surface, so nothing was learned: \
                 got {:?}",
                parsed.availability
            );
            assert_eq!(
                parsed.auth_mode,
                AuthMode::Unknown,
                "{result} establishes no auth mode either"
            );
            assert!(parsed.availability.windows().is_empty(), "{result}");
        }
    }

    #[test]
    fn codex_non_object_bucket_is_a_probe_failure() {
        for malformed in [json!(17), json!(["not", "a", "snapshot"])] {
            let parsed = parse_codex_rate_limits(&json!({
                "id": 2,
                "result": {
                    "rateLimits": codex_snapshot("codex", Value::Null, 31, Value::Null),
                    "rateLimitsByLimitId": {"codex": malformed}
                }
            }));

            assert_eq!(parsed.auth_mode, AuthMode::Unknown);
            assert_eq!(
                parsed.availability,
                UsageAvailability::Unknown {
                    reason: UnknownReason::ProbeFailed {
                        message: "`rateLimitsByLimitId.codex` is not a rate-limit snapshot object"
                            .to_string()
                    }
                }
            );
        }
    }

    #[test]
    fn codex_reports_no_windows_only_from_a_snapshot_that_carries_none() {
        // The contract-valid empty snapshot is the one shape that *is* an answer:
        // `rateLimits` is present with no window in it, which the schema permits
        // (every `RateLimitSnapshot` field is optional).
        let parsed = parse_codex_rate_limits(&json!({
            "id": 2,
            "result": {"rateLimits": {"planType": "plus", "primary": null, "secondary": null}}
        }));

        assert_eq!(parsed.auth_mode, AuthMode::Subscription);
        assert_eq!(parsed.plan.as_deref(), Some("plus"));
        assert_eq!(
            parsed.availability,
            UsageAvailability::Unavailable {
                reason: UnavailableReason::NoWindowsReported
            }
        );
    }

    /// The observed body: two unlimited snapshots and one exhausted metered one.
    fn copilot_body() -> Value {
        json!({
            "copilot_plan": "individual",
            "access_type_sku": "monthly_subscriber_quota",
            "quota_reset_date": "2026-08-01",
            "quota_reset_date_utc": "2026-08-01T00:00:00.000Z",
            "token_based_billing": true,
            "quota_snapshots": {
                "chat": {
                    "unlimited": true, "percent_remaining": 100.0, "has_quota": true,
                    "entitlement": 0, "remaining": 0, "credits_used": 0,
                    "overage_permitted": false, "quota_reset_at": 0
                },
                "completions": {
                    "unlimited": true, "percent_remaining": 100.0, "has_quota": true,
                    "entitlement": 0, "remaining": 0, "credits_used": 0,
                    "overage_permitted": false, "quota_reset_at": 0
                },
                "premium_interactions": {
                    "unlimited": false, "percent_remaining": 0.0, "has_quota": false,
                    "entitlement": 1500, "credits_used": 13518, "remaining": -12019,
                    "quota_remaining": -12018.1, "overage_count": 12016,
                    "overage_permitted": false, "overage_entitlement": 20000,
                    "timestamp_utc": "2026-07-29T12:54:35.200Z", "quota_reset_at": 0
                }
            }
        })
    }

    #[test]
    fn copilot_unlimited_snapshots_carry_no_counters() {
        let parsed = parse_copilot_user(&copilot_body());

        assert_eq!(parsed.plan.as_deref(), Some("individual"));
        for id in ["chat", "completions"] {
            assert_eq!(
                window(&parsed, id).usage,
                WindowUsage::Unlimited,
                "{id} is unlimited: its 0/0/100.0 counters must never be read"
            );
        }
    }

    #[test]
    fn copilot_exhausted_snapshot_is_percent_used_and_blocked() {
        let parsed = parse_copilot_user(&copilot_body());

        let premium = window(&parsed, "premium_interactions");
        assert_eq!(
            used_percent(premium),
            100.0,
            "percent_remaining 0.0 is 100% used, not 0%"
        );
        let WindowUsage::Metered {
            counters: Some(counters),
            ..
        } = &premium.usage
        else {
            panic!("premium_interactions must carry counters");
        };
        assert_eq!(counters.entitlement.get(), 1500);
        assert_eq!(counters.used.get(), 13518);
        assert_eq!(
            counters.remaining, -12019,
            "the server's own remaining figure wins over entitlement - used"
        );
        assert_eq!(counters.unit, QuotaUnit::AiCredits);
        assert!(
            counters.blocked(),
            "has_quota and overage_permitted both false is exhausted-and-blocked"
        );
    }

    #[test]
    fn copilot_healthy_snapshot_is_not_blocked_and_converts_polarity() {
        let mut body = copilot_body();
        body["quota_snapshots"]["premium_interactions"] = json!({
            "unlimited": false, "percent_remaining": 75.0, "has_quota": true,
            "entitlement": 1500, "credits_used": 375, "remaining": 1125,
            "overage_permitted": true, "quota_reset_at": 0
        });

        let parsed = parse_copilot_user(&body);

        let premium = window(&parsed, "premium_interactions");
        assert_eq!(used_percent(premium), 25.0);
        let WindowUsage::Metered {
            counters: Some(counters),
            ..
        } = &premium.usage
        else {
            panic!("counters expected");
        };
        assert!(!counters.blocked());
    }

    #[test]
    fn copilot_reset_normalizes_and_month_length_stays_unknown() {
        let parsed = parse_copilot_user(&copilot_body());

        let chat = window(&parsed, "chat");
        assert_eq!(
            chat.resets_at.as_ref().map(UtcInstant::as_str),
            Some("2026-08-01T00:00:00Z")
        );
        assert_eq!(
            chat.duration,
            WindowDuration::Unknown,
            "a calendar month has no fixed length in seconds"
        );
    }

    #[test]
    fn copilot_partial_snapshot_keeps_the_percentage_without_fabricated_counters() {
        let body = json!({
            "copilot_plan": "individual",
            "token_based_billing": false,
            "quota_snapshots": {
                "premium_interactions": {"unlimited": false, "percent_remaining": 40.0}
            }
        });

        let parsed = parse_copilot_user(&body);

        assert_eq!(
            window(&parsed, "premium_interactions").usage,
            metered(60.0),
            "no counter field at all still yields the percentage"
        );

        // Every single missing member drops the whole counter set: a default
        // for any one of the five would be a fabricated figure.
        for missing in [
            "entitlement",
            "credits_used",
            "remaining",
            "has_quota",
            "overage_permitted",
        ] {
            let mut partial = copilot_body();
            partial["quota_snapshots"]["premium_interactions"]
                .as_object_mut()
                .unwrap()
                .remove(missing);

            let parsed = parse_copilot_user(&partial);

            assert_eq!(
                window(&parsed, "premium_interactions").usage,
                metered(100.0),
                "a snapshot missing {missing} must carry no counters at all"
            );
        }
    }

    #[test]
    fn an_unusable_percentage_drops_its_window_rather_than_rendering_nonsense() {
        assert_eq!(UsedPercent::new(0.0).map(UsedPercent::get), Some(0.0));
        assert_eq!(UsedPercent::new(100.0).map(UsedPercent::get), Some(100.0));
        assert_eq!(
            UsedPercent::new(140.0).map(UsedPercent::get),
            Some(140.0),
            "a harness reporting consumption past its ceiling is real overage, not an error"
        );
        assert_eq!(UsedPercent::new(-1.0), None);
        assert_eq!(UsedPercent::new(f64::NAN), None);
        assert_eq!(UsedPercent::new(f64::INFINITY), None);
        assert!(serde_json::from_str::<UsedPercent>("-1").is_err());

        let mut claude = claude_subscription_payload();
        claude["rate_limits"]["five_hour"]["utilization"] = json!(-3);
        assert_eq!(
            ids(&parse_claude_get_usage(&claude)),
            vec!["seven_day", "weekly_scoped/Opus 5"]
        );

        let mut codex = codex_result();
        codex["result"]["rateLimitsByLimitId"]["codex"]["primary"]["usedPercent"] = json!(-5);
        assert_eq!(
            ids(&parse_codex_rate_limits(&codex)),
            vec!["limit_model_x/primary"]
        );

        let mut copilot = copilot_body();
        copilot["quota_snapshots"]["premium_interactions"]["percent_remaining"] = json!(140.0);
        assert_eq!(
            ids(&parse_copilot_user(&copilot)),
            vec!["chat", "completions"],
            "a percent_remaining above 100 inverts to a negative used percentage"
        );
    }

    #[test]
    fn an_unavailable_identity_carries_no_percentage_at_all() {
        let parsed = parse_codex_rate_limits(&json!({
            "id": 4,
            "error": {"code": -32600, "message": CODEX_API_KEY_ERROR}
        }));
        let identity = UsageIdentity::new(
            "codex",
            IdentitySelector::EnvPath {
                env: "CODEX_HOME".to_string(),
                path: "/home/u/.codex".to_string(),
            },
            parsed,
        );

        let text = serde_json::to_string(&UsageReport::new(observed_at(), vec![identity])).unwrap();

        assert!(text.contains(r#""state":"unavailable""#));
        assert!(text.contains(r#""reason":"api_key_auth""#));
        assert!(
            !text.contains("used_percent"),
            "an unavailable identity must have no percentage to render as 0%"
        );
        assert!(!text.contains("windows"));
    }

    #[test]
    fn an_available_identity_cannot_be_built_with_no_windows() {
        assert!(Windows::new(Vec::new()).is_none());
        assert_eq!(
            UsageAvailability::from_windows(Vec::new()),
            UsageAvailability::Unavailable {
                reason: UnavailableReason::NoWindowsReported
            }
        );
        let empty: Result<Windows, _> = serde_json::from_str("[]");
        assert!(
            empty.is_err(),
            "an empty available window list must not deserialize either"
        );
    }

    #[test]
    fn a_report_round_trips_through_json() {
        let identities = vec![
            UsageIdentity::new(
                "claude-code",
                IdentitySelector::EnvPath {
                    env: "CLAUDE_CONFIG_DIR".to_string(),
                    path: "/home/u/.claude".to_string(),
                },
                parse_claude_get_usage(&claude_subscription_payload()),
            ),
            UsageIdentity::new(
                "codex",
                IdentitySelector::Ambient,
                parse_codex_rate_limits(&codex_result()),
            ),
            UsageIdentity::new(
                "copilot",
                IdentitySelector::EnvSecret {
                    env: "GH_TOKEN".to_string(),
                },
                parse_copilot_user(&copilot_body()),
            ),
            UsageIdentity::new(
                "goose",
                IdentitySelector::Ambient,
                ParsedUsage::unknown(UnknownReason::Unprobed),
            ),
        ];
        let report = UsageReport::new(observed_at(), identities);

        let text = serde_json::to_string(&report).unwrap();
        let parsed: UsageReport = serde_json::from_str(&text).unwrap();

        assert_eq!(parsed, report);
        assert_eq!(parsed.schema_version.as_str(), SCHEMA_VERSION);
        assert!(text.contains(r#""window_seconds_source":"inferred_from_id""#));
        assert!(text.contains(r#""window_seconds_source":"reported""#));
        assert!(text.contains(r#""window_seconds_source":"unknown""#));
        assert!(
            !text.contains("GH_TOKEN\",\"value"),
            "a secret selector records the variable name only"
        );
    }

    #[test]
    fn selector_keys_never_carry_a_credential() {
        assert_eq!(
            IdentitySelector::EnvPath {
                env: "CODEX_HOME".to_string(),
                path: "/home/u/.codex".to_string()
            }
            .key(),
            "CODEX_HOME=/home/u/.codex"
        );
        assert_eq!(
            IdentitySelector::EnvSecret {
                env: "GH_TOKEN".to_string()
            }
            .key(),
            "GH_TOKEN=<secret>"
        );
        assert_eq!(IdentitySelector::Ambient.key(), "ambient");
    }

    #[test]
    fn timestamps_normalize_to_absolute_utc() {
        assert_eq!(
            normalize_timestamp("2026-08-01T00:00:00.000Z")
                .as_ref()
                .map(UtcInstant::as_str),
            Some("2026-08-01T00:00:00Z")
        );
        assert_eq!(
            normalize_timestamp("2026-08-02T09:00:00.000000-04:00")
                .as_ref()
                .map(UtcInstant::as_str),
            Some("2026-08-02T13:00:00Z")
        );
        assert_eq!(
            normalize_timestamp("2026-08-02T09:00:00+0530")
                .as_ref()
                .map(UtcInstant::as_str),
            Some("2026-08-02T03:30:00Z")
        );
        assert_eq!(
            normalize_timestamp("2026-08-02T09:00:00-07")
                .as_ref()
                .map(UtcInstant::as_str),
            Some("2026-08-02T16:00:00Z"),
            "an hours-only offset is a valid RFC 3339 shape"
        );
        assert_eq!(
            normalize_timestamp("2028-02-29T23:59:59Z")
                .as_ref()
                .map(UtcInstant::as_str),
            Some("2028-02-29T23:59:59Z"),
            "a real leap day must survive"
        );
        assert_eq!(
            normalize_timestamp("1970-01-01T00:00:00Z")
                .as_ref()
                .map(UtcInstant::as_str),
            Some("1970-01-01T00:00:00Z")
        );
    }

    #[test]
    fn an_instant_without_an_offset_is_rejected_rather_than_guessed() {
        assert_eq!(normalize_timestamp("2026-08-01T00:00:00"), None);
        assert_eq!(normalize_timestamp("2026-08-01"), None);
        assert_eq!(normalize_timestamp(""), None);
        assert_eq!(normalize_timestamp("not a timestamp at all"), None);
        assert_eq!(normalize_timestamp("2026-13-01T00:00:00Z"), None);
        assert_eq!(normalize_timestamp("2026-08-01T25:00:00Z"), None);
        assert_eq!(normalize_timestamp("2026-08-01T00:00:00.Z"), None);
        for malformed_separator in [
            "2026x08x01T00:00:00Z", // date separators are not checked by offset alone
            "2026-08-01T00.00.00Z", // nor are the time separators
            "2026-08-01X00:00:00Z", // nor the date/time separator
            "20x6-08-01T00:00:00Z", // a non-digit year
            "+026-08-01T00:00:00Z", // a signed year parses as a number unless digits are required
            "-026-08-01T00:00:00Z", // and a negative one is no RFC 3339 instant either
            "2026-+8-01T00:00:00Z", // `+8` parses as 8 unless digits are required
        ] {
            assert_eq!(
                normalize_timestamp(malformed_separator),
                None,
                "{malformed_separator} is not a well-formed instant"
            );
        }
        for malformed_offset in [
            "2026-08-01T00:00:0005:00", // offset-shaped but unsigned: direction unknown
            "2026-08-01T00:00:00 05:00", // no sign at all
            "2026-08-01T00:00:00+5:00", // not two digits
            "2026-08-01T00:00:00+05:0", // truncated
            "2026-08-01T00:00:00+050",  // neither `+hh`, `+hhmm`, nor `+hh:mm`
            "2026-08-01T00:00:00+24:00", // hours out of range
            "2026-08-01T00:00:00+05:60", // minutes out of range
        ] {
            assert_eq!(
                normalize_timestamp(malformed_offset),
                None,
                "{malformed_offset} is not a usable offset"
            );
        }
        assert_eq!(
            normalize_timestamp("2026-02-29T00:00:00Z"),
            None,
            "2026 is not a leap year: the date must be rejected, not rolled into March"
        );
    }

    #[test]
    fn claude_drift_guard_passes_the_observed_payload_and_its_api_key_shape() {
        assert_eq!(claude_usage_drift(&claude_subscription_payload()), None);

        let api_key = json!({
            "subscription_type": null,
            "rate_limits_available": false,
            "rate_limits": null
        });
        assert_eq!(
            claude_usage_drift(&api_key),
            None,
            "an affirmative `false` needs no window surface — it is the API-key answer"
        );
    }

    #[test]
    fn claude_drift_guard_catches_a_vanished_availability_flag() {
        // The parser reads this flag with `unwrap_or(false)`, so a rename would
        // silently turn every subscription into "no headroom reported". The
        // guard is what makes that read as `unknown` instead.
        let mut payload = claude_subscription_payload();
        payload
            .as_object_mut()
            .expect("an object")
            .remove("rate_limits_available");

        let drift = claude_usage_drift(&payload).expect("a renamed flag is drift");
        assert!(drift.contains("rate_limits_available"), "{drift}");

        payload["rate_limits_available"] = json!("yes");
        assert!(claude_usage_drift(&payload).is_some(), "a non-boolean too");
    }

    #[test]
    fn claude_drift_guard_catches_a_window_surface_that_moved() {
        let mut payload = claude_subscription_payload();
        // Every window key renamed and every `limits[].kind` unrecognized: the
        // parser would find nothing and report an affirmative "no windows".
        payload["rate_limits"] = json!({
            "renamed_five_hour": {"pct_used": 42},
            "limits": [{"kind": "brand_new_kind", "percent": 42}]
        });

        let drift = claude_usage_drift(&payload).expect("a moved window surface is drift");
        assert!(drift.contains("session"), "{drift}");
        assert!(
            matches!(
                parse_claude_get_usage(&payload).availability,
                UsageAvailability::Unknown { .. }
            ),
            "and the parser reports the drift instead of `no windows reported`"
        );
    }

    #[test]
    fn claude_drift_guard_accepts_additive_change_around_a_recognized_surface() {
        // The key set is open by contract: a new codename or a new `kind` next to
        // a recognized one is normal evolution, not drift.
        let mut payload = claude_subscription_payload();
        payload["rate_limits"]["brand_new_codename"] = json!({"utilization": 3});
        payload["rate_limits"]["limits"] = json!([
            {"kind": "brand_new_kind", "percent": 1},
            {"kind": "session", "percent": 42, "is_active": true}
        ]);
        assert_eq!(claude_usage_drift(&payload), None);

        // A recognized window key alone also suffices — the `limits[]` array is
        // not the only surface.
        let no_limits = json!({
            "subscription_type": "max",
            "rate_limits_available": true,
            "rate_limits": {"five_hour": {"utilization": 12}}
        });
        assert_eq!(claude_usage_drift(&no_limits), None);
    }

    #[test]
    fn claude_drift_guard_catches_rate_limits_that_stopped_being_an_object() {
        let mut payload = json!({
            "subscription_type": "max",
            "rate_limits_available": true,
            "rate_limits": []
        });

        let drift = claude_usage_drift(&payload).expect("a non-object surface is drift");
        assert!(drift.contains("rate_limits"), "{drift}");

        // Every non-object but the transient null keeps that verdict.
        for non_object in [json!("none"), json!(0), json!(false)] {
            let mut payload = payload.clone();
            payload["rate_limits"] = non_object.clone();
            let drift = claude_usage_drift(&payload)
                .unwrap_or_else(|| panic!("{non_object} is a non-object surface, so drift"));
            assert!(drift.contains("rate_limits"), "{drift}");
        }

        // The observed transient is the one null the guard lets through: nothing
        // in it moved, and the parser names it as a transient instead.
        payload["rate_limits"] = Value::Null;
        assert_eq!(claude_usage_drift(&payload), None);
    }

    #[test]
    fn claude_drift_guard_catches_a_renamed_or_wrong_typed_auth_discriminator() {
        // `subscription_type` decides API-key versus subscription by its
        // *absence*, and API-key auth is an affirmative "no plan headroom", so a
        // rename would hand every subscriber that verdict as fact.
        let mut renamed = claude_subscription_payload();
        let fields = renamed.as_object_mut().expect("an object");
        fields.remove("subscription_type");
        fields.insert("plan_type".to_string(), json!("max"));

        let drift = claude_usage_drift(&renamed).expect("a renamed discriminator is drift");
        assert!(drift.contains("subscription_type"), "{drift}");

        // Contracted as the plan string or null; anything else was not read.
        for wrong_typed in [json!(7), json!(true), json!({"name": "max"})] {
            let mut payload = claude_subscription_payload();
            payload["subscription_type"] = wrong_typed.clone();

            let drift =
                claude_usage_drift(&payload).unwrap_or_else(|| panic!("{wrong_typed} is drift"));
            assert!(drift.contains("subscription_type"), "{drift}");
        }

        // A null discriminator is the contracted API-key value, not drift.
        let mut api_key = claude_subscription_payload();
        api_key["subscription_type"] = Value::Null;
        assert_eq!(claude_usage_drift(&api_key), None);
    }

    #[test]
    fn cursor_reports_a_plan_tier_and_affirmatively_no_headroom_reader() {
        let about = json!({
            "cliVersion": "2026.07.23-e383d2b",
            "model": "Auto",
            "subscriptionTier": "Team",
            "userEmail": "someone@example.com"
        });

        let parsed = parse_cursor_about(&about);

        assert_eq!(parsed.auth_mode, AuthMode::Subscription);
        assert_eq!(
            parsed.plan.as_deref(),
            Some("Team"),
            "Cursor's tier is a display name, kept verbatim like every other plan"
        );
        assert_eq!(
            parsed.availability,
            UsageAvailability::Unavailable {
                reason: UnavailableReason::NoHeadroomReader
            },
            "the dollar pools exist but reach only the interactive TUI"
        );
        assert!(parsed.availability.windows().is_empty());
    }

    #[test]
    fn cursor_output_without_the_tier_field_is_drift_not_a_logged_out_account() {
        // A renamed field must not read as "no stored login": that is an answer
        // about someone's account, drawn from a document that no longer says it.
        let renamed = json!({
            "cliVersion": "2026.07.23-e383d2b",
            "planName": "Team",
            "userEmail": "someone@example.com"
        });

        let parsed = parse_cursor_about(&renamed);

        let UsageAvailability::Unknown {
            reason: UnknownReason::ProbeFailed { message },
        } = &parsed.availability
        else {
            panic!("got {:?}", parsed.availability);
        };
        assert!(message.contains("subscriptionTier"), "{message}");
        assert_eq!(parsed.auth_mode, AuthMode::Unknown);
        assert_eq!(parsed.plan, None);
    }

    #[test]
    fn cursor_without_a_stored_login_is_not_logged_in_rather_than_unknown() {
        for tier in [json!(null), json!(""), json!("   ")] {
            let parsed = parse_cursor_about(&json!({
                "cliVersion": "2026.07.23-e383d2b",
                "subscriptionTier": tier,
                "userEmail": null
            }));

            assert_eq!(
                parsed.availability,
                UsageAvailability::Unavailable {
                    reason: UnavailableReason::NotLoggedIn
                },
                "a null tier means no stored token pair — reported, never resolved by logging in"
            );
            assert_eq!(parsed.plan, None);
        }
    }

    #[test]
    fn copilot_payload_without_a_quota_surface_is_drift_not_an_answer() {
        // The endpoint is undocumented internal with no schema to diff, so a
        // renamed quota key must degrade to unknown. Reported as "no windows"
        // it would read as a confident answer for every Copilot user at once.
        let renamed = json!({
            "copilot_plan": "individual",
            "token_based_billing": true,
            "quotas": {"premium_interactions": {"unlimited": false, "percent_remaining": 40.0}}
        });

        let parsed = parse_copilot_user(&renamed);

        assert!(
            matches!(
                parsed.availability,
                UsageAvailability::Unknown {
                    reason: UnknownReason::ProbeFailed { .. }
                }
            ),
            "got {:?}",
            parsed.availability
        );
        assert!(parsed.availability.windows().is_empty());

        // An *empty* quota surface is still an answer: the key is there, the
        // account simply has no quota bucket on it.
        let empty = json!({"copilot_plan": "individual", "quota_snapshots": {}});
        assert_eq!(
            parse_copilot_user(&empty).availability,
            UsageAvailability::Unavailable {
                reason: UnavailableReason::NoWindowsReported
            }
        );
    }

    #[test]
    fn copilot_snapshots_with_renamed_fields_are_drift_not_an_empty_quota() {
        // The entry contract is mirrored from an undocumented endpoint, so a
        // renamed `percent_remaining` must not collapse into "no windows" — the
        // guard covers the entries, not just the key they live under.
        let renamed_fields = json!({
            "copilot_plan": "individual",
            "token_based_billing": true,
            "quota_snapshots": {
                "premium_interactions": {"is_unlimited": false, "pct_left": 40.0}
            }
        });

        let parsed = parse_copilot_user(&renamed_fields);

        let UsageAvailability::Unknown {
            reason: UnknownReason::ProbeFailed { message },
        } = &parsed.availability
        else {
            panic!("got {:?}", parsed.availability);
        };
        assert!(message.contains("percent_remaining"), "{message}");

        // One readable entry alongside an unreadable one is still an answer:
        // the shape is intact, and a partial payload already degrades per-window.
        let mixed = json!({
            "copilot_plan": "individual",
            "quota_snapshots": {
                "chat": {"unlimited": true},
                "premium_interactions": {"pct_left": 40.0}
            }
        });
        let parsed = parse_copilot_user(&mixed);
        assert_eq!(ids(&parsed), vec!["chat"]);
    }

    #[test]
    fn copilot_http_200_parses_and_401_is_not_logged_in() {
        let body = copilot_body().to_string();
        let parsed = parse_copilot_http(200, &body);
        assert_eq!(parsed.plan.as_deref(), Some("individual"));
        assert_eq!(used_percent(window(&parsed, "premium_interactions")), 100.0);

        assert_eq!(
            parse_copilot_http(401, "{\"message\":\"Bad credentials\"}").availability,
            UsageAvailability::Unavailable {
                reason: UnavailableReason::NotLoggedIn
            }
        );
    }

    #[test]
    fn copilot_http_failures_degrade_to_unknown_never_to_zero_used() {
        let server_error = parse_copilot_http(503, "upstream unavailable");
        let UsageAvailability::Unknown {
            reason: UnknownReason::ProbeFailed { message },
        } = &server_error.availability
        else {
            panic!("a 503 is nothing learned, not an answer");
        };
        assert!(message.contains("503"), "{message}");
        assert!(message.contains("upstream unavailable"), "{message}");

        let not_json = parse_copilot_http(200, "<html>proxy login</html>");
        assert!(matches!(
            not_json.availability,
            UsageAvailability::Unknown {
                reason: UnknownReason::ProbeFailed { .. }
            }
        ));
        assert!(
            not_json.availability.windows().is_empty(),
            "no percentage is reachable from a failed probe"
        );
    }

    #[test]
    fn a_failing_body_is_quoted_back_bounded_and_on_one_line() {
        let long = format!("é{}", "x".repeat(500));

        let parsed = parse_copilot_http(500, &format!("first\nsecond {long}"));
        let UsageAvailability::Unknown {
            reason: UnknownReason::ProbeFailed { message },
        } = &parsed.availability
        else {
            panic!("expected a probe failure");
        };

        assert!(!message.contains('\n'), "a report line stays one line");
        assert!(message.ends_with('…'), "and is bounded: {message}");
        assert!(
            message.chars().count() < ERROR_BODY_CHARS + 100,
            "an unbounded body would swamp the report"
        );
    }

    #[test]
    fn usage_support_maps_each_tier_to_a_probe_or_an_affirmative_reason() {
        assert_eq!(
            UsageSupport::Probed(UsageProbe::ClaudeGetUsage).probe(),
            Some(UsageProbe::ClaudeGetUsage)
        );
        assert_eq!(
            UsageSupport::Probed(UsageProbe::ClaudeGetUsage).unprobed_reason(),
            None
        );
        assert_eq!(
            UsageSupport::Probed(UsageProbe::CursorAbout).probe(),
            Some(UsageProbe::CursorAbout)
        );
        // How much a probe reports belongs to the probe, so the registry cannot
        // claim headroom from one that only reads a plan name.
        assert_eq!(
            UsageProbe::CursorAbout.reports(),
            UsageReporting::PlanTier,
            "`about` yields a plan tier and nothing more"
        );
        for probe in [
            UsageProbe::ClaudeGetUsage,
            UsageProbe::CodexAppServer,
            UsageProbe::CopilotUserEndpoint,
        ] {
            assert_eq!(probe.reports(), UsageReporting::Headroom);
        }

        assert_eq!(UsageSupport::NoPlanQuota.probe(), None);
        assert_eq!(
            UsageSupport::NoPlanQuota.unprobed_reason(),
            Some(UnavailableReason::NoPlanQuota)
        );
        assert_eq!(
            UsageSupport::NoHeadroomReader.unprobed_reason(),
            Some(UnavailableReason::NoHeadroomReader),
            "a quota that exists with no reader is a different answer from no quota at all"
        );

        assert!(
            !UsageProbe::CopilotUserEndpoint.spawns_harness(),
            "the Copilot probe is out of band: it needs no Copilot binary"
        );
        for probe in [
            UsageProbe::ClaudeGetUsage,
            UsageProbe::CodexAppServer,
            UsageProbe::CursorAbout,
        ] {
            assert!(probe.spawns_harness());
        }
    }

    #[test]
    fn days_from_civil_inverts_the_history_epoch_helper() {
        for secs in [0_i64, 1_785_000_000, -86_400, 4_102_444_800] {
            let text = format_rfc3339(secs);
            assert_eq!(
                epoch_from_rfc3339(&text),
                Some(secs),
                "{text} must round-trip"
            );
        }
    }
}
