//! What the subprocess doubles share.
//!
//! These are **doubles for what is outside the crate under test**, never for
//! anything inside it, and never for a library it links — there is no `onevcs`
//! double here, because that sibling is called rather than spawned. One is a
//! real executable speaking `oneagentgraph`'s command surface, so the code under
//! test composes it exactly as it composes the real one, by executing a program
//! and reading its stdout. The other three stand further out than a sibling:
//! Claude Code's headless surface, at oneharness's own
//! `ONEHARNESS_BIN_CLAUDE_CODE`, which is where the paid model turn is replaced
//! for the journeys that drive the real `oneagentgraph`; `oneharness`'s, at the
//! process boundary that sibling still has; and `gh`'s, at `onevcs`'s own
//! override, for the journeys that need a host to decide something without a
//! network or a credential.
//!
//! Each is scripted from a directory the test prepares: what a node's dispatch
//! does, whether it waits for a rendezvous, and what it exits with are all files
//! on disk, so a test states its scenario instead of arranging call
//! expectations.

use std::path::{Path, PathBuf};

/// The environment variable naming the directory a double is scripted from.
pub const SCRIPT_DIR_ENV: &str = "ONEPIPELINE_FAKE_DIR";

/// The environment variable naming the run a launch or a dispatch belongs to.
///
/// The doubles read it out of their own environment, because that is where the
/// processes they stand in for read it: an observer addresses the run it was
/// started for, and a worker addresses the run it may ask its manager on.
// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] the crate under test declares
// this key in a module `src/lib.rs` keeps private, so there is no item to import and no
// source to share. The reconciling gate is a journey, as it is for `MODES` in
// `fake-claude.rs`: a spelling that drifted leaves [`ask_manager`] with no run to ask on,
// and the journeys that read the question off the channel fail.
pub const RUN_ID_ENV: &str = "ONEPIPELINE_RUN_ID";

/// The environment variable naming the `onepipeline` executable a double asks
/// its manager through.
///
/// The operator's `ask-manager` wrapper finds it on the `PATH`; a double is told
/// where it is, because the binary under test is built to a path this suite
/// holds a private name for and the directory it sits in holds every other
/// binary cargo built too.
pub const CLI_BIN_ENV: &str = "ONEPIPELINE_FAKE_CLI_BIN";

/// The environment variable a member's own harness config stamps its name into.
///
/// A single-sided member's turn is a library call inside `oneagentgraph`, so the
/// harness oneharness spawns for it is handed no member name and the run
/// publishes no argv to read one off. What it *is* handed is that member's
/// resolved oneharness config `[env]` block, which is where a journey writing one
/// puts this key so the turn can say which member it was.
pub const MEMBER_ENV: &str = "ONEPIPELINE_FAKE_MEMBER";

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
    flags(args, name).into_iter().next()
}

/// Every value of a repeatable `--flag VALUE` pair, in the order they were
/// given.
///
/// A named flag with nothing after it is refused rather than skipped. Read
/// leniently it is indistinguishable from the flag never having been passed —
/// so a caller that stopped sending a value would look, to every assertion in
/// the suite, exactly like one that still sent it correctly.
pub fn flags(args: &[String], name: &str) -> Vec<String> {
    args.iter()
        .enumerate()
        .filter(|(_, arg)| arg.as_str() == name)
        .map(|(at, _)| match args.get(at + 1) {
            Some(value) => value.clone(),
            None => fail(&format!("{name} was given with no value after it")),
        })
        .collect()
}

/// One reserved label's value, from the `--label k=v` pairs.
pub fn label(args: &[String], key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    flags(args, "--label")
        .into_iter()
        .find_map(|pair| pair.strip_prefix(&prefix).map(str::to_string))
}

/// One externally-supplied name, as a single path segment.
///
/// A double is handed node ids and session tokens that came off a plan or a
/// command line, and it writes files named after them. Interpolated raw, a name
/// carrying a separator or a `..` would put a double's scratch outside the
/// directory the test gave it. So exactly three characters survive beside
/// letters and digits — `-`, `_`, and nothing else — which leaves neither a
/// separator nor a dot to build one out of, and an empty result gets a name of
/// its own rather than resolving to the directory itself.
pub fn segment(name: &str) -> String {
    let mapped: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if mapped.is_empty() {
        "unnamed".to_string()
    } else {
        mapped
    }
}

/// A per-node script file, e.g. `build.fail`.
pub fn node_script(dir: &Path, node: &str, suffix: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(format!("{node}.{suffix}")))
        .ok()
        .map(|text| text.trim().to_string())
}

