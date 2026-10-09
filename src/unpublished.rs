//! `onepipeline unpublished` — whether a manager session still owes a preserved
//! branch, decided in process over the linked `onevcs`.
//!
//! What the verb promises — its targets, its JSON document, the acknowledgement
//! file and the exit statuses — is entry 113 of `docs/contract-divergences.md`,
//! and is not restated here. What belongs beside the code is the one rule every
//! ending is built on: **`unanswered` is never `none`.** A read that failed, a row
//! that could not be read whole, and a label filter that did not filter each leave
//! the verdict `unanswered`, because "nothing is owed" is a claim and none of those
//! established it.
//!
//! The rows are `onevcs`'s own: [`onevcs::Vcs::recoverable_matching`] at
//! [`onevcs::Detail::Decision`], called rather than spawned, so the only processes
//! a read starts are the `git` that library runs itself. Nothing here re-derives a
//! landing, a hold or a retirement; what this module adds is the counting rule, the
//! acknowledgement, the landing command a person pastes, and the disk reading.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use onevcs::{Detail, Landed, Recoverable, Scope, Selection, SessionToken};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::executor::{SESSION_LAUNCHER_LABEL, SESSION_NODE_LABEL, SESSION_RUN_LABEL};

/// Nothing is counted for the target.
pub const EXIT_NOTHING_COUNTED: i32 = 0;

/// The read could not answer; [`Unpublished::unresolved`] says why.
pub const EXIT_UNANSWERED: i32 = 1;

/// At least one preserved branch is counted: not in flight, not landed, and not
/// acknowledged at its tip.
///
/// `unpublished`'s own answer, and a code above `3` belongs to one verb's
/// protocol, as entries 58 and 68 rule: `watch` spells `7` for a run that moved,
/// and no caller of one verb meets the other's. Entry 113 records the sharing.
// llmlint: ignore[cli_output_contract] `7` is shared with `watch`'s run-changed return
// in the global table, and the per-verb reading entries 58 and 68 already rule on is
// what makes that one code space per verb; entry 113 records it as a proposal.
pub const EXIT_COUNTED: i32 = 7;

/// The acknowledgement file's version: ai-orchestrator's version-1 format,
/// adopted verbatim so files that consumer already wrote keep working.
pub const ACKNOWLEDGEMENT_FILE_VERSION: u32 = 1;

/// Where the acknowledgements live under the state root.
pub const ACKNOWLEDGEMENTS_UNDER: &str = "onepipeline/unpublished/acknowledged";

/// The build-output directories the disk reading names under a worktree.
pub const BUILD_OUTPUT_DIRECTORIES: [&str; 5] = ["target", "node_modules", ".venv", ".nx", "dist"];

/// The `onevcs` verbs whose landing a drafter is put in front of, and the
/// `onepipeline` verb that does it.
const DRAFTED_VERBS: [(&str, &str); 2] = [
    ("publish-branch", "publish-branch"),
    ("recover", "repo-recover"),
];

/// The `at` stamp's format, `%Y-%m-%dT%H:%M:%SZ`, as a length.
const STAMP_LEN: usize = "2026-01-01T00:00:00Z".len();

/// What one listing is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Target {
    /// Every session a run this manager session launched opened: the rows whose
    /// session carries `launcher=<session>`.
    Session {
        /// The manager session.
        session: String,
    },
    /// Every registered identity's preserved branches.
    Host,
    /// The branches the named `onevcs` sessions hold or held.
    Tokens {
        /// The session tokens, as given.
        tokens: Vec<String>,
    },
}

/// The one answer a listing gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    /// Nothing is counted.
    None,
    /// At least one row is counted.
    Owed,
    /// The read failed, a row could not be read whole, or the filter did not
    /// filter. Never `none`.
    Unanswered,
}

/// One acknowledgement: a branch of an identity, deliberately left at a tip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcknowledgedBranch {
    /// The branch.
    pub branch: String,
    /// The identity it belongs to.
    pub identity: String,
    /// The full commit object name the branch stood at, 40–64 lowercase hex.
    pub tip: String,
    /// Why it is left: one line of text with something visible in it.
    pub reason: String,
    /// When, UTC, `%Y-%m-%dT%H:%M:%SZ`.
    pub at: String,
}

/// The acknowledgement file, as written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcknowledgementFile {
    /// Always [`ACKNOWLEDGEMENT_FILE_VERSION`].
    pub version: u32,
    /// The entries, one per identity and branch.
    pub acknowledged: Vec<AcknowledgedBranch>,
}

/// A row's disk reading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Disk {
    /// The run root: the parent of a worktree at `<…>/<s-token>/worktree`.
    pub run_root: Option<PathBuf>,
    /// Its allocated bytes, never following a symbolic link; `null` where it
    /// could not be measured.
    pub run_root_bytes: Option<u64>,
    /// Each build-output directory under the worktree that exists, by name.
    pub build_output: BTreeMap<String, u64>,
}

