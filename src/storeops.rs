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
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Map, Value};

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

/// Where this process's lines go.
struct Sink {
    file: Option<PathBuf>,
    held: Vec<String>,
}

static SINK: Mutex<Sink> = Mutex::new(Sink {
    file: None,
    held: Vec::new(),
});

thread_local! {
    /// What the operations this thread asks for now are for.
    static ABOUT: RefCell<Map<String, Value>> = RefCell::new(Map::new());
}

/// Send this process's operations to the run whose directory is `dir`, the ones it asked for
/// before it knew its run first.
pub(crate) fn attach(dir: &Path) {
    let mut sink = SINK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let file = dir.join(FILE);
    let held = std::mem::take(&mut sink.held);
    append(&file, &held);
    sink.file = Some(file);
}

/// Run `within` with every operation it asks for on this thread said to be for `about`: an
/// object whose members each line carries beside the operation, nested scopes' members
/// standing over the outer ones' for their length.
pub(crate) fn about<T>(about: Value, within: impl FnOnce() -> T) -> T {
    let Value::Object(members) = about else {
        return within();
    };
    let outer = ABOUT.with(|current| {
        let mut current = current.borrow_mut();
        let outer = current.clone();
        current.extend(members);
        outer
    });
    let answer = within();
    ABOUT.with(|current| *current.borrow_mut() = outer);
    answer
}

/// Count one operation asked for now.
pub(crate) fn record(operation: Operation) {
    let line = line(operation).to_string();
    let mut sink = SINK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match &sink.file {
        Some(file) => append(file, std::slice::from_ref(&line)),
        None => {
            if sink.held.len() >= HELD {
                sink.held.remove(0);
            }
            sink.held.push(line);
        }
    }
}

/// The line one operation asked for now, on this thread, is written as.
fn line(operation: Operation) -> Value {
    let mut line = Map::new();
    line.insert("at_ms".to_owned(), json!(crate::sys::now_millis()));
    line.insert("operation".to_owned(), json!(operation.as_str()));
    ABOUT.with(|about| {
        for (key, value) in about.borrow().iter() {
            line.insert(key.clone(), value.clone());
        }
    });
    Value::Object(line)
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
        let read = about(json!({"phase": "re-read", "node": "a"}), || {
            about(json!({"node": "b"}), || line(Operation::Dependencies))
        });
        assert_eq!(read["operation"], "task-dependencies");
        assert_eq!(read["phase"], "re-read");
        assert_eq!(read["node"], "b");
        let bare = line(Operation::Task);
        assert_eq!(bare.get("phase"), None, "{bare}");
        assert_eq!(bare.get("node"), None, "{bare}");
    }
}
