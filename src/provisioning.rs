//! A session whose `post-checkout` provisioning failed: what its worker is told
//! before the first turn, what its manager is told, and what `results` keeps.
//!
//! `onevcs` records the run on the session's own stream. This module reads that
//! record through the linked `onevcs::WorktreeProvision` and restates none of its
//! shape: the session opens regardless, so nothing else here says the worktree's
//! toolchain may be missing. `docs/contract.md` states the names this composes.

use serde::{Deserialize, Serialize};

use crate::channel::{Surface, SurfaceKind};
use crate::event::Envelope;

/// The heading of the section a failed provisioning composes into a task.
pub(crate) const HEADING: &str = "## Worktree provisioning failed";

/// How many of the output's last lines a `provisioning-failed` surface carries.
const SURFACE_OUTPUT_LINES: usize = 20;

/// The `onevcs` label naming the session a stream record belongs to.
pub(crate) const SESSION_LABEL: &str = "session";

/// One session's failed `post-checkout`, as a node's record keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Failure {
    /// The session whose stream recorded it.
    // llmlint: ignore[invalid_states_unrepresentable] the token exactly as `onevcs` labelled the record, read back rather than parsed; `results --json` publishes it as this plain string.
    pub(crate) session: String,
    /// What `onevcs` recorded, under its own keys.
    #[serde(flatten)]
    pub(crate) provision: onevcs::WorktreeProvision,
}

impl Failure {
    /// The failure a `worktree-provision-failed` record states, and `None` for any
    /// other record or one that does not read as `onevcs`'s own payload.
    pub(crate) fn of(envelope: &Envelope) -> Option<Self> {
        let provision = crate::vcs::provision_failure(envelope)?;
        let session = envelope.labels.extra.get(SESSION_LABEL)?.as_str()?;
        Some(Self {
            session: session.to_owned(),
            provision,
        })
    }

    /// How the hook ended, as a clause: `exited with status 3`.
    pub(crate) fn ended(&self) -> String {
        let provision = &self.provision;
        if provision.timed_out {
            return "timed out".to_owned();
        }
        match (provision.exit_status, provision.signal) {
            (_, Some(signal)) => format!("was ended by signal {signal}"),
            (Some(status), None) => format!("exited with status {status}"),
            (None, None) => "ended without an exit status".to_owned(),
        }
    }

    /// The word `onevcs` spells this run's trigger with.
    fn trigger(&self) -> String {
        serde_json::to_value(self.provision.trigger)
            .ok()
            .and_then(|word| word.as_str().map(str::to_owned))
            .unwrap_or_default()
    }

    /// The one line `results` and `status` print for this failure.
    pub(crate) fn line(&self) -> String {
        format!(
            "provisioning failed in session {}: {} {}; rerun in {}: {}",
            self.session,
            self.provision.hook,
            self.ended(),
            self.provision.worktree,
            self.provision.rerun
        )
    }

    /// The section a worker in this session reads before its first turn.
    fn section(&self) -> String {
        let provision = &self.provision;
        let output = provision.output.trim_matches('\n');
        let fence = fence_for(output);
        let truncated = if provision.output_truncated {
            " It was truncated: this is only the end of what the hook printed."
        } else {
            ""
        };
        format!(
            "{HEADING}\n\nThe repository's `{hook}` hook failed at this worktree's \
             `{trigger}` checkout: it {ended}. The session opened regardless.\n\n\
             Its output, trimmed.{truncated}\n\n{fence}\n{output}\n{fence}\n\n\
             Run this in the worktree to repeat it:\n\n```sh\n{rerun}\n```\n\n\
             The worktree's toolchain may be missing until that command succeeds.\n",
            hook = provision.hook,
            trigger = self.trigger(),
            ended = self.ended(),
            rerun = provision.rerun,
        )
    }

