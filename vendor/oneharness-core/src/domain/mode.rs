//! The normalized permission/approval mode — one vocabulary across every
//! harness's own approval system.
//!
//! Each harness spells autonomy differently (Claude Code's `--permission-mode`,
//! Codex's `--sandbox`, Qwen's `--approval-mode`, Goose's `GOOSE_MODE`, …).
//! [`PermissionMode`] is oneharness's single ordered spectrum; the registry
//! ([`crate::domain::harness`]) maps each value to the selected harness's native
//! mechanism (argv flags and/or environment), and declares which modes a harness
//! can actually honor in a *headless* run via [`ModeHeadless`]. One mode can
//! need two spellings on one harness: Codex's sandbox rides `--sandbox` on a
//! fresh `codex exec`, but `codex exec resume` has no such option, so a resumed
//! turn carries the same sandbox as the `-c sandbox_mode=` config override.
//! This module is pure data + parsing; the mapping and the spawn live in the
//! harness registry and the command/io layers.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The unified approval mode, from least to most autonomy. A harness may not
/// support every value (see [`crate::domain::harness::HarnessSpec::mode`]); the
/// command layer refuses an unsupported one before spawning, never silently
/// downgrading it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionMode {
    /// No mutations — the agent may read but not edit files or run commands —
    /// *without* the plan workflow: it just does whatever read-only work the task
    /// allows. Mapped to each harness's strongest per-run no-mutation enforcement
    /// (Codex's read-only sandbox, Claude's deny rules, Copilot's `--deny-tool`,
    /// Cursor's `ask` mode). The enforced counterpart to [`Self::Plan`].
    ReadOnly,
    /// Read and propose only — like [`Self::ReadOnly`], but additionally engages
    /// the harness's *plan* workflow: research the task and write a plan rather
    /// than act. Supported only where a native plan mode exists (Claude/Qwen/
    /// Cursor/Copilot, OpenCode's `plan` agent); harnesses with no plan workflow
    /// (Codex, Goose, Crush) reject it — use [`Self::ReadOnly`] for those.
    Plan,
    /// The harness's normal ask flow, mapped to its cleanest *non-interactive*
    /// variant so a headless run never blocks waiting for input (Claude's
    /// `dontAsk` deny-and-continue, Goose's fail-closed `approve`, Copilot's
    /// auto-deny). Where no such variant exists the harness's [`ModeHeadless`]
    /// is [`ModeHeadless::Hangs`].
    Default,
    /// Auto-approve file edits, still gate shell/commands. (Claude's
    /// `acceptEdits`, Qwen's `auto-edit`.)
    Edit,
    /// Auto-approve operations the harness deems safe (a classifier), gate the
    /// risky ones. (Claude's `auto`, Qwen's `auto`, Goose's `smart_approve`.)
    Auto,
    /// Approve everything — no prompts, no gating. The headless default, because
    /// an unattended agent otherwise hangs on the first approval. Every harness
    /// supports it. (Claude's `bypassPermissions`, Codex's
    /// `--dangerously-bypass-approvals-and-sandbox`, Qwen/Crush `--yolo`, …)
    Bypass,
}

/// What a harness's OWN run does when a tool needs approval in a given mode.
///
/// A two-variant type rather than a `bool` for the reason
/// [`PermissionDecision`](crate::domain::http::PermissionDecision) is one: it
/// decides what an agent may do, and at a call site `true`/`false` says nothing
/// about which way round it is, while an inverted one does not fail — it
/// quietly hands a run authority it was not given.
///
/// Distinct from that decision even so. This is a *fact about the harness*,
/// declared per mode on its [`ModeSpec`](crate::domain::harness::ModeSpec) and
/// read off its real argv/environment mapping; the decision is what a driven
/// turn then answers a server with. Keeping them apart is what makes
/// "the controlled posture is the uncontrolled one" a statement that can be
/// checked rather than an identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalPosture {
    /// The harness acts without asking.
    Unattended,
    /// A tool that needs approval is gated.
    Gated,
}

