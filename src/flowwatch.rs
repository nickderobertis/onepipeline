//! `onepipeline watch --flow` — a bounded, resumable wait on one flow and every
//! run launched in it.
//!
//! What it returns on, with which status, is the Flows section of
//! `docs/stop-guard.md` and entry 114 of `docs/contract-divergences.md`; neither
//! is restated here. What belongs beside the code is that this wait **polls**
//! where a run's waits on a fingerprint: what it watches is the flow's own
//! files, its holder's liveness — which moves with no file moving — and a set of
//! runs that grows while it waits. Each poll is a handful of small reads, made at
//! most every [`POLL`], and a run's whole journal is never read twice: each
//! member is tailed from the cursor the last poll left it at.
//!
//! Like a run's watch it takes no lock, consumes no surface, and records itself
//! as a lease beside what it watches — the flow's own `watchers/` — so
//! `unwatched` can ask whether anything is watching the flow.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::cli::{WatchTimeout, WatchUntil, WATCH_CURSOR_VERSION};
use crate::error::{
    Error, Result, EXIT_FLOW_FAILED, EXIT_NODE_SETTLED, EXIT_RUN_JOINED, EXIT_SUCCESS,
    EXIT_SURFACE_WAITING, EXIT_WATCH_ELAPSED,
};
use crate::flow::{Flow, Standing};
use crate::ledger::RunPaths;
use crate::watch::{Cursor, Lines};

/// The longest a flow watch goes between two looks at what it watches.
const POLL: Duration = crate::watch::DEADLINE_CHECK;

/// The conditions a flow watch returns on when it is told none: all three.
const DEFAULT_UNTIL: [WatchUntil; 3] = [
    WatchUntil::Surface,
    WatchUntil::NodeSettled,
    WatchUntil::RunJoined,
];

/// What a flow watch is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Request {
    /// How long to wait before giving up.
    pub(crate) timeout: WatchTimeout,
    /// How long a silence may last before the stream says it is still there.
    /// Zero turns the heartbeat off.
    pub(crate) tick: Duration,
    /// A cursor an earlier flow watch printed, to resume from.
    // llmlint: ignore[invalid_states_unrepresentable] a cursor token is external input, placed against the flow and each member's journal by `resume`, which is the check no type can make — the same reason `watch::Request::cursor` gives.
    pub(crate) cursor: Option<String>,
    /// What ends the wait beside the flow's ending: empty is all three.
    pub(crate) until: Vec<WatchUntil>,
}

/// Why a flow watch returned.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ending {
    /// A planner surface is waiting on the flow's own channel.
    SurfaceOnFlow,
    /// A planner surface is waiting on a member run's channel.
    // llmlint: ignore[invalid_states_unrepresentable] a run id read off the flow's own membership, which `Flow::members` admits only as a run id; the crate spells one `String` everywhere.
    SurfaceOnRun(String),
    /// A member run's node settled.
    // llmlint: ignore[invalid_states_unrepresentable] the run and the node as a journalled settlement names them, for `watch::Ending::NodeSettled`'s reason.
    NodeSettled { run: String, node: String },
    /// A run joined the flow.
    // llmlint: ignore[invalid_states_unrepresentable] a run id read off the flow's own membership, as `SurfaceOnRun`'s is.
    RunJoined(String),
    /// The flow's program ended `0`.
    Ended,
    /// The flow's program ended with this non-zero status.
    Failed(i32),
    /// The flow's holder died with no ending recorded.
    Died,
    /// The wait's own bound ran out.
    Elapsed,
}

