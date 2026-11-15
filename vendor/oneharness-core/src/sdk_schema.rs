//! The Rust-owned schema bundle consumed by language SDK generators.
//!
//! Keeping every shared contract root here gives all SDKs one generation
//! source. Language packages may add client behavior, but must not restate
//! these wire shapes by hand.

use schemars::{schema_for, Schema};
use serde::Serialize;

use crate::domain::fallback::RunWork;
use crate::domain::history::{
    gated_failure_kind_version, HistoryLine, HistoryRecord, HistoryShowEntry,
    HistoryStreamEnvelope, FIRST_CANCELLED_SCHEMA_VERSION, FIRST_ERROR_SCHEMA_VERSION,
    FIRST_EVENT_SCHEMA_VERSION, FIRST_MODEL_OBSERVATION_SCHEMA_VERSION,
    FIRST_PARTIAL_TIMING_SCHEMA_VERSION, FIRST_WORK_EVIDENCE_SCHEMA_VERSION,
    OBSERVED_TIMING_SCHEMA_VERSION, PREVIOUS_CURRENT_SCHEMA_VERSION, PRE_LIFECYCLE_RECORD_VERSIONS,
};
use crate::domain::history::{requires_provider_finish, run_failed, versions_from};
use crate::domain::report::{attempted_failure, RunReport, RunStreamEnvelope, Status};
use crate::domain::sdk::{
    schema_for_serialize, ConfigOptions, DetectOptions, GateOptions, HistoryClearOptions,
    HistoryListOptions, HistoryLookup, HistoryMigrateOptions, HistoryPointersOptions,
    HistoryReindexOptions, HistoryWatchOptions, InitOptions, InterruptOptions, MockOptions,
    RunOptions, SyncOptions, UsageOptions,
};
use crate::domain::signals::FailureKind;
use crate::io::history::SessionSummary;

/// All core schemas shared by oneharness SDKs.
#[derive(Debug, Serialize)]
pub struct SdkSchemaBundle {
    /// The declared capability surface: what each SDK must expose, and which
    /// CLI flag each of its options renders to. Emitted alongside the schemas
    /// because it is generation input too — the argv a typed method builds is
    /// as much a contract as the shapes it sends and receives.
    pub capabilities: &'static [crate::domain::capability::Capability],
    pub run_report: Schema,
    pub run_stream_envelope: Schema,
    pub run_options: Schema,
    pub history_lookup: Schema,
    pub history_list_options: Schema,
    pub history_watch_options: Schema,
    pub history_line: Schema,
    pub history_record: Schema,
    pub history_stream_envelope: Schema,
    pub history_show_entries: Schema,
    pub history_list: Schema,
    pub list_report: Schema,
    pub detect_report: Schema,
    // The per-verb contracts behind the capabilities beyond `run` and `history`.
    // Every root a capability names must appear here — `capability.rs`'s
    // `every_named_schema_root_is_emitted_by_the_bundle` is what makes that a
    // build failure rather than an SDK pointed at a contract with no source.
    pub detect_options: Schema,
    pub config_options: Schema,
    pub config_report: Schema,
    pub sync_options: Schema,
    pub sync_report: Schema,
    pub init_options: Schema,
    pub usage_options: Schema,
    pub usage_report: Schema,
    pub gate_options: Schema,
    pub mock_options: Schema,
    pub interrupt_options: Schema,
    pub interrupt_response: Schema,
    pub history_clear_options: Schema,
    pub history_clear_report: Schema,
    pub history_migrate_options: Schema,
    pub history_migrate_report: Schema,
    pub history_reindex_options: Schema,
    pub history_reindex_report: Schema,
    pub history_pointers_options: Schema,
    /// What `history pointers` reads back: the run's pointer lines, each a
    /// `HistoryPointer` — the one declaration of that line, so the SDK types
    /// and the CLI's output are generated from the same source.
    pub history_pointers: Schema,
}

/// Generate the shared SDK schema roots from their Rust contract types.
pub fn bundle() -> SdkSchemaBundle {
    SdkSchemaBundle {
        capabilities: crate::domain::capability::CAPABILITIES,
        run_report: schema_for_serialize::<RunReport>(),
        run_stream_envelope: Schema::try_from(expand_tagged_line_variants(schema_for_serialize::<
            RunStreamEnvelope,
        >()))
        .expect("stream line schema remains an object"),
        run_options: schema_for!(RunOptions),
        history_lookup: schema_for!(HistoryLookup),
        history_list_options: schema_for!(HistoryListOptions),
        history_watch_options: schema_for!(HistoryWatchOptions),
        history_line: history_line_schema(schema_for_serialize::<HistoryLine>()),
        history_record: history_schema(schema_for_serialize::<HistoryRecord>()),
        history_stream_envelope: history_stream_schema(),
        history_show_entries: history_schema(schema_for_serialize::<Vec<HistoryShowEntry>>()),
        history_list: schema_for_serialize::<Vec<SessionSummary>>(),
        list_report: schema_for_serialize::<crate::io::registry::ListReport>(),
        detect_report: schema_for_serialize::<crate::io::detect::DetectReport>(),
        detect_options: schema_for!(DetectOptions),
        config_options: schema_for!(ConfigOptions),
        config_report: schema_for_serialize::<crate::domain::config::ConfigReport>(),
        sync_options: schema_for!(SyncOptions),
        sync_report: schema_for_serialize::<crate::io::sync::SyncReport>(),
        init_options: schema_for!(InitOptions),
        usage_options: schema_for!(UsageOptions),
        usage_report: schema_for_serialize::<crate::domain::usage::UsageReport>(),
        gate_options: schema_for!(GateOptions),
        mock_options: schema_for!(MockOptions),
        interrupt_options: schema_for!(InterruptOptions),
        interrupt_response: schema_for_serialize::<crate::domain::control::ControlResponse>(),
        history_clear_options: schema_for!(HistoryClearOptions),
        history_clear_report: schema_for_serialize::<crate::io::history::HistoryClearReport>(),
        history_migrate_options: schema_for!(HistoryMigrateOptions),
        history_migrate_report: schema_for_serialize::<crate::io::history::HistoryMigrateReport>(),
        history_reindex_options: schema_for!(HistoryReindexOptions),
        history_reindex_report: schema_for_serialize::<crate::io::history::HistoryReindexReport>(),
        history_pointers_options: schema_for!(HistoryPointersOptions),
        history_pointers: schema_for_serialize::<crate::io::history::HistoryPointers>(),
    }
}

