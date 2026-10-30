//! Human attention records and their version-one wire contract.
mod approval;
pub mod config;
mod delivery;
mod durable;
mod live;
pub mod phases;
pub(crate) mod runtime;
pub use approval::REREAD;
pub use live::{item_id_for, Actor, HumanItems, ReadMode, ReadTelemetry, RUN_ENDED, TELEMETRY_ENV};
/// The store priority vocabulary; human-item boundaries reject None.
pub use onetaskgraph_plugin_api::Priority;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The human-item journal relative to the engine home (the runs root).
pub const JOURNAL_PATH: &str = "human-items/events.ndjson";

fn nonempty<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let value = String::deserialize(d)?;
    if value.trim().is_empty() {
        return Err(serde::de::Error::custom("must not be blank"));
    }
    Ok(value)
}
fn priority<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Priority, D::Error> {
    let value = Priority::deserialize(d)?;
    if value == Priority::None {
        return Err(serde::de::Error::custom("human items require a priority"));
    }
    Ok(value)
}
fn priority_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    let values: Vec<_> = Priority::ALL
        .into_iter()
        .filter(|p| *p != Priority::None)
        .map(Priority::as_str)
        .collect();
    schemars::json_schema!({"type":"string","enum":values})
}
fn version<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    match u32::deserialize(d)? {
        1 => Ok(1),
        _ => Err(serde::de::Error::custom("expected schema_version 1")),
    }
}

// llmlint: ignore-block[invalid_states_unrepresentable] The approved human-item contract publishes flat records with these fields and imports onetaskgraph Priority, whose None variant cannot be removed here. Replacing those public fields with private constructors or a state enum changes that surface; deserialization and sample action boundaries validate the combinations instead.
macro_rules! validated_record {
    ($(#[$meta:meta])* pub struct $name:ident, $wire:ident { $( $(#[$field_meta:meta])* pub $field:ident: $ty:ty, )* }) => {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
        $(#[$meta])*
        #[serde(deny_unknown_fields)]
        pub struct $name { $( $(#[$field_meta])* pub $field: $ty, )* }
        #[derive(Deserialize, JsonSchema)]
        #[serde(deny_unknown_fields)]
        struct $wire { $( $(#[$field_meta])* $field: $ty, )* }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
                let wire = $wire::deserialize(decoder)?;
                let value = Self { $( $field: wire.$field, )* };
                value.validate().map_err(serde::de::Error::custom)?;
                Ok(value)
            }
        }
    };
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// The human item kind.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, clap::ValueEnum,
)]
#[serde(rename_all = "kebab-case")]
pub enum HumanItemKind {
    /// Task.
    Task,
    /// Question.
    Question,
}

/// The human item state.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, clap::ValueEnum,
)]
#[serde(rename_all = "kebab-case")]
pub enum HumanItemState {
    /// Open.
    Open,
    /// Answered.
    Answered,
    /// Completed.
    Completed,
    /// Dismissed.
    Dismissed,
}

/// The originating session and optional run and plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HumanItemOwner {
    /// Session.
    pub session: String,
    /// Run.
    #[serde(deserialize_with = "required_option")]
    #[schemars(required, schema_with = "nullable::<String>")]
    pub run: Option<String>,
    /// Plan.
    #[serde(deserialize_with = "required_option")]
    #[schemars(required, schema_with = "nullable::<String>")]
    pub plan: Option<String>,
}

/// The scope held by an open item; never a presentation filter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "scope", rename_all = "kebab-case", deny_unknown_fields)]
pub enum HumanItemBlock {
    /// Holds nothing.
    Nothing,
    /// Holds work in a run.
    Work {
        /// Run identifier.
        run: String,
        /// Node identifier.
        node: String,
    },
    /// Holds a whole plan.
    Plan {
        /// Plan identifier.
        plan: String,
    },
    /// Holds all work on the host.
    Host,
}

/// Who resolved an item and what they said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HumanItemResolution {
    /// Actor.
    pub actor: String,
    /// On behalf of.
    #[serde(deserialize_with = "required_option")]
    #[schemars(required, schema_with = "nullable::<String>")]
    pub on_behalf_of: Option<String>,
    /// Text.
    #[serde(deserialize_with = "required_option")]
    #[schemars(required, schema_with = "nullable::<String>")]
    pub text: Option<String>,
    /// Reason.
    #[serde(deserialize_with = "required_option")]
    #[schemars(required, schema_with = "nullable::<String>")]
    pub reason: Option<String>,
}

/// An opaque approval authority reference captured when raised.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HumanItemApproval {
    /// Board copy.
    pub board_copy: String,
    /// Key.
    pub key: String,
}

/// An associated tracker task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HumanItemTracker {
    /// Task.
    pub task: String,
    /// Url.
    pub url: String,
}

// llmlint: ignore-block[invalid_states_unrepresentable] The approved Raise input publishes schema_version as a number, title/body as strings, imported Priority and an optional approval reference on a flat record. A different public constructor or a local priority enum would depart from that interface; the decoder and apply boundary check version, nonempty prose, priority and question/approval combinations.
validated_record! {
/// Immutable fields supplied when raising a human item.
#[schemars(extend("allOf" = input_constraints()))]
pub struct HumanItemInput, HumanItemInputWire {
    /// The immutable input schema version.
    #[serde(deserialize_with = "version")]
    #[schemars(range(min = 1, max = 1))]
    pub schema_version: u32,
    /// Dedup key.
    pub dedup_key: String,
    /// Kind.
    pub kind: HumanItemKind,
    /// Title.
    #[serde(deserialize_with = "nonempty")]
    #[schemars(length(min = 1), pattern(r"\S"))]
    pub title: String,
    /// Body.
    #[serde(deserialize_with = "nonempty")]
    #[schemars(length(min = 1), pattern(r"\S"))]
    pub body: String,
    /// Priority.
    #[serde(deserialize_with = "priority")]
    #[schemars(schema_with = "priority_schema")]
    pub priority: Priority,
    /// Owner.
    pub owner: HumanItemOwner,
    /// Block.
    pub block: HumanItemBlock,
    /// Approval.
#[serde(deserialize_with = "required_option")]
    #[schemars(required, schema_with = "nullable::<HumanItemApproval>")]
    pub approval: Option<HumanItemApproval>,
}
}

// llmlint: ignore-end[invalid_states_unrepresentable]