impl Ending {
    /// The condition's word, as the machine record's `condition` spells it.
    const fn as_str(&self) -> &'static str {
        match self {
            Self::SurfaceOnFlow | Self::SurfaceOnRun(_) => "surface",
            Self::NodeSettled { .. } => "node-settled",
            Self::RunJoined(_) => "run-joined",
            Self::Ended => "ended",
            Self::Failed(_) | Self::Died => "flow-failed",
            Self::Elapsed => "elapsed",
        }
    }

    /// The status the binary exits with for this ending.
    const fn exit_code(&self) -> i32 {
        match self {
            Self::SurfaceOnFlow | Self::SurfaceOnRun(_) => EXIT_SURFACE_WAITING,
            Self::NodeSettled { .. } => EXIT_NODE_SETTLED,
            Self::RunJoined(_) => EXIT_RUN_JOINED,
            Self::Ended => EXIT_SUCCESS,
            Self::Failed(_) | Self::Died => EXIT_FLOW_FAILED,
            Self::Elapsed => EXIT_WATCH_ELAPSED,
        }
    }

    /// The human line's words for this ending, naming what it is about.
    fn phrase(&self, flow: &str) -> String {
        let close = format!(
            "close it with: onepipeline unwatched --acknowledge-flow {flow} --reason <TEXT>"
        );
        match self {
            Self::SurfaceOnFlow => format!("surface on flow {flow}"),
            Self::SurfaceOnRun(run) => format!("surface on run {run}"),
            Self::NodeSettled { run, node } => format!("node-settled {run} {node}"),
            Self::RunJoined(run) => format!("run-joined {run}"),
            Self::Ended => "ended with status 0".to_owned(),
            Self::Failed(status) => {
                format!("flow-failed: it ended with status {status} — {close}")
            }
            Self::Died => format!(
                "flow-failed: it died — its holder is gone with no ending recorded — {close}"
            ),
            Self::Elapsed => "elapsed".to_owned(),
        }
    }

    /// The machine record's fields about this ending, beyond its word and code.
    fn fields(&self, record: &mut serde_json::Map<String, serde_json::Value>) {
        match self {
            Self::SurfaceOnRun(run) | Self::RunJoined(run) => {
                record.insert("run_id".to_owned(), json!(run));
            }
            Self::NodeSettled { run, node } => {
                record.insert("run_id".to_owned(), json!(run));
                record.insert("node".to_owned(), json!(node));
            }
            Self::Ended => {
                record.insert("status".to_owned(), json!(0));
            }
            Self::Failed(status) => {
                record.insert("status".to_owned(), json!(status));
            }
            Self::Died => {
                record.insert("died".to_owned(), json!(true));
            }
            Self::SurfaceOnFlow | Self::Elapsed => {}
        }
    }
}

/// The conditions a flow watch was told to return on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Selected {
    surface: bool,
    node_settled: bool,
    run_joined: bool,
}

impl Selected {
    /// Read the conditions, refusing one a flow watch does not return on.
    fn resolve(until: &[WatchUntil]) -> Result<Self> {
        let mut chosen = Self::default();
        for condition in until {
            match condition {
                WatchUntil::Surface => chosen.surface = true,
                WatchUntil::NodeSettled => chosen.node_settled = true,
                WatchUntil::RunJoined => chosen.run_joined = true,
                other => {
                    return Err(Error::Invalid(format!(
                        "`--until {other}` is a run watch's condition: `watch --flow` returns on \
                         surface, node-settled and run-joined, and always on the flow's ending"
                    )))
                }
            }
        }
        Ok(chosen)
    }
}

/// The cursor-token prefix a flow watch prints and reads back.
const CURSOR_PREFIX: &str = "flow";

/// `watch --flow`: block until the flow or one of its runs needs a supervisor,
/// or until the wait runs out, and answer the status.
///
/// Every refusal — the conditions and the cursor — is made before the lease is
/// written or a second is waited. Every line goes to `say` as it happens, and a
/// `say` that refuses ends the wait with its refusal.
///
/// # Errors
///
/// A condition a flow watch does not return on, a cursor this flow cannot
/// place, a deadline past what the clock can name, and a `say` that refused.
pub(crate) fn watch(
    root: &std::path::Path,
    flow: &Flow,
    request: &Request,
    say: &mut dyn FnMut(&Lines) -> Result<()>,
) -> Result<i32> {
    let until: &[WatchUntil] = match request.until.is_empty() {
        true => &DEFAULT_UNTIL,
        false => &request.until,
    };
    let selected = Selected::resolve(until)?;
    let deadline = crate::watch::deadline(request.timeout)?;
    let mut known = match &request.cursor {
        Some(token) => resume(root, flow, token)?,
        None => armed_at(root, flow)?,
    };
    let _armed = crate::watchers::Armed::arm(&flow.paths, request.timeout, until);
    let mut quiet_since = Instant::now();
    let mut unread = 0_usize;
    let mut first = true;
    let ended = |ending: Ending,
                 known: &BTreeMap<String, Cursor>,
                 unread: usize,
                 say: &mut dyn FnMut(&Lines) -> Result<()>| {
        say(&ending_lines(
            flow.id(),
            &ending,
            &cursor_of(flow.id(), known),
            unread,
        )?)?;
        Ok(ending.exit_code())
    };
    loop {
        // Asked before the pass rather than only after it, so a wait whose
        // deadline came due while it slept does nothing past the deadline but
        // say how it ended.
        let due = || deadline.is_some_and(|deadline| Instant::now() >= deadline);
        if !first && due() {
            return ended(Ending::Elapsed, &known, unread, say);
        }
        first = false;
        let (ending, waiting) = pass(root, flow, selected, &mut known)?;
        unread = waiting;
        if let Some(ending) = ending {
            return ended(ending, &known, unread, say);
        }
        if due() {
            return ended(Ending::Elapsed, &known, unread, say);
        }
        if !request.tick.is_zero() && quiet_since.elapsed() >= request.tick {
            say(&heartbeat_lines(flow, unread)?)?;
            quiet_since = Instant::now();
        }
        std::thread::sleep(
            crate::watch::wake_within(deadline, request.tick, quiet_since).min(POLL),
        );
    }
}

