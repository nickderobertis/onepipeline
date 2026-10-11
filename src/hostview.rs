//! `onepipeline host`, read from the cheapest record that decides each run, so
//! the cost of a read follows the work that is live rather than every run the
//! root has ever kept.
//!
//! A read that opened every run — its whole journal, merge-sorted, then the
//! checkpoint's seal over the covered bytes and the fold — and only then asked
//! which nodes were running cost minutes on a host keeping a few hundred ended
//! runs. This reading asks that first, of the documents the engine already keeps
//! beside each journal:
//!
//! 1. **`summary.json`, current by its own stamp** (the journal's length and
//!    modification time, which a `stat` reads without opening the journal). A
//!    current summary counting no `running` node is a run with no row: nothing
//!    else of it is read. That is every settled run.
//! 2. **The registry and the launch record**, for a run that counts a running
//!    node: every registry entry is proved by its pid and start token, and the
//!    driver by the launch record's. Any entry that is live (held, or a live pid
//!    this host cannot decide), or a driver not proved gone, makes the run
//!    **live**, and a live run is folded as every reader folds it — resumed from
//!    its checkpoint, the seal re-taken, the tail folded on — so every live
//!    dispatch is still proved by its own entry. A dead driver alone proves
//!    nothing: a dispatch can outlive it.
//! 3. **`checkpoint.json` covering the whole journal**, for a run proved ended:
//!    its state names the running nodes, when each was dispatched, and the stop
//!    the fold recorded. Trusted where it covers exactly the bytes and records the
//!    current summary stamps, which is the summary's own freshness rule; no ended
//!    journal is opened.
//! 4. **Cold**: a run whose summary is absent, stale or at another version, or an
//!    ended one whose checkpoint does not cover the whole journal, is folded once
//!    from its journal and both documents are left current — an ended run's
//!    checkpoint covering every record (see [`crate::checkpoint::fold_ended`]) —
//!    so the next read is steady.
//!
//! **No journal past [`MAX_COLD_JOURNAL_BYTES`] is opened, in any state.** Such a
//! run is decided from its writer-maintained summary, its checkpoint and its
//! registry where they suffice, and is otherwise refused into
//! [`HostReading::skipped`] with the reason: flagged, never hidden.
//!
//! What a row means does not change: a row is rendered as live only where its
//! registry entry's pid and start token hold, and one this host cannot decide is
//! rendered saying so. What changes is that an ended run's dispatches are
//! **reported gone** (with the reason) rather than folded to discover it.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::Serialize;

use crate::graph::NodeStatus;
use crate::hostphases::phase;
use crate::ledger::{self, LaunchRecord, RunPaths};
use crate::projection::{RunState, StopState};
use crate::summary::RunSummary;
use crate::sys;
use crate::views::{one_line, Proof, Registry};

/// The version of [`HostReading`], the document `onepipeline host --json` prints.
pub const HOST_READING_SCHEMA_VERSION: u32 = 1;

/// The largest journal `onepipeline host` will open, in any state.
///
/// Above every journal the host this was measured on keeps but one: its p99 is
/// 28.7 MiB and its largest live one 0.5 GiB, while a 3.6 GiB journal of a long
/// ended run is never opened at all. A cold read folds about three times the bytes
/// it opens, so this bounds what one read can cost however a root was left.
pub const MAX_COLD_JOURNAL_BYTES: u64 = 1 << 30;

/// `onepipeline host --json`: every live dispatch on this host, as one document.
///
/// The text form is this document rendered — [`render`] — so the two cannot
/// disagree. `schemas/host-reading.schema.json` is generated from this type.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct HostReading {
    /// Always [`HOST_READING_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// This host's name, as the text form's header prints it.
    pub host: String,
    /// The runs root that was read.
    pub root: PathBuf,
    /// Why the runs root itself could not be read; `null` when it was read.
    pub root_error: Option<String>,
    /// One per `free space:` line the text form prints.
    pub free_space: Vec<crate::freespace::FreeSpace>,
    /// One per dispatch row the text form prints, in the same order.
    pub dispatches: Vec<DispatchRow>,
    /// Every dispatch the view proved gone and so left out of `dispatches`.
    pub gone: Vec<Gone>,
    /// One per run root the view refused. The runs root itself is never here.
    pub skipped: Vec<SkippedRoot>,
}

