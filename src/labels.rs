//! Launch labels and the replaceable, non-journalled reading overlay.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    ledger::{self, LaunchRecord, RunPaths},
    Error, Result,
};

/// A label key matching `^[a-z][a-z0-9-]{0,31}$`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct LabelKey(String);

impl TryFrom<String> for LabelKey {
    type Error = String;
    fn try_from(key: String) -> std::result::Result<Self, String> {
        if key.is_empty()
            || key.len() > 32
            || !key.as_bytes()[0].is_ascii_lowercase()
            || !key
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        {
            return Err(format!("invalid label key '{key}'"));
        }
        Ok(Self(key))
    }
}

impl LabelKey {
    /// The validated key.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One line of 1 to 64 Unicode characters, held opaquely by the engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct LabelValue(String);

impl TryFrom<String> for LabelValue {
    type Error = String;
    fn try_from(value: String) -> std::result::Result<Self, String> {
        if value.is_empty() || value.chars().count() > 64 || value.contains(['\n', '\r']) {
            return Err("label value must be one line of 1 to 64 characters".into());
        }
        Ok(Self(value))
    }
}

impl LabelValue {
    /// The opaque value.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A syntactically qualified plan-store project id, never looked up by labelling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct RunGroup(String);

impl TryFrom<String> for RunGroup {
    type Error = String;
    fn try_from(value: String) -> std::result::Result<Self, String> {
        crate::taskgraph::QualifiedId::try_from(value.clone())?;
        Ok(Self(value))
    }
}

impl RunGroup {
    /// The qualified project id.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A replacement overlay's grouping instruction: omission, explicit id, or explicit null.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum LabelGroup {
    /// Use the group resolved from the immutable launch record.
    #[default]
    Keep,
    /// Use this group instead.
    Set(RunGroup),
    /// Suppress even the launch record's inferred grouping.
    Clear,
}

impl LabelGroup {
    fn is_keep(&self) -> bool {
        matches!(self, Self::Keep)
    }
}

impl Serialize for LabelGroup {
    fn serialize<S: serde::Serializer>(&self, writer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Set(group) => group.serialize(writer),
            Self::Keep | Self::Clear => writer.serialize_none(),
        }
    }
}

impl<'de> Deserialize<'de> for LabelGroup {
    fn deserialize<D: serde::Deserializer<'de>>(reader: D) -> std::result::Result<Self, D::Error> {
        Option::<RunGroup>::deserialize(reader).map(|group| group.map_or(Self::Clear, Self::Set))
    }
}

/// An RFC 3339 timestamp retained exactly as written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct LabelTimestamp(String);

impl TryFrom<String> for LabelTimestamp {
    type Error = String;
    fn try_from(value: String) -> std::result::Result<Self, String> {
        if !crate::watchers::is_rfc3339(&value) {
            return Err(format!("invalid label set_at '{value}': expected RFC 3339"));
        }
        Ok(Self(value))
    }
}

impl LabelTimestamp {
    /// The validated RFC 3339 timestamp.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The complete `labels.json` overlay written by the label verb.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LabelOverlay {
    /// Overlay schema version, currently 1.
    pub schema_version: u32,
    /// Labels applied after removing `unlabel` keys from launch labels.
    pub labels: BTreeMap<LabelKey, LabelValue>,
    /// Launch keys to remove before applying overlay labels.
    pub unlabel: Vec<LabelKey>,
    /// Absent preserves launch grouping; null suppresses it.
    #[serde(default, skip_serializing_if = "LabelGroup::is_keep")]
    pub group: LabelGroup,
    /// When this overlay was written, in RFC 3339.
    pub set_at: LabelTimestamp,
    /// The caller who wrote the overlay.
    pub set_by: LabelAuthor,
}

/// Who replaced a run's label overlay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LabelAuthor {
    /// The calling launcher.
    pub launcher: String,
    /// The calling session.
    pub session: String,
}

pub(crate) fn parse(values: &[String]) -> Result<BTreeMap<LabelKey, LabelValue>> {
    let mut labels = BTreeMap::new();
    for entry in values {
        let (name, value) = entry.split_once('=').ok_or_else(|| {
            Error::Refused(format!("invalid label '{entry}': expected KEY=VALUE"))
        })?;
        let key = LabelKey::try_from(name.to_owned()).map_err(Error::Refused)?;
        let value = LabelValue::try_from(value.to_owned())
            .map_err(|why| Error::Refused(format!("invalid label '{name}': {why}")))?;
        if labels.insert(key, value).is_some() {
            return Err(Error::Refused(format!("duplicate label key '{name}'")));
        }
    }
    Ok(labels)
}

