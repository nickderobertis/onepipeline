//! The report a settled member left behind.
//!
//! `oneagentgraph` stores each member's full report and puts its `report_path`
//! on the `member-settled` it relays here, so the evidence behind a dispatch —
//! every turn, its tools, its text, and what the two sides of the conversation
//! spent — is retained rather than summarised away.
//!
//! **Whose report depends on the member.** A two-party (`kind: onejudge`) member
//! settles on onejudge's own report, whose `transcript` is the conversation its
//! two sides had. A single-sided (`kind: oneharness`) member settles on
//! oneharness's run report: one entry per harness the run tried, each carrying
//! that harness's final text, the tool events it took, and — for a run whose
//! config declared a schema — the answer that was validated against it. Both are
//! read here, because both are what a dispatch of this stack can leave behind.
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
//! report into the run's own [`reports_dir`](crate::views::RunPaths::reports_dir)
//! — refusing anything that is not a plain file of the producing library's own
//! name and size. `evidence` and `read` run at **read** time and open nothing
//! but that copy, at a path derived from the settlement rather than taken from
//! it. A settlement whose copy is not there is reported as unretained, naming
//! the path that was not read.
//!
//! # What is published, and what is behind it
//!
//! [`retain`] is the writer half of the contract's retention path, published
//! beside the items a caller needs to build an envelope it will accept —
//! [`MEMBER_SETTLED`], [`REPORT_PATH`], [`ACCEPTED_REPORT_FILE`], and
//! [`MAX_REPORT_BYTES`] — so a consumer writes a report through the same
//! promise it resolves one back through,
//! [`RunPaths::report_for`](crate::views::RunPaths::report_for). Everything
//! else here is the engine behind that surface and is crate-visible: what this
//! crate's own views render out of a retained report is a rendering, not a
//! promise.
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
use crate::views::RunPaths;

/// The kind `oneagentgraph` settles a member with.
pub const MEMBER_SETTLED: &str = "member-settled";

/// The payload key naming where the member's report was stored.
pub const REPORT_PATH: &str = "report_path";

/// The one base name [`retain`] accepts at [`REPORT_PATH`].
///
/// The producing library's own report file name, re-exported here so a consumer
/// proving this path holds it from the crate that enforces it rather than from a
/// `oneagentgraph` dependency of its own — and so there is one spelling of it,
/// which is the one the refusal below is written against.
pub const ACCEPTED_REPORT_FILE: &str = oneagentgraph::member::REPORT_FILE;

/// The most of a report this run copies into its own storage.
///
/// A bound rather than a promise about size: the copy happens on the engine's
/// single-writer thread, on the reconcile pass that ingests the settlement, and
/// that thread is what drives every other node in the run — so a producer that named
/// something enormous must not be able to stall it or fill the runs root. Well
/// past a real report, which is a transcript and its verdicts.
pub const MAX_REPORT_BYTES: u64 = 32 * 1024 * 1024;

