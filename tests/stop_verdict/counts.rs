//! What one counted call did, observed rather than timed: the git executions git's
//! counting shim saw, and the processes the binary itself started, read from `strace`.
//!
//! Parentage is the point of the process count: `git` starts helpers of its own, and
//! those are git's, not the binary's. So only the binary's **direct** children are
//! counted, across every thread of its process.
//!
//! [`EXPECTED`] is the one statement of what each counted call does. A change that moves
//! a count fails [`assert_expected`] naming the call and both numbers, and updates the
//! number here in its own diff.
//!
//! Files read are not counted: the binary's opens under the fixture's root held still on
//! one host but differed by a constant on CI's runner (27 fewer at scale 1, 6 on the host
//! shape, run 44989805907), so they say as much about the host as about the binary.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

/// One call's counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Counts {
    /// Git executions through the counting shim on `PATH`.
    pub git: u64,
    /// Processes the binary started directly, every one of its threads included.
    pub processes: u64,
}

/// One counted call: what it is, what it did, and the programs its children ran.
#[derive(Debug, Clone)]
pub(super) struct Observed {
    pub call: String,
    pub counts: Counts,
    pub programs: Vec<String>,
}

const fn counts(git: u64, processes: u64) -> Counts {
    Counts { git, processes }
}

/// Every counted call, in the order the journey makes them, and what it does.
pub(super) const EXPECTED: [(&str, Counts); 11] = [
    ("scale-1 none cold stop-guard", counts(55, 55)),
    ("scale-1 none warm stop-guard", counts(1, 1)),
    ("scale-1 owed cold stop-guard", counts(55, 55)),
    ("scale-1 owed warm stop-guard", counts(1, 1)),
    ("scale-1 unreadable cold stop-guard", counts(0, 0)),
    ("scale-1 unreadable warm stop-guard", counts(0, 0)),
    ("host-shaped unpublished --host", counts(11, 11)),
    ("host-shaped unpublished --acknowledge", counts(11, 11)),
    ("host-shaped none stop-guard", counts(11, 11)),
    ("host-shaped owed stop-guard", counts(11, 11)),
    ("host-shaped unreadable stop-guard", counts(0, 0)),
];

/// Fail, naming every call whose counts moved with its observed and expected numbers.
pub(super) fn assert_expected(observed: &[Observed]) {
    let calls: Vec<&str> = observed.iter().map(|seen| seen.call.as_str()).collect();
    let expected: Vec<&str> = EXPECTED.iter().map(|(call, _)| *call).collect();
    assert_eq!(
        calls, expected,
        "the counted calls are not the ones EXPECTED names"
    );
    let mut moved = Vec::new();
    for (seen, (_, want)) in observed.iter().zip(EXPECTED) {
        let mut programs: BTreeMap<&str, usize> = BTreeMap::new();
        for program in &seen.programs {
            *programs.entry(program.as_str()).or_default() += 1;
        }
        eprintln!(
            "stop-verdict counted {}: {} git execution(s), {} process(es) {programs:?}",
            seen.call, seen.counts.git, seen.counts.processes
        );
        for (what, got, wanted) in [
            ("git execution(s)", seen.counts.git, want.git),
            ("process(es) started", seen.counts.processes, want.processes),
        ] {
            if got != wanted {
                moved.push(format!(
                    "{}: observed {got} {what}, expected {wanted} (children ran {programs:?})",
                    seen.call
                ));
            }
        }
    }
    assert!(
        moved.is_empty(),
        "the stop hook's counted calls moved; a change that means to move a count updates its \
         expected number in tests/stop_verdict/counts.rs:\n{}",
        moved.join("\n")
    );
}

/// What one traced process did: the threads and processes it created, and the programs
/// it executed.
#[derive(Default)]
struct Traced {
    threads: Vec<u32>,
    children: Vec<u32>,
    executed: Vec<String>,
    opened: Vec<String>,
}