/// Collect every property name a schema root accepts, following the `$ref`s of
/// a union's branches.
///
/// Used by `every_bound_option_is_a_field_of_its_options_contract` and shared
/// with nothing else: a lookup contract is an untagged union whose branches are
/// definitions, so "does this option exist?" cannot be answered from the root's
/// own `properties` alone.
#[cfg(test)]
fn accepted_properties(node: &serde_json::Value, root: &serde_json::Value) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(properties) = node.get("properties").and_then(|v| v.as_object()) {
        names.extend(properties.keys().cloned());
    }
    for keyword in ["anyOf", "oneOf"] {
        for branch in node
            .get(keyword)
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            match branch.get("$ref").and_then(|v| v.as_str()) {
                Some(reference) => {
                    let name = reference.rsplit('/').next().unwrap_or_default();
                    if let Some(target) = root.pointer(&format!("/$defs/{name}")) {
                        names.extend(accepted_properties(target, root));
                    }
                }
                None => names.extend(accepted_properties(branch, root)),
            }
        }
    }
    names
}

fn history_stream_schema() -> Schema {
    // llmlint: ignore[no_panics_on_recoverable_errors] SDK schema generation is a build-time codegen boundary, and every sibling schema transformation in this module treats an impossible schemars-to-JSON failure as a generator invariant; returning a partial bundle would be less actionable than failing generation here.
    let mut value = serde_json::to_value(schema_for_serialize::<HistoryStreamEnvelope>())
        .expect("Schema serializes");
    // llmlint: ignore[no_panics_on_recoverable_errors] This is the same build-time schemars invariant as the envelope serialization above; callers cannot recover from a malformed generated contract.
    let history_line =
        serde_json::to_value(schema_for_serialize::<HistoryLine>()).expect("Schema serializes");
    let definitions = history_line["$defs"]
        .as_object()
        .expect("history line definitions");
    for name in ["ActionEvent", "HistoryEventLine"] {
        value["$defs"][name] = definitions[name].clone();
    }
    add_history_line_conditions(&mut value);
    add_v03_condition(&mut value);
    Schema::try_from(value).expect("history stream schema remains an object")
}

fn expand_tagged_line_variants(schema: Schema) -> serde_json::Value {
    let mut value = serde_json::to_value(schema).expect("Schema serializes");
    let definitions = value["$defs"].clone();
    let mut expanded_names = Vec::new();
    if let Some(variants) = value
        .get_mut("oneOf")
        .and_then(serde_json::Value::as_array_mut)
    {
        for variant in variants.iter_mut() {
            let Some(reference) = variant.get("$ref").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let name = reference
                .rsplit('/')
                .next()
                .expect("schema reference has a name");
            expanded_names.push(name.to_string());
            let mut expanded = definitions[name].clone();
            let discriminator = variant["properties"].clone();
            expanded["properties"]
                .as_object_mut()
                .expect("object properties")
                .extend(
                    discriminator
                        .as_object()
                        .expect("discriminator properties")
                        .clone(),
                );
            expanded["required"]
                .as_array_mut()
                .expect("required array")
                .extend(
                    variant["required"]
                        .as_array()
                        .expect("required array")
                        .clone(),
                );
            *variant = expanded;
        }
    }
    // Inlined definitions have no remaining references. Keeping them would make
    // Zod import interfaces that TypeScript correctly omits as unreachable.
    for name in expanded_names {
        value["$defs"]
            .as_object_mut()
            .expect("schema definitions")
            .remove(&name);
    }
    value
}

fn history_line_schema(schema: Schema) -> Schema {
    let mut value = expand_tagged_line_variants(schema);
    add_history_line_conditions(&mut value);
    if let Some(variants) = value
        .get_mut("oneOf")
        .and_then(serde_json::Value::as_array_mut)
    {
        for variant in variants {
            *variant = serde_json::json!({"allOf": [variant.clone()]});
        }
    }
    Schema::try_from(value).expect("conditional history line schema remains an object")
}

