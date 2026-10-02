//! What a test run under this repository's nextest configuration does when it
//! goes wrong, and what CI says about it afterwards. Driven through real nextest
//! on fixture crates, because the subject is the scheduler and the scripts
//! around it.
//!
//! # Tests left unrun with no reason
//!
//! The macOS leg of run 36876601588 (job 110417653725) passed 1097 of 2247
//! tests, printed `warning: 1150/2247 tests were not run`, and named nothing
//! that failed. nextest's scheduler re-reads a test group's queue only when a
//! member of that group finishes, so a queued four-thread test that does not fit
//! when the last member ends is never started, and the run ends without it.
//! `.config/nextest.toml` avoids that by scheduling every test that needs more
//! than one thread first. The fixture here carries **that file, verbatim**, with
//! binaries named as the file's filters name them. It sets up the condition the
//! leg hit: the group's one-thread tests finish while tests outside the group
//! hold the slots a queued four-thread test needs.
//!
//! # After a failure
//!
//! `scripts/rerun-failed.sh` is what CI runs once a test step has failed. These
//! tests drive it the way `just rerun-failed` does, over a real nextest log, and
//! hold the four things it promises: only the failed tests run again, each
//! one's re-run failures are counted, a known flake is annotated and nothing
//! more, and a failed run is never reported as anything but failed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Where `bash` resolves on this host, skipping the Windows system directory,
/// whose `bash.exe` is the WSL launcher rather than a shell (the reason
/// `tests/linked_engines.rs` gives at length).
fn bash() -> PathBuf {
    let windows_dir =
        std::env::var_os("SystemRoot").map(|root| root.to_string_lossy().to_lowercase());
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .filter(|dir| {
            windows_dir.as_ref().is_none_or(|root| {
                !dir.to_string_lossy()
                    .to_lowercase()
                    .starts_with(root.as_str())
            })
        })
        .flat_map(|dir| ["bash", "bash.exe"].map(|name| dir.join(name)))
        .find(|candidate| candidate.is_file())
        .expect("a bash on PATH outside the Windows system directory runs the CI scripts")
}

/// A crate outside every workspace, written fresh under this clone's `target/`.
struct Fixture(PathBuf);

impl Fixture {
    fn new(case: &str, manifest: &str, files: &[(&str, &str)]) -> Self {
        let root = repo_root().join("target/nextest-runs").join(case);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("the fixture directory is made");
        fs::write(root.join("Cargo.toml"), manifest).expect("the fixture manifest is written");
        for (path, text) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().expect("a fixture file has a directory"))
                .expect("the fixture's directories are made");
            fs::write(path, text).expect("a fixture file is written");
        }
        Self(root)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// `program args...` from the fixture's root, in an environment holding
    /// nothing of the run this test is part of. Without the scrub, the outer
    /// run's `NEXTEST_PROFILE` (CI's `ci`) names a profile the fixture does not
    /// have, and coverage's `RUSTFLAGS` would instrument the fixture.
    fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(program);
        command.current_dir(&self.0);
        for (name, _) in std::env::vars_os() {
            let name = name.to_string_lossy();
            if name.starts_with("NEXTEST")
                || name.starts_with("__NEXTEST")
                || name.starts_with("CARGO_LLVM_COV")
                || name.starts_with("LLVM_PROFILE")
                || name.contains("RUSTFLAGS")
            {
                command.env_remove(name.as_ref());
            }
        }
        command.env("CARGO_TARGET_DIR", self.path("target"));
        command
    }

    /// A repository script, run with its output captured into one file the way
    /// a CI step's log captures both streams.
    fn script(&self, script: &str, args: &[&str], log: &str) -> (Output, String) {
        let file = fs::File::create(self.path(log)).expect("the log file is made");
        let output = self
            .command(bash())
            .arg(repo_root().join("scripts").join(script))
            .args(args)
            .stdout(file.try_clone().expect("the log file is shared"))
            .stderr(file)
            .stdin(Stdio::null())
            .output()
            .expect("bash runs the script");
        let text = fs::read_to_string(self.path(log)).expect("the log reads");
        (output, text)
    }

    /// The suite's run, as `just test-quick` makes it: nextest, through the
    /// wrapper that names its exit status.
    fn nextest_run(&self, args: &[&str]) -> (Output, String) {
        let mut argv = vec!["cargo", "nextest", "run"];
        argv.extend_from_slice(args);
        self.script("nextest-run.sh", &argv, "step.log")
    }

    fn rerun_failed(&self, known_flakes: &Path) -> (Output, String) {
        let log = self.path("step.log");
        self.script(
            "rerun-failed.sh",
            &[
                "--log",
                log.to_str().expect("a UTF-8 path"),
                "--times",
                &rerun_times().to_string(),
                "--known-flakes",
                known_flakes.to_str().expect("a UTF-8 path"),
                "--",
                "cargo",
                "nextest",
                "run",
            ],
            "rerun.log",
        )
    }
}