impl ApprovalPosture {
    /// The posture the normalized spectrum implies: `auto` and `bypass` mean
    /// "act without asking", everything else gates. The default a harness gets
    /// unless its own CLI cannot honor it.
    #[must_use]
    pub const fn of(mode: PermissionMode) -> Self {
        match mode {
            PermissionMode::Auto | PermissionMode::Bypass => ApprovalPosture::Unattended,
            _ => ApprovalPosture::Gated,
        }
    }

    /// Whether the harness acts without asking.
    #[must_use]
    pub fn is_unattended(self) -> bool {
        matches!(self, ApprovalPosture::Unattended)
    }
}

impl PermissionMode {
    /// Every mode, in spectrum order. The source of truth for `--help`, the
    /// `--mode` value parser, and `oneharness list`.
    pub const ALL: [PermissionMode; 6] = [
        PermissionMode::ReadOnly,
        PermissionMode::Plan,
        PermissionMode::Default,
        PermissionMode::Edit,
        PermissionMode::Auto,
        PermissionMode::Bypass,
    ];

    /// The CLI / JSON token for this mode (kebab-case, matching `Serialize`).
    pub fn as_str(self) -> &'static str {
        match self {
            PermissionMode::ReadOnly => "read-only",
            PermissionMode::Plan => "plan",
            PermissionMode::Default => "default",
            PermissionMode::Edit => "edit",
            PermissionMode::Auto => "auto",
            PermissionMode::Bypass => "bypass",
        }
    }

    /// Parse a `--mode` / `ONEHARNESS_MODE` token, with a loud error listing the
    /// valid values (the same contract as an unknown config value).
    pub fn parse(s: &str) -> Result<Self, String> {
        PermissionMode::ALL
            .into_iter()
            .find(|m| m.as_str() == s)
            .ok_or_else(|| format!("must be one of {}, got `{s}`", Self::valid_list()))
    }

    /// Comma-joined valid tokens, for error messages and help.
    pub fn valid_list() -> String {
        PermissionMode::ALL
            .iter()
            .map(|m| m.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The legacy boolean `bypass`: `true` is [`PermissionMode::Bypass`], `false`
    /// the [`PermissionMode::Default`] ask flow. Lets the old `--bypass` /
    /// `--no-bypass` flags and `bypass =` config map onto the mode spectrum.
    pub fn from_bypass(bypass: bool) -> Self {
        if bypass {
            PermissionMode::Bypass
        } else {
            PermissionMode::Default
        }
    }

    /// Whether this is the all-approving bypass mode (kept so the report's
    /// back-compat `bypass_permissions` boolean stays derivable from the mode).
    pub fn is_bypass(self) -> bool {
        matches!(self, PermissionMode::Bypass)
    }
}

/// How a harness honors one [`PermissionMode`] in a *headless* run — the
/// classification that lets oneharness refuse a hang instead of waiting it out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ModeHeadless {
    /// Never blocks: the harness denies-and-continues, fails closed, or runs
    /// fully autonomously. Safe to spawn unattended.
    Clean,
    /// May block on an interactive approval prompt that a headless run cannot
    /// answer (the harness has no clean non-interactive variant of this mode).
    /// The command layer refuses this combination unless `--permit-prompts` is
    /// set, turning a silent hang into a loud, immediate error.
    Hangs,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_round_trips_every_mode() {
        for m in PermissionMode::ALL {
            assert_eq!(PermissionMode::parse(m.as_str()).unwrap(), m);
        }
    }

    #[test]
    fn parse_rejects_unknown_with_a_listing() {
        let err = PermissionMode::parse("yolo").unwrap_err();
        assert!(err.contains("yolo"), "{err}");
        assert!(err.contains("bypass"), "lists valid values: {err}");
    }

    #[test]
    fn bypass_bridge_maps_both_ways() {
        assert_eq!(PermissionMode::from_bypass(true), PermissionMode::Bypass);
        assert_eq!(PermissionMode::from_bypass(false), PermissionMode::Default);
        assert!(PermissionMode::Bypass.is_bypass());
        assert!(!PermissionMode::Default.is_bypass());
    }

    #[test]
    fn serializes_as_kebab_tokens() {
        let json = serde_json::to_string(&PermissionMode::Bypass).unwrap();
        assert_eq!(json, "\"bypass\"");
        let parsed: PermissionMode = serde_json::from_str("\"plan\"").unwrap();
        assert_eq!(parsed, PermissionMode::Plan);
    }
}