fn add_history_line_conditions(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(values) => values.iter_mut().for_each(add_history_line_conditions),
        serde_json::Value::Object(object) => {
            let Some(properties) = object
                .get("properties")
                .and_then(serde_json::Value::as_object)
            else {
                object.values_mut().for_each(add_history_line_conditions);
                return;
            };
            if properties.contains_key("history_id") && properties.contains_key("started") {
                object["properties"]["schema_version"] =
                    versions_schema(&crate::domain::history::versions_from(
                        crate::domain::history::FIRST_RUN_STARTED_SCHEMA_VERSION,
                    ));
                return;
            }
            if properties.contains_key("run_id") && properties.contains_key("event") {
                let mut current = serde_json::Value::Object(object.clone());
                current["properties"]["schema_version"] =
                    versions_schema(&timing_provenance_versions());
                let mut previous = serde_json::Value::Object(object.clone());
                previous["properties"]["schema_version"] =
                    versions_schema(PRE_TIMING_PROVENANCE_VERSIONS);
                forbid_action_event_timing_source(&mut previous["properties"]["event"]);
                object.clear();
                object.insert(
                    "oneOf".to_string(),
                    serde_json::json!([
                        {"allOf": [current]},
                        {"allOf": [previous]}
                    ]),
                );
                return;
            }
            if properties.contains_key("history_id")
                && properties.contains_key("schema_version")
                && properties.contains_key("harness")
                && !properties.contains_key("events")
            {
                let base = serde_json::Value::Object(object.clone());
                let mut measured = base.clone();
                measured["properties"]["schema_version"] = current_history_versions_schema();
                for field in ["started_at", "duration_ms", "model_ms", "tool_ms"] {
                    let required = measured["required"].as_array_mut().expect("required array");
                    if !required.iter().any(|value| value.as_str() == Some(field)) {
                        required.push(serde_json::Value::String(field.to_string()));
                    }
                }
                measured["properties"]["started_at"] =
                    serde_json::json!({"type": "string", "minLength": 1});
                for field in ["duration_ms", "model_ms", "tool_ms"] {
                    measured["properties"][field] =
                        serde_json::json!({"type": "integer", "minimum": 0});
                }
                measured["properties"]["observed_tool_ms"] = serde_json::Value::Bool(false);
                // Split by the runtime's own finish rule rather than a hand-kept
                // list, so a new `Status` variant lands on the right branch here
                // instead of quietly falling out of both.
                let (finished, unfinished) = status_split(requires_provider_finish);
                let mut terminal = measured.clone();
                terminal["properties"]["status"] = finished;
                terminal["properties"]["finished_at"] = serde_json::json!({"type": "string"});
                measured["properties"]["status"] = unfinished;
                let mut unavailable = base;
                unavailable["properties"]["schema_version"] = current_history_versions_schema();
                unavailable["properties"]["finished_at"] = serde_json::json!({"type": "null"});
                for field in [
                    "started_at",
                    "model_ms",
                    "tool_ms",
                    "time_to_first_token_ms",
                    "observed_tool_ms",
                ] {
                    unavailable["properties"][field] = serde_json::Value::Bool(false);
                    unavailable["required"]
                        .as_array_mut()
                        .expect("required array")
                        .retain(|value| value.as_str() != Some(field));
                }
                let mut observed = unavailable.clone();
                observed["properties"]["schema_version"] =
                    versions_schema(&timing_provenance_versions());
                observed["properties"]["observed_tool_ms"] =
                    serde_json::json!({"type": "integer", "minimum": 0});
                observed["properties"]["duration_ms"] =
                    serde_json::json!({"type": "integer", "minimum": 0});
                for field in ["duration_ms", "observed_tool_ms"] {
                    let required = observed["required"].as_array_mut().expect("required array");
                    if !required.iter().any(|value| value.as_str() == Some(field)) {
                        required.push(serde_json::Value::String(field.to_string()));
                    }
                }
                // A run that failed has no timing, or only the invocation bounds,
                // *because* it failed. Split by status so both stay disjoint from
                // the untimed success branch and `oneOf` keeps meaning exactly one.
                let (failed_statuses, untimed_success_statuses) = status_partition();
                let mut failed = unavailable.clone();
                failed["properties"]["status"] = failed_statuses.clone();
                let failed_partial = partial_branch(&unavailable, &failed_statuses);
                unavailable["properties"]["status"] = untimed_success_statuses;
                object.clear();
                object.insert(
                    "oneOf".to_string(),
                    serde_json::json!([
                        {"allOf": [terminal]},
                        {"allOf": [measured]},
                        {"allOf": [observed]},
                        {"allOf": [failed]},
                        {"allOf": [failed_partial]},
                        {"allOf": [unavailable]}
                    ]),
                );
                object.insert(
                    "allOf".to_string(),
                    serde_json::json!([
                        error_placement_gate(),
                        work_placement_gate(),
                        observed_model_version_gate(),
                        cancelled_version_gate(),
                        failure_kind_version_gate()
                    ]),
                );
                return;
            }
            object.values_mut().for_each(add_history_line_conditions);
        }
        _ => {}
    }
}

fn history_schema(schema: Schema) -> Schema {
    let mut value = serde_json::to_value(schema).expect("Schema serializes");
    add_v03_condition(&mut value);
    Schema::try_from(value).expect("conditional history schema remains an object")
}

/// Versions whose readers understand event timing *provenance* and
/// stdout-observed tool time, and whose writers always emit the composed harness
/// identity. Provider-measured timing itself predates them — this is about where
/// a measurement came from, not whether one exists.
fn timing_provenance_versions() -> Vec<&'static str> {
    versions_from(OBSERVED_TIMING_SCHEMA_VERSION)
}
/// The event-sourced versions before that: they still carry provider-measured
/// timing, but no event may say where its timing came from.
const PRE_TIMING_PROVENANCE_VERSIONS: &[&str] =
    &[FIRST_EVENT_SCHEMA_VERSION, PREVIOUS_CURRENT_SCHEMA_VERSION];
fn versions_schema(versions: &[&str]) -> serde_json::Value {
    serde_json::json!({"enum": versions, "type": "string"})
}

fn current_history_versions_schema() -> serde_json::Value {
    versions_schema(&versions_from(FIRST_EVENT_SCHEMA_VERSION))
}

/// Every [`Status`] as its serialized wire token, read out of the enum's own
/// generated schema so no list here can drift from the statuses a record can
/// actually carry.
fn status_values() -> Vec<serde_json::Value> {
    // llmlint: ignore[no_panics_on_recoverable_errors] Schema generation is a build-time codegen boundary like every sibling transformation in this module; a `Status` that no longer renders as a union of serialized consts is a generator invariant a caller cannot recover from, and emitting an empty status list would ship an SDK that silently accepts anything.
    let rendered = serde_json::to_value(schema_for!(Status)).expect("Status schema serializes");
    rendered["oneOf"]
        .as_array()
        .expect("Status renders as a union of serialized consts")
        .iter()
        .map(|variant| variant["const"].clone())
        .collect()
}

/// Split every [`Status`] by [`run_failed`] — the same predicate the runtime
/// completeness check applies. The variants come from the enum's own generated
/// schema and the split from the one runtime rule, so neither list can drift
/// from the statuses a record can carry or from which of them may omit timing.
///
/// Returns (failed, succeeded) as schemas ready to drop into a branch.
fn status_partition() -> (serde_json::Value, serde_json::Value) {
    status_split(run_failed)
}

/// Split every [`Status`] by `predicate`, as (matching, rest), each ready to drop
/// into a branch. The variants come from the enum's own generated schema, so no
/// caller can restate a list that drifts from the statuses a record can carry.
fn status_split(predicate: fn(Status) -> bool) -> (serde_json::Value, serde_json::Value) {
    let (mut matching, mut rest) = (Vec::new(), Vec::new());
    for name in status_values() {
        let status: Status =
            serde_json::from_value(name.clone()).expect("a generated Status variant reads back");
        if predicate(status) {
            matching.push(name);
        } else {
            rest.push(name);
        }
    }
    assert!(
        !matching.is_empty() && !rest.is_empty(),
        "both status branches must stay reachable"
    );
    (
        serde_json::json!({"enum": matching, "type": "string"}),
        serde_json::json!({"enum": rest, "type": "string"}),
    )
}

