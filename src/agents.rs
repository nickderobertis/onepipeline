//! Every agent a run launches, visible through oneharness's own run history —
//! automatically, whatever the agent structure of the target repository.
//!
//! The engine does not know what runs under a dispatch. A node-scope graph
//! declares its own members, a worker may run a nested tool that is itself an
//! oneharness turn, a judge may spawn a harness of its own, and a repository
//! wires all of it however it likes. What every one of those turns has in
//! common is the one thing this crate can reach: the **environment** the launch
//! inherits, which `oneharness` reads as a configuration layer of its own. So
//! the engine overlays three variables on every launch it starts — history on,
//! the run's **pointer file**, and its labels — and every oneharness turn under
//! that launch, however it was reached, appends one line to that file saying
//! where its session went. Nothing in the target repository has to be
//! configured for it, which is the property this module exists for.
//!
//! What the engine does **not** decide is where the history lives. It sets no
//! [`HISTORY_DIR_ENV`]: a repository's own `history_dir`, or the store its
//! environment names, or the platform default, is honoured untouched, and the
//! pointer line says which. The transcripts stay in each oneharness's own store,
//! where the operator already reads them with `oneharness history`, and "what
//! ran under this run" becomes one small read of the run's own directory.
//!
//! The reader here opens **only the pointer file**, through the linked
//! `oneharness-core`'s own [`read_pointers`]: never a store listing, never the
//! locking indexed reader, never a spawned `oneharness`, and never a session
//! file. On the host this was written for the default store is five gigabytes
//! across thirty thousand sessions with a gigabyte of index, and the only
//! lock-free read the library offers scans every file.
//!
//! The environment names, the key names, the scope words, the merge rule and
//! the pointer file's name are stated once in `docs/contract.md`, and
//! `tests/contract.rs` holds the constants here to it.

use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use oneharness_core::domain::history::{parse_labels, HistoryLabels, HistoryPointer};
use oneharness_core::io::history::read_pointers;
use serde::Serialize;

use crate::error::{Error, Result};
use crate::ledger::RunPaths;

/// Turns history on for every oneharness turn under the launch: `1`.
pub const HISTORY_ENV: &str = "ONEHARNESS_HISTORY";

/// Names the run's pointer file, [`SESSIONS_FILE`] under the run root,
/// absolute: every harness run with history on appends one line to it.
pub const POINTER_FILE_ENV: &str = "ONEHARNESS_HISTORY_POINTER_FILE";

/// The labels every session under the launch is stamped with, in oneharness's
/// wire format — comma-separated `key=value` — composed key-wise from what the
/// launch inherited and the engine's own keys. See [`compose_labels`].
pub const LABELS_ENV: &str = "ONEHARNESS_HISTORY_LABELS";

/// Where a oneharness keeps its history. **Never set by the engine**: a value
/// the launch inherits — the driver's environment, the dispatch-env hook's
/// document, a repository's own configuration — passes through untouched, so
/// the store stays wherever that repository's oneharness puts it.
pub const HISTORY_DIR_ENV: &str = "ONEHARNESS_HISTORY_DIR";

/// The pointer file's name under the run root.
pub const SESSIONS_FILE: &str = "oneharness-sessions.jsonl";

/// The prefix of every key the engine owns. A key under it in an inherited
/// label set is the engine's to replace; every other key is the repository's
/// and survives.
pub const LABEL_PREFIX: &str = "onepipeline.";

/// The run the dispatch belongs to.
pub const RUN_ID_LABEL: &str = "onepipeline.run_id";

/// The qualified project id off the launch record; absent when it names none.
pub const PROJECT_LABEL: &str = "onepipeline.project";

/// Which launch of the run this is — one of [`Scope`]'s words.
pub const SCOPE_LABEL: &str = "onepipeline.scope";

/// The node the dispatch is for; absent on the observer.
pub const NODE_LABEL: &str = "onepipeline.node";

/// The lifecycle step, on a step's dispatch and nowhere else.
pub const STEP_LABEL: &str = "onepipeline.step";

/// The attempt number that dispatch's `node-dispatched` records, decimal;
/// absent on the observer.
pub const ATTEMPT_LABEL: &str = "onepipeline.attempt";

/// Which launch of a run a dispatch is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    /// A node-scope dispatch, a lifecycle step included.
    Node,
    /// The dag-scope observer graph.
    Observer,
    /// The drafting graph a lifecycle node's change request body comes from.
    PrAuthor,
}

