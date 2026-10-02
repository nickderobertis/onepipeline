//! What a test run under this repository's nextest configuration does when it
//! goes wrong, and what CI says about it afterwards. Driven through real nextest
//! on fixture crates, because the subject is the scheduler and the scripts
//! around it.
//!
//! # Tests left unrun with no reason
//!
//! A CI leg once ended with tests unrun and no reason; the `priority` note in
//! `.config/nextest.toml` says why. The fixture here carries **that file, verbatim**, with binaries named as its
//! filters name them, and sets up the condition the leg hit: the group's
//! one-thread tests finish while tests outside the group hold the slots a queued
//! four-thread test needs.
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
        // Set by every CI runner, and it changes what the report prints.
        command.env_remove("GITHUB_ACTIONS");
        // `ci.yml` sets it for every job, and nextest then colours the counts
        // these tests read; `failed_step` asks for colour where it means to.
        command.env_remove("CARGO_TERM_COLOR");
        command
    }

    /// A repository script, run with its two streams kept apart for the
    /// assertions and written together to `log`, as a CI step's tee writes them.
    fn script(&self, script: &str, args: &[&str], log: &str) -> (Output, String) {
        self.script_in(script, args, log, &[])
    }

    fn script_in(
        &self,
        script: &str,
        args: &[&str],
        log: &str,
        env: &[(&str, &str)],
    ) -> (Output, String) {
        let output = self
            .command(bash())
            .envs(env.iter().copied())
            .arg(repo_root().join("scripts").join(script))
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("bash runs the script");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        fs::write(self.path(log), &text).expect("the log is written");
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
        self.rerun_failed_with(known_flakes, &["cargo", "nextest", "run"])
    }

    fn rerun_failed_with(&self, known_flakes: &Path, runner: &[&str]) -> (Output, String) {
        self.rerun_failed_in(known_flakes, runner, &[])
    }

    fn rerun_failed_in(
        &self,
        known_flakes: &Path,
        runner: &[&str],
        env: &[(&str, &str)],
    ) -> (Output, String) {
        let log = self.path("step.log");
        let times = rerun_times().to_string();
        let mut args = vec![
            "--log",
            log.to_str().expect("a UTF-8 path"),
            "--times",
            &times,
            "--known-flakes",
            known_flakes.to_str().expect("a UTF-8 path"),
            "--",
        ];
        args.extend_from_slice(runner);
        self.script_in("rerun-failed.sh", &args, "rerun.log", env)
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

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
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
    let wrapper_said = stderr(&output);
    assert!(
        wrapper_said.contains("nextest-run: nextest exited with status 100\n")
            && wrapper_said.contains(
                "nextest-run: 'just rerun-failed <this output>' re-runs the tests it failed"
            ),
        "the wrapper names nextest's exit status and the next step on stderr:\n{log}"
    );
    assert!(!stdout(&output).contains("nextest-run:"), "{log}");

    let (report, text) = fixture.rerun_failed(&repo_root().join("scripts/known-flakes.txt"));
    assert_eq!(
        report.status.code(),
        Some(1),
        "a failed run is reported as failed:\n{text}"
    );
    assert!(
        stderr(&report).lines().any(|line| line
            .starts_with("rerun-failed: nextest exited with status 100 and left ")
            && line.contains("tests not run, and gave no reason")),
        "the report names the status and says nextest gave no reason, on stderr:\n{text}"
    );
    assert!(
        text.contains("nothing to re-run") && text.contains("verdict stays failed"),
        "with no failed test there is nothing to re-run, and the verdict stands:\n{text}"
    );
}

const RERUN_MANIFEST: &str =
    "[package]\nname = \"rerun-fixture\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[workspace]\n";

/// A two-second ceiling so the timing-out test is ended quickly, and one retry
/// for `fails` and `flaky_on_rerun`, so their last status lines read
/// `TRY 2 FAIL` and `FLAKY 2/2`.
const RERUN_CONFIG: &str = "[profile.default]\nslow-timeout = { period = \"1s\", terminate-after = 2 }\n\n[[profile.default.overrides]]\nfilter = 'test(=tests::fails) or test(=tests::flaky_on_rerun)'\nretries = 1\n";