/// How many times the repository re-runs a failed test: the justfile's own
/// `rerun-times`, so these tests hold the number CI uses.
fn rerun_times() -> usize {
    let justfile = fs::read_to_string(repo_root().join("justfile")).expect("the justfile reads");
    justfile
        .lines()
        .find_map(|line| line.strip_prefix("rerun-times := \""))
        .and_then(|rest| rest.strip_suffix('"'))
        .expect("the justfile states rerun-times")
        .parse()
        .expect("rerun-times is a number")
}

const SCHEDULING_MANIFEST: &str = "[package]\nname = \"nextest-scheduling-fixture\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[workspace]\n\n[[test]]\nname = \"e2e\"\npath = \"tests/e2e.rs\"\n\n[[test]]\nname = \"note\"\npath = \"tests/note.rs\"\n\n[[test]]\nname = \"release_channel\"\npath = \"tests/release_channel.rs\"\n\n[[test]]\nname = \"oneagentgraph_liveness\"\npath = \"tests/oneagentgraph_liveness.rs\"\n";

/// One test matching each of the configuration's three four-thread overrides,
/// with a one-thread test before them and one after in the binary's order.
const SCHEDULING_E2E: &str = r#"
fn nap(ms: u64) {
    std::thread::sleep(std::time::Duration::from_millis(ms));
}
mod a {
    #[test]
    fn one_thread_first() {
        super::nap(2000)
    }
}
mod adoption {
    #[test]
    fn a_held_release_is_asked_about_on_its_own_interval_however_fast_the_loop_runs() {
        super::nap(200)
    }
}
mod listing {
    #[test]
    fn a_host_sized_listing() {
        super::nap(200)
    }
}
mod maintenance {
    #[test]
    fn a_maintenance_journey() {
        super::nap(200)
    }
}
mod z {
    #[test]
    fn one_thread_last() {
        super::nap(2000)
    }
}
"#;

/// Outside the group, and still running when the group's one-thread tests end.
const SCHEDULING_OUTSIDE_THE_GROUP: &str = r#"
fn nap() {
    std::thread::sleep(std::time::Duration::from_millis(4000));
}
#[test]
fn one() {
    nap()
}
#[test]
fn two() {
    nap()
}
#[test]
fn three() {
    nap()
}
"#;

/// Five threads: one more than the group's cap, which is where nextest can
/// queue a four-thread test inside the group while the run still has room. The
/// macOS leg ran at least that many: its log has four of the group's tests
/// running at once beside tests outside it.
const THREADS: &str = "5";

fn scheduling_fixture(case: &str, config: &str) -> Fixture {
    Fixture::new(
        case,
        SCHEDULING_MANIFEST,
        &[
            ("src/lib.rs", ""),
            (".config/nextest.toml", config),
            ("tests/e2e.rs", SCHEDULING_E2E),
            ("tests/note.rs", "#[test]\nfn one() {}\n"),
            ("tests/release_channel.rs", "#[test]\nfn one() {}\n"),
            (
                "tests/oneagentgraph_liveness.rs",
                SCHEDULING_OUTSIDE_THE_GROUP,
            ),
        ],
    )
}