impl Scope {
    /// Every scope, for a reader holding the words to the contract.
    pub const ALL: [Scope; 3] = [Scope::Node, Scope::Observer, Scope::PrAuthor];

    /// The word [`SCOPE_LABEL`] carries.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Scope::Node => "node",
            Scope::Observer => "observer",
            Scope::PrAuthor => "pr-author",
        }
    }
}

/// The engine's own keys for one dispatch: the run and the project every
/// launch carries, and what the launch is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp<'a> {
    /// [`RUN_ID_LABEL`].
    pub run: &'a str,
    /// [`PROJECT_LABEL`], when the launch record names one.
    pub project: Option<&'a str>,
    /// Which launch this is, carrying exactly the keys its scope has.
    pub launched: Launched<'a>,
}

/// What a launch is, and the keys that go with it — one shape per scope, so a
/// stamp cannot name a node on the observer or leave a node's attempt off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launched<'a> {
    /// A node-scope dispatch: [`NODE_LABEL`] and [`ATTEMPT_LABEL`] always, and
    /// [`STEP_LABEL`] on a lifecycle step's dispatch.
    Node {
        /// The node.
        node: &'a str,
        /// The lifecycle step, on a step's dispatch.
        step: Option<&'a str>,
        /// The attempt the dispatch's `node-dispatched` records.
        attempt: NonZeroU32,
    },
    /// The dag-scope observer graph: the run and the project alone.
    Observer,
    /// The drafting graph: the node whose change request it drafts, and that
    /// node's own attempt.
    PrAuthor {
        /// The node.
        node: &'a str,
        /// The node's attempt, which the drafting is part of.
        attempt: NonZeroU32,
    },
}

impl Launched<'_> {
    /// The scope word this launch is stamped under.
    #[must_use]
    pub const fn scope(self) -> Scope {
        match self {
            Launched::Node { .. } => Scope::Node,
            Launched::Observer => Scope::Observer,
            Launched::PrAuthor { .. } => Scope::PrAuthor,
        }
    }
}

impl Stamp<'_> {
    /// The engine's pairs, in key order.
    fn pairs(&self) -> Vec<(&'static str, String)> {
        let mut pairs = vec![
            (RUN_ID_LABEL, self.run.to_string()),
            (SCOPE_LABEL, self.launched.scope().as_str().to_string()),
        ];
        if let Some(project) = self.project {
            pairs.push((PROJECT_LABEL, project.to_string()));
        }
        match self.launched {
            Launched::Node {
                node,
                step,
                attempt,
            } => {
                pairs.push((NODE_LABEL, node.to_string()));
                if let Some(step) = step {
                    pairs.push((STEP_LABEL, step.to_string()));
                }
                pairs.push((ATTEMPT_LABEL, attempt.to_string()));
            }
            Launched::Observer => {}
            Launched::PrAuthor { node, attempt } => {
                pairs.push((NODE_LABEL, node.to_string()));
                pairs.push((ATTEMPT_LABEL, attempt.to_string()));
            }
        }
        pairs
    }
}

/// The [`LABELS_ENV`] value this process's own environment carries, which is
/// what a launch inherits where no dispatch-env hook set one.
///
/// # Errors
///
/// [`Error::Refused`] naming the variable where it is set to something that is
/// not Unicode: a value the merge cannot read is not an absent one, and reading
/// it as absent would drop every label the repository stamped.
pub(crate) fn inherited_labels() -> Result<Option<String>> {
    match std::env::var_os(LABELS_ENV) {
        None => Ok(None),
        Some(value) => value.into_string().map(Some).map_err(|value| {
            Error::Refused(format!(
                "the launch inherits a {LABELS_ENV} that is not Unicode ({value:?}), which \
                 oneharness cannot read as a label set"
            ))
        }),
    }
}

