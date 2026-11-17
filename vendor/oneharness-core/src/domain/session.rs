//! Pure logic for the uniform session handle (`run --session <name>`).
//!
//! oneharness lets a caller thread ONE stable name across turns instead of
//! extracting each harness's native session id and re-passing it. This module is
//! the pure half: the record persisted per named conversation, the
//! create-vs-continue decision given what the store already holds, and the
//! harness-binding check. All disk/clock I/O lives in [`crate::io::session`].
//!
//! The handle is supported only for harnesses that emit a native session id
//! headlessly (the `extract_session` sources — see
//! [`crate::domain::harness::HarnessSpec::session_capable`]); for the rest the
//! command layer refuses `--session` loudly rather than silently starting fresh.

use crate::domain::harness::HarnessIdentity;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The on-disk session record's schema version, independent of the report's and
/// the history's — the session store shape versions on its own cadence.
///
/// `0.2` binds [`SessionRecord::harness`] to the **variant-qualified** harness id
/// (`claude-code:alternate`) rather than the base id. The field name and type are
/// unchanged, which is exactly why the version has to move: a `0.1` record's
/// `"claude-code"` is indistinguishable from a `0.2` record's for a harness with
/// no variants, so nothing in the value itself says which identity minted the
/// token. See [`resumable`] for what a reader does about that.
pub const SCHEMA_VERSION: &str = "0.2";

/// The one prior store shape, kept named so [`resumable`] documents what it is
/// declining to resume rather than comparing against an anonymous literal.
pub const LEGACY_SCHEMA_VERSION: &str = "0.1";

/// One named conversation's persisted state: the harness it runs on and that
/// harness's native token to resume it with. Written after a run captures a
/// session id, read on the next run to continue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    /// The store shape version ([`SCHEMA_VERSION`]).
    pub schema_version: String,
    /// The caller's stable handle (`--session <name>`, sanitized for a path).
    pub name: String,
    /// The project the session belongs to (its display path), so the same name
    /// in different projects never collides.
    pub project: String,
    /// The **variant-qualified** harness identity this session runs on
    /// (`claude-code:alternate`, or plain `codex` when no variant is configured)
    /// — the whole id, because a variant is a whole identity: each one points the
    /// harness at its own home directory, so each keeps a *disjoint* session
    /// namespace and a token minted under one means nothing under another. A
    /// session is bound to the identity that created it; resuming it on another
    /// is a usage error ([`harness_conflict`]).
    ///
    /// Typed as [`HarnessIdentity`] rather than a `String` so the store cannot
    /// hold text that is not even a well-formed id: such a record fails to
    /// deserialize, and [`crate::io::session::read`] reports no record at all —
    /// the same fresh start a legacy record gets. A record whose variant is
    /// merely *unconfigured* still parses (see [`HarnessIdentity`]) — a config
    /// edit must not corrupt the store — and is caught by [`harness_conflict`]
    /// instead, which is the check that needs it to still say what it was bound
    /// to.
    pub harness: HarnessIdentity,
    /// The harness's native continuation token (its emitted session id) — what
    /// the next run resumes with.
    pub token: String,
    /// RFC 3339 timestamp the session was first created.
    pub created: String,
    /// RFC 3339 timestamp of the most recent run that touched it.
    pub updated: String,
}

/// Whether a run starts a new named session or continues an existing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SessionPhase {
    /// No stored token yet — the harness runs fresh and its emitted session id is
    /// captured for next time.
    Create,
    /// A stored token exists — the run resumes it.
    Continue,
}

impl SessionPhase {
    /// The lowercase token used in the JSON report and text output.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            SessionPhase::Create => "create",
            SessionPhase::Continue => "continue",
        }
    }
}

/// How to build this run's argv for a `--session` request, given what the store
/// holds: the phase, and the token to resume with (`Some` iff continuing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPlan {
    pub phase: SessionPhase,
    /// The native token to resume with — fed to the harness's verified `--resume`
    /// mapping. `Some` exactly when `phase == Continue`.
    pub resume_token: Option<String>,
}

impl SessionPlan {
    /// Decide the plan from the existing record (if any). A stored record
    /// continues its token; no record starts fresh.
    #[must_use]
    pub fn decide(existing: Option<&SessionRecord>) -> SessionPlan {
        match existing {
            Some(record) => SessionPlan {
                phase: SessionPhase::Continue,
                resume_token: Some(record.token.clone()),
            },
            None => SessionPlan {
                phase: SessionPhase::Create,
                resume_token: None,
            },
        }
    }
}

