//! The generic phase facility: a configured hook, expanded once against the
//! context its phase supplies, raises an ordinary item — or, for a
//! `ticket_review` hook, one review task over a set of tickets.
//!
//! The context is data, never instructions: it names the owner, the dedup key,
//! template variables, an optional node, an optional document version and the
//! tickets. Which approval authority a completion runs is the hook's, chosen by
//! configuration; nothing a context or a raised item carries can name a command.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::config::{EffectiveConfig, HookAggregate, HookBlock, HumanItemHook};
use super::{
    priority_label, HumanItemApproval, HumanItemBlock, HumanItemInput, HumanItemKind,
    HumanItemOwner, Priority,
};

/// The dedup-key prefix every phase-raised item carries, followed by the phase.
pub const PHASE_PREFIX: &str = "phase:";

/// One ticket a review phase aggregates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Ticket {
    /// The ticket's id; the last occurrence of an id wins.
    pub id: String,
    /// Where a person opens it.
    pub url: String,
    /// Its priority, absent or `none` when it has no usable one.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub priority: Option<Priority>,
}

/// What a phase supplies when it raises.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PhaseContext {
    /// Who owns the item.
    pub owner: HumanItemOwner,
    /// The phase's own identity for this occurrence; the item's dedup key is
    /// derived from it and the phase.
    pub dedup_key: String,
    /// The node a work hold binds, for a hook whose block is `work`.
    #[serde(default)]
    pub node: Option<String>,
    /// Template variables for the title and body.
    #[serde(default)]
    pub vars: BTreeMap<String, String>,
    /// The document version a design review completes against.
    #[serde(default)]
    pub approval: Option<HumanItemApproval>,
    /// The tickets a `ticket_review` hook aggregates.
    #[serde(default)]
    pub tickets: Vec<Ticket>,
}

/// What a phase raise amounts to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expanded {
    /// Raise (or update) this item, and these companion findings.
    Raise {
        /// The item.
        item: Box<HumanItemInput>,
        /// Companion items, such as a finding naming tickets without a priority.
        findings: Vec<HumanItemInput>,
        /// Dedup keys of companions that no longer apply and are to be dismissed.
        retired: Vec<(String, String)>,
    },
    /// Nothing to raise; dismiss what an earlier occurrence raised, if open.
    Retire(Vec<(String, String)>),
}

/// The phase a phase-raised dedup key names.
pub fn phase_of(dedup_key: &str) -> Option<&str> {
    dedup_key.strip_prefix(PHASE_PREFIX)?.split(':').next()
}

fn render(template: &str, context: &PhaseContext) -> Result<String, String> {
    let mut vars: BTreeMap<String, String> = context.vars.clone();
    vars.insert("session".into(), context.owner.session.clone());
    if let Some(run) = &context.owner.run {
        vars.insert("run".into(), run.clone());
    }
    if let Some(plan) = &context.owner.plan {
        vars.insert("plan".into(), plan.clone());
    }
    if let Some(node) = &context.node {
        vars.insert("node".into(), node.clone());
    }
    let mut env = minijinja::Environment::new();
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
    env.render_str(template, vars)
        .map_err(|e| format!("the template does not expand against this context: {e}"))
}

/// The priority a set of review tickets derives: any urgent is urgent;
/// otherwise three or more high is high; otherwise only lows plus at most one
/// medium is low; otherwise medium. A ticket with no usable priority counts as
/// medium.
pub fn derive_priority(tickets: &[Ticket]) -> Priority {
    let effective = |t: &Ticket| match t.priority {
        Some(Priority::None) | None => Priority::Medium,
        Some(p) => p,
    };
    let count = |p: Priority| tickets.iter().filter(|t| effective(t) == p).count();
    if count(Priority::Urgent) > 0 {
        Priority::Urgent
    } else if count(Priority::High) >= 3 {
        Priority::High
    } else if count(Priority::High) == 0 && count(Priority::Medium) <= 1 {
        Priority::Low
    } else {
        Priority::Medium
    }
}

/// The tickets deduplicated by id, the last occurrence's priority winning, in
/// first-seen order.
pub fn deduplicated(tickets: &[Ticket]) -> Vec<Ticket> {
    let mut order: Vec<String> = Vec::new();
    let mut last: BTreeMap<String, Ticket> = BTreeMap::new();
    for ticket in tickets {
        if !last.contains_key(&ticket.id) {
            order.push(ticket.id.clone());
        }
        last.insert(ticket.id.clone(), ticket.clone());
    }
    order
        .into_iter()
        .filter_map(|id| last.remove(&id))
        .collect()
}