/// One dispatch row.
// llmlint: ignore-block[invalid_states_unrepresentable] `proof` and `unproven` are two keys
// because version 1 of the reading the plan fixed states them as two, and this type is what
// `schemas/host-reading.schema.json` is generated from, so its shape is the document's. The
// one place a row is built, `push`, derives both from a single `Proof`, so a held row with a
// reason or an unproven one without cannot be made by this crate.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DispatchRow {
    /// The run.
    pub run: String,
    /// The node.
    pub node: String,
    /// Who launched the run.
    pub launcher: String,
    /// How long ago the node was dispatched, or `null` when no dispatch time is recorded.
    pub age_ms: Option<u64>,
    /// Whether this host proved the dispatch's process is still the one recorded.
    pub proof: ProofWord,
    /// Why it could not, non-null exactly when `proof` is `unproven`.
    pub unproven: Option<String>,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// What a row's proof established.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ProofWord {
    /// The registry entry's pid and start token hold.
    Held,
    /// This host cannot decide.
    Unproven,
}

/// A dispatch proved gone.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Gone {
    /// The run.
    pub run: String,
    /// The node.
    pub node: String,
    /// What proved it gone.
    pub reason: String,
    /// Which text line names it. Not part of the document: the text form keeps
    /// stale registry entries on their own line, apart from stopped runs.
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) by: GoneBy,
}

/// What proved a dispatch gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum GoneBy {
    /// Its registry entry names a process that is gone or reissued.
    #[default]
    StaleEntry,
    /// Its registry-less run recorded a stop that signalled its workers, with no
    /// adoption since.
    Stopped,
}

/// A run root the view refused.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SkippedRoot {
    /// The run root.
    pub path: PathBuf,
    /// Why.
    pub reason: String,
}

/// Read every run under `root`.
pub(crate) fn read(root: &Path) -> HostReading {
    let index = {
        let _list = phase("list");
        ledger::all_runs(root)
    };
    let mut root_error = None;
    let mut skipped: Vec<SkippedRoot> = Vec::new();
    for refused in index.skipped {
        if refused.path == root {
            root_error = Some(refused.reason);
        } else {
            skipped.push(SkippedRoot {
                path: refused.path,
                reason: refused.reason,
            });
        }
    }
    let free_space = {
        let _freespace = phase("free-space");
        crate::freespace::readings(root)
    };
    let mut dispatches = Vec::new();
    let mut gone = Vec::new();
    for paths in index.runs {
        match read_run(&paths) {
            Ok(run) => {
                dispatches.extend(run.rows);
                gone.extend(run.gone);
            }
            Err(reason) => skipped.push(SkippedRoot {
                path: paths.dir,
                reason,
            }),
        }
    }
    skipped.sort_by(|a, b| a.path.cmp(&b.path));
    HostReading {
        schema_version: HOST_READING_SCHEMA_VERSION,
        host: sys::hostname(),
        root: root.to_path_buf(),
        root_error,
        free_space,
        dispatches,
        gone,
        skipped,
    }
}

#[derive(Default)]
struct RunRead {
    rows: Vec<DispatchRow>,
    gone: Vec<Gone>,
}

