//! What one reconcile loop actually did, counted while it does it.
//!
//! A converged run's cost is that it does nothing, which a journal cannot show —
//! nothing is written — and which CPU time cannot measure, because a loaded host
//! hands that out as it likes. So the loop counts its own work as *work done*,
//! and a journey reads the counts.
//!
//! Counted **always**: a relaxed increment costs nothing measurable, and a
//! counter that exists only under a flag is one nothing has proven counts the
//! real path. **Written** only when [`STATS_ENV`] names a file.
//!
//! The counts are per **process**, and a journey takes a delta across an
//! interval rather than an absolute.

use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;

/// The environment variable asking a driver to report what its loop did.
///
/// Any non-empty value turns it on, and the counts go to [`STATS_FILE`] inside
/// the run's own directory — so a host measuring several drivers at once gets one
/// file per run rather than one file they overwrite in turn. Absent, which is
/// every run outside this repository's own journeys, nothing is written and
/// nothing is opened.
pub(crate) const STATS_ENV: &str = "ONEPIPELINE_LOOP_STATS";

/// Where in a run's directory those counts are written.
pub(crate) const STATS_FILE: &str = "loop-stats.json";

/// Scheduling passes: iterations of the reconcile body.
///
/// A wake that finds nothing to do and goes back to waiting is not one of these,
/// which is exactly the distinction the bound is stated in: what costs the host
/// is the body, and the wait around it is two `stat` calls.
static PASSES: AtomicU64 = AtomicU64::new(0);
static STATUSES: AtomicU64 = AtomicU64::new(0);
static PUBLICATIONS: AtomicU64 = AtomicU64::new(0);
static UPSTREAM_READS: AtomicU64 = AtomicU64::new(0);
static RELEASE_ASKS: AtomicU64 = AtomicU64::new(0);
/// Bytes read out of a run store by this process, whichever run's they came from
/// — this one's journal, or another's answering a cross-DAG edge.
static STORE_BYTES: AtomicU64 = AtomicU64::new(0);

pub(crate) fn pass() {
    PASSES.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn statuses_derived() {
    STATUSES.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn published() {
    PUBLICATIONS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn upstream_read() {
    UPSTREAM_READS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn release_asked() {
    RELEASE_ASKS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn store_read(bytes: u64) {
    STORE_BYTES.fetch_add(bytes, Ordering::Relaxed);
}

/// Whether this process was launched to report what its loop did.
fn asked() -> bool {
    std::env::var_os(STATS_ENV).is_some_and(|value| !value.is_empty())
}

/// Write the counts into the run's own directory, if this process was launched
/// to report them.
///
/// **A write that failed is returned rather than swallowed.** A run nobody asked
/// to measure never opens the file at all, so the only way here is a host that
/// asked this driver for the counts — and answering that with silence leaves the
/// caller reading a file that is absent or frozen at an earlier pass, with
/// nothing anywhere saying why. The error names the path and what the filesystem
/// said, and the driver hands it back the way it hands back any other write into
/// the run's own directory.
pub(crate) fn flush(paths: &crate::ledger::RunPaths) -> crate::error::Result<()> {
    if !asked() {
        return Ok(());
    }
    let document = json!({
        "passes": PASSES.load(Ordering::Relaxed),
        "statuses": STATUSES.load(Ordering::Relaxed),
        "publications": PUBLICATIONS.load(Ordering::Relaxed),
        "upstream_reads": UPSTREAM_READS.load(Ordering::Relaxed),
        "release_asks": RELEASE_ASKS.load(Ordering::Relaxed),
        "store_bytes": STORE_BYTES.load(Ordering::Relaxed),
    });
    crate::ledger::write_json(&paths.dir.join(STATS_FILE), &document)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_written_when_nobody_asked_for_it() {
        // The shipped configuration: the variable is unset, so a driver opens no
        // file at all. Asserted through the one function that decides it, because
        // the counters themselves are process-wide and a second test running
        // beside this one moves them.
        std::env::remove_var(STATS_ENV);
        assert!(!asked());
        std::env::set_var(STATS_ENV, "");
        assert!(!asked(), "an empty setting asks for nothing");
        std::env::remove_var(STATS_ENV);
        // And the flush is a no-op rather than a panic when nobody asked, which is
        // what makes counting free on every run but a measured one. It answers
        // `Ok` without looking at the directory, which here does not exist.
        let paths = crate::ledger::RunPaths::under(&std::env::temp_dir(), "nobody");
        flush(&paths).expect("an unmeasured run writes nothing and cannot fail");
        assert!(!paths.dir.join(STATS_FILE).exists());
    }

    #[test]
    fn every_count_a_journey_reads_is_written_under_its_own_name() {
        let root =
            std::env::temp_dir().join(format!("onepipeline-loopstats-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = crate::ledger::RunPaths::under(&root, "measured");
        paths.create().expect("the run directory");
        std::env::set_var(STATS_ENV, "1");
        pass();
        statuses_derived();
        published();
        upstream_read();
        release_asked();
        store_read(7);
        flush(&paths).expect("the counts are written");
        std::env::remove_var(STATS_ENV);
        let written: serde_json::Value = crate::ledger::read_json_opt(&paths.dir.join(STATS_FILE))
            .expect("the counts are written");
        // The names are the wire a journey reads by, so each is asserted present
        // and non-zero rather than the document compared whole: the counters are
        // process-wide and whatever else this binary ran has already moved them.
        for name in [
            "passes",
            "statuses",
            "publications",
            "upstream_reads",
            "release_asks",
            "store_bytes",
        ] {
            assert!(
                written[name].as_u64().is_some_and(|count| count > 0),
                "{name} is not a count in {written}"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
