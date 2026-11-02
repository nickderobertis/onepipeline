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
    // llmlint: ignore-block[invalid_states_unrepresentable] the task's ticket shape is `{id: string, url: string, priority}`, arriving as a phase's JSON context; the decoder refuses blank ids and URLs, `expand` refuses them again for a context built in process, and the review prints them as given.
    /// The ticket's id; the last occurrence of an id wins.
    #[serde(deserialize_with = "nonblank")]
    pub id: String,
    /// Where a person opens it.
    #[serde(deserialize_with = "nonblank")]
    pub url: String,
    // llmlint: ignore-end[invalid_states_unrepresentable]
    /// Its priority, absent or `none` when it has no usable one.
    #[serde(default)]
    #[schemars(schema_with = "ticket_priority_schema")]
    pub priority: Option<Priority>,
    /// Its title, the text the review links it by; absent, it is linked by its
    /// short repository reference. Optional and omitted when absent, so a context
    /// written before it existed is read unchanged.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "nonblank_title"
    )]
    #[schemars(length(min = 1), pattern(r"\S"))]
    // llmlint: ignore[invalid_states_unrepresentable] the task adds an optional `title` string to the context's `{id, url, priority}` ticket, beside its `String` id and url; the decoder refuses a blank one, `expand` refuses it again for a context built in process, and absence is the legacy shape a reference link stands in for.
    pub title: Option<String>,
}

impl Ticket {
    /// The ticket as one markdown link: its title, or else its short repository
    /// reference, linking its URL.
    pub fn link(&self) -> String {
        let text = match &self.title {
            Some(title) => title.trim().to_owned(),
            None => short_reference(&self.url).unwrap_or_else(|| self.url.trim().to_owned()),
        };
        let mut escaped = String::with_capacity(text.len());
        for c in text.chars() {
            if matches!(c, '\\' | '[' | ']') {
                escaped.push('\\');
            }
            escaped.push(c);
        }
        let url = self.url.trim();
        if url.contains(|c: char| c.is_whitespace() || c == '(' || c == ')') {
            format!("[{escaped}](<{url}>)")
        } else {
            format!("[{escaped}]({url})")
        }
    }
}

/// `<repo>#<number>` for a URL whose path ends in a number under a repository
/// (`…/owner/repo/issues/12`, `…/group/repo/-/merge_requests/3`); `None` when
/// its path does not.
pub fn short_reference(url: &str) -> Option<String> {
    let path = url.trim().split_once("://").map_or(url, |(_, rest)| rest);
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let segments: Vec<&str> = path.split('/').skip(1).filter(|s| !s.is_empty()).collect();
    let (number, rest) = segments.split_last()?;
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // The segment naming what the number counts, then the repository before it.
    let (_, before) = rest.split_last()?;
    let repo = before.iter().rev().find(|s| **s != "-")?;
    Some(format!("{repo}#{number}"))
}

fn nonblank_title<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    match Option::<String>::deserialize(d)? {
        Some(title) if title.trim().is_empty() => Err(serde::de::Error::custom(
            "a ticket's title, when given, is not blank",
        )),
        title => Ok(title),
    }
}

/// A ticket's priority as the store spells every value, `none` and absence alike
/// meaning it has no usable one.
fn ticket_priority_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    let values: Vec<_> = std::iter::once(serde_json::Value::Null)
        .chain(
            Priority::ALL
                .into_iter()
                .map(|p| serde_json::json!(p.as_str())),
        )
        .collect();
    schemars::json_schema!({"type": ["string", "null"], "enum": values})
}

fn nonblank<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let value = String::deserialize(d)?;
    if value.trim().is_empty() {
        return Err(serde::de::Error::custom(
            "a ticket names a nonblank id and url",
        ));
    }
    Ok(value)
}

/// The version of the phase context's contract a writer wrote against.
///
/// Version 1 is the context as first published, and is what a context without
/// `schema_version` is read as; version 2 adds a ticket's optional `title`. A
/// reader refuses any other number rather than guessing what it adds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PhaseContextVersion {
    /// The context as first published: tickets are `{id, url, priority}`.
    V1,
    /// Tickets may carry a `title`.
    V2,
}

impl PhaseContextVersion {
    /// The version this onepipeline writes.
    pub const CURRENT: Self = Self::V2;
    /// Every version this onepipeline reads, oldest first.
    pub const SUPPORTED: [Self; 2] = [Self::V1, Self::V2];

    /// The version as it is spelled on the wire.
    pub fn number(self) -> u32 {
        match self {
            Self::V1 => 1,
            Self::V2 => 2,
        }
    }
}

impl Serialize for PhaseContextVersion {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u32(self.number())
    }
}

impl<'de> Deserialize<'de> for PhaseContextVersion {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let number = u64::deserialize(d)?;
        Self::SUPPORTED
            .into_iter()
            .find(|v| u64::from(v.number()) == number)
            .ok_or_else(|| {
                serde::de::Error::custom(format!(
                    "unsupported phase context schema_version {number}; this onepipeline reads 1 and 2"
                ))
            })
    }
}

impl JsonSchema for PhaseContextVersion {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "PhaseContextVersion".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "The phase context's contract version: 1, the context as first published and what an absent `schema_version` means, or 2, which adds a ticket's optional `title`.",
            "type": "integer",
            "enum": Self::SUPPORTED.map(Self::number),
        })
    }
}

