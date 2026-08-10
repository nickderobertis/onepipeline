//! The onejudge report a settled member left behind.
//!
//! `oneagentgraph` stores each member's full report and puts its `report_path`
//! on the `member-settled` it relays here, so the evidence behind a dispatch —
//! every turn, its tools, its text, and what the two sides of the conversation
//! spent — is retained rather than summarised away.
//!
//! # Ingest copies; readers never follow
//!
//! `report_path` points into the *producing* library's own scratch, which is a
//! directory this crate neither chooses nor can attest. A reader that opened
//! whatever a journal line named would be an arbitrary-file reader driven by
//! whatever wrote to the journal — an absolute path anywhere on the host, or a
//! symlink to something else entirely, and its contents printed by `transcript`.
//!
//! So the two halves are split. [`retain`] runs at **ingest**, on the envelope a
//! process this crate started has just written to its own stdout, and copies the
//! report into the run's own [`reports_dir`](crate::ledger::RunPaths::reports_dir)
//! — refusing anything that is not a plain file of the producing library's own
//! name and size. [`evidence`] and [`read`] run at **read** time and open
//! nothing but that copy, at a path derived from the settlement rather than
//! taken from it. A settlement whose copy is not there is reported as unretained,
//! naming the path that was not read.
//!
//! The document itself is read **structurally**, by field name, rather than into
//! the producing library's own types. The report is a sibling's artifact and this
//! crate is a consumer of it: a stricter read would refuse a whole report over
//! one field it did not recognise and report nothing at all, which for evidence
//! is the wrong direction to fail in. Every other cross-library read here is
//! lenient for the same reason.

// llmlint: ignore-file[invalid_states_unrepresentable] `Turn::role` and `Tool::kind` are
// **onejudge's** vocabulary, read out of an artifact that library wrote, and this crate
// only renders them. Narrowing either into an enum here would re-declare a vocabulary a
// sibling owns — the re-declaration src/AGENTS.md forbids — and would make a role or a
// tool kind that a newer onejudge emits unrenderable, which for evidence is the wrong
// direction to fail in. `src/event.rs` and `src/vcs.rs` carry the same suppression for the
// same reason. The one place this crate *branches* on a sibling's role,
// `telemetry::of_run`, parses it through `oneagentgraph::event::Role` rather than matching
// strings.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::event::{Envelope, Source};
use crate::ledger::RunPaths;

/// The kind `oneagentgraph` settles a member with.
pub const MEMBER_SETTLED: &str = "member-settled";

/// The payload key naming where the member's report was stored.
pub const REPORT_PATH: &str = "report_path";

/// The most of a report this run copies into its own storage.
///
/// A bound rather than a promise about size: the copy happens on the engine's
/// single-writer thread while a round is converging, and a producer that named
/// something enormous must not be able to stall it or fill the runs root. Well
/// past a real report, which is a transcript and its verdicts.
pub const MAX_REPORT_BYTES: u64 = 32 * 1024 * 1024;

/// What one settlement said about its report, and where this run's copy would
/// be.
///
/// Deliberately *not* named for having kept one: a settlement names a report
/// whether or not the copy was made, and telling a reader which of those it is
/// meeting is the whole job. [`read`] on [`kept`](Self::kept) is the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    /// The node whose dispatch produced it, when the envelope named one.
    pub node: Option<String>,
    /// The member within that dispatch, when the producer stamped one.
    pub member: Option<String>,
    /// Where the producing library said it stored it. **Displayed, never
    /// opened**: it is a stranger's path on a journal line.
    pub named: PathBuf,
    /// This run's own copy, under its
    /// [`reports_dir`](crate::ledger::RunPaths::reports_dir). The only file any
    /// reader here opens, and derived from the settlement rather than taken
    /// from it.
    pub kept: PathBuf,
}