/// A deliberately passing test, and one failing test in each way nextest
/// reports a failure here: every time, the first time only, every other time,
/// by timing out, and by aborting. Two more fail in the step and then pass the
/// two other ways nextest reports a pass: leaking a child, and on a retry. Each
/// writes to a ledger every time it runs, which is what says which tests ran
/// again.
const RERUN_TESTS: &str = r#"
#[cfg(test)]
mod tests {
    use std::io::Write;

    /// Records this run of `name` and answers how many runs it has had: one
    /// byte per run, in a file of its own, so no two tests write one file.
    fn ledger(name: &str) -> u64 {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ledger");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b".")
            .unwrap();
        std::fs::metadata(&path).unwrap().len()
    }

    #[test]
    fn passes() {
        ledger("passes");
    }

    /// Prints what cargo prints when it builds: inside a test's output that is
    /// no rebuild, and the report must not read it as one.
    #[test]
    fn fails() {
        ledger("fails");
        println!("   Compiling impostor v0.0.0");
        panic!("deliberately failing");
    }

    #[test]
    fn fails_once() {
        if ledger("fails_once") == 1 {
            panic!("fails the first time only");
        }
    }

    #[test]
    fn fails_alternately() {
        if ledger("fails_alternately") % 2 == 1 {
            panic!("fails every other time");
        }
    }

    #[test]
    fn flaky_on_rerun() {
        let run = ledger("flaky_on_rerun");
        if run <= 2 || run % 2 == 1 {
            panic!("fails both attempts in the step, then the first attempt of each re-run");
        }
    }

    /// Passes, once it has failed, by returning while a child it started still
    /// holds its output: nextest reports that as `LEAK`.
    #[test]
    fn leaks_after_failing_once() {
        if std::env::var_os("FIXTURE_LEAK_CHILD").is_some() {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            return;
        }
        if ledger("leaks_after_failing_once") == 1 {
            panic!("fails the first time only");
        }
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tests::leaks_after_failing_once"])
            .env("FIXTURE_LEAK_CHILD", "1")
            .spawn()
            .unwrap();
    }

    #[test]
    fn times_out() {
        ledger("times_out");
        std::thread::sleep(std::time::Duration::from_secs(60));
    }

    #[test]
    fn aborts() {
        ledger("aborts");
        std::process::abort();
    }
}
"#;

const FAILING: [&str; 7] = [
    "fails",
    "fails_once",
    "fails_alternately",
    "flaky_on_rerun",
    "leaks_after_failing_once",
    "times_out",
    "aborts",
];

/// The tests with a retry, which run twice each time they fail.
const RETRIED: [&str; 2] = ["fails", "flaky_on_rerun"];

fn rerun_fixture(case: &str) -> Fixture {
    Fixture::new(
        case,
        RERUN_MANIFEST,
        &[
            ("src/lib.rs", RERUN_TESTS),
            (".config/nextest.toml", RERUN_CONFIG),
        ],
    )
}

/// The fixture's first run, failed: what a CI test step leaves behind, colour
/// escapes included, as a runner with `CARGO_TERM_COLOR=always` writes them.
fn failed_step(case: &str) -> Fixture {
    let fixture = rerun_fixture(case);
    let (output, log) = fixture.nextest_run(&["--no-fail-fast", "--color", "always"]);
    assert!(!output.status.success(), "the fixture's run failed:\n{log}");
    for test in FAILING {
        assert_eq!(
            runs_of(&fixture, test),
            if RETRIED.contains(&test) { 2 } else { 1 },
            "{log}"
        );
    }
    assert!(
        log.contains('\u{1b}'),
        "the step's log carries colour escapes:\n{log}"
    );
    fixture
}

fn runs_of(fixture: &Fixture, test: &str) -> usize {
    fs::metadata(fixture.path("ledger").join(test)).map_or(0, |ledger| {
        usize::try_from(ledger.len()).expect("a run count fits")
    })
}