/// One preserved branch, as the listing states it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Row {
    /// The identity it belongs to.
    pub identity: String,
    /// The branch.
    pub branch: String,
    /// The base it was cut from.
    pub base: String,
    /// Why `onevcs` preserved it.
    pub provenance: onevcs::Provenance,
    /// Its full commit object name, or `null` where `onevcs` could not read it.
    pub tip: Option<String>,
    /// `onevcs`'s landing answer, whole.
    pub landed: Landed,
    /// The change request it belongs to.
    pub change_url: Option<String>,
    /// Why the workstream stopped.
    pub stopped_because: String,
    /// The session that answers for it.
    pub session: Option<String>,
    /// The session's `run` label.
    pub run: Option<String>,
    /// The session's `node` label.
    pub node: Option<String>,
    /// The session's `launcher` label.
    pub manager_session: Option<String>,
    /// `onevcs`'s argv that lands it, whole.
    pub recover_command: Vec<String>,
    /// The argv this listing prints to land it: `[]` where `recover_command` is.
    pub land_command: Vec<String>,
    /// `onevcs`'s retirement classification, whole.
    pub retirement: Option<onevcs::Retirement>,
    /// Whether something still holds it.
    pub in_flight: bool,
    /// Whether it is counted.
    pub counted: bool,
    /// The acknowledgement standing for it at its tip.
    pub acknowledgement: Option<AcknowledgedBranch>,
    /// The disk reading, with `--disk`.
    pub disk: Option<Disk>,
}

/// One listing: the JSON document `--format json` writes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Unpublished {
    /// What was asked about.
    pub target: Target,
    /// The answer.
    pub verdict: Verdict,
    /// Every row read.
    pub rows: Vec<Row>,
    /// Everything that could not be resolved, one line each.
    pub unresolved: Vec<String>,
}

impl Unpublished {
    /// The exit status the binary answers with.
    pub const fn exit_code(&self) -> i32 {
        match self.verdict {
            Verdict::None => EXIT_NOTHING_COUNTED,
            Verdict::Owed => EXIT_COUNTED,
            Verdict::Unanswered => EXIT_UNANSWERED,
        }
    }

    /// The JSON document, one line.
    pub fn json(&self) -> String {
        // llmlint: ignore[no_panics_on_recoverable_errors] every field is a string, a
        // number, a bool or a value onevcs itself serialized, so this cannot fail.
        serde_json::to_string(self).expect("the listing serializes")
    }
}

/// What one listing is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnpublishedRequest {
    /// What to list.
    pub target: Target,
    /// Whose acknowledgements apply: the session target's own session, and for
    /// the other two the caller's, where one is named.
    pub acknowledging: Option<String>,
    /// The acknowledgements directory.
    pub acknowledgements: PathBuf,
    /// Take each row's disk reading.
    pub disk: bool,
    /// The drafting graph a printed landing command names, made absolute and
    /// checked to be a readable file by [`pr_author_graph`].
    pub pr_author_graph: Option<PathBuf>,
}

/// What one acknowledgement is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcknowledgeRequest {
    /// The branch.
    pub branch: String,
    /// Why it is left.
    pub reason: String,
    /// The identity it belongs to, where several name the branch.
    pub repo: Option<String>,
    /// The manager session it is recorded for.
    pub session: String,
    /// The acknowledgements directory.
    pub acknowledgements: PathBuf,
}

/// An acknowledgement recorded, and the session listing read after it.
#[derive(Debug, Clone, PartialEq)]
pub struct Acknowledged {
    /// What was recorded.
    pub entry: AcknowledgedBranch,
    /// The file it was recorded in.
    pub file: PathBuf,
    /// The session target's listing, after the write.
    pub listing: Unpublished,
}

impl Acknowledged {
    /// The receipt line.
    pub fn line(&self) -> String {
        format!(
            "acknowledged {} [{}] at {}: {} ({})\n",
            self.entry.branch,
            self.entry.identity,
            &self.entry.tip[..12.min(self.entry.tip.len())],
            self.entry.reason,
            self.file.display()
        )
    }
}

/// The default acknowledgements directory: `$XDG_STATE_HOME` when absolute, else
/// `~/.local/state`, joined with [`ACKNOWLEDGEMENTS_UNDER`].
///
/// # Errors
///
/// Neither an absolute `XDG_STATE_HOME` nor an absolute home directory.
pub fn default_acknowledgements() -> Result<PathBuf> {
    crate::stopguard::state_root()
        .map(|root| root.join(ACKNOWLEDGEMENTS_UNDER))
        .map_err(Error::Invalid)
}

/// The file `session`'s acknowledgements are kept in under `directory`.
pub fn acknowledgement_file(directory: &Path, session: &str) -> PathBuf {
    directory.join(format!("{}.json", hex(&Sha256::digest(session.as_bytes()))))
}

/// A drafting graph a printed landing command can name: absolute, and a readable
/// file.
///
/// # Errors
///
/// A path that is not a readable file, refused before anything is read.
pub fn pr_author_graph(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path).map_err(|error| {
        Error::Invalid(format!(
            "the drafting graph {} cannot be made absolute ({error})",
            path.display()
        ))
    })?;
    let readable = std::fs::metadata(&absolute)
        .ok()
        .filter(std::fs::Metadata::is_file)
        .and_then(|_| std::fs::File::open(&absolute).ok());
    if readable.is_none() {
        return Err(Error::Invalid(format!(
            "the drafting graph {} is not a readable file, so no landing command could name \
             it; name the pr-author graph a landing drafts with",
            absolute.display()
        )));
    }
    Ok(absolute)
}

/// A session that names somebody: not blank and free of NUL.
fn named_session(session: &str) -> Result<()> {
    if session.trim().is_empty() || session.contains('\0') {
        return Err(Error::Invalid(
            "no manager session to answer for: name one with `--session`, or export \
             ONEPIPELINE_LAUNCHER_SESSION in the environment this runs in"
                .to_owned(),
        ));
    }
    Ok(())
}