// llmlint: ignore-block[boundary_inputs_validated] this function is the *only* place a
// producer-named path is opened, and there is no producer-owned root left to confine it
// to. `oneagentgraph` mints its report path under a state directory whose location its
// **binary** resolves — the const is private to `src/main.rs`, the library exposes no
// accessor, an operator moves it with an environment variable this crate does not set, and
// a future executor stores it on another machine — so a root pinned here would be this
// crate re-declaring a sibling's config, and would refuse legitimate reports the moment it
// was wrong. What bounds this instead is *when* it runs and *what it accepts*: the envelope
// is arriving on the stdout of a process this crate spawned, before the line exists
// anywhere a stranger could have written it; the name must be the producing library's own
// `REPORT_FILE`; a symlink, a directory, and anything past `MAX_REPORT_BYTES` are refused
// out loud; and the destination is derived, never taken. Every *reader* — `transcript`,
// `telemetry` — opens only that destination, so a line forged into a journal afterwards
// reaches nothing. What remains is a producer that has been compromised copying one
// `report.json` it wrote into the run that spawned it, which is inside the authority it
// already has: that same producer chooses the report's contents. Divergence 12 in
// `docs/contract-divergences.md` records the missing accessor as the open proposal it is.
/// Copy the report a relayed settlement names into the run's own storage.
///
/// Called as the envelope is **ingested** — from the stdout of a process this
/// crate started, before the line exists anywhere a stranger could have written
/// it — which is the one moment the named path carries the producer's authority
/// rather than the journal's.
///
/// Everything it refuses, it refuses out loud and without opening: a name that
/// is not the producing library's own [`REPORT_FILE`](oneagentgraph::member::REPORT_FILE),
/// anything that is not a plain file (a symlink is the case this exists for —
/// it names one file and delivers another), and anything past
/// [`MAX_REPORT_BYTES`]. A refusal costs the transcript its words and nothing
/// else: the settlement still relays, and every reader says the copy is not
/// there.
pub fn retain(paths: &RunPaths, event: &Envelope) {
    if event.source != Source::Agentgraph || event.kind.0 != MEMBER_SETTLED {
        return;
    }
    let Some(named) = event
        .payload
        .get(REPORT_PATH)
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty())
    else {
        return;
    };
    let refuse = |why: &str| {
        eprintln!("onepipeline: not retaining the report at '{named}': {why}");
    };
    if Path::new(named).file_name().and_then(|name| name.to_str())
        != Some(oneagentgraph::member::REPORT_FILE)
    {
        return refuse(&format!(
            "a report the producing library wrote is named {}",
            oneagentgraph::member::REPORT_FILE
        ));
    }
    // A symlink named as a report is a path that says one thing and delivers
    // another, so it is looked at without following — for the *message*. What
    // makes the refusal hold is the open below, which will not follow the last
    // component whatever changed under this check in between.
    if std::fs::symlink_metadata(named).is_ok_and(|about| about.file_type().is_symlink()) {
        return refuse("it is a symlink, and a report is a file the producer wrote");
    }
    let source = match open_no_follow(Path::new(named)) {
        Ok(source) => source,
        Err(error) => return refuse(&format!("it cannot be opened as a plain file: {error}")),
    };
    // Asked of the open handle, so what is measured is what will be read: a
    // path checked and then opened is two different files on a bad day.
    match source.metadata() {
        Err(error) => return refuse(&format!("it cannot be read: {error}")),
        Ok(about) if !about.is_file() => return refuse("it is not a file"),
        Ok(about) if about.len() > MAX_REPORT_BYTES => {
            return refuse(&format!("it is larger than {MAX_REPORT_BYTES} bytes"))
        }
        Ok(_) => {}
    }

    // The run's own storage has to *be* the run's own: a directory swapped for
    // a link points every copy this run makes somewhere else.
    let reports = paths.reports_dir();
    if std::fs::symlink_metadata(&reports).is_ok_and(|about| about.file_type().is_symlink()) {
        return refuse(&format!(
            "{} is a symlink, and this run's own storage is a directory it owns",
            reports.display()
        ));
    }
    if let Err(error) = std::fs::create_dir_all(&reports) {
        return refuse(&format!("{} cannot be created: {error}", reports.display()));
    }
    let kept = paths.report_for(&event.stream, event.seq);
    let written = match create_new_no_follow(&kept) {
        Ok(destination) => destination,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            // `create_new` never follows and never truncates, so nothing was
            // written whatever is there. A plain file is this run's own copy,
            // already made; anything else is something a tamperer put in its
            // place, and it is said out loud rather than worked around.
            if !std::fs::symlink_metadata(&kept).is_ok_and(|about| about.is_file()) {
                refuse(&format!(
                    "{} already exists and is not a plain file, so nothing was written \
                     through it",
                    kept.display()
                ));
            }
            return;
        }
        Err(error) => {
            return refuse(&format!(
                "it cannot be copied to {}: {error}",
                kept.display()
            ))
        }
    };
    // Bounded again on the way through: the size was true of the handle when it
    // was measured, and a file being appended to while it is read is not.
    use std::io::Read;
    let copied = std::io::copy(&mut source.take(MAX_REPORT_BYTES), &mut { written });
    if let Err(error) = copied {
        refuse(&format!(
            "it cannot be copied to {}: {error}",
            kept.display()
        ));
    }
}
// llmlint: ignore-end[boundary_inputs_validated]