/// Keep working through the ask to stop, as a wedged worker does.
///
/// The one dispatch behaviour a rendezvous cannot act out. A teardown's polite
/// ask is `SIGTERM`, whose default action ends the process, so every other
/// double here goes the instant it is signalled — and a suite where nothing
/// survives the ask cannot tell a stop that *ended* a run's tree from one that
/// only signalled it and walked away. This is the second half of that pair; the
/// forceful ask still ends it, because `SIGKILL` cannot be handled.
///
/// A no-op off Unix, where the teardown draws no such distinction: `taskkill`
/// is asked forcefully in both modes, for the reason `sys::platform_stop`
/// records there.
/// The one failure here is **fatal to the double**, and deliberately loud: a
/// dispatch that was asked to survive `SIGTERM` and did not install the
/// disposition dies at the first ask, and the journey around it then proves the
/// opposite of what it says — that a stop ended a tree that would have gone
/// anyway. There is nothing to recover to, so the double says so and stops.
pub fn ignore_the_polite_ask() {
    #[cfg(unix)]
    {
        // SAFETY: `signal` sets this process's disposition for one signal and
        // borrows nothing; `SIG_IGN` is a valid disposition for `SIGTERM`.
        let installed = unsafe { libc::signal(libc::SIGTERM, libc::SIG_IGN) };
        // llmlint: ignore[no_panics_on_recoverable_errors] there is nothing to
        // recover to here, which is the paragraph above rather than an oversight.
        // This double's whole contract is to still be running after `SIGTERM`, so a
        // host that refuses the disposition has taken away the only behaviour the
        // call has; returning would leave it dying at the first ask while the
        // journey around it recorded that a stop had ended a wedged tree. Nor is
        // there a caller to propagate to: the one call site is `fake-oneagentgraph`
        // acting out `<key>.ignores-the-ask`, which has no other way to be that
        // worker. Failing loudly at the disposition is what keeps the journey from
        // proving the opposite of what it claims.
        assert!(
            installed != libc::SIG_ERR,
            "this double was asked to work through a polite stop and this host would not let it \
             ignore SIGTERM: {}",
            std::io::Error::last_os_error()
        );
    }
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
    wait_for_any(std::slice::from_ref(&path.to_path_buf()));
}

/// Wait until **any** of several rendezvous files appears.
///
/// The second one is what a turn that stops when it is asked to looks like: it
/// is held open like any other dispatch, and the redirection an `interrupt`
/// delivers is itself what releases it. A hold that only the test could release
/// can act out a worker that ignores the ask, and nothing else — so a suite with
/// one form of hold cannot tell a dispatch that stopped politely from one that
/// had to be reaped.
///
/// Bounded exactly as [`wait_for`] is, and for the same reason: an expired
/// rendezvous ends the process rather than continuing as though it had been
/// released.
pub fn wait_for_any(paths: &[PathBuf]) {
    // A hold with no clock of its own: `Duration::MAX` never elapses, so the
    // tick below never fires and this waits exactly as it always has.
    wait_for_any_ticking(paths, std::time::Duration::MAX, &mut || {});
}