/// One run, from the cheapest record that decides it.
fn read_run(paths: &RunPaths) -> Result<RunRead, String> {
    let (launch, current, stamp) = {
        let _documents = phase("launch-and-summary");
        let launch: LaunchRecord =
            ledger::read_json(&paths.launch()).map_err(|error| error.to_string())?;
        let (current, stamp) = current_summary(paths);
        (launch, current, stamp)
    };
    let journal_bytes = stamp.0;
    let past_the_bound = journal_bytes > MAX_COLD_JOURNAL_BYTES;
    // Cold: the summary is absent, stale or at another version. It is refreshed
    // first — one bounded fold, written where the engine's readers write it — so
    // whatever this run turns out to be, the next read finds it current.
    let summary = match current {
        Some(summary) => summary,
        None if past_the_bound => {
            return from_the_registry(paths, &launch, &registry_of(paths), journal_bytes)
        }
        None => {
            foldable(paths)?;
            let _cold = phase("cold-fold");
            crate::hostphases::add("cold-journal-bytes", journal_bytes);
            RunSummary::of(paths).map_err(|error| error.to_string())?
        }
    };
    if !summary
        .node_counts
        .contains_key(NodeStatus::Running.as_str())
    {
        return Ok(RunRead::default());
    }
    let registry = registry_of(paths);
    if is_live(&launch, &registry) {
        if past_the_bound {
            return from_the_registry(paths, &launch, &registry, journal_bytes);
        }
        return live(paths, &launch, &registry);
    }
    // Ended: every process the run recorded is proved over. Its verdict comes off
    // the checkpoint where that covers the whole journal the summary stamps, and
    // otherwise from one fold that leaves the checkpoint covering it.
    let covered = {
        let _decide = phase("decide-ended");
        crate::checkpoint::covering_whole(paths, summary.journal_len, summary.event_count)
    };
    let state = match covered {
        Some(state) => state,
        None if past_the_bound => {
            return from_the_registry(paths, &launch, &registry, journal_bytes)
        }
        None => {
            foldable(paths)?;
            let state = {
                let _cold = phase("cold-fold");
                crate::hostphases::add("cold-journal-bytes", journal_bytes);
                crate::checkpoint::fold_ended(paths)
            };
            // The fold read the journal as it stands now, which a driver adopting
            // the run may have grown since the proof above. Asked again after it, so
            // an adoption that raced this read is read as the live run it is.
            let launch = ledger::read_json::<LaunchRecord>(&paths.launch()).unwrap_or(launch);
            let registry = registry_of(paths);
            if is_live(&launch, &registry) {
                return Ok(rows_of(
                    paths,
                    &launch,
                    &registry,
                    &state,
                    Stops::NotEvidence,
                ));
            }
            return Ok(rows_of(paths, &launch, &registry, &state, Stops::Evidence));
        }
    };
    Ok(rows_of(paths, &launch, &registry, &state, Stops::Evidence))
}

fn registry_of(paths: &RunPaths) -> Registry {
    let _registry = phase("registry");
    Registry::of(paths)
}

/// Whether the run may still have work in flight: an entry that is not proved
/// stale, or a driver not proved over.
fn is_live(launch: &LaunchRecord, registry: &Registry) -> bool {
    let _proofs = phase("pid-proofs");
    registry.has_live_entry() || !driver_proved_gone(launch)
}

/// The run's summary where it is current for the journal as it stands now, and
/// that stamp.
fn current_summary(paths: &RunPaths) -> (Option<RunSummary>, (u64, u64)) {
    let stamp = crate::summary::journal_stamp(paths);
    let summary = ledger::read_json_opt::<RunSummary>(&paths.summary()).filter(|summary| {
        summary.run_id == paths.run && (summary.journal_len, summary.journal_mtime_ms) == stamp
    });
    (summary, stamp)
}

/// Whether the launch record's driver is proved over on this host.
fn driver_proved_gone(launch: &LaunchRecord) -> bool {
    launch.host == sys::hostname() && sys::claim_on(launch.pid, &launch.started).is_over()
}

/// A live run: its fold, resumed from its checkpoint and sealed as every reader
/// seals it, with each running node proved by its own registry entry.
///
/// Not [`crate::views::RunView::open`], which also parses the whole merged store
/// into memory for a caller that renders events: the rows need only the fold.
fn live(paths: &RunPaths, launch: &LaunchRecord, registry: &Registry) -> Result<RunRead, String> {
    foldable(paths)?;
    let state = {
        let _fold = phase("live-fold");
        crate::hostphases::add("live-journal-bytes", crate::summary::journal_stamp(paths).0);
        crate::checkpoint::fold_and_checkpoint(paths)
    };
    Ok(rows_of(paths, launch, registry, &state, Stops::NotEvidence))
}

/// Refuse a run whose journal this read has to fold and cannot open.
///
/// A fold of a journal that cannot be read is a fold of nothing, and a run folded
/// to nothing records nothing running — its dispatches would be left out rather
/// than flagged. A run with no journal yet is a real state, and is folded as one.
fn foldable(paths: &RunPaths) -> Result<(), String> {
    match std::fs::File::open(paths.journal()) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(format!(
            "its journal cannot be read, and this read has to fold it to say what the run \
             has running: {error}"
        )),
        _ => Ok(()),
    }
}