/// The stored record a run may build on: `Some` only for one this build can
/// resume, `None` for one it must replace with a fresh session.
///
/// The only rejection today is a record at [`LEGACY_SCHEMA_VERSION`], whose
/// `harness` is a base id that predates variant qualification. Which identity
/// minted its token is unrecoverable — `"claude-code"` names the adapter, not the
/// `CLAUDE_CONFIG_DIR` the conversation lives in — and *guessing* reproduces the
/// bug the qualification exists to fix: handing one identity's token to another
/// yields an unresumable session at best, and at worst strands a whole fallback
/// chain on the resume that can never succeed. So the run starts fresh. That
/// costs one conversation's continuity, once, on records written before this
/// version.
///
/// Applied at the store boundary, so [`SessionPlan::decide`] and
/// [`harness_conflict`] see a legacy record as no record at all.
#[must_use]
pub fn resumable(existing: Option<SessionRecord>) -> Option<SessionRecord> {
    existing.filter(|record| record.schema_version == SCHEMA_VERSION)
}

/// The harness a prior record is bound to when it differs from `harness` — a
/// usage error, since a named session cannot migrate between harnesses. `None`
/// when there is no record or it already belongs to `harness`.
///
/// Both sides are the **variant-qualified** identity (see
/// [`SessionRecord::harness`]), so a cross-*variant* migration conflicts exactly
/// like a cross-harness one: `claude-code:alternate`'s token is no more usable by
/// `claude-code:primary` than by `codex`, and comparing base ids would have
/// called that a match.
#[must_use]
pub fn harness_conflict<'a>(
    existing: Option<&'a SessionRecord>,
    harness: &HarnessIdentity,
) -> Option<&'a HarnessIdentity> {
    existing
        .filter(|record| &record.harness != harness)
        .map(|record| &record.harness)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(text: &str) -> HarnessIdentity {
        text.parse().expect("test identity parses")
    }

    fn record(token: &str, harness: &str) -> SessionRecord {
        SessionRecord {
            schema_version: SCHEMA_VERSION.to_string(),
            name: "greet".to_string(),
            project: "/p".to_string(),
            harness: id(harness),
            token: token.to_string(),
            created: "2026-07-10T00:00:00Z".to_string(),
            updated: "2026-07-10T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn decide_creates_without_a_record() {
        let plan = SessionPlan::decide(None);
        assert_eq!(plan.phase, SessionPhase::Create);
        assert_eq!(plan.resume_token, None);
    }

    #[test]
    fn decide_continues_a_stored_token() {
        let existing = record("sess-1", "claude-code");
        let plan = SessionPlan::decide(Some(&existing));
        assert_eq!(plan.phase, SessionPhase::Continue);
        assert_eq!(plan.resume_token.as_deref(), Some("sess-1"));
    }

    #[test]
    fn phase_strings_are_stable() {
        assert_eq!(SessionPhase::Create.as_str(), "create");
        assert_eq!(SessionPhase::Continue.as_str(), "continue");
    }

    #[test]
    fn harness_conflict_flags_a_mismatch_only() {
        let claude = record("s", "claude-code");
        assert_eq!(harness_conflict(None, &id("claude-code")), None);
        assert_eq!(harness_conflict(Some(&claude), &id("claude-code")), None);
        assert_eq!(
            harness_conflict(Some(&claude), &id("codex")),
            Some(&id("claude-code"))
        );
    }

    #[test]
    fn harness_conflict_rejects_a_cross_variant_migration() {
        // Two identities of the same harness are two disjoint session
        // namespaces, so the token binds to the qualified id: handing
        // `alternate`'s token to `primary` (or to the bare base id) is the same
        // category of error as handing it to another harness entirely.
        let alternate = record("s", "claude-code:alternate");
        assert_eq!(
            harness_conflict(Some(&alternate), &id("claude-code:alternate")),
            None
        );
        for other in ["claude-code:primary", "claude-code", "codex"] {
            assert_eq!(
                harness_conflict(Some(&alternate), &id(other)),
                Some(&id("claude-code:alternate")),
                "`{other}` must not inherit another identity's session"
            );
        }
    }

    #[test]
    fn a_legacy_record_is_not_resumable_and_starts_fresh() {
        // A v0.1 record's `harness` is a base id: nothing in it says which
        // identity minted the token, so it is dropped rather than guessed at —
        // the run creates a new session and conflicts with nothing.
        let legacy = SessionRecord {
            schema_version: LEGACY_SCHEMA_VERSION.to_string(),
            ..record("sess-1", "claude-code")
        };
        assert_eq!(resumable(Some(legacy.clone())), None);
        let plan = SessionPlan::decide(resumable(Some(legacy.clone())).as_ref());
        assert_eq!(plan.phase, SessionPhase::Create);
        assert_eq!(plan.resume_token, None);
        assert_eq!(
            harness_conflict(
                resumable(Some(legacy)).as_ref(),
                &id("claude-code:alternate")
            ),
            None
        );

        // A record at the current version keeps resuming.
        let current = record("sess-1", "claude-code:alternate");
        assert_eq!(resumable(Some(current.clone())), Some(current));
    }

    #[test]
    fn record_round_trips_through_json() {
        let original = record("sess-9", "opencode");
        let text = serde_json::to_string(&original).unwrap();
        let parsed: SessionRecord = serde_json::from_str(&text).unwrap();
        assert_eq!(original, parsed);
    }
}