static DUMPED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Run `command` under `strace -ff` into `into`, with `stdin` written to it, and answer its
/// output and the programs its direct children executed, one entry per child.
pub(super) fn traced(
    command: &Command,
    stdin: Option<&[u8]>,
    into: &Path,
) -> (Output, Vec<String>) {
    let _ = std::fs::remove_dir_all(into);
    std::fs::create_dir_all(into).expect("a trace directory");
    let mut strace = Command::new("strace");
    strace
        .args([
            "-ff",
            "-qq",
            "-e",
            "trace=execve,clone,clone3,fork,vfork,open,openat,openat2",
            "-o",
        ])
        .arg(into.join("trace"))
        .arg(command.get_program())
        .args(command.get_args())
        .env_clear()
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(directory) = command.get_current_dir() {
        strace.current_dir(directory);
    }
    for (key, value) in command.get_envs() {
        if let Some(value) = value {
            strace.env(key, value);
        }
    }
    let mut child = strace.spawn().unwrap_or_else(|error| {
        panic!(
            "the counted calls run under strace, which would not start: {error}. Install strace \
             (`just _strace-preflight` names how), or run where ptrace is permitted"
        )
    });
    if let Some(stdin) = stdin {
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(stdin)
            .expect("the payload is written");
    }
    let output = child.wait_with_output().expect("the tracer runs");
    let program = Path::new(command.get_program())
        .file_name()
        .expect("a program name")
        .to_string_lossy()
        .into_owned();
    (output, children_of(into, &program))
}

/// The programs `program`'s direct children executed, from the per-thread traces in `into`.
fn children_of(into: &Path, program: &str) -> Vec<String> {
    let mut processes: BTreeMap<u32, Traced> = BTreeMap::new();
    for entry in std::fs::read_dir(into).expect("the traces") {
        let path = entry.expect("a trace").path();
        let Some(pid) = path
            .extension()
            .and_then(|pid| pid.to_str())
            .and_then(|pid| pid.parse::<u32>().ok())
        else {
            continue;
        };
        let mut traced = Traced::default();
        for line in std::fs::read_to_string(&path).expect("a trace").lines() {
            let created = line
                .rsplit_once("= ")
                .and_then(|(_, n)| n.trim().parse::<u32>().ok());
            if ["open(", "openat(", "openat2("].iter().any(|call| line.starts_with(call)) {
                if line.contains("O_RDONLY") && created.is_some() {
                    traced.opened.push(line.split('"').nth(1).unwrap_or("?").to_owned());
                }
            } else if line.starts_with("execve(\"") && line.trim_end().ends_with("= 0") {
                let executed = line.split('"').nth(1).expect("a path");
                traced.executed.push(
                    Path::new(executed)
                        .file_name()
                        .expect("a file name")
                        .to_string_lossy()
                        .into_owned(),
                );
            } else if ["clone(", "clone3(", "fork(", "vfork("]
                .iter()
                .any(|call| line.starts_with(call))
            {
                if let Some(created) = created {
                    if line.contains("CLONE_THREAD") {
                        traced.threads.push(created);
                    } else {
                        traced.children.push(created);
                    }
                }
            }
        }
        processes.insert(pid, traced);
    }
    let root = processes
        .iter()
        .find(|(_, traced)| traced.executed.first().map(String::as_str) == Some(program))
        .map(|(pid, _)| *pid)
        .unwrap_or_else(|| panic!("no traced process executed {program}"));
    let mut group = vec![root];
    let mut at = 0;
    while at < group.len() {
        group.extend(processes[&group[at]].threads.iter().copied());
        at += 1;
    }
    let mut programs = Vec::new();
    if let Some(dir) = std::env::var_os("STOP_VERDICT_READS_DIR") {
        let workload = into.parent().expect("a workload").display().to_string();
        let mut lines: Vec<String> = group
            .iter()
            .flat_map(|member| processes[member].opened.clone())
            .map(|path| path.replace(&workload, "<root>"))
            .collect();
        lines.sort();
        let n = DUMPED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        std::fs::create_dir_all(&dir).expect("dump dir");
        std::fs::write(
            std::path::Path::new(&dir).join(format!("{n:02}.txt")),
            lines.join("\n"),
        )
        .expect("dump");
    }
    for member in &group {
        for child in &processes[member].children {
            let ran = processes
                .get(child)
                .and_then(|child| child.executed.first())
                .cloned()
                .unwrap_or_else(|| "(no program)".to_owned());
            programs.push(ran);
        }
    }
    programs
}