// llmlint: ignore-block[invalid_states_unrepresentable] The agreed optional metadata is this flat kind/priority/title/body record using imported Priority. Its loader boundary rejects blank prose and None; introducing new string or priority wrapper types would change the prescribed domain surface.
/// Optional onepipeline.human_item node metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HumanItemMetadata {
    /// Kind.
    pub kind: HumanItemKind,
    /// Priority.
    #[serde(deserialize_with = "priority")]
    #[schemars(schema_with = "priority_schema")]
    pub priority: Priority,
    /// Title.
    #[serde(deserialize_with = "nonempty")]
    #[schemars(length(min = 1), pattern(r"\S"))]
    pub title: String,
    /// Body.
    #[serde(deserialize_with = "nonempty")]
    #[schemars(length(min = 1), pattern(r"\S"))]
    pub body: String,
}

// llmlint: ignore-end[invalid_states_unrepresentable]

// llmlint: ignore-block[invalid_states_unrepresentable] HumanItem v1 fixes kind, state, resolution and approval as independent wire fields and imports the store Priority. Keep that declared public shape; the boundary decoder checks state/resolution/kind/approval combinations and rejects None priority.
validated_record! {
/// A stable host-unique human item at schema version one.
#[schemars(extend("allOf" = item_constraints()))]
pub struct HumanItem, HumanItemWire {
    /// Schema version.
    #[serde(deserialize_with = "version")]
    #[schemars(range(min = 1, max = 1))]
    pub schema_version: u32,
    /// Id.
    pub id: String,
    /// Dedup key.
    pub dedup_key: String,
    /// Kind.
    pub kind: HumanItemKind,
    /// Title.
    #[serde(deserialize_with = "nonempty")]
    #[schemars(length(min = 1), pattern(r"\S"))]
    pub title: String,
    /// Body.
    #[serde(deserialize_with = "nonempty")]
    #[schemars(length(min = 1), pattern(r"\S"))]
    pub body: String,
    /// Priority.
    #[serde(deserialize_with = "priority")]
    #[schemars(schema_with = "priority_schema")]
    pub priority: Priority,
    /// Owner.
    pub owner: HumanItemOwner,
    /// Block.
    pub block: HumanItemBlock,
    /// Approval.
#[serde(deserialize_with = "required_option")]
    #[schemars(required, schema_with = "nullable::<HumanItemApproval>")]
    pub approval: Option<HumanItemApproval>,
    /// State.
    pub state: HumanItemState,
    /// Resolution.
#[serde(deserialize_with = "required_option")]
    #[schemars(required, schema_with = "nullable::<HumanItemResolution>")]
    pub resolution: Option<HumanItemResolution>,
    /// Tracker.
#[serde(deserialize_with = "required_option")]
    #[schemars(required, schema_with = "nullable::<HumanItemTracker>")]
    pub tracker: Option<HumanItemTracker>,
    /// Created at.
    pub created_at: String,
    /// Updated at.
    pub updated_at: String,
}
}

// llmlint: ignore-end[invalid_states_unrepresentable]

impl HumanItem {
    /// Whether this item currently holds work, derived solely from state and scope.
    pub fn is_blocking(&self) -> bool {
        self.state == HumanItemState::Open && self.block != HumanItemBlock::Nothing
    }
}
/// The authoritative human-item action vocabulary. Tasks use Complete, including design reviews.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "kebab-case", deny_unknown_fields)]
pub enum HumanItemAction {
    /// Raise an item with a required deduplication key.
    Raise(HumanItemInput),
    /// Answer an open item.
    Answer {
        /// Id.
        id: String,
        /// Text.
        #[serde(deserialize_with = "nonempty")]
        #[schemars(length(min = 1), pattern(r"\S"))]
        // llmlint: ignore[invalid_states_unrepresentable] The authoritative Answer action declares text as a JSON string on its flat public variant. Its shared nonempty decoder and apply boundary reject blank answers without introducing another public text type.
        text: String,
        /// Actor.
        actor: String,
        /// On behalf of.
        #[serde(deserialize_with = "required_option")]
        #[schemars(required, schema_with = "nullable::<String>")]
        on_behalf_of: Option<String>,
    },
    /// Complete an open item.
    Complete {
        /// Id.
        id: String,
        /// Actor.
        actor: String,
        /// On behalf of.
        #[serde(deserialize_with = "required_option")]
        #[schemars(required, schema_with = "nullable::<String>")]
        on_behalf_of: Option<String>,
    },
    /// Dismiss an open item.
    Dismiss {
        /// Id.
        id: String,
        /// Reason.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// Actor.
        actor: String,
        /// On behalf of.
        #[serde(deserialize_with = "required_option")]
        #[schemars(required, schema_with = "nullable::<String>")]
        on_behalf_of: Option<String>,
    },
}

/// Query selectors; omitted state selects open items.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[derive(Default, clap::Args)]
pub struct HumanItemFilter {
    /// Kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[arg(long, value_enum)]
    pub kind: Option<HumanItemKind>,
    /// Blocking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[arg(long, value_parser = parse_blocking)]
    pub blocking: Option<bool>,
    /// Priority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(deserialize_with = "optional_priority")]
    #[schemars(schema_with = "optional_priority_schema")]
    #[arg(long, value_parser = parse_priority)]
    // llmlint: ignore[invalid_states_unrepresentable] The contract requires an optional imported Priority selector; the JSON boundary rejects Some(None) without redeclaring the sibling vocabulary.
    pub priority: Option<Priority>,
    /// State.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[arg(long, value_enum)]
    pub state: Option<HumanItemState>,
    /// Session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[arg(long)]
    pub session: Option<String>,
    /// Run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[arg(long)]
    pub run: Option<String>,
    /// Plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[arg(long)]
    pub plan: Option<String>,
    /// Host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    pub host: Option<bool>,
}

/// An affected step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HumanItemNode {
    /// Run.
    pub run: String,
    /// Node.
    pub node: String,
    /// Title.
    pub title: String,
}

// llmlint: ignore-block[invalid_states_unrepresentable] The declared plan impact is a flat plan/count/preview record. Its containing view/detail decoders validate count/preview agreement and the detail checks the first-three partition, without replacing the public primitive fields.
/// A plan affected by an item; preview contains its first three nodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HumanItemBlockedPlan {
    /// Plan.
    pub plan: String,
    /// Count.
    pub count: u64,
    /// Preview.
    #[serde(deserialize_with = "preview")]
    #[schemars(length(max = 3))]
    pub preview: Vec<HumanItemNode>,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

