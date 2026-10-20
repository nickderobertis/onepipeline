//! What a live driver is doing with each claimed envelope whose live note has
//! not been answered: the `deliveries.json` sidecar beside the channel's queues.
//!
//! A claimed envelope with no outcome line is, read off the queues alone, three
//! different facts: a note a live driver is still delivering into a turn, an
//! envelope that driver holds behind another envelope's note to the same node,
//! and one a driver that has gone claimed and will never answer. Only the driver
//! knows the first two, so it writes them down here — atomically, whenever what
//! it holds outstanding or held changes, and empty when it exits — and `status`
//! reads them together with the driver's liveness.
//!
//! **One declaration.** [`Delivery`] is both the sidecar's entry and the
//! `delivery` object `status --json` carries on each such envelope's reading, so
//! the writer and every reader import this type rather than restating it, and
//! `schemas/run-reading.schema.json` is generated from it.
//!
//! The file is additive: a build that does not know it never reads it, and a
//! missing or unreadable file means the run has no live delivery information,
//! never that a delivery was abandoned.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::views::RunPaths;

/// The sidecar's file name, inside the run's channel directory.
pub(crate) const DELIVERIES_FILE: &str = "deliveries.json";

/// The version of the sidecar document.
pub(crate) const DELIVERIES_SCHEMA_VERSION: u32 = 1;

/// Where one claimed, unanswered envelope's live note stands, as the run's live
/// driver last wrote it.
///
/// Envelope ids and instants are carried as numbers and written as the strings
/// the document states — a channel envelope id in decimal, an instant in RFC
/// 3339 — so a value read back that is neither is refused where it is read
/// rather than shown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Delivery {
    /// The driver is waiting on `node`'s conversation to answer the envelope's
    /// live note.
    Delivering {
        /// The node whose conversation the note was offered to.
        // llmlint: ignore[invalid_states_unrepresentable] a node id is the plan's own `String` everywhere this crate carries one — `Command`, `Node`, the journal's labels — and a newtype here alone would be a second spelling of it; what a reader is handed is checked non-blank and free of control characters by `node_id` as it is read.
        #[serde(deserialize_with = "node_id")]
        node: String,
        /// When it was offered, in epoch milliseconds — RFC 3339 on the wire.
        #[serde(with = "instant")]
        #[schemars(with = "String")]
        since: u64,
        /// When the note being offered stops waiting for a turn to take it, in
        /// epoch milliseconds — RFC 3339 on the wire. Present exactly where that
        /// note carries a `delivery_timeout_seconds`, and omitted otherwise.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "optional_instant"
        )]
        #[schemars(with = "Option<String>")]
        deadline: Option<u64>,
    },
    /// The driver holds the envelope unjudged behind `behind_envelope`'s live
    /// note to `node`, so that no later edit to that node overtakes it.
    Held {
        /// The node whose outstanding note it waits behind.
        // llmlint: ignore[invalid_states_unrepresentable] the node id, as `Delivering`'s `node` says.
        #[serde(deserialize_with = "node_id")]
        node: String,
        /// When it was first held, in epoch milliseconds — RFC 3339 on the wire.
        #[serde(with = "instant")]
        #[schemars(with = "String")]
        since: u64,
        /// The channel envelope id of the envelope whose note it waits behind —
        /// a decimal string on the wire.
        #[serde(with = "envelope_id")]
        #[schemars(with = "String")]
        behind_envelope: u64,
    },
}

/// One entry of the sidecar: an envelope, by its channel id, and where its
/// delivery stands. An envelope with notes to several receivers has one entry
/// per receiver still outstanding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct LiveDelivery {
    /// The channel envelope id — a decimal string on the wire.
    #[serde(with = "envelope_id")]
    #[schemars(with = "String")]
    pub(crate) envelope: u64,
    /// Where it stands.
    #[serde(flatten)]
    pub(crate) delivery: Delivery,
}

/// The whole sidecar document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct LiveDeliveries {
    /// [`DELIVERIES_SCHEMA_VERSION`].
    pub(crate) schema_version: u32,
    /// Every outstanding and held delivery, outstanding ones first.
    pub(crate) deliveries: Vec<LiveDelivery>,
}

/// A channel envelope id, written as the decimal string the document states.
pub(crate) mod envelope_id {
    use serde::{Deserialize, Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer>(id: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&id.to_string())
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse()
            .map_err(|_| serde::de::Error::custom(format!("{text:?} is not a channel envelope id")))
    }
}

