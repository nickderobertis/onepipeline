//! Journeys over the visual-docs capture's own machinery.
//!
//! The capture as a whole is driven by `just screenshots` and is deliberately
//! outside this tier: every scene it renders goes through `freeze` and is
//! classified by `screencomp`, two third-party tools `just check` does not
//! install, so a journey over the whole of it could only assert against stubs
//! of those two. What is driven here needs neither. It is the capture's own
//! **world** — the throwaway repositories it seeds with `git` before any scene
//! is rendered — and the two properties of it a rejected push once turned up:
//! which repository its git commands act on, and what it says when one fails.
//!
//! Both drive the real `screenshots/world.py` as a subprocess, against real
//! `git`, exactly as `screenshots/capture.py` imports and calls it.

#[cfg(unix)] // The capture is `capture.sh` over `capture.py`; it runs where they do.
mod unix {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).expect("the visual-docs scratch directory is removed");
        }
    }

    fn scratch(what: &str) -> (Scratch, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "onepipeline-visual-docs-{what}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("a fresh visual-docs scratch directory");
        (Scratch(root.clone()), root)
    }

    fn git(args: &[&str], at: &Path) -> Output {
        let done = Command::new("git")
            .args(args)
            .current_dir(at)
            .output()
            .unwrap_or_else(|why| panic!("git {args:?} runs: {why}"));
        assert!(
            done.status.success(),
            "git {args:?} failed:\n{}\n{}",
            String::from_utf8_lossy(&done.stdout),
            String::from_utf8_lossy(&done.stderr)
        );
        done
    }

    /// Run a driver against the committed `screenshots/world.py`, with `extra`
    /// exported on top of this process's environment the way a caller's shell —
    /// or git, running a hook — would export it.
    fn drive(driver: &Path, body: &str, extra: &[(&str, &Path)]) -> Output {
        fs::write(driver, body).expect("the driver is written");
        let screenshots = Path::new(env!("CARGO_MANIFEST_DIR")).join("screenshots");
        let mut python = Command::new("python3");
        python.arg(driver).env("SCREENSHOTS_DIR", &screenshots);
        for (name, value) in extra {
            python.env(name, value);
        }
        python
            .output()
            .expect("python3 runs the driver; the capture is python and this tier drives it")
    }

    /// A step of the capture that fails reports what the command itself said,
    /// on whichever stream the command chose to say it on.
    ///
    /// This is the exact command a refused push died at — `git -C <world>/repo-1
    /// commit --quiet -m seed` — and the exact reason it said nothing: `git
    /// commit` with nothing staged exits non-zero having written its whole
    /// message to **stdout**, so a diagnostic quoting `stderr` alone arrives as
    /// the words "Fix what its own error below names" followed by a blank line.
    /// The failure is reproduced here rather than described: the seed commit is
    /// made to fail for git's own reason, and the refusal must carry git's own
    /// words.
    #[test]
    fn a_failed_world_step_reports_what_the_command_said_on_stdout() {
        let (_scratch, root) = scratch("diagnostic");
        let seed = root.join("repo-1");
        fs::create_dir(&seed).expect("the throwaway repository has a directory");
        git(&["init", "--quiet", "-b", "main"], &seed);

        let done = drive(
            &root.join("driver.py"),
            r#"
import os, sys
sys.path.insert(0, os.environ["SCREENSHOTS_DIR"])
import world

# The world's own committer identity, so the commit fails for having nothing to
# commit rather than for not knowing who is committing.
env = dict(
    os.environ,
    GIT_AUTHOR_NAME="onepipeline shots",
    GIT_AUTHOR_EMAIL="shots@onepipeline.invalid",
    GIT_COMMITTER_NAME="onepipeline shots",
    GIT_COMMITTER_EMAIL="shots@onepipeline.invalid",
)
try:
    world.run(["git", "-C", os.environ["SEED_REPO"], "commit", "--quiet", "-m", "seed"], env)
except SystemExit as refusal:
    sys.stdout.write(str(refusal))
    raise SystemExit(0)
raise SystemExit("the seed commit succeeded, so nothing was reported and this proved nothing")
"#,
            &[("SEED_REPO", &seed)],
        );
        let refusal = String::from_utf8_lossy(&done.stdout);
        assert!(
            done.status.success(),
            "the driver did not reach a refusal:\n{refusal}\n{}",
            String::from_utf8_lossy(&done.stderr)
        );
        assert!(
            refusal.contains("commit --quiet -m seed"),
            "the refusal does not name the command that failed:\n{refusal}"
        );
        assert!(
            refusal.contains("nothing to commit"),
            "the refusal does not carry git's own message, which git wrote to stdout — \
             so a capture can still fail with no diagnostic:\n{refusal}"
        );
        assert!(
            refusal.contains("stdout:"),
            "the refusal quotes a stream without naming it, so a reader cannot tell \
             which one the command spoke on:\n{refusal}"
        );
    }

    /// Nothing a caller exports decides which repository the capture's own git
    /// commands act on.
    ///
    /// Git runs every hook with `GIT_DIR` naming the repository being pushed,
    /// and `GIT_DIR` beats `-C` and beats discovery — so when the capture ran
    /// from the pre-push guard, the `git init` / `add -A` / `commit` that seed
    /// each throwaway repository acted on the repository under push and
    /// committed its whole tree away. The hazard is real, so it is demonstrated
    /// here first and then closed: the same `git` question is asked from the
    /// throwaway directory on either side of the capture's own
    /// `clear_inherited_settings`, and the answer must move from the caller's
    /// repository to the throwaway one.
    #[test]
    fn an_exported_git_dir_does_not_decide_which_repository_the_world_seeds() {
        let (_scratch, root) = scratch("git-dir");
        let caller = root.join("caller");
        let throwaway = root.join("world").join("repo-0");
        fs::create_dir_all(&caller).expect("the caller's repository has a directory");
        fs::create_dir_all(&throwaway).expect("the throwaway repository has a directory");
        git(&["init", "--quiet", "-b", "main"], &caller);
        git(&["init", "--quiet", "-b", "main"], &throwaway);
        let caller_git_dir = caller.join(".git");

        let done = drive(
            &root.join("driver.py"),
            r#"
import os, subprocess, sys
sys.path.insert(0, os.environ["SCREENSHOTS_DIR"])
import world

here = os.environ["THROWAWAY"]

def whose_repository():
    """Which repository git would act on, asked from inside the throwaway one."""
    return subprocess.run(
        ["git", "-C", here, "rev-parse", "--absolute-git-dir"],
        capture_output=True, text=True, check=True,
    ).stdout.strip()

print("before", whose_repository())
world.clear_inherited_settings()
print("after", whose_repository())
print("remaining", sorted(n for n in os.environ if n.startswith("GIT_")))
"#,
            &[("THROWAWAY", &throwaway), ("GIT_DIR", &caller_git_dir)],
        );
        let report = String::from_utf8_lossy(&done.stdout);
        assert!(
            done.status.success(),
            "the driver did not answer:\n{report}\n{}",
            String::from_utf8_lossy(&done.stderr)
        );
        let line = |key: &str| {
            report
                .lines()
                .find_map(|line| line.strip_prefix(key).map(str::trim))
                .unwrap_or_else(|| panic!("the driver reported no {key:?} line:\n{report}"))
        };

        // The hazard, demonstrated rather than asserted: with the caller's
        // `GIT_DIR` exported, a git command aimed at the throwaway repository
        // acts on the caller's. Without this the test below would pass over a
        // world that had simply never been in danger.
        assert_eq!(
            fs::canonicalize(line("before")).expect("the caller's git dir resolves"),
            fs::canonicalize(&caller_git_dir).expect("the caller's git dir resolves"),
            "an exported GIT_DIR no longer decides which repository git acts on, so \
             this journey is asserting against a hazard that no longer exists:\n{report}"
        );
        assert_eq!(
            fs::canonicalize(line("after")).expect("the throwaway git dir resolves"),
            fs::canonicalize(throwaway.join(".git")).expect("the throwaway git dir resolves"),
            "the capture's own git commands still act on the caller's repository, so a \
             capture run from a git hook would seed the repository being pushed:\n{report}"
        );
        assert_eq!(
            line("remaining"),
            "[]",
            "the capture left a GIT_* setting of the caller's standing; the world states \
             the whole of its own git environment and inherits none of it:\n{report}"
        );
    }
}