/// Open a path for reading **without following** its last component.
///
/// The guarantee is the open's, not a check's: a path tested and then opened is
/// two different files on a bad day, and the whole point of refusing a symlink
/// is that the name and the file disagree.
fn open_no_follow(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    // Nothing narrows the open itself on other platforms, so the link is
    // refused before it instead. The window between the two is that platform's;
    // every symlink journey in this crate's suite runs where the flag exists.
    #[cfg(not(unix))]
    if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the path is a symlink",
        ));
    }
    options.open(path)
}

/// Create a file that must not already exist, and must not be reached through a
/// link.
///
/// `create_new` is `O_CREAT | O_EXCL`, which POSIX requires to fail on a
/// symlink whatever it points at — so a destination pre-planted as a link is
/// refused rather than written *through*, and an existing copy is never
/// truncated. Both properties are the open's, in one atomic step.
fn create_new_no_follow(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

/// What every `member-settled` in this store said about its report, in
/// settlement order — including the ones whose copy was refused, which are
/// exactly the ones a reader has to be able to name.
///
/// A settlement that stored no report is absent rather than listed with an
/// empty path: the producer says so with a null `report_path`, and a consumer
/// that invented a path for it would send a reader looking for a file nobody
/// wrote.
///
/// What comes back names *both* paths and opens neither. The copy's name is
/// derived from the settlement's own stream and sequence, so a reader reaches
/// this run's storage whatever the journal line says the producer's path was.
pub fn evidence(paths: &RunPaths, events: &[Envelope]) -> Vec<Evidence> {
    events
        .iter()
        .filter(|event| event.source == Source::Agentgraph && event.kind.0 == MEMBER_SETTLED)
        .filter_map(|event| {
            let named = event
                .payload
                .get(REPORT_PATH)
                .and_then(Value::as_str)
                .filter(|path| !path.is_empty())?;
            Some(Evidence {
                node: event.labels.node.clone(),
                member: event
                    .labels
                    .extra
                    .get("member")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                named: PathBuf::from(named),
                kept: paths.report_for(&event.stream, event.seq),
            })
        })
        .collect()
}

/// Read this run's own copy of one report, or `None` when it did not keep one.
///
/// The path is [`Evidence::kept`] and nothing else — but "the run owns that
/// directory" is a claim about a directory, not a fact about the file found
/// there. A run directory a tamperer reached can hold a **symlink** where the
/// copy was, and following one would print whatever it points at under the
/// name of a dispatch's own words. So the copy is opened without following and
/// read only as a plain file, bounded as it was when it was written.
///
/// Absent is quiet, because it is ordinary: a dispatch whose report was refused
/// at ingest, ran on another machine, or was swept has no copy here, and the
/// caller says so on its own line. A copy that *is* there and is not a plain
/// file is not ordinary, and is said out loud.
pub fn read(kept: &Path) -> Option<Value> {
    let refuse = |why: &str| {
        eprintln!(
            "onepipeline: not reading the retained report at {}: {why}",
            kept.display()
        );
    };
    let file = match open_no_follow(kept) {
        Ok(file) => file,
        // Nothing there is the ordinary answer, and the reader has a line for
        // it already.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            refuse(&format!("it is not a plain file this run wrote: {error}"));
            return None;
        }
    };
    match file.metadata() {
        Err(error) => {
            refuse(&format!("it cannot be read: {error}"));
            return None;
        }
        Ok(about) if !about.is_file() => {
            refuse("it is not a plain file this run wrote");
            return None;
        }
        Ok(about) if about.len() > MAX_REPORT_BYTES => {
            refuse(&format!("it is larger than {MAX_REPORT_BYTES} bytes"));
            return None;
        }
        Ok(_) => {}
    }
    use std::io::Read;
    let mut text = String::new();
    file.take(MAX_REPORT_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    serde_json::from_str(&text).ok()
}

/// The turns a report's transcript carries, in order.
///
/// Empty for a report that carries no transcript, which is a report this build
/// can say nothing further about rather than a conversation that never happened.
pub fn turns(document: &Value) -> Vec<Turn> {
    document
        .get("transcript")
        .and_then(|transcript| transcript.get("messages"))
        .and_then(Value::as_array)
        .map(|messages| messages.iter().map(Turn::of).collect())
        .unwrap_or_default()
}

/// One turn of a retained transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    /// Who produced it, as the report names them.
    pub role: String,
    /// What they said.
    pub text: String,
    /// The tools the turn used, in the order it used them.
    pub tools: Vec<Tool>,
}

