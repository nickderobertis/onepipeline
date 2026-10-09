//! The producer's typed wire shape for the stop-verdict budget records.
//!
//! `scripts/stop-guard-unpublished-budget.schema.json` is the reader's statement of
//! the same shape, and [`the_checked_schema_is_the_shape_this_producer_writes`] holds
//! the two to one another, key for key, so the journey and the read-only budget
//! commands cannot drift apart.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The record's own version.
pub(super) const VERSION: u32 = 1;

/// The record the journey writes, under `target/budget-records/`.
pub(super) const RECORD: &str = "stop-guard-unpublished.json";

/// The invocation manifest the producing tier starts and the journey completes.
pub(super) const INVOCATION: &str = "stop-guard-unpublished-invocation.json";

/// The copy of the producing binary, owned beside its telemetry.
pub(super) const BINARY_COPY: &str = "stop-guard-unpublished-onepipeline";

/// The record a load run writes instead, which no budget reads.
pub(super) const LOAD_RECORD: &str = "stop-guard-unpublished-load.json";

/// The scenarios every workload is measured in.
pub(super) const SCENARIOS: [&str; 3] = ["none", "owed", "unreadable"];

/// Where the measured binary came from.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Build {
    /// The producing invocation's identity: 64 lowercase hex.
    pub run_id: String,
    /// The file name of the binary copy beside the record.
    pub binary: String,
    /// Its SHA-256.
    pub binary_sha256: String,
    /// `release` for a gate record.
    pub profile: String,
    /// The fingerprint of the build inputs the manifest names.
    pub source_sha256: String,
}

/// Ten timed calls of one mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Sample {
    /// Each call's wall clock, in microseconds, in the order taken.
    pub calls_us: Vec<u64>,
    /// Their median.
    pub median_us: u64,
    /// Their maximum.
    pub max_us: u64,
    /// load1 as the first call started.
    pub load1: f64,
}

/// One scenario of one workload: its verdict, its timings and its separate counts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Scenario {
    /// `none`, `owed` or `unreadable`.
    pub scenario: String,
    /// SHA-256 of the guard's standard output, the verdict every timed and counted
    /// call answered.
    pub verdict_sha256: String,
    /// One priming call, then ten unchanged-state calls.
    pub warm: Sample,
    /// Ten calls, every cache cleared before each; measured at scale 1 only.
    pub cold: Option<Sample>,
    /// Git executions of one separate warm counting call.
    pub warm_git: u64,
    /// Git executions of one separate cold counting call; scale 1 only.
    pub cold_git: Option<u64>,
}

/// One full workload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Workload {
    /// 1 or 10.
    pub scale: u32,
    /// Identities, retained sessions, launcher-labelled sessions, streams.
    pub shape: [usize; 4],
    /// The measured launcher's branches by recovery class.
    pub class_counts: BTreeMap<String, usize>,
    /// The run roots the measured session owns, for `unwatched`.
    pub own_runs: usize,
    /// Every scenario, in [`SCENARIOS`] order.
    pub scenarios: Vec<Scenario>,
}

/// The whole record.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Record {
    pub version: u32,
    pub build: Build,
    /// 0 for a gate record.
    pub load_workers: u32,
    /// Scale 1, then scale 10.
    pub workloads: Vec<Workload>,
    /// Empty fixture roots to both workloads built, in milliseconds.
    pub preparation_ms: u64,
    /// The whole journey, preparation included, in milliseconds.
    pub total_ms: u64,
    /// load1 as the journey started.
    pub load1: f64,
}

/// SHA-256 as lowercase hex.
pub(super) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prefix {
    directory: String,
    prefix: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inputs {
    version: u32,
    files: Vec<String>,
    directories: Vec<String>,
    prefixes: Vec<Prefix>,
}

/// The fingerprint `scripts/stop-guard-unpublished-build.mjs` computes over the same
/// manifest: SHA-256 of the JSON array of `[path, sha256(content)]`, paths sorted.
pub(super) fn source_fingerprint(root: &Path) -> String {
    let manifest: Inputs = serde_json::from_slice(
        &std::fs::read(root.join("scripts/stop-guard-unpublished-build-inputs.json"))
            .expect("the build-input manifest"),
    )
    .expect("the build-input manifest reads");
    assert_eq!(manifest.version, 1, "build-input manifest version");
    fn walk(root: &Path, relative: &str, into: &mut std::collections::BTreeSet<String>) {
        let path = root.join(relative);
        let meta = std::fs::symlink_metadata(&path).expect("a build input");
        if meta.is_dir() {
            for entry in std::fs::read_dir(&path).expect("a directory") {
                let name = entry.expect("an entry").file_name();
                walk(
                    root,
                    &format!("{relative}/{}", name.to_string_lossy()),
                    into,
                );
            }
        } else {
            assert!(meta.is_file(), "unsupported build input {relative}");
            into.insert(relative.to_owned());
        }
    }
    let mut files: std::collections::BTreeSet<String> = manifest.files.into_iter().collect();
    for directory in manifest.directories {
        walk(root, &directory, &mut files);
    }
    for Prefix { directory, prefix } in manifest.prefixes {
        for entry in std::fs::read_dir(root.join(&directory)).expect("a directory") {
            let name = entry.expect("an entry").file_name();
            let name = name.to_string_lossy();
            if name.starts_with(&prefix) {
                walk(root, &format!("{directory}/{name}"), &mut files);
            }
        }
    }
    let pairs: Vec<(String, String)> = files
        .into_iter()
        .map(|path| {
            let hash = digest(&std::fs::read(root.join(&path)).expect("a build input"));
            (path, hash)
        })
        .collect();
    digest(serde_json::to_string(&pairs).expect("pairs").as_bytes())
}

/// The key structure of a JSON value: every object's keys, recursively, with
/// arrays read through their first element.
fn shape_of(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), shape_of(value)))
                .collect(),
        ),
        serde_json::Value::Array(items) => items
            .iter()
            .find(|item| item.is_object())
            .map_or(serde_json::Value::Null, shape_of),
        _ => serde_json::Value::Null,
    }
}

