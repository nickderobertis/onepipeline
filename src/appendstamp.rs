//! Append continuity for long-lived journal readers.
//!
//! A receipt is published under the journal writer's existing exclusive lock.
//! Its epoch survives only a write whose starting metadata matches the previous
//! receipt. An editor, replacement, truncation, failed write or old writer thus
//! breaks continuity, including a prefix edit followed by an ordinary append.
//! Readers never infer prefix integrity from growth alone. This is a cache,
//! not an authentication boundary: an actor forging both files owns the ledger.
use std::fs::{File, Metadata};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::ledger;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Stamp {
    pub(crate) len: u64,
    modified: SystemTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    created: Option<SystemTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    identity: Option<(u64, u64, i64, i64)>,
}

impl Stamp {
    fn of(metadata: Metadata) -> Option<Self> {
        #[cfg(unix)]
        let identity = {
            use std::os::unix::fs::MetadataExt;
            Some((
                metadata.dev(),
                metadata.ino(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            ))
        };
        #[cfg(not(unix))]
        let identity = None;
        Some(Self {
            len: metadata.len(),
            modified: metadata.modified().ok()?,
            created: metadata.created().ok(),
            identity,
        })
    }

    pub(crate) fn read(path: &Path) -> Option<Self> {
        Self::of(std::fs::metadata(path).ok()?)
    }
}

/// Private cache format. A different version is a cache miss, never journal loss.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Receipt {
    schema_version: u32,
    epoch: String,
    stamp: Stamp,
}

fn receipt_path(journal: &Path) -> PathBuf {
    journal.with_extension("append.json")
}

impl Receipt {
    pub(crate) fn read(journal: &Path, stamp: &Stamp) -> Option<Self> {
        let receipt: Self = ledger::read_json_opt(&receipt_path(journal))?;
        (receipt.schema_version == 1 && receipt.stamp == *stamp).then_some(receipt)
    }

    pub(crate) fn continues(&self, previous: &Self) -> bool {
        self.epoch == previous.epoch && self.stamp.len >= previous.stamp.len
    }
}

/// Called with the same locked descriptor that will append the next record.
/// A missing receipt starts a fresh epoch; it never vouches for older readers.
pub(crate) fn before(path: &Path, file: &File) -> Option<Receipt> {
    if path.file_name()? != "events.jsonl" {
        return None;
    }
    let stamp = Stamp::of(file.metadata().ok()?)?;
    let previous = Receipt::read(path, &stamp);
    Some(Receipt {
        schema_version: 1,
        epoch: previous.map_or_else(
            || {
                format!(
                    "{}-{}",
                    crate::sys::pid(),
                    SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos()
                )
            },
            |previous| previous.epoch,
        ),
        stamp,
    })
}

/// Failure to publish only disables the optimization: the journal already won.
pub(crate) fn after(path: &Path, file: &File, receipt: Option<Receipt>) {
    let Some(mut receipt) = receipt else { return };
    let Some(stamp) = file.metadata().ok().and_then(Stamp::of) else {
        return;
    };
    receipt.stamp = stamp;
    if let Err(error) = ledger::write_json(&receipt_path(path), &receipt) {
        eprintln!("onepipeline: append receipt: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_receipt_version_one_round_trips_the_checked_in_shape() {
        let golden = include_str!("../tests/golden/append-receipt-v1.json");
        let receipt = Receipt {
            schema_version: 1,
            epoch: "writer-epoch".into(),
            stamp: Stamp {
                len: 4096,
                // Windows file metadata uses 100 ns ticks; keep the fixture representable
                // there as well as on platforms with nanosecond SystemTime precision.
                modified: SystemTime::UNIX_EPOCH + std::time::Duration::new(1700000000, 100),
                created: None,
                identity: Some((1, 42, 1700000000, 123)),
            },
        };
        assert_eq!(
            serde_json::to_value(&receipt).expect("serializes"),
            serde_json::from_str::<serde_json::Value>(golden).expect("golden")
        );
        assert_eq!(
            serde_json::from_str::<Receipt>(golden).expect("round trip"),
            receipt
        );
    }
}