fn summary_line<'a>(report: &'a str, test: &str) -> &'a str {
    report
        .lines()
        .find(|line| line.starts_with(&format!("  rerun-fixture tests::{test}: ")))
        .unwrap_or_else(|| panic!("no summary line for {test}:\n{report}"))
}

fn known_flakes() -> PathBuf {
    repo_root().join("scripts/known-flakes.txt")
}

#[test]
fn only_the_failed_tests_run_again_and_each_ones_failures_are_counted() {
    let fixture = failed_step("rerun-counted");
    let times = rerun_times();
    let (output, report) = fixture.rerun_failed(&known_flakes());

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
    assert_eq!(runs_of(&fixture, "fails"), 2 * (1 + times), "{report}");
    for test in [
        "fails_once",
        "fails_alternately",
        "leaks_after_failing_once",
        "times_out",
        "aborts",
    ] {
        assert_eq!(runs_of(&fixture, test), 1 + times, "{test}:\n{report}");
    }
    assert_eq!(
        runs_of(&fixture, "flaky_on_rerun"),
        2 + 2 * times,
        "{report}"
    );

    let every_time = |test: &str| {
        format!(
            "  rerun-fixture tests::{test}: failed {times} of {times} re-runs; fails every time on this build, which reads as a regression: reproduce it with cargo nextest run -E 'binary_id(=rerun-fixture) and test(=tests::{test})' and fix it"
        )
    };
    for test in ["fails", "times_out", "aborts"] {
        assert_eq!(summary_line(&report, test), every_time(test));
    }
    for test in ["fails_once", "flaky_on_rerun", "leaks_after_failing_once"] {
        assert_eq!(
            summary_line(&report, test),
            format!(
                "  rerun-fixture tests::{test}: failed 0 of {times} re-runs; did not fail again, which reads as a flake: fix it, or open an issue and list it in scripts/known-flakes.txt"
            )
        );
    }
    let re_runs = report
        .lines()
        .filter(|line| line.starts_with("rerun-failed: re-run 1 of "))
        .count();
    assert_eq!(re_runs, FAILING.len(), "{report}");
    let alternate_failures = (2..=1 + times).filter(|run| run % 2 == 1).count();
    assert_eq!(
        summary_line(&report, "fails_alternately"),
        format!(
            "  rerun-fixture tests::fails_alternately: failed {alternate_failures} of {times} re-runs; fails some of the time on this build, which reads as a flake: fix it, or open an issue and list it in scripts/known-flakes.txt"
        )
    );
    assert!(
        stderr(&output).contains("rerun-failed: the test step failed, and that verdict stands."),
        "the verdict is stated on stderr:\n{report}"
    );
    let reported = stdout(&output);
    for test in FAILING {
        assert!(
            reported.contains(summary_line(&report, test)),
            "the summary is the report, on stdout:\n{report}"
        );
    }
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
        format!("# one entry\r\n\r\nrerun-fixture tests::fails {issue} # tracked\r\n"),
    )
    .expect("the list is written");
    let (output, report) = fixture.rerun_failed_in(
        &flakes,
        &["cargo", "nextest", "run"],
        &[("GITHUB_ACTIONS", "true")],
    );

    assert_eq!(
        output.status.code(),
        Some(1),
        "a known flake excused the run:\n{report}"
    );
    assert_eq!(runs_of(&fixture, "fails"), 2 * (1 + times), "{report}");
    let fails = summary_line(&report, "fails");
    assert!(
        fails.starts_with(&format!(
            "  rerun-fixture tests::fails: failed {times} of {times} re-runs; fails every time"
        )) && fails.ends_with(&format!(" (known flake, issue {issue})")),
        "{report}"
    );
    assert!(
        !summary_line(&report, "fails_once").contains("known flake"),
        "{report}"
    );
    assert!(
        report.contains(&format!("::group::re-run 1 of {times}: nextest output"))
            && report.contains("::endgroup::"),
        "on a GitHub runner each re-run's output is folded into a group:\n{report}"
    );
}

