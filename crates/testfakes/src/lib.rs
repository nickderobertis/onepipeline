//! What the subprocess doubles share.
//!
//! These are **doubles for what is outside the crate under test**, never for
//! anything inside it. Two of them are real executables speaking
//! `oneagentgraph`'s and `onevcs`'s command surfaces, so the code under test
//! composes them exactly as it composes the real ones — by executing a program
//! and reading its stdout. The third stands one layer further out: it speaks
//! `oneharness`'s surface, for the journeys that drive the real `oneagentgraph`
//! and need only the paid model turn replaced.
//!
//! Each is scripted from a directory the test prepares: what a node's dispatch
//! does, whether it waits for a rendezvous, and what it exits with are all files
//! on disk, so a test states its scenario instead of arranging call
//! expectations.

use std::path::{Path, PathBuf};

/// The environment variable naming the directory a double is scripted from.
pub const SCRIPT_DIR_ENV: &str = "ONEPIPELINE_FAKE_DIR";

/// The directory this double reads its script from and records into.
///
/// A double with no script directory has nothing to act out, which is a
/// misconfigured test rather than a scenario — so it is reported and the
/// process exits, the way any program does when its configuration is missing.
pub fn script_dir() -> PathBuf {
    match std::env::var(SCRIPT_DIR_ENV) {
        Ok(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => fail(&format!(
            "{SCRIPT_DIR_ENV} is unset: no scenario to act out"
        )),
    }
}

/// Report a configuration failure and exit, rather than unwinding.
pub fn fail(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(78);
}

/// The exit code a double answers a command line it does not speak with.
pub const USAGE: u8 = 64;

/// Refuse a command line this double does not speak.
///
/// A double that succeeds on anything is a weak oracle: the crate under test
/// could reach the sibling with a verb the real one has never had, or leave a
/// required argument off, and the suite would go green on it. The real CLIs
/// refuse both, so these do too.
pub fn refuse(message: &str) -> std::process::ExitCode {
    eprintln!("{message}");
    std::process::ExitCode::from(USAGE)
}

/// A required positional argument, or a refusal naming what was missing.
pub fn required(
    args: &[String],
    at: usize,
    name: &str,
) -> std::result::Result<String, std::process::ExitCode> {
    args.get(at)
        .filter(|value| !value.is_empty())
        .cloned()
        .ok_or_else(|| refuse(&format!("missing required argument {name}")))
}

/// Record one invocation, so a test can assert on what it was asked for.
pub fn record(dir: &Path, tool: &str, args: &[String]) {
    let line = serde_json::json!({"tool": tool, "args": args}).to_string();
    append(&dir.join("invocations.jsonl"), &line);
}

/// Append one line to a file, creating it if needed.
///
/// A double that cannot record what it was asked for would let a test assert
/// against a gap, so the failure ends the process instead.
pub fn append(path: &Path, line: &str) {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let opened = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path);
    // One `write_all` of the record and its terminator: several doubles record
    // into this file at once, and `writeln!` writes the text and the newline
    // separately, so a second appender in between tears the line and the test
    // reading it sees a gap rather than a dispatch.
    let written = opened.and_then(|mut file| file.write_all(format!("{line}\n").as_bytes()));
    if let Err(error) = written {
        fail(&format!("cannot record into {}: {error}", path.display()));
    }
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
/// Bounded, and the bound **ends this process**. Returning instead would let a
/// hold nobody released continue as though it had been: the dispatch completes,
/// the node settles, the run settles, and the test fails several steps later on
/// something that reads like a real defect — a reply refused because the run it
/// named had settled. That disguise cost this suite two rounds of diagnosis, so
/// an expired rendezvous now says so and takes the dispatch with it.
pub fn wait_for(path: &Path) {
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(rendezvous_timeout_seconds());
    while std::time::Instant::now() < deadline {
        if path.exists() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    fail(&format!(
        "rendezvous {} never appeared: nothing released this dispatch",
        path.display()
    ));
}

/// How long a hold waits before it gives up.
///
/// A scripted value that is not a positive number of seconds is refused rather
/// than defaulted: `0` would make every hold expire before it began, which is
/// the silent opposite of what a test asking for one means.
fn rendezvous_timeout_seconds() -> u64 {
    match std::env::var("ONEPIPELINE_FAKE_RENDEZVOUS_SECONDS") {
        Err(_) => 30,
        Ok(value) => match value.trim().parse() {
            Ok(seconds) if seconds > 0 => seconds,
            _ => fail(&format!(
                "ONEPIPELINE_FAKE_RENDEZVOUS_SECONDS holds {value:?}, \
                 which is not a positive number of seconds"
            )),
        },
    }
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

/// Act as the dag-scope graph's orchestrator member: drive the engine verbs.
///
/// Exactly what the shipped `orchestrator` persona instructs — `round run` then
/// `round next`, and nothing else that changes run state — so `start`'s journey
/// is exercised through the same commands a real orchestrator would use.
///
/// Shared by both doubles, because the orchestrator is a *member* rather than a
/// graph: the `oneagentgraph` double acts it out when it is standing in for the
/// whole sibling, and the `oneharness` double acts it out when the real sibling
/// is running the graph and only the paid turn is being replaced.
pub fn drive(dir: &Path) -> std::process::ExitCode {
    // Required, not defaulted: an empty run id would send every engine verb at
    // a run named by nothing, and the refusals that came back would read as the
    // engine's fault rather than as the launcher never having said which run
    // this driver is for.
    let run = match std::env::var("ONEPIPELINE_RUN_ID") {
        Ok(run) if !run.is_empty() => run,
        _ => fail("ONEPIPELINE_RUN_ID is unset: no run to drive"),
    };
    let binary = match std::env::var("ONEPIPELINE_FAKE_DRIVER_BIN") {
        Ok(binary) if !binary.is_empty() => binary,
        _ => fail("ONEPIPELINE_FAKE_DRIVER_BIN is unset: nothing to drive the run with"),
    };
    // The first thing a real orchestrator member does is read the run's ledger,
    // so the first thing this one records is whether that ledger was there to
    // be read. A launcher that started its driver before writing the launch
    // record leaves this `false` — and the driver then dies on a file its own
    // launcher had not written yet, with the run stuck at `run-started`.
    append(
        &dir.join("driver-saw.jsonl"),
        &serde_json::json!({
            "run": run,
            "launch_record": launch_record(&run).is_some_and(|path| path.is_file()),
        })
        .to_string(),
    );

    if dir.join("driver.wait").exists() {
        wait_for(&dir.join("driver.go"));
    }
    // Bounded: a graph that never settles ends this driver rather than
    // spinning, so a test fails on its own assertion instead of hanging.
    for _ in 0..8 {
        // Inherited on purpose: an attached launcher relays what the engine
        // verb says, and a real orchestrator member does not swallow it. What
        // it *exited* with is recorded, because that is the fact this driver
        // then acts on and nothing else in the tree writes it down.
        let ran = std::process::Command::new(&binary)
            .args(["round", "run", &run])
            .status();
        let Ok(ran) = ran else {
            return std::process::ExitCode::from(1);
        };
        append(
            &dir.join("driver-saw.jsonl"),
            &serde_json::json!({"run": run, "round_run": ran.code()}).to_string(),
        );
        // A round that settled unfinished is the planner's move, not the
        // orchestrator's: it never re-dispatches a failed node on its own
        // judgement. The shipped persona says the same thing in prose.
        if !ran.success() {
            return std::process::ExitCode::SUCCESS;
        }
        let next = std::process::Command::new(&binary)
            .args(["round", "next", &run])
            .output();
        let Ok(next) = next else {
            return std::process::ExitCode::from(1);
        };
        if !next.status.success() {
            append(
                &dir.join("driver-saw.jsonl"),
                &serde_json::json!({
                    "run": run,
                    "round_next": next.status.code(),
                    "stderr": String::from_utf8_lossy(&next.stderr),
                })
                .to_string(),
            );
            return std::process::ExitCode::from(1);
        }
        // `continuing` is the only answer that means there is another round to
        // run. Complete, and every gated state, ends the driver.
        if !String::from_utf8_lossy(&next.stdout).contains("\"continuing\"") {
            return std::process::ExitCode::SUCCESS;
        }
    }
    std::process::ExitCode::SUCCESS
}

/// The launch record of the run this driver was started for.
///
/// Resolved from the same two variables the launcher hands every driver, so the
/// probe above reads exactly the file `onepipeline round run` opens first.
fn launch_record(run: &str) -> Option<PathBuf> {
    let root = std::env::var("ONEPIPELINE_RUNS_DIR").ok()?;
    (!root.is_empty() && !run.is_empty()).then(|| PathBuf::from(root).join(run).join("launch.json"))
}
