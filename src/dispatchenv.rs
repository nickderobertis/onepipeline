//! The **dispatch-env hook**: a command a launch names, run immediately before
//! every node-scope dispatch, whose stdout adds environment to that one child
//! launch.
//!
//! `docs/contract.md`'s dispatch-env hook paragraph is the whole rule. What this
//! file adds is where each half of it lives: [`run_and_check`] is the spawn, the
//! bounded wait and the check, called by the executor before anything of a
//! launch begins; [`Document`] reads the hook's stdout in two steps so that no
//! refusal can quote a value it printed; and [`validate`] reads the oneharness
//! configs through the sibling's own parser, resolved against the graph exactly
//! as the sibling resolves them. Nothing here emits an event or writes a
//! settlement: a refusal is an error the executor's caller settles.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::Path;
use std::process::Stdio;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use oneagentgraph::config::{ConfigRef, GraphConfig, JudgeSide, Member};
use oneagentgraph::resolve::{self, Resolver};

use crate::cli::DEFAULT_DISPATCH_ENV_HOOK_TIMEOUT_SECONDS;
use crate::error::{Error, Result};
use crate::hooks::{self, HOOK_ENV, POLL, RUN_ID_ENV, RUN_ROOT_ENV};
use crate::ledger::{LaunchRecord, RunPaths};
use crate::sys;

/// The name this hook is run under: what `ONEPIPELINE_HOOK` says, and the stem
/// of its log under the run's `hooks/`.
pub(crate) const HOOK: &str = "dispatch-env";

/// The environment variable naming the node whose dispatch the hook runs for.
pub(crate) const NODE_ID_ENV: &str = "ONEPIPELINE_NODE_ID";

/// The one version of the document a hook prints.
const DOCUMENT_VERSION: u64 = 1;

/// The two members the document has, and no other.
const DOCUMENT_MEMBERS: [&str; 2] = ["version", "env"];

/// The most of a hook's stdout that is read.
///
/// An environment is small, and a hook that prints more than this is not
/// printing one: the read is bounded so a hook that never stops printing cannot
/// hold the launch — or this process's memory — open, and what it printed past
/// the bound makes the document malformed rather than truncated into one that
/// parses.
const MAX_STDOUT_BYTES: u64 = 1 << 20;

/// How long a hook that has exited is given to close its stdout.
///
/// A hook that left a child holding its stdout has ended without the document
/// being whole; that child is ended with the hook's process tree, so the pipe
/// closes at once in every case but a stranger holding it, which this bounds.
const STDOUT_GRACE: Duration = Duration::from_secs(2);

/// One launch, as the hook is asked about it.
pub(crate) struct Launching<'a> {
    /// The run the dispatch belongs to.
    pub paths: &'a RunPaths,
    /// That run's launch record, which names the hook and its timeout.
    pub record: &'a LaunchRecord,
    /// The node being dispatched.
    pub node: &'a str,
    /// The graph the dispatch launches.
    pub graph: &'a ConfigRef,
    /// The overrides that launch applies to the graph, in order.
    pub sets: &'a [String],
}

/// Run the hook the launch names — spawned in the launch directory, its stderr
/// kept in the run's log, awaited for up to its timeout and its process tree
/// ended past it — read the document it printed, and check every `env_from`
/// source the launch's configs name against the environment refreshed with it;
/// answer what the hook added. A launch naming no hook runs nothing, reads
/// nothing and adds nothing.
///
/// # Errors
///
/// [`Error::Refused`] naming the node and either how the hook ended — its exit
/// code, `timeout`, `could-not-start` or `malformed` with what was malformed —
/// or the config file, the harness variant and the `env_from` source that is
/// still missing. Nothing has been dispatched when it does.
pub(crate) fn run_and_check(launching: &Launching<'_>) -> Result<Vec<(String, String)>> {
    let Some(command) = launching.record.dispatch_env_hook() else {
        return Ok(Vec::new());
    };
    let node = launching.node;
    let added = run(launching, command).map_err(|ending| {
        Error::Refused(format!(
            "the {HOOK} hook '{command}' refused the launch of node '{node}': {ending}; its \
             stderr is kept in {}",
            hooks::hook_log(launching.paths, HOOK).display()
        ))
    })?;
    // The environment the child would be launched with: this process's own, with
    // the hook's members over it. This crate's own per-dispatch keys are set
    // beside both by the executor and name no `env_from` source.
    let mut refreshed = process_env();
    refreshed.extend(added.iter().cloned());
    validate(launching.graph, launching.sets, &refreshed).map_err(|why| {
        Error::Refused(format!(
            "the launch of node '{node}' was refused after the {HOOK} hook '{command}' ran: \
             {why}"
        ))
    })?;
    Ok(added)
}