/// List the target's preserved branches and decide whether any is owed.
///
/// # Errors
///
/// A refusal — a blank session, an unknown session token — made before
/// anything is read. A read that fails is not an error: it is
/// [`Verdict::Unanswered`].
pub fn unpublished(request: &UnpublishedRequest) -> Result<Unpublished> {
    let selection = match &request.target {
        Target::Session { session } => {
            named_session(session)?;
            Selection {
                detail: Detail::Decision,
                sessions: Vec::new(),
                labels: BTreeMap::from([(SESSION_LAUNCHER_LABEL.to_owned(), session.clone())]),
            }
        }
        Target::Host => Selection {
            detail: Detail::Decision,
            ..Selection::default()
        },
        Target::Tokens { tokens } => {
            if tokens.is_empty() {
                return Err(Error::Invalid("`--token` names no session".to_owned()));
            }
            let providers = onevcs::Providers::real();
            for token in tokens {
                // Refused as onevcs refuses it: a token no record names is a
                // different answer from a session that left nothing.
                providers
                    .vcs
                    .session(&SessionToken(token.clone()))
                    .map_err(|error| Error::Invalid(format!("{token}: {error}")))?;
            }
            Selection {
                detail: Detail::Decision,
                sessions: tokens.iter().cloned().map(SessionToken).collect(),
                labels: BTreeMap::new(),
            }
        }
    };
    Ok(listing(request, &selection))
}

/// The listing for one selection, read and decided.
fn listing(request: &UnpublishedRequest, selection: &Selection) -> Unpublished {
    let mut unresolved = Vec::new();
    let read = onevcs::Providers::real()
        .vcs
        .recoverable_matching(Scope::All, selection);
    let recoverable = match read {
        Ok(rows) => rows,
        Err(error) => {
            unresolved.push(format!(
                "the recovery read failed, so whether anything is owed is unanswered: {error}"
            ));
            return Unpublished {
                target: request.target.clone(),
                verdict: Verdict::Unanswered,
                rows: Vec::new(),
                unresolved,
            };
        }
    };
    let mut unanswered = false;
    if let Target::Session { session } = &request.target {
        for row in &recoverable {
            let label = row.labels.get(SESSION_LAUNCHER_LABEL);
            if label != Some(session) {
                unanswered = true;
                unresolved.push(format!(
                    "the read filtered on {SESSION_LAUNCHER_LABEL}={session} and answered {} \
                     [{}], whose session is labelled {}, so the filter did not filter and \
                     nothing it answered can be counted for this session",
                    row.branch.branch,
                    row.identity,
                    label.map_or_else(|| "nothing".to_owned(), |label| format!("{label:?}")),
                ));
            }
        }
    }
    for row in &recoverable {
        if row.identity.is_empty() || row.branch.branch.is_empty() {
            unanswered = true;
            unresolved.push(format!(
                "a row names no identity or no branch and could not be read whole: {}",
                serde_json::to_string(row).unwrap_or_default()
            ));
        }
    }
    let acknowledged = match &request.acknowledging {
        Some(session) => read_acknowledgements(
            &acknowledgement_file(&request.acknowledgements, session),
            &mut unresolved,
        ),
        None => Vec::new(),
    };
    let rows: Vec<Row> = recoverable
        .into_iter()
        .map(|row| {
            let standing = standing(&row, &acknowledged, &mut unresolved);
            let disk = request.disk.then(|| disk(&row, &mut unresolved));
            shaped(row, standing, disk, request.pr_author_graph.as_deref())
        })
        .collect();
    let verdict = if unanswered {
        Verdict::Unanswered
    } else if rows.iter().any(|row| row.counted) {
        Verdict::Owed
    } else {
        Verdict::None
    };
    Unpublished {
        target: request.target.clone(),
        verdict,
        rows,
        unresolved,
    }
}

/// The acknowledgement standing for `row`, if one does: its identity and branch,
/// and its tip equal to the row's.
fn standing(
    row: &Recoverable,
    acknowledged: &[AcknowledgedBranch],
    unresolved: &mut Vec<String>,
) -> Option<AcknowledgedBranch> {
    let entry = acknowledged
        .iter()
        .find(|entry| entry.identity == row.identity && entry.branch == row.branch.branch)?;
    match &row.tip {
        Some(tip) if *tip == entry.tip => Some(entry.clone()),
        Some(_) => None,
        None => {
            unresolved.push(format!(
                "{} [{}]: onevcs could not read its tip, so its acknowledgement does not stand \
                 and it counts",
                row.branch.branch, row.identity
            ));
            None
        }
    }
}

/// Whether a landing answer is one the counting rule counts: `no`, `unknown`
/// and `in-part`.
fn counts(landed: &Landed) -> bool {
    matches!(landed, Landed::No | Landed::Unknown | Landed::InPart { .. })
}

/// One `onevcs` row as this listing states it.
fn shaped(
    row: Recoverable,
    acknowledgement: Option<AcknowledgedBranch>,
    disk: Option<Disk>,
    graph: Option<&Path>,
) -> Row {
    let in_flight = row.held_by.is_some();
    let counted = !in_flight && counts(&row.landed) && acknowledgement.is_none();
    let label = |key: &str| row.labels.get(key).cloned();
    let session = row
        .session
        .as_ref()
        .map(|token| token.0.clone())
        .or_else(|| {
            row.held_by
                .as_ref()
                .and_then(|held| held.token.as_ref().map(|token| token.0.clone()))
        });
    Row {
        land_command: land_command(&row.recover_command, graph),
        run: label(SESSION_RUN_LABEL),
        node: label(SESSION_NODE_LABEL),
        manager_session: label(SESSION_LAUNCHER_LABEL),
        identity: row.identity,
        branch: row.branch.branch,
        base: row.branch.base,
        provenance: row.branch.provenance,
        tip: row.tip,
        landed: row.landed,
        change_url: row.branch.change_url.map(|url| url.to_string()),
        stopped_because: row.stopped_because,
        session,
        recover_command: row.recover_command,
        retirement: row.retirement,
        in_flight,
        counted,
        acknowledgement,
        disk,
    }
}