/// Every member run positioned at the end of its journal as it stands, which is
/// what a watch given no cursor waits past: what was journalled before it armed
/// is history, and answers no condition.
fn armed_at(root: &std::path::Path, flow: &Flow) -> Result<BTreeMap<String, Cursor>> {
    Ok(flow
        .members()?
        .into_iter()
        .map(|run| {
            let mut cursor = Cursor::start(&run);
            crate::watch::tail(&RunPaths::under(root, &run), &mut cursor);
            (run, cursor)
        })
        .collect())
}

/// The member runs a cursor token names, each placed against its own journal.
///
/// A member the token does not name is past the cursor: it joined after the
/// watch that printed it, and the first pass reports it.
fn resume(root: &std::path::Path, flow: &Flow, token: &str) -> Result<BTreeMap<String, Cursor>> {
    let refusal = || {
        Error::Invalid(format!(
            "'{token}' is not a cursor this build reads for a flow; a flow's cursor is what an \
             earlier `onepipeline watch --flow` printed, spelled \
             `{CURSOR_PREFIX}:{WATCH_CURSOR_VERSION}:<flow>:<run>@<byte>/...`"
        ))
    };
    let rest = token
        .strip_prefix(&format!("{CURSOR_PREFIX}:{WATCH_CURSOR_VERSION}:"))
        .ok_or_else(refusal)?;
    // A flow id carries no `:`, so the first one ends it.
    let (named, places) = rest.split_once(':').ok_or_else(refusal)?;
    if named != flow.id() {
        return Err(Error::Invalid(format!(
            "cursor '{token}' was printed by a watch of flow '{named}', and this watches flow \
             '{}'; a cursor is only readable by the flow it was printed for",
            flow.id()
        )));
    }
    let members = flow.members()?;
    let mut known = BTreeMap::new();
    for place in places.split('/').filter(|place| !place.is_empty()) {
        // The byte follows the last `@`, so a run id carrying one reads back.
        let (run, at) = place.rsplit_once('@').ok_or_else(refusal)?;
        if !members.iter().any(|member| member == run) {
            return Err(Error::Invalid(format!(
                "cursor '{token}' names run '{run}', which is not a run of flow '{}'",
                flow.id()
            )));
        }
        let cursor = crate::watch::resolve_cursor(
            &RunPaths::under(root, run),
            &format!("{WATCH_CURSOR_VERSION}:{run}:{at}"),
        )?;
        known.insert(run.to_owned(), cursor);
    }
    Ok(known)
}

/// The token a later flow watch resumes from: every member this watch knows,
/// and how far into its journal it has read.
fn cursor_of(flow: &str, known: &BTreeMap<String, Cursor>) -> String {
    let places: Vec<String> = known
        .iter()
        .map(|(run, cursor)| format!("{run}@{}", cursor.at))
        .collect();
    format!(
        "{CURSOR_PREFIX}:{WATCH_CURSOR_VERSION}:{flow}:{}",
        places.join("/")
    )
}