/// This process's environment, as string pairs.
///
/// A name or a value this host cannot spell as text is left out: the check
/// below asks whether a *named* variable is present, and one this crate could
/// not name is one no config could name either.
fn process_env() -> BTreeMap<String, String> {
    std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect()
}

/// Spawn the hook, hand it nothing on stdin, keep its stderr, read its stdout,
/// wait for it — for up to the launch's timeout, and then end its process tree
/// — and read the document it printed.
///
/// The error is the ending, in the words the refusal names it by.
fn run(
    launching: &Launching<'_>,
    command: &str,
) -> std::result::Result<Vec<(String, String)>, String> {
    let paths = launching.paths;
    let record = launching.record;
    let log = hooks::hook_log(paths, HOOK);
    let opened = log
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log)
        });
    let mut stderr = match opened {
        Ok(stderr) => stderr,
        Err(error) => {
            return Err(format!(
                "could-not-start: its log {} could not be opened: {error}",
                log.display()
            ))
        }
    };
    // One line naming the node ahead of what the hook says, so a log that gathers
    // every dispatch's stderr can be read per dispatch. Names only: no value the
    // hook prints reaches this file.
    let _ = writeln!(
        stderr,
        "onepipeline: {HOOK} hook for node '{}'",
        launching.node
    );

    let mut spawning = std::process::Command::new(command);
    // The launch directory, exactly as a run-end hook is started in it.
    if !record.dir.as_os_str().is_empty() {
        spawning.current_dir(&record.dir);
    }
    let stderr = match stderr.try_clone() {
        Ok(stderr) => stderr,
        Err(error) => return Err(format!("could-not-start: {error}")),
    };
    spawning
        .env(HOOK_ENV, HOOK)
        .env(RUN_ID_ENV, &paths.run)
        .env(RUN_ROOT_ENV, hooks::run_root(paths))
        .env(NODE_ID_ENV, launching.node)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(stderr);
    let mut child = spawning
        .spawn()
        .map_err(|error| format!("could-not-start: {error}"))?;

    // Read on a thread of its own, so a hook that prints more than a pipe holds
    // cannot deadlock against a wait that only looks at its exit — and bounded,
    // so a hook that never stops printing cannot hold this process's memory.
    let (printed_tx, printed_rx) = mpsc::channel();
    let mut stdout = child.stdout.take();
    std::thread::spawn(move || {
        let mut printed = Vec::new();
        let read = match stdout.as_mut() {
            Some(stdout) => stdout
                .take(MAX_STDOUT_BYTES + 1)
                .read_to_end(&mut printed)
                .map(|_| ()),
            None => Ok(()),
        };
        let _ = printed_tx.send(read.map(|()| printed));
    });

    let timeout = record.dispatch_env_hook_timeout();
    let deadline = hooks::deadline_after(timeout);
    let ended = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if deadline.is_none_or(|deadline| Instant::now() < deadline) => {}
            // Past the timeout — or a child this process can no longer ask about,
            // which is ended the same way rather than left running unrecorded.
            waited => {
                let _ = sys::stop(child.id(), sys::Stop::Now);
                let _ = child.kill();
                let _ = child.wait();
                break Err(match waited {
                    Ok(_) => format!(
                        "timeout: still running after {timeout} seconds, so its process tree \
                         was ended"
                    ),
                    Err(error) => format!("it could not be waited for: {error}"),
                });
            }
        }
        std::thread::sleep(POLL);
    };
    let status = ended?;
    if !status.success() {
        return Err(match status.code() {
            Some(code) => format!("exit {code}"),
            None => "it ended with no exit code".to_string(),
        });
    }
    let printed = match printed_rx.recv_timeout(STDOUT_GRACE) {
        Ok(Ok(printed)) => printed,
        Ok(Err(error)) => return Err(format!("its stdout could not be read: {error}")),
        Err(_) => {
            return Err(
                "malformed: its stdout was still held open after it exited, so what it \
                 printed is not one whole document"
                    .to_string(),
            )
        }
    };
    if printed.len() as u64 > MAX_STDOUT_BYTES {
        return Err(format!(
            "malformed: it printed more than {MAX_STDOUT_BYTES} bytes, which is not one \
             environment document"
        ));
    }
    Document::read(&printed).map_err(|what| format!("malformed: {what}"))
}