/// An instant, in epoch milliseconds, written as RFC 3339.
mod instant {
    use serde::{Deserialize, Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer>(at: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&crate::sys::rfc3339_from_millis(*at))
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        let text = String::deserialize(deserializer)?;
        crate::projection::millis_of(&text)
            .ok_or_else(|| serde::de::Error::custom(format!("{text:?} is not an RFC 3339 instant")))
    }
}

/// An instant that may be absent, as [`instant`] writes one that is present.
mod optional_instant {
    use serde::{Deserialize, Deserializer, Serializer};

    // serde hands a `skip_serializing_if` field to its serializer only when it
    // is present.
    #[allow(clippy::ref_option, reason = "the signature serde's `with` requires")]
    pub(crate) fn serialize<S: Serializer>(
        at: &Option<u64>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match at {
            Some(at) => super::instant::serialize(at, serializer),
            None => serializer.serialize_none(),
        }
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<u64>, D::Error> {
        match Option::<String>::deserialize(deserializer)? {
            None => Ok(None),
            Some(text) => crate::projection::millis_of(&text)
                .map(Some)
                .ok_or_else(|| {
                    serde::de::Error::custom(format!("{text:?} is not an RFC 3339 instant"))
                }),
        }
    }
}

/// A node id as a reader is handed it: not blank, and with no control character.
fn node_id<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let node = String::deserialize(deserializer)?;
    if node.trim().is_empty() || node.chars().any(char::is_control) {
        return Err(serde::de::Error::custom(format!(
            "{node:?} is not a node id"
        )));
    }
    Ok(node)
}

impl LiveDeliveries {
    pub(crate) fn of(deliveries: Vec<LiveDelivery>) -> Self {
        Self {
            schema_version: DELIVERIES_SCHEMA_VERSION,
            deliveries,
        }
    }

    /// Write it, atomically, into the run's channel directory.
    pub(crate) fn write(&self, paths: &RunPaths) -> crate::Result<()> {
        crate::ledger::write_json(&paths.channel(DELIVERIES_FILE), self)
    }

    /// What the run's channel directory holds, or nothing where the file is
    /// absent, unreadable, of a version this build does not read, or carrying an
    /// entry that is not an envelope id, a node id and an RFC 3339 instant — each
    /// of which is no live delivery information.
    pub(crate) fn read(paths: &RunPaths) -> Option<Self> {
        let text = std::fs::read_to_string(paths.channel(DELIVERIES_FILE)).ok()?;
        serde_json::from_str::<Self>(&text)
            .ok()
            .filter(|read| read.schema_version == DELIVERIES_SCHEMA_VERSION)
    }

