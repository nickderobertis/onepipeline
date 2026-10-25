//! Every call this engine makes into the plan store's API, counted where it is made.
//!
//! The store is a sibling whose own budgets bound what each of its backends spends per
//! operation; what this repository budgets is how many operations it asks for — a dependency
//! read or re-read, a status write, a settlement update, any other — and that is counted
//! here, at the seam each caller drives the store's futures through, naming the operation
//! and never a backend.
//!
//! Each operation is one line of the run's `store-operations.jsonl`: when it was asked for,
//! which operation it was, and what it was for — the phase, and the node or item it concerns
//! where one does. A process asks for some before it knows its run — a launch reads the plan
//! before the run's directory exists — so lines are held in memory until the process
//! [`attach`]es to a run, and a process that never does (a `plan check`) writes nothing.
//! Telemetry and nothing more: no decision reads it, and a line that cannot be written is
//! dropped rather than failing the call it counts.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Value};

/// The file, in a run's directory, each operation is appended to.
pub(crate) const FILE: &str = "store-operations.jsonl";

/// How many lines a process holds before it knows its run; past it the oldest are dropped,
/// so a process that never attaches cannot grow without bound.
const HELD: usize = 10_000;

/// One kind of call into the store's API, named for the call and not for any backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Operation {
    /// One page of a task's dependency edges.
    Dependencies,
    /// One task, by its id.
    Task,
    /// One page of a project's tasks.
    Tasks,
    /// One project, by its id.
    Project,
    /// One document, by its id.
    Document,
    /// One targeted update of a task: a status write or a settlement.
    TaskUpdate,
    /// A copy, which creates what it carries.
    Copy,
    /// One project metadata key set.
    ProjectMetadataSet,
    /// The image assets one task holds.
    TaskAssets,
    /// The items a landing's closed issues stand for, moved to done.
    CompleteIssues,
    /// A project's own status set, as a run that settled marks its home.
    ProjectStatusSet,
    /// The end of one command, which clears what the store held for it.
    EndCommand,
}

impl Operation {
    /// The word a line carries.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Dependencies => "task-dependencies",
            Self::Task => "task",
            Self::Tasks => "tasks",
            Self::Project => "project",
            Self::Document => "document",
            Self::TaskUpdate => "task-update",
            Self::Copy => "copy",
            Self::ProjectMetadataSet => "project-metadata-set",
            Self::TaskAssets => "task-assets",
            Self::CompleteIssues => "complete-issues",
            Self::ProjectStatusSet => "project-status-set",
            Self::EndCommand => "end-command",
        }
    }
}

/// Where this process's lines go: held until it knows its run, then appended to its file.
struct Sink {
    file: Option<PathBuf>,
    held: VecDeque<String>,
}

impl Sink {
    const fn new() -> Self {
        Self {
            file: None,
            held: VecDeque::new(),
        }
    }

    /// Take one line: appended where the run is known, and held — the oldest dropped past
    /// [`HELD`] — where it is not.
    fn take(&mut self, line: String) {
        match &self.file {
            Some(file) => append(file, std::slice::from_ref(&line)),
            None => {
                if self.held.len() >= HELD {
                    self.held.pop_front();
                }
                self.held.push_back(line);
            }
        }
    }

    /// Send every line from now to the run whose directory is `dir`, the held ones first.
    fn attach(&mut self, dir: &Path) {
        let file = dir.join(FILE);
        let held: Vec<String> = std::mem::take(&mut self.held).into();
        append(&file, &held);
        self.file = Some(file);
    }
}

static SINK: Mutex<Sink> = Mutex::new(Sink::new());

