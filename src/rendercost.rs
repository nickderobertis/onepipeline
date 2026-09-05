//! What one render of a view did **per node**, counted while it does it.
//!
//! A view that decides a node's landing when it renders pays for that decision
//! on every supervisory look, and what it costs a host is work rather than
//! seconds: a loaded machine hands out time as it likes, so a bound stated in
//! elapsed time fails correct work and passes a slow regression on an idle box.
//! So a render records its own work when it is asked to, and
//! `tests/e2e/landing.rs` reads the record back and holds the bound.
//!
//! The unit is one node's landing decision. [`deciding`] opens that scope, and
//! everything counted here — the sibling's published landing read, a read out of
//! the run store, a process this crate started — is attributed to the node whose
//! landing was being decided when it happened. An act outside such a scope is
//! not per-node work and is recorded by nothing: the journal read that
//! [`crate::views::RunView::open`] makes is one read for the whole render, and
//! counting it against a node would report it growing with the graph.
//!
//! Counted **always** — appending nothing costs nothing measurable — and
//! **written** only where [`RENDER_COST_ENV`] names a file, which is every run
//! of this repository's own journeys and no other run of this binary.

use std::cell::RefCell;
use std::io::Write;

use serde_json::json;

/// The environment variable asking a process to record what its renders did.
///
/// It names a **file**, which every render in the process appends a line to, so
/// a caller measuring one command points it at a path of that command's own.
/// Absent, nothing is opened and nothing is written.
pub(crate) const RENDER_COST_ENV: &str = "ONEPIPELINE_RENDER_COST";

thread_local! {
    /// The render this thread is inside, and the node whose landing it is
    /// currently deciding.
    ///
    /// A thread-local rather than a parameter threaded through every view: the
    /// two acts counted below happen in `crate::ledger` and `crate::sys`, which
    /// no render calls directly and which must not learn what a view is.
    static INSIDE: RefCell<Option<Inside>> = const { RefCell::new(None) };
}

/// Which view is rendering.
///
/// A closed set rather than a string: these are the three places a landing is
/// reported, and a fourth is a decision somebody makes here rather than a name
/// a caller can spell. `tests/e2e/landing.rs` reads them off the record by these
/// words, so they are the wire as well as the vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rendered {
    /// `onepipeline results` — the per-node lines.
    Results,
    /// The run summary's count of what has not landed.
    Summary,
    /// `onepipeline status`.
    Status,
}

impl Rendered {
    fn as_str(self) -> &'static str {
        match self {
            Rendered::Results => "results",
            Rendered::Summary => "summary",
            Rendered::Status => "status",
        }
    }
}

/// What a render did, one act at a time.
///
/// Closed for the reason [`Rendered`] is, and load-bearing in the same way: the
/// bound is "nothing per node but the landing read", which is a claim about this
/// set — an act nobody can spell is an act nobody can leave out of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Act {
    Began,
    Reported,
    LandingRead,
    RepositoryResolved,
    StoreRead,
    ProcessSpawn,
}

impl Act {
    fn as_str(self) -> &'static str {
        match self {
            Act::Began => "render",
            Act::Reported => "reported",
            Act::LandingRead => "landing-read",
            Act::RepositoryResolved => "repository-resolved",
            Act::StoreRead => "store-read",
            Act::ProcessSpawn => "process-spawn",
        }
    }
}

/// What a render is, while it renders.
// llmlint: ignore[invalid_states_unrepresentable] a run id and a node id are the plain strings this whole crate spells them as — `RunPaths.run`, `RunState::landings`, `RunState::branches` — for the reason `src/ledger.rs`'s file-level suppression records, and `src/AGENTS.md` names a `RunId` newtype as interface drift. Newtypes at these two sites alone would disagree with every neighbour and convert at every boundary. Neither value is unchecked: the run id crossed `ledger::is_valid_run_id` when the run was launched, and the node id is one the run's own journal projected into its graph.
struct Inside {
    /// Which view, so a reader of a file three renders appended to can tell them
    /// apart.
    view: Rendered,
    run: String,
    /// The node whose landing is being decided right now, if any.
    node: Option<String>,
}

/// Record one act, where this process was asked to record them.
///
/// A line per act, appended: a reader takes the file after the command exits, so
/// there is nothing to flush and a crashed render still says what it had done.
fn record(act: Act, detail: serde_json::Value) {
    let Some(path) = std::env::var_os(RENDER_COST_ENV).filter(|value| !value.is_empty()) else {
        return;
    };
    let line = INSIDE.with_borrow(|inside| {
        let inside = inside.as_ref()?;
        let mut line = json!({
            "act": act.as_str(),
            "view": inside.view.as_str(),
            "run": inside.run,
        });
        if let Some(node) = &inside.node {
            line["node"] = json!(node);
        }
        if let (Some(line), Some(detail)) = (line.as_object_mut(), detail.as_object()) {
            for (key, value) in detail {
                line.insert(key.clone(), value.clone());
            }
        }
        Some(line)
    });
    let Some(line) = line else {
        return;
    };
    // Best effort, and deliberately so: this is a measurement of a read-only
    // view, and a view that failed because a measurement file would not open
    // would be a worse thing than a measurement nobody got.
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}