    /// The non-blocking surface that tells the manager, once per failed session.
    pub(crate) fn surface(&self, node: &str) -> Surface {
        let provision = &self.provision;
        let lines: Vec<&str> = provision.output.trim_matches('\n').lines().collect();
        let last = &lines[lines.len().saturating_sub(SURFACE_OUTPUT_LINES)..];
        Surface {
            id: 0,
            kind: SurfaceKind::PROVISIONING_FAILED.into(),
            message: format!(
                "node '{node}' opened on a worktree whose provisioning failed\n\
                 node: {node}\nsession: {session}\nworktree: {worktree}\n\
                 hook: {hook} ({trigger})\nended: {ended}\nrerun: {rerun}\n\
                 output (last lines{truncated}):\n{output}",
                session = self.session,
                worktree = provision.worktree,
                hook = provision.hook,
                trigger = self.trigger(),
                ended = self.ended(),
                rerun = provision.rerun,
                truncated = if provision.output_truncated {
                    ", truncated"
                } else {
                    ""
                },
                output = last.join("\n"),
            ),
            source: crate::channel::source::RECONCILER.into(),
            blocking: false,
            queued_at: crate::sys::now_millis(),
            abandoned: false,
            asker: None,
            workstream: Some(node.to_owned()),
            correlation: None,
        }
    }
}

/// A code fence no run of backticks inside `output` can close.
fn fence_for(output: &str) -> String {
    let longest = output
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    "`".repeat(longest.max(2) + 1)
}

/// The task with the failure's section above its operational notes.
pub(crate) fn with_section(task: &str, failure: &Failure) -> String {
    crate::plan::above_notes(task, &failure.section())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(output: &str) -> Failure {
        Failure {
            session: "s-1".into(),
            provision: onevcs::WorktreeProvision {
                hook: "post-checkout".into(),
                trigger: onevcs::ProvisionTrigger::WorktreeAdd,
                worktree: "/w".into(),
                exit_status: Some(3),
                signal: None,
                timed_out: false,
                duration_ms: 1,
                output: output.into(),
                output_truncated: false,
                rerun: "git hook run post-checkout".into(),
            },
        }
    }

    /// The section lands above the notes a rendering appended, wherever they begin,
    /// and a task with none takes it last. A unit, because the rendered headings
    /// other than `## Additional info` each take a whole plan feature to produce.
    #[test]
    fn the_section_sits_above_whichever_notes_the_task_carries() {
        let failed = failure("out");
        let with = |task: &str| with_section(task, &failed);
        let at = |text: &str, what: &str| text.find(what).expect(what);
        let info = with("## What\nx\n\n## Additional info\nnotes\n");
        assert!(
            at(&info, HEADING) < at(&info, "## Additional info"),
            "{info}"
        );
        let bare = with("## What\nx\n");
        assert!(at(&bare, "## What") < at(&bare, HEADING), "{bare}");
        let context = format!(
            "## What\nx\n\n{}\nnotes\n",
            crate::plan::CROSS_REPO_REFERENCES_HEADING
        );
        let appended = with(&context);
        assert!(
            at(&appended, HEADING) < at(&appended, crate::plan::CROSS_REPO_REFERENCES_HEADING),
            "{appended}"
        );
    }

    /// A hook can print a fence of its own, which must not close the block
    /// holding it and spill the rest of its output into the task as prose.
    #[test]
    fn output_carrying_a_fence_is_held_inside_a_longer_one() {
        let section = failure("before\n```\nafter").section();
        assert!(
            section.contains("````\nbefore\n```\nafter\n````"),
            "{section}"
        );
    }

    /// Each way a hook can end is named for what it was, and a timeout outranks
    /// the status git's killed checkout happened to leave behind. A unit, because
    /// git hands a journey's killed hook back as an exit status (143 for a TERM), so
    /// a signal is unreachable by one; a real status and a timeout are journeys.
    #[test]
    fn how_the_hook_ended_names_the_timeout_then_the_signal_then_the_status() {
        let mut failed = failure("");
        assert_eq!(failed.ended(), "exited with status 3");
        failed.provision.exit_status = None;
        assert_eq!(failed.ended(), "ended without an exit status");
        failed.provision.signal = Some(9);
        assert_eq!(failed.ended(), "was ended by signal 9");
        failed.provision.timed_out = true;
        assert_eq!(failed.ended(), "timed out");
    }
}
