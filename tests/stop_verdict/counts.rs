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
//! Files read are the binary's own threads' read-only opens under the workload's root — the
//! fixture host's onevcs home, repositories, sessions and run roots — which is the data the
//! stop hook scans. Opens outside it (the dynamic loader, `/etc/gitconfig`, `/proc`) depend on
//! the host, and so does one inside it: a clone's `.git/branches`, which git's templates create
//! on some versions (2.43 here) and not on CI's runner, so it is opened 27 times at scale 1 and
//! 6 times on the host shape on one host and never on the other (runs 44989805907 and
//! 45011807472). Every other read is the same on both.

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
    /// Files and directories the binary's own threads opened to read under the workload's
    /// root, a clone's `.git/branches` aside.
    pub files: u64,
}

/// One counted call: what it is, what it did, and the programs its children ran.
#[derive(Debug, Clone)]
pub(super) struct Observed {
    pub call: String,
    pub counts: Counts,
    pub programs: Vec<String>,
}

/// What [`traced`] read off one call's traces.
pub(super) struct Trace {
    /// The program each direct child executed, one entry per child.
    pub programs: Vec<String>,
    /// The opens [`Counts::files`] counts.
    pub files: u64,
}

const fn counts(git: u64, processes: u64, files: u64) -> Counts {
    Counts {
        git,
        processes,
        files,
    }
}

/// Every counted call, in the order the journey makes them, and what it does.
pub(super) const EXPECTED: [(&str, Counts); 11] = [
    ("scale-1 none cold stop-guard", counts(55, 55, 6440)),
    ("scale-1 none warm stop-guard", counts(1, 1, 2743)),
    ("scale-1 owed cold stop-guard", counts(55, 55, 6439)),
    ("scale-1 owed warm stop-guard", counts(1, 1, 2742)),
    ("scale-1 unreadable cold stop-guard", counts(0, 0, 50)),
    ("scale-1 unreadable warm stop-guard", counts(0, 0, 50)),
    ("host-shaped unpublished --host", counts(11, 11, 4436)),
    (
        "host-shaped unpublished --acknowledge",
        counts(11, 11, 5588),
    ),
    ("host-shaped none stop-guard", counts(11, 11, 3956)),
    ("host-shaped owed stop-guard", counts(11, 11, 3956)),
    ("host-shaped unreadable stop-guard", counts(0, 0, 50)),
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
            "stop-verdict counted {}: {} git execution(s), {} process(es) {programs:?}, {} \
             file(s) read",
            seen.call, seen.counts.git, seen.counts.processes, seen.counts.files
        );
        for (what, got, wanted) in [
            ("git execution(s)", seen.counts.git, want.git),
            ("process(es) started", seen.counts.processes, want.processes),
            ("file(s) read", seen.counts.files, want.files),
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

/// What one traced process did: the threads and processes it created, the programs it
/// executed, and the opens [`Counts::files`] counts.
#[derive(Default)]
struct Traced {
    threads: Vec<u32>,
    children: Vec<u32>,
    executed: Vec<String>,
    reads: u64,
}

/// Run `command` under `strace -ff` into `into`, with `stdin` written to it, and answer its
/// output and what its traces show, its reads counted under `workload` alone.
pub(super) fn traced(
    command: &Command,
    stdin: Option<&[u8]>,
    into: &Path,
    workload: &Path,
) -> (Output, Trace) {
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
    (output, read(into, &program, workload))
}

/// Whether one traced open is a read [`Counts::files`] counts.
fn counted_read(line: &str, workload: &str) -> bool {
    let Some(path) = line.split('"').nth(1) else {
        return false;
    };
    path.starts_with(workload)
        && line.contains("O_RDONLY")
        && !path.ends_with("/.git/branches")
        && !path.contains("/.git/branches/")
}

/// `program`'s direct children and reads, from the per-thread traces in `into`.
fn read(into: &Path, program: &str, workload: &Path) -> Trace {
    let workload = format!("{}/", workload.display());
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
            if ["open(", "openat(", "openat2("]
                .iter()
                .any(|call| line.starts_with(call))
            {
                if created.is_some() && counted_read(line, &workload) {
                    traced.reads += 1;
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
    let mut files = 0;
    for member in &group {
        files += processes[member].reads;
        for child in &processes[member].children {
            let ran = processes
                .get(child)
                .and_then(|child| child.executed.first())
                .cloned()
                .unwrap_or_else(|| "(no program)".to_owned());
            programs.push(ran);
        }
    }
    Trace { programs, files }
}
