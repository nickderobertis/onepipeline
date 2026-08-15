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
//! How a future control is kept honest: `NodeControls::declared` destructures
//! the struct field by field with **no `..` rest pattern**, so a control added
//! to [`NodeControls`] does not compile until this module says what it applies
//! to. A control whose honest answer is "nothing this crate can apply" is given
//! `set: None` there, and the renderer beneath it refuses that control by name.

use crate::plan::{Node, Step};

/// The node-scope graph member a node's work runs as, and therefore the member
/// every per-node override addresses.
///
/// The shipped `graphs/node-scope.yaml` names it, and a graph a plan substitutes
/// with `agent_graph` is expected to name its worker the same — which is the
/// assumption the persona override has always made.
pub(crate) const WORKER_MEMBER: &str = "worker";

/// The controls one node or step declares.
///
/// Copied out of the plan rather than borrowed from it, because a dispatch
/// outlives the borrow of the graph it was built from.
// llmlint: ignore-block[invalid_states_unrepresentable] `max_turns` is `Option<u32>`
// rather than a positive-integer type because it *mirrors* the plan field, which is
// `Option<u32>` for the reason `plan.rs` records: `docs/contract.md` fixes the node shapes
// as schema v7's, and a live edit merges arbitrary submitted JSON into one. So the zero is
// representable at the boundary whatever this type says, and narrowing here alone would
// only move it — while making `of_node`/`of_step` fallible would put an error arm no
// validated plan can reach on every dispatch path. It is *rejected* instead, in
// `overrides` below, which is the one function both plan validation and launch
// composition call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct NodeControls {
    /// The dispatch's turn budget, applied as the worker member's `max_turns`.
    pub max_turns: Option<u32>,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// One declared control: the name a plan writes, and the override that applies
/// it — or `None` for one this build accepts and cannot apply.
struct Control {
    /// The plan field's own name, which is what a refusal has to say.
    name: &'static str,
    /// The `PATH=VALUE` this control renders to, and `None` where there is none.
    set: Option<String>,
}

impl NodeControls {
    /// What a node declares.
    pub fn of_node(node: &Node) -> Self {
        Self {
            max_turns: node.max_turns,
        }
    }

    /// What one step of a lifecycle node declares.
    pub fn of_step(step: &Step) -> Self {
        Self {
            max_turns: step.max_turns,
        }
    }

    /// The `--set` overrides these controls render to, or the reason the first
    /// one that cannot be applied is refused.
    ///
    /// The refusal is a plain message rather than an [`Error`](crate::Error) so
    /// that each caller says *where* it was refused — a plan node at validation,
    /// or a dispatch at launch — without a second error prefix inside the
    /// sentence.
    pub fn overrides(&self) -> std::result::Result<Vec<String>, String> {
        // A budget of zero is a value no dispatch can honour: it lets the member
        // take no turn at all, which is what `oneagentgraph` refuses when it
        // validates the graph it has been handed. Refused here as well, where the
        // plan is read, so a planner is told before a launch is composed rather
        // than by a member that could not start.
        if self.max_turns == Some(0) {
            return Err(
                "`max_turns: 0` lets the dispatch take no turn at all; omit it to \
                        run under the agent graph's own ceiling"
                    .to_string(),
            );
        }
        rendered(self.declared())
    }

    /// Every control declared here, paired with what applies it.
    fn declared(&self) -> Vec<Control> {
        // Destructured with no `..` rest pattern on purpose: a control added to
        // `NodeControls` does not compile until it is given a line below. One
        // this crate cannot apply is written `set: None` — accepted by the
        // schema, refused at validation, never silently dropped.
        let Self { max_turns } = self;
        let mut declared = Vec::new();
        if let Some(budget) = max_turns {
            // `OnejudgeMember::max_turns` is the sibling's own turn ceiling for
            // the two-party conversation a node's dispatch is, so the plan's
            // budget is that field and needs no merge layer here.
            declared.push(Control {
                name: "max_turns",
                set: Some(format!("members.{WORKER_MEMBER}.max_turns={budget}")),
            });
        }
        declared
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
            control.set.ok_or_else(|| {
                format!(
                    "`{}` is a control this build accepts and cannot apply to a dispatch, \
                     so the dispatch would silently run under a default nobody asked for",
                    control.name
                )
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_declared_turn_budget_renders_as_the_workers_own_override() {
        let controls = NodeControls {
            max_turns: Some(45),
        };
        assert_eq!(
            controls.overrides().expect("a budget is appliable"),
            vec!["members.worker.max_turns=45".to_string()]
        );
    }

    #[test]
    fn a_node_that_declares_no_control_overrides_nothing() {
        assert_eq!(
            NodeControls::default()
                .overrides()
                .expect("nothing to apply"),
            Vec::<String>::new(),
            "a set nobody asked for would override the graph's own value"
        );
    }

    #[test]
    fn a_turn_budget_of_zero_is_refused_where_the_plan_is_read() {
        let refused = NodeControls { max_turns: Some(0) }
            .overrides()
            .expect_err("a dispatch cannot run for no turns");
        assert!(refused.contains("no turn at all"), "{refused}");
        assert!(
            refused.contains("omit it"),
            "the refusal does not say what to do instead: {refused}"
        );
    }

    #[test]
    fn a_control_with_nowhere_to_land_is_refused_by_name_rather_than_dropped() {
        // The shape a future control takes when this crate accepts it and has no
        // override for it — what `declared` writes as `set: None`. `done_when`
        // was exactly this and said nothing at all for the whole of its life.
        let refused = rendered(vec![Control {
            name: "someday",
            set: None,
        }])
        .expect_err("a control with no override cannot be applied");
        assert!(refused.contains("someday"), "{refused}");
        assert!(
            refused.contains("default nobody asked for"),
            "the refusal does not say what the silence would have cost: {refused}"
        );
    }
}