/// What one stretch of work asks the store for operations for: the phase, and the node or the
/// item the operations concern, where they concern one.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Scope<'a> {
    /// The launch's read of the plan.
    Launch,
    /// A re-read of `node`'s edges, held on the sources `held_on` names.
    Reread {
        node: &'a str,
        held_on: &'a std::collections::BTreeSet<String>,
    },
    /// The write-back attempt started at `projection_at`, as its projection record stamps it.
    WriteBack { projection_at: &'a str },
    /// A stop's release attempt started at `projection_at`.
    Release { projection_at: &'a str },
    /// The one item an attempt's operations now concern, inside that attempt.
    Item(&'a str),
    /// The check of an edit a planner sent.
    Edit,
}

/// What the operations this thread asks for now are for, each phase carrying exactly the
/// attribution it has: a re-read its node and the sources it is held on, a projection its
/// attempt's stamp and — inside it — the item, the others nothing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "phase", rename_all = "kebab-case")]
enum Context {
    Launch,
    ReRead {
        node: String,
        held_on: Vec<String>,
    },
    WriteBack {
        projection_at: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        item: Option<String>,
    },
    Release {
        projection_at: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        item: Option<String>,
    },
    Edit,
}

impl Context {
    /// The context inside `scope`, entered from `outer`: a phase replaces the outer one, and an
    /// item narrows the projection it is inside — and names nothing outside one.
    fn within(outer: Option<&Self>, scope: Scope<'_>) -> Option<Self> {
        Some(match scope {
            Scope::Launch => Self::Launch,
            Scope::Reread { node, held_on } => Self::ReRead {
                node: node.to_owned(),
                held_on: held_on.iter().cloned().collect(),
            },
            Scope::WriteBack { projection_at } => Self::WriteBack {
                projection_at: projection_at.to_owned(),
                item: None,
            },
            Scope::Release { projection_at } => Self::Release {
                projection_at: projection_at.to_owned(),
                item: None,
            },
            Scope::Edit => Self::Edit,
            Scope::Item(named) => match outer? {
                Self::WriteBack { projection_at, .. } => Self::WriteBack {
                    projection_at: projection_at.clone(),
                    item: Some(named.to_owned()),
                },
                Self::Release { projection_at, .. } => Self::Release {
                    projection_at: projection_at.clone(),
                    item: Some(named.to_owned()),
                },
                other => other.clone(),
            },
        })
    }
}

thread_local! {
    /// What the operations this thread asks for now are for.
    static ABOUT: RefCell<Option<Context>> = const { RefCell::new(None) };
}

/// Send this process's operations to the run whose directory is `dir`, the ones it asked for
/// before it knew its run first.
pub(crate) fn attach(dir: &Path) {
    SINK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .attach(dir);
}

/// Run `within` with every operation it asks for on this thread said to be for `scope`, an
/// inner scope's fields standing over the outer ones' for its length.
pub(crate) fn about<T>(scope: Scope<'_>, within: impl FnOnce() -> T) -> T {
    let outer = ABOUT.with(|current| {
        let mut current = current.borrow_mut();
        let inner = Context::within(current.as_ref(), scope);
        std::mem::replace(&mut *current, inner)
    });
    let answer = within();
    ABOUT.with(|current| *current.borrow_mut() = outer);
    answer
}

/// Count one operation as it is asked for, before its future is driven: one that never
/// answers, or is cancelled at its deadline, was still asked of the store and is counted.
pub(crate) fn record(operation: Operation) {
    let line = line(operation).to_string();
    SINK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take(line);
}

/// The line one operation asked for now, on this thread, is written as.
fn line(operation: Operation) -> Value {
    #[derive(serde::Serialize)]
    struct Line<'a> {
        at_ms: u64,
        operation: &'static str,
        #[serde(flatten, skip_serializing_if = "Option::is_none")]
        context: &'a Option<Context>,
    }
    ABOUT.with(|about| {
        json!(Line {
            at_ms: crate::sys::now_millis(),
            operation: operation.as_str(),
            context: &about.borrow(),
        })
    })
}

/// Append `lines` to `file`, dropping them where it cannot be written: telemetry never fails
/// what it counts.
fn append(file: &Path, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
    if let Ok(mut opened) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
    {
        let _ = opened.write_all(text.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scope's members ride each line asked for inside it, an inner scope's over an outer
    /// one's, and none outlives its scope.
    #[test]
    fn a_line_carries_the_scope_it_was_asked_for_in() {
        let held_on = std::collections::BTreeSet::from(["vault".to_owned()]);
        let read = about(
            Scope::WriteBack {
                projection_at: "2026-10-24T14:02:46.658Z",
            },
            || about(Scope::Item("design"), || line(Operation::TaskUpdate)),
        );
        assert_eq!(
            read,
            json!({"at_ms": read["at_ms"], "operation": "task-update", "phase": "write-back",
                   "projection_at": "2026-10-24T14:02:46.658Z", "item": "design"})
        );
        let reread = about(
            Scope::Reread {
                node: "build",
                held_on: &held_on,
            },
            || line(Operation::Dependencies),
        );
        assert_eq!(reread["phase"], "re-read");
        assert_eq!(reread["node"], "build");
        assert_eq!(reread["held_on"], json!(["vault"]));
        // An item names nothing outside the projection it narrows.
        let stray = about(Scope::Item("design"), || line(Operation::Task));
        assert_eq!(stray.get("item"), None, "{stray}");
        assert_eq!(stray.get("phase"), None, "{stray}");
        let bare = line(Operation::Task);
        assert_eq!(bare.get("phase"), None, "{bare}");
        assert_eq!(bare.get("node"), None, "{bare}");
    }

    /// Lines held before the run is known are bounded, the oldest dropped first, and reach
    /// the run's file, in order, once it is.
    ///
    /// Held here, on the sink itself, rather than by a journey: reaching the bound through the
    /// CLI needs a process asking the store for more than ten thousand operations before its
    /// run exists — a launch reading a plan of thousands of tasks — for a defect that costs
    /// telemetry lines and never a run; the journey
    /// `store::a_run_whose_store_operations_cannot_be_recorded_runs_and_writes_back_as_it_would`
    /// holds the other half, a run whose lines cannot be written running as it would.
    #[test]
    fn held_lines_are_bounded_and_reach_the_run_once_it_is_known() {
        let dir = std::env::temp_dir().join(format!("storeops-held-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a run directory");
        let mut sink = Sink::new();
        for n in 0..=HELD {
            sink.take(format!("{{\"n\":{n}}}"));
        }
        assert_eq!(sink.held.len(), HELD);
        assert_eq!(sink.held.front().map(String::as_str), Some("{\"n\":1}"));
        sink.attach(&dir);
        sink.take("{\"n\":\"after\"}".to_owned());
        let written = std::fs::read_to_string(dir.join(FILE)).expect("the run's file");
        let lines: Vec<&str> = written.lines().collect();
        assert_eq!(lines.len(), HELD + 1);
        assert_eq!(lines.first(), Some(&"{\"n\":1}"));
        assert_eq!(lines.last(), Some(&"{\"n\":\"after\"}"));
        std::fs::remove_dir_all(&dir).expect("the run directory is removed");
    }

    /// A line the run's file cannot take is dropped and nothing fails: telemetry never fails
    /// the operation it counts.
    #[test]
    fn a_line_the_run_cannot_take_is_dropped() {
        let dir = std::env::temp_dir().join(format!("storeops-blocked-{}", std::process::id()));
        // The file's name is a directory, which no append opens.
        std::fs::create_dir_all(dir.join(FILE)).expect("a directory where the file goes");
        let mut sink = Sink::new();
        sink.take("{\"n\":0}".to_owned());
        sink.attach(&dir);
        sink.take("{\"n\":1}".to_owned());
        assert!(
            dir.join(FILE).is_dir(),
            "a line was written over the directory"
        );
        assert!(
            sink.held.is_empty(),
            "lines stayed held after the run was known"
        );
        std::fs::remove_dir_all(&dir).expect("the run directory is removed");
    }
}