/// Restate an untimed branch as the *partial* one: the invocation bounds the
/// runner observed, with no provider/tool split derived from them. Legible only
/// on a run that failed, which is what `statuses` pins — and disjoint from every
/// other branch, since the measured ones require the split this forbids and the
/// untimed ones forbid the `started_at` this requires.
///
/// The arithmetic the runtime also checks (a first-token offset within the run's
/// duration) is not expressible here, exactly as `model_ms + tool_ms <= duration`
/// is not on the measured branches; this states the shape, and the runtime states
/// the sums.
fn partial_branch(untimed: &serde_json::Value, statuses: &serde_json::Value) -> serde_json::Value {
    let mut partial = untimed.clone();
    partial["properties"]["schema_version"] =
        versions_schema(&versions_from(FIRST_PARTIAL_TIMING_SCHEMA_VERSION));
    partial["properties"]["status"] = statuses.clone();
    partial["properties"]["started_at"] = serde_json::json!({"type": "string", "minLength": 1});
    partial["properties"]["duration_ms"] = serde_json::json!({"type": "integer", "minimum": 0});
    partial["properties"]["time_to_first_token_ms"] =
        serde_json::json!({"type": ["integer", "null"], "minimum": 0});
    // `finished_at` stays as the untimed branch left it: a run cut short has no
    // provider finish to report, whatever instant the process itself stopped at.
    for field in ["started_at", "duration_ms"] {
        let required = partial["required"].as_array_mut().expect("required array");
        if !required.iter().any(|value| value.as_str() == Some(field)) {
            required.push(serde_json::Value::String(field.to_string()));
        }
    }
    partial
}

/// The failure `error` text is legible only to a reader at or after the version
/// that introduced it, and only on a record whose run reported a failure — by
/// status, or by a classified `failure_kind` on an otherwise clean exit (the
/// deferred-tool dead-end). Both are cross-field rules the runtime check applies,
/// and both are stated here so a generated SDK validator accepts exactly what the
/// CLI's own reader does.
///
/// Returned as a standalone `oneOf` so a caller can hang it beside a branch list
/// rather than inside it: JSON Schema ANDs keywords at one level, so one gate
/// covers every timing branch instead of being restated in each.
fn error_placement_gate() -> serde_json::Value {
    let (failed_statuses, _) = status_partition();
    // llmlint: ignore[no_panics_on_recoverable_errors] Schema generation is a build-time codegen boundary like every sibling transformation here; a `FailureKind` that no longer serializes to its wire token is a generator invariant, and emitting a gate that accepts any kind would ship an SDK looser than the reader it describes.
    let deferred_dead_end = serde_json::to_value(FailureKind::ToolDeferred)
        .expect("FailureKind serializes to its wire token");
    serde_json::json!({
        "oneOf": [
            // Nothing to report: the field is absent, or present but empty of a value.
            {"type": "object", "properties": {"error": {"type": "null"}}},
            // Reported: only at the version that reads it, and only where there
            // was a failure to report.
            {"allOf": [
                {
                    "type": "object",
                    "properties": {
                        "schema_version": versions_schema(
                            &versions_from(FIRST_ERROR_SCHEMA_VERSION)
                        ),
                        "error": {"type": "string"}
                    },
                    "required": ["error"]
                },
                {"anyOf": [
                    {"type": "object", "properties": {"status": failed_statuses}},
                    {
                        "type": "object",
                        "properties": {"failure_kind": {"const": deferred_dead_end}},
                        "required": ["failure_kind"]
                    }
                ]}
            ]}
        ]
    })
}

/// The `work` reading is legible only to a reader at or after the version that
/// introduced it, and belongs only to a record whose run the **harness itself**
/// failed ([`attempted_failure`]). Both are the cross-field rules the runtime
/// check applies ([`crate::domain::history`]'s `work_evidence_valid`), stated
/// here so a generated SDK validator accepts exactly what the CLI's own reader
/// does.
///
/// Narrower than [`error_placement_gate`] on purpose: that one admits the
/// deferred-tool clean exit and a harness that was never spawned, and this one
/// admits neither — the first has a classified kind and the second says in its
/// status that nothing ran.
///
/// The *writer* is narrower still: it records a reading only where nothing
/// classified the failure, since a kind that names the cause has already
/// answered the question. That stays a writer rule rather than a fourth
/// condition here, for one reason and not for taste: a record carrying both is
/// consistent rather than corrupt (the kind and the reading say two true
/// things), and a third narrowing of `failure_kind` in this bundle makes the
/// generated TypeScript union unresolvable — so enforcing it would buy a
/// stricter reader at the price of an SDK that cannot describe the contract at
/// all. `domain::history`'s own tests hold the writer to it.
fn work_placement_gate() -> serde_json::Value {
    // Split by the runtime's own predicate, not by `run_failed`: `skipped` and
    // `spawn_error` never carry a reading, so a validator that accepted one
    // would be looser than the reader it describes.
    let (attempted, _) = status_split(attempted_failure);
    serde_json::json!({
        "oneOf": [
            // Nothing to read: the field is absent, or present but empty of a value.
            {"type": "object", "properties": {"work": {"type": "null"}}},
            // Read: only at the version that has it, and only on a failure the
            // harness itself reached.
            {"allOf": [
                {
                    "type": "object",
                    "properties": {
                        "schema_version": versions_schema(
                            &versions_from(FIRST_WORK_EVIDENCE_SCHEMA_VERSION)
                        ),
                        "work": {"enum": work_values(), "type": "string"},
                        "status": attempted
                    },
                    "required": ["work"]
                }
            ]}
        ]
    })
}

/// Every [`RunWork`] reading as its serialized wire token, read out of the
/// enum's own generated schema so no list here can drift from the values a
/// record can carry — the [`status_values`] rule, applied once more.
fn work_values() -> Vec<serde_json::Value> {
    // llmlint: ignore[no_panics_on_recoverable_errors] Schema generation is a build-time codegen boundary like every sibling transformation in this module; a `RunWork` that no longer renders as a union of serialized consts is a generator invariant a caller cannot recover from, and emitting an empty list would ship an SDK that silently accepts anything.
    let rendered = serde_json::to_value(schema_for!(RunWork)).expect("RunWork schema serializes");
    rendered["oneOf"]
        .as_array()
        .expect("RunWork renders as a union of serialized consts")
        .iter()
        .map(|variant| variant["const"].clone())
        .collect()
}