/// What one settlement said about its report, and where this run's copy would
/// be.
///
/// Deliberately *not* named for having kept one: a settlement names a report
/// whether or not the copy was made, and telling a reader which of those it is
/// meeting is the whole job. [`read`] on [`kept`](Self::kept) is the answer.
///
/// Crate-visible: `docs/contract.md` names the retention path and the views
/// rendered from it, not the shape this crate assembles a view out of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Evidence {
    /// The node whose dispatch produced it, when the envelope named one.
    pub node: Option<String>,
    /// The member within that dispatch, when the producer stamped one.
    pub member: Option<String>,
    /// Where the producing library said it stored it. **Displayed, never
    /// opened**: it is a stranger's path on a journal line.
    pub named: PathBuf,
    /// This run's own copy, under its
    /// [`reports_dir`](crate::views::RunPaths::reports_dir). The only file any
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
/// Copy the report a relayed settlement names into the run's own storage, at
/// the path [`RunPaths::report_for`] derives for it.
///
/// # The caller holds the producer's authority for the path
///
/// This is the **precondition**, and publishing the function does not widen it.
/// It is called as the envelope is **ingested** — from the stdout of a process
/// the caller started, before the line exists anywhere a stranger could have
/// written it — which is the one moment the named path carries the producer's
/// authority rather than the journal's. A caller that does not hold that
/// authority for the path the envelope names is handing this an arbitrary file
/// to copy, chosen by whatever wrote the line. Reading a run's store back is not
/// that moment: nothing here re-opens what a producer named, and a settlement
/// this run kept no copy of stays uncopied rather than being fetched later.
///
/// Everything it refuses, it refuses out loud and without opening: an envelope
/// that is not an `oneagentgraph` [`MEMBER_SETTLED`], a name at
/// [`REPORT_PATH`] that is not [`ACCEPTED_REPORT_FILE`], anything that is not a
/// plain file (a symlink is the case this exists for — it names one file and
/// delivers another), and anything past [`MAX_REPORT_BYTES`]. A refusal costs
/// the transcript its words and nothing else: the settlement still relays, and
/// every reader says the copy is not there.
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
    if Path::new(named).file_name().and_then(|name| name.to_str()) != Some(ACCEPTED_REPORT_FILE) {
        return refuse(&format!(
            "a report the producing library wrote is named {ACCEPTED_REPORT_FILE}"
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
pub(crate) fn evidence(paths: &RunPaths, events: &[Envelope]) -> Vec<Evidence> {
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

/// The payload key a settlement carries its verdicts inline on.
///
/// `oneagentgraph` copies the report's own `verdicts` onto every
/// [`MEMBER_SETTLED`] it publishes, so what failed a node's judge is on the
/// stream this run already merged — the reason it can be said without any file
/// being opened, retained or not.
const VERDICT: &str = "verdict";

/// One judge verdict that **failed** the member it was scored against.
///
/// Crate-visible, like everything else here that is not the retention path: what
/// a view renders out of a settlement is a rendering rather than a promise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FailedVerdict {
    /// The criterion that was scored, when the record named one.
    pub criterion: Option<String>,
    /// The judge's own justification, when the record carried one.
    ///
    /// The single most useful sentence about a node that failed on its judge,
    /// and until it was rendered it was reachable only by opening the node's
    /// retained report by hand.
    pub reason: Option<String>,
}

/// Every judge verdict that failed one node's dispatches, in settlement order.
///
/// **Only a boolean verdict that came back false is one.** That is the whole of
/// what onejudge fails a run over — a numeric score is reported and gates
/// nothing — so a consumer that treated a low score as a failure would name a
/// verdict that failed nothing as the reason a node failed.
///
/// Read **structurally**, by field name, exactly as the report document beside
/// it is and for the reason this module's header gives: these are a sibling's
/// records, nothing here acts on them, and a stricter read would answer a real
/// settlement by rendering nothing at all. A field a newer producer spells
/// differently costs that field rather than the verdict.
// llmlint: ignore-block[boundary_inputs_validated] the leniency is the decision above,
// and the same one every other cross-library read here makes: this is a relayed envelope
// already in the run's own merged store, and what comes out of it is rendered for a
// person and acted on by nothing.
pub(crate) fn failed_verdicts(events: &[Envelope], node: &str) -> Vec<FailedVerdict> {
    events
        .iter()
        .filter(|event| event.source == Source::Agentgraph && event.kind.0 == MEMBER_SETTLED)
        .filter(|event| event.labels.node.as_deref() == Some(node))
        .filter_map(|event| event.payload.get(VERDICT))
        .filter_map(Value::as_array)
        .flatten()
        .filter(|named| named.pointer("/verdict/value") == Some(&Value::Bool(false)))
        .map(|named| FailedVerdict {
            criterion: text_of(named.get("criterion")),
            reason: text_of(named.pointer("/verdict/reason")),
        })
        .collect()
}

/// One string a sibling's record carried, or `None` for one it did not.
///
/// Empty is absent: a criterion nobody wrote and an empty one are the same fact
/// to a reader, and rendering the empty string would put a bare pair of quotes
/// where the missing-value phrase belongs.
fn text_of(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .map(str::to_string)
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
pub(crate) fn read(kept: &Path) -> Option<Value> {
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
    file.take(MAX_REPORT_BYTES).read_to_string(&mut text).ok()?;
    serde_json::from_str(&text).ok()
}

/// The turns a report carries, in order.
///
/// **Two report shapes, because a node is dispatched to either member kind.** A
/// two-party `kind: onejudge` member settles with onejudge's report, whose
/// `transcript.messages` is a conversation of turns. A single-sided
/// `kind: oneharness` member settles with **oneharness's own** run report, which
/// has no conversation at all: it is one entry per harness the run attempted,
/// carrying that harness's final answer and the actions it took. Both are
/// contract-shipped node graphs, so a reader that knew only the first answered
/// `it carries no transcript` for every single-sided dispatch — a report that was
/// retained, named, and readable, reported as one this build cannot read.
///
/// Empty for a report carrying neither, which is a report this build can say
/// nothing further about rather than a conversation that never happened.
///
/// **Read leniently, on purpose, and only ever rendered.** The document is a
/// producer's, not this crate's, and nothing here acts on it: [`read`] has
/// already refused anything that is not a plain file this run retained, and what
/// survives is printed for a person. So a field a newer producer spells
/// differently costs that field rather than the whole transcript — refusing the
/// document would answer a real, retained, readable report by claiming this build
/// cannot read it, which is the failure this function exists to remove. What is
/// *not* lenient is attribution: a turn nothing names is dropped rather than
/// rendered under a blank identity.
// llmlint: ignore-block[boundary_inputs_validated] the leniency above is the
// decision, and it is the same one the onejudge arm has always made; `read` is the
// boundary this document is validated at.
pub(crate) fn turns(document: &Value) -> Vec<Turn> {
    if let Some(messages) = document
        .get("transcript")
        .and_then(|transcript| transcript.get("messages"))
        .and_then(Value::as_array)
    {
        return messages.iter().map(Turn::of).collect();
    }
    results(document).filter_map(Turn::of_result).collect()
}

/// The per-harness results a run report carries, or nothing for a document that
/// is not one.
fn results(document: &Value) -> impl Iterator<Item = &Value> {
    document
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// What a drafting dispatch's retained reports answered with.
///
/// Three readings rather than a body or nothing, because the two endings that
/// are not a body need different fixes: a graph the schema will not accept an
/// answer from is a graph or a schema to correct, and one that answers inside
/// the schema with nothing in it is a prompt to correct. A single `None` said
/// neither, and a run that had just wired a drafter could not tell them apart
/// from a launch that had wired none.
///
/// Crate-visible: `docs/contract.md` names the retention path and the views
/// rendered from it, and what this crate reads out of a retained report to
/// decide a publication is a reading, not a promise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Drafted {
    /// The validated answer's `body` — the prose the change request opens with.
    Body(String),
    /// A schema was asked for an answer, refused one, and accepted none.
    SchemaRefused,
    /// Nothing that was asked of the schema is a body worth publishing: no
    /// schema was asked for, no candidate ran, or one conformed and answered
    /// with nothing in it.
    Bodyless,
}

/// What a dispatch's retained reports drafted, read as one of the three endings
/// above.
///
/// Every report at once, because a dispatch retains one per member that settled
/// and each holds one entry per candidate an identity chain tried: which report
/// an entry landed in is not the reader's business, and reading them one at a
/// time is what makes a rule about "any refusal" disagree with itself across the
/// boundary between two of them.
///
/// The answer lives at `results[].structured` of the harness that **ran** — a
/// candidate the chain stepped past carries no answer — and it is taken as a
/// body only where the producing library says it conformed to the schema it was
/// validated against. A last-attempted value that failed validation is retained
/// there on purpose, so a consumer reading `structured` alone would take prose
/// the schema rejected.
///
/// The precedence is what the three endings mean. A body wins outright wherever
/// one is there to take, so a chain refused on the way to an answer that
/// conformed drafted a body. And a refusal is the ending only where **nothing**
/// conformed: a chain whose next candidate the schema accepted has answered the
/// question the schema was asked, and calling that `SchemaRefused` would send a
/// reader to correct a schema that is working.
pub(crate) fn drafted(reports: &[Value]) -> Drafted {
    let mut refused = false;
    let mut conformed = false;
    for result in reports.iter().flat_map(results) {
        match result.get("schema_valid").and_then(Value::as_bool) {
            Some(true) => {
                conformed = true;
                let body = result
                    .get("structured")
                    .and_then(|structured| structured.get("body"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .unwrap_or_default();
                if !body.is_empty() {
                    return Drafted::Body(body.to_owned());
                }
            }
            // The value the run last attempted is retained beside this flag, and
            // it is deliberately not read: publishing prose a schema refused is
            // the defect the flag exists to prevent.
            Some(false) => refused = true,
            // A run nobody asked for a schema leaves both fields null, which is
            // neither an answer refused nor one that conformed.
            None => {}
        }
    }
    if refused && !conformed {
        Drafted::SchemaRefused
    } else {
        Drafted::Bodyless
    }
}
// llmlint: ignore-end[boundary_inputs_validated]

/// One turn of a retained transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Turn {
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
            tools: Self::tools_of(message),
        }
    }

    /// One entry of an oneharness run report's `results`, as a turn.
    ///
    /// The role is the **harness** that produced it rather than `assistant`: a
    /// fallback chain records every candidate it attempted, so a reader with two
    /// entries in front of it needs to know which identity said which — and one
    /// that named them all `assistant` would read as a conversation that never
    /// happened.
    ///
    /// `None` for an entry that neither answered nor acted, which is a candidate
    /// the chain stepped past. Rendered, it would be a turn with a harness name
    /// and nothing under it, and a chain that fell through four times before it
    /// ran would bury the one turn that did. `None` too for an entry naming no
    /// harness: a turn is attributed or it is not shown, because an unnamed one
    /// among several is a reader guessing which identity said it.
    fn of_result(result: &Value) -> Option<Self> {
        let turn = Self {
            role: string(result, "harness"),
            text: string(result, "text"),
            tools: Self::tools_of(result),
        };
        let said_something = !turn.text.is_empty() || !turn.tools.is_empty();
        (!turn.role.is_empty() && said_something).then_some(turn)
    }

    fn tools_of(value: &Value) -> Vec<Tool> {
        value
            .get("events")
            .and_then(Value::as_array)
            .map(|events| events.iter().map(Tool::of).collect())
            .unwrap_or_default()
    }
}

/// One tool call a turn made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Tool {
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
    /// on the engine's single-writer thread, mid-pass.
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

    /// A single-sided member's report is oneharness's own, and it reads as the
    /// turns the chain actually took.
    ///
    /// The candidate that was stepped past carries neither an answer nor an
    /// action, and it is *not* a turn: a chain that fell through twice before it
    /// ran would otherwise show two empty ones above the only one a reader came
    /// for. What survives is named by the harness that produced it, because that
    /// is the identity the chain landed on.
    #[test]
    fn a_single_sided_members_report_reads_as_the_turns_its_chain_took() {
        let document = json!({
            "schema_version": "0.6",
            "results": [
                {"harness": "codex", "status": "skipped", "text": null},
                {"harness": "claude-code", "status": "ok", "text": "Ran the gate.",
                 "events": [
                     {"kind": "tool_call", "name": "bash",
                      "input": {"command": "just check"}, "index": 0},
                 ]},
            ],
        });
        let turns = turns(&document);
        assert_eq!(turns.len(), 1, "{turns:?}");
        assert_eq!(turns[0].role, "claude-code");
        assert_eq!(turns[0].text, "Ran the gate.");
        assert_eq!(turns[0].tools[0].name, "bash");
        assert!(turns[0].tools[0].detail.contains("just check"));
    }

    #[test]
    fn a_report_carrying_no_transcript_has_no_turns_rather_than_a_refusal() {
        assert!(turns(&json!({"usage": {"input_tokens": 1}})).is_empty());
        assert!(turns(&json!({"transcript": {}})).is_empty());
        assert!(turns(&Value::Null).is_empty());
        // A run whose every candidate was stepped past said nothing and did
        // nothing, which is a report with no turns rather than two blank ones.
        assert!(turns(&json!({"results": [{"harness": "codex", "status": "skipped"}]})).is_empty());
        // And an entry that answered but named no producer is not shown under a
        // blank identity.
        assert!(turns(&json!({"results": [{"text": "done"}]})).is_empty());
    }

    /// The drafted body is the validated answer of the result that ran, and
    /// nothing else is read as one — and where there is none, which ending it
    /// was.
    #[test]
    fn a_report_is_read_as_a_body_only_where_the_schema_accepted_one() {
        let one = |valid: Value, structured: Value| {
            vec![json!({"results": [{"schema_valid": valid, "structured": structured}]})]
        };
        assert_eq!(
            drafted(&one(json!(true), json!({"body": "## What\nit landed"}))),
            Drafted::Body("## What\nit landed".to_owned())
        );

        // The last-attempted value of a run that never conformed is retained on
        // the report on purpose: taking it would publish prose the schema
        // rejected. The refusal is the ending, and it is named as one.
        assert_eq!(
            drafted(&one(json!(false), json!({"body": "half a "}))),
            Drafted::SchemaRefused
        );
        // A schema nobody asked for leaves both fields null, which is neither a
        // refusal nor an answer.
        assert_eq!(drafted(&one(Value::Null, Value::Null)), Drafted::Bodyless);
        // A conforming answer whose body is blank is no body at all.
        assert_eq!(
            drafted(&one(json!(true), json!({"body": "  "}))),
            Drafted::Bodyless
        );
        assert_eq!(
            drafted(&one(json!(true), json!({"title": "feat: x"}))),
            Drafted::Bodyless
        );
        // And a document that is not a run report at all — or no report at all.
        assert_eq!(
            drafted(&[json!({"transcript": {"messages": []}})]),
            Drafted::Bodyless
        );
        assert_eq!(drafted(&[]), Drafted::Bodyless);

        // A chain that was refused on the way to an answer that conformed
        // drafted a body: the refusal is not the ending where there is prose to
        // publish.
        assert_eq!(
            drafted(&[json!({"results": [
                {"schema_valid": false, "structured": {"body": "half a "}},
                {"schema_valid": true, "structured": {"body": "## What\nit landed"}},
            ]})]),
            Drafted::Body("## What\nit landed".to_owned())
        );

        // And one refused on the way to an answer that conformed and said
        // nothing is **not** a refusal: the schema accepted an answer, so a
        // reader sent to correct it would be sent to correct the wrong thing.
        // The drafter is what answered with nothing.
        let refused_then_blank = json!({"results": [
            {"schema_valid": false, "structured": {"body": "half a "}},
            {"schema_valid": true, "structured": {"body": "   "}},
        ]});
        assert_eq!(
            drafted(std::slice::from_ref(&refused_then_blank)),
            Drafted::Bodyless
        );

        // Read across the reports rather than one at a time, so which report an
        // entry landed in cannot change the ending: the same two entries split
        // between two of a dispatch's retained reports answer identically.
        assert_eq!(
            drafted(&[
                json!({"results": [{"schema_valid": false, "structured": {"body": "half a "}}]}),
                json!({"results": [{"schema_valid": true, "structured": {"body": "   "}}]}),
            ]),
            Drafted::Bodyless
        );
        // In either order, because a chain is read for what it answered rather
        // than for the order two files were written in.
        assert_eq!(
            drafted(&[
                json!({"results": [{"schema_valid": true, "structured": {"body": "   "}}]}),
                json!({"results": [{"schema_valid": false, "structured": {"body": "half a "}}]}),
            ]),
            Drafted::Bodyless
        );
    }

    #[test]
    fn a_report_that_is_not_there_to_read_is_absent() {
        assert!(read(Path::new("/nowhere/onepipeline/report.json")).is_none());
    }
}