/// A present `schema_version` is a supported number; `null` is not absence.
fn present_version<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<PhaseContextVersion>, D::Error> {
    PhaseContextVersion::deserialize(d).map(Some)
}

/// A context below version 2 names no ticket title: the published schema says
/// so as a conditional beside the properties the derive writes.
fn titles_need_version_2(schema: &mut schemars::Schema) {
    schema.insert(
        "if".into(),
        serde_json::json!({
            "not": {"required": ["schema_version"], "properties": {"schema_version": {"const": 2}}}
        }),
    );
    schema.insert(
        "then".into(),
        serde_json::json!({
            "properties": {"tickets": {"items": {"not": {"required": ["title"]}}}}
        }),
    );
}

/// What a phase supplies when it raises.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = titles_need_version_2)]
pub struct PhaseContext {
    /// The contract version the context was written against; absent, it is a
    /// version 1 context, and it is omitted again when written back.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_version"
    )]
    #[schemars(with = "PhaseContextVersion")]
    pub schema_version: Option<PhaseContextVersion>,
    /// Who owns the item.
    pub owner: HumanItemOwner,
    /// The phase's own identity for this occurrence; the item's dedup key is
    /// derived from it and the phase.
    // llmlint: ignore[invalid_states_unrepresentable] the task names the phase context's `owner` and `dedup_key` as its supplied identity; `expand` refuses a blank one before raising, and the resulting item is validated as any raise is.
    pub dedup_key: String,
    /// The node a work hold binds, for a hook whose block is `work`.
    #[serde(default)]
    // llmlint: ignore[invalid_states_unrepresentable] the phase context is the task's data-only `{owner, dedup_key, ...}`; the node it names becomes a work block, which the raise validates as nonblank like every block, and which names a node of the owner's run or holds nothing.
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

impl PhaseContext {
    /// The version the context is read as: its `schema_version`, or 1 when it
    /// names none.
    pub fn version(&self) -> PhaseContextVersion {
        self.schema_version.unwrap_or(PhaseContextVersion::V1)
    }

    /// Refuse what the context's version does not carry: a ticket title in a
    /// version 1 context.
    pub fn validate(&self) -> Result<(), String> {
        if self.version() < PhaseContextVersion::V2
            && self.tickets.iter().any(|t| t.title.is_some())
        {
            return Err(
                "a ticket's title is phase context schema_version 2; write \"schema_version\": 2"
                    .into(),
            );
        }
        Ok(())
    }
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

/// The ticket links grouped by priority, the ones with none in their own group;
/// each is a markdown link named by the ticket's title, never its board id.
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
            out.push_str(&format!("- {}\n", ticket.link()));
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
    if hook.kind == HumanItemKind::Question && (hook.approval.is_some() || hook.aggregate.is_some())
    {
        return Err(format!(
            "phase {phase} is a question, which carries no approval and no aggregation"
        ));
    }
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
        (Some(_), Some(approval))
            if approval.board_copy.trim().is_empty() || approval.key.trim().is_empty() =>
        {
            return Err(format!(
                "phase {phase}'s document version names a nonblank board_copy and key"
            ))
        }
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
    if hook.priority == Priority::None {
        return Err(format!("phase {phase} states no usable priority"));
    }
    if context
        .tickets
        .iter()
        .any(|t| t.id.trim().is_empty() || t.url.trim().is_empty())
    {
        return Err("a ticket names a nonblank id and url".into());
    }
    if context.tickets.iter().any(|t| {
        t.title
            .as_ref()
            .is_some_and(|title| title.trim().is_empty())
    }) {
        return Err("a ticket's title, when given, is not blank".into());
    }
    context.validate()?;
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
                        .map(|t| format!("- {}", t.link()))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_reference_names_the_repository_and_number() {
        for (url, expected) in [
            (
                "https://github.com/acme/widgets/issues/12",
                Some("widgets#12"),
            ),
            (
                "https://github.com/acme/widgets/pull/40/",
                Some("widgets#40"),
            ),
            (
                "https://github.com/acme/widgets/issues/12?x=1#c",
                Some("widgets#12"),
            ),
            (
                "https://gitlab.example/group/gadgets/-/merge_requests/3",
                Some("gadgets#3"),
            ),
            ("https://tracker/t-1", None),
            ("https://tracker/12", None),
            ("https://github.com/acme/widgets", None),
        ] {
            assert_eq!(short_reference(url).as_deref(), expected, "{url}");
        }
    }

    #[test]
    fn a_link_escapes_its_text_and_brackets_an_awkward_url() {
        let ticket = |title: Option<&str>, url: &str| Ticket {
            id: "board-1".into(),
            url: url.into(),
            priority: None,
            title: title.map(Into::into),
        };
        assert_eq!(
            ticket(Some("Fix [it] \\ now"), "https://x/y").link(),
            "[Fix \\[it\\] \\\\ now](https://x/y)"
        );
        assert_eq!(
            ticket(None, "https://x/a (b)").link(),
            "[https://x/a (b)](<https://x/a (b)>)"
        );
        assert_eq!(
            ticket(None, "https://github.com/o/r/issues/5").link(),
            "[r#5](https://github.com/o/r/issues/5)"
        );
    }
}