// llmlint: ignore-block[invalid_states_unrepresentable] The contract publishes total and per-plan counts as u64 fields. Their sum is validated by the view/detail boundary; a private counted collection would replace the prescribed record.
/// Affected node counts grouped by plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HumanItemBlocked {
    /// Total.
    pub total: u64,
    /// Plans.
    pub plans: Vec<HumanItemBlockedPlan>,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

// llmlint: ignore-block[invalid_states_unrepresentable] The public view is the declared item/blocked record. Its boundary validates blocking state and totals; public Rust fields retain that fixed shape rather than introducing another state-bearing view enum.
validated_record! {
/// An item and its derived impact.
pub struct HumanItemView, HumanItemViewWire {
    /// Item.
    pub item: HumanItem,
    /// Blocked.
    pub blocked: HumanItemBlocked,
}
}
// llmlint: ignore-end[invalid_states_unrepresentable]

// llmlint: ignore-block[invalid_states_unrepresentable] The detail contract publishes item, blocked and nodes as flat fields. Its boundary validates cardinality, uniqueness and plan-ordered preview partitions, retaining the agreed representation.
validated_record! {
/// An item and its complete affected-node list.
pub struct HumanItemDetail, HumanItemDetailWire {
    /// Item.
    pub item: HumanItem,
    /// Blocked.
    pub blocked: HumanItemBlocked,
    /// Nodes.
    pub nodes: Vec<HumanItemNode>,
}
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// A successful mutation and its event identifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HumanItemReceipt {
    /// Item.
    pub item: HumanItemView,
    /// Event id.
    pub event_id: String,
}

// llmlint: ignore-block[invalid_states_unrepresentable] The event contract publishes item_id, owner, item, resolution and a numeric version together. Keep those flat public fields; the boundary verifies the duplicated identity/resolution and action/state consistency instead of replacing the prescribed envelope payload.
validated_record! {
/// Version-one payload transported by a generic bus envelope.
#[schemars(extend("allOf" = event_constraints()))]
pub struct HumanItemEventPayload, HumanItemEventPayloadWire {
    /// Schema version.
    #[serde(deserialize_with = "version")]
    #[schemars(range(min = 1, max = 1))]
    pub schema_version: u32,
    /// Item id.
    pub item_id: String,
    /// Owner.
    pub owner: HumanItemOwner,
    /// Action.
    pub action: HumanItemEventAction,
    /// Item.
    pub item: HumanItem,
    /// Resolution.
    #[serde(deserialize_with = "required_option")]
    #[schemars(required, schema_with = "nullable::<HumanItemResolution>")]
    pub resolution: Option<HumanItemResolution>,
}
}
// llmlint: ignore-end[invalid_states_unrepresentable]

macro_rules! event_actions {
    ($($variant:ident => $wire:literal),+ $(,)?) => {
        /// The closed action ids transported in human-item event payloads.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
        pub enum HumanItemEventAction {
            $(#[doc = $wire] #[serde(rename = $wire)] $variant,)+
        }
        /// The five human-item envelope kinds, sharing the payload action ids.
        pub const EVENT_KINDS: [&str; 5] = [$($wire),+];
    };
}
event_actions! {
    Raised => "human-item-raised",
    Answered => "human-item-answered",
    Completed => "human-item-completed",
    Dismissed => "human-item-dismissed",
    AutoResolved => "human-item-auto-resolved",
}

/// Export all engine-owned human-item schemas for CLI and browser conformance.
pub fn schemas() -> serde_json::Value {
    serde_json::json!({
        "priority_labels": Priority::ALL.into_iter().filter(|p| *p != Priority::None)
            .map(|p| (p.as_str(), priority_label(p))).collect::<std::collections::BTreeMap<_, _>>(),
        "HumanItemSample": schemars::schema_for!(HumanItemSample),
        "HumanItemRefusal": schemars::schema_for!(HumanItemRefusal),
        "HumanItem": schemars::schema_for!(HumanItem),
        "HumanItemInput": schemars::schema_for!(HumanItemInput),
        "HumanItemAction": schemars::schema_for!(HumanItemAction),
        "HumanItemFilter": schemars::schema_for!(HumanItemFilter),
        "HumanItemView": schemars::schema_for!(HumanItemView),
        "HumanItemDetail": schemars::schema_for!(HumanItemDetail),
        "HumanItemReceipt": schemars::schema_for!(HumanItemReceipt),
        "HumanItemMetadata": schemars::schema_for!(HumanItemMetadata),
        "HumanItemEventPayload": schemars::schema_for!(HumanItemEventPayload)
    })
}

/// Explicit sample adapter selection and command.
#[derive(Debug, Clone, PartialEq, Eq, clap::Args)]
pub struct HumanItemsArgs {
    /// Fixture bundle to query or mutate instead of the host's live journal.
    #[arg(long, global = true)]
    pub sample: Option<std::path::PathBuf>,
    /// Human attention operation.
    #[command(subcommand)]
    pub command: HumanItemsCommand,
}

/// The CLI operations; every task, including design review, uses complete.
#[derive(Debug, Clone, PartialEq, Eq, clap::Subcommand)]
pub enum HumanItemsCommand {
    /// Raise an item from JSON immutable fields, or what a configured phase's
    /// hook expands to against a JSON context.
    Raise {
        /// JSON input file.
        #[arg(long, required_unless_present = "phase", conflicts_with_all = ["phase", "context"])]
        input: Option<std::path::PathBuf>,
        /// A phase configured under `human_items.hooks`.
        #[arg(long, requires = "context")]
        phase: Option<String>,
        /// The phase's JSON context: owner, dedup_key, and what the hook expands.
        #[arg(long, requires = "phase")]
        context: Option<std::path::PathBuf>,
    },
    /// List matching items and their full bodies.
    List(HumanItemsQuery),
    /// Show an item and every blocked step.
    Show {
        /// Opaque item identifier.
        id: String,
        /// Emit engine JSON.
        #[arg(long)]
        json: bool,
    },
    /// Answer an open question.
    Answer {
        /// Item identifier.
        id: String,
        /// Nonempty answer.
        #[arg(long, value_parser = parse_answer)]
        // llmlint: ignore[invalid_states_unrepresentable] This argument becomes HumanItemAction::Answer's contract-declared String text; parse_answer refuses blank text at the CLI boundary, so no blank answer reaches the action.
        text: String,
    },
    /// Complete an open task using its stored approval reference, if any.
    Complete {
        /// Item identifier.
        id: String,
    },
    /// Dismiss an open item.
    Dismiss {
        /// Item identifier.
        id: String,
        /// Optional dismissal reason.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Report matching items and their full bodies.
    Report(HumanItemsQuery),
    /// Print the effective `human_items` configuration and where it was read from.
    Config {
        /// Emit engine JSON.
        #[arg(long)]
        json: bool,
    },
    /// Print the item events delivered to one manager session that its watch
    /// has not printed yet, waiting up to `--timeout` seconds for one.
    Watch {
        /// The owning session; omitted, `ONEPIPELINE_LAUNCHER_SESSION`.
        #[arg(long)]
        session: Option<String>,
        /// Seconds to wait when nothing is pending; `0` reads once.
        #[arg(long, default_value_t = 0)]
        timeout: u64,
    },
}

/// Human-facing selectors. Scope is deliberately not an input.
#[derive(Debug, Clone, Default, PartialEq, Eq, clap::Args)]
pub struct HumanItemsQuery {
    /// Selectors from the same engine schema used by API queries.
    #[command(flatten)]
    pub filter: HumanItemFilter,
    /// Emit engine JSON.
    #[arg(long)]
    pub json: bool,
}

fn parse_priority(value: &str) -> Result<Priority, String> {
    match value.to_ascii_lowercase().as_str() {
        "critical" => Ok(Priority::Urgent),
        "high" => Ok(Priority::High),
        "medium" => Ok(Priority::Medium),
        "low" => Ok(Priority::Low),
        _ => Err("priority must be Critical, High, Medium or Low".into()),
    }
}

/// Human-facing priority label; wire values continue to use the imported vocabulary.
pub fn priority_label(priority: Priority) -> &'static str {
    match priority {
        Priority::Urgent => "Critical",
        Priority::High => "High",
        Priority::Medium => "Medium",
        Priority::Low => "Low",
        Priority::None => "Low",
    }
}

/// A typed refusal from a query or action adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, thiserror::Error)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum HumanItemRefusal {
    /// Runtime integration is not installed.
    #[error("human-items unavailable against live state")]
    Unavailable,
    /// The requested transition conflicts with stored state.
    #[error("{reason}")]
    Conflict {
        /// Actionable refusal text.
        reason: String,
    },
}