#[test]
fn a_known_flake_entry_in_any_other_shape_is_refused_before_anything_runs() {
    let fixture = failed_step("rerun-malformed-list");
    let flakes = fixture.path("known-flakes.txt");
    let issue = "https://github.com/nickderobertis/onepipeline/issues/1";
    for entry in [
        "rerun-fixture tests::fails not-an-issue-url".to_owned(),
        format!("rerun-fixture tests::fails {issue} extra"),
        format!("rerun-fixture {issue}"),
        format!("rerun-fixture tests::f$ils {issue}"),
    ] {
        fs::write(&flakes, format!("# a comment\n{entry}\n")).expect("the list is written");
        let (output, report) = fixture.rerun_failed(&flakes);
        assert_eq!(output.status.code(), Some(2), "{entry}: {report}");
        assert!(
            report.contains("known-flakes.txt:2 is not"),
            "{entry}: {report}"
        );
    }
    assert_eq!(
        runs_of(&fixture, "fails"),
        2,
        "a test re-ran past a refusal"
    );
}

#[test]
fn a_rerun_that_compiles_or_cannot_run_says_so_and_still_reports_failure() {
    let fixture = failed_step("rerun-rebuilt");
    let times = rerun_times();
    // A source edit after the step: cargo now has to build before it can run.
    let source = fixture.path("src/lib.rs");
    let edited = format!(
        "{}\n// edited after the step\n",
        fs::read_to_string(&source).expect("the source reads")
    );
    fs::write(&source, edited).expect("the source is edited");
    let (output, report) = fixture.rerun_failed(&known_flakes());
    assert_eq!(output.status.code(), Some(1), "{report}");
    assert!(
        report.contains("re-run 1 of ") && report.contains("compiled rerun-fixture first"),
        "the re-run that built says it did not run the step's build:\n{report}"
    );

    let (output, report) = fixture.rerun_failed_with(
        &known_flakes(),
        &["cargo", "nextest", "run", "--no-such-flag"],
    );
    assert_eq!(output.status.code(), Some(1), "{report}");
    assert!(
        report.contains(&format!("re-run 1 of {times}: rerun-fixture tests::fails did not run (the re-run exited with status ")),
        "{report}"
    );
    assert!(
        summary_line(&report, "fails").starts_with(&format!(
            "  rerun-fixture tests::fails: failed 0 of {times} re-runs ({times} did not run); did not run in any re-run"
        )),
        "{report}"
    );

    // A runner that refuses its second call: one re-run of every test is lost,
    // and each test is judged on the ones that ran.
    fs::write(
        fixture.path("runner.sh"),
        "n=$(cat calls 2>/dev/null || echo 0); n=$((n + 1)); echo \"$n\" > calls\n\
         if [ \"$n\" -eq 2 ]; then echo 'this runner refuses its second call'; exit 3; fi\n\
         exec cargo nextest run \"$@\"\n",
    )
    .expect("the runner is written");
    let runner = fixture.path("runner.sh");
    let (output, report) = fixture.rerun_failed_with(
        &known_flakes(),
        &["bash", runner.to_str().expect("a UTF-8 path")],
    );
    assert_eq!(output.status.code(), Some(1), "{report}");
    assert!(
        report.contains(&format!(
            "re-run 2 of {times}: rerun-fixture tests::fails did not run (the re-run exited with status 3"
        )),
        "{report}"
    );
    assert!(
        summary_line(&report, "fails").starts_with(&format!(
            "  rerun-fixture tests::fails: failed {} of {times} re-runs (1 did not run); fails every time",
            times - 1
        )),
        "{report}"
    );
}

#[test]
fn tests_a_cancelled_run_left_unrun_are_reported_with_nextests_reason() {
    let fixture = rerun_fixture("rerun-fail-fast");
    let (output, log) = fixture.nextest_run(&["--test-threads", "1"]);
    assert!(!output.status.success(), "{log}");
    let (output, report) = fixture.rerun_failed(&known_flakes());
    assert_eq!(output.status.code(), Some(1), "{report}");
    let reason = report
        .lines()
        .find(|line| {
            line.starts_with("rerun-failed: nextest left ")
                && line.contains(" not run due to test failure")
        })
        .unwrap_or_else(|| panic!("the report relays nextest's reason:\n{report}"));
    let tests = FAILING.len() + 1;
    assert!(
        reason.contains(&format!("/{tests} tests not run")),
        "{reason}"
    );
}