/// One render of one view, held for as long as it renders.
///
/// **Not re-entrant**: `status` renders the run summary inside itself, and two
/// nested scopes would attribute the summary's reads to a render nobody ran. The
/// outermost scope is the render, and an inner one records nothing and restores
/// nothing.
pub(crate) struct Render {
    /// Whether this scope is the one that opened the render — an inner scope
    /// leaves the outer one exactly as it found it.
    outermost: bool,
}

/// Begin a render of `view` over `run`.
pub(crate) fn rendering(view: Rendered, run: &str) -> Render {
    let outermost = INSIDE.with_borrow_mut(|inside| {
        if inside.is_some() {
            return false;
        }
        *inside = Some(Inside {
            view,
            run: run.to_owned(),
            node: None,
        });
        true
    });
    if outermost {
        record(Act::Began, json!({}));
    }
    Render { outermost }
}

impl Render {
    /// Whether this scope is the render, rather than one nested inside it.
    ///
    /// Read by [`crate::views`], which empties its per-render memo of landing
    /// reads when a render opens: emptying it on a nested scope would ask every
    /// node a second time half-way through one render.
    pub(crate) fn outermost(&self) -> bool {
        self.outermost
    }

    /// Record that this render reported on one node's landing.
    ///
    /// The set every landing read is held against: a read for a node this render
    /// never reported on is a read nobody is shown, which is the one shape of
    /// waste the bound rules out by name.
    pub(crate) fn reported(&self, node: &str) {
        record(Act::Reported, json!({ "node": node }));
    }
}

impl Drop for Render {
    fn drop(&mut self) {
        if self.outermost {
            INSIDE.with_borrow_mut(|inside| *inside = None);
        }
    }
}

/// The scope in which one node's landing is decided.
pub(crate) struct Deciding {
    /// Whether this scope set the node, so an inner one cannot clear it.
    outermost: bool,
}

/// Attribute what happens next to one node's landing decision.
///
/// Not re-entrant, like [`rendering`]: the innermost decision is the one whose
/// acts would be miscounted, and a nested scope naming a second node would move
/// the first one's remaining acts onto it.
pub(crate) fn deciding(node: &str) -> Deciding {
    let outermost = INSIDE.with_borrow_mut(|inside| match inside.as_mut() {
        Some(inside) if inside.node.is_none() => {
            inside.node = Some(node.to_owned());
            true
        }
        _ => false,
    });
    Deciding { outermost }
}

impl Drop for Deciding {
    fn drop(&mut self) {
        if self.outermost {
            INSIDE.with_borrow_mut(|inside| {
                if let Some(inside) = inside.as_mut() {
                    inside.node = None;
                }
            });
        }
    }
}

/// Record that one landing decision made the sibling resolve its repository.
///
/// Named for what is certainly true rather than for what a caller hopes: the read
/// this crate calls loads the registry and resolves `repo` **before it reads
/// anything**, on every call, so one resolution per read is by construction. What
/// it goes on to open depends on the answer, and this does not claim to know.
///
/// Recorded *after* the read returns, so it records something that happened. It
/// is the quantity the render bound is about: a repository resolved once for each
/// node rather than once for the render, which is the sibling's limit and not this
/// crate's choice — see [`crate::vcs::landing_now`] and divergence 33.
pub(crate) fn repository_resolved(repo: Option<&str>) {
    record(Act::RepositoryResolved, json!({ "repo": repo }));
}

/// Record that the one read a landing decision is allowed to make was taken.
pub(crate) fn landing_read_taken(reference: &str, repo: Option<&str>) {
    record(
        Act::LandingRead,
        json!({ "reference": reference, "repo": repo }),
    );
}

/// Record a read out of a run's store, where one happened while a landing was
/// being decided.
///
/// Nothing is recorded outside such a scope: reading the journal once for the
/// whole render is what a view has always done, and the bound is about a read
/// that happens once *per node*.
pub(crate) fn store_read(bytes: u64) {
    if inside_a_decision() {
        record(Act::StoreRead, json!({ "bytes": bytes }));
    }
}

/// Record a process this crate started while a landing was being decided.
///
/// The seam every process this crate starts outside a dispatch goes through, so
/// a landing decided by walking a base's history — or by asking a host over the
/// network, which from here is a `gh` — is counted rather than merely forbidden
/// in prose.
pub(crate) fn process_spawned(program: &str) {
    if inside_a_decision() {
        record(Act::ProcessSpawn, json!({ "program": program }));
    }
}