validated_record! {
/// An explicit sample bundle, validated before any query or action.
pub struct HumanItemSample, HumanItemSampleWire {
    /// Bundle version.
    #[serde(deserialize_with = "version")]
    #[schemars(range(min = 1, max = 1))]
    // llmlint: ignore[invalid_states_unrepresentable] The approved sample bundle publishes schema_version as a number; its decoder and apply boundary enforce 1 while preserving that versioned fixture shape.
    pub schema_version: u32,
    /// Sample items with complete affected-node lists.
    pub items: Vec<HumanItemDetail>,
    /// Sample approval authority's current key for each board copy.
    pub approval_keys: std::collections::BTreeMap<String, String>,
    /// Approvals accepted by the sample authority before item completion.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approvals: Vec<HumanItemSampleApproval>,
}
}

fn conflict(reason: impl Into<String>) -> HumanItemRefusal {
    HumanItemRefusal::Conflict {
        reason: reason.into(),
    }
}

impl HumanItemSample {
    /// Query sample records with default open-state and declared ordering.
    pub fn query(&self, filter: &HumanItemFilter) -> Vec<HumanItemView> {
        let mut items: Vec<_> = self
            .items
            .iter()
            .filter(|detail| {
                let item = &detail.item;
                filter.kind.is_none_or(|kind| item.kind == kind)
                    && filter
                        .blocking
                        .is_none_or(|blocking| item.is_blocking() == blocking)
                    && filter
                        .priority
                        .is_none_or(|priority| item.priority == priority)
                    && item.state == filter.state.unwrap_or(HumanItemState::Open)
                    && filter
                        .session
                        .as_ref()
                        .is_none_or(|session| &item.owner.session == session)
                    && filter
                        .run
                        .as_ref()
                        .is_none_or(|run| item.owner.run.as_ref() == Some(run))
                    && filter
                        .plan
                        .as_ref()
                        .is_none_or(|plan| item.owner.plan.as_ref() == Some(plan))
                    && filter
                        .host
                        .is_none_or(|host| (item.block == HumanItemBlock::Host) == host)
            })
            .map(view)
            .collect();
        items.sort_by_key(|view| {
            let item = &view.item;
            let rank = match item.priority {
                Priority::Urgent => 0,
                Priority::High => 1,
                Priority::Medium => 2,
                Priority::Low => 3,
                Priority::None => 4,
            };
            (
                !item.is_blocking(),
                rank,
                item.created_at.clone(),
                item.id.clone(),
            )
        });
        items
    }