#[test]
fn a_log_with_no_failed_test_reruns_nothing_and_never_reads_as_a_pass() {
    let fixture = Fixture::new("rerun-no-tests", RERUN_MANIFEST, &[]);
    let rerun = |log: &str| {
        fs::write(fixture.path("step.log"), log).expect("the log is written");
        fixture.rerun_failed(&known_flakes())
    };

    let (output, report) = rerun("error: could not compile `onepipeline`\n");
    assert_eq!(output.status.code(), Some(0), "{report}");
    assert!(
        report.contains("names no failed test, so there is nothing to re-run"),
        "{report}"
    );

    let (output, report) =
        rerun("     Summary [   1.000s] 1 test run: 1 passed\nerror: test run failed\n");
    assert_eq!(output.status.code(), Some(1), "{report}");
    assert!(report.contains("verdict stays failed"), "{report}");

    let (output, report) = rerun(
        "     Summary [   1.000s] 1/2 tests run: 1 passed\nwarning: 1/2 tests were not run\nerror: test run failed\n",
    );
    assert_eq!(output.status.code(), Some(1), "{report}");
    assert!(
        report.contains("nextest left 1/2 tests not run and gave no reason, and its exit status is not in this log"),
        "{report}"
    );
}

#[test]
fn a_failed_test_whose_name_cannot_become_a_filter_is_named_and_not_rerun() {
    let fixture = Fixture::new("rerun-unusable-name", RERUN_MANIFEST, &[]);
    fs::write(
        fixture.path("step.log"),
        "        FAIL [   0.010s] (1/1) rerun-fixture tests::we$ird\nerror: test run failed\n",
    )
    .expect("the log is written");
    let (output, report) = fixture.rerun_failed_with(&known_flakes(), &["false"]);
    assert_eq!(output.status.code(), Some(1), "{report}");
    assert!(
        report.contains("not re-running 'rerun-fixture tests::we$ird'")
            && report.contains("widen id_shape"),
        "{report}"
    );
    assert!(!report.contains("re-run 1 of"), "{report}");
}

#[test]
fn the_scripts_refuse_what_they_cannot_run() {
    let fixture = Fixture::new("rerun-refusals", RERUN_MANIFEST, &[]);
    let (output, report) = fixture.script("nextest-run.sh", &[], "wrapper.log");
    assert_eq!(output.status.code(), Some(2), "{report}");
    assert!(report.starts_with("usage: nextest-run.sh"), "{report}");

    let flakes = known_flakes();
    let flakes = flakes.to_str().expect("a UTF-8 path");
    let missing = fixture.path("no-such.log");
    let missing = missing.to_str().expect("a UTF-8 path");
    for (args, says) in [
        (
            vec!["--log", missing, "--times", "3", "--known-flakes", flakes],
            "usage:",
        ),
        (
            vec![
                "--log",
                missing,
                "--times",
                "0",
                "--known-flakes",
                flakes,
                "--",
                "true",
            ],
            "--times must be a whole number from 1 to 10",
        ),
        (
            vec![
                "--log",
                missing,
                "--times",
                "3",
                "--known-flakes",
                flakes,
                "--",
                "true",
            ],
            "cannot read the failed step's log",
        ),
        (vec!["--bogus"], "unknown argument '--bogus'"),
    ] {
        let (output, report) = fixture.script("rerun-failed.sh", &args, "refusal.log");
        assert_eq!(output.status.code(), Some(2), "{args:?}: {report}");
        assert!(report.contains(says), "{args:?}: {report}");
    }

    fs::write(fixture.path("step.log"), "error: test run failed\n").expect("the log is written");
    let log = fixture.path("step.log");
    let log = log.to_str().expect("a UTF-8 path");
    let no_list = fixture.path("no-such-list.txt");
    let a_directory = fixture.path("a-directory");
    fs::create_dir_all(&a_directory).expect("a directory is made");
    let no_tmp = fixture.path("no-such-tmp");
    for (args, env, says) in [
        (
            [
                "--log",
                log,
                "--known-flakes",
                no_list.to_str().expect("a UTF-8 path"),
            ],
            vec![],
            "restore it from git",
        ),
        (
            [
                "--log",
                log,
                "--known-flakes",
                a_directory.to_str().expect("a UTF-8 path"),
            ],
            vec![],
            "restore it from git",
        ),
        (
            [
                "--log",
                a_directory.to_str().expect("a UTF-8 path"),
                "--known-flakes",
                flakes,
            ],
            vec![],
            "pass the file the failed step's output was teed into",
        ),
        (
            ["--log", log, "--known-flakes", flakes],
            vec![("TMPDIR", no_tmp.to_str().expect("a UTF-8 path"))],
            "point TMPDIR at a writable directory",
        ),
    ] {
        let mut argv = args.to_vec();
        argv.extend(["--times", "3", "--", "true"]);
        let (output, report) = fixture.script_in("rerun-failed.sh", &argv, "refusal.log", &env);
        assert_eq!(output.status.code(), Some(2), "{argv:?}: {report}");
        assert!(report.contains(says), "{argv:?}: {report}");
    }
}