/// The observed model is legible only to a reader at or after the version that
/// introduced it — the rule [`crate::domain::history`]'s `observed_model_valid`
/// applies, stated here so a generated SDK validator refuses the same v1.7
/// record carrying a field v1.7 never had. No cross-field condition: the field
/// is the harness's own report and is legal on any run that produced one.
fn observed_model_version_gate() -> serde_json::Value {
    serde_json::json!({
        "oneOf": [
            {"type": "object", "properties": {"observed_model": {"type": "null"}}},
            {
                "type": "object",
                "properties": {
                    "schema_version": versions_schema(
                        &versions_from(FIRST_MODEL_OBSERVATION_SCHEMA_VERSION)
                    ),
                    "observed_model": {"type": "string"}
                },
                "required": ["observed_model"]
            }
        ]
    })
}

/// The `cancelled` status is legible only to a reader at or after the version
/// that introduced it. Stated as its own gate, alongside
/// [`error_placement_gate`], so every timing branch inherits the rule instead of
/// each branch restating it — and so a v1.3 record claiming a status v1.3 never
/// had is refused rather than silently accepted by a generated validator.
fn cancelled_version_gate() -> serde_json::Value {
    // llmlint: ignore[no_panics_on_recoverable_errors] Schema generation is a build-time codegen boundary like every sibling transformation here; a `Status` that no longer serializes to its wire token is a generator invariant, and emitting a gate that accepts the value at any version would ship an SDK looser than the reader it describes.
    let cancelled =
        serde_json::to_value(Status::Cancelled).expect("Status serializes to its wire token");
    // Spelled as the *other* statuses rather than a negated one: every generated
    // validator enforces an enum, and `not` is a keyword they would have to grow.
    let others: Vec<_> = status_values()
        .into_iter()
        .filter(|value| *value != cancelled)
        .collect();
    serde_json::json!({
        "anyOf": [
            {
                "type": "object",
                "properties": {"status": {"enum": others, "type": "string"}}
            },
            {
                "type": "object",
                "properties": {
                    "schema_version": versions_schema(
                        &versions_from(FIRST_CANCELLED_SCHEMA_VERSION)
                    )
                }
            }
        ]
    })
}

/// A failure kind introduced after a record's version is legible only to a
/// reader at or after the version that introduced it — the same promise
/// [`cancelled_version_gate`] makes for the `cancelled` status, on the other
/// gated enum. Stated as its own gate so every timing branch inherits it once.
///
/// One sub-gate per introducing version, `allOf`-composed, and both the versions
/// and their kinds come from [`gated_failure_kind_version`] — the table the
/// runtime reader gates on. That is the whole point: a kind whose gate moves, or
/// a kind added with one, changes both validators from one edit rather than
/// leaving the SDK a version looser than the reader it describes.
///
/// Each sub-gate's allowed list carries `null` alongside the kinds it does not
/// gate, because `failure_kind` is optional: an unclassified record must satisfy
/// that branch without having to fall back on the version one.
fn failure_kind_version_gate() -> serde_json::Value {
    let mut gates: Vec<serde_json::Value> = Vec::new();
    for version in gated_versions() {
        let gated: Vec<serde_json::Value> = FailureKind::ALL
            .into_iter()
            .filter(|kind| gated_failure_kind_version(Some(*kind)) == Some(version))
            .map(failure_kind_value)
            .collect();
        let mut others: Vec<_> = failure_kind_values()
            .into_iter()
            .filter(|value| !gated.contains(value))
            .collect();
        others.push(serde_json::Value::Null);
        gates.push(serde_json::json!({
            "anyOf": [
                {
                    "type": "object",
                    "properties": {"failure_kind": {"enum": others}}
                },
                {
                    "type": "object",
                    "properties": {
                        "schema_version": versions_schema(&versions_from(version))
                    }
                }
            ]
        }));
    }
    serde_json::json!({"allOf": gates})
}

/// Every version that introduced a gated failure kind, oldest first and each
/// named once — read off [`FailureKind::ALL`] so the list cannot drift from the
/// gates themselves.
fn gated_versions() -> Vec<&'static str> {
    let gated: Vec<&'static str> = FailureKind::ALL
        .into_iter()
        .filter_map(|kind| gated_failure_kind_version(Some(kind)))
        .collect();
    // Every readable version, oldest first, kept only where a kind arrived —
    // so the order is the history contract's own and each version appears once.
    versions_from(FIRST_EVENT_SCHEMA_VERSION)
        .into_iter()
        .filter(|version| gated.contains(version))
        .collect()
}

/// One kind as its serialized wire token.
fn failure_kind_value(kind: FailureKind) -> serde_json::Value {
    // llmlint: ignore[no_panics_on_recoverable_errors] Schema generation is a build-time codegen boundary like every sibling transformation here; a `FailureKind` that no longer serializes to its wire token is a generator invariant, and emitting a gate that accepts the value at any version would ship an SDK looser than the reader it describes.
    serde_json::to_value(kind).expect("FailureKind serializes to its wire token")
}

/// Every [`FailureKind`] as its serialized wire token, read out of the enum's own
/// generated schema so no list here can drift from the kinds a record can carry —
/// the [`status_values`] rule, applied to the other gated enum.
fn failure_kind_values() -> Vec<serde_json::Value> {
    // llmlint: ignore[no_panics_on_recoverable_errors] Schema generation is a build-time codegen boundary like every sibling transformation in this module; a `FailureKind` that no longer renders as a union of serialized consts is a generator invariant a caller cannot recover from, and emitting an empty list would ship an SDK that silently accepts anything.
    let rendered =
        serde_json::to_value(schema_for!(FailureKind)).expect("FailureKind schema serializes");
    rendered["oneOf"]
        .as_array()
        .expect("FailureKind renders as a union of serialized consts")
        .iter()
        .map(|variant| variant["const"].clone())
        .collect()
}

fn set_history_identity_version(
    schema: &mut serde_json::Value,
    versions: &[&str],
    identity_required: bool,
) {
    schema["properties"]["schema_version"] = versions_schema(versions);
    let required = schema["required"]
        .as_array_mut()
        .expect("history required array");
    required.retain(|value| value.as_str() != Some("harness_id"));
    if identity_required {
        required.push(serde_json::Value::String("harness_id".to_string()));
    }
}