/// The one document a hook prints: `{"version": 1, "env": {NAME: value, ...}}`.
///
/// Read in two steps rather than through a typed deserialization, because what a
/// refusal may say about it is bounded: a typed error quotes the value it could
/// not read, and a value the hook printed is one no record of this run may
/// carry. So the document is read as JSON, and each way it is malformed is named
/// by the member — never the value — that made it so.
struct Document;

impl Document {
    // llmlint: ignore[invalid_states_unrepresentable] the pairs answered here go straight
    // into the `env: &[(String, String)]` the sibling seam's `Launch` already types as
    // strings, one line after this returns in the executor; every name has passed
    // oneharness's own `valid_env_name` and every value is NUL-free by the time it is
    // pushed, and a name newtype would be unwrapped at that next line for nothing.
    fn read(printed: &[u8]) -> std::result::Result<Vec<(String, String)>, String> {
        let document: serde_json::Value = serde_json::from_slice(printed).map_err(|error| {
            // serde_json's syntax errors name a line and a column and never the
            // text, so this quotes nothing the hook printed.
            format!("its stdout is not one JSON document: {error}")
        })?;
        let Some(members) = document.as_object() else {
            return Err("its document is not a JSON object".to_string());
        };
        for member in members.keys() {
            if !DOCUMENT_MEMBERS.contains(&member.as_str()) {
                return Err(format!(
                    "its document carries a top-level member `{member}`, and the only members \
                     are `version` and `env`"
                ));
            }
        }
        match members.get("version").and_then(serde_json::Value::as_u64) {
            Some(DOCUMENT_VERSION) => {}
            Some(other) => {
                return Err(format!(
                    "its document is version {other}, and the only version is {DOCUMENT_VERSION}"
                ))
            }
            None => {
                return Err(format!(
                    "its document names no `version`, which is {DOCUMENT_VERSION}"
                ))
            }
        }
        let Some(env) = members.get("env").and_then(serde_json::Value::as_object) else {
            return Err("its document's `env` is not an object of names to values".to_string());
        };
        let mut added = Vec::with_capacity(env.len());
        for (name, value) in env {
            if !oneharness_core::domain::config::valid_env_name(name) {
                return Err(format!(
                    "its document's `env` names `{name}`, which is not a valid environment \
                     variable name"
                ));
            }
            let Some(value) = value.as_str() else {
                return Err(format!("its document's `env.{name}` is not a string"));
            };
            if value.contains('\0') {
                return Err(format!(
                    "its document's `env.{name}` holds a NUL, which no environment can carry"
                ));
            }
            added.push((name.clone(), value.to_string()));
        }
        Ok(added)
    }
}

/// One `env_from` source a launch's configs name, and where.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct EnvFromSource {
    /// The oneharness config file, as the launch reads it.
    file: String,
    /// The harness variant naming it, composed as oneharness names one.
    variant: String,
    /// The environment variable the variant sources a value from.
    source: String,
}

/// Check that every `env_from` source the launch's oneharness configs name is
/// in `refreshed`, reading each config **as the file is on disk now**.
///
/// The configs are the ones the launch is about to read and no others: the
/// graph document, with the launch's overrides applied through the sibling's
/// own override path, names each member's `oneharness_config` — a single-sided
/// member's own, a two-party member's agent side and each harness judge side —
/// and each is resolved against the graph's directory exactly as the sibling
/// resolves it.
fn validate(
    graph: &ConfigRef,
    sets: &[String],
    refreshed: &BTreeMap<String, String>,
) -> std::result::Result<(), String> {
    let missing: Vec<String> = env_from_sources(graph, sets)?
        .into_iter()
        .filter(|named| !refreshed.contains_key(&named.source))
        .map(|named| {
            format!(
                "{} names harness variant '{}' whose env_from source '{}' is not in the \
                 environment this dispatch would be launched with",
                named.file, named.variant, named.source
            )
        })
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing.join("; "))
    }
}

