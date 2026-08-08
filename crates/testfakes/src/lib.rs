//! What the two subprocess doubles share.
//!
//! `onepipeline` composes `oneagentgraph` and `onevcs` by running their CLIs, so
//! the honest way to drive it end to end is against real executables speaking
//! those command surfaces. These doubles are scripted from a directory the test
//! prepares: what a node's dispatch does, whether it waits for a rendezvous,
//! and what it exits with are all files on disk, so a test states its scenario
//! rather than mocking the seam under test.

use std::path::{Path, PathBuf};

/// The environment variable naming the directory a double is scripted from.
pub const SCRIPT_DIR_ENV: &str = "ONEPIPELINE_FAKE_DIR";

/// The directory this double reads its script from and records into.
///
/// Panicking is right here: a double with no script directory has nothing to
/// say, and failing loudly beats a test that silently exercises a default.
pub fn script_dir() -> PathBuf {
    PathBuf::from(
        std::env::var(SCRIPT_DIR_ENV)
            .unwrap_or_else(|_| panic!("{SCRIPT_DIR_ENV} is unset: no scenario to act out")),
    )
}

/// Record one invocation, so a test can assert on what it was asked for.
pub fn record(dir: &Path, tool: &str, args: &[String]) {
    let line = serde_json::json!({"tool": tool, "args": args}).to_string();
    append(&dir.join("invocations.jsonl"), &line);
}

/// Append one line to a file, creating it if needed.
pub fn append(path: &Path, line: &str) {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap_or_else(|e| panic!("cannot record into {}: {e}", path.display()));
    writeln!(file, "{line}").expect("the record is written");
}

/// The value of a `--flag VALUE` pair, if it was given.
pub fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|at| args.get(at + 1))
        .cloned()
}

/// Every value of a repeatable `--flag k=v` pair.
pub fn flags(args: &[String], name: &str) -> Vec<String> {
    args.iter()
        .enumerate()
        .filter(|(_, arg)| arg.as_str() == name)
        .filter_map(|(at, _)| args.get(at + 1).cloned())
        .collect()
}

/// One reserved label's value, from the `--label k=v` pairs.
pub fn label(args: &[String], key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    flags(args, "--label")
        .into_iter()
        .find_map(|pair| pair.strip_prefix(&prefix).map(str::to_string))
}

/// A per-node script file, e.g. `build.fail`.
pub fn node_script(dir: &Path, node: &str, suffix: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(format!("{node}.{suffix}")))
        .ok()
        .map(|text| text.trim().to_string())
}

/// Wait until a rendezvous file appears, so a test can hold a dispatch open
/// while it does something else — issue a live edit, kill a driver, read a
/// surface.
///
/// Bounded, so a test that never releases the rendezvous fails as a timeout
/// rather than hanging the suite.
pub fn wait_for(path: &Path) {
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(rendezvous_timeout_seconds());
    while std::time::Instant::now() < deadline {
        if path.exists() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    eprintln!("rendezvous {} never appeared", path.display());
}

fn rendezvous_timeout_seconds() -> u64 {
    std::env::var("ONEPIPELINE_FAKE_RENDEZVOUS_SECONDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(30)
}

/// An RFC 3339 millisecond UTC timestamp, in the envelope's one format.
pub fn now() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let secs = millis / 1_000;
    let ms = millis % 1_000;
    let days = (secs / 86_400) as i64;
    let sod = secs % 86_400;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    format!(
        "{year:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{ms:03}Z",
        sod / 3_600,
        (sod % 3_600) / 60,
        sod % 60
    )
}