/// Restate one already-built record branch for the versions that predate event
/// timing provenance: the composed identity is optional there, and no event may
/// claim where its timing came from.
fn as_pre_provenance_branch(branch: &serde_json::Value) -> serde_json::Value {
    let mut restated = branch.clone();
    set_history_identity_version(&mut restated, PRE_TIMING_PROVENANCE_VERSIONS, false);
    forbid_record_event_timing_source(&mut restated);
    restated
}

fn forbid_action_event_timing_source(schema: &mut serde_json::Value) {
    let event = schema.take();
    *schema = serde_json::json!({
        "allOf": [
            event,
            {
                "type": "object",
                "properties": {"timing_source": false}
            }
        ]
    });
}

fn forbid_record_event_timing_source(schema: &mut serde_json::Value) {
    forbid_action_event_timing_source(&mut schema["properties"]["events"]["items"]);
}

fn add_v03_condition(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(values) => values.iter_mut().for_each(add_v03_condition),
        serde_json::Value::Object(object) => {
            let is_history = object
                .get("properties")
                .and_then(serde_json::Value::as_object)
                .is_some_and(|properties| {
                    properties.contains_key("history_id")
                        && properties.contains_key("schema_version")
                        && properties.contains_key("harness")
                });
            if is_history {
                let metadata = ["$schema", "$defs", "title", "description"]
                    .into_iter()
                    .filter_map(|key| object.remove(key).map(|value| (key.to_string(), value)))
                    .collect::<serde_json::Map<_, _>>();
                let base = serde_json::Value::Object(object.clone());
                let mut current = base.clone();
                set_history_identity_version(&mut current, &timing_provenance_versions(), true);
                let required = current["required"]
                    .as_array_mut()
                    .expect("history required array");
                for field in [
                    "started_at",
                    "finished_at",
                    "duration_ms",
                    "model_ms",
                    "tool_ms",
                ] {
                    if !required.iter().any(|value| value.as_str() == Some(field)) {
                        required.push(serde_json::Value::String(field.to_string()));
                    }
                }
                current["properties"]["started_at"] =
                    serde_json::json!({"type": "string", "minLength": 1});
                for field in ["duration_ms", "model_ms", "tool_ms"] {
                    current["properties"][field] =
                        serde_json::json!({"type": "integer", "minimum": 0});
                }
                current["properties"]["observed_tool_ms"] = serde_json::Value::Bool(false);
                let event_base = current["properties"]["events"]["items"].clone();
                let terminal_event = |status: &str, ended: bool| {
                    let mut properties = serde_json::json!({
                        "kind": {"const": "tool_call", "type": "string"},
                        "tool_call_id": {"type": "string", "minLength": 1},
                        "started_at": {"type": "string"},
                        "status": {"const": status, "type": "string"}
                    });
                    if ended {
                        properties["finished_at"] = serde_json::json!({"type": "string"});
                        properties["duration_ms"] =
                            serde_json::json!({"type": "integer", "minimum": 0});
                    }
                    let mut required =
                        serde_json::json!(["kind", "tool_call_id", "started_at", "status"]);
                    if ended {
                        required
                            .as_array_mut()
                            .expect("required fields are an array")
                            .extend([
                                serde_json::Value::String("finished_at".to_string()),
                                serde_json::Value::String("duration_ms".to_string()),
                            ]);
                    }
                    serde_json::json!({
                        "allOf": [event_base.clone(), {
                            "type": "object",
                            "properties": properties,
                            "required": required
                        }]
                    })
                };
                current["properties"]["events"]["items"] = serde_json::json!({
                    "oneOf": [
                        terminal_event("completed", true),
                        terminal_event("failed", true),
                        terminal_event("timeout", false),
                        terminal_event("interrupted", false),
                        {"allOf": [event_base.clone(), {
                            "type": "object",
                            "properties": {"kind": {"const": "tool_result", "type": "string"}}
                        }]},
                        // The agent's own text: one finished item, never timed.
                        // Ungated by version on purpose — `kind` is an open
                        // string on the wire, and a version bump would make
                        // every earlier reader refuse the record outright.
                        {"allOf": [event_base, {
                            "type": "object",
                            "properties": {
                                "kind": {"enum": ["message", "reasoning"], "type": "string"}
                            }
                        }]}
                    ]
                });
                let pre_provenance_current = as_pre_provenance_branch(&current);
                let mut unavailable = base.clone();
                set_history_identity_version(&mut unavailable, &timing_provenance_versions(), true);
                unavailable["properties"]["finished_at"] = serde_json::json!({"type": "null"});
                if let Some(required) = unavailable["required"].as_array_mut() {
                    required.retain(|value| {
                        !value.as_str().is_some_and(|field| {
                            matches!(
                                field,
                                "started_at" | "model_ms" | "tool_ms" | "time_to_first_token_ms"
                            )
                        })
                    });
                }
                for field in [
                    "started_at",
                    "model_ms",
                    "tool_ms",
                    "time_to_first_token_ms",
                    "observed_tool_ms",
                ] {
                    unavailable["properties"][field] = serde_json::Value::Bool(false);
                }
                unavailable["properties"]["events"]["items"] = serde_json::json!({
                    "type": "object",
                    "properties": {
                        "kind": {"type": "string"},
                        "name": {"type": ["string", "null"]},
                        "input": true,
                        "output": {"type": ["string", "null"]},
                        "index": {"type": "integer", "minimum": 0},
                        "tool_call_id": {"type": ["string", "null"]},
                        "started_at": {"type": "null"},
                        "finished_at": {"type": "null"},
                        "duration_ms": {"type": "null"},
                        "status": {"type": "null"}
                    },
                    "required": ["kind", "name", "input", "output", "index"]
                });
                // A run that failed has no timing *because* it failed, and the
                // events its failure interrupted keep the boundaries they did
                // reach. Splitting by status keeps this disjoint from the untimed
                // success branch above, so `oneOf` still means exactly one.
                let (failed_statuses, untimed_success_statuses) = status_partition();
                let mut failed = unavailable.clone();
                failed["properties"]["status"] = failed_statuses.clone();
                failed["properties"]["events"] = base["properties"]["events"].clone();
                let failed_partial = partial_branch(&failed, &failed_statuses);
                unavailable["properties"]["status"] = untimed_success_statuses;
                let pre_provenance_failed = as_pre_provenance_branch(&failed);
                let pre_provenance_unavailable = as_pre_provenance_branch(&unavailable);
                let mut observed = base.clone();
                set_history_identity_version(&mut observed, &timing_provenance_versions(), true);
                observed["properties"]["finished_at"] = serde_json::json!({"type": "null"});
                for field in [
                    "started_at",
                    "model_ms",
                    "tool_ms",
                    "time_to_first_token_ms",
                ] {
                    observed["properties"][field] = serde_json::Value::Bool(false);
                    observed["required"]
                        .as_array_mut()
                        .expect("required array")
                        .retain(|value| value.as_str() != Some(field));
                }
                for field in ["duration_ms", "observed_tool_ms"] {
                    let required = observed["required"].as_array_mut().expect("required array");
                    if !required.iter().any(|value| value.as_str() == Some(field)) {
                        required.push(serde_json::Value::String(field.to_string()));
                    }
                }
                observed["properties"]["duration_ms"] =
                    serde_json::json!({"type": "integer", "minimum": 0});
                observed["properties"]["observed_tool_ms"] =
                    serde_json::json!({"type": "integer", "minimum": 0});
                let mut legacy = base;
                legacy["properties"]["schema_version"] =
                    versions_schema(PRE_LIFECYCLE_RECORD_VERSIONS.as_slice());
                forbid_record_event_timing_source(&mut legacy);
                if let Some(required) = legacy["required"].as_array_mut() {
                    required.retain(|value| {
                        !value.as_str().is_some_and(|field| {
                            matches!(
                                field,
                                "started_at"
                                    | "finished_at"
                                    | "model_ms"
                                    | "tool_ms"
                                    | "time_to_first_token_ms"
                            )
                        })
                    });
                }
                legacy["properties"]["events"] = serde_json::json!({
                    "type": ["array", "null"],
                    "items": {
                        "type": "object",
                        "properties": {
                            "kind": {"type": "string"},
                            "name": {"type": ["string", "null"]},
                            "input": true,
                            "output": {"type": ["string", "null"]},
                            "index": {"type": "integer", "minimum": 0}
                        },
                        "required": ["kind", "name", "input", "output", "index"]
                    }
                });
                object.clear();
                object.extend(metadata);
                object.insert(
                    "oneOf".to_string(),
                    serde_json::json!([
                        current,
                        pre_provenance_current,
                        unavailable,
                        observed,
                        failed,
                        failed_partial,
                        pre_provenance_failed,
                        pre_provenance_unavailable,
                        legacy
                    ]),
                );
                object.insert(
                    "allOf".to_string(),
                    serde_json::json!([
                        error_placement_gate(),
                        work_placement_gate(),
                        observed_model_version_gate(),
                        cancelled_version_gate(),
                        failure_kind_version_gate()
                    ]),
                );
                return;
            }
            object.values_mut().for_each(add_v03_condition);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn every_named_schema_root_is_emitted_by_the_bundle() {
        // A capability names the contracts its SDK methods validate against. A
        // root nothing emits is an SDK pointed at a contract with no generated
        // source and no freshness gate — which is the exact defect the manifest
        // exists to prevent, so it must not be expressible in the manifest
        // itself. A verb whose output is not yet a typed contract says `None`.
        let emitted = serde_json::to_value(bundle()).expect("the bundle serializes");
        let emitted = emitted.as_object().expect("the bundle is an object");
        for capability in crate::domain::capability::CAPABILITIES {
            for root in capability.options.into_iter().chain(capability.output()) {
                assert!(
                    emitted.contains_key(root),
                    "capability `{}` names the schema root `{root}`, which `bundle()` does not \
                     emit. Add it to `SdkSchemaBundle` (so both SDKs generate and validate \
                     against it), or record the gap with `None` rather than naming a root that \
                     does not exist.",
                    capability.method
                );
            }
        }
    }

    #[test]
    fn every_bound_option_is_a_field_of_its_options_contract() {
        // The manifest says which SDK option renders which CLI flag, and the
        // options contract is what a caller may actually set. When the two
        // disagree the flag is unreachable from every SDK at once — silently,
        // because the argv builder simply never sees the key. That is how
        // `history list --variant`, `--config` and `--no-config` came to be
        // bound by the manifest and settable by nobody.
        let emitted = serde_json::to_value(bundle()).expect("the bundle serializes");
        let mut unreachable: Vec<String> = Vec::new();
        for capability in crate::domain::capability::CAPABILITIES {
            let Some(root) = capability.options else {
                continue;
            };
            let contract = &emitted[root];
            let fields = accepted_properties(contract, contract);
            for binding in capability.bindings {
                if !fields.iter().any(|field| field == binding.option) {
                    unreachable.push(format!(
                        "{}: `{}` (rendering `{}`) is not a field of `{root}`",
                        capability.method,
                        binding.option,
                        if binding.flag().is_empty() {
                            "a positional argument"
                        } else {
                            binding.flag()
                        },
                    ));
                }
            }
        }
        assert!(
            unreachable.is_empty(),
            "every bound option must be a field a caller can set, or the flag it renders is \
             reachable from no SDK at all. Add the field to the options struct in \
             `domain::sdk`, or drop the binding and declare the flag uncovered with a \
             reason:\n  {}",
            unreachable.join("\n  ")
        );
    }

    use super::*;
    use serde_json::json;

    fn current_record(event: serde_json::Value) -> serde_json::Value {
        json!({
            "schema_version": "1.0",
            "history_id": "0198f0d0-7b31-7000-8000-000000000001",
            "session": "session", "name": "name", "labels": {}, "project": "/tmp/project",
            "timestamp": "2026-07-19T00:00:00Z", "harness": "codex", "model": null,
            "prompt": "test", "permission_mode": "default", "status": "ok", "exit_code": 0,
            "duration_ms": 10, "started_at": "2026-07-19T00:00:00Z",
            "finished_at": "2026-07-19T00:00:00Z", "model_ms": 4, "tool_ms": 3,
            "time_to_first_token_ms": null, "text": "done", "text_source": "json",
            "usage": {"input_tokens": null, "output_tokens": null, "cache_read_tokens": null,
                      "cache_write_tokens": null, "cost_usd": null},
            "session_id": null, "events": [event], "failure_kind": null
        })
    }

    #[test]
    fn history_show_contract_accepts_an_in_flight_run_beside_a_record() {
        use crate::domain::history::{HistoryId, IncompleteHistoryRun};
        let schema = serde_json::to_value(bundle().history_show_entries).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let event = json!({
            "kind": "message", "name": null, "input": null, "output": "partial", "index": 0,
            "tool_call_id": null, "started_at": null, "finished_at": null,
            "duration_ms": null, "status": null
        });
        // Built from the type `history show` serializes, so the contract is
        // checked against what the CLI prints rather than a hand-written copy.
        let run_id = "0198f0d0-7b31-7000-8000-000000000002"
            .parse::<HistoryId>()
            .unwrap();
        let in_flight = IncompleteHistoryRun::new(
            run_id,
            "codex".to_string(),
            vec![serde_json::from_value(event.clone()).unwrap()],
        )
        .unwrap();
        assert!(IncompleteHistoryRun::new(run_id, "codex".to_string(), Vec::new()).is_none());
        let in_flight = serde_json::to_value(in_flight).unwrap();
        assert_eq!(in_flight["type"], "incomplete");
        assert!(validator.is_valid(&json!([current_record(event.clone()), in_flight])));

        let mut untyped = in_flight.clone();
        untyped.as_object_mut().unwrap().remove("type");
        assert!(!validator.is_valid(&json!([untyped])));
        let mut emptied = in_flight.clone();
        emptied["events"] = json!([]);
        assert!(!validator.is_valid(&json!([emptied])));
        let mut eventless = in_flight;
        eventless.as_object_mut().unwrap().remove("events");
        assert!(!validator.is_valid(&json!([eventless])));
    }

    #[test]
    fn current_schema_rejects_incomplete_and_unfinished_terminal_tool_calls() {
        let schema = serde_json::to_value(bundle().history_record).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let base = json!({
            "kind": "tool_call", "name": "shell", "input": {}, "output": "ok", "index": 0,
            "tool_call_id": "call-1", "started_at": "2026-07-19T00:00:00Z",
            "finished_at": "2026-07-19T00:00:00Z", "duration_ms": 3, "status": "completed"
        });
        assert!(validator.is_valid(&current_record(base.clone())));
        for field in ["tool_call_id", "started_at", "status"] {
            let mut invalid = base.clone();
            invalid[field] = serde_json::Value::Null;
            assert!(!validator.is_valid(&current_record(invalid)), "{field}");
        }
        for field in ["finished_at", "duration_ms"] {
            let mut invalid = base.clone();
            invalid[field] = serde_json::Value::Null;
            assert!(!validator.is_valid(&current_record(invalid)), "{field}");
            let mut omitted = base.clone();
            omitted.as_object_mut().unwrap().remove(field);
            assert!(!validator.is_valid(&current_record(omitted)), "{field}");
        }
    }

    #[test]
    fn current_schema_rejects_null_required_timing_fields() {
        let schema = serde_json::to_value(bundle().history_record).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let event = json!({
            "kind": "tool_result", "name": null, "input": null, "output": "ok", "index": 0,
            "tool_call_id": null, "started_at": null, "finished_at": null,
            "duration_ms": null, "status": null
        });
        let valid = current_record(event);
        assert!(validator.is_valid(&valid));
        for field in ["started_at", "duration_ms", "model_ms", "tool_ms"] {
            let mut invalid = valid.clone();
            invalid[field] = serde_json::Value::Null;
            assert!(!validator.is_valid(&invalid), "{field}");
        }
    }

    #[test]
    fn current_schema_accepts_unavailable_timing_but_rejects_partial_timing() {
        let schema = serde_json::to_value(bundle().history_record).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let event = json!({
            "kind": "tool_result", "name": null, "input": null, "output": "ok", "index": 0,
            "tool_call_id": null, "started_at": null, "finished_at": null,
            "duration_ms": null, "status": null
        });
        let mut unavailable = current_record(event);
        unavailable["finished_at"] = serde_json::Value::Null;
        for field in [
            "started_at",
            "model_ms",
            "tool_ms",
            "time_to_first_token_ms",
        ] {
            unavailable.as_object_mut().unwrap().remove(field);
        }
        assert!(validator.is_valid(&unavailable));

        let mut omitted_event_timing = unavailable.clone();
        for field in ["started_at", "finished_at", "duration_ms", "status"] {
            omitted_event_timing["events"][0]
                .as_object_mut()
                .unwrap()
                .remove(field);
        }
        assert!(validator.is_valid(&omitted_event_timing));

        unavailable["model_ms"] = json!(1);
        assert!(!validator.is_valid(&unavailable));

        unavailable.as_object_mut().unwrap().remove("model_ms");
        unavailable["events"][0]["started_at"] = json!("2026-07-19T00:00:00Z");
        assert!(!validator.is_valid(&unavailable));
    }

    #[test]
    fn current_schema_distinguishes_stdout_observed_from_provider_timing() {
        let schema = serde_json::to_value(bundle().history_record).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let event = json!({
            "kind": "tool_call", "name": "Bash", "input": {}, "output": "ok", "index": 0,
            "tool_call_id": "call-1", "started_at": "2026-07-19T00:00:00Z",
            "finished_at": "2026-07-19T00:00:00Z", "duration_ms": 3,
            "status": "completed", "timing_source": "stdout_observed"
        });
        let mut previous = current_record(event.clone());
        previous["schema_version"] = json!("1.1");
        assert!(!validator.is_valid(&previous));

        let mut observed = current_record(event);
        observed["schema_version"] = json!("1.2");
        observed["harness_id"] = json!("codex");
        observed["finished_at"] = serde_json::Value::Null;
        for field in [
            "started_at",
            "model_ms",
            "tool_ms",
            "time_to_first_token_ms",
        ] {
            observed.as_object_mut().unwrap().remove(field);
        }
        observed["observed_tool_ms"] = json!(3);
        assert!(validator.is_valid(&observed));

        observed["model_ms"] = json!(1);
        assert!(!validator.is_valid(&observed));
        observed.as_object_mut().unwrap().remove("model_ms");
        observed["events"][0]["timing_source"] = json!("estimated");
        assert!(!validator.is_valid(&observed));
    }
}