    /// Apply an action to sample state; conflicts never overwrite a resolution.
    /// Stored design-review evidence is checked against this bundle's sample authority.
    pub fn apply(&mut self, action: HumanItemAction) -> Result<HumanItemReceipt, HumanItemRefusal> {
        self.validate().map_err(conflict)?;
        if let HumanItemAction::Raise(input) = action {
            input.validate().map_err(conflict)?;
            if self
                .items
                .iter()
                .any(|d| d.item.dedup_key == input.dedup_key)
            {
                return Err(conflict("dedup_key already exists"));
            }
            let mut suffix = self.items.len() + 1;
            while self
                .items
                .iter()
                .any(|d| d.item.id == format!("sample-{suffix}"))
            {
                suffix += 1;
            }
            // A work hold's impact is derived from the fixture: the stored
            // current impact of another item holding the same run/node. Without
            // one the sample cannot say what is blocked, so it refuses rather
            // than raise an open hold that reports no affected steps.
            let (blocked, nodes) = match &input.block {
                HumanItemBlock::Work { run, node } => self
                    .items
                    .iter()
                    .find(|d| d.item.block == input.block && !d.nodes.is_empty())
                    .map(|d| (d.blocked.clone(), d.nodes.clone()))
                    .ok_or_else(|| {
                        conflict(format!(
                            "sample has no affected-node context for {run} / {node}"
                        ))
                    })?,
                _ => (
                    HumanItemBlocked {
                        total: 0,
                        plans: vec![],
                    },
                    vec![],
                ),
            };
            let item = HumanItem {
                schema_version: 1,
                id: format!("sample-{suffix}"),
                dedup_key: input.dedup_key,
                kind: input.kind,
                title: input.title,
                body: input.body,
                priority: input.priority,
                owner: input.owner,
                block: input.block,
                approval: input.approval,
                state: HumanItemState::Open,
                resolution: None,
                tracker: None,
                created_at: "sample".into(),
                updated_at: "sample".into(),
            };
            self.items.push(HumanItemDetail {
                item,
                blocked,
                nodes,
            });
            return Ok(receipt(self.items.last().expect("just appended"), "raised"));
        }
        let (id, actor, on_behalf_of, state, text, reason, operation) = match action {
            HumanItemAction::Answer {
                id,
                text,
                actor,
                on_behalf_of,
            } => {
                if text.trim().is_empty() {
                    return Err(conflict("answer text must not be blank"));
                }
                (
                    id,
                    actor,
                    on_behalf_of,
                    HumanItemState::Answered,
                    Some(text),
                    None,
                    "answered",
                )
            }
            HumanItemAction::Complete {
                id,
                actor,
                on_behalf_of,
            } => (
                id,
                actor,
                on_behalf_of,
                HumanItemState::Completed,
                None,
                None,
                "completed",
            ),
            HumanItemAction::Dismiss {
                id,
                reason,
                actor,
                on_behalf_of,
            } => (
                id,
                actor,
                on_behalf_of,
                HumanItemState::Dismissed,
                None,
                reason.filter(|s| !s.trim().is_empty()),
                "dismissed",
            ),
            HumanItemAction::Raise(_) => unreachable!(),
        };
        let index = self
            .items
            .iter()
            .position(|d| d.item.id == id)
            .ok_or_else(|| conflict("no such human item"))?;
        let item = &self.items[index].item;
        if item.state != HumanItemState::Open {
            return Err(conflict("item is already resolved"));
        }
        if (state == HumanItemState::Answered && item.kind != HumanItemKind::Question)
            || (state == HumanItemState::Completed && item.kind != HumanItemKind::Task)
        {
            return Err(conflict("action does not match item kind"));
        }
        if state == HumanItemState::Completed {
            if let Some(approval) = &item.approval {
                let current = self
                    .approval_keys
                    .get(&approval.board_copy)
                    .ok_or_else(|| conflict("sample approval authority has no evidence"))?
                    .clone();
                if current != approval.key {
                    // The replacement is checked before anything changes: a
                    // dedup_key already held refuses with the fixture intact,
                    // and an id already held moves to the next free suffix, so
                    // the written bundle never carries a duplicate.
                    let dedup_key = format!("{}:{current}", item.dedup_key);
                    if let Some(held) = self.items.iter().find(|d| d.item.dedup_key == dedup_key) {
                        return Err(conflict(format!(
                            "Re-read the current document; replacement dedup_key {dedup_key} \
                             is already held by {}",
                            held.item.id
                        )));
                    }
                    let mut suffix = self.items.len();
                    let id = loop {
                        let id = format!("{}-replacement-{suffix}", item.id);
                        if !self.items.iter().any(|d| d.item.id == id) {
                            break id;
                        }
                        suffix += 1;
                    };
                    let mut replacement = self.items[index].clone();
                    replacement.item.approval = Some(HumanItemApproval {
                        board_copy: approval.board_copy.clone(),
                        key: current.clone(),
                    });
                    replacement.item.id = id;
                    replacement.item.dedup_key = dedup_key;
                    replacement.item.created_at = "sample-replacement".into();
                    replacement.item.updated_at = "sample-replacement".into();
                    let obsolete = &mut self.items[index].item;
                    obsolete.state = HumanItemState::Dismissed;
                    obsolete.resolution = Some(HumanItemResolution {
                        actor,
                        on_behalf_of,
                        text: None,
                        reason: Some("Re-read the current document".into()),
                    });
                    obsolete.updated_at = "sample-obsolete".into();
                    self.items[index].blocked = HumanItemBlocked {
                        total: 0,
                        plans: vec![],
                    };
                    self.items[index].nodes.clear();
                    self.items.push(replacement);
                    return Err(conflict("Re-read the current document"));
                }
                self.approvals.push(HumanItemSampleApproval {
                    approval: approval.clone(),
                    actor: actor.clone(),
                    on_behalf_of: on_behalf_of.clone(),
                });
            }
        }
        let detail = &mut self.items[index];
        detail.item.state = state;
        detail.item.resolution = Some(HumanItemResolution {
            actor,
            on_behalf_of,
            text,
            reason,
        });
        detail.item.updated_at = "sample-resolved".into();
        detail.blocked = HumanItemBlocked {
            total: 0,
            plans: vec![],
        };
        detail.nodes.clear();
        Ok(receipt(detail, operation))
    }
}

fn view(detail: &HumanItemDetail) -> HumanItemView {
    let mut blocked = detail.blocked.clone();
    if !detail.item.is_blocking() {
        blocked = HumanItemBlocked {
            total: 0,
            plans: vec![],
        };
    }
    HumanItemView {
        item: detail.item.clone(),
        blocked,
    }
}
fn receipt(detail: &HumanItemDetail, action: &str) -> HumanItemReceipt {
    HumanItemReceipt {
        item: view(detail),
        event_id: format!("sample-{}-{action}", detail.item.id),
    }
}

fn render(item: &HumanItem, blocked: &HumanItemBlocked) -> String {
    let source = if item.block == HumanItemBlock::Host {
        "host".into()
    } else {
        match (&item.owner.plan, &item.owner.run) {
            (Some(plan), Some(run)) => format!("plan {plan} / run {run}"),
            (Some(plan), None) => format!("plan {plan}"),
            (None, Some(run)) => format!("run {run}"),
            (None, None) => format!("session {}", item.owner.session),
        }
    };
    let tracker = item
        .tracker
        .as_ref()
        .map(|t| format!("tracker {}\n", t.url))
        .unwrap_or_default();
    let blocking = if item.is_blocking() {
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
    } else {
        String::new()
    };
    format!(
        "{}  {}  {:?}  {:?}  {}  {}\n{}\n{}{}\n",
        item.id,
        priority_label(item.priority),
        item.kind,
        item.state,
        source,
        blocking,
        item.title,
        tracker,
        item.body
    )
}