/// The ticket links grouped by priority, the ones with none in their own group.
fn grouped(tickets: &[Ticket]) -> String {
    let mut out = String::new();
    for (heading, wanted) in [
        (priority_label(Priority::Urgent), Some(Priority::Urgent)),
        (priority_label(Priority::High), Some(Priority::High)),
        (priority_label(Priority::Medium), Some(Priority::Medium)),
        (priority_label(Priority::Low), Some(Priority::Low)),
        ("No priority", None),
    ] {
        let group: Vec<&Ticket> = tickets
            .iter()
            .filter(|t| t.priority.filter(|p| *p != Priority::None) == wanted)
            .collect();
        if group.is_empty() {
            continue;
        }
        out.push_str(&format!("\n{heading}\n"));
        for ticket in group {
            out.push_str(&format!("- {}: {}\n", ticket.id, ticket.url));
        }
    }
    out
}

/// Expand one hook against its context, validated once here at the engine
/// boundary.
///
/// # Errors
///
/// An unconfigured phase, a context the hook cannot expand against, or an
/// expansion that is not a valid item.
pub fn expand(
    config: &EffectiveConfig,
    phase: &str,
    context: &PhaseContext,
) -> Result<Expanded, String> {
    let hook: &HumanItemHook = config
        .hook(phase)
        .ok_or_else(|| format!("no hook is configured for phase {phase}"))?;
    if context.dedup_key.trim().is_empty() || context.owner.session.trim().is_empty() {
        return Err("a phase context names its owner session and dedup_key".into());
    }
    let dedup_key = format!("{PHASE_PREFIX}{phase}:{}", context.dedup_key);
    let block = match hook.block {
        HookBlock::Nothing => HumanItemBlock::Nothing,
        HookBlock::Host => HumanItemBlock::Host,
        HookBlock::Plan => HumanItemBlock::Plan {
            plan: context
                .owner
                .plan
                .clone()
                .ok_or("a plan hold needs the owner's plan")?,
        },
        HookBlock::Work => HumanItemBlock::Work {
            run: context
                .owner
                .run
                .clone()
                .ok_or("a work hold needs the owner's run")?,
            node: context
                .node
                .clone()
                .ok_or("a work hold needs the context's node")?,
        },
    };
    let approval = match (&hook.approval, &context.approval) {
        (Some(_), Some(approval)) => Some(approval.clone()),
        (Some(_), None) => {
            return Err(format!(
                "phase {phase} completes through an approval authority, and its context names \
                 no document version"
            ))
        }
        (None, Some(_)) => {
            return Err(format!(
                "phase {phase} configures no approval adapter, so its context may not carry an \
                 approval reference"
            ))
        }
        (None, None) => None,
    };
    let title = render(&hook.title, context)?;
    let mut body = render(&hook.body, context)?;
    let mut priority = hook.priority;
    let mut findings = Vec::new();
    let mut retired = Vec::new();
    let finding_key = format!("{dedup_key}:no-priority");
    if hook.aggregate == Some(HookAggregate::TicketReview) {
        let tickets = deduplicated(&context.tickets);
        if tickets.is_empty() {
            return Ok(Expanded::Retire(vec![
                (dedup_key, "no tickets remain to review".into()),
                (finding_key, "no tickets remain to review".into()),
            ]));
        }
        priority = derive_priority(&tickets);
        body = format!("{}\n{}", body.trim_end(), grouped(&tickets));
        let unknown: Vec<&Ticket> = tickets
            .iter()
            .filter(|t| t.priority.is_none_or(|p| p == Priority::None))
            .collect();
        if unknown.is_empty() {
            retired.push((finding_key, "every review ticket has a priority".into()));
        } else {
            findings.push(HumanItemInput {
                schema_version: 1,
                dedup_key: finding_key,
                kind: HumanItemKind::Task,
                title: format!("Set a priority on {} review ticket(s)", unknown.len()),
                body: format!(
                    "These tickets have no priority, so the review counts each as Medium:\n{}",
                    unknown
                        .iter()
                        .map(|t| format!("- {}: {}", t.id, t.url))
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
                priority: Priority::Low,
                owner: context.owner.clone(),
                block: HumanItemBlock::Nothing,
                approval: None,
            });
        }
    }
    let item = HumanItemInput {
        schema_version: 1,
        dedup_key,
        kind: hook.kind,
        title,
        body,
        priority,
        owner: context.owner.clone(),
        block,
        approval,
    };
    item.validate()
        .map_err(|e| format!("phase {phase} expands to an invalid item: {e}"))?;
    Ok(Expanded::Raise {
        item: Box::new(item),
        findings,
        retired,
    })
}
