//! The gate's coverage tier, held against the two artifacts it finds in the
//! directory it measures — one left by the run before this one, one left by this
//! one.
//!
//! The earlier run's is an instrumented *binary*. `llvm-cov report` walks that
//! directory and measures every object it finds there, not the ones this run
//! built, so a test binary whose source has since moved on is measured too, with
//! no profile to its name and every line of it counted as missed — and the 95%
//! floor fails over code this run covered. `_crate-coverage-clean` is what stops
//! it, by removing that directory whole before the first instrumented run, and
//! the three tests below drive that recipe.
//!
//! This run's own is a *truncated profile*. The cancellation journeys kill
//! instrumented processes, and one killed while the profiling runtime is still
//! flushing leaves a truncated profile in the set `_crate-coverage` merges.
//! `llvm-profdata` rejects the whole merge over a single one of those, which the
//! recipe reports as a test failure.
//!
//! So the last test plants that artifact, on every coverage run, in the directory
//! the recipe merges from — and the recipe is the assertion. Take
//! `--failure-mode all` out of the justfile and it fails again.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::{env, fs};

/// The clean step removes the instrumented tree whole, so nothing an earlier
/// build left behind can reach the report.
///
/// The recipe is pointed at a scratch directory through
/// `CARGO_LLVM_COV_TARGET_DIR` — cargo-llvm-cov's own override, which the recipe
/// reads for exactly this reason — because the real one is holding the binaries
/// and profiles of the run executing this test. What is planted there is this
/// executable, copied: under the coverage tier that is precisely the artifact at
/// issue, an instrumented object file carrying a coverage map, and a fixture the
/// report would have skipped proves nothing about a step that exists to keep it
/// out.
///
/// The scratch directory is named with a quote and a space in it because the
/// recipe takes that name from the environment: a step that read the value as
/// shell source would either remove the wrong thing or fail to parse, and both
/// look like a passing removal from anywhere but here.
#[test]
fn the_clean_step_removes_an_instrumented_binary_an_earlier_run_left() {
    // Under `target/` for the reason `build_config` puts its probes there: a
    // failed run leaves the tree that failed where the other build artifacts are.
    let scratch = repo_root().join("target/coverage-clean-probe's tree");
    let _ = fs::remove_dir_all(&scratch);
    let deps = scratch.join("debug/deps");
    fs::create_dir_all(&deps)
        .unwrap_or_else(|e| panic!("could not create {}: {e}", deps.display()));

    let running = env::current_exe().expect("the test binary knows its own path");
    let stale = deps.join("onepipeline_left_by_an_earlier_run-0123456789abcdef");
    fs::copy(&running, &stale)
        .unwrap_or_else(|e| panic!("could not plant {}: {e}", stale.display()));
    assert!(
        stale.is_file(),
        "the planted binary is not there to be removed: {}",
        stale.display()
    );

    let cleaned = just(&["_crate-coverage-clean"], Some(scratch.as_os_str()));
    assert!(
        cleaned.status.success(),
        "the clean step failed: {}",
        said(&cleaned)
    );
    assert!(
        !stale.exists(),
        "{} survived the clean step, so the report would measure it: {}",
        stale.display(),
        said(&cleaned)
    );
    assert!(
        !scratch.exists(),
        "{} survived the clean step, so it was emptied rather than removed: {}",
        scratch.display(),
        said(&cleaned)
    );
}

/// And the directory it removes is *this* one: the tree this run's own profiles
/// are being written into.
///
/// A recipe that removed a plausible directory nothing writes to would pass every
/// removal assertion above and fix nothing, so the path is asked of `just` rather
/// than restated here, and held against `LLVM_PROFILE_FILE` — which the profiling
/// runtime was pointed at by the same `cargo llvm-cov` invocation that built this
/// binary.
#[test]
fn the_clean_step_removes_the_directory_this_run_is_measured_from() {
    let Some(measured) = profile_directory() else {
        // Uninstrumented: `just test-quick` runs this same suite on the
        // cross-platform legs, where there is no instrumented tree to clean.
        return;
    };
    let evaluated = just(&["--evaluate", "llvm-cov-target-dir"], None);
    assert!(
        evaluated.status.success(),
        "the justfile does not resolve `llvm-cov-target-dir`: {}",
        said(&evaluated)
    );
    let named = PathBuf::from(String::from_utf8_lossy(&evaluated.stdout).trim());
    // Both ends resolved, so two spellings of one directory compare equal.
    let resolve = |path: &PathBuf| {
        path.canonicalize()
            .unwrap_or_else(|e| panic!("{} does not resolve: {e}", path.display()))
    };
    assert_eq!(
        resolve(&named),
        resolve(&measured),
        "the clean step removes {}, but this run is measured from {} — so the \
         stale objects it exists to remove would stay",
        named.display(),
        measured.display()
    );
}