/// The three pairs the engine overlays on a launch, ready for
/// `Launch::env`: history on, the run's pointer file, and the labels.
///
/// `inherited` is the [`LABELS_ENV`] value the launch would otherwise carry —
/// the dispatch-env hook's document where it set one, else the driver's own
/// environment — and it is the repository's: every key of it survives except
/// the ones under [`LABEL_PREFIX`], which are the engine's to say. The pointer
/// file is [`RunPaths::oneharness_sessions`] made absolute, because the
/// dispatch does not run where this process was started.
///
/// # Errors
///
/// [`Error::Refused`] naming the key where a value the label grammar refuses
/// would otherwise refuse every oneharness run under the launch — the way a
/// missing `env_from` source refuses it — and where the inherited value is not
/// a label set at all. [`Error::Ledger`] where the pointer file's path cannot
/// be made absolute.
pub(crate) fn overlay(
    paths: &RunPaths,
    inherited: Option<&str>,
    stamp: &Stamp<'_>,
) -> Result<Vec<(String, String)>> {
    let pointer_file = paths.oneharness_sessions();
    let pointer_file = std::path::absolute(&pointer_file).map_err(|source| Error::Ledger {
        path: pointer_file,
        source,
    })?;
    Ok(vec![
        (HISTORY_ENV.to_string(), "1".to_string()),
        (
            POINTER_FILE_ENV.to_string(),
            pointer_file.display().to_string(),
        ),
        (LABELS_ENV.to_string(), compose_labels(inherited, stamp)?),
    ])
}

/// The [`LABELS_ENV`] value for one dispatch, composed key-wise.
///
/// The pairs already in `inherited`, with every key under [`LABEL_PREFIX`]
/// removed, then the engine's own keys put in — so a stale `onepipeline.node`
/// a nested launch inherited is replaced, and an `owner=ci` the repository
/// stamped stands beside the engine's keys. Rendered in oneharness's wire
/// format, every value held to that library's label grammar through its own
/// [`HistoryLabels::new`], and refused where a value carries a comma, which
/// the wire format cannot carry.
///
/// # Errors
///
/// [`Error::Refused`] naming the key where a value the label grammar refuses
/// would otherwise refuse every oneharness run under the launch — the way a
/// missing `env_from` source does — and where the inherited value is not a
/// label set at all.
pub fn compose_labels(inherited: Option<&str>, stamp: &Stamp<'_>) -> Result<String> {
    let mut labels: BTreeMap<String, String> = match inherited.map(str::trim) {
        Some(value) if !value.is_empty() => parse_labels(value.split(',').map(str::trim))
            .map_err(|error| {
                Error::Refused(format!(
                    "the launch inherits a {LABELS_ENV} that oneharness would refuse: {error}"
                ))
            })?
            .as_map()
            .iter()
            .filter(|(key, _)| !key.starts_with(LABEL_PREFIX))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        _ => BTreeMap::new(),
    };
    for (key, value) in stamp.pairs() {
        labels.insert(key.to_string(), value);
    }
    let labels = HistoryLabels::new(labels).map_err(|error| {
        Error::Refused(format!(
            "the dispatch cannot be stamped with a history label oneharness would refuse: {error}"
        ))
    })?;
    if let Some((key, _)) = labels
        .as_map()
        .iter()
        .find(|(_, value)| value.contains(','))
    {
        return Err(Error::Refused(format!(
            "the dispatch cannot be stamped: history label `{key}` carries a comma, which the \
             {LABELS_ENV} wire format has no escape for"
        )));
    }
    Ok(render_labels(labels.as_map()))
}

/// A label set in oneharness's wire format: `key=value`, comma-separated, in
/// key order.
#[must_use]
pub fn render_labels(labels: &BTreeMap<String, String>) -> String {
    labels
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Which of a run's sessions a read asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentScope<'a> {
    /// Every line of the run's pointer file.
    Run,
    /// The lines whose [`NODE_LABEL`] is this node.
    Node(&'a str),
}

/// The agents a run, a node or a project launched: one entry per oneharness
/// session, read off the run's pointer file and nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Agents {
    /// One entry per session, in the order each first appeared.
    pub sessions: Vec<AgentSession>,
    /// The lines the reader counted and did not read: a torn tail an
    /// interrupted writer left, or a foreign line.
    pub skipped: usize,
}

/// One oneharness session a launch under a run wrote, and where it is.
///
/// `history_dir`, `history_project` and `history_session` are exactly the three
/// fields the `oneharness_session` reference already resolves through — see
/// `oneharness_core::io::history::find_session_path` — so a reader opens this
/// one to its transcript the way it opens that reference kind, through the
/// store the line names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentSession {
    /// The session id — the session file's stem.
    pub history_session: String,
    /// The session's human-meaningful name.
    pub name: String,
    /// The store the session is under, absolute.
    pub history_dir: String,
    /// The project slug — the session file's parent directory name.
    pub history_project: String,
    /// The session file, absolute.
    pub history_file: String,
    /// The project directory the session's runs operated in.
    pub project: String,
    /// When the session's first harness run began: the earliest `started`
    /// among its lines, RFC 3339 UTC.
    pub started: String,
    /// The session's labels: the engine's keys, and whatever the repository
    /// stamped beside them.
    pub labels: BTreeMap<String, String>,
    /// The harness runs the session recorded, in file order.
    pub runs: Vec<AgentRun>,
}

