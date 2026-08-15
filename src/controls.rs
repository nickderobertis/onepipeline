//! The per-node controls a dispatch carries, and how each one reaches it.
//!
//! A *control* is something a plan node declares that changes **how** its
//! dispatch runs, as distinct from what it is asked to do. Each one reaches the
//! node-scope agent graph the way an operator's `--node-set` does: as one
//! `PATH=VALUE` override the sibling applies to the effective configuration,
//! refusing a path that names nothing.
//!
//! The rule this module exists to hold: **a control this crate accepts and
//! cannot apply is refused, rather than falling back to a default.** The plan
//! schema used to carry a `done_when` that nothing ever transmitted — the
//! planner's documented way to set a per-node review bar did nothing, and
//! reported nothing, so every attempt to tune the bar produced no change and no
//! warning. A control with nowhere to land now stops the plan at validation,
//! which is where a launch reads it.
//!
//! [`NodeControls`] is the *dispatch's* view of those controls, and it is valid
//! by construction: a turn budget is a [`NonZeroU32`], so "run for no turns" —
//! which no dispatch can honour, and which `oneagentgraph` refuses when it
//! validates the graph — cannot be built, passed along, or launched with. The
//! plan schema keeps `Option<u32>`, because that is the shape a v7 plan file is
//! written in and a live edit merges submitted JSON into; the checked conversion
//! between the two is [`NodeControls::of_node`] and [`NodeControls::of_step`],
//! and it happens at the trust boundary every plan and every edit crosses.
//!
//! How a future control is kept honest: `NodeControls::declared` destructures
//! the struct field by field with **no `..` rest pattern**, so a control added
//! to [`NodeControls`] does not compile until this module says what it applies
//! to. A control whose honest answer is "nothing this crate can apply" is given
//! `set: None` there, and the renderer beneath it refuses that control by name.

use std::num::NonZeroU32;

use crate::plan::{Node, Step};

/// The node-scope graph member a node's work runs as, and therefore the member
/// every per-node override addresses.
///
/// The shipped `graphs/node-scope.yaml` names it, and a graph a plan substitutes
/// with `agent_graph` is expected to name its worker the same — which is the
/// assumption the persona override has always made.
pub(crate) const WORKER_MEMBER: &str = "worker";

/// What a plan declaring a turn budget of zero is told.
///
/// Its own sentence rather than an inline string, because two conversions
/// produce it — a node's and a step's — and one wording is what makes the answer
/// the same wherever a planner wrote the zero.
pub(crate) const ZERO_TURNS: &str =
    "`max_turns: 0` lets the dispatch take no turn at all; omit it to run under the agent \
     graph's own ceiling";

/// The controls one node or step declares, as a dispatch carries them.
///
/// Copied out of the plan rather than borrowed from it, because a dispatch
/// outlives the borrow of the graph it was built from — and *narrowed* on the way
/// through: the plan's `Option<u32>` can say zero and this cannot, so no code
/// downstream of the conversion has to ask whether the budget it holds is one a
/// dispatch could run under.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct NodeControls {
    /// The dispatch's turn budget, applied as the worker member's `max_turns`.
    pub max_turns: Option<NonZeroU32>,
}

/// One control a node declared.
///
/// A closed set, and each variant carries its own value: what a control is
/// called and what applies it are both decided by the variant, so a control
/// cannot be built under one name and applied as another, and a name this build
/// does not know cannot be built at all.
enum Control {
    /// The turn budget, which the worker member's own `max_turns` applies.
    MaxTurns(NonZeroU32),
    /// A control this build accepts and has no override for.
    ///
    /// None exists today, which is why it is `cfg(test)`: production cannot name
    /// one, and the refusal that answers one is proven rather than assumed. A
    /// future control that has nowhere to land takes this shape without the
    /// `cfg` — and until it is given one, `declared` will not compile.
    #[cfg(test)]
    Unappliable(&'static str),
}

impl Control {
    /// The name the *plan* spells it with, which is what a refusal has to say.
    fn name(&self) -> &'static str {
        match self {
            Self::MaxTurns(_) => "max_turns",
            #[cfg(test)]
            Self::Unappliable(name) => name,
        }
    }

    /// The `PATH=VALUE` that applies it, or `None` where this build has none.
    fn set(&self) -> Option<String> {
        match self {
            // `OnejudgeMember::max_turns` is the sibling's own turn ceiling for
            // the two-party conversation a node's dispatch is, so the plan's
            // budget is that field and needs no merge layer here.
            Self::MaxTurns(budget) => Some(format!("members.{WORKER_MEMBER}.max_turns={budget}")),
            #[cfg(test)]
            Self::Unappliable(_) => None,
        }
    }
}

impl NodeControls {
    /// What a node declares, checked.
    ///
    /// # Errors
    ///
    /// The reason the node's declaration is not one a dispatch can run under —
    /// today, a turn budget of zero.
    pub fn of_node(node: &Node) -> std::result::Result<Self, String> {
        Ok(Self {
            max_turns: turn_budget(node.max_turns)?,
        })
    }

    /// What one step of a lifecycle node declares, checked.
    ///
    /// # Errors
    ///
    /// As [`of_node`](Self::of_node): a step's budget crosses the same boundary,
    /// and a step is where a workstream's budgets are actually written.
    pub fn of_step(step: &Step) -> std::result::Result<Self, String> {
        Ok(Self {
            max_turns: turn_budget(step.max_turns)?,
        })
    }