impl Turn {
    fn of(message: &Value) -> Self {
        Self {
            role: string(message, "role"),
            text: string(message, "content"),
            tools: message
                .get("events")
                .and_then(Value::as_array)
                .map(|events| events.iter().map(Tool::of).collect())
                .unwrap_or_default(),
        }
    }
}

/// One tool call a turn made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    /// `tool_call` or `tool_result`, as the report names it.
    pub kind: String,
    /// The tool, where the harness named one.
    pub name: String,
    /// What it acted on, rendered compactly.
    pub detail: String,
}

impl Tool {
    fn of(event: &Value) -> Self {
        Self {
            kind: string(event, "kind"),
            name: string(event, "name"),
            detail: match event.get("input") {
                None | Some(Value::Null) => String::new(),
                Some(Value::String(text)) => text.clone(),
                Some(input) => input.to_string(),
            },
        }
    }
}

fn string(value: &Value, key: &str) -> String {
    match value.get(key) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventKind, Labels, ENVELOPE_VERSION};
    use serde_json::json;

    /// A scratch root for one test, removed when it ends.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "onepipeline-report-{name}-{}-{:?}",
            crate::sys::pid(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch root");
        dir
    }

    /// A producer's scratch holding the file it says it wrote.
    fn produced(root: &Path, body: &str) -> PathBuf {
        let dir = root.join("producer");
        std::fs::create_dir_all(&dir).expect("a producer scratch");
        let path = dir.join(oneagentgraph::member::REPORT_FILE);
        std::fs::write(&path, body).expect("a stored report");
        path
    }

    fn settled(node: Option<&str>, path: Option<&str>) -> Envelope {
        let mut labels = Labels {
            node: node.map(str::to_string),
            ..Labels::default()
        };
        labels.extra.insert("member".into(), "worker".into());
        Envelope {
            v: ENVELOPE_VERSION,
            ts: "2026-08-08T00:00:00.000Z".into(),
            stream: "oneagentgraph-1".into(),
            seq: 4,
            source: Source::Agentgraph,
            kind: EventKind(MEMBER_SETTLED.into()),
            labels,
            payload: crate::journal::payload(&[(REPORT_PATH, json!(path))]),
            artifacts: Vec::new(),
        }
    }

    /// Ingest copies the report into the run's own storage, and the reader is
    /// pointed at that copy rather than at the path the settlement named.
    #[test]
    fn ingest_keeps_the_run_its_own_copy_and_the_reader_opens_that() {
        let root = scratch("kept");
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");
        let produced = produced(&root, r#"{"transcript":{"messages":[]}}"#);

        let event = settled(Some("build"), Some(&produced.display().to_string()));
        retain(&paths, &event);

        let retained = evidence(&paths, &[event]);
        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].node.as_deref(), Some("build"));
        assert_eq!(retained[0].member.as_deref(), Some("worker"));
        assert_eq!(retained[0].named, produced);
        assert_eq!(retained[0].kept, paths.report_for("oneagentgraph-1", 4));
        assert!(
            retained[0].kept.starts_with(paths.reports_dir()),
            "the copy is not in the run's own storage: {:?}",
            retained[0].kept
        );
        assert!(read(&retained[0].kept).is_some());

        // The producer's file going away afterwards costs the run nothing: its
        // own copy is what every reader opens.
        std::fs::remove_file(&produced).expect("the producer's copy is removed");
        assert!(read(&retained[0].kept).is_some());
        std::fs::remove_dir_all(&root).ok();
    }

    /// A `null` or empty `report_path` is the producer saying it stored none.
    #[test]
    fn a_settlement_that_stored_none_is_not_listed_with_an_invented_path() {
        let paths = RunPaths::under(Path::new("/nowhere"), "demo");
        assert!(evidence(&paths, &[settled(Some("build"), None)]).is_empty());
        assert!(evidence(&paths, &[settled(Some("build"), Some(""))]).is_empty());
    }

    /// Ingest opens a path a *live producer* named, so it refuses everything
    /// that is not the plain file that producer writes — a symlink most of all,
    /// which names one file and delivers another.
    #[test]
    fn ingest_refuses_anything_that_is_not_the_producers_own_plain_file() {
        let root = scratch("refused");
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");
        let secret = root.join("secret.json");
        std::fs::write(&secret, r#"{"transcript":{"messages":[]}}"#).expect("a secret");

        let planted = root.join("planted");
        std::fs::create_dir_all(&planted).expect("a planted directory");
        let link = planted.join(oneagentgraph::member::REPORT_FILE);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&secret, &link).expect("a symlink");
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&secret, &link).expect("a symlink");

        for named in [
            // A symlink wearing the producer's own file name.
            link.display().to_string(),
            // A file the producing library never writes.
            secret.display().to_string(),
            // Nothing at all.
            root.join("gone")
                .join(oneagentgraph::member::REPORT_FILE)
                .display()
                .to_string(),
            // A directory.
            planted.display().to_string(),
        ] {
            let event = settled(Some("build"), Some(&named));
            retain(&paths, &event);
            let kept = &evidence(&paths, &[event])[0].kept;
            assert!(
                read(kept).is_none(),
                "'{named}' was copied into the run's storage"
            );
        }
        std::fs::remove_dir_all(&root).ok();
    }

    /// A report past the bound is refused rather than copied: the copy happens
    /// on the engine's single-writer thread, mid-round.
    #[test]
    fn ingest_refuses_a_report_past_its_bound() {
        let root = scratch("oversize");
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");
        let produced = produced(&root, "x");
        // Claimed rather than written: the check is on the size the filesystem
        // reports, and a real 32MiB fixture would be a slow way to say so.
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&produced)
            .expect("the stored report");
        file.set_len(MAX_REPORT_BYTES + 1).expect("a large report");
        drop(file);

        let event = settled(Some("build"), Some(&produced.display().to_string()));
        retain(&paths, &event);
        assert!(read(&evidence(&paths, &[event])[0].kept).is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_pipeline_event_of_the_same_shape_is_not_a_members_report() {
        let root = scratch("ours");
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");
        let produced = produced(&root, "{}");
        let mut ours = settled(Some("build"), Some(&produced.display().to_string()));
        ours.source = Source::Pipeline;

        retain(&paths, &ours);
        assert!(evidence(&paths, &[ours]).is_empty());
        assert!(
            !paths.reports_dir().exists(),
            "this crate's own event was ingested as a sibling's report"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_transcripts_turns_carry_their_text_and_their_tools() {
        let document = json!({
            "transcript": {"messages": [
                {"role": "user", "content": "## What\nship it"},
                {"role": "assistant", "content": "Ran the gate.", "events": [
                    {"kind": "tool_call", "name": "bash",
                     "input": {"command": "just check"}, "index": 0},
                    {"kind": "tool_result", "output": "ok", "index": 1},
                ]},
            ]},
        });
        let turns = turns(&document);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].role, "user");
        assert!(turns[0].tools.is_empty());
        assert_eq!(turns[1].text, "Ran the gate.");
        assert_eq!(turns[1].tools[0].name, "bash");
        assert!(turns[1].tools[0].detail.contains("just check"));
        // A result names no tool, and is not given one.
        assert_eq!(turns[1].tools[1].kind, "tool_result");
        assert!(turns[1].tools[1].name.is_empty());
    }

    #[test]
    fn a_report_carrying_no_transcript_has_no_turns_rather_than_a_refusal() {
        assert!(turns(&json!({"usage": {"input_tokens": 1}})).is_empty());
        assert!(turns(&json!({"transcript": {}})).is_empty());
        assert!(turns(&Value::Null).is_empty());
    }

    #[test]
    fn a_report_that_is_not_there_to_read_is_absent() {
        assert!(read(Path::new("/nowhere/onepipeline/report.json")).is_none());
    }
}