/// One look at the flow and every member, in the order the conditions are
/// answered: a waiting surface, a settled node, a joined run, and the flow's
/// own ending. Answers the ending this look reached, if any, and how many
/// planner surfaces are unread across the flow's channel and its members'.
///
/// # Errors
///
/// The flow's membership becoming unreadable while the watch waits.
fn pass(
    root: &std::path::Path,
    flow: &Flow,
    selected: Selected,
    known: &mut BTreeMap<String, Cursor>,
) -> Result<(Option<Ending>, usize)> {
    // Read before the members are, so a run that joins while this pass runs is
    // the next pass's to report, with its settlements from byte zero.
    let members = flow.members()?;
    let unread = flow.channel().queue().waiting.len()
        + known
            .keys()
            .map(|run| {
                crate::channel::ChannelState::new(&RunPaths::under(root, run))
                    .queue()
                    .waiting
                    .len()
            })
            .sum::<usize>();
    if selected.surface {
        if crate::watch::surface_waiting(&flow.paths).is_some() {
            return Ok((Some(Ending::SurfaceOnFlow), unread));
        }
        if let Some(run) = known
            .keys()
            .find(|run| crate::watch::surface_waiting(&RunPaths::under(root, run)).is_some())
        {
            return Ok((Some(Ending::SurfaceOnRun(run.clone())), unread));
        }
    }
    // One settlement at a time: each member is read up to and including its
    // first settlement and no further, and the members after it not at all, so
    // the cursor this ends on is past exactly what it reported and a second
    // settlement is the next watch's to report rather than lost behind it.
    for (run, cursor) in known.iter_mut() {
        let journal = RunPaths::under(root, run).journal();
        if !selected.node_settled {
            crate::watch::tail(&RunPaths::under(root, run), cursor);
            continue;
        }
        let (fresh, at) = crate::journal::finished_through_first(&journal, cursor.at, |event| {
            crate::watch::settlement_of(event).is_some()
        });
        cursor.at = at;
        if let Some(node) = fresh.iter().find_map(crate::watch::settlement_of) {
            return Ok((
                Some(Ending::NodeSettled {
                    run: run.clone(),
                    node: node.to_owned(),
                }),
                unread,
            ));
        }
    }
    // And one join at a time, for the same reason: a run this watch has not
    // reported joining stays out of its cursor, so the next watch reports it.
    for run in members {
        if known.contains_key(&run) {
            continue;
        }
        known.insert(run.clone(), Cursor::start(&run));
        if selected.run_joined {
            return Ok((Some(Ending::RunJoined(run)), unread));
        }
    }
    let ending = match flow.standing() {
        Ok(Standing::Ended(0)) => Some(Ending::Ended),
        Ok(Standing::Ended(status)) => Some(Ending::Failed(status)),
        Ok(Standing::Died) => Some(Ending::Died),
        // An ending that cannot be read says neither that the flow ended nor that
        // it did not, and `unwatched` names it: the wait goes on.
        Ok(Standing::Live) | Err(_) => None,
    };
    Ok((ending, unread))
}

/// How many unread planner surfaces, as one clause, a zero said out loud.
fn unread_phrase(unread: usize) -> String {
    match unread {
        0 => "0 unread planner surfaces".to_owned(),
        count => format!("{count} unread planner surface(s)"),
    }
}

fn rendered(record: &serde_json::Value) -> Result<String> {
    serde_json::to_string(record)
        .map_err(|e| Error::Invalid(format!("the watch could not render a record: {e}")))
}

/// The two lines a flow watch ends on.
fn ending_lines(flow: &str, ending: &Ending, cursor: &str, unread: usize) -> Result<Lines> {
    let mut record = serde_json::Map::new();
    record.insert("watch".to_owned(), json!("return"));
    record.insert("flow_id".to_owned(), json!(flow));
    record.insert("condition".to_owned(), json!(ending.as_str()));
    record.insert("exit".to_owned(), json!(ending.exit_code()));
    ending.fields(&mut record);
    record.insert("cursor".to_owned(), json!(cursor));
    record.insert("unread".to_owned(), json!({"count": unread}));
    Ok(Lines {
        human: format!(
            "-- watch flow {flow} {}  {}  cursor {cursor}",
            ending.phrase(flow),
            unread_phrase(unread)
        ),
        machine: rendered(&serde_json::Value::Object(record))?,
    })
}