/// The argv this listing prints to land a row: `publish-branch` and `recover`
/// through this host's drafter, every other verb as `onevcs` gave it.
pub(crate) fn land_command(recover: &[String], graph: Option<&Path>) -> Vec<String> {
    let [program, verb, rest @ ..] = recover else {
        return recover.to_vec();
    };
    let Some((_, ours)) = DRAFTED_VERBS
        .iter()
        .find(|(theirs, _)| program == "onevcs" && verb == theirs)
    else {
        return recover.to_vec();
    };
    let mut argv = vec!["onepipeline".to_owned(), (*ours).to_owned()];
    argv.extend(rest.iter().cloned());
    match graph {
        Some(graph) => {
            argv.push(crate::land::PR_AUTHOR_GRAPH_FLAG.to_owned());
            argv.push(graph.display().to_string());
        }
        None => argv.push(crate::land::NO_DRAFT_FLAG.to_owned()),
    }
    argv
}

/// The run root a row's work is pinned in, and the worktree under it.
fn run_root(row: &Recoverable, unresolved: &mut Vec<String>) -> Option<PathBuf> {
    let worktree = match (&row.held_by, &row.session) {
        (Some(held), _) => Some(held.worktree.clone()),
        (None, Some(token)) => match onevcs::Providers::real().vcs.session(token) {
            Ok(record) => Some(record.session.worktree),
            Err(error) => {
                unresolved.push(format!(
                    "{} [{}]: its session {} could not be read, so its disk was not measured: \
                     {error}",
                    row.branch.branch, row.identity, token.0
                ));
                None
            }
        },
        (None, None) => None,
    }?;
    let parent = worktree.parent()?;
    let shaped = worktree.file_name().is_some_and(|name| name == "worktree")
        && parent
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("s-"));
    if !shaped {
        unresolved.push(format!(
            "{} [{}]: its worktree {} is not at `<…>/<s-token>/worktree`, so it has no run root \
             to measure",
            row.branch.branch,
            row.identity,
            worktree.display()
        ));
        return None;
    }
    Some(parent.to_path_buf())
}

/// A row's disk reading.
fn disk(row: &Recoverable, unresolved: &mut Vec<String>) -> Disk {
    let Some(root) = run_root(row, unresolved) else {
        return Disk {
            run_root: None,
            run_root_bytes: None,
            build_output: BTreeMap::new(),
        };
    };
    let directory = std::fs::symlink_metadata(&root)
        .ok()
        .filter(|meta| meta.is_dir() && !meta.file_type().is_symlink());
    if directory.is_none() {
        unresolved.push(format!(
            "{} [{}]: its run root {} is not a directory, so its disk was not measured",
            row.branch.branch,
            row.identity,
            root.display()
        ));
        return Disk {
            run_root: Some(root),
            run_root_bytes: None,
            build_output: BTreeMap::new(),
        };
    }
    let worktree = root.join("worktree");
    let mut build_output = BTreeMap::new();
    for name in BUILD_OUTPUT_DIRECTORIES {
        let candidate = worktree.join(name);
        let real = std::fs::symlink_metadata(&candidate)
            .ok()
            .is_some_and(|meta| meta.is_dir() && !meta.file_type().is_symlink());
        if real {
            build_output.insert(name.to_owned(), allocated(&candidate, unresolved));
        }
    }
    Disk {
        run_root_bytes: Some(allocated(&root, unresolved)),
        run_root: Some(root),
        build_output,
    }
}

/// The allocated bytes of every regular file under `root`, never following a
/// symbolic link.
fn allocated(root: &Path, unresolved: &mut Vec<String>) -> u64 {
    let mut total = 0_u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                unresolved.push(format!(
                    "{}: could not be read while measuring disk: {error}",
                    directory.display()
                ));
                continue;
            }
        };
        for entry in entries.flatten() {
            let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                pending.push(entry.path());
            } else if meta.is_file() {
                total = total.saturating_add(allocated_bytes(&meta));
            }
        }
    }
    total
}

#[cfg(unix)]
fn allocated_bytes(meta: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.blocks().saturating_mul(512)
}

// llmlint: ignore[changed_behavior_has_e2e] Windows reports no allocated block count through
// std, so the apparent length stands in; the Unix arm is what the `--disk` journey drives.
#[cfg(not(unix))]
fn allocated_bytes(meta: &std::fs::Metadata) -> u64 {
    meta.len()
}

/// What is wrong with `reason` as a recorded reason, or nothing.
///
/// One line of text a person reads: something visible in it, and no character
/// that is not printable — a newline among them.
pub(crate) fn reason_fault(reason: &str) -> Option<String> {
    let trimmed = reason.trim();
    if !trimmed.chars().any(|c| printable(c) && c != ' ') {
        return Some(format!(
            "the reason {reason:?} carries no visible character, so it says nothing"
        ));
    }
    let unprintable: BTreeSet<char> = trimmed.chars().filter(|&c| !printable(c)).collect();
    if !unprintable.is_empty() {
        return Some(format!(
            "the reason {reason:?} carries the unprintable character(s) {unprintable:?}, and a \
             reason is one line of text"
        ));
    }
    None
}

