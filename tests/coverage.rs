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
//! the four tests below drive that recipe.
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
/// The recipe is pointed at a scratch tree by argument, because the tree it
/// cleans by default is holding the binaries and profiles of the run executing
/// this test. What is planted there is this executable, copied: under the
/// coverage tier that is precisely the artifact at issue, an instrumented object
/// file carrying a coverage map, and a fixture the report would have skipped
/// proves nothing about a step that exists to keep one out.
///
/// The scratch tree is named with a quote and a space in it because the recipe is
/// handed that name from outside: a step that read it as shell source would
/// remove the wrong thing or fail to parse, and from anywhere but here both look
/// like a removal that worked.
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

    let cleaned = just(&["_crate-coverage-clean".as_ref(), scratch.as_os_str()]);
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

/// And the tree it would remove by default is *this* one: the one this run's own
/// profiles are being written into. Nothing is removed here — that tree is the
/// running tier's — so what this holds is the two paths against each other.
///
/// A recipe that removed a plausible directory nothing writes to would pass every
/// removal assertion above and fix nothing, so the default is asked of `just`
/// rather than restated here, and held against `LLVM_PROFILE_FILE` — which the
/// profiling runtime was pointed at by the same `cargo llvm-cov` invocation that
/// built this binary. This is also what a run configured to build somewhere else
/// runs into: the tier fails here rather than cleaning a tree nothing wrote to.
#[test]
fn the_clean_steps_default_tree_is_the_one_this_run_is_measured_from() {
    let Some(measured) = profile_directory() else {
        // Uninstrumented: `just test-quick` runs this same suite on the
        // cross-platform legs, where there is no instrumented tree to clean.
        return;
    };
    let evaluated = just(&["--evaluate".as_ref(), "llvm-cov-target-dir".as_ref()]);
    assert!(
        evaluated.status.success(),
        "the justfile does not resolve `llvm-cov-target-dir`: {}",
        said(&evaluated)
    );
    let named = PathBuf::from(String::from_utf8_lossy(&evaluated.stdout).trim());
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

/// A tree that does not exist yet is the ordinary first run of a fresh clone: the
/// step removes nothing and says nothing, rather than failing the tier before it
/// has built anything.
#[test]
fn the_clean_step_passes_over_a_tree_that_was_never_built() {
    let never = repo_root().join("target/coverage-clean-probe-never-built");
    let _ = fs::remove_dir_all(&never);

    let cleaned = just(&["_crate-coverage-clean".as_ref(), never.as_os_str()]);
    assert!(
        cleaned.status.success(),
        "the clean step failed over a tree that was never built, which is every \
         first run: {}",
        said(&cleaned)
    );
    assert!(
        !never.exists(),
        "the clean step created {}, which the instrumented build owns",
        never.display()
    );
}

/// And it refuses what does not reach this clone's build directory, rather than
/// removing it.
///
/// `.cargo/config.toml` puts every build in this clone under `<clone>/target`, so
/// a tree to clean that reaches anywhere else is a misconfiguration rather than a
/// tree to remove — and the bound is that directory rather than the clone,
/// because the clone also holds `src` and `.git`.
///
/// Each case is one a step that checked the *spelling* of what it was handed
/// would accept: a path beside the build tree but inside the clone, an absolute
/// path outside the clone, that same path reached through `..`, a symlink inside
/// the build tree pointing out of it, and a name carrying a quote. The empty and
/// relative ones are there for a different reason — they reach nothing at all,
/// which is the shape a mistyped argument takes, and the step has to refuse them
/// rather than fall back to some tree of its own.
///
/// Every case that reaches anything reaches a fixture this test made, so were the
/// check gone the journey would delete its own fixtures and nothing else — it
/// cannot be the accident it guards against. That is also why `<clone>/target`
/// itself is not among them, though the step refuses it: the tier running this
/// test is inside it.
#[test]
fn the_clean_step_refuses_what_does_not_reach_this_clones_build_directory() {
    let outside = env::temp_dir().join(format!(
        "onepipeline-coverage-clean-outside-{}",
        std::process::id()
    ));
    let quoted = outside.join("a tree named with ' in it");
    fs::create_dir_all(outside.join("below"))
        .unwrap_or_else(|e| panic!("could not create {}: {e}", outside.display()));
    fs::create_dir_all(&quoted)
        .unwrap_or_else(|e| panic!("could not create {}: {e}", quoted.display()));
    let through_dots = outside.join("below/..");

    // Inside the clone and beside the build tree, which is the case that says the
    // bound is the build directory and not the repository.
    let beside = repo_root().join("coverage-clean-probe-beside-the-build-tree");
    fs::create_dir_all(&beside)
        .unwrap_or_else(|e| panic!("could not create {}: {e}", beside.display()));

    let mut refused: Vec<&OsStr> = vec![
        beside.as_os_str(),
        outside.as_os_str(),
        through_dots.as_os_str(),
        quoted.as_os_str(),
        OsStr::new(""),
        OsStr::new("relative/dir"),
    ];
    // A symlink is the one spelling that sits inside the build tree and still
    // leaves it, so it is the case that says the step resolves rather than reads.
    #[cfg(unix)]
    let link = repo_root().join("target/coverage-clean-probe-link");
    #[cfg(unix)]
    {
        let _ = fs::remove_file(&link);
        fs::create_dir_all(repo_root().join("target"))
            .unwrap_or_else(|e| panic!("could not create the build directory: {e}"));
        std::os::unix::fs::symlink(&outside, &link)
            .unwrap_or_else(|e| panic!("could not link {}: {e}", link.display()));
        refused.push(link.as_os_str());
    }

    for target in refused {
        let attempt = just(&["_crate-coverage-clean".as_ref(), target]);
        assert!(
            !attempt.status.success(),
            "the clean step accepted {target:?} as a tree to remove whole: {}",
            said(&attempt)
        );
        let complaint = String::from_utf8_lossy(&attempt.stderr);
        assert!(
            complaint.contains("refusing to remove"),
            "the refusal of {target:?} does not say what it refused, so a run \
             that set it wrong is left to guess: {}",
            said(&attempt)
        );
    }

    assert!(
        quoted.is_dir(),
        "{} was removed by a step that had refused to remove it",
        quoted.display()
    );
    assert!(
        beside.is_dir(),
        "{} was removed by a step that had refused to remove it",
        beside.display()
    );
    #[cfg(unix)]
    fs::remove_file(&link).unwrap_or_else(|e| panic!("could not unlink {}: {e}", link.display()));
    fs::remove_dir_all(&beside)
        .unwrap_or_else(|e| panic!("could not remove {}: {e}", beside.display()));
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

/// The recipe reads the bound on what it may remove off its own working
/// directory, so this runs from the repository root, as the `coverage-clean` Nx
/// target does.
fn just(args: &[&OsStr]) -> Output {
    Command::new("just")
        .args(args)
        .current_dir(repo_root())
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