/// Whether a recorded stop may stand in for a registry that cannot be read.
#[derive(PartialEq)]
enum Stops {
    /// Only for a run proved ended: its driver gone and no live entry.
    Evidence,
    NotEvidence,
}

fn rows_of(
    paths: &RunPaths,
    launch: &LaunchRecord,
    registry: &Registry,
    state: &RunState,
    stops: Stops,
) -> RunRead {
    let _proofs = phase("pid-proofs");
    let mut read = RunRead::default();
    for (node, status) in &state.statuses() {
        if *status != NodeStatus::Running {
            continue;
        }
        let proof = match registry {
            // The manager's ruling: a registry-less run whose last stop signalled
            // its workers — the fold's `workers-signalled`, which only a `signalled`
            // teardown (or none recorded, as engines before the field wrote) folds
            // to, and which a later `driver-adopted` clears — has its dispatches
            // counted gone. That is weaker evidence than a watched teardown: an
            // engine before v0.7.2 sent its signal and did not wait. The reason
            // says what was recorded, and no more.
            Registry::Unreadable(_)
                if stops == Stops::Evidence && state.stop == StopState::WorkersSignalled =>
            {
                read.gone.push(Gone {
                    run: paths.run.clone(),
                    node: node.clone(),
                    reason: stopped_reason(state),
                    by: GoneBy::Stopped,
                });
                continue;
            }
            registry => registry.proves(node),
        };
        let dispatched_at = state.dispatched_at.get(node).copied();
        let proof = match proof {
            Proof::Stale(_) if recorded_after_every_entry(registry, node, dispatched_at) => {
                Proof::Unproven(
                    "the run recorded this dispatch after every entry its registry holds for \
                     it, and those are of an earlier attempt that is gone, so nothing here \
                     names its process yet"
                        .to_string(),
                )
            }
            proof => proof,
        };
        let age_ms = dispatched_at.map(|at| sys::now_millis().saturating_sub(at));
        push(&mut read, paths, launch, node, age_ms, proof);
    }
    read
}

/// Whether the run recorded `node`'s current dispatch after every registry entry
/// it holds for that node was written.
///
/// A driver records a dispatch before the executor writes its entry, so an entry
/// stamped before the recorded dispatch is an earlier attempt's: one that is
/// stale says that attempt ended, and nothing about the dispatch the run is
/// running now — which is what a re-dispatch after an adoption looks like until
/// its own entry lands. An entry whose stamp cannot be read is not counted as
/// earlier.
fn recorded_after_every_entry(registry: &Registry, node: &str, dispatched_at: Option<u64>) -> bool {
    let (Registry::Read(by_node), Some(dispatched_at)) = (registry, dispatched_at) else {
        return false;
    };
    by_node.get(node).is_some_and(|records| {
        records.iter().all(|record| {
            crate::projection::millis_of(&record.dispatched_at)
                .is_some_and(|written| written < dispatched_at)
        })
    })
}

/// Why a stopped registry-less run's dispatch is gone.
///
/// The age is the run's last record's, which on a run proved ended is the stop
/// or a record after it, so the stop is at least that old.
fn stopped_reason(state: &RunState) -> String {
    match state.last_write_at {
        Some(at) => format!(
            "its run was stopped {} ago and its workers signalled",
            crate::telemetry::duration(sys::now_millis().saturating_sub(at))
        ),
        None => "its run was stopped and its workers signalled".to_string(),
    }
}

/// One running node's dispatch, as its proof places it.
fn push(
    read: &mut RunRead,
    paths: &RunPaths,
    launch: &LaunchRecord,
    node: &str,
    age_ms: Option<u64>,
    proof: Proof,
) {
    let row = |unproven: Option<String>| DispatchRow {
        run: paths.run.clone(),
        node: node.to_string(),
        launcher: launch.launcher.clone(),
        age_ms,
        proof: if unproven.is_some() {
            ProofWord::Unproven
        } else {
            ProofWord::Held
        },
        unproven,
    };
    match proof {
        Proof::Stale(why) => read.gone.push(Gone {
            run: paths.run.clone(),
            node: node.to_string(),
            reason: why,
            by: GoneBy::StaleEntry,
        }),
        Proof::Held => read.rows.push(row(None)),
        Proof::Unproven(why) => read.rows.push(row(Some(why))),
    }
}

