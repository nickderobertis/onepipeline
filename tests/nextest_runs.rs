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
//! # A test that leaves a process running
//!
//! A test that returns while a process it started still holds its output is a
//! `LEAK`, which nextest counts as a pass, and on Windows that process holds the
//! pipe a CI step tees the run through. The `ci` profile fails such a test by
//! name instead; driven here through the configuration verbatim.
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

/// A crate outside every workspace, written fresh under the system's temporary
/// directory and removed with this value. Not under `target/`: CI's
/// `Swatinem/rust-cache` cleans that tree and reports a crate it finds there
/// with a `tests/` directory as an error on every job.
struct Fixture(PathBuf);

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl Fixture {
    fn new(case: &str, manifest: &str, files: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!(
            "onepipeline-nextest-runs-{case}-{}",
            std::process::id()
        ));
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
    /// have, and coverage's rustc wrapper (`RUSTC_WRAPPER`, which reads its
    /// `__CARGO_LLVM_COV_*` settings) would build the fixture through itself.
    fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(program);
        command.current_dir(&self.0);
        for (name, _) in std::env::vars_os() {
            let name = name.to_string_lossy();
            if name.starts_with("NEXTEST")
                || name.starts_with("__NEXTEST")
                || name.contains("CARGO_LLVM_COV")
                || name.starts_with("LLVM_PROFILE")
                || name.starts_with("RUSTC_WRAPPER")
                || name.starts_with("RUSTC_WORKSPACE_WRAPPER")
                || name.contains("RUSTFLAGS")
            {
                command.env_remove(name.as_ref());
            }
        }
        command.env("CARGO_TARGET_DIR", self.path("target"));
        // Set by every CI runner, and it changes what the report prints.
        command.env_remove("GITHUB_ACTIONS");
        // CI's test step sets it for the whole suite, naming the suite's own
        // archive: a fixture inheriting it would archive itself over that one.
        command.env_remove("ONEPIPELINE_TEST_ARCHIVE");
        // `ci.yml` sets the first for every job and Nx the second for every
        // task, and nextest then colours the counts these tests read;
        // `failed_step` asks for colour where it means to.
        for colour in ["CARGO_TERM_COLOR", "FORCE_COLOR", "CLICOLOR_FORCE"] {
            command.env_remove(colour);
        }
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

    /// One shell line from the fixture's root, as a recipe runs it.
    fn script_line(&self, line: &str, env: &[(&str, &str)]) -> (Output, String) {
        let output = self
            .command(bash())
            .envs(env.iter().copied())
            .args(["-eu", "-o", "pipefail", "-c", line])
            .stdin(Stdio::null())
            .output()
            .expect("bash runs the line");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        (output, text)
    }

    /// The suite's run, as each project's `test-quick` target makes it:
    /// nextest, through the wrapper that names its exit status.
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
        let archive = self.path(ARCHIVE);
        let mut args = vec![
            "--log",
            log.to_str().expect("a UTF-8 path"),
            "--archive",
            archive.to_str().expect("a UTF-8 path"),
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

/// Where a fixture's step archives its build, as `ONEPIPELINE_TEST_ARCHIVE`
/// makes the `onepipeline:test-quick-archive` target do in CI.
const ARCHIVE: &str = "suite.tar.zst";

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
                "nextest-run: 'just rerun-failed <this output> <its archive>' re-runs the tests it failed"
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

/// Where each process the fixture's tests start writes its pid, one empty file
/// named for it, under the fixture's root.
const LINGERERS: &str = "lingerers";

/// One test that returns while a process it started still holds its output, one
/// that waits for what it started, and the program both start: this binary
/// again, which records its pid under [`LINGERERS`] and sleeps when told it is
/// the one started.
const LEAKING_E2E: &str = r#"
fn start_the_lingerer() -> std::process::Child {
    std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "lingers_when_started_by_another_test"])
        .env("FIXTURE_LINGER", "1")
        .spawn()
        .unwrap()
}
#[test]
fn lingers_when_started_by_another_test() {
    if std::env::var_os("FIXTURE_LINGER").is_some() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("lingerers");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(std::process::id().to_string()), "").unwrap();
        std::thread::sleep(std::time::Duration::from_secs(3));
    }
}
#[test]
fn leaves_a_process_holding_its_output() {
    start_the_lingerer();
}
#[test]
fn waits_for_what_it_started() {
    start_the_lingerer().wait().unwrap();
}
"#;

fn leaking_fixture(case: &str) -> Fixture {
    Fixture::new(
        case,
        SCHEDULING_MANIFEST,
        &[
            ("src/lib.rs", ""),
            (".config/nextest.toml", &repository_config()),
            ("tests/e2e.rs", LEAKING_E2E),
            ("tests/note.rs", "#[test]\nfn one() {}\n"),
            ("tests/release_channel.rs", "#[test]\nfn one() {}\n"),
            ("tests/oneagentgraph_liveness.rs", "#[test]\nfn one() {}\n"),
        ],
    )
}