fn inside_a_decision() -> bool {
    INSIDE.with_borrow(|inside| inside.as_ref().is_some_and(|inside| inside.node.is_some()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire this module writes, pinned against the reader that parses it.
    ///
    /// `tests/e2e/landing.rs` and `tests/e2e/harness.rs` name these words as
    /// string literals — a test binary cannot see a private module's constants —
    /// so this is the gate that keeps the two copies one vocabulary. The matches
    /// are **exhaustive on purpose**: a variant added without a word here fails to
    /// compile, and a word changed without the journey fails below.
    #[test]
    fn every_word_this_record_is_read_by_is_the_word_it_writes() {
        assert_eq!(
            RENDER_COST_ENV, "ONEPIPELINE_RENDER_COST",
            "the variable `tests/e2e/harness.rs` sets was renamed"
        );
        for (view, word) in [
            (Rendered::Results, "results"),
            (Rendered::Summary, "summary"),
            (Rendered::Status, "status"),
        ] {
            // Exhaustive, so a fourth view cannot be added without a word.
            match view {
                Rendered::Results | Rendered::Summary | Rendered::Status => {}
            }
            assert_eq!(view.as_str(), word, "{view:?} is read by another name");
        }
        for (act, word) in [
            (Act::Began, "render"),
            (Act::Reported, "reported"),
            (Act::LandingRead, "landing-read"),
            (Act::RepositoryResolved, "repository-resolved"),
            (Act::StoreRead, "store-read"),
            (Act::ProcessSpawn, "process-spawn"),
        ] {
            match act {
                Act::Began
                | Act::Reported
                | Act::LandingRead
                | Act::RepositoryResolved
                | Act::StoreRead
                | Act::ProcessSpawn => {}
            }
            assert_eq!(act.as_str(), word, "{act:?} is read by another name");
        }
    }

    /// One lock over the two tests below, because the variable they set and
    /// clear is process-wide: each would otherwise turn the other's recording
    /// off half-way through it.
    static MEASURING: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn scratch(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "onepipeline-rendercost-{name}-{}",
            std::process::id()
        ))
    }

    /// Nothing is written by a process nobody asked to measure.
    ///
    /// Asserted over the whole guard, because every act goes through
    /// [`record`]: a build that opened the file before it read the variable
    /// would write into whatever path an operator's environment carried.
    #[test]
    fn a_process_nobody_asked_to_measure_writes_nothing() {
        let _held = MEASURING.lock().unwrap_or_else(|held| held.into_inner());
        let path = scratch("unmeasured");
        let _ = std::fs::remove_file(&path);
        std::env::remove_var(RENDER_COST_ENV);
        {
            let render = rendering(Rendered::Results, "unmeasured");
            render.reported("node");
            let _deciding = deciding("node");
            landing_read_taken("branch", Some("repo"));
            store_read(7);
            process_spawned("git");
        }
        assert!(
            !path.exists(),
            "an unmeasured render opened {}",
            path.display()
        );
    }

    /// Every act a render performs is attributed to the node it was performed
    /// for, and an act outside a decision is not per-node work at all.
    #[test]
    fn each_act_is_attributed_to_the_node_whose_landing_was_being_decided() {
        let _held = MEASURING.lock().unwrap_or_else(|held| held.into_inner());
        let path = scratch("acts");
        let _ = std::fs::remove_file(&path);
        std::env::set_var(RENDER_COST_ENV, &path);
        {
            let render = rendering(Rendered::Status, "measured");
            render.reported("alpha");
            // Outside any decision: one read the whole render made once.
            store_read(11);
            {
                let _deciding = deciding("alpha");
                landing_read_taken("alpha-branch", None);
                store_read(3);
                process_spawned("git");
                // An inner scope names no second node.
                let _nested = deciding("beta");
                landing_read_taken("beta-branch", None);
            }
            // A nested render records nothing of its own.
            let _inner = rendering(Rendered::Summary, "measured");
        }
        std::env::remove_var(RENDER_COST_ENV);
        let written = std::fs::read_to_string(&path).expect("the record is written");
        let acts: Vec<serde_json::Value> = written
            .lines()
            .map(|line| serde_json::from_str(line).expect("a JSON line"))
            .filter(|act: &serde_json::Value| act["run"] == "measured")
            .collect();
        let kinds: Vec<&str> = acts
            .iter()
            .map(|act| act["act"].as_str().expect("every act is named"))
            .collect();
        assert_eq!(
            kinds,
            vec![
                "render",
                "reported",
                "landing-read",
                "store-read",
                "process-spawn",
                "landing-read"
            ],
            "{written}"
        );
        assert!(
            acts.iter().all(|act| act["view"] == "status"),
            "a nested render renamed the one already open: {written}"
        );
        // The read taken outside a decision is not per-node work and is
        // recorded by nothing.
        assert_eq!(
            acts.iter().filter(|act| act["act"] == "store-read").count(),
            1,
            "{written}"
        );
        for act in acts.iter().skip(1) {
            assert_eq!(act["node"], "alpha", "{act}");
        }
        let _ = std::fs::remove_file(&path);
    }
}