/// The same key structure as the checked schema states it.
fn shape_of_schema(schema: &serde_json::Value, root: &serde_json::Value) -> serde_json::Value {
    let schema = match schema.get("$ref").and_then(serde_json::Value::as_str) {
        Some(reference) => {
            let name = reference.trim_start_matches("#/$defs/");
            &root["$defs"][name]
        }
        None => schema,
    };
    if let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) {
        let required: Vec<&str> = schema["required"]
            .as_array()
            .expect("every object schema names its required keys")
            .iter()
            .map(|key| key.as_str().expect("a key"))
            .collect();
        assert_eq!(schema["additionalProperties"], false, "closed objects only");
        assert_eq!(
            required.len(),
            properties.len(),
            "every key is required: {schema}"
        );
        return serde_json::Value::Object(
            properties
                .iter()
                .map(|(key, value)| (key.clone(), shape_of_schema(value, root)))
                .collect(),
        );
    }
    if let Some(items) = schema.get("items") {
        return shape_of_schema(items, root);
    }
    if let Some(any) = schema.get("anyOf").and_then(|a| a.as_array()) {
        return any
            .iter()
            .map(|option| shape_of_schema(option, root))
            .find(|shape| !shape.is_null())
            .unwrap_or(serde_json::Value::Null);
    }
    serde_json::Value::Null
}

/// A record with every optional field present, as the drift test writes it.
pub(super) fn sample_record() -> Record {
    let sample = Sample {
        calls_us: vec![1; 10],
        median_us: 1,
        max_us: 1,
        load1: 0.5,
    };
    Record {
        version: VERSION,
        build: Build {
            run_id: "a".repeat(64),
            binary: BINARY_COPY.into(),
            binary_sha256: "b".repeat(64),
            profile: "release".into(),
            source_sha256: "c".repeat(64),
        },
        load_workers: 0,
        workloads: vec![Workload {
            scale: 1,
            shape: [20, 371, 41, 2308],
            class_counts: BTreeMap::from([("no".into(), 1)]),
            own_runs: 1,
            scenarios: vec![Scenario {
                scenario: "owed".into(),
                verdict_sha256: "d".repeat(64),
                warm: sample.clone(),
                cold: Some(sample),
                warm_git: 1,
                cold_git: Some(1),
            }],
        }],
        preparation_ms: 1,
        total_ms: 2,
        load1: 0.5,
    }
}

#[test]
fn the_checked_schema_is_the_shape_this_producer_writes() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("scripts/stop-guard-unpublished-budget.schema.json");
    let schema: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).expect("the checked schema"))
            .expect("schema JSON");
    let mut written = serde_json::to_value(sample_record()).expect("a record serializes");
    // A map whose keys are data rather than fields: its values are what the schema
    // states, so it is compared as a leaf.
    written["workloads"][0]["class_counts"] = serde_json::Value::Null;
    assert_eq!(
        shape_of(&written),
        shape_of_schema(&schema, &schema),
        "the producer and the read-only reader must share one shape"
    );
    assert_eq!(schema["x-version"], VERSION);
    assert_eq!(
        schema["x-scenarios"],
        serde_json::json!(SCENARIOS),
        "the scenarios the reader requires"
    );
    assert_eq!(
        schema["x-workloads"],
        serde_json::json!({
            "1": onevcs_testing::recovery::Scale::One.counts(),
            "10": onevcs_testing::recovery::Scale::Ten.counts(),
        }),
        "the workloads the reader requires are the ones the fixture builds"
    );
    assert_eq!(schema["x-record"], RECORD);
    assert_eq!(schema["x-invocation"], INVOCATION);
    assert_eq!(schema["x-binary"], BINARY_COPY);
}