/// Printable as one line of text: a space, or a character that is neither a
/// control, other whitespace, nor an invisible format character.
fn printable(c: char) -> bool {
    c == ' '
        || !(c.is_control()
            || c.is_whitespace()
            || matches!(c, '\u{ad}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{2064}' | '\u{feff}'))
}

/// Whether `tip` is a full commit object name: 40 to 64 lowercase hex.
fn is_tip(tip: &str) -> bool {
    (40..=64).contains(&tip.len())
        && tip
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Whether `at` is a UTC stamp in `%Y-%m-%dT%H:%M:%SZ` naming a real instant.
fn is_stamp(at: &str) -> bool {
    at.len() == STAMP_LEN
        && at.ends_with('Z')
        && crate::watchers::instant_millis(at)
            .and_then(|millis| u64::try_from(millis).ok())
            .is_some_and(|millis| stamp(millis) == at)
}

/// Epoch milliseconds as `%Y-%m-%dT%H:%M:%SZ`.
fn stamp(millis: u64) -> String {
    let rendered = crate::sys::rfc3339_from_millis(millis - millis % 1_000);
    rendered.replace(".000Z", "Z")
}

/// Why one recorded entry may not suppress a count, or nothing.
fn entry_fault(entry: &AcknowledgedBranch) -> Option<String> {
    if entry.branch.is_empty() || entry.identity.is_empty() {
        return Some("it names no branch or no identity".to_owned());
    }
    if !is_tip(&entry.tip) {
        return Some(format!(
            "its tip {:?} is not 40-64 lowercase hex",
            entry.tip
        ));
    }
    if !is_stamp(&entry.at) {
        return Some(format!(
            "its at {:?} is not a UTC %Y-%m-%dT%H:%M:%SZ stamp",
            entry.at
        ));
    }
    reason_fault(&entry.reason)
}

/// Why a file's contents could not be read as a whole version-1 file.
enum FileFault {
    /// Absent: no acknowledgement, and nothing to say.
    Absent,
    /// Present and unreadable as version 1.
    Unreadable(String),
}

/// The file at `path`, read whole as version 1.
fn read_file(path: &Path) -> std::result::Result<Vec<serde_json::Value>, FileFault> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(FileFault::Absent)
        }
        Err(error) => return Err(FileFault::Unreadable(error.to_string())),
    };
    let document: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| FileFault::Unreadable(format!("not JSON ({error})")))?;
    if document.get("version").and_then(serde_json::Value::as_u64)
        != Some(u64::from(ACKNOWLEDGEMENT_FILE_VERSION))
    {
        return Err(FileFault::Unreadable(format!(
            "not a version {ACKNOWLEDGEMENT_FILE_VERSION} acknowledgement file"
        )));
    }
    let object = document.as_object().map(|object| object.len());
    let entries = document
        .get("acknowledged")
        .and_then(serde_json::Value::as_array)
        .filter(|_| object == Some(2))
        .ok_or_else(|| {
            FileFault::Unreadable(
                "it carries no `acknowledged` array, or a field the format does not name"
                    .to_owned(),
            )
        })?;
    Ok(entries.clone())
}

/// Every acknowledgement in the file that may suppress a count, saying each one
/// left out.
fn read_acknowledgements(path: &Path, unresolved: &mut Vec<String>) -> Vec<AcknowledgedBranch> {
    let entries = match read_file(path) {
        Ok(entries) => entries,
        Err(FileFault::Absent) => return Vec::new(),
        Err(FileFault::Unreadable(why)) => {
            unresolved.push(format!(
                "{}: the acknowledgement file applies no acknowledgement, so every branch \
                 counts: {why}",
                path.display()
            ));
            return Vec::new();
        }
    };
    let mut kept: Vec<AcknowledgedBranch> = Vec::new();
    for value in entries {
        let entry: AcknowledgedBranch = match serde_json::from_value(value.clone()) {
            Ok(entry) => entry,
            Err(error) => {
                unresolved.push(format!(
                    "{}: an entry is not an acknowledgement and was left out ({error}): {value}",
                    path.display()
                ));
                continue;
            }
        };
        if let Some(why) = entry_fault(&entry) {
            unresolved.push(format!(
                "{}: the entry for {} [{}] was left out: {why}",
                path.display(),
                entry.branch,
                entry.identity
            ));
            continue;
        }
        kept.push(entry);
    }
    let mut seen: BTreeMap<(String, String), usize> = BTreeMap::new();
    for entry in &kept {
        *seen
            .entry((entry.identity.clone(), entry.branch.clone()))
            .or_default() += 1;
    }
    for ((identity, branch), times) in &seen {
        if *times > 1 {
            unresolved.push(format!(
                "{}: {branch} [{identity}] is acknowledged {times} times, so none of them \
                 applies",
                path.display()
            ));
        }
    }
    kept.retain(|entry| seen[&(entry.identity.clone(), entry.branch.clone())] == 1);
    kept
}