pub(crate) fn raw(labels: &BTreeMap<LabelKey, LabelValue>) -> BTreeMap<String, String> {
    labels
        .iter()
        .map(|(key, value)| (key.0.clone(), value.0.clone()))
        .collect()
}

pub(crate) fn group(value: Option<&str>) -> Result<Option<RunGroup>> {
    value
        .map(|value| RunGroup::try_from(value.to_owned()).map_err(Error::Refused))
        .transpose()
}

pub(crate) fn effective(
    paths: &RunPaths,
    launch: &LaunchRecord,
) -> Result<(BTreeMap<String, String>, Option<String>)> {
    apply(paths, launch.labels.clone(), resolved_group(launch))
}

pub(crate) fn resolved_group(launch: &LaunchRecord) -> Option<String> {
    launch
        .group
        .clone()
        .or_else(|| launch.project_origin.clone())
        .or_else(|| (!launch.project.is_empty()).then(|| launch.project.clone()))
}

pub(crate) fn apply(
    paths: &RunPaths,
    mut labels: BTreeMap<String, String>,
    mut group: Option<String>,
) -> Result<(BTreeMap<String, String>, Option<String>)> {
    let path = paths.dir.join("labels.json");
    if path.exists() {
        let overlay: LabelOverlay = ledger::read_json(&path)?;
        if overlay.schema_version != 1 {
            return Err(Error::Refused(format!(
                "unsupported labels schema_version {}",
                overlay.schema_version
            )));
        }
        for name in &overlay.unlabel {
            labels.remove(name.as_str());
        }
        labels.extend(raw(&overlay.labels));
        match overlay.group {
            LabelGroup::Keep => {}
            LabelGroup::Set(value) => group = Some(value.0),
            LabelGroup::Clear => group = None,
        }
    }
    Ok((labels, group))
}

pub(crate) fn replace(
    paths: &RunPaths,
    request: crate::verbs::LabelRequest<'_>,
) -> Result<LabelOverlay> {
    let launch: LaunchRecord = ledger::read_json(&paths.launch())?;
    let state = crate::checkpoint::fold_and_checkpoint(paths);
    // A copied run can prove its driver exited without this host knowing that PID.
    // The exit is written before releasing ownership, so the matching lock still wins.
    let exited = crate::journal::read(&paths.journal())
        .iter()
        .rev()
        .find(|event| {
            event.source == crate::event::Source::Pipeline
                && event.kind.0 == crate::journal::PipelineKind::DriverExited.as_str()
        })
        .and_then(|event| crate::projection::DriverClaim::of_stream(&event.stream))
        .filter(|claim| claim.is(launch.recorded_host(), launch.driver_pid()));
    let driven = if let Some(claim) = exited.as_ref() {
        !crate::views::let_go(
            launch.recorded_host(),
            launch.driver_pid(),
            Some(claim),
            paths,
        )
    } else {
        !crate::views::liveness(&launch, &state, paths).is_undriven()
    };
    if driven && !launch.owned_by(request.session) && !request.force {
        return Err(Error::NotOwned {
            run: paths.run.clone(),
            owner: launch.owner_label(request.session),
        });
    }
    let overlay = LabelOverlay {
        schema_version: 1,
        labels: request.labels.clone(),
        unlabel: request.unlabel.to_vec(),
        group: request.group,
        set_at: LabelTimestamp(crate::sys::now_rfc3339()),
        set_by: LabelAuthor {
            launcher: request.launcher.into(),
            session: request.session.into(),
        },
    };
    ledger::write_json(&paths.dir.join("labels.json"), &overlay)?;
    Ok(overlay)
}

pub(crate) fn launch_labels<'de, D: serde::Deserializer<'de>>(
    reader: D,
) -> std::result::Result<BTreeMap<String, String>, D::Error> {
    BTreeMap::<LabelKey, LabelValue>::deserialize(reader).map(|labels| raw(&labels))
}

pub(crate) fn launch_group<'de, D: serde::Deserializer<'de>>(
    reader: D,
) -> std::result::Result<Option<String>, D::Error> {
    Option::<RunGroup>::deserialize(reader).map(|group| group.map(|value| value.0))
}

pub(crate) fn origin(metadata: &BTreeMap<String, serde_json::Value>) -> Result<Option<String>> {
    match metadata.get("onetaskgraph.origin") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(value)) => group(Some(value))
            .map(|value| value.map(|value| value.0))
            .map_err(|why| Error::Refused(format!("onetaskgraph.origin: {why}"))),
        Some(_) => Err(Error::Refused(
            "onetaskgraph.origin must be a qualified project id string or null".into(),
        )),
    }
}