pub(crate) fn dispatch(args: HumanItemsArgs) -> crate::error::Result<i32> {
    use crate::error::Error;
    let Some(path) = args.sample else {
        return live_dispatch(args.command);
    };
    let read =
        std::fs::read(&path).map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))?;
    let mut sample: HumanItemSample =
        serde_json::from_slice(&read).map_err(|e| Error::Invalid(e.to_string()))?;
    let action = match args.command {
        HumanItemsCommand::List(query) | HumanItemsCommand::Report(query) => {
            let items = sample.query(&query.filter);
            if query.json {
                println!(
                    "{}",
                    serde_json::to_string(&items).map_err(|e| Error::Invalid(e.to_string()))?
                );
            } else {
                for item in items {
                    print!("{}", render(&item.item, &item.blocked));
                }
            }
            return Ok(0);
        }
        HumanItemsCommand::Show { id, json } => {
            let detail = sample
                .items
                .iter()
                .find(|d| d.item.id == id)
                .ok_or_else(|| Error::Refused("no such human item".into()))?;
            let current = view(detail);
            let detail = HumanItemDetail {
                item: current.item,
                blocked: current.blocked,
                nodes: if detail.item.is_blocking() {
                    detail.nodes.clone()
                } else {
                    vec![]
                },
            };
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&detail).map_err(|e| Error::Invalid(e.to_string()))?
                );
            } else {
                print!("{}", render(&detail.item, &detail.blocked));
                for node in detail.nodes {
                    println!("{} / {}: {}", node.run, node.node, node.title);
                }
            }
            return Ok(0);
        }
        HumanItemsCommand::Watch { .. } | HumanItemsCommand::Config { .. } => {
            return Err(Error::Invalid(
                "watch and config read the live store; they take no --sample".into(),
            ))
        }
        HumanItemsCommand::Raise { phase: Some(_), .. } => {
            return Err(Error::Invalid(
                "a phase raise expands a configured hook; it takes no --sample".into(),
            ))
        }
        HumanItemsCommand::Raise { input, .. } => {
            let input = input.ok_or_else(|| Error::Invalid("raise names --input".into()))?;
            let input = std::fs::read(input).map_err(|e| Error::Invalid(e.to_string()))?;
            HumanItemAction::Raise(
                serde_json::from_slice(&input).map_err(|e| Error::Invalid(e.to_string()))?,
            )
        }
        HumanItemsCommand::Answer { id, text } => HumanItemAction::Answer {
            id,
            text,
            actor: "sample-operator".into(),
            on_behalf_of: None,
        },
        HumanItemsCommand::Complete { id } => HumanItemAction::Complete {
            id,
            actor: "sample-operator".into(),
            on_behalf_of: None,
        },
        HumanItemsCommand::Dismiss { id, reason } => HumanItemAction::Dismiss {
            id,
            reason,
            actor: "sample-operator".into(),
            on_behalf_of: None,
        },
    };
    let before = sample.clone();
    let outcome = sample.apply(action);
    // Ordinary refusals preserve the fixture bytes. A stale design refusal is
    // different: its obsolete-item dismissal and replacement must be retained.
    if sample != before {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&sample).map_err(|e| Error::Invalid(e.to_string()))?,
        )
        .map_err(|e| Error::Invalid(e.to_string()))?;
    }
    let receipt = outcome
        .map_err(|e| Error::Refused(serde_json::to_string(&e).unwrap_or_else(|_| e.to_string())))?;
    println!(
        "{}",
        serde_json::to_string(&receipt).map_err(|e| Error::Invalid(e.to_string()))?
    );
    Ok(0)
}

fn refused(refusal: &HumanItemRefusal) -> crate::error::Error {
    crate::error::Error::Refused(
        serde_json::to_string(refusal).unwrap_or_else(|_| refusal.to_string()),
    )
}

fn to_json<T: Serialize>(value: &T) -> crate::error::Result<String> {
    serde_json::to_string(value).map_err(|e| crate::error::Error::Invalid(e.to_string()))
}

/// The one printable manager report: a count, then the blocking items and the
/// rest, each with its full body.
fn report(views: &[HumanItemView]) -> String {
    let blocking: Vec<_> = views.iter().filter(|v| v.item.is_blocking()).collect();
    let rest: Vec<_> = views.iter().filter(|v| !v.item.is_blocking()).collect();
    let mut out = format!(
        "Human items: {} listed, {} blocking\n",
        views.len(),
        blocking.len()
    );
    for (heading, group) in [("Blocking", blocking), ("Not blocking", rest)] {
        if group.is_empty() {
            continue;
        }
        out.push_str(&format!("\n{heading} ({})\n\n", group.len()));
        for view in group {
            out.push_str(&render(&view.item, &view.blocked));
            for plan in &view.blocked.plans {
                for node in &plan.preview {
                    out.push_str(&format!(
                        "  - {} / {}: {}\n",
                        node.run, node.node, node.title
                    ));
                }
                if plan.count > plan.preview.len() as u64 {
                    out.push_str(&format!(
                        "  - and {} more in {}\n",
                        plan.count - plan.preview.len() as u64,
                        plan.plan
                    ));
                }
            }
            out.push('\n');
        }
    }
    out
}