/// Record that `request.session` has seen and deliberately left a branch, at the
/// tip it stands at now, and answer with the session listing read after it.
///
/// # Errors
///
/// Refused before anything is written: a blank or multi-line reason, a session
/// nothing identifies, a branch no host row names or several identities name
/// without `--repo`, a row whose tip onevcs could not read, an existing file this
/// build cannot read whole, and a host read that failed.
pub fn acknowledge(request: &AcknowledgeRequest, graph: Option<PathBuf>) -> Result<Acknowledged> {
    if let Some(fault) = reason_fault(&request.reason) {
        return Err(Error::Invalid(format!(
            "{fault}; an acknowledgement needs `--reason \"<why this branch is deliberately left>\"`"
        )));
    }
    named_session(&request.session)?;
    let providers = onevcs::Providers::real();
    let rows = providers
        .vcs
        .recoverable_matching(
            Scope::All,
            &Selection {
                detail: Detail::Decision,
                ..Selection::default()
            },
        )
        .map_err(|error| {
            Error::Invalid(format!(
                "the host's preserved branches could not be read, so nothing was acknowledged: \
                 {error}"
            ))
        })?;
    let identity = match &request.repo {
        Some(repo) if rows.iter().any(|row| row.identity == *repo) => Some(repo.clone()),
        Some(repo) => Some(
            providers
                .vcs
                .resolve_identity(repo)
                .map_err(|error| Error::Invalid(format!("--repo {repo}: {error}")))?
                .origin,
        ),
        None => None,
    };
    let named: Vec<&Recoverable> = rows
        .iter()
        .filter(|row| row.branch.branch == request.branch)
        .filter(|row| identity.as_ref().is_none_or(|key| row.identity == *key))
        .collect();
    let row = match named.as_slice() {
        [] => {
            return Err(Error::Invalid(format!(
                "branch {} is on no row of `onepipeline unpublished --host`{}, so there is \
                 nothing to acknowledge",
                request.branch,
                identity
                    .as_ref()
                    .map(|key| format!(" for {key}"))
                    .unwrap_or_default()
            )))
        }
        [row] => *row,
        several => {
            let identities: BTreeSet<&str> =
                several.iter().map(|row| row.identity.as_str()).collect();
            return Err(Error::Invalid(format!(
                "branch {} is preserved on several identities ({}); name one with `--repo`",
                request.branch,
                identities.into_iter().collect::<Vec<_>>().join(", ")
            )));
        }
    };
    let tip = row.tip.clone().filter(|tip| is_tip(tip)).ok_or_else(|| {
        Error::Invalid(format!(
            "onevcs could not read the tip of {} [{}], and an acknowledgement is keyed on it",
            row.branch.branch, row.identity
        ))
    })?;
    let file = acknowledgement_file(&request.acknowledgements, &request.session);
    let mut kept = match read_file(&file) {
        Ok(entries) => {
            let mut read = Vec::new();
            for value in entries {
                let entry: AcknowledgedBranch = serde_json::from_value(value).map_err(|error| {
                    Error::Invalid(format!(
                        "{}: an entry cannot be read ({error}); repair the file before \
                         recording another acknowledgement",
                        file.display()
                    ))
                })?;
                read.push(entry);
            }
            read
        }
        Err(FileFault::Absent) => Vec::new(),
        Err(FileFault::Unreadable(why)) => {
            return Err(Error::Invalid(format!(
                "{}: {why}; repair or remove it before recording another acknowledgement",
                file.display()
            )))
        }
    };
    let entry = AcknowledgedBranch {
        branch: row.branch.branch.clone(),
        identity: row.identity.clone(),
        tip,
        reason: request.reason.trim().to_owned(),
        at: stamp(crate::sys::now_millis()),
    };
    kept.retain(|other| !(other.identity == entry.identity && other.branch == entry.branch));
    kept.push(entry.clone());
    write_file(
        &file,
        &AcknowledgementFile {
            version: ACKNOWLEDGEMENT_FILE_VERSION,
            acknowledged: kept,
        },
    )?;
    let listing = unpublished(&UnpublishedRequest {
        target: Target::Session {
            session: request.session.clone(),
        },
        acknowledging: Some(request.session.clone()),
        acknowledgements: request.acknowledgements.clone(),
        disk: false,
        pr_author_graph: graph,
    })?;
    Ok(Acknowledged {
        entry,
        file,
        listing,
    })
}

/// Replace the file whole: a temporary file beside it, then a rename.
fn write_file(path: &Path, document: &AcknowledgementFile) -> Result<()> {
    let failed = |error: std::io::Error| {
        Error::Invalid(format!(
            "the acknowledgement could not be written to {}: {error}",
            path.display()
        ))
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(failed)?;
    }
    let pending = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let mut text = serde_json::to_string_pretty(document)
        .map_err(|error| Error::Invalid(error.to_string()))?;
    text.push('\n');
    if let Err(error) =
        std::fs::write(&pending, text).and_then(|()| std::fs::rename(&pending, path))
    {
        let _ = std::fs::remove_file(&pending);
        return Err(failed(error));
    }
    Ok(())
}