fn repository_config() -> String {
    fs::read_to_string(repo_root().join(".config/nextest.toml"))
        .expect("the runner's configuration ships")
}

#[test]
fn a_test_needing_the_whole_group_never_strands_the_tests_queued_behind_it() {
    let fixture = scheduling_fixture("scheduling-as-configured", &repository_config());
    let (output, log) = fixture.nextest_run(&["--test-threads", THREADS]);
    assert!(
        output.status.success() && !log.contains("not run"),
        "under .config/nextest.toml, nextest left tests unrun:\n{log}"
    );
    assert!(
        log.contains("10 tests run: 10 passed"),
        "every test of the fixture ran and passed:\n{log}"
    );
}

/// The configuration as it was before its `priority` lines. That is still a run
/// that leaves tests unrun with no reason, and what CI prints for one is the
/// subject here. If this nextest no longer strands them, the scheduler has been
/// fixed upstream: the note in `.config/nextest.toml` and this test are due a
/// revisit, and the assertion says so.
#[test]
fn a_run_ended_with_tests_unrun_and_no_reason_is_named_by_nextests_exit_status() {
    let unordered: String = repository_config()
        .lines()
        .filter(|line| !line.starts_with("priority = "))
        .map(|line| format!("{line}\n"))
        .collect();
    let fixture = scheduling_fixture("scheduling-unordered", &unordered);
    let (output, log) = fixture.nextest_run(&["--test-threads", THREADS]);
    let unrun = log
        .lines()
        .find(|line| line.contains("tests were not run") || line.contains("test was not run"))
        .unwrap_or_else(|| {
            panic!("this nextest no longer strands the queued tests; revisit the priority note:\n{log}")
        });
    assert!(!unrun.contains("due to"), "nextest gave a reason: {unrun}");
    assert_eq!(
        output.status.code(),
        Some(100),
        "the wrapper exits with nextest's status:\n{log}"
    );
    assert!(
        log.contains("nextest-run: nextest exited with status 100"),
        "the wrapper names nextest's exit status:\n{log}"
    );

    let (report, text) = fixture.rerun_failed(&repo_root().join("scripts/known-flakes.txt"));
    assert_eq!(
        report.status.code(),
        Some(1),
        "a failed run is reported as failed:\n{text}"
    );
    assert!(
        text.lines().any(|line| line
            .starts_with("rerun-failed: nextest exited with status 100 and left ")
            && line.contains("tests not run, and gave no reason")),
        "the report names the status and says nextest gave no reason:\n{text}"
    );
    assert!(
        text.contains("nothing to re-run") && text.contains("verdict stays failed"),
        "with no failed test there is nothing to re-run, and the verdict stands:\n{text}"
    );
}

const RERUN_MANIFEST: &str =
    "[package]\nname = \"rerun-fixture\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[workspace]\n";

/// A deliberately passing test, a deliberately failing one, and one that fails
/// only the first time it runs. Each writes its name to a ledger every time it
/// runs, which is what says which tests ran again.
const RERUN_TESTS: &str = r#"
#[cfg(test)]
mod tests {
    use std::io::Write;

    fn ledger(name: &str) -> std::path::PathBuf {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ledger");
        std::fs::create_dir_all(&dir).unwrap();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("ran"))
            .unwrap();
        writeln!(file, "{name}").unwrap();
        dir
    }

    #[test]
    fn passes() {
        ledger("passes");
    }

    #[test]
    fn fails() {
        ledger("fails");
        panic!("deliberately failing");
    }

    #[test]
    fn fails_once() {
        let dir = ledger("fails_once");
        if std::fs::create_dir(dir.join("failed-once")).is_ok() {
            panic!("fails the first time only");
        }
    }
}
"#;