/// The pids the fixture's started processes recorded under [`LINGERERS`] that
/// have not ended by `patience` from now, waited for together.
///
/// Every one of them is a deliberate leak of an inner run, and on Windows each
/// inherits every inheritable handle in its chain of ancestors — this test's own
/// output among them, which `cargo nextest` reads it through. One still running
/// when this test returns holds that output past its end, and the outer run
/// marks this test `LEAK`. So it waits them out rather than returning first.
fn lingerers_outliving(fixture: &Fixture, patience: std::time::Duration) -> (usize, Vec<u32>) {
    let pids: Vec<u32> = fs::read_dir(fixture.path(LINGERERS))
        .expect("the fixture's started processes recorded their pids")
        .map(|entry| {
            let name = entry.expect("a recorded pid reads").file_name();
            name.to_str()
                .and_then(|pid| pid.parse().ok())
                .unwrap_or_else(|| panic!("{name:?} names no pid"))
        })
        .collect();
    let deadline = std::time::Instant::now() + patience;
    loop {
        let running: Vec<u32> = pids
            .iter()
            .copied()
            .filter(|pid| !onepipeline_testfakes::containment::process_ended(*pid))
            .collect();
        if running.is_empty() || std::time::Instant::now() >= deadline {
            return (pids.len(), running);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Under the profile CI's cross-platform legs run, a test that returns while a
/// process it started still holds its output fails the run and is named for it,
/// and `rerun-failed` reads that as the failure it is. Under the default profile
/// the same test is still a leaky pass. Every process those runs leak has ended,
/// by its pid, before this test returns, so this test leaks nothing itself.
#[test]
fn a_test_leaving_a_process_holding_its_output_fails_the_ci_profile_by_name() {
    let fixture = leaking_fixture("leak-fails");
    archive(&fixture);
    let leaker = "nextest-scheduling-fixture::e2e leaves_a_process_holding_its_output";

    let (output, log) = fixture.nextest_run(&["--no-fail-fast"]);
    assert!(
        output.status.success() && log.contains("passed (1 leaky)"),
        "under the default profile a leak is still counted as a leaky pass:\n{log}"
    );

    let (output, log) = fixture.nextest_run(&["--no-fail-fast", "--profile", "ci"]);
    assert!(
        !output.status.success(),
        "under the ci profile a test that left a process holding its output passed:\n{log}"
    );
    assert!(
        log.lines()
            .any(|line| line.trim_start().starts_with("LEAK-FAIL ") && line.ends_with(leaker)),
        "the ci profile's failure does not name the test that leaked:\n{log}"
    );
    for passing in [
        "waits_for_what_it_started",
        "lingers_when_started_by_another_test",
    ] {
        assert!(
            log.lines()
                .any(|line| line.trim_start().starts_with("PASS ")
                    && line.ends_with(&format!("e2e {passing}"))),
            "{passing} did not pass beside the leak:\n{log}"
        );
    }

    let (report, text) = fixture.rerun_failed_in(
        &known_flakes(),
        &["cargo", "nextest", "run"],
        &[("NEXTEST_PROFILE", "ci")],
    );
    assert_eq!(
        report.status.code(),
        Some(1),
        "a run failed by a leak is reported as failed:\n{text}"
    );
    let times = rerun_times();
    assert!(
        text.lines().any(|line| line.starts_with(&format!(
            "  {leaker}: failed {times} of {times} re-runs; fails every time"
        ))),
        "the leak is not re-run as the failure it is:\n{text}"
    );

    let (recorded, running) = lingerers_outliving(&fixture, std::time::Duration::from_secs(60));
    assert!(
        recorded >= 2 + times,
        "only {recorded} started processes recorded their pids, fewer than the runs leaked"
    );
    assert!(
        running.is_empty(),
        "pids {running:?} the inner runs leaked were still running when this test returned"
    );
}

/// The journey the `ci` profile gives longer, under the name its filter matches,
/// and one beside it in the same module. The fixture puts both in the `e2e` and
/// `note` binaries, so the same name in another binary is one the store journey's
/// filter does not reach. `STORE` and `BESIDE` are milliseconds, which the fixture
/// writes as [`STORE_MILLIS`] and [`BESIDE_MILLIS`].
const LONG_JOURNEY_E2E: &str = r#"
mod readjust {
    #[test]
    fn the_store_is_written_only_for_what_its_projection_holds_changed() {
        std::thread::sleep(std::time::Duration::from_millis(STORE));
    }
    #[test]
    fn beside_it() {
        std::thread::sleep(std::time::Duration::from_millis(BESIDE));
    }
}
"#;

/// The period [`long_journey_fixture`] scales the configuration's 60s to, so a
/// run proves which test is given how many periods without waiting ten minutes.
const SCALED_PERIOD_MILLIS: u64 = 4_000;

/// Past the nine periods Windows gives every test, and short of the store
/// journey's ten, by two seconds each way. Only ten periods pass it.
const STORE_MILLIS: u64 = 38_000;
const _: () =
    assert!(STORE_MILLIS > SCALED_PERIOD_MILLIS * 9 && STORE_MILLIS < SCALED_PERIOD_MILLIS * 10);

/// Past six periods and short of nine, by six seconds each way. Only the
/// Windows bound passes it.
const BESIDE_MILLIS: u64 = 30_000;
const _: () =
    assert!(BESIDE_MILLIS > SCALED_PERIOD_MILLIS * 6 && BESIDE_MILLIS < SCALED_PERIOD_MILLIS * 9);

/// The ci profile's override that scopes a bound to Windows, as the
/// configuration spells it.
const WINDOWS_ONLY: &str = "platform = 'cfg(windows)'";

/// `config` with every `slow-timeout` period scaled from 60s to
/// [`SCALED_PERIOD_MILLIS`] and nothing else changed.
fn long_journey_fixture(name: &str, config: &str) -> Fixture {
    assert!(
        config.matches("period = \"60s\"").count() >= 3,
        "the default limit and the ci overrides are no longer in 60s periods:\n{config}"
    );
    let scaled = config.replace(
        "period = \"60s\"",
        &format!("period = \"{SCALED_PERIOD_MILLIS}ms\""),
    );
    let journeys = LONG_JOURNEY_E2E
        .replace("STORE", &STORE_MILLIS.to_string())
        .replace("BESIDE", &BESIDE_MILLIS.to_string());
    Fixture::new(
        name,
        SCHEDULING_MANIFEST,
        &[
            ("src/lib.rs", ""),
            (".config/nextest.toml", &scaled),
            ("tests/e2e.rs", &journeys),
            ("tests/note.rs", &journeys),
            ("tests/release_channel.rs", "#[test]\nfn one() {}\n"),
            ("tests/oneagentgraph_liveness.rs", "#[test]\nfn one() {}\n"),
        ],
    )
}

/// Run `config`'s long journeys under the ci profile, and hold what each is
/// given. The store journey passes on its own ten periods on every platform.
/// The same name in the `note` binary is outside that filter and times out under
/// both six and nine. The journeys beside it pass when the bound is raised,
/// which is nine periods, and time out when it is not, which is six.
fn holds_the_ci_limits(name: &str, config: &str, raised: bool) {
    let fixture = long_journey_fixture(name, config);
    let (output, log) = fixture.nextest_run(&["--no-fail-fast", "--profile", "ci"]);
    assert!(!output.status.success(), "{log}");
    let journey = "readjust::the_store_is_written_only_for_what_its_projection_holds_changed";
    let ended = |status: &str, test: &str| {
        log.lines()
            .any(|line| line.trim_start().starts_with(status) && line.ends_with(test))
    };
    assert!(
        ended("PASS ", &format!("e2e {journey}")),
        "the store journey was not given ten periods under the ci profile:\n{log}"
    );
    assert!(
        ended("TIMEOUT ", &format!("note {journey}")),
        "note {journey} was given the store journey's ten periods under the ci profile:\n{log}"
    );
    let (status, given) = if raised {
        ("PASS ", "was not given the raised bound")
    } else {
        ("TIMEOUT ", "was given more than six periods")
    };
    for other in ["e2e readjust::beside_it", "note readjust::beside_it"] {
        assert!(
            ended(status, other),
            "{other} {given} under the ci profile:\n{log}"
        );
    }
}

/// Under the profile CI's cross-platform legs run, the one journey
/// `.config/nextest.toml` names is given ten periods, 600s, before nextest ends
/// it. Every other test keeps the six it had here, unless this host is Windows,
/// where it is given nine: the same module's other test, and a test of the same
/// name in another binary, are ended at whichever applies.
#[test]
fn the_ci_profile_gives_the_store_journey_ten_periods_and_no_other_test_more() {
    let config: toml::Value = toml::from_str(&repository_config()).expect("the config parses");
    let override_ = config["profile"]["ci"]["overrides"]
        .as_array()
        .and_then(|overrides| {
            overrides.iter().find(|entry| {
                entry["filter"].as_str().is_some_and(|filter| {
                    filter.contains(
                        "readjust::the_store_is_written_only_for_what_its_projection_holds_changed",
                    )
                })
            })
        })
        .expect("the ci profile overrides the store journey's limit");
    assert_eq!(
        override_["slow-timeout"]["period"].as_str(),
        Some("60s"),
        "{override_}"
    );
    assert_eq!(
        override_["slow-timeout"]["terminate-after"].as_integer(),
        Some(10),
        "the store journey is given 600s: {override_}"
    );
    holds_the_ci_limits("long-journey", &repository_config(), cfg!(windows));
}

/// On Windows alone, the ci profile gives every test but the store journey nine
/// periods, 540s, for the runner image's slowdown; Linux and macOS keep six.
///
/// The test above runs the configuration as written on this host; this one runs
/// it with the Windows condition inverted. Between them this host is on each side
/// of that condition, whatever platform it is, so it sees both the raised bound
/// and the unraised one.
#[test]
fn the_ci_profile_gives_every_other_test_nine_periods_on_windows_alone() {
    let text = repository_config();
    let config: toml::Value = toml::from_str(&text).expect("the config parses");
    let overrides = config["profile"]["ci"]["overrides"]
        .as_array()
        .expect("the ci profile has overrides");
    let windows: Vec<_> = overrides
        .iter()
        .filter(|entry| entry.get("platform").is_some())
        .collect();
    assert_eq!(
        windows.len(),
        1,
        "the ci profile scopes one override, and only one, by platform: {overrides:?}"
    );
    let windows = windows[0];
    assert_eq!(
        windows["platform"].as_str(),
        Some("cfg(windows)"),
        "{windows}"
    );
    assert!(
        windows.get("filter").is_none(),
        "the Windows bound reaches every test, not a filter's: {windows}"
    );
    assert_eq!(
        windows["slow-timeout"]["period"].as_str(),
        Some("60s"),
        "{windows}"
    );
    assert_eq!(
        windows["slow-timeout"]["terminate-after"].as_integer(),
        Some(9),
        "every test on Windows is given 540s: {windows}"
    );
    assert_eq!(
        text.matches(WINDOWS_ONLY).count(),
        1,
        "the Windows condition is not spelled as this test inverts it:\n{text}"
    );

    holds_the_ci_limits(
        "windows-bound-inverted",
        &text.replace(WINDOWS_ONLY, "platform = 'cfg(not(windows))'"),
        !cfg!(windows),
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

    #[test]
    fn fails() {
        ledger("fails");
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

/// One line of a justfile recipe's body, as the shell is given it.
fn recipe_line(recipe: &str, containing: &str) -> String {
    let justfile = fs::read_to_string(repo_root().join("justfile")).expect("the justfile reads");
    justfile
        .split(&format!("\n{recipe}:\n"))
        .nth(1)
        .unwrap_or_else(|| panic!("the justfile has a `{recipe}` recipe"))
        .lines()
        .take_while(|line| line.starts_with("    "))
        .find(|line| line.contains(containing))
        .unwrap_or_else(|| panic!("`{recipe}` has a line running {containing}"))
        .trim_start()
        .trim_start_matches('@')
        .to_owned()
}

/// The uninstrumented suite's own archive line, run in the fixture as CI runs
/// it, before the run: with `ONEPIPELINE_TEST_ARCHIVE` naming where. Without it
/// the line archives nothing, which is how every run outside CI goes.
fn archive(fixture: &Fixture) {
    let line = recipe_line("_crate-test-quick-archive", "nextest archive");
    let locked = fixture
        .command(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["generate-lockfile", "--offline"])
        .output()
        .expect("cargo writes the fixture's lock");
    assert!(locked.status.success(), "the fixture's lock is written");
    let (output, said) = fixture.script_line(&line, &[]);
    assert!(
        output.status.success() && !fixture.path(ARCHIVE).exists(),
        "without ONEPIPELINE_TEST_ARCHIVE the line archives nothing:\n{said}"
    );
    let (output, said) = fixture.script_line(&line, &[("ONEPIPELINE_TEST_ARCHIVE", ARCHIVE)]);
    assert!(
        output.status.success() && fixture.path(ARCHIVE).is_file(),
        "the fixture's build is archived by `{line}`:\n{said}"
    );
}

/// The fixture's first run, failed: what a CI test step leaves behind, colour
/// escapes included, as a runner with `CARGO_TERM_COLOR=always` writes them.
fn failed_step(case: &str) -> Fixture {
    let fixture = rerun_fixture(case);
    archive(&fixture);
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
fn a_rerun_runs_the_archived_build_and_cannot_compile() {
    let fixture = failed_step("rerun-archived");
    let times = rerun_times();
    // After the step, the source no longer fails `fails` and no longer matches
    // what was built. A re-run that built would compile it and see `fails` pass.
    let source = fixture.path("src/lib.rs");
    let edited = fs::read_to_string(&source)
        .expect("the source reads")
        .replace("panic!(\"deliberately failing\");", "");
    fs::write(&source, edited).expect("the source is edited");
    let cargo_speaks = [("CARGO_TERM_QUIET", "false")];
    let (output, report) =
        fixture.rerun_failed_in(&known_flakes(), &["cargo", "nextest", "run"], &cargo_speaks);
    assert_eq!(output.status.code(), Some(1), "{report}");
    assert!(
        !report.contains("Compiling"),
        "a re-run started a compilation:\n{report}"
    );
    assert!(
        summary_line(&report, "fails").starts_with(&format!(
            "  rerun-fixture tests::fails: failed {times} of {times} re-runs; fails every time"
        )),
        "the re-runs ran the archived build, not the edited source:\n{report}"
    );

    // A build option beside the archive is refused by nextest, not obeyed.
    let (output, report) =
        fixture.rerun_failed_with(&known_flakes(), &["cargo", "nextest", "run", "--locked"]);
    assert_eq!(output.status.code(), Some(1), "{report}");
    assert!(
        !report.contains("Compiling")
            && report.contains(&format!(
                "re-run 1 of {times}: rerun-fixture tests::fails did not run (the re-run exited with status "
            )),
        "{report}"
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
    archive(&fixture);
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

    // The NUL is what a step's output can carry and grep then calls binary.
    let (output, report) = rerun(
        "\0 a byte a test printed\n     Summary [   1.000s] 1/2 tests run: 1 passed\nwarning: 1/2 tests were not run\nerror: test run failed\n",
    );
    assert_eq!(output.status.code(), Some(1), "{report}");
    assert!(
        report.contains("nextest left 1/2 tests not run and gave no reason, and its exit status is not in this log"),
        "{report}"
    );
}

#[test]
fn a_failed_test_with_no_archive_of_its_build_is_refused_rather_than_built_again() {
    let fixture = Fixture::new("rerun-no-archive", RERUN_MANIFEST, &[]);
    fs::write(
        fixture.path("step.log"),
        "        FAIL [   0.010s] (1/1) rerun-fixture tests::fails\nerror: test run failed\n",
    )
    .expect("the log is written");
    let (output, report) = fixture.rerun_failed(&known_flakes());
    assert_eq!(output.status.code(), Some(2), "{report}");
    assert!(
        report.contains("no readable archive of the failed step's build")
            && report.contains("ONEPIPELINE_TEST_ARCHIVE"),
        "{report}"
    );
    assert!(!report.contains("re-run 1 of"), "{report}");
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
            vec![
                "--log",
                missing,
                "--archive",
                missing,
                "--times",
                "3",
                "--known-flakes",
                flakes,
            ],
            "usage:",
        ),
        (
            vec![
                "--log",
                missing,
                "--archive",
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
                "--archive",
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
    // A `mktemp` that cannot make a directory, first on PATH. A TMPDIR that does
    // not exist is no way to fail it: BSD `mktemp -d` falls back past one.
    let bin = fixture.path("bin");
    fs::create_dir_all(&bin).expect("a directory for the stand-in");
    let stand_in = bin.join("mktemp");
    fs::write(
        &stand_in,
        "#!/bin/sh\necho 'mktemp: cannot make a directory' >&2\nexit 1\n",
    )
    .expect("the stand-in is written");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&stand_in, fs::Permissions::from_mode(0o755))
            .expect("the stand-in is executable");
    }
    let failing_path = std::env::join_paths(std::iter::once(bin.clone()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("a PATH");
    let failing_path = failing_path.to_str().expect("a UTF-8 PATH");
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
            vec![("PATH", failing_path)],
            "point TMPDIR at a writable directory",
        ),
    ] {
        let mut argv = args.to_vec();
        argv.extend(["--archive", log, "--times", "3", "--", "true"]);
        let (output, report) = fixture.script_in("rerun-failed.sh", &argv, "refusal.log", &env);
        assert_eq!(output.status.code(), Some(2), "{argv:?}: {report}");
        assert!(report.contains(says), "{argv:?}: {report}");
    }
}

/// The steps that run `just rerun-failed` hold the four things the workflow
/// promises about them: they run only after their job's test step failed, they
/// cannot change the job's verdict, they re-run from the archive their job's
/// test step made, and their timeout covers every re-run of a test that hangs
/// until nextest ends it, each after the two minutes the justfile allows for
/// extracting the archive.
// llmlint: ignore-block[changed_behavior_has_e2e] a workflow's `if:` and
// `continue-on-error` are evaluated by GitHub's runner and by nothing on this
// host, so no test here can execute them. What can be is held here, and the
// script the steps run is driven end to end above. The steps themselves are
// proven on GitHub by the demonstration run recorded on this change request.
#[test]
fn the_rerun_steps_run_only_after_a_failure_and_are_bounded_by_what_they_run() {
    // `ci.yml` holds gate, Windows cross, PR Mac and manual Mac diagnostics;
    // `release-sweep.yml` holds the full
    // sweep on the release pull request, which re-runs its failures the same way.
    let workflow = [
        ".github/workflows/ci.yml",
        ".github/workflows/release-sweep.yml",
    ]
    .map(|path| fs::read_to_string(repo_root().join(path)).expect("the workflow reads"))
    .join("\n");
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
    const EXTRACTION_MINUTES: u64 = 2;
    let hung_reruns_minutes = (rerun_times() as u64 * period * terminate_after).div_ceil(60)
        + rerun_times() as u64 * EXTRACTION_MINUTES;
    let archive = "\"$RUNNER_TEMP/tests.tar.zst\"";
    let archived_runs = workflow
        .lines()
        // The Windows shards name their recipe in `GATE`: `check-cross` for the
        // first, `test-quick` for the rest.
        .filter(|line| {
            ["just check", "just \"$GATE\""].iter().any(|recipe| {
                line.trim()
                    .starts_with(&format!("run: ONEPIPELINE_TEST_ARCHIVE={archive} {recipe}"))
            })
        })
        .count();
    assert_eq!(
        archived_runs, 5,
        "gate, Windows cross, PR Mac, manual Mac and release sweep archive their build"
    );

    let steps: Vec<&str> = workflow
        .split("      - name: ")
        .filter(|step| step.starts_with("Does each failed test fail again on this build"))
        .collect();
    assert_eq!(
        steps.len(),
        5,
        "one rerun step each in gate, Windows cross, PR Mac, manual Mac and release sweep"
    );
    for step in steps {
        let field = |key: &str| {
            step.lines()
                .find_map(|line| line.trim().strip_prefix(key))
                .map(str::trim)
                .unwrap_or_else(|| panic!("the step sets {key}:\n{step}"))
        };
        assert!(field("if:").starts_with("failure() && "), "{step}");
        assert_eq!(field("continue-on-error:"), "true", "{step}");
        assert!(
            field("run:").starts_with("just rerun-failed")
                && field("run:").ends_with(&format!(" {archive}")),
            "the step re-runs from its test step's archive:\n{step}"
        );
        let minutes: u64 = field("timeout-minutes:")
            .parse()
            .expect("a whole number of minutes");
        assert!(
            minutes >= hung_reruns_minutes,
            "{minutes} minutes cannot hold {} re-runs, each extracting the archive, of a test nextest ends after {}s:\n{step}",
            rerun_times(),
            period * terminate_after
        );
    }
} // llmlint: ignore-end[changed_behavior_has_e2e]

/// The gate's instrumented suite archives the build it is about to run, under
/// the same flags, before any tier runs it, and only when CI names where.
///
/// The suite runs as one Nx project per tier, so "before" is the project
/// graph's to hold: every tier's instrumented `test` waits on the crate's
/// `test-archive` target, and every uninstrumented `test-quick` on its
/// `test-quick-archive`. A tier that did not would race the archive, and a test
/// it failed could be missing from the build the re-run step extracts.
// llmlint: ignore-block[changed_behavior_has_e2e] running this line needs
// cargo-llvm-cov, which the cross legs that run every test do not install, so a
// test executing it could only pass there by skipping. The uninstrumented
// archive line is executed above; this one is held to the same shape, and the
// gate job runs it for real on every run.
#[test]
fn the_instrumented_suite_archives_the_build_it_runs_before_running_it() {
    let justfile = fs::read_to_string(repo_root().join("justfile")).expect("the justfile reads");
    let body = |recipe: &str| -> Vec<String> {
        justfile
            .lines()
            .skip_while(|line| {
                !line.starts_with(&format!("{recipe} ")) && *line != format!("{recipe}:")
            })
            .skip(1)
            .take_while(|line| line.starts_with("    "))
            .map(str::to_owned)
            .collect()
    };
    let archive = body("_crate-test-archive");
    let run = body("_tier-test");
    let flags = "RUSTFLAGS=\"-D warnings\" cargo llvm-cov --no-report nextest";
    assert!(
        run.iter()
            .any(|line| line.contains(flags) && line.contains("--locked -E")),
        "every tier runs the instrumented suite under the denied-warnings flags:\n{run:#?}"
    );
    assert!(
        archive
            .iter()
            .any(|line| line.contains(&format!(
                "{flags}-archive --locked --archive-file \"$ONEPIPELINE_TEST_ARCHIVE\""
            )) && line.contains("[ -z \"${ONEPIPELINE_TEST_ARCHIVE:-}\" ] ||")),
        "the archive is built as the run builds, only when CI names where:\n{archive:#?}"
    );

    let projects = [
        "project.json",
        "tests/e2e/project.json",
        "tests/contract/project.json",
        "tests/note/project.json",
    ];
    for path in projects {
        let declared: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(repo_root().join(path)).expect("the project reads"),
        )
        .expect("the project parses");
        let name = declared["name"].as_str().expect("the project is named");
        let tier = if name == "onepipeline" {
            "test-rest"
        } else {
            "test"
        };
        for (target, archive) in [(tier, "test-archive"), ("test-quick", "test-quick-archive")] {
            let needs = &declared["targets"][target]["dependsOn"];
            let waits = needs.as_array().is_some_and(|needs| {
                needs.iter().any(|need| {
                    need.as_str() == Some(archive)
                        || (need["target"] == archive
                            && need["projects"].as_array().is_some_and(|projects| {
                                projects.iter().any(|project| project == "onepipeline")
                            }))
                })
            });
            assert!(
                waits,
                "{name}:{target} does not wait on onepipeline:{archive}, so it can run before the build is archived: {needs}"
            );
        }
    }
} // llmlint: ignore-end[changed_behavior_has_e2e]

/// Two binaries of tests, each recording every run of itself in a ledger of its
/// own, so a shard's share is read off what ran rather than off what nextest said.
const PARTITION_TESTS: &str = r#"
pub fn ledger(name: &str) {
    use std::io::Write;
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ledger");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(name))
        .unwrap()
        .write_all(b".")
        .unwrap();
}

#[cfg(test)]
mod tests {
    #[test] fn one() { super::ledger("lib-one"); }
    #[test] fn two() { super::ledger("lib-two"); }
    #[test] fn three() { super::ledger("lib-three"); }
    #[test] fn four() { super::ledger("lib-four"); }
    #[test] fn five() { super::ledger("lib-five"); }
}
"#;

const PARTITION_SUITE: &str = r#"
#[test] fn one() { partition_fixture::ledger("suite-one"); }
#[test] fn two() { partition_fixture::ledger("suite-two"); }
#[test] fn three() { partition_fixture::ledger("suite-three"); }
#[test] fn four() { partition_fixture::ledger("suite-four"); }
"#;

const PARTITION_LEDGERS: [&str; 9] = [
    "lib-one",
    "lib-two",
    "lib-three",
    "lib-four",
    "lib-five",
    "suite-one",
    "suite-two",
    "suite-three",
    "suite-four",
];

/// The Windows leg split across jobs, as each job runs a tier: the recipe line
/// `_tier-test-quick` runs, given the partition `ci.yml` hands that job. The jobs'
/// shares between them run every test exactly once, a run with no partition runs
/// them all, and a partition no other job makes up the rest of is refused before
/// anything runs.
#[test]
fn a_split_leg_runs_every_test_in_exactly_one_shard_and_refuses_a_share_nothing_completes() {
    let fixture = Fixture::new(
        "partitioned",
        "[package]\nname = \"partition-fixture\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[workspace]\n",
        &[
            ("src/lib.rs", PARTITION_TESTS),
            ("tests/suite.rs", PARTITION_SUITE),
        ],
    );
    let locked = fixture
        .command(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["generate-lockfile", "--offline"])
        .output()
        .expect("cargo writes the fixture's lock");
    assert!(locked.status.success(), "the fixture's lock is written");
    // The recipe's own line, with the tier's filter as its argument and the
    // repository's scripts reached from the fixture's root.
    let line = format!(
        "set -- 'all()'; {}",
        recipe_line("_tier-test-quick filter", "nextest run")
            .replace("bash scripts/", "bash \"$ONEPIPELINE_REPO\"/scripts/")
    );
    let repo = repo_root();
    let repo = repo.to_str().expect("a UTF-8 path");
    let ran = |fixture: &Fixture| -> Vec<usize> {
        PARTITION_LEDGERS
            .iter()
            .map(|test| {
                fs::metadata(fixture.path("ledger").join(test)).map_or(0, |ledger| {
                    usize::try_from(ledger.len()).expect("a run count fits")
                })
            })
            .collect()
    };

    let mut shares = Vec::new();
    for shard in 1..=3 {
        let before = ran(&fixture);
        let partition = format!("count:{shard}/3");
        let (output, said) = fixture.script_line(
            &line,
            &[
                ("ONEPIPELINE_REPO", repo),
                ("ONEPIPELINE_TEST_PARTITION", &partition),
            ],
        );
        assert!(output.status.success(), "shard {partition} runs:\n{said}");
        let share: usize = ran(&fixture)
            .iter()
            .zip(&before)
            .map(|(after, before)| after - before)
            .sum();
        shares.push(share);
    }
    assert_eq!(
        ran(&fixture),
        vec![1; PARTITION_LEDGERS.len()],
        "the three shards did not run each test exactly once between them (shares {shares:?})"
    );
    assert!(
        shares.iter().all(|share| *share > 0),
        "a shard ran nothing: {shares:?}"
    );

    let (output, said) = fixture.script_line(&line, &[("ONEPIPELINE_REPO", repo)]);
    assert!(output.status.success(), "the unsplit run runs:\n{said}");
    assert_eq!(
        ran(&fixture),
        vec![2; PARTITION_LEDGERS.len()],
        "with no partition every test runs:\n{said}"
    );

    for refused in [
        "count:4/3",
        "count:0/3",
        "hash:1/2",
        "count:1/2 ",
        "slice:1/2",
        "count:1/1000",
    ] {
        let (output, said) = fixture.script_line(
            &line,
            &[
                ("ONEPIPELINE_REPO", repo),
                ("ONEPIPELINE_TEST_PARTITION", refused),
            ],
        );
        assert_eq!(
            output.status.code(),
            Some(2),
            "{refused:?} was run:\n{said}"
        );
        assert!(
            said.contains(&format!("ONEPIPELINE_TEST_PARTITION holds '{refused}'")),
            "{refused:?} was refused without naming it:\n{said}"
        );
        assert_eq!(
            ran(&fixture),
            vec![2; PARTITION_LEDGERS.len()],
            "{refused:?} ran tests before it was refused:\n{said}"
        );
    }
}

/// A `ci.yml` splitting its Windows leg three ways, as the repository's own does
/// two: a `cross` job with a `shard` axis and the partition it hands each shard.
const SHARDS_CI: &str = "jobs:\n  cross:\n    strategy:\n      matrix:\n        os: [windows-latest]\n        shard: [1, 2, 3]\n    env:\n      ONEPIPELINE_TEST_PARTITION: count:${{ matrix.shard }}/3\n";

/// A justfile laid out as the repository's is: each `_<project>-test-quick`
/// recipe runs one tier through `_tier-test-quick`, beside the archive recipe.
const SHARDS_JUSTFILE: &str = "lib-tier := \"kind(lib)\"\nsuite2-tier := \"binary(suite) or binary(more)\"\n\n_tier-test-quick filter:\n    @true\n\n_crate-test-quick: (_tier-test-quick lib-tier)\n\n_suite-test-quick: (_tier-test-quick suite2-tier)\n\n_crate-test-quick-archive:\n    @true\n";

/// A second binary in one tier, so a deal that went on counting across binaries
/// rather than starting again for each would put its tests in other shards.
const SHARDS_MORE: &str = "#[test] fn alpha() {}\n#[test] fn beta() {}\n";

/// The Windows inventory of the fixture's leg: every test its two binaries hold,
/// and one more that Windows builds and this host does not, which no listing here
/// can account for and the deal over the inventory has to.
const SHARDS_INVENTORY: &str = "# Every test the Windows leg of run 1 (job 2) ran, at 3.\n\
lib-tier\tpartition-fixture\ttests::five\n\
lib-tier\tpartition-fixture\ttests::four\n\
lib-tier\tpartition-fixture\ttests::one\n\
lib-tier\tpartition-fixture\ttests::three\n\
lib-tier\tpartition-fixture\ttests::two\n\
lib-tier\tpartition-fixture\ttests::windows_only\n\
suite2-tier\tpartition-fixture::more\talpha\n\
suite2-tier\tpartition-fixture::more\tbeta\n\
suite2-tier\tpartition-fixture::suite\tfour\n\
suite2-tier\tpartition-fixture::suite\tone\n\
suite2-tier\tpartition-fixture::suite\tthree\n\
suite2-tier\tpartition-fixture::suite\ttwo\n";

/// `just cross-shards`, run from a fixture repository holding `ci`, `justfile` and
/// the Windows `inventory` beside the partition fixture's two binaries.
fn cross_shards(case: &str, ci: &str, justfile: &str, inventory: &str) -> (Output, String) {
    let fixture = Fixture::new(
        case,
        "[package]\nname = \"partition-fixture\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[workspace]\n",
        &[
            ("src/lib.rs", PARTITION_TESTS),
            ("tests/suite.rs", PARTITION_SUITE),
            ("tests/more.rs", SHARDS_MORE),
            (".github/workflows/ci.yml", ci),
            ("justfile", justfile),
            (".github/windows-test-inventory.tsv", inventory),
        ],
    );
    let locked = fixture
        .command(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["generate-lockfile", "--offline"])
        .output()
        .expect("cargo writes the fixture's lock");
    assert!(locked.status.success(), "the fixture's lock is written");
    let output = fixture
        .command("node")
        .arg(repo_root().join(".github/cross-shards.mjs"))
        .stdin(Stdio::null())
        .output()
        .expect("node runs the check");
    let said = format!("{}{}", stdout(&output), stderr(&output));
    (output, said)
}

/// What says the Windows leg's split drops and repeats nothing: it lists every
/// tier the justfile runs, whole and once per shard `ci.yml` hands out, and passes
/// only when each test is in exactly one shard. A split it cannot account for —
/// shards out of order, a partition that is not the axis's own, a tier it cannot
/// place, no split at all — is refused by name rather than proved over less. And
/// the same deal, once it has reproduced nextest's own shards, places every test of
/// the Windows inventory in exactly one shard, a test this host does not build
/// among them; an inventory it cannot read whole is refused.
#[test]
fn the_shard_check_accounts_for_every_tier_and_refuses_a_split_it_cannot_prove() {
    let (output, said) = cross_shards(
        "shards-proved",
        SHARDS_CI,
        SHARDS_JUSTFILE,
        SHARDS_INVENTORY,
    );
    assert!(output.status.success(), "the split is proved:\n{said}");
    for line in [
        "cross-shards: lib-tier: 5 tests, 2 + 2 + 1 across 3 shards",
        "cross-shards: suite2-tier: 6 tests, 3 + 2 + 1 across 3 shards",
        "cross-shards: ok, 11 tests across 2 tiers, each in exactly one of 3 shards",
        "cross-shards: Every test the Windows leg of run 1 (job 2) ran, at 3.",
        "cross-shards: windows ok, 12 tests, 5 + 4 + 3 across 3 shards, each in exactly one; 1 of them this host does not list",
    ] {
        assert!(said.contains(line), "no `{line}`:\n{said}");
    }

    for (case, ci, justfile, says) in [
        (
            "shards-misnumbered",
            SHARDS_CI.replace("[1, 2, 3]", "[1, 3, 2]"),
            SHARDS_JUSTFILE.to_owned(),
            "the `shard` axis is [1,3,2], not 1 to 3 in order",
        ),
        (
            "shards-mishanded",
            SHARDS_CI.replace("}}/3", "}}/2"),
            SHARDS_JUSTFILE.to_owned(),
            "the `cross` job hands its shards \"count:${{ matrix.shard }}/2\"",
        ),
        (
            "shards-unsplit",
            SHARDS_CI.replace("        shard: [1, 2, 3]\n", ""),
            SHARDS_JUSTFILE.to_owned(),
            "the `cross` job has no `shard` axis",
        ),
        (
            "shards-no-leg",
            "jobs:\n  gate:\n    runs-on: ubuntu-latest\n".to_owned(),
            SHARDS_JUSTFILE.to_owned(),
            "ci.yml has no `cross` job",
        ),
        (
            "shards-no-tiers",
            SHARDS_CI.to_owned(),
            "_tier-test-quick filter:\n    @true\n".to_owned(),
            "the justfile runs no tier through `_tier-test-quick`",
        ),
        (
            "shards-unplaced",
            SHARDS_CI.to_owned(),
            SHARDS_JUSTFILE.replace(
                "_suite-test-quick: (_tier-test-quick suite2-tier)",
                "_suite-test-quick: (_tier-test-quick suite2-tier) (_tier-test-quick lib-tier)",
            ),
            "_suite-test-quick runs 2 tiers through `_tier-test-quick`, not one",
        ),
    ] {
        let (output, said) = cross_shards(case, &ci, &justfile, SHARDS_INVENTORY);
        assert_eq!(output.status.code(), Some(1), "{case} was proved:\n{said}");
        assert!(
            said.contains(says),
            "{case} was refused without `{says}`:\n{said}"
        );
        assert!(!said.contains("cross-shards: ok"), "{case}:\n{said}");
    }

    for (case, inventory, says) in [
        (
            "inventory-untiered",
            SHARDS_INVENTORY.replace(
                "suite2-tier\tpartition-fixture::suite\ttwo",
                "e2e-tier\tpartition-fixture::suite\ttwo",
            ),
            "names e2e-tier, which no `test-quick` recipe runs",
        ),
        (
            "inventory-twice",
            format!("{SHARDS_INVENTORY}lib-tier\tpartition-fixture\ttests::one\n"),
            "lists partition-fixture tests::one a second time",
        ),
        (
            "inventory-unreadable",
            format!("{SHARDS_INVENTORY}lib-tier\tpartition-fixture\n"),
            "is not a tier, a binary and a test",
        ),
        (
            "inventory-partial",
            SHARDS_INVENTORY
                .lines()
                .filter(|line| !line.starts_with("suite2-tier"))
                .map(|line| format!("{line}\n"))
                .collect(),
            "holds no test of suite2-tier, so it is not a whole leg",
        ),
    ] {
        let (output, said) = cross_shards(case, SHARDS_CI, SHARDS_JUSTFILE, &inventory);
        assert_eq!(output.status.code(), Some(1), "{case} was proved:\n{said}");
        assert!(
            said.contains(says),
            "{case} was refused without `{says}`:\n{said}"
        );
        assert!(!said.contains("windows ok"), "{case}:\n{said}");
    }
}

/// One `test-quick` target's nextest run as a green Windows leg's log records it:
/// under the Nx line that started it, with its `Starting` line, a status line per
/// test it named, and its `Summary`.
struct LoggedRun<'a> {
    project: &'a str,
    binary: &'a str,
    tests: Vec<&'a str>,
    starting: Option<usize>,
    summary: String,
}

impl<'a> LoggedRun<'a> {
    fn green(project: &'a str, binary: &'a str, tests: &[&'a str]) -> Self {
        Self {
            project,
            binary,
            tests: tests.to_vec(),
            starting: Some(tests.len()),
            summary: format!("{0} tests run: {0} passed, 0 skipped", tests.len()),
        }
    }
}

/// The four runs of a green leg, one per tier.
fn green_leg() -> Vec<LoggedRun<'static>> {
    vec![
        LoggedRun::green("onepipeline-contract", "onepipeline::contract", &["a"]),
        LoggedRun::green("onepipeline-note-journeys", "onepipeline::note", &["n"]),
        LoggedRun::green("onepipeline-e2e", "onepipeline::e2e", &["x::b", "x::a"]),
        LoggedRun::green("onepipeline", "onepipeline", &["unit"]),
    ]
}

/// A job's log as GitHub serves it: a timestamp before each line, a byte-order
/// mark before the first, and colour in the status lines.
fn leg_log(runs: &[LoggedRun<'_>]) -> String {
    let mut log = String::from("\u{feff}2026-10-17T07:22:19.0Z Current runner version\n");
    for run in runs {
        log.push_str(&format!(
            "2026-10-17T07:22:20.0Z > nx run {}:test-quick\n",
            run.project
        ));
        if let Some(starting) = run.starting {
            log.push_str(&format!(
                "2026-10-17T07:22:20.1Z \u{1b}[32;1m    Starting\u{1b}[0m {starting} tests across 1 binary\n"
            ));
        }
        for (at, test) in run.tests.iter().enumerate() {
            log.push_str(&format!(
                "2026-10-17T07:22:20.2Z \u{1b}[32;1m        PASS\u{1b}[0m [   0.10{at}s] ({}/{}) \u{1b}[35;1m{}\u{1b}[0m {test}\n",
                at + 1,
                run.tests.len(),
                run.binary
            ));
        }
        log.push_str(&format!(
            "2026-10-17T07:22:20.3Z      Summary [   0.5s] {}\n",
            run.summary
        ));
    }
    log
}

/// The four projects' `test-quick` targets, as this repository declares them, and
/// the justfile recipes they run.
const INVENTORY_PROJECTS: [(&str, &str, &str); 4] = [
    ("project.json", "onepipeline", "just _crate-test-quick"),
    (
        "tests/e2e/project.json",
        "onepipeline-e2e",
        "just _e2e-test-quick",
    ),
    (
        "tests/contract/project.json",
        "onepipeline-contract",
        "just _contract-test-quick",
    ),
    (
        "tests/note/project.json",
        "onepipeline-note-journeys",
        "just _note-test-quick",
    ),
];

const INVENTORY_JUSTFILE: &str = "_tier-test-quick filter:\n    @true\n\n_crate-test-quick: (_tier-test-quick rest-tier)\n\n_e2e-test-quick: _preflight (_tier-test-quick e2e-tier)\n\n_contract-test-quick: (_tier-test-quick contract-tier)\n\n_note-test-quick: (_tier-test-quick note-tier)\n";

/// `node .github/windows-inventory.mjs`, from a fixture repository declaring
/// `projects` — path, name and `test-quick` command — over `log`, with `args` after the log's path — or the usual
/// three when `args` is `None`. Exit code, the inventory written, and stderr.
fn windows_inventory(
    case: &str,
    projects: &[(&str, &str, &str)],
    justfile: &str,
    log: &str,
    args: Option<&[&str]>,
) -> (Option<i32>, String, String) {
    let projects: Vec<(String, String)> = projects
        .iter()
        .map(|(path, name, command)| {
            (
                (*path).to_owned(),
                format!(
                    "{{\"name\": \"{name}\", \"targets\": {{\"test-quick\": {{\"command\": \"{command}\"}}}}}}"
                ),
            )
        })
        .collect();
    let mut files: Vec<(&str, &str)> = projects
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    files.push(("justfile", justfile));
    files.push(("leg.log", log));
    let fixture = Fixture::new(case, RERUN_MANIFEST, &files);
    let output = fixture
        .command("node")
        .arg(repo_root().join(".github/windows-inventory.mjs"))
        .arg(fixture.path("leg.log"))
        .args(args.unwrap_or(&["11", "22", "abc"]))
        .stdin(Stdio::null())
        .output()
        .expect("node runs the extractor");
    (output.status.code(), stdout(&output), stderr(&output))
}

/// A log the extractor has to refuse: its case, the projects and justfile beside it, the log,
/// the arguments after the log's path, and what the refusal says.
struct Refused<'a>(
    &'a str,
    Vec<(&'a str, &'a str, &'a str)>,
    String,
    String,
    Option<&'a [&'a str]>,
    &'a str,
);

/// The inventory is written off a green leg's own log, every test it ran under the
/// tier its project's `test-quick` target runs, read off where those are declared.
/// A log whose counts do not add up, or that does not hold one finished run of each
/// tier, and a project that does not run one tier, are refused rather than written
/// down short.
#[test]
fn the_windows_inventory_is_every_test_a_finished_leg_ran_and_nothing_less() {
    let (code, written, said) = windows_inventory(
        "inventory-green",
        &INVENTORY_PROJECTS,
        INVENTORY_JUSTFILE,
        &leg_log(&green_leg()),
        None,
    );
    assert_eq!(code, Some(0), "{said}");
    assert_eq!(
        written,
        "# Every test the Windows leg of run 11 (job 22) ran, at abc.\n\
         # Written by .github/windows-inventory.mjs from that job's log; checked by\n\
         # .github/cross-shards.mjs. Columns: tier, binary, test.\n\
         contract-tier\tonepipeline::contract\ta\n\
         e2e-tier\tonepipeline::e2e\tx::a\n\
         e2e-tier\tonepipeline::e2e\tx::b\n\
         note-tier\tonepipeline::note\tn\n\
         rest-tier\tonepipeline\tunit\n"
    );

    let changed = |change: &dyn Fn(&mut Vec<LoggedRun<'static>>)| {
        let mut runs = green_leg();
        change(&mut runs);
        leg_log(&runs)
    };
    let green = leg_log(&green_leg());
    let cases: Vec<Refused<'_>> = vec![
        Refused(
            "inventory-usage",
            INVENTORY_PROJECTS.to_vec(),
            INVENTORY_JUSTFILE.to_owned(),
            green.clone(),
            Some(&["11", "22"]),
            "usage: windows-inventory.mjs <job log> <run> <job> <commit>",
        ),
        Refused(
            "inventory-surplus",
            INVENTORY_PROJECTS.to_vec(),
            INVENTORY_JUSTFILE.to_owned(),
            green.clone(),
            Some(&["11", "22", "abc", "extra"]),
            "usage: windows-inventory.mjs <job log> <run> <job> <commit>",
        ),
        Refused(
            "inventory-unknown-project",
            INVENTORY_PROJECTS.to_vec(),
            INVENTORY_JUSTFILE.to_owned(),
            changed(&|runs| runs[0].project = "onepipeline-mystery"),
            None,
            "onepipeline-mystery:test-quick runs no tier this inventory knows",
        ),
        Refused(
            "inventory-reported-twice",
            INVENTORY_PROJECTS.to_vec(),
            INVENTORY_JUSTFILE.to_owned(),
            changed(&|runs| runs[2].tests.push("x::a")),
            None,
            "e2e-tier: onepipeline::e2e x::a reported twice",
        ),
        Refused(
            "inventory-tier-twice",
            INVENTORY_PROJECTS.to_vec(),
            INVENTORY_JUSTFILE.to_owned(),
            changed(&|runs| {
                runs.pop();
                runs.push(LoggedRun::green(
                    "onepipeline-contract",
                    "onepipeline::contract",
                    &["a"],
                ));
            }),
            None,
            "the log holds 4 test-quick runs over contract-tier, note-tier, e2e-tier, not one of each tier",
        ),
        Refused(
            "inventory-unstarted",
            INVENTORY_PROJECTS.to_vec(),
            INVENTORY_JUSTFILE.to_owned(),
            changed(&|runs| runs[1].starting = None),
            None,
            "note-tier: the run has no Starting or no Summary line, so it did not finish",
        ),
        Refused(
            "inventory-unreported",
            INVENTORY_PROJECTS.to_vec(),
            INVENTORY_JUSTFILE.to_owned(),
            changed(&|runs| {
                runs[2].tests.pop();
            }),
            None,
            "e2e-tier: 1 tests named, 2 starting, 2 run and 2 passed",
        ),
        Refused(
            "inventory-overcounted",
            INVENTORY_PROJECTS.to_vec(),
            INVENTORY_JUSTFILE.to_owned(),
            changed(&|runs| runs[2].summary = "3 tests run: 3 passed, 0 skipped".to_owned()),
            None,
            "e2e-tier: 2 tests named, 2 starting, 3 run and 3 passed",
        ),
        Refused(
            "inventory-failed",
            INVENTORY_PROJECTS.to_vec(),
            INVENTORY_JUSTFILE.to_owned(),
            changed(&|runs| runs[2].summary = "2 tests run: 1 passed, 1 failed".to_owned()),
            None,
            "e2e-tier: 2 tests named, 2 starting, 2 run and 1 passed",
        ),
        Refused(
            "inventory-two-tier-recipe",
            INVENTORY_PROJECTS.to_vec(),
            INVENTORY_JUSTFILE.replace(
                "_note-test-quick: (_tier-test-quick note-tier)",
                "_note-test-quick: (_tier-test-quick note-tier) (_tier-test-quick e2e-tier)",
            ),
            green.clone(),
            None,
            "onepipeline-note-journeys:test-quick runs _note-test-quick, which runs 2 tiers, not one",
        ),
        Refused(
            "inventory-not-a-recipe",
            INVENTORY_PROJECTS
                .iter()
                .map(|&(path, name, command)| {
                    (path, name, if name == "onepipeline-e2e" { "cargo nextest run" } else { command })
                })
                .collect(),
            INVENTORY_JUSTFILE.to_owned(),
            green.clone(),
            None,
            "onepipeline-e2e:test-quick runs \"cargo nextest run\", not one justfile recipe",
        ),
        Refused(
            "inventory-no-projects",
            Vec::new(),
            INVENTORY_JUSTFILE.to_owned(),
            green.clone(),
            None,
            "no project here has a `test-quick` target",
        ),
    ];
    for Refused(case, projects, justfile, log, args, says) in cases {
        let (code, written, said) = windows_inventory(case, &projects, &justfile, &log, args);
        assert_eq!(code, Some(1), "{case} was written:\n{written}");
        assert!(
            said.contains(says),
            "{case} was refused without `{says}`:\n{said}"
        );
        assert!(written.is_empty(), "{case} wrote an inventory:\n{written}");
    }
}

/// Nx's answer to `show <args> --json`, asked through the workspace's one entry
/// point to it.
fn nx_shows(args: &[&str]) -> serde_json::Value {
    let output = Command::new(bash())
        .arg(repo_root().join("scripts/nx.sh"))
        .arg("show")
        .args(args)
        .arg("--json")
        .current_dir(repo_root())
        .env("ONEPIPELINE_NX_SHOW_OUTPUT", "1")
        .stdin(Stdio::null())
        .output()
        .expect("bash runs Nx");
    assert!(
        output.status.success(),
        "nx show {args:?} failed:\n{}",
        stderr(&output)
    );
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("nx show {args:?} is not JSON: {error}"))
}

/// Nx caches every `test-quick` target, and two shards of one tree differ in
/// nothing but `ONEPIPELINE_TEST_PARTITION` — so unless that is one of a target's
/// inputs, a warm cache replays one shard's pass for the next, which then runs
/// nothing. Read off Nx's own resolution of each project's target, defaults merged:
/// every cached `test-quick` is keyed on the partition.
#[test]
fn a_shards_cached_pass_never_stands_in_for_another_shards_run() {
    let projects = nx_shows(&["projects", "--with-target", "test-quick"]);
    let projects = projects
        .as_array()
        .expect("Nx lists the projects with a `test-quick` target");
    assert!(!projects.is_empty(), "no project has a `test-quick` target");
    let partition = serde_json::json!({ "env": "ONEPIPELINE_TEST_PARTITION" });
    for project in projects {
        let project = project.as_str().expect("a project is named");
        let target = nx_shows(&["target", &format!("{project}:test-quick")]);
        if target["cache"] != true {
            continue;
        }
        let inputs = target["inputs"]
            .as_array()
            .unwrap_or_else(|| panic!("{project}:test-quick resolves no inputs: {target}"));
        assert!(
            inputs.contains(&partition),
            "{project}:test-quick is cached without the partition among its inputs, so one \
             shard's pass replays for another: {inputs:?}"
        );
    }
}

/// The log grammar the inventory is read with is nextest's, so it is held against
/// nextest's own: the partition fixture run under this repository's `ci` profile —
/// the one the Windows leg reports with — in colour, written into a leg's log once
/// per tier under the Nx line that starts it and with the timestamp GitHub puts
/// before every line, has to come back as every test it ran.
#[test]
fn the_windows_inventory_reads_what_nextest_itself_reports() {
    // The binaries the profile's own filters name, as the scheduling fixture has.
    let fixture = Fixture::new(
        "inventory-real-nextest",
        SCHEDULING_MANIFEST,
        &[
            (
                "src/lib.rs",
                "#[cfg(test)]\nmod tests {\n    #[test]\n    fn unit() {}\n}\n",
            ),
            (".config/nextest.toml", &repository_config()),
            (
                "tests/e2e.rs",
                "#[test]\nfn one() {}\n#[test]\nfn two() {}\n",
            ),
            ("tests/note.rs", "#[test]\nfn one() {}\n"),
            ("tests/release_channel.rs", "#[test]\nfn one() {}\n"),
            ("tests/oneagentgraph_liveness.rs", "#[test]\nfn one() {}\n"),
        ],
    );
    let nextest = |args: &[&str]| {
        fixture
            .command(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .arg("nextest")
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("nextest runs in the fixture")
    };
    let ran = nextest(&["run", "--profile", "ci", "--color", "always"]);
    let reported = format!("{}{}", stdout(&ran), stderr(&ran));
    assert!(
        ran.status.success(),
        "the fixture's run passed:\n{reported}"
    );
    let listed = nextest(&["list", "--message-format", "oneline"]);
    assert!(listed.status.success(), "{}", stderr(&listed));
    let tests: Vec<String> = stdout(&listed)
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(binary, test)| format!("{binary}\t{test}"))
        .collect();
    assert_eq!(tests.len(), 6, "{}", stdout(&listed));

    let mut log = String::new();
    for (_, project, _) in INVENTORY_PROJECTS {
        log.push_str(&format!(
            "2026-10-17T07:22:20.0Z > nx run {project}:test-quick\n"
        ));
        for line in reported.lines() {
            log.push_str(&format!("2026-10-17T07:22:20.1Z {line}\n"));
        }
    }
    let (code, written, said) = windows_inventory(
        "inventory-real-nextest-log",
        &INVENTORY_PROJECTS,
        INVENTORY_JUSTFILE,
        &log,
        None,
    );
    assert_eq!(code, Some(0), "{said}\n{log}");
    let mut expected: Vec<String> = ["rest-tier", "e2e-tier", "contract-tier", "note-tier"]
        .iter()
        .flat_map(|tier| tests.iter().map(move |test| format!("{tier}\t{test}")))
        .collect();
    expected.sort();
    let inventoried: Vec<&str> = written
        .lines()
        .filter(|line| !line.starts_with('#'))
        .collect();
    assert_eq!(inventoried, expected, "{said}");
}