/// Every `env_from` source the launch's configs name, each once, in a stable
/// order.
// llmlint: ignore-block[code_lands_in_the_domain_that_owns_it] the harness model is
// consumed, never restated: each config is read through `oneharness_core`'s own parser
// into its own typed `FileConfig`, and what is walked below is that public type's
// `harness` → `variant` → `env_from` fields — the same three the sibling's identity
// resolver walks. oneharness publishes no query for "every env_from source a config
// names", so the walk lives with the one caller that asks it; the day it publishes one,
// this becomes that call, and a field the sibling renames fails here at compile time
// rather than in a string.
fn env_from_sources(
    graph: &ConfigRef,
    sets: &[String],
) -> std::result::Result<BTreeSet<EnvFromSource>, String> {
    let mut resolver = Resolver::new();
    let document = resolver
        .resolve(graph, None)
        .map_err(|error| format!("the graph '{}' could not be read: {error}", graph.0))?
        .clone();
    let graph_dir = document.base_dir.clone();
    let config = graph_with_overrides(&document.content, &graph.0, sets)?;
    let mut refs: Vec<&ConfigRef> = Vec::new();
    for member in config.members.values() {
        match member {
            Member::Oneharness(member) => refs.push(&member.oneharness_config),
            Member::Onejudge(member) => {
                refs.push(&member.agent.oneharness_config);
                for side in &member.judge {
                    if let JudgeSide::Harness(harness) = side {
                        refs.push(&harness.oneharness_config);
                    }
                }
            }
        }
    }
    let mut named = BTreeSet::new();
    for reference in refs {
        let file = config_file(reference, graph_dir.as_deref());
        let resolved = resolver
            .resolve(reference, graph_dir.as_deref())
            .map_err(|error| format!("{file} could not be read: {error}"))?;
        let config = oneharness_core::domain::config::parse(&resolved.content)
            .map_err(|error| format!("{file} is not an oneharness config: {error}"))?;
        for (id, harness) in &config.harness {
            for (name, variant) in &harness.variant {
                for source in variant.env_from.values() {
                    named.insert(EnvFromSource {
                        file: file.clone(),
                        variant: format!("{id}:{name}"),
                        source: source.clone(),
                    });
                }
            }
        }
    }
    Ok(named)
} // llmlint: ignore-end[code_lands_in_the_domain_that_owns_it]

/// The graph document, with the launch's overrides applied the way the sibling
/// applies them, as the graph the sibling will run.
fn graph_with_overrides(
    content: &str,
    origin: &str,
    sets: &[String],
) -> std::result::Result<GraphConfig, String> {
    let mut parsed: serde_json::Value = serde_norway::from_str(content)
        .map_err(|error| format!("the graph '{origin}' could not be read: {error}"))?;
    let overrides = sets
        .iter()
        .map(|value| oneagentgraph::run::parse_set(value).map_err(|error| error.to_string()))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    oneagentgraph::run::apply_overrides(&mut parsed, &overrides)
        .map_err(|error| error.to_string())?;
    serde_norway::to_value(&parsed)
        .and_then(serde_norway::from_value)
        .map_err(|error| format!("the graph '{origin}' could not be read: {error}"))
}

/// How a refusal names one config file: the path the launch reads, or the URL
/// where the ref is remote.
fn config_file(reference: &ConfigRef, graph_dir: Option<&Path>) -> String {
    if resolve::is_remote(reference) {
        reference.0.clone()
    } else {
        resolve::local_path(reference, graph_dir)
            .display()
            .to_string()
    }
}

/// The refusal for a dispatch-env hook timeout of zero, by the spelling that
/// carried it — one sentence for the flag and the launch-config key, as the
/// run-end hooks' timeout has one.
pub(crate) fn refused_zero_timeout(spelling: &str) -> String {
    format!(
        "{spelling} names a {HOOK} hook timeout of zero seconds, which ends the hook before it \
         has begun — give it a positive whole number of seconds, or leave it out to take \
         {DEFAULT_DISPATCH_ENV_HOOK_TIMEOUT_SECONDS} seconds"
    )
}