/// The steps that run `just rerun-failed` hold the three things the workflow
/// promises about them: they run only after their job's test step failed, they
/// cannot change the job's verdict, and their timeout covers every re-run of a
/// test that hangs until nextest ends it.
// llmlint: ignore-block[changed_behavior_has_e2e] a workflow's `if:` and
// `continue-on-error` are evaluated by GitHub's runner and by nothing on this
// host, so no test here can execute them. What can be is held here, and the
// script the steps run is driven end to end above. The steps themselves are
// proven on GitHub by the demonstration run recorded on this change request.
#[test]
fn the_rerun_steps_run_only_after_a_failure_and_are_bounded_by_what_they_run() {
    let workflow = fs::read_to_string(repo_root().join(".github/workflows/ci.yml"))
        .expect("the workflow reads");
    let config: toml::Value = toml::from_str(&repository_config()).expect("the config parses");
    let slow = &config["profile"]["default"]["slow-timeout"];
    let period: u64 = slow["period"]
        .as_str()
        .and_then(|period| period.strip_suffix('s'))
        .and_then(|seconds| seconds.parse().ok())
        .expect("slow-timeout's period is in seconds");
    let terminate_after = slow["terminate-after"]
        .as_integer()
        .and_then(|count| u64::try_from(count).ok())
        .expect("slow-timeout has terminate-after");
    let hung_reruns_minutes = (rerun_times() as u64 * period * terminate_after).div_ceil(60);

    let steps: Vec<&str> = workflow
        .split("      - name: ")
        .filter(|step| step.starts_with("Does each failed test fail again on this build"))
        .collect();
    assert_eq!(steps.len(), 2, "one rerun step each in gate and cross");
    for step in steps {
        let field = |key: &str| {
            step.lines()
                .find_map(|line| line.trim().strip_prefix(key))
                .map(str::trim)
                .unwrap_or_else(|| panic!("the step sets {key}:\n{step}"))
        };
        assert!(field("if:").starts_with("failure() && "), "{step}");
        assert_eq!(field("continue-on-error:"), "true", "{step}");
        assert!(field("run:").starts_with("just rerun-failed"), "{step}");
        let minutes: u64 = field("timeout-minutes:")
            .parse()
            .expect("a whole number of minutes");
        assert!(
            minutes >= hung_reruns_minutes,
            "{minutes} minutes cannot hold {} re-runs of a test nextest ends after {}s:\n{step}",
            rerun_times(),
            period * terminate_after
        );
    }
} // llmlint: ignore-end[changed_behavior_has_e2e]