fn live_dispatch(command: HumanItemsCommand) -> crate::error::Result<i32> {
    use crate::error::Error;
    if let HumanItemsCommand::Config { json } = &command {
        let config = config::here().map_err(Error::Invalid)?;
        if *json {
            println!("{}", to_json(&config)?);
        } else {
            let effective = &config.human_items;
            println!("enabled: {}", effective.enabled);
            println!(
                "task_source: {}",
                effective.task_source.as_deref().unwrap_or("(none)")
            );
            println!(
                "hooks: {}",
                effective
                    .hooks
                    .iter()
                    .map(|h| h.phase.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            for source in &config.sources {
                println!("read from: {}", source.display());
            }
        }
        return Ok(0);
    }
    let dir = std::env::current_dir().map_err(|e| Error::Invalid(e.to_string()))?;
    let store = HumanItems::open(crate::ledger::runs_root(), &dir).map_err(|e| refused(&e))?;
    let actor = Actor::from_environment();
    let action = match command {
        HumanItemsCommand::Config { .. } => unreachable!("answered above"),
        HumanItemsCommand::Raise {
            phase: Some(phase),
            context: Some(context),
            ..
        } => {
            let bytes = std::fs::read(&context)
                .map_err(|e| Error::Invalid(format!("{}: {e}", context.display())))?;
            let context: phases::PhaseContext = serde_json::from_slice(&bytes)
                .map_err(|e| Error::Invalid(format!("{}: {e}", context.display())))?;
            match store
                .raise_phase(&phase, &context)
                .map_err(|e| refused(&e))?
            {
                Some(receipt) => println!("{}", to_json(&receipt)?),
                None => println!("null"),
            }
            return Ok(0);
        }
        HumanItemsCommand::List(query) | HumanItemsCommand::Report(query) if query.json => {
            let views = store.query(&query.filter).map_err(|e| refused(&e))?;
            println!("{}", to_json(&views)?);
            return Ok(0);
        }
        HumanItemsCommand::List(query) => {
            let views = store.query(&query.filter).map_err(|e| refused(&e))?;
            for view in &views {
                print!("{}", render(&view.item, &view.blocked));
            }
            return Ok(0);
        }
        HumanItemsCommand::Report(query) => {
            let views = store.query(&query.filter).map_err(|e| refused(&e))?;
            print!("{}", report(&views));
            return Ok(0);
        }
        HumanItemsCommand::Show { id, json } => {
            let detail = store.show(&id).map_err(|e| refused(&e))?;
            if json {
                println!("{}", to_json(&detail)?);
            } else {
                print!("{}", render(&detail.item, &detail.blocked));
                for node in &detail.nodes {
                    println!("{} / {}: {}", node.run, node.node, node.title);
                }
            }
            return Ok(0);
        }
        HumanItemsCommand::Watch { session, timeout } => {
            let session = session
                .or_else(|| std::env::var(crate::sys::LAUNCHER_SESSION_ENV).ok())
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| {
                    Error::Invalid(format!(
                        "no session to watch: name one with --session, or export {}",
                        crate::sys::LAUNCHER_SESSION_ENV
                    ))
                })?;
            let mut out = std::io::stdout().lock();
            let printed = delivery::watch(
                &crate::ledger::runs_root(),
                &session,
                std::time::Duration::from_secs(timeout),
                &mut |envelope| {
                    use std::io::Write as _;
                    writeln!(
                        out,
                        "{}",
                        serde_json::to_string(envelope).unwrap_or_default()
                    )?;
                    out.flush()
                },
            )
            .map_err(Error::Invalid)?;
            return Ok(if printed == 0 {
                crate::error::EXIT_WATCH_ELAPSED
            } else {
                0
            });
        }
        HumanItemsCommand::Raise { input, .. } => {
            let input = input.ok_or_else(|| Error::Invalid("raise names --input".into()))?;
            let bytes = std::fs::read(&input)
                .map_err(|e| Error::Invalid(format!("{}: {e}", input.display())))?;
            HumanItemAction::Raise(
                serde_json::from_slice(&bytes).map_err(|e| Error::Invalid(e.to_string()))?,
            )
        }
        HumanItemsCommand::Answer { id, text } => HumanItemAction::Answer {
            id,
            text,
            actor: actor.actor,
            on_behalf_of: actor.on_behalf_of,
        },
        HumanItemsCommand::Complete { id } => HumanItemAction::Complete {
            id,
            actor: actor.actor,
            on_behalf_of: actor.on_behalf_of,
        },
        HumanItemsCommand::Dismiss { id, reason } => HumanItemAction::Dismiss {
            id,
            reason,
            actor: actor.actor,
            on_behalf_of: actor.on_behalf_of,
        },
    };
    let receipt = store.apply(action).map_err(|e| refused(&e))?;
    println!("{}", to_json(&receipt)?);
    // The resolution is durable; the step it stands for is released through the
    // run's own commands. One that cannot be released now is released by the
    // run's driver, so this is said and the receipt stands.
    if let Err(why) = runtime::release(&receipt.item.item) {
        eprintln!("onepipeline: {why}");
    }
    Ok(0)
}

impl HumanItemInput {
    fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err("expected schema_version 1".into());
        }
        if self.kind == HumanItemKind::Question && self.approval.is_some() {
            return Err("approval is only valid on tasks".into());
        }
        if self.priority == Priority::None
            || self.title.trim().is_empty()
            || self.body.trim().is_empty()
        {
            return Err("human items require title, body and priority".into());
        }
        Ok(())
    }
}
impl HumanItem {
    fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err("expected schema_version 1".into());
        }
        if self.priority == Priority::None
            || self.title.trim().is_empty()
            || self.body.trim().is_empty()
        {
            return Err("human items require title, body and priority".into());
        }
        let valid = match (self.kind, self.state, self.resolution.as_ref()) {
            (_, HumanItemState::Open, None) => true,
            (HumanItemKind::Question, HumanItemState::Answered, Some(r)) => {
                r.text.as_ref().is_some_and(|s| !s.trim().is_empty()) && r.reason.is_none()
            }
            (HumanItemKind::Task, HumanItemState::Completed, Some(r)) => {
                r.text.is_none() && r.reason.is_none()
            }
            (_, HumanItemState::Dismissed, Some(r)) => r.text.is_none(),
            _ => false,
        };
        if !valid {
            return Err("state, kind and resolution conflict".into());
        }
        if self.kind == HumanItemKind::Question && self.approval.is_some() {
            return Err("approval is only valid on tasks".into());
        }
        Ok(())
    }
}

fn optional_priority<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Priority>, D::Error> {
    let value = Option::<Priority>::deserialize(d)?;
    if value == Some(Priority::None) {
        return Err(serde::de::Error::custom("human items require a priority"));
    }
    Ok(value)
}
fn optional_priority_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    let values: Vec<_> = std::iter::once(serde_json::Value::Null)
        .chain(
            Priority::ALL
                .into_iter()
                .filter(|p| *p != Priority::None)
                .map(|p| serde_json::json!(p.as_str())),
        )
        .collect();
    schemars::json_schema!({"type":["string","null"],"enum":values})
}

fn item_constraints() -> serde_json::Value {
    serde_json::json!([
        {"if":{"properties":{"state":{"const":"open"}}},"then":{"properties":{"resolution":{"type":"null"}}},"else":{"properties":{"resolution":{"type":"object"}}}},
        {"if":{"properties":{"kind":{"const":"task"}}},"then":{"properties":{"state":{"enum":["open","completed","dismissed"]}}},"else":{"properties":{"state":{"enum":["open","answered","dismissed"]},"approval":{"type":"null"}}}},
        {"if":{"properties":{"state":{"const":"answered"}}},"then":{"properties":{"resolution":{"properties":{"text":{"type":"string","minLength":1,"pattern":"\\S"},"reason":{"type":"null"}},"required":["text"]}}}},
        {"if":{"properties":{"state":{"enum":["completed","dismissed"]}}},"then":{"properties":{"resolution":{"properties":{"text":{"type":"null"}}}}}},
        {"if":{"properties":{"state":{"const":"completed"}}},"then":{"properties":{"resolution":{"properties":{"reason":{"type":"null"}}}}}}
    ])
}