/// The path a launch's graph is read from, for the tests below.
#[cfg(test)]
fn graph_at(path: &Path) -> ConfigRef {
    ConfigRef(path.display().to_string())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "onepipeline-dispatchenv-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a scratch root");
        root
    }

    /// Every way the document is malformed is named by the member that made it
    /// so, and never by a value the hook printed.
    #[test]
    fn a_malformed_document_is_named_by_its_member_and_never_by_a_value() {
        let sentinel = "hunter2-sentinel";
        for (printed, names) in [
            (
                r#"{"version": 1, "env": {"A": "b"}, "extra": 1}"#,
                "`extra`",
            ),
            (
                &format!(r#"{{"version": 1, "env": {{"A": 5, "B": "{sentinel}"}}}}"#),
                "`env.A` is not a string",
            ),
            (r#"{"version": 2, "env": {}}"#, "version 2"),
            (r#"{"env": {}}"#, "no `version`"),
            (r#"{"version": 1}"#, "`env` is not an object"),
            (r#"{"version": 1, "env": []}"#, "`env` is not an object"),
            (r#"{"version": 1, "env": {"1BAD": "x"}}"#, "`1BAD`"),
            (r#"{"version": 1, "env": {"A=B": "x"}}"#, "`A=B`"),
            (r#"[1]"#, "not a JSON object"),
            (
                &format!(
                    r#"{{"version": 1, "env": {{}}}} {{"version": 1, "env": {{"S": "{sentinel}"}}}}"#
                ),
                "not one JSON document",
            ),
            (&format!("not json {sentinel}"), "not one JSON document"),
            ("", "not one JSON document"),
        ] {
            let what = Document::read(printed.as_bytes())
                .expect_err(&format!("{printed} was read as a document"));
            assert!(what.contains(names), "{printed}: {what}");
            assert!(
                !what.contains(sentinel),
                "the refusal of {printed} quoted a value: {what}"
            );
        }
        let nul = format!(r#"{{"version": 1, "env": {{"A": "x\u0000{sentinel}"}}}}"#);
        let what = Document::read(nul.as_bytes()).expect_err("a NUL was accepted");
        assert!(what.contains("`env.A`") && what.contains("NUL"), "{what}");
        assert!(!what.contains(sentinel), "{what}");

        // And the document itself reads, whole, as the hook printed it, with an
        // empty `env` meaning nothing added.
        assert_eq!(
            Document::read(br#"{"version": 1, "env": {"B": "2", "A": "1"}}"#).expect("it reads"),
            vec![
                ("B".to_string(), "2".to_string()),
                ("A".to_string(), "1".to_string())
            ]
        );
        assert_eq!(
            Document::read(b"{\"version\": 1, \"env\": {}}\n").expect("it reads"),
            Vec::new()
        );
    }

    /// The sources a launch's configs name are read off the graph **after** its
    /// overrides, from every side that names a config, and each is held against
    /// the refreshed environment by file, variant and name.
    #[test]
    fn every_env_from_source_the_launch_would_read_is_checked_after_the_overrides() {
        let root = scratch("sources");
        let write = |name: &str, text: &str| {
            std::fs::write(root.join(name), text).expect("the fixture is written");
        };
        write(
            "worker.toml",
            "run_mode = \"fallback\"\nharnesses = [\"claude-code:work\"]\n\
             [harness.claude-code.variant.work.env_from]\nCLAUDE_CONFIG_DIR = \"HOST_CLAUDE_HOME\"\n",
        );
        write(
            "judge.toml",
            "harnesses = [\"codex:review\"]\n[harness.codex.variant.review.env_from]\n\
             CODEX_HOME = \"HOST_CODEX_HOME\"\nOPENAI_API_KEY = \"HOST_OPENAI_KEY\"\n",
        );
        write(
            "other.toml",
            "harnesses = [\"claude-code\"]\n[harness.claude-code.variant.alt.env_from]\n\
             CLAUDE_CONFIG_DIR = \"HOST_ALT_HOME\"\n",
        );
        write("base.yaml", "system_prompt: Do the work.\n");
        write(
            "graph.yaml",
            "version: 1\nname: node-scope\nmembers:\n  worker:\n    kind: onejudge\n    \
             base_config: ./base.yaml\n    agent:\n      oneharness_config: ./worker.toml\n    \
             judge:\n      - oneharness_config: ./judge.toml\n      - kind: llmlint\n    \
             mode: bypass\n  reporter:\n    kind: oneharness\n    oneharness_config: ./worker.toml\n",
        );
        let graph = graph_at(&root.join("graph.yaml"));
        let file = |name: &str| root.join(name).display().to_string();

        let named = env_from_sources(&graph, &[]).expect("the sources read");
        let seen: Vec<(String, String, String)> = named
            .iter()
            .map(|named| {
                (
                    named.file.clone(),
                    named.variant.clone(),
                    named.source.clone(),
                )
            })
            .collect();
        assert_eq!(
            seen,
            vec![
                (
                    file("judge.toml"),
                    "codex:review".into(),
                    "HOST_CODEX_HOME".into()
                ),
                (
                    file("judge.toml"),
                    "codex:review".into(),
                    "HOST_OPENAI_KEY".into()
                ),
                (
                    file("worker.toml"),
                    "claude-code:work".into(),
                    "HOST_CLAUDE_HOME".into()
                ),
            ]
        );

        // An override that moves a side's config moves what is read.
        let overridden = env_from_sources(
            &graph,
            &["members.worker.agent.oneharness_config=./other.toml".to_string()],
        )
        .expect("the overridden sources read");
        assert!(
            overridden
                .iter()
                .any(|named| named.source == "HOST_ALT_HOME"),
            "{overridden:?}"
        );

        // Held against the environment: a source present passes, and each one
        // absent is named by file, variant and source.
        let mut refreshed = BTreeMap::new();
        for present in ["HOST_CODEX_HOME", "HOST_OPENAI_KEY", "HOST_CLAUDE_HOME"] {
            refreshed.insert(present.to_string(), "set".to_string());
        }
        validate(&graph, &[], &refreshed).expect("every source is present");
        refreshed.remove("HOST_CLAUDE_HOME");
        refreshed.remove("HOST_OPENAI_KEY");
        let why = validate(&graph, &[], &refreshed).expect_err("two sources are missing");
        assert!(
            why.contains(&format!(
                "{} names harness variant 'claude-code:work' whose env_from source \
                 'HOST_CLAUDE_HOME' is not in the environment",
                file("worker.toml")
            )),
            "{why}"
        );
        assert!(
            why.contains(&format!(
                "{} names harness variant 'codex:review' whose env_from source \
                 'HOST_OPENAI_KEY'",
                file("judge.toml")
            )),
            "{why}"
        );
        assert!(!why.contains("HOST_CODEX_HOME"), "{why}");

        // A config the launch would read and cannot is named, as is a graph that
        // names one this build cannot read as oneharness's.
        write("graph-absent.yaml", "version: 1\nname: g\nmembers:\n  worker:\n    kind: oneharness\n    oneharness_config: ./nowhere.toml\n");
        let why = validate(&graph_at(&root.join("graph-absent.yaml")), &[], &refreshed)
            .expect_err("an absent config is refused");
        assert!(
            why.contains(&file("nowhere.toml")) && why.contains("could not be read"),
            "{why}"
        );
        write("bad.toml", "harnesses = [\"no-such-harness\"]\n");
        write("graph-bad.yaml", "version: 1\nname: g\nmembers:\n  worker:\n    kind: oneharness\n    oneharness_config: ./bad.toml\n");
        let why = validate(&graph_at(&root.join("graph-bad.yaml")), &[], &refreshed)
            .expect_err("a config oneharness refuses is refused");
        assert!(
            why.contains(&file("bad.toml")) && why.contains("not an oneharness config"),
            "{why}"
        );
        let why = validate(&graph_at(&root.join("nowhere.yaml")), &[], &refreshed)
            .expect_err("an absent graph is refused");
        assert!(
            why.contains("nowhere.yaml") && why.contains("could not be read"),
            "{why}"
        );
        let why = validate(&graph, &["members.nobody.model=x".to_string()], &refreshed)
            .expect_err("an override naming nothing is refused");
        assert!(why.contains("nobody"), "{why}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A record naming no hook adds nothing and reads nothing: the graph it would
    /// have checked need not even exist.
    #[test]
    fn a_launch_naming_no_hook_adds_nothing_and_reads_no_graph() {
        let root = scratch("unhooked");
        let paths = RunPaths::under(&root, "demo");
        let record: LaunchRecord = serde_json::from_value(serde_json::json!({
            "run_id": "demo",
            "project": "plans:demo",
            "dir": root.display().to_string(),
        }))
        .expect("a record naming no hook, as an earlier build wrote one");
        assert_eq!(record.dispatch_env_hook(), None);
        let added = run_and_check(&Launching {
            paths: &paths,
            record: &record,
            node: "build",
            graph: &graph_at(&root.join("no-such-graph.yaml")),
            sets: &[],
        })
        .expect("a launch naming no hook is not refused");
        assert!(added.is_empty());
        assert!(!hooks::hook_log(&paths, HOOK).exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