/// The fixture's first run, failed: what a CI test step leaves behind.
fn failed_step(case: &str) -> Fixture {
    let fixture = Fixture::new(case, RERUN_MANIFEST, &[("src/lib.rs", RERUN_TESTS)]);
    let (output, log) = fixture.nextest_run(&["--no-fail-fast"]);
    assert!(!output.status.success(), "the fixture's run failed:\n{log}");
    assert!(
        log.contains("3 tests run: 1 passed, 2 failed"),
        "the fixture's run failed the two tests it means to:\n{log}"
    );
    fixture
}

fn runs_of(fixture: &Fixture, test: &str) -> usize {
    fs::read_to_string(fixture.path("ledger/ran"))
        .expect("the ledger reads")
        .lines()
        .filter(|line| *line == test)
        .count()
}

fn summary_line<'a>(report: &'a str, test: &str) -> &'a str {
    report
        .lines()
        .find(|line| line.starts_with(&format!("  rerun-fixture tests::{test}: ")))
        .unwrap_or_else(|| panic!("no summary line for {test}:\n{report}"))
}

#[test]
fn only_the_failed_tests_run_again_and_each_ones_failures_are_counted() {
    let fixture = failed_step("rerun-counted");
    let times = rerun_times();
    let (output, report) = fixture.rerun_failed(&repo_root().join("scripts/known-flakes.txt"));

    assert_eq!(
        output.status.code(),
        Some(1),
        "a failed run is reported as failed:\n{report}"
    );
    assert_eq!(
        runs_of(&fixture, "passes"),
        1,
        "the passing test ran again:\n{report}"
    );
    assert_eq!(runs_of(&fixture, "fails"), 1 + times, "{report}");
    assert_eq!(runs_of(&fixture, "fails_once"), 1 + times, "{report}");

    assert_eq!(
        summary_line(&report, "fails"),
        format!(
            "  rerun-fixture tests::fails: failed {times} of {times} re-runs; fails every time on this build, which reads as a regression"
        )
    );
    assert_eq!(
        summary_line(&report, "fails_once"),
        format!(
            "  rerun-fixture tests::fails_once: failed 0 of {times} re-runs; did not fail again, which reads as a flake"
        )
    );
    assert!(
        report.contains("rerun-failed: the test step failed, and that verdict stands."),
        "{report}"
    );
    assert!(
        !report.contains("compiled"),
        "a re-run rebuilt instead of running the failed step's build:\n{report}"
    );
}

#[test]
fn a_known_flake_is_annotated_and_its_failure_still_stands() {
    let fixture = failed_step("rerun-known-flake");
    let times = rerun_times();
    let issue = "https://github.com/nickderobertis/onepipeline/issues/1";
    let flakes = fixture.path("known-flakes.txt");
    fs::write(
        &flakes,
        format!("# one entry\nrerun-fixture tests::fails {issue}\n"),
    )
    .expect("the list is written");
    let (output, report) = fixture.rerun_failed(&flakes);

    assert_eq!(
        output.status.code(),
        Some(1),
        "a known flake excused the run:\n{report}"
    );
    assert_eq!(runs_of(&fixture, "fails"), 1 + times, "{report}");
    assert_eq!(
        summary_line(&report, "fails"),
        format!(
            "  rerun-fixture tests::fails: failed {times} of {times} re-runs; fails every time on this build, which reads as a regression (known flake, issue {issue})"
        )
    );
    assert!(
        !summary_line(&report, "fails_once").contains("known flake"),
        "{report}"
    );
}

#[test]
fn a_known_flake_entry_in_any_other_shape_is_refused_before_anything_runs() {
    let fixture = failed_step("rerun-malformed-list");
    let flakes = fixture.path("known-flakes.txt");
    fs::write(&flakes, "rerun-fixture tests::fails not-an-issue-url\n")
        .expect("the list is written");
    let (output, report) = fixture.rerun_failed(&flakes);

    assert_eq!(output.status.code(), Some(2), "{report}");
    assert!(report.contains("known-flakes.txt:1 is not"), "{report}");
    assert_eq!(
        runs_of(&fixture, "fails"),
        1,
        "a test re-ran past the refusal:\n{report}"
    );
}