/// An approval written by the fixture authority, never a live design approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HumanItemSampleApproval {
    /// Stored document reference.
    pub approval: HumanItemApproval,
    /// Resolving actor.
    pub actor: String,
    /// User represented by the actor, if any.
    #[serde(deserialize_with = "required_option")]
    #[schemars(required, schema_with = "nullable::<String>")]
    pub on_behalf_of: Option<String>,
}

fn required_option<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    decoder: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(decoder)
}

fn input_constraints() -> serde_json::Value {
    serde_json::json!([{"if":{"properties":{"kind":{"const":"question"}}},"then":{"properties":{"approval":{"type":"null"}}}}])
}

fn nullable<T: JsonSchema>(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    generator.subschema_for::<Option<T>>()
}

fn parse_answer(value: &str) -> Result<String, String> {
    if value.trim().is_empty() {
        return Err("answer text must not be blank".into());
    }
    Ok(value.to_owned())
}

fn parse_blocking(value: &str) -> Result<bool, String> {
    match value {
        "yes" => Ok(true),
        "no" => Ok(false),
        _ => Err("blocking must be yes or no".into()),
    }
}

fn preview<'de, D: serde::Deserializer<'de>>(decoder: D) -> Result<Vec<HumanItemNode>, D::Error> {
    let nodes = Vec::<HumanItemNode>::deserialize(decoder)?;
    if nodes.len() > 3 {
        return Err(serde::de::Error::custom(
            "preview contains more than three nodes",
        ));
    }
    Ok(nodes)
}
fn validate_impact(item: &HumanItem, blocked: &HumanItemBlocked) -> Result<(), String> {
    if !item.is_blocking() && (blocked.total != 0 || !blocked.plans.is_empty()) {
        return Err("nonblocking items cannot report blocked work".into());
    }
    let mut total = 0_u64;
    let mut plans = std::collections::BTreeSet::new();
    for plan in &blocked.plans {
        if !plans.insert(&plan.plan) {
            return Err("duplicate blocked plan".into());
        }
        if plan.preview.len() as u64 != plan.count.min(3) {
            return Err("preview must contain the first three affected nodes".into());
        }
        total = total
            .checked_add(plan.count)
            .ok_or("blocked count overflow")?;
    }
    if total != blocked.total {
        return Err("blocked total differs from plan counts".into());
    }
    Ok(())
}
impl HumanItemView {
    fn validate(&self) -> Result<(), String> {
        validate_impact(&self.item, &self.blocked)
    }
}
impl HumanItemDetail {
    fn validate(&self) -> Result<(), String> {
        validate_impact(&self.item, &self.blocked)?;
        if self.nodes.len() as u64 != self.blocked.total {
            return Err("blocked total differs from affected nodes".into());
        }
        let mut nodes = std::collections::BTreeSet::new();
        for node in &self.nodes {
            if !nodes.insert((&node.run, &node.node)) {
                return Err("duplicate affected node".into());
            }
        }
        let mut offset = 0;
        for plan in &self.blocked.plans {
            let count =
                usize::try_from(plan.count).map_err(|_| "blocked count does not fit this host")?;
            // validate_impact established the sum, and nodes.len matches that sum.
            let group = &self.nodes[offset..offset + count];
            if group.iter().take(3).ne(plan.preview.iter()) {
                return Err("preview differs from the first affected nodes of its plan".into());
            }
            offset += count;
        }
        Ok(())
    }
}
impl HumanItemSample {
    fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err("expected sample schema_version 1".into());
        }
        let mut ids = std::collections::BTreeSet::new();
        let mut dedup = std::collections::BTreeSet::new();
        for detail in &self.items {
            detail.item.validate()?;
            detail.validate()?;
            if !ids.insert(&detail.item.id) {
                return Err("duplicate human-item id".into());
            }
            if !dedup.insert(&detail.item.dedup_key) {
                return Err("duplicate human-item dedup_key".into());
            }
        }
        Ok(())
    }
}

/// Derive the future adapter's one task from a legacy human node without changing plan loading.
/// A blank stored title falls back to the first nonempty action line, then the node id.
/// The body is preserved verbatim; absent or None store priority defaults to Low.
pub fn derive_legacy(
    node_id: &str,
    stored_title: &str,
    action_body: &str,
    store_priority: Option<Priority>,
) -> HumanItemMetadata {
    let title = if stored_title.trim().is_empty() {
        action_body
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or(node_id)
    } else {
        stored_title
    };
    HumanItemMetadata {
        kind: HumanItemKind::Task,
        priority: store_priority
            .filter(|p| *p != Priority::None)
            .unwrap_or(Priority::Low),
        title: title.into(),
        body: action_body.into(),
    }
}

impl HumanItemEventPayload {
    fn validate(&self) -> Result<(), String> {
        self.item.validate()?;
        if self.schema_version != 1
            || self.item_id != self.item.id
            || self.owner != self.item.owner
            || self.resolution != self.item.resolution
        {
            return Err(
                "event identity, owner, version or resolution differs from its item".into(),
            );
        }
        let expected = match self.action {
            HumanItemEventAction::Raised => HumanItemState::Open,
            HumanItemEventAction::Answered => HumanItemState::Answered,
            HumanItemEventAction::Completed => HumanItemState::Completed,
            HumanItemEventAction::Dismissed | HumanItemEventAction::AutoResolved => {
                HumanItemState::Dismissed
            }
        };
        if self.item.state != expected {
            return Err("event action and item state conflict".into());
        }
        Ok(())
    }
}
fn event_constraints() -> serde_json::Value {
    use HumanItemEventAction as Action;
    use HumanItemState as State;
    let constraints = [
        (Action::Raised, State::Open), (Action::Answered, State::Answered),
        (Action::Completed, State::Completed), (Action::Dismissed, State::Dismissed),
        (Action::AutoResolved, State::Dismissed),
    ].into_iter().map(|(action, state)| serde_json::json!({
        "if":{"properties":{"action":{"const":action}}},
        "then":{"properties":{"item":{"properties":{"state":{"const":state}}},"resolution":{"type":if state == State::Open { "null" } else { "object" }}}}
    })).collect::<Vec<_>>();
    serde_json::json!(constraints)
}