/// [`wait_for_any`], running `tick` about every `every` for as long as it waits.
///
/// What a dispatch that goes on *saying* something while it holds needs: a real
/// harness publishes its liveness whether or not the turn is doing anything, and
/// a double that could only hold silently cannot act out a wedged worker at all.
/// The bound is unchanged — an unreleased hold still ends the process.
pub fn wait_for_any_ticking(paths: &[PathBuf], every: std::time::Duration, tick: &mut dyn FnMut()) {
    let timeout = rendezvous_timeout();
    // Checked, because adding to an `Instant` panics on overflow and a panic
    // here is the one failure this file cannot report: it unwinds out of a
    // double whose whole contract is to say what was misconfigured and exit.
    // The bound below makes an overflow impossible on any clock this suite
    // runs on, so this reports a clock that cannot hold the hold at all rather
    // than standing in for that bound.
    let deadline = match std::time::Instant::now().checked_add(timeout) {
        Some(deadline) => deadline,
        None => fail(&format!(
            "a hold of {} seconds is further ahead than this clock can represent",
            timeout.as_secs()
        )),
    };
    let mut ticked = std::time::Instant::now();
    while std::time::Instant::now() < deadline {
        if paths.iter().any(|path| path.exists()) {
            return;
        }
        if ticked.elapsed() >= every {
            tick();
            ticked = std::time::Instant::now();
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let named: Vec<String> = paths
        .iter()
        .map(|path| path.display().to_string())
        .collect();
    fail(&format!(
        "no rendezvous of {} ever appeared: nothing released this dispatch",
        named.join(", ")
    ));
}

/// The longest hold this double will wait out.
///
/// A rendezvous is a test holding a dispatch open while it does something else,
/// so an hour is already far longer than any suite waits — a scripted value past
/// it is a mistyped number rather than a longer test. Bounding it is also what
/// keeps the deadline representable: seconds arrive off the environment, and an
/// arbitrarily large number of them is a duration no clock can be advanced by.
const MAX_RENDEZVOUS_SECONDS: u64 = 60 * 60;

/// How long a hold waits before it gives up.
///
/// A scripted value that is not a number of seconds inside that bound is refused
/// rather than defaulted or clamped: `0` would make every hold expire before it
/// began, which is the silent opposite of what a test asking for one means, and
/// a value past the ceiling is a scenario nobody wrote — either way the double
/// says which value it was handed and exits, the way it reports every other
/// misconfiguration.
fn rendezvous_timeout() -> std::time::Duration {
    let seconds = match std::env::var("ONEPIPELINE_FAKE_RENDEZVOUS_SECONDS") {
        Err(_) => 30,
        Ok(value) => match value.trim().parse::<u64>() {
            Ok(seconds) if seconds > 0 && seconds <= MAX_RENDEZVOUS_SECONDS => seconds,
            _ => fail(&format!(
                "ONEPIPELINE_FAKE_RENDEZVOUS_SECONDS holds {value:?}, which is not a \
                 number of seconds between 1 and {MAX_RENDEZVOUS_SECONDS}"
            )),
        },
    };
    std::time::Duration::from_secs(seconds)
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

/// Put one question to this run's manager, the way a dispatched agent does.
///
/// The operator's `ask-manager` wrapper is a dispatched agent's one supported
/// way to stop and ask: it reads the run out of **its own environment** — it is
/// told none and infers none, and refuses without one — and puts the question on
/// that run's channel. So this does that, through the real `onepipeline` and the
/// same verb an operator raises a surface with; a double that wrote the queue
/// itself would leave a journey asserting against a file rather than against a
/// question its manager can read.
///
/// A refusal ends the dispatch, loudly. An ask that did not land leaves the
/// journey around it waiting on a question nobody put, and why it did not land
/// belongs in that node's own evidence rather than in a gap.
pub fn ask_manager(question: &str) {
    let run = match std::env::var(RUN_ID_ENV) {
        Ok(run) if !run.is_empty() => run,
        _ => fail(&format!(
            "{RUN_ID_ENV} is unset: no run to ask a manager on"
        )),
    };
    let cli = match std::env::var(CLI_BIN_ENV) {
        Ok(cli) if !cli.is_empty() => cli,
        _ => fail(&format!(
            "{CLI_BIN_ENV} is unset: no channel to put a question on"
        )),
    };
    let asked = std::process::Command::new(&cli)
        .args(["surface", &run, "--kind", "check-in", "--message", question])
        .stdin(std::process::Stdio::null())
        .output();
    match asked {
        Err(error) => fail(&format!("cannot run `{cli} surface {run}`: {error}")),
        Ok(asked) if !asked.status.success() => fail(&format!(
            "`onepipeline surface {run}` was refused, so this dispatch has no manager to \
             ask: {}",
            String::from_utf8_lossy(&asked.stderr).trim()
        )),
        Ok(_) => {}
    }
}

/// Act as the dag-scope graph's monitor member: observe, and change nothing.
///
/// Exactly what the shipped `monitor` persona is for. It runs **no engine
/// verb** — there are none, and nothing an agent does makes a run advance — so
/// what it acts out is watching: it records that it saw the run and that the
/// run's ledger was there to read, holds if a test asks it to, and returns.
///
/// Shared by both doubles, because the monitor is a *member* rather than a
/// graph: the `oneagentgraph` double acts it out when it is standing in for the
/// whole sibling, and the harness double acts it out when the real sibling is
/// running the graph and only the paid turn is being replaced.
pub fn observe(dir: &Path) -> std::process::ExitCode {
    // Required, not defaulted: an empty run id would leave every assertion
    // about what this observer saw pointing at a run named by nothing.
    let run = match std::env::var(RUN_ID_ENV) {
        Ok(run) if !run.is_empty() => run,
        _ => fail(&format!("{RUN_ID_ENV} is unset: no run to observe")),
    };
    // The first thing a real monitor member does is read the run's ledger, so
    // the first thing this one records is whether that ledger was there to be
    // read. A launcher that started its observer before writing the launch
    // record leaves this `false`.
    append(
        &dir.join("observer-saw.jsonl"),
        &serde_json::json!({
            "run": run,
            "launch_record": launch_record(&run).is_some_and(|path| path.is_file()),
        })
        .to_string(),
    );

    if dir.join("observer.wait").exists() {
        wait_for(&dir.join("observer.go"));
    }
    std::process::ExitCode::SUCCESS
}

/// The launch record of the run this observer was started for.
///
/// Resolved from the same two variables the launcher hands every launched
/// graph, so the probe above reads exactly the file the engine loop opens
/// first.
fn launch_record(run: &str) -> Option<PathBuf> {
    let root = std::env::var("ONEPIPELINE_RUNS_DIR").ok()?;
    (!root.is_empty() && !run.is_empty()).then(|| PathBuf::from(root).join(run).join("launch.json"))
}
