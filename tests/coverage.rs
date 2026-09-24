use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::{env, fs};

/// A copy of this test binary represents the stale coverage object. The scratch
/// path includes a quote and a space to check argument handling.
#[test]
fn the_clean_step_removes_a_test_binary_an_earlier_run_left() {
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

#[test]
fn the_default_clean_step_removes_the_instrumented_tree() {
    // Give the recipe its own clone-sized root so this test cannot delete the
    // instrumented binaries that the running suite still needs.
    let scratch = repo_root().join(format!(
        "target/onepipeline-coverage-default-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).expect("create a clean recipe root");
    fs::copy(repo_root().join("justfile"), scratch.join("justfile"))
        .expect("copy the real recipe to the isolated root");
    fs::copy(repo_root().join("Cargo.toml"), scratch.join("Cargo.toml"))
        .expect("copy the manifest read while parsing the justfile");
    let tree = scratch.join("target/llvm-cov-target");
    let deps = tree.join("debug/deps");
    fs::create_dir_all(&deps).expect("create an old instrumented build");
    let stale = deps.join("onepipeline_left_by_an_earlier_run-0123456789abcdef");
    fs::copy(env::current_exe().expect("test executable"), &stale)
        .expect("plant the stale test binary");

    let cleaned = Command::new("just")
        .arg("--justfile")
        .arg(scratch.join("justfile"))
        .arg("--working-directory")
        .arg(&scratch)
        .arg("_crate-coverage-clean")
        .output()
        .expect("run the clean recipe with its default argument");
    assert!(cleaned.status.success(), "{}", said(&cleaned));
    assert!(
        !stale.exists(),
        "the stale binary survived: {}",
        said(&cleaned)
    );
    assert!(
        !tree.exists(),
        "the instrumented tree survived: {}",
        said(&cleaned)
    );
    fs::remove_dir_all(&scratch).expect("remove the isolated recipe root");
}

#[cfg(unix)]
#[test]
fn the_clean_step_removes_a_tree_reached_through_an_inbound_symlink() {
    use std::os::unix::fs::symlink;

    let build = repo_root().join("target");
    let reached = build.join(format!("coverage-clean-inbound-{}", std::process::id()));
    let link = build.join(format!(
        "coverage-clean-inbound-link-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&reached);
    let _ = fs::remove_file(&link);
    fs::create_dir_all(&reached).expect("create an in-bound instrumented tree");
    let stale = reached.join("stale-binary");
    fs::copy(env::current_exe().expect("test executable"), &stale)
        .expect("plant an instrumented binary");
    symlink(&reached, &link).expect("link to the in-bound tree");

    let cleaned = just(&["_crate-coverage-clean".as_ref(), link.as_os_str()]);
    assert!(
        cleaned.status.success(),
        "the clean step failed: {}",
        said(&cleaned)
    );
    assert!(
        !reached.exists(),
        "the reached tree survived: {}",
        said(&cleaned)
    );
    assert!(
        link.symlink_metadata().is_ok(),
        "the argument link was removed"
    );
    fs::remove_file(&link).expect("remove the dangling fixture link");
}

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

/// A missing tree is ordinary on the first instrumented run.
#[test]
fn the_clean_step_passes_over_a_name_that_reaches_nothing() {
    let never = repo_root().join("target/coverage-clean-probe-never-built");
    let _ = fs::remove_dir_all(&never);
    let cleaned = just(&["_crate-coverage-clean".as_ref(), never.as_os_str()]);
    assert!(cleaned.status.success(), "{}", said(&cleaned));

    assert!(
        !never.exists(),
        "the clean step created {}, which the instrumented build owns",
        never.display()
    );

    let parent = repo_root().join(format!(
        "target/coverage-clean-nested-{}",
        std::process::id()
    ));
    fs::create_dir_all(&parent).expect("create a nested build directory");
    let missing = parent.join("never-built");
    let cleaned = just(&["_crate-coverage-clean".as_ref(), missing.as_os_str()]);
    assert!(cleaned.status.success(), "{}", said(&cleaned));
    assert!(!missing.exists(), "the clean step created a missing tree");
    fs::remove_dir(&parent).expect("remove the nested build directory");
}

#[test]
fn the_clean_step_refuses_a_missing_name_outside_the_build_directory() {
    let mistyped = repo_root().join("coverage-clean-probe-mistyped");
    let _ = fs::remove_dir_all(&mistyped);
    for (name, diagnostic) in [
        (
            "coverage-clean-probe-mistyped/tree",
            "parent cannot be entered",
        ),
        ("coverage-clean-probe-mistyped", "parent is outside"),
    ] {
        let cleaned = just(&["_crate-coverage-clean".as_ref(), OsStr::new(name)]);
        assert!(!cleaned.status.success(), "{}", said(&cleaned));
        assert!(
            String::from_utf8_lossy(&cleaned.stderr).contains(diagnostic),
            "{}",
            said(&cleaned)
        );
    }
    assert!(!mistyped.exists(), "the refused name was created");
}

#[test]
fn the_clean_step_refuses_an_empty_target_path() {
    let cleaned = just(&["_crate-coverage-clean".as_ref(), OsStr::new("")]);
    assert!(!cleaned.status.success(), "{}", said(&cleaned));
    assert!(
        String::from_utf8_lossy(&cleaned.stderr).contains("empty target path"),
        "{}",
        said(&cleaned)
    );
}

#[cfg(unix)]
#[test]
fn the_clean_step_reports_when_it_cannot_remove_an_existing_tree() {
    use std::os::unix::fs::PermissionsExt;

    let tree = repo_root().join(format!(
        "target/coverage-clean-removal-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&tree);
    fs::create_dir_all(&tree).expect("create an instrumented tree");
    let stale = tree.join("stale-binary");
    fs::copy(env::current_exe().expect("test executable"), &stale)
        .expect("plant a binary in the tree");
    fs::set_permissions(&tree, fs::Permissions::from_mode(0o500))
        .expect("make the tree unremovable");

    let cleaned = just(&["_crate-coverage-clean".as_ref(), tree.as_os_str()]);
    let survived = stale.is_file();

    fs::set_permissions(&tree, fs::Permissions::from_mode(0o700))
        .expect("restore tree permissions");
    fs::remove_dir_all(&tree).expect("remove the fixture");
    assert!(!cleaned.status.success(), "{}", said(&cleaned));
    assert!(
        String::from_utf8_lossy(&cleaned.stderr).contains("could not remove instrumented tree"),
        "{}",
        said(&cleaned)
    );
    assert!(survived, "the attempted removal changed the fixture");
}

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

    #[cfg(unix)]
    let link = repo_root().join("target/coverage-clean-probe-link");
    #[cfg(unix)]
    let symlinked: Option<&OsStr> = {
        let _ = fs::remove_file(&link);
        fs::create_dir_all(repo_root().join("target"))
            .unwrap_or_else(|e| panic!("could not create the build directory: {e}"));
        std::os::unix::fs::symlink(&outside, &link)
            .unwrap_or_else(|e| panic!("could not link {}: {e}", link.display()));
        Some(link.as_os_str())
    };
    #[cfg(not(unix))]
    let symlinked: Option<&OsStr> = None;

    let refused: Vec<&OsStr> = [
        beside.as_os_str(),
        outside.as_os_str(),
        through_dots.as_os_str(),
        quoted.as_os_str(),
    ]
    .into_iter()
    .chain(symlinked)
    .collect();

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

/// And it refuses what is there but cannot be entered, rather than passing over
/// it as a name that reaches nothing: a file, a link whose target is gone, or a
/// directory this account may not search each sits under the build directory by
/// its spelling, but where it leads cannot be resolved, so the bound cannot be
/// asked of it and a silent pass would hide the misconfiguration.
#[test]
fn the_clean_step_refuses_what_is_there_but_cannot_be_entered() {
    let build = repo_root().join("target");
    fs::create_dir_all(&build)
        .unwrap_or_else(|e| panic!("could not create the build directory: {e}"));
    let file = build.join(format!("coverage-clean-probe-file-{}", std::process::id()));
    fs::write(&file, "not a tree")
        .unwrap_or_else(|e| panic!("could not create {}: {e}", file.display()));

    #[cfg(unix)]
    let dangling = build.join(format!(
        "coverage-clean-probe-dangling-{}",
        std::process::id()
    ));
    #[cfg(unix)]
    let broken_link: Option<&OsStr> = {
        let _ = fs::remove_file(&dangling);
        std::os::unix::fs::symlink(build.join("coverage-clean-probe-gone"), &dangling)
            .unwrap_or_else(|e| panic!("could not link {}: {e}", dangling.display()));
        Some(dangling.as_os_str())
    };
    #[cfg(not(unix))]
    let broken_link: Option<&OsStr> = None;

    // A directory with no search permission is refused the same way. It is only a
    // case where this account really cannot enter it — root can — so it is asked
    // of the shell first, the same way the recipe will ask it.
    #[cfg(unix)]
    let sealed = build.join(format!(
        "coverage-clean-probe-sealed-{}",
        std::process::id()
    ));
    #[cfg(unix)]
    let unsearchable: Option<&OsStr> = {
        use std::os::unix::fs::PermissionsExt;
        fs::create_dir_all(&sealed)
            .unwrap_or_else(|e| panic!("could not create {}: {e}", sealed.display()));
        fs::set_permissions(&sealed, fs::Permissions::from_mode(0o000))
            .unwrap_or_else(|e| panic!("could not seal {}: {e}", sealed.display()));
        let enterable = Command::new("sh")
            .args(["-c", "cd -- \"$1\"", "sh"])
            .arg(&sealed)
            .status()
            .unwrap_or_else(|e| panic!("could not run sh: {e}"))
            .success();
        (!enterable).then_some(sealed.as_os_str())
    };
    #[cfg(not(unix))]
    let unsearchable: Option<&OsStr> = None;

    let unenterable: Vec<&OsStr> = [file.as_os_str()]
        .into_iter()
        .chain(broken_link)
        .chain(unsearchable)
        .collect();
    for target in unenterable {
        let attempt = just(&["_crate-coverage-clean".as_ref(), target]);
        assert!(
            !attempt.status.success(),
            "the clean step passed over {target:?}, which is there but cannot be \
             entered, as though it named nothing: {}",
            said(&attempt)
        );
        let complaint = String::from_utf8_lossy(&attempt.stderr);
        assert!(
            complaint.contains("refusing to remove"),
            "the refusal of {target:?} does not say what it refused: {}",
            said(&attempt)
        );
    }

    assert!(
        file.is_file(),
        "{} was removed by a step that had refused to remove it",
        file.display()
    );
    fs::remove_file(&file).unwrap_or_else(|e| panic!("could not remove {}: {e}", file.display()));
    #[cfg(unix)]
    fs::remove_file(&dangling)
        .unwrap_or_else(|e| panic!("could not unlink {}: {e}", dangling.display()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&sealed, fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|e| panic!("could not unseal {}: {e}", sealed.display()));
        fs::remove_dir(&sealed)
            .unwrap_or_else(|e| panic!("could not remove {}: {e}", sealed.display()));
    }
}

/// A stand-in clone lets us test refusal of its whole build directory safely.
#[test]
fn the_clean_step_refuses_a_build_directory_whole() {
    let clone = env::temp_dir().join(format!(
        "onepipeline-coverage-clean-clone-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&clone);
    let build = clone.join("target");
    let instrumented = build.join("llvm-cov-target");
    fs::create_dir_all(instrumented.join("debug/deps"))
        .unwrap_or_else(|e| panic!("could not create {}: {e}", instrumented.display()));
    fs::copy(repo_root().join("Cargo.toml"), clone.join("Cargo.toml"))
        .unwrap_or_else(|e| panic!("could not give the stand-in clone a manifest: {e}"));

    let wrong_root = Command::new("just")
        .arg("--justfile")
        .arg(repo_root().join("justfile"))
        .arg("--working-directory")
        .arg(&clone)
        .arg("_crate-coverage-clean")
        .arg(&instrumented)
        .output()
        .expect("run the repository recipe outside its own root");
    assert!(!wrong_root.status.success(), "{}", said(&wrong_root));
    assert!(
        String::from_utf8_lossy(&wrong_root.stderr).contains("outside this clone's root"),
        "{}",
        said(&wrong_root)
    );
    assert!(
        instrumented.exists(),
        "the wrong-root call removed the tree"
    );

    fs::copy(repo_root().join("justfile"), clone.join("justfile"))
        .expect("give the stand-in clone the real recipe");

    let refused = just_from(
        &clone,
        &["_crate-coverage-clean".as_ref(), build.as_os_str()],
    );
    assert!(
        !refused.status.success(),
        "the clean step accepted {} — a whole build directory — as a tree to \
         remove: {}",
        build.display(),
        said(&refused)
    );
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("refusing to remove"),
        "the refusal does not say what it refused, so a run that set it wrong is \
         left to guess: {}",
        said(&refused)
    );
    assert!(
        build.is_dir(),
        "{} was removed by a step that had refused to remove it",
        build.display()
    );

    let cleaned = just_from(
        &clone,
        &["_crate-coverage-clean".as_ref(), instrumented.as_os_str()],
    );
    assert!(
        cleaned.status.success(),
        "{} sits one level under that same build directory and was refused too, \
         so the bound refuses everything: {}",
        instrumented.display(),
        said(&cleaned)
    );
    assert!(
        !instrumented.exists(),
        "{} survived the clean step: {}",
        instrumented.display(),
        said(&cleaned)
    );

    fs::remove_dir_all(&clone)
        .unwrap_or_else(|e| panic!("could not remove {}: {e}", clone.display()));
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
    just_from(&repo_root(), args)
}

/// The same recipe in a stand-in clone, which lets a test name its whole build
/// directory without touching the one this tier is running out of.
fn just_from(working: &Path, args: &[&OsStr]) -> Output {
    Command::new("just")
        .arg("--justfile")
        .arg(working.join("justfile"))
        .arg("--working-directory")
        .arg(working)
        .args(args)
        .current_dir(repo_root())
        .output()
        .expect("just runs this repository's recipes")
}

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
    let dir = pattern
        .parent()
        .expect("$LLVM_PROFILE_FILE has no parent directory");
    assert!(
        dir.is_dir(),
        "$LLVM_PROFILE_FILE points into {}, which is not a directory",
        dir.display()
    );
    Some(dir.to_path_buf())
}

/// `llvm-profdata`, found the way `cargo llvm-cov` finds it: an explicit override
/// first, then the llvm-tools shipped beside the active toolchain's target libdir.
fn llvm_profdata() -> Option<PathBuf> {
    if let Some(explicit) = env::var_os("LLVM_PROFDATA") {
        let explicit = PathBuf::from(explicit);
        let probe = Command::new(&explicit)
            .arg("--version")
            .output()
            .unwrap_or_else(|e| {
                panic!(
                    "$LLVM_PROFDATA is {:?}, which this account cannot run: {e}",
                    explicit
                )
            });
        assert!(
            probe.status.success(),
            "$LLVM_PROFDATA {:?} failed --version: {}",
            explicit,
            said(&probe)
        );
        return Some(explicit);
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