/// A run whose journal is past [`MAX_COLD_JOURNAL_BYTES`] and whose derived
/// documents do not decide it: decided from its registry alone, or refused.
///
/// The journal is never opened, whatever the registry says. Each node an entry
/// names is proved as a row would be: a held entry is a row, one this host cannot
/// decide is a row saying so, and a node whose every entry is stale is gone. What
/// cannot be read without the journal is a running node with no entry at all, so
/// a registry-less run here is refused rather than guessed, and so is one whose
/// driver may still be running with no entry live.
fn from_the_registry(
    paths: &RunPaths,
    launch: &LaunchRecord,
    registry: &Registry,
    bytes: u64,
) -> Result<RunRead, String> {
    let refused = |why: &str| {
        format!(
            "its journal is {bytes} bytes, past the {MAX_COLD_JOURNAL_BYTES} a host read opens, \
             nothing current beside it says what it records running, and {why}"
        )
    };
    let by_node = match registry {
        Registry::Read(by_node) => by_node,
        Registry::Unreadable(why) => {
            return Err(refused(&format!(
                "its dispatch registry cannot be read: {why}"
            )))
        }
    };
    let _proofs = phase("pid-proofs");
    if !registry.has_live_entry() && !driver_proved_gone(launch) {
        return Err(refused("its driver is not proved gone"));
    }
    let mut read = RunRead::default();
    for (node, records) in by_node {
        let age_ms = records
            .iter()
            .filter_map(|record| crate::projection::millis_of(&record.dispatched_at))
            .max()
            .map(|at| sys::now_millis().saturating_sub(at));
        push(
            &mut read,
            paths,
            launch,
            node,
            age_ms,
            registry.proves(node),
        );
    }
    Ok(read)
}

/// The text `onepipeline host` prints: [`HostReading`] rendered.
pub fn render(reading: &HostReading) -> String {
    let mut out = format!("host {}\n", reading.host);
    // The scope of the claim: a run recorded under another runs root is a live
    // dispatch this view will never list, and a reader who does not know which
    // root was read cannot tell that absence from an idle host.
    out.push_str(&format!(
        "  reading {}\n",
        one_line(&reading.root.display().to_string())
    ));
    out.push_str(&crate::freespace::render(&reading.free_space));
    for row in &reading.dispatches {
        out.push_str(&format!(
            "  {:<24} {:<20} {:<16} {}",
            row.run,
            row.node,
            row.launcher,
            crate::telemetry::duration(row.age_ms.unwrap_or(0))
        ));
        if let Some(why) = &row.unproven {
            out.push_str(&format!("  UNPROVEN: {}", one_line(why)));
        }
        out.push('\n');
    }
    if reading.dispatches.is_empty() {
        out.push_str("  no live dispatches\n");
    }
    for by in [GoneBy::StaleEntry, GoneBy::Stopped] {
        let named: Vec<String> = reading
            .gone
            .iter()
            .filter(|gone| gone.by == by)
            .map(|gone| one_line(&format!("{}/{}: {}", gone.run, gone.node, gone.reason)))
            .collect();
        let noun = match (by, named.len()) {
            (_, 0) => continue,
            (GoneBy::StaleEntry, 1) => "stale registry entry ignored",
            (GoneBy::StaleEntry, _) => "stale registry entries ignored",
            (GoneBy::Stopped, 1) => "dispatch of a stopped run proven gone",
            (GoneBy::Stopped, _) => "dispatches of stopped runs proven gone",
        };
        out.push_str(&format!("  {} {noun}: {}\n", named.len(), named.join("; ")));
    }
    let mut refused: Vec<ledger::Skipped> = reading
        .skipped
        .iter()
        .map(|skipped| ledger::Skipped {
            path: skipped.path.clone(),
            reason: skipped.reason.clone(),
        })
        .collect();
    if let Some(why) = &reading.root_error {
        refused.insert(
            0,
            ledger::Skipped {
                path: reading.root.clone(),
                reason: why.clone(),
            },
        );
    }
    out.push_str(&crate::views::skipped_lines(&refused));
    out
}