/// And it refuses a directory outside this clone rather than removing it.
///
/// `.cargo/config.toml` puts every build under this clone into `<clone>/target`,
/// so a tree to clean that resolves anywhere else is a misconfiguration and not a
/// tree to remove — and what the recipe is handed comes from the environment.
///
/// Each case below is one a recipe that checked the *spelling* of the value would
/// accept: an absolute path outside the clone, that same path reached through
/// `..`, and one whose name carries a quote. Each resolves to a directory this
/// test made under the temporary directory, so were the check gone this journey
/// would delete its own fixture and nothing else — it cannot be the accident it
/// guards against. The relative one is there because the recipe runs from the
/// repository root, where an unresolvable relative path is the shape a mistyped
/// override actually takes.
#[test]
fn the_clean_step_refuses_a_target_directory_outside_this_clone() {
    let outside = env::temp_dir().join(format!(
        "onepipeline-coverage-clean-outside-{}",
        std::process::id()
    ));
    let quoted = outside.join("a tree named with ' in it");
    fs::create_dir_all(outside.join("below"))
        .unwrap_or_else(|e| panic!("could not create {}: {e}", outside.display()));
    fs::create_dir_all(&quoted)
        .unwrap_or_else(|e| panic!("could not create {}: {e}", quoted.display()));

    let reached_through_dots = outside.join("below/..");
    for refused in [
        outside.as_os_str(),
        reached_through_dots.as_os_str(),
        quoted.as_os_str(),
        OsStr::new("relative/dir"),
    ] {
        let attempt = just(&["_crate-coverage-clean"], Some(refused));
        assert!(
            !attempt.status.success(),
            "the clean step accepted {refused:?} as a tree to remove whole: {}",
            said(&attempt)
        );
        let complaint = String::from_utf8_lossy(&attempt.stderr);
        assert!(
            complaint.contains("refusing to remove"),
            "the refusal of {refused:?} does not say what it refused, so a run \
             that set it wrong is left to guess: {}",
            said(&attempt)
        );
    }
    assert!(
        quoted.is_dir(),
        "{} was removed by a step that had refused to remove it",
        quoted.display()
    );
    fs::remove_dir_all(&outside)
        .unwrap_or_else(|e| panic!("could not remove {}: {e}", outside.display()));
}

/// `INSTR_PROF_RAW_MAGIC_64` little-endian and not one byte more: as far into its
/// header as a killed child got. Racing the real thing instead yields profiles cut
/// off at a page boundary, but LLVM rejects both with the same "file header is
/// corrupt" — and unlike a race, eight fixed bytes are the same every run.
const TRUNCATED_HEADER: [u8; 8] = [0x81, b'r', b'f', b'o', b'r', b'p', b'l', 0xff];

#[test]
fn a_truncated_profile_left_by_a_killed_child_does_not_fail_the_coverage_step() {
    let Some(dir) = profile_directory() else {
        // Uninstrumented: `just test-quick` runs this same suite on the
        // cross-platform legs, where there is no merge to survive.
        return;
    };
    let planted = dir.join("truncated-by-a-killed-child_0.profraw");
    fs::write(&planted, TRUNCATED_HEADER)
        .unwrap_or_else(|e| panic!("could not plant {}: {e}", planted.display()));

    // A fixture the merge would happily accept proves nothing, so hold it against
    // the very tool the recipe merges with.
    let profdata = llvm_profdata().unwrap_or_else(|| {
        panic!(
            "coverage is instrumented but llvm-profdata is neither in $LLVM_PROFDATA \
             nor beside the rustc target libdir — run `rustup component add llvm-tools`"
        )
    });
    let shown = Command::new(&profdata)
        .arg("show")
        .arg(&planted)
        .output()
        .unwrap_or_else(|e| panic!("could not run {}: {e}", profdata.display()));
    assert!(
        !shown.status.success(),
        "the planted profile is readable, so it is not the failure condition the \
         coverage step has to survive: {}",
        planted.display()
    );
    let complaint = String::from_utf8_lossy(&shown.stderr);
    assert!(
        complaint.contains("file header is corrupt"),
        "the planted profile fails for the wrong reason — an empty profile is \
         skipped silently and proves nothing. llvm-profdata said: {}",
        complaint.trim()
    );
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// `just`, run the way the `coverage-clean` Nx target runs it: from the
/// repository root, over this repository's real justfile. `llvm_cov_target_dir`
/// is cargo-llvm-cov's own override, set where a caller needs the recipe pointed
/// somewhere other than the tree it is running in.
fn just(args: &[&str], llvm_cov_target_dir: Option<&OsStr>) -> Output {
    let mut command = Command::new("just");
    command.args(args).current_dir(repo_root());
    match llvm_cov_target_dir {
        Some(dir) => command.env("CARGO_LLVM_COV_TARGET_DIR", dir),
        // Removed rather than left: the recipe reads it first, so an inherited
        // one would decide what the recipe under test names.
        None => command.env_remove("CARGO_LLVM_COV_TARGET_DIR"),
    };
    command
        .output()
        .expect("just runs this repository's recipes")
}

/// Everything the run said, so an assertion's failure names the whole report
/// rather than the half it looked in.
fn said(output: &Output) -> String {
    format!(
        "exit {:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

/// The directory `cargo llvm-cov` globs `*.profraw` out of, read from the pattern
/// it points instrumented processes at. `None` when the run is not instrumented.
fn profile_directory() -> Option<PathBuf> {
    let pattern = PathBuf::from(env::var_os("LLVM_PROFILE_FILE")?);
    let dir = pattern.parent()?;
    dir.is_dir().then(|| dir.to_path_buf())
}

/// `llvm-profdata`, found the way `cargo llvm-cov` finds it: an explicit override
/// first, then the llvm-tools shipped beside the active toolchain's target libdir.
fn llvm_profdata() -> Option<PathBuf> {
    if let Some(explicit) = env::var_os("LLVM_PROFDATA") {
        return Some(PathBuf::from(explicit));
    }
    let printed = Command::new("rustc")
        .args(["--print", "target-libdir"])
        .output()
        .ok()?;
    if !printed.status.success() {
        return None;
    }
    let libdir = String::from_utf8(printed.stdout).ok()?;
    let bin = PathBuf::from(libdir.trim()).parent()?.join("bin");
    ["llvm-profdata", "llvm-profdata.exe"]
        .into_iter()
        .map(|name| bin.join(name))
        .find(|candidate| candidate.is_file())
}
