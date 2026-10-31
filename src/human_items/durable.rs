//! The human-item journal's durable append, and the emergency capacity that
//! lets the store record a full disk on the disk that is full.
//!
//! A record is acknowledged only once it has been synced. A write the
//! filesystem refuses for want of space is taken back off the file, and the
//! store's **reserve** — a file of `RESERVE_BYTES` allocated while there was
//! room — is released so the same record can be written into the space it held.
//! Where even that write fails, the refusal says so and nothing is acknowledged.
//! The reserve is allocated again the next time the filesystem has room, which
//! is also the probe that says a journal outage is over.

use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use super::HumanItemRefusal;

/// How much space the reserve holds back for the records that report a full disk.
pub(crate) const RESERVE_BYTES: usize = 256 * 1024;

/// How an append reached the disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Appended {
    /// Into space the filesystem had.
    Normally,
    /// Into the space the reserve was holding: the filesystem had none.
    FromReserve,
}

/// Whether an error is the filesystem having no room for the write.
pub(crate) fn is_no_space(error: &std::io::Error) -> bool {
    // llmlint: ignore[changed_behavior_has_e2e] a full filesystem (ENOSPC) and a file past its size limit (EFBIG) are each driven for real by `tests/human_items_live.rs`; a disk quota (EDQUOT) needs quotas enabled on a mounted filesystem, which no unprivileged journey can arrange, and it is the same refusal of the same write.
    if error.kind() == std::io::ErrorKind::StorageFull
        || error.kind() == std::io::ErrorKind::QuotaExceeded
        || error.kind() == std::io::ErrorKind::FileTooLarge
    {
        return true;
    }
    #[cfg(unix)]
    {
        matches!(
            error.raw_os_error(),
            Some(libc::ENOSPC | libc::EDQUOT | libc::EFBIG)
        )
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// The reserve beside a journal.
pub(crate) fn reserve_of(journal: &Path) -> PathBuf {
    journal.with_file_name("reserve")
}

/// Allocate the reserve if it is not held, answering whether the filesystem had
/// room for it. A partial reserve is removed rather than kept.
pub(crate) fn ensure_reserve(journal: &Path) -> std::io::Result<()> {
    let path = reserve_of(journal);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() == RESERVE_BYTES as u64) {
        return Ok(());
    }
    let written = std::fs::File::create(&path).and_then(|mut file| {
        file.write_all(&vec![0u8; RESERVE_BYTES])?;
        file.sync_all()
    });
    if written.is_err() {
        let _ = std::fs::remove_file(&path);
    }
    written
}

/// Write one record at the end of a locked, healed journal handle and sync it.
/// A failed write leaves the file on the boundary it started at.
fn write_synced(file: &mut std::fs::File, line: &str) -> std::io::Result<()> {
    let boundary = file.seek(SeekFrom::End(0))?;
    let written = crate::sys::append_all(file, format!("{line}\n").as_bytes())
        .and_then(|()| file.sync_data());
    if written.is_err() {
        let _ = file.set_len(boundary);
        let _ = file.sync_data();
    }
    written
}

/// Append one record durably, releasing the reserve for it when the filesystem
/// has no room. Refuses, acknowledging nothing, when the record could not be
/// made durable either way.
pub(crate) fn append(
    path: &Path,
    file: &mut std::fs::File,
    line: &str,
) -> Result<Appended, HumanItemRefusal> {
    match write_synced(file, line) {
        Ok(()) => Ok(Appended::Normally),
        Err(error) if is_no_space(&error) => {
            let reserve = reserve_of(path);
            let released = std::fs::remove_file(&reserve).is_ok();
            match (released, write_synced(file, line)) {
                (true, Ok(())) => Ok(Appended::FromReserve),
                (_, Err(again)) => Err(not_durable(path, &again, released)),
                (false, Ok(())) => Ok(Appended::Normally),
            }
        }
        Err(error) => Err(not_durable(path, &error, false)),
    }
}

fn not_durable(path: &Path, error: &std::io::Error, released: bool) -> HumanItemRefusal {
    HumanItemRefusal::Conflict {
        reason: format!(
            "the human-item journal {} could not record this durably ({error}){}; nothing was \
             acknowledged — free space on that filesystem and act again",
            path.display(),
            if released {
                ", even into its emergency reserve"
            } else {
                ", and it held no emergency reserve to release"
            }
        ),
    }
}