    /// Every entry for channel envelope `id`, in the order written.
    pub(crate) fn of_envelope(&self, id: u64) -> impl Iterator<Item = &Delivery> {
        self.deliveries
            .iter()
            .filter(move |entry| entry.envelope == id)
            .map(|entry| &entry.delivery)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The sidecar's wire shape is the one the run reading states: `state`
    /// beside the fields of its variant, and `behind_envelope` only when held.
    #[test]
    fn the_sidecar_round_trips_its_documented_shape() {
        let wire = json!({
            "schema_version": 1,
            "deliveries": [
                {"envelope": "1", "state": "delivering", "node": "build",
                 "since": "2026-10-18T09:00:00.000Z"},
                {"envelope": "4", "state": "delivering", "node": "docs",
                 "since": "2026-10-18T09:00:00.000Z", "deadline": "2026-10-18T09:00:02.000Z"},
                {"envelope": "2", "state": "held", "node": "build",
                 "since": "2026-10-18T09:00:01.000Z", "behind_envelope": "1"},
            ],
        });
        let read: LiveDeliveries = serde_json::from_value(wire.clone()).expect("it reads");
        let at = |ts: &str| crate::projection::millis_of(ts).expect("an instant");
        assert_eq!(
            read,
            LiveDeliveries::of(vec![
                LiveDelivery {
                    envelope: 1,
                    delivery: Delivery::Delivering {
                        node: "build".into(),
                        since: at("2026-10-18T09:00:00.000Z"),
                        deadline: None,
                    },
                },
                LiveDelivery {
                    envelope: 4,
                    delivery: Delivery::Delivering {
                        node: "docs".into(),
                        since: at("2026-10-18T09:00:00.000Z"),
                        deadline: Some(at("2026-10-18T09:00:02.000Z")),
                    },
                },
                LiveDelivery {
                    envelope: 2,
                    delivery: Delivery::Held {
                        node: "build".into(),
                        since: at("2026-10-18T09:00:01.000Z"),
                        behind_envelope: 1,
                    },
                },
            ])
        );
        assert_eq!(serde_json::to_value(&read).expect("it writes"), wire);
        assert_eq!(read.of_envelope(2).count(), 1);
        assert_eq!(read.of_envelope(3).count(), 0);
    }

    /// The sidecar's entry is the `delivery` object the run reading states:
    /// `schemas/run-reading.schema.json`'s `Delivery` is this type's schema,
    /// and every entry the sidecar carries is an envelope id beside exactly that
    /// object — so the file the driver writes and the document `status --json`
    /// prints cannot come to state two shapes.
    #[test]
    fn the_sidecar_entry_is_the_delivery_the_run_reading_schema_states() {
        let generated = schemars::generate::SchemaSettings::draft2020_12()
            .for_serialize()
            .into_generator()
            .into_root_schema_for::<Delivery>()
            .to_value();
        let committed: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("schemas/run-reading.schema.json"),
            )
            .expect("schemas/run-reading.schema.json ships"),
        )
        .expect("the committed schema is JSON");
        let body = |schema: &serde_json::Value| {
            let mut schema = schema.clone();
            if let Some(schema) = schema.as_object_mut() {
                schema.remove("$schema");
                schema.remove("title");
            }
            schema
        };
        assert_eq!(
            body(&committed["$defs"]["Delivery"]),
            body(&generated),
            "the run reading's `delivery` object is not the sidecar's `Delivery`"
        );
        // Each variant the reading states, as the sidecar writes it, is the
        // envelope id beside that variant's own fields.
        for variant in generated["oneOf"].as_array().expect("one schema per state") {
            let state = variant["properties"]["state"]["const"]
                .as_str()
                .expect("each variant names its state");
            let mut fields: Vec<String> = variant["properties"]
                .as_object()
                .expect("each variant has properties")
                .keys()
                .cloned()
                .collect();
            let delivery = match state {
                "delivering" => Delivery::Delivering {
                    node: "n".into(),
                    since: 0,
                    deadline: Some(2_000),
                },
                "held" => Delivery::Held {
                    node: "n".into(),
                    since: 0,
                    behind_envelope: 1,
                },
                other => panic!("the reading states a state this type has not: {other}"),
            };
            let written = serde_json::to_value(LiveDelivery {
                envelope: 2,
                delivery,
            })
            .expect("an entry writes");
            fields.push("envelope".into());
            fields.sort();
            let mut keys: Vec<String> = written
                .as_object()
                .expect("an entry is an object")
                .keys()
                .cloned()
                .collect();
            keys.sort();
            assert_eq!(
                keys, fields,
                "the sidecar's `{state}` entry is not the reading's"
            );
        }
    }

    /// An absent file, an unreadable one, and one of another version all read
    /// as no live delivery information.
    #[test]
    fn a_sidecar_this_build_cannot_read_is_no_information() {
        let root =
            std::env::temp_dir().join(format!("onepipeline-deliveries-{}", crate::sys::pid()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");
        assert_eq!(LiveDeliveries::read(&paths), None);
        std::fs::write(paths.channel(DELIVERIES_FILE), "not json").expect("written");
        assert_eq!(LiveDeliveries::read(&paths), None);
        std::fs::write(
            paths.channel(DELIVERIES_FILE),
            json!({"schema_version": 2, "deliveries": []}).to_string(),
        )
        .expect("written");
        assert_eq!(LiveDeliveries::read(&paths), None);
        // A document whose every field is the right kind, and whose values are
        // not what they say, reads as nothing too.
        for entry in [
            json!({"envelope": "one", "state": "delivering", "node": "build",
                   "since": "2026-10-18T09:00:00.000Z"}),
            json!({"envelope": "1", "state": "delivering", "node": "build",
                   "since": "yesterday"}),
            json!({"envelope": "1", "state": "delivering", "node": " ",
                   "since": "2026-10-18T09:00:00.000Z"}),
            json!({"envelope": "1", "state": "held", "node": "build\u{7}",
                   "since": "2026-10-18T09:00:00.000Z", "behind_envelope": "0"}),
            json!({"envelope": "1", "state": "held", "node": "build",
                   "since": "2026-10-18T09:00:00.000Z", "behind_envelope": "-1"}),
        ] {
            std::fs::write(
                paths.channel(DELIVERIES_FILE),
                json!({"schema_version": 1, "deliveries": [entry.clone()]}).to_string(),
            )
            .expect("written");
            assert_eq!(LiveDeliveries::read(&paths), None, "{entry} was read");
        }
        LiveDeliveries::of(Vec::new())
            .write(&paths)
            .expect("the sidecar is written");
        assert_eq!(
            LiveDeliveries::read(&paths),
            Some(LiveDeliveries::of(Vec::new()))
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