/// One harness run within a session, as its pointer line names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentRun {
    /// The history id the run's record closes with — what
    /// `oneharness history show <id>` resolves.
    pub history_id: String,
    /// The harness id's base, e.g. `claude-code`.
    pub harness: String,
    /// The variant, when the identity names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// The whole configured id, e.g. `claude-code:primary`.
    pub harness_id: String,
    /// When the run began, RFC 3339 UTC.
    pub started: String,
}

impl Agents {
    /// Read one run's pointer file, keeping the lines `scope` asks for.
    ///
    /// A run with no pointer file — an older record, or one that has
    /// dispatched nothing yet — is an empty list rather than an error.
    ///
    /// # Errors
    ///
    /// A pointer file that exists and cannot be read.
    pub fn of_run(paths: &RunPaths, scope: AgentScope<'_>) -> Result<Self> {
        let mut agents = Self::default();
        agents.absorb(&paths.oneharness_sessions(), scope)?;
        Ok(agents)
    }

    /// Fold one run's pointer file into this listing: the lines `scope` keeps,
    /// and every line the reader could not read, whatever the scope — a torn
    /// tail is a fact about the file, not about the node asked for.
    pub(crate) fn absorb(&mut self, pointer_file: &Path, scope: AgentScope<'_>) -> Result<()> {
        let read = read_pointers(pointer_file).map_err(|error| {
            Error::Invalid(format!(
                "cannot read the run's pointer file {}: {error}",
                pointer_file.display()
            ))
        })?;
        self.skipped += read.skipped;
        for pointer in read
            .pointers
            .iter()
            .filter(|pointer| selects(scope, pointer))
        {
            self.record(pointer);
        }
        Ok(())
    }

    fn record(&mut self, pointer: &HistoryPointer) {
        let run = AgentRun {
            history_id: pointer.history_id().to_string(),
            harness: pointer.harness().to_string(),
            variant: pointer.variant().map(str::to_string),
            harness_id: pointer.harness_id().to_string(),
            started: pointer.started().as_str().to_string(),
        };
        if let Some(session) = self
            .sessions
            .iter_mut()
            .find(|session| session.history_session == pointer.history_session())
        {
            if run.started < session.started {
                session.started = run.started.clone();
            }
            session.runs.push(run);
            return;
        }
        self.sessions.push(AgentSession {
            history_session: pointer.history_session().to_string(),
            name: pointer.name().to_string(),
            history_dir: pointer.history_dir().to_string(),
            history_project: pointer.history_project().to_string(),
            history_file: pointer.history_file().to_string(),
            project: pointer.project().to_string(),
            started: run.started.clone(),
            labels: pointer.labels().as_map().clone(),
            runs: vec![run],
        });
    }
}

fn selects(scope: AgentScope<'_>, pointer: &HistoryPointer) -> bool {
    match scope {
        AgentScope::Run => true,
        AgentScope::Node(node) => {
            pointer
                .labels()
                .as_map()
                .get(NODE_LABEL)
                .map(String::as_str)
                == Some(node)
        }
    }
}

/// The text `onepipeline agents` prints: one block per session, each run of
/// it beneath, and the count of lines the reader could not read.
#[must_use]
pub fn render(agents: &Agents) -> String {
    let mut out = String::new();
    if agents.sessions.is_empty() {
        out.push_str("no sessions recorded\n");
    }
    for session in &agents.sessions {
        out.push_str(&format!(
            "{}  {}  started {}\n",
            session.history_session, session.name, session.started
        ));
        out.push_str(&format!(
            "  store {}  project {}\n",
            session.history_dir, session.history_project
        ));
        out.push_str(&format!("  file {}\n", session.history_file));
        out.push_str(&format!("  cwd {}\n", session.project));
        if !session.labels.is_empty() {
            out.push_str(&format!("  labels {}\n", render_labels(&session.labels)));
        }
        for run in &session.runs {
            out.push_str(&format!(
                "  run {}  {}  started {}\n",
                run.history_id, run.harness_id, run.started
            ));
        }
    }
    if agents.skipped > 0 {
        out.push_str(&format!(
            "{} line(s) skipped: torn or not a pointer\n",
            agents.skipped
        ));
    }
    out
}

