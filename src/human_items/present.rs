//! What the human-readable `list` and `report` print against the live store:
//! which items a reader sees, and how much of each.
//!
//! By default a reader sees what its own session owns and what from elsewhere
//! holds its work, each item as one compact entry; `--all` widens that to the
//! host and `--full` prints every body. An explicit filter keeps its meaning
//! across the whole host, and `--json` is never narrowed: both are answered
//! before anything here is consulted.

use std::collections::BTreeSet;

use super::{
    priority_label, HumanItem, HumanItemBlock, HumanItemKind, HumanItemState, HumanItemView,
};

/// The longest description a compact entry prints, in characters, before it is
/// cut with an ellipsis.
pub(crate) const DESCRIPTION_CHARS: usize = 80;

/// The session a reader acts for, and the runs and plans it owns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Asker {
    // llmlint: ignore-block[invalid_states_unrepresentable] session, run and plan here are compared only against `HumanItemOwner`, `HumanItemBlock` and `LaunchRecord`, which spell all three as `String`; an `Asker` is built only by `HumanItems::asker` from a resolved session and the launch records it reads, and a newtype here would be unwrapped at every comparison.
    /// The asking session.
    pub session: String,
    /// The runs that session launched, judged as the run views judge `[mine]`.
    pub runs: BTreeSet<String>,
    /// The plans of those runs.
    pub plans: BTreeSet<String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

impl Asker {
    /// Whether the session raised the item, or a run it owns did.
    fn owns(&self, item: &HumanItem) -> bool {
        item.owner.session == self.session
            || item
                .owner
                .run
                .as_ref()
                .is_some_and(|run| self.runs.contains(run))
    }

    /// Whether the item holds this session's work: all work on the host, or a
    /// plan or run the session owns.
    fn held_by(&self, item: &HumanItem) -> bool {
        item.is_blocking()
            && match &item.block {
                HumanItemBlock::Host => true,
                HumanItemBlock::Plan { plan } => self.plans.contains(plan),
                HumanItemBlock::Work { run, .. } => self.runs.contains(run),
                HumanItemBlock::Nothing => false,
            }
    }

    fn sees(&self, item: &HumanItem) -> bool {
        self.owns(item) || self.held_by(item)
    }
}

/// Which items a human-readable read prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reach {
    /// An explicit filter, which selects across the whole host as it always has.
    Filtered,
    /// `--all`: every item on the host.
    Host,
    /// No session could be resolved, so the reader sees the whole host.
    Unresolved,
    /// One session's own items and those holding its work.
    Session(Asker),
}

/// The items `reach` shows, and the line that says what was left out.
pub(crate) fn narrowed(
    views: Vec<HumanItemView>,
    reach: &Reach,
) -> (Vec<HumanItemView>, Option<String>) {
    match reach {
        Reach::Filtered => (views, None),
        Reach::Host => (views, Some("Showing every item on this host.\n".into())),
        Reach::Unresolved => (
            views,
            Some(format!(
                "No manager session resolved ({} is unset), so this is showing the whole host.\n",
                crate::sys::LAUNCHER_SESSION_ENV
            )),
        ),
        Reach::Session(asker) => {
            let (shown, others): (Vec<_>, Vec<_>) =
                views.into_iter().partition(|view| asker.sees(&view.item));
            let line = format!(
                "Showing this session's items and those holding its work; {} other open item{} \
                 on this host (--all lists every item).\n",
                others.len(),
                if others.len() == 1 { "" } else { "s" }
            );
            (shown, Some(line))
        }
    }
}

/// Where an item came from: plan and run, plan, run, manager session or host.
pub(crate) fn source(item: &HumanItem) -> String {
    if item.block == HumanItemBlock::Host {
        return "host".into();
    }
    match (&item.owner.plan, &item.owner.run) {
        (Some(plan), Some(run)) => format!("plan {plan} / run {run}"),
        (Some(plan), None) => format!("plan {plan}"),
        (None, Some(run)) => format!("run {run}"),
        (None, None) => format!("session {}", item.owner.session),
    }
}

/// What an open item holds, in the contract's wording; empty when it holds nothing.
pub(crate) fn blocking(item: &HumanItem, blocked: &super::HumanItemBlocked) -> String {
    if !item.is_blocking() {
        return String::new();
    }
    match &item.block {
        HumanItemBlock::Host => "blocks all work on this host".into(),
        HumanItemBlock::Plan { plan } => format!("blocks all of {plan}"),
        HumanItemBlock::Work { .. } => blocked
            .plans
            .iter()
            .map(|p| format!("blocks {} steps in {}", p.count, p.plan))
            .collect::<Vec<_>>()
            .join("; "),
        HumanItemBlock::Nothing => String::new(),
    }
}

/// The start of a body as one line of at most [`DESCRIPTION_CHARS`]
/// characters, ending in an ellipsis whenever anything of the body was cut.
pub(crate) fn description(body: &str) -> String {
    let body = body.trim();
    let first = body.lines().next().unwrap_or_default().trim_end();
    let cut = first.len() < body.len() || first.chars().count() > DESCRIPTION_CHARS;
    if !cut {
        return first.to_owned();
    }
    // Leave room for the ellipsis, whether the cut is inside the line or after it.
    let kept: String = first.chars().take(DESCRIPTION_CHARS - 1).collect();
    format!("{}…", kept.trim_end())
}

/// One item as a compact entry: title and id, then priority, kind, what it
/// blocks and its source, then a one-line description.
pub(crate) fn compact(view: &HumanItemView) -> String {
    let item = &view.item;
    let kind = match item.kind {
        HumanItemKind::Task => "task",
        HumanItemKind::Question => "question",
    };
    let blocks = match blocking(item, &view.blocked) {
        held if held.is_empty() => "blocks nothing".to_owned(),
        held => held,
    };
    let state = match item.state {
        HumanItemState::Open => String::new(),
        state => format!(" · {}", super::live::state_word(state)),
    };
    format!(
        "- {} ({})\n  {} {kind} · {blocks} · {}{state}\n  {}\n",
        item.title.trim(),
        item.id,
        priority_label(item.priority),
        source(item),
        description(&item.body)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_description_is_one_line_cut_with_an_ellipsis() {
        assert_eq!(description("  Short body.  "), "Short body.");
        assert_eq!(description("First line.\nSecond line."), "First line.…");
        let long = "word ".repeat(40);
        let cut = description(&long);
        assert!(cut.ends_with('…'), "{cut}");
        assert!(cut.chars().count() <= DESCRIPTION_CHARS, "{cut}");
        let exact = "x".repeat(DESCRIPTION_CHARS);
        assert_eq!(description(&exact), exact);
        // A full-length first line with more after it is still cut to fit.
        let then = description(&format!("{exact}\nmore"));
        assert_eq!(then.chars().count(), DESCRIPTION_CHARS, "{then}");
        assert!(then.ends_with('…'), "{then}");
        // Multibyte text is cut on a character, never inside one.
        let wide = "é".repeat(DESCRIPTION_CHARS + 5);
        assert_eq!(description(&wide).chars().count(), DESCRIPTION_CHARS);
    }
}