    /// The `--set` overrides these controls render to, or the reason the first
    /// one that cannot be applied is refused.
    ///
    /// The refusal is a plain message rather than an [`Error`](crate::Error) so
    /// that each caller says *where* it was refused — a plan node at validation,
    /// or a dispatch at launch — without a second error prefix inside the
    /// sentence.
    ///
    /// # Errors
    ///
    /// The name of the first control this build accepts and cannot apply.
    pub fn overrides(&self) -> std::result::Result<Vec<String>, String> {
        rendered(self.declared())
    }

    fn declared(&self) -> Vec<Control> {
        // Destructured with no `..` rest pattern on purpose: a control added to
        // `NodeControls` does not compile until it is given a line below. One
        // this crate cannot apply is written `set: None` — accepted by the
        // schema, refused at validation, never silently dropped.
        let Self { max_turns } = self;
        let mut declared = Vec::new();
        if let Some(budget) = max_turns {
            declared.push(Control::MaxTurns(*budget));
        }
        declared
    }
}

/// The plan's turn budget as a dispatch can hold one.
///
/// The whole narrowing, in one place: absent stays absent, a positive budget
/// becomes one that cannot later be read as zero, and the zero is refused here
/// rather than carried to a member that could not start. `oneagentgraph` refuses
/// `max_turns: 0` when it validates the graph it is handed, so a zero allowed
/// through would fail the launch anyway — several minutes and one composed
/// dispatch later, in a sibling's words rather than in the plan's.
fn turn_budget(declared: Option<u32>) -> std::result::Result<Option<NonZeroU32>, String> {
    match declared {
        None => Ok(None),
        Some(budget) => NonZeroU32::new(budget)
            .map(Some)
            .ok_or(ZERO_TURNS.to_string()),
    }
}

/// The overrides a declared set renders to, or the first one with nowhere to go.
///
/// The single place a control becomes an override, so there is one answer to
/// "what happens to a control this build cannot apply" rather than one per
/// caller.
fn rendered(declared: Vec<Control>) -> std::result::Result<Vec<String>, String> {
    declared
        .into_iter()
        .map(|control| {
            control.set().ok_or_else(|| {
                format!(
                    "`{}` is a control this build accepts and cannot apply to a dispatch, \
                     so the dispatch would silently run under a default nobody asked for",
                    control.name()
                )
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(max_turns: Option<u32>) -> Node {
        Node {
            id: "build".into(),
            persona: Some("engineer".into()),
            task: Some("## What\nship it".into()),
            max_turns,
            ..Node::default()
        }
    }

    fn step(max_turns: Option<u32>) -> Step {
        Step {
            id: "implement".into(),
            persona: Some("engineer".into()),
            task: Some("## What\nship it".into()),
            max_turns,
            ..Step::default()
        }
    }

    #[test]
    fn a_declared_turn_budget_renders_as_the_workers_own_override() {
        let controls = NodeControls::of_node(&node(Some(45))).expect("45 is a budget");
        assert_eq!(
            controls.overrides().expect("a budget is appliable"),
            vec!["members.worker.max_turns=45".to_string()]
        );
    }

    #[test]
    fn a_node_that_declares_no_control_overrides_nothing() {
        let controls = NodeControls::of_node(&node(None)).expect("nothing to convert");
        assert_eq!(controls.max_turns, None);
        assert_eq!(
            controls.overrides().expect("nothing to apply"),
            Vec::<String>::new(),
            "a set nobody asked for would override the graph's own value"
        );
    }

    /// The conversion is what narrows the plan's `Option<u32>` to a budget a
    /// dispatch can run under, and it is checked: the value a dispatch cannot
    /// honour is refused here rather than represented and carried.
    #[test]
    fn the_checked_conversion_keeps_a_positive_budget_and_refuses_zero() {
        assert_eq!(
            NodeControls::of_node(&node(Some(45)))
                .expect("45 converts")
                .max_turns,
            NonZeroU32::new(45)
        );
        assert_eq!(
            NodeControls::of_step(&step(Some(45)))
                .expect("45 converts")
                .max_turns,
            NonZeroU32::new(45)
        );

        for refused in [
            NodeControls::of_node(&node(Some(0))).expect_err("a node cannot run for no turns"),
            NodeControls::of_step(&step(Some(0))).expect_err("a step cannot run for no turns"),
        ] {
            assert!(refused.contains("no turn at all"), "{refused}");
            assert!(
                refused.contains("omit it"),
                "the refusal does not say what to do instead: {refused}"
            );
        }
    }

    #[test]
    fn a_control_with_nowhere_to_land_is_refused_by_name_rather_than_dropped() {
        // The shape a future control takes when this crate accepts it and has no
        // override for it. `done_when` was exactly this and said nothing at all
        // for the whole of its life.
        let refused = rendered(vec![Control::Unappliable("someday")])
            .expect_err("a control with no override cannot be applied");
        assert!(refused.contains("someday"), "{refused}");
        assert!(
            refused.contains("default nobody asked for"),
            "the refusal does not say what the silence would have cost: {refused}"
        );
    }
}
