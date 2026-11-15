//! Reading and writing the uniform session store on disk — the I/O half of
//! `run --session`. Resolves the platform state directory, reads/writes one JSON
//! file per named session, and reads the clock for timestamps. The record shape
//! and the create-vs-continue decision stay pure in [`crate::domain::session`].
//!
//! Layout: `<dir>/<project-slug>/<name>.json`, partitioned by a slug of the
//! project directory (shared with the history store) so the same name in
//! different projects never collides. One small file per named session, rewritten
//! atomically (temp + rename) as each run refreshes the token.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::domain::harness::HarnessIdentity;
use crate::domain::history::{format_rfc3339, project_slug, sanitize_name};
use crate::domain::session::{SessionRecord, SCHEMA_VERSION};

/// The file extension for every session record (one JSON document).
const SESSION_EXT: &str = "json";

/// Seconds since the UNIX epoch, UTC — the single clock read the session store
/// makes, kept here so [`crate::domain::session`] stays pure. A pre-1970 clock
/// falls back to the epoch rather than panicking (the store is best-effort).
fn now_epoch_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The per-user state directory, resolved exactly like [`crate::io::history`]:
/// `%LOCALAPPDATA%` on Windows; `$XDG_STATE_HOME` (else `~/.local/state`)
/// elsewhere.
fn state_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        return std::env::var_os("LOCALAPPDATA")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
    }
    if let Some(xdg) = std::env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(xdg));
    }
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(|home| PathBuf::from(home).join(".local").join("state"))
}

/// The effective session directory: the configured path if given, else the
/// platform default `<state dir>/oneharness/sessions`. `None` only when no path
/// was configured and the platform state dir cannot be resolved (no `$HOME`).
pub fn resolve_dir(configured: Option<&str>) -> Option<PathBuf> {
    match configured {
        Some(p) if !p.is_empty() => Some(PathBuf::from(p)),
        _ => state_dir().map(|d| d.join("oneharness").join("sessions")),
    }
}

/// The file backing session `name` for `project` under `dir`
/// (`<dir>/<project-slug>/<name>.json`). Pure path arithmetic; touches no disk.
#[must_use]
pub fn session_path(dir: &Path, project: &Path, name: &str) -> PathBuf {
    let slug = project_slug(&project.display().to_string());
    dir.join(slug)
        .join(format!("{}.{SESSION_EXT}", sanitize_name(name)))
}

/// Read the stored record at `path`, or `None` when it is absent or unreadable
/// (a corrupt/unparseable record is treated as absent — the run starts fresh
/// rather than aborting on a store fault). That includes a record whose `harness`
/// no longer parses as a [`crate::domain::harness::HarnessIdentity`]: there is
/// nothing to resume it against, so it reads as no record.
#[must_use]
pub fn read(path: &Path) -> Option<SessionRecord> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Create or update the record binding `name` → `token` for `harness` (the
/// **variant-qualified** identity — see [`SessionRecord::harness`]), preserving
/// the original `created` when a prior record exists. The project
/// subdirectory is created as needed and the file is written atomically (temp +
/// rename). Returns the I/O error for the caller to warn on — the store is
/// best-effort and never takes a run's results down.
pub fn write(
    path: &Path,
    project: &Path,
    harness: &HarnessIdentity,
    name: &str,
    token: &str,
    existing: Option<&SessionRecord>,
) -> std::io::Result<()> {
    let now = format_rfc3339(now_epoch_secs());
    let created = existing.map_or_else(|| now.clone(), |record| record.created.clone());
    let record = SessionRecord {
        schema_version: SCHEMA_VERSION.to_string(),
        name: sanitize_name(name),
        project: project.display().to_string(),
        harness: harness.clone(),
        token: token.to_string(),
        created,
        updated: now,
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut text = serde_json::to_string_pretty(&record)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    text.push('\n');
    // Write to a sibling temp file then rename, so a reader never observes a
    // half-written record.
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::scratch::ScratchDir;

    fn temp_dir(tag: &str) -> ScratchDir {
        ScratchDir::new(&format!("session-{tag}-{}", now_epoch_secs())).unwrap()
    }

    #[test]
    fn resolve_dir_prefers_configured_path() {
        assert_eq!(
            resolve_dir(Some("/custom/sessions")),
            Some(PathBuf::from("/custom/sessions"))
        );
        // Empty configured value falls back to the default resolution.
        let empty = resolve_dir(Some(""));
        let none = resolve_dir(None);
        assert_eq!(empty, none);
    }

    #[test]
    fn session_path_partitions_by_project_slug_and_name() {
        let path = session_path(
            Path::new("/store"),
            Path::new("/home/me/My Proj"),
            "Greet Flow",
        );
        assert_eq!(
            path,
            PathBuf::from("/store/home-me-My-Proj/greet-flow.json")
        );
    }

    #[test]
    fn read_missing_is_none() {
        let dir = temp_dir("missing");
        assert!(read(&dir.join("nope.json")).is_none());
    }

    #[test]
    fn write_then_read_round_trips_and_preserves_created() {
        let dir = temp_dir("roundtrip");
        let project = Path::new("/home/me/proj");
        let path = session_path(&dir, project, "chat");

        let alternate: HarnessIdentity = "claude-code:alternate".parse().unwrap();
        write(&path, project, &alternate, "chat", "sess-1", None).unwrap();
        let first = read(&path).unwrap();
        assert_eq!(first.token, "sess-1");
        // The whole identity, not the adapter: the token is only usable under the
        // variant that minted it.
        assert_eq!(first.harness, alternate);
        assert_eq!(first.schema_version, SCHEMA_VERSION);
        assert_eq!(first.name, "chat");

        // A later run refreshes the token but keeps the original `created`.
        write(&path, project, &alternate, "chat", "sess-2", Some(&first)).unwrap();
        let second = read(&path).unwrap();
        assert_eq!(second.token, "sess-2");
        assert_eq!(second.created, first.created);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_ignores_a_corrupt_record() {
        let dir = temp_dir("corrupt");
        let path = dir.join("bad.json");
        fs::write(&path, b"{ not json").unwrap();
        assert!(read(&path).is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_ignores_a_record_bound_to_an_unrunnable_identity() {
        // Well-formed JSON whose `harness` is not a well-formed identity — a
        // harness dropped from the registry, or a hand-edited store. There is
        // nothing to resume it against, so it reads as absent and the run starts
        // fresh rather than carrying an id no candidate can ever match.
        let dir = temp_dir("unrunnable");
        for harness in ["gone-harness", "claude-code:", "claude-code:a:b", ""] {
            let path = dir.join("x.json");
            let record = serde_json::json!({
                "schema_version": SCHEMA_VERSION,
                "name": "chat",
                "project": "/home/me/proj",
                "harness": harness,
                "token": "sess-1",
                "created": "2026-01-01T00:00:00Z",
                "updated": "2026-01-01T00:00:00Z",
            });
            fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
            assert!(read(&path).is_none(), "`{harness}` must not read back");
        }
        fs::remove_dir_all(&dir).ok();
    }
}