/// Where a run's pointer file is, for the launch record and the pointer
/// variable: a path a stranger to this crate can compose from the run root.
impl RunPaths {
    /// The run's pointer file, [`SESSIONS_FILE`] under the run root: one line
    /// per harness run every oneharness under this run's launches began, saying
    /// where its session went. Absent for a run that dispatched nothing yet,
    /// and for one an earlier build launched.
    #[must_use]
    pub fn oneharness_sessions(&self) -> PathBuf {
        self.dir.join(SESSIONS_FILE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp<'a>(node: &'a str, step: Option<&'a str>) -> Stamp<'a> {
        Stamp {
            run: "demo-1",
            project: Some("plans:demo"),
            launched: Launched::Node {
                node,
                step,
                attempt: NonZeroU32::MIN,
            },
        }
    }

    /// The merge rule, key by key: the repository's pairs survive, the
    /// engine's prefix is the engine's, and the wire is key-ordered.
    #[test]
    fn inherited_labels_survive_except_under_the_engines_prefix() {
        let composed = compose_labels(
            Some("owner=ci, onepipeline.node=other,onepipeline.scope=observer,team=core"),
            &stamp("build", None),
        )
        .expect("a well-formed inherited set composes");
        assert_eq!(
            composed,
            "onepipeline.attempt=1,onepipeline.node=build,onepipeline.project=plans:demo,\
             onepipeline.run_id=demo-1,onepipeline.scope=node,owner=ci,team=core"
        );
        // Nothing inherited, and a blank, are the engine's keys alone.
        for inherited in [None, Some(""), Some("   ")] {
            let composed = compose_labels(inherited, &stamp("build", None)).expect("composes");
            assert_eq!(
                composed,
                "onepipeline.attempt=1,onepipeline.node=build,onepipeline.project=plans:demo,\
                 onepipeline.run_id=demo-1,onepipeline.scope=node"
            );
        }
        // The keys are present exactly when the launch's shape says.
        let observer = compose_labels(
            None,
            &Stamp {
                run: "demo-1",
                project: None,
                launched: Launched::Observer,
            },
        )
        .expect("composes");
        assert_eq!(
            observer,
            "onepipeline.run_id=demo-1,onepipeline.scope=observer"
        );
        let step = compose_labels(None, &stamp("service", Some("implement"))).expect("composes");
        assert!(step.contains("onepipeline.step=implement"), "{step}");
        let drafting = compose_labels(
            None,
            &Stamp {
                run: "demo-1",
                project: None,
                launched: Launched::PrAuthor {
                    node: "service",
                    attempt: NonZeroU32::MIN.saturating_add(1),
                },
            },
        )
        .expect("composes");
        assert_eq!(
            drafting,
            "onepipeline.attempt=2,onepipeline.node=service,onepipeline.run_id=demo-1,\
             onepipeline.scope=pr-author"
        );
    }

    /// A value the grammar refuses refuses the launch naming the key, and so
    /// does the one thing the wire cannot carry.
    #[test]
    fn a_value_the_grammar_or_the_wire_refuses_refuses_the_launch_naming_the_key() {
        let comma = compose_labels(None, &stamp("a,b", None)).expect_err("a comma");
        assert!(
            comma.to_string().contains(NODE_LABEL) && comma.to_string().contains("comma"),
            "{comma}"
        );
        let control = compose_labels(None, &stamp("a\u{7}b", None)).expect_err("a control");
        assert!(control.to_string().contains(NODE_LABEL), "{control}");
        let long = "x".repeat(257);
        let too_long = compose_labels(None, &stamp(&long, None)).expect_err("too long");
        assert!(too_long.to_string().contains(NODE_LABEL), "{too_long}");
        let malformed =
            compose_labels(Some("nokey"), &stamp("build", None)).expect_err("malformed");
        assert!(malformed.to_string().contains(LABELS_ENV), "{malformed}");
    }

    /// The scope words are the contract's, and each is its own.
    #[test]
    fn every_scope_has_a_word_of_its_own() {
        let words: std::collections::BTreeSet<&str> =
            Scope::ALL.iter().map(|scope| scope.as_str()).collect();
        assert_eq!(words.len(), Scope::ALL.len());
        assert_eq!(Scope::Node.as_str(), "node");
        assert_eq!(Scope::Observer.as_str(), "observer");
        assert_eq!(Scope::PrAuthor.as_str(), "pr-author");
    }

    /// A run with no pointer file is an empty list; a written one groups by
    /// session, keeps file order within it, and counts what it could not read.
    #[test]
    fn a_pointer_file_reads_grouped_by_session_and_a_missing_one_reads_empty() {
        use oneharness_core::domain::harness::HarnessIdentity;
        use oneharness_core::domain::history::{HistoryId, PointerSession};
        use oneharness_core::domain::usage::UtcInstant;

        let root = std::env::temp_dir().join(format!("onepipeline-agents-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = RunPaths::under(&root, "demo-1");
        std::fs::create_dir_all(&paths.dir).expect("a run root");
        assert_eq!(
            Agents::of_run(&paths, AgentScope::Run).expect("no file reads empty"),
            Agents::default()
        );

        let store = if cfg!(windows) { "C:\\store" } else { "/store" };
        let line = |session: &str, node: &str, id: u32, started: i64| {
            let labels = HistoryLabels::new(BTreeMap::from([
                (NODE_LABEL.to_string(), node.to_string()),
                ("owner".to_string(), "ci".to_string()),
            ]))
            .expect("valid labels");
            let session = PointerSession::new(
                Path::new(store),
                &Path::new(store)
                    .join("proj")
                    .join(format!("{session}.jsonl")),
                "turn",
                if cfg!(windows) { "C:\\work" } else { "/work" },
                labels,
            )
            .expect("a session");
            let identity: HarnessIdentity = "claude-code".parse().expect("an identity");
            // A version-4, RFC-variant uuid, which is what the id's parser
            // admits; the last digits tell the runs apart.
            let history_id: HistoryId = format!("00000000-0000-4000-8000-{id:012}")
                .parse()
                .expect("a history id");
            let pointer = HistoryPointer::new(
                &session,
                history_id,
                &identity,
                UtcInstant::from_epoch(started),
            )
            .expect("a pointer");
            format!("{}\n", serde_json::to_string(&pointer).expect("serialises"))
        };
        let mut text = String::new();
        text.push_str(&line("s-one", "build", 1, 200));
        text.push_str("{\"not\": \"a pointer\"}\n");
        text.push_str(&line("s-two", "test", 2, 150));
        text.push_str(&line("s-one", "build", 3, 100));
        text.push_str("{\"torn");
        std::fs::write(paths.oneharness_sessions(), text).expect("the file is written");

        let all = Agents::of_run(&paths, AgentScope::Run).expect("reads");
        assert_eq!(all.skipped, 2, "{all:?}");
        assert_eq!(all.sessions.len(), 2, "{all:?}");
        let one = &all.sessions[0];
        assert_eq!(one.history_session, "s-one");
        assert_eq!(one.history_dir, store);
        assert_eq!(one.history_project, "proj");
        assert_eq!(one.labels["owner"], "ci");
        assert_eq!(
            one.runs
                .iter()
                .map(|run| run.history_id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "00000000-0000-4000-8000-000000000001",
                "00000000-0000-4000-8000-000000000003"
            ]
        );
        // The earliest start among the session's lines, not the first line's.
        assert_eq!(one.started, UtcInstant::from_epoch(100).as_str());
        assert_eq!(one.runs[0].harness_id, "claude-code");

        let build = Agents::of_run(&paths, AgentScope::Node("build")).expect("reads");
        assert_eq!(build.sessions.len(), 1);
        assert_eq!(build.sessions[0].history_session, "s-one");
        let none = Agents::of_run(&paths, AgentScope::Node("nope")).expect("reads");
        assert!(none.sessions.is_empty());
        assert_eq!(
            none.skipped, 2,
            "skipped lines are counted whatever the scope"
        );

        let rendered = render(&all);
        assert!(rendered.starts_with("s-one  turn  started "), "{rendered}");
        assert!(
            rendered.contains("  labels onepipeline.node=build,owner=ci\n"),
            "{rendered}"
        );
        assert!(rendered.contains("2 line(s) skipped"), "{rendered}");
        assert_eq!(render(&Agents::default()), "no sessions recorded\n");
        let _ = std::fs::remove_dir_all(&root);
    }
}