/// The two lines a flow watch says while nothing has happened for a tick.
fn heartbeat_lines(flow: &Flow, unread: usize) -> Result<Lines> {
    let standing = match flow.standing() {
        Ok(Standing::Live) => "LIVE",
        Ok(Standing::Ended(_)) => "ENDED",
        Ok(Standing::Died) => "DIED",
        Err(_) => "UNKNOWN",
    };
    Ok(Lines {
        human: format!(
            "-- watching flow {}  {standing}  {}",
            flow.id(),
            unread_phrase(unread)
        ),
        machine: rendered(&json!({
            "watch": "heartbeat",
            "flow_id": flow.id(),
            "unread": {"count": unread},
        }))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cursor names the flow and every member's place, and reads back as what
    /// it printed; a flow's cursor is refused by a run watch's parser and the
    /// other way round, because each is spelled with its own prefix.
    #[test]
    fn a_flow_cursor_is_spelled_with_its_flow_and_every_members_place() {
        let mut known = BTreeMap::new();
        let mut a = Cursor::start("draft");
        a.at = 120;
        known.insert("draft".to_owned(), a);
        known.insert("spikes@x".to_owned(), Cursor::start("spikes@x"));
        assert_eq!(
            cursor_of("plan", &known),
            format!("flow:{WATCH_CURSOR_VERSION}:plan:draft@120/spikes@x@0")
        );
        assert_eq!(
            cursor_of("plan", &BTreeMap::new()),
            format!("flow:{WATCH_CURSOR_VERSION}:plan:")
        );
    }

    /// Each ending exits the status the contract gives it, and the record names
    /// what it is about.
    #[test]
    fn each_ending_exits_its_own_status_and_names_what_it_is_about() {
        for (ending, code, word) in [
            (Ending::SurfaceOnFlow, 4, "surface"),
            (Ending::SurfaceOnRun("r".into()), 4, "surface"),
            (
                Ending::NodeSettled {
                    run: "r".into(),
                    node: "n".into(),
                },
                6,
                "node-settled",
            ),
            (Ending::RunJoined("r".into()), 8, "run-joined"),
            (Ending::Ended, 0, "ended"),
            (Ending::Failed(3), 9, "flow-failed"),
            (Ending::Died, 9, "flow-failed"),
            (Ending::Elapsed, 5, "elapsed"),
        ] {
            let lines = ending_lines("plan", &ending, "flow:1:plan:", 2).expect("lines");
            let record: serde_json::Value =
                serde_json::from_str(&lines.machine).expect("one JSON object");
            assert_eq!(record["exit"], code, "{lines:?}");
            assert_eq!(record["condition"], word, "{lines:?}");
            assert_eq!(record["cursor"], "flow:1:plan:", "{lines:?}");
            assert!(
                lines.human.ends_with("cursor flow:1:plan:"),
                "{}",
                lines.human
            );
        }
        let failed = ending_lines("plan", &Ending::Failed(3), "c", 0).expect("lines");
        assert!(
            failed.human.contains("ended with status 3")
                && failed
                    .human
                    .contains("onepipeline unwatched --acknowledge-flow plan --reason <TEXT>"),
            "{}",
            failed.human
        );
    }

    /// The Flows section of `docs/stop-guard.md` is the one statement of this
    /// surface, so it is held to the code both ways: each synopsis names exactly
    /// the flags clap offers, the exit table exactly the endings and their codes,
    /// the return record exactly the keys a return can carry, and the wake reserve
    /// and the deadline check the constants the verbs use.
    #[test]
    fn the_flows_section_states_exactly_this_surface() {
        use clap::CommandFactory;
        use std::collections::BTreeSet;

        let page = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("docs")
                .join("stop-guard.md"),
        )
        .expect("the page ships");
        let section = page
            .split_once("\n## Flows\n")
            .expect("the page has a Flows section")
            .1
            .split_once("\n## ")
            .map_or(String::new(), |(section, _)| section.to_owned());
        let flags_of = |synopsis: &str| -> BTreeSet<String> {
            synopsis
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                .filter_map(|word| word.strip_prefix("--"))
                .filter(|word| !word.is_empty())
                .map(str::to_owned)
                .collect()
        };
        let synopsis = |opening: &str| -> String {
            section
                .lines()
                .find(|line| line.starts_with(opening))
                .unwrap_or_else(|| panic!("the section states `{opening}`"))
                .to_owned()
        };
        let command = crate::cli::Cli::command();
        let offered = |path: &[&str]| -> BTreeSet<String> {
            let mut verb = command.clone();
            for name in path {
                verb = verb
                    .find_subcommand(name)
                    .unwrap_or_else(|| panic!("the binary offers `{name}`"))
                    .clone();
            }
            verb.get_arguments()
                .filter_map(|arg| arg.get_long().map(str::to_owned))
                .collect()
        };
        assert_eq!(
            flags_of(&synopsis("onepipeline flow run ")),
            offered(&["flow", "run"])
        );
        // A run's own two are what `--flow` refuses beside it.
        let mut watch = offered(&["watch"]);
        watch.remove("filter");
        watch.remove("all");
        assert_eq!(flags_of(&synopsis("onepipeline watch --flow ")), watch);

        let endings = [
            Ending::SurfaceOnFlow,
            Ending::NodeSettled {
                run: "r".into(),
                node: "n".into(),
            },
            Ending::RunJoined("r".into()),
            Ending::Ended,
            Ending::Failed(3),
            Ending::Elapsed,
        ];
        let stated: BTreeSet<(String, String)> = section
            .lines()
            .filter_map(|row| {
                let cells: Vec<&str> = row.split('|').map(str::trim).collect();
                let code = cells.get(1)?.strip_prefix('`')?.strip_suffix('`')?;
                let word = cells.get(2)?.strip_prefix('`')?.strip_suffix('`')?;
                code.parse::<i32>()
                    .ok()
                    .map(|_| (code.to_owned(), word.to_owned()))
            })
            .collect();
        let built: BTreeSet<(String, String)> = endings
            .iter()
            .map(|ending| (ending.exit_code().to_string(), ending.as_str().to_owned()))
            .collect();
        assert_eq!(stated, built, "the exit table is not this build's endings");

        // The return record as the section spells it: the keys every return
        // carries, as one object in one span, and the ones a condition adds,
        // named after it. Each emitted record is held to both, both ways.
        let (_, after) = section
            .split_once("`{\"watch\":\"return\"")
            .expect("the section states the return record");
        let (span, after) = after.split_once('`').expect("the record is one span");
        let spelled: serde_json::Value =
            serde_json::from_str(&format!("{{\"watch\":\"return\"{span}").replace('…', "0"))
                .expect("the documented record is an object once its values are filled in");
        let always: BTreeSet<String> = spelled
            .as_object()
            .expect("an object")
            .keys()
            .cloned()
            .collect();
        let optional: BTreeSet<String> = after
            .split_once("with ")
            .and_then(|(_, rest)| rest.split_once(" where"))
            .expect("the section names the keys a condition adds")
            .0
            .split('`')
            .skip(1)
            .step_by(2)
            .map(str::to_owned)
            .collect();
        let mut added: BTreeSet<String> = BTreeSet::new();
        for ending in endings.iter().chain([&Ending::Died]) {
            let lines = ending_lines("plan", ending, "c", 0).expect("lines");
            let record: serde_json::Value = serde_json::from_str(&lines.machine).expect("JSON");
            let keys: BTreeSet<String> = record
                .as_object()
                .expect("an object")
                .keys()
                .cloned()
                .collect();
            assert!(
                keys.is_superset(&always),
                "{ending:?} returns without a key the section says every return carries: {record}"
            );
            let extra: BTreeSet<String> = keys.difference(&always).cloned().collect();
            assert!(
                extra.is_subset(&optional),
                "{ending:?} returns a key the section does not name: {record}"
            );
            added.extend(extra);
            assert_eq!(
                record["unread"]
                    .as_object()
                    .map(|unread| unread.keys().cloned().collect::<BTreeSet<_>>()),
                spelled["unread"]
                    .as_object()
                    .map(|unread| unread.keys().cloned().collect::<BTreeSet<_>>()),
                "{record}"
            );
        }
        assert_eq!(
            added, optional,
            "the section names a key no return carries, or misses one that does"
        );

        for stated in [
            format!(
                "**wake reserve of {} seconds**",
                crate::cli::WAKE_RESERVE_SECONDS
            ),
            format!("**{} milliseconds**", crate::cli::DEADLINE_CHECK_MILLIS),
            format!("`{}`", crate::cli::FLOW_ENV),
        ] {
            assert!(
                section.contains(&stated),
                "the section does not state {stated}"
            );
        }
    }

    /// A run watch's conditions are refused on a flow watch, naming the three it
    /// takes.
    #[test]
    fn a_run_watchs_condition_is_refused_on_a_flow() {
        let refused = Selected::resolve(&[WatchUntil::Settled]).expect_err("a run's condition");
        assert!(refused.to_string().contains("run-joined"), "{refused}");
        assert_eq!(
            Selected::resolve(&DEFAULT_UNTIL).expect("the default set"),
            Selected {
                surface: true,
                node_settled: true,
                run_joined: true
            }
        );
    }
}