/// One word a person pastes into a POSIX shell: bare where nothing in it is
/// special, single-quoted otherwise.
fn shell_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./:=@+,%".contains(&b));
    if plain {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// An argv as a person pastes it.
pub fn shell_line(argv: &[String]) -> String {
    argv.iter()
        .map(|word| shell_word(word))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The state word a row's `landed` carries.
fn landed_word(landed: &Landed) -> &'static str {
    match landed {
        Landed::Yes { .. } => "yes",
        Landed::InPart { .. } => "in-part",
        Landed::No => "no",
        Landed::Unknown => "unknown",
    }
}

/// The text listing `--format text` writes on standard output.
pub fn render(listing: &Unpublished) -> String {
    let mut out = String::new();
    let drafted = listing
        .rows
        .iter()
        .flat_map(|row| row.land_command.iter())
        .any(|word| word == crate::land::PR_AUTHOR_GRAPH_FLAG);
    for row in &listing.rows {
        let standing = if row.in_flight {
            "in flight".to_owned()
        } else if let Some(entry) = &row.acknowledgement {
            format!("acknowledged: {}", entry.reason)
        } else if row.counted {
            "counted".to_owned()
        } else {
            format!("not counted (landed {})", landed_word(&row.landed))
        };
        out.push_str(&format!(
            "{}  [{}]  landed {}  session {}  {standing}\n",
            row.branch,
            row.identity,
            landed_word(&row.landed),
            row.session.as_deref().unwrap_or("-"),
        ));
        if let Some(disk) = &row.disk {
            let bytes = disk
                .run_root_bytes
                .map_or_else(|| "-".to_owned(), |bytes| bytes.to_string());
            let build: Vec<String> = disk
                .build_output
                .iter()
                .map(|(name, bytes)| format!("{name}={bytes}"))
                .collect();
            out.push_str(&format!(
                "    disk: run root {} {bytes} bytes; build output {}\n",
                disk.run_root
                    .as_ref()
                    .map_or_else(|| "-".to_owned(), |root| root.display().to_string()),
                if build.is_empty() {
                    "-".to_owned()
                } else {
                    build.join(" ")
                }
            ));
        }
    }
    if !listing.rows.is_empty() {
        out.push('\n');
    }
    let acknowledger = match &listing.target {
        Target::Session { session } => Some(session.as_str()),
        _ => None,
    };
    for row in listing.rows.iter().filter(|row| row.counted) {
        out.push_str(&format!("{} [{}]", row.branch, row.identity));
        if !row.stopped_because.is_empty() {
            out.push_str(&format!(" — {}", row.stopped_because));
        }
        out.push('\n');
        let superseded = row.retirement.as_ref().filter(|retirement| {
            retirement.class == onevcs::RetirementClass::SupersededWithChanges
        });
        if let Some(retirement) = superseded {
            if let Some(by) = &retirement.superseded_by {
                let node = by
                    .labels
                    .get(SESSION_NODE_LABEL)
                    .map(|node| format!(" (node {node})"))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "    superseded by:   {}{node}, landed at {}\n",
                    by.branch, by.landing
                ));
            }
            out.push_str(&format!(
                "    differs in:      {}\n",
                retirement.differing_paths.join(", ")
            ));
            out.push_str(&format!(
                "    reclaim it:      {}\n",
                shell_line(&[
                    "onevcs".to_owned(),
                    "reclaim".to_owned(),
                    row.branch.clone(),
                    "--repo".to_owned(),
                    row.identity.clone(),
                ])
            ));
        }
        if !row.land_command.is_empty() {
            out.push_str(&format!(
                "    land it:         {}\n",
                shell_line(&row.land_command)
            ));
        }
        let mut acknowledge = vec![
            "onepipeline".to_owned(),
            "unpublished".to_owned(),
            "--acknowledge".to_owned(),
            row.branch.clone(),
            "--repo".to_owned(),
            row.identity.clone(),
        ];
        if let Some(session) = acknowledger {
            acknowledge.extend(["--session".to_owned(), session.to_owned()]);
        }
        out.push_str(&format!(
            "    or acknowledge:  {} --reason \"<why it is deliberately left>\"\n",
            shell_line(&acknowledge)
        ));
    }
    let counted = listing.rows.iter().filter(|row| row.counted).count();
    let in_flight = listing.rows.iter().filter(|row| row.in_flight).count();
    let acknowledged = listing
        .rows
        .iter()
        .filter(|row| row.acknowledgement.is_some())
        .count();
    let landing = listing.rows.iter().any(|row| {
        row.land_command
            .iter()
            .any(|word| word == crate::land::NO_DRAFT_FLAG)
    });
    let drafter = if drafted || !landing {
        String::new()
    } else {
        "; no drafter is configured, so each landing command carries --no-draft (pass \
         --pr-author-graph to draft its change request's body)"
            .to_owned()
    };
    let verdict = match listing.verdict {
        Verdict::None => "none",
        Verdict::Owed => "owed",
        Verdict::Unanswered => "unanswered",
    };
    out.push_str(&format!(
        "{verdict}: {counted} counted of {} preserved unpublished branch(es); {in_flight} in \
         flight, {acknowledged} acknowledged{drafter}\n",
        listing.rows.len()
    ));
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_landing_command_runs_this_hosts_drafter_or_says_it_drafts_nothing() {
        let argv = |words: &[&str]| words.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>();
        let graph = Path::new("/graphs/pr-author.yaml");
        assert_eq!(
            land_command(
                &argv(&["onevcs", "publish-branch", "b", "--repo", "/r"]),
                Some(graph)
            ),
            argv(&[
                "onepipeline",
                "publish-branch",
                "b",
                "--repo",
                "/r",
                "--pr-author-graph",
                "/graphs/pr-author.yaml"
            ])
        );
        assert_eq!(
            land_command(&argv(&["onevcs", "recover", "b", "--repo", "/r"]), None),
            argv(&[
                "onepipeline",
                "repo-recover",
                "b",
                "--repo",
                "/r",
                "--no-draft"
            ])
        );
        for kept in [
            argv(&["onevcs", "integrate", "b"]),
            argv(&["onevcs", "reclaim", "b"]),
            Vec::new(),
        ] {
            assert_eq!(land_command(&kept, Some(graph)), kept);
        }
    }

    #[test]
    fn a_reason_is_one_visible_line() {
        assert!(reason_fault("kept for the spike").is_none());
        for refused in ["", "   ", "two\nlines", "tab\there", "\u{200b}"] {
            assert!(reason_fault(refused).is_some(), "{refused:?}");
        }
    }

    #[test]
    fn a_stamp_is_a_real_utc_second() {
        assert!(is_stamp("2026-10-07T12:34:56Z"));
        for refused in [
            "2026-10-07T12:34:56.000Z",
            "2026-13-07T12:34:56Z",
            "2026-10-07 12:34:56Z",
            "2026-10-07T12:34:56+00:00",
        ] {
            assert!(!is_stamp(refused), "{refused}");
        }
        assert!(is_stamp(&stamp(crate::sys::now_millis())));
    }

    #[test]
    fn a_word_is_quoted_only_where_a_shell_would_read_it_otherwise() {
        assert_eq!(
            shell_line(&["a".into(), "b c".into(), "it's".into(), "/x/y.yaml".into()]),
            "a 'b c' 'it'\\''s' /x/y.yaml"
        );
    }

    /// Entry 113 of the divergence record, which states this verb's contract.
    fn entry_block() -> serde_json::Value {
        let record = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/contract-divergences.md"),
        )
        .expect("the divergence record ships");
        let entry = record
            .split_once("\n## 113.")
            .expect("this verb is recorded under entry 113")
            .1;
        let entry = entry.split_once("\n## ").map_or(entry, |(head, _)| head);
        let block = entry
            .split_once("```json")
            .expect("entry 113 carries a json block")
            .1
            .split_once("```")
            .expect("the block is fenced")
            .0;
        serde_json::from_str(block).expect("entry 113's block is JSON")
    }

    fn keys(value: &serde_json::Value) -> Vec<String> {
        value
            .as_object()
            .expect("an object")
            .keys()
            .cloned()
            .collect()
    }

    fn named(block: &serde_json::Value, field: &str) -> Vec<String> {
        serde_json::from_value(block[field].clone())
            .unwrap_or_else(|_| panic!("entry 113 names {field}"))
    }

    /// The document, the row, the disk reading and the acknowledgement file are
    /// exactly what entry 113 and `docs/stop-guard.md` state, in their order, and
    /// the file round-trips.
    #[test]
    fn the_documented_shapes_are_the_ones_this_build_writes() {
        let block = entry_block();
        let entry = AcknowledgedBranch {
            branch: "b".into(),
            identity: "i".into(),
            tip: "a".repeat(40),
            reason: "kept".into(),
            at: "2026-10-07T12:34:56Z".into(),
        };
        let row = Row {
            identity: "i".into(),
            branch: "b".into(),
            base: "main".into(),
            provenance: onevcs::Provenance::Complete,
            tip: Some("a".repeat(40)),
            landed: Landed::No,
            change_url: None,
            stopped_because: "stopped".into(),
            session: None,
            run: None,
            node: None,
            manager_session: None,
            recover_command: Vec::new(),
            land_command: Vec::new(),
            retirement: None,
            in_flight: false,
            counted: true,
            acknowledgement: Some(entry.clone()),
            disk: Some(Disk {
                run_root: None,
                run_root_bytes: None,
                build_output: BTreeMap::new(),
            }),
        };
        let listing = Unpublished {
            target: Target::Host,
            verdict: Verdict::Owed,
            rows: vec![row],
            unresolved: Vec::new(),
        };
        let document = serde_json::to_value(&listing).expect("serializes");
        assert_eq!(keys(&document), named(&block, "document_fields"));
        assert_eq!(keys(&document["rows"][0]), named(&block, "row_fields"));
        assert_eq!(
            keys(&document["rows"][0]["disk"]),
            named(&block, "disk_fields")
        );
        let file = AcknowledgementFile {
            version: ACKNOWLEDGEMENT_FILE_VERSION,
            acknowledged: vec![entry],
        };
        let written = serde_json::to_value(&file).expect("serializes");
        assert_eq!(keys(&written), named(&block, "acknowledgement_file_fields"));
        assert_eq!(
            keys(&written["acknowledged"][0]),
            named(&block, "acknowledgement_fields")
        );
        assert_eq!(
            block["acknowledgement_version"].as_u64(),
            Some(u64::from(ACKNOWLEDGEMENT_FILE_VERSION))
        );
        assert_eq!(
            block["acknowledgements_under"].as_str(),
            Some(ACKNOWLEDGEMENTS_UNDER)
        );
        assert_eq!(
            named(&block, "build_output"),
            BUILD_OUTPUT_DIRECTORIES.map(str::to_owned).to_vec()
        );
        let read: AcknowledgementFile =
            serde_json::from_value(written).expect("the file reads back");
        assert_eq!(read, file);
        for (kind, target) in [
            (
                "session",
                Target::Session {
                    session: "m".into(),
                },
            ),
            ("host", Target::Host),
            (
                "tokens",
                Target::Tokens {
                    tokens: vec!["s-1".into()],
                },
            ),
        ] {
            assert_eq!(serde_json::to_value(&target).expect("target")["kind"], kind);
        }
        let exits = &block["exit"];
        for (name, code) in [
            ("none", EXIT_NOTHING_COUNTED),
            ("owed", EXIT_COUNTED),
            ("unanswered", EXIT_UNANSWERED),
            ("refused", crate::error::EXIT_REFUSED),
        ] {
            assert_eq!(exits[name].as_i64(), Some(i64::from(code)), "{name}");
        }
        // And the stop-guard page states the same row fields.
        let page = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/stop-guard.md"),
        )
        .expect("the page ships");
        let stated = page
            .split("Each `<row>` is `{")
            .nth(1)
            .and_then(|rest| rest.split("}`").next())
            .expect("the page states the row");
        let stated: Vec<String> = stated.split(", ").map(str::to_owned).collect();
        assert_eq!(stated, named(&block, "row_fields"));
    }
}
