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
//!
//! The third journey is over what the capture recipes *produce* rather than how
//! they run: the README's images. `just screenshots` and `just screenshots-gif`
//! both end by writing into `screenshots/images/`, and what makes either run
//! worth making is that the README then resolves to what it wrote. That needs
//! neither `freeze` nor `screencomp` nor Pillow — it reads the committed tree —
//! so, like the two above, it is not excused from this tier.

#[cfg(unix)] // The capture is `capture.sh` over `capture.py`; it runs where they do.
mod unix {
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output, Stdio};

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

    /// Every image `README.md` points at, in the order the README names them.
    ///
    /// Markdown's inline image is `![alt](path)`, and every image in this README
    /// is that form; a reference-style one would be a link this returns nothing
    /// for, which the "first image" assertion below would then fail on rather
    /// than pass over.
    fn readme_images(readme: &str) -> Vec<(usize, String, String)> {
        let mut found = Vec::new();
        let mut rest = readme;
        let mut consumed = 0usize;
        while let Some(open) = rest.find("![") {
            let after_open = open + 2;
            let Some(alt_end) = rest[after_open..].find("](") else {
                break;
            };
            let alt = &rest[after_open..after_open + alt_end];
            let path_start = after_open + alt_end + 2;
            let Some(path_end) = rest[path_start..].find(')') else {
                break;
            };
            let path = &rest[path_start..path_start + path_end];
            found.push((consumed + open, alt.to_string(), path.to_string()));
            consumed += path_start + path_end;
            rest = &rest[path_start + path_end..];
        }
        found
    }

    /// The README's pictures are the whole point of the capture, and nothing
    /// else checks that the files it writes are the files the README reads.
    ///
    /// Three things are asserted, because three different edits break them
    /// separately: that every referenced image is committed here (a capture
    /// whose output was never added leaves a README full of broken images on
    /// the registries, which render it from the published tarball); that the
    /// hero is the animation rather than a still (`just screenshots-gif` is the
    /// only recipe that writes it, and a still would silently satisfy every
    /// other check); and that the hero is *animated*, which is the one property
    /// of that file no hash gate covers — the GIF is deliberately outside the
    /// baseline, so a one-frame render is otherwise nobody's failure.
    #[test]
    fn the_readme_reads_the_images_the_capture_recipes_write() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let readme = fs::read_to_string(root.join("README.md")).expect("the README is readable");
        let images = readme_images(&readme);

        assert!(
            !images.is_empty(),
            "the README references no images at all, so either the capture's output was \
             dropped from it or this journey no longer knows how it names one"
        );

        // One `ls-files` for the whole README rather than one per image: what is
        // asked is membership, and `--error-unmatch` answers it by exiting
        // non-zero, which `git` above turns into a panic of its own wording.
        let listed = git(&["ls-files"], root);
        let tracked: Vec<String> = String::from_utf8_lossy(&listed.stdout)
            .lines()
            .map(str::to_string)
            .collect();

        for (_, alt, path) in &images {
            let on_disk = root.join(path);
            assert!(
                on_disk.is_file(),
                "the README shows `{path}`, which no file in this tree resolves to — a \
                 capture wrote it and it was never committed, so the rendered README is \
                 broken wherever it is read from the tree"
            );
            assert!(
                tracked.iter().any(|t| t == path),
                "`{path}` exists here but git does not track it, so it is absent from the \
                 published tarball the registries render the README from"
            );
            assert!(
                !alt.trim().is_empty(),
                "the README's image `{path}` carries no alt text, so a reader who cannot \
                 see it is told nothing about what is in the picture"
            );
        }

        let (at, _, first) = &images[0];
        assert_eq!(
            first, "screenshots/images/demo.gif",
            "the README's first image is `{first}` rather than the animated hero; the \
             recording of an attached run driving a plan to settlement is what a reader \
             deciding whether to install this sees before any prose"
        );
        let title_end = readme.find('\n').expect("the README has a title line");
        let between = readme[title_end..*at].trim();
        assert!(
            between.is_empty(),
            "prose has been inserted between the README's title and its hero image, which \
             now reads: {between:?}"
        );

        // The hero, structurally. Pillow writes a Graphic Control Extension
        // ahead of each frame of an animation and a `NETSCAPE2.0` application
        // extension to say it loops; a single still saved to the same path has
        // neither, and `GIF89a` is the only header that carries either.
        let hero = fs::read(root.join(first)).expect("the README's hero image is readable");
        assert!(
            hero.starts_with(b"GIF89a"),
            "the README's hero is not a GIF89a file, so whatever wrote it was not \
             `just screenshots-gif`"
        );
        let frames = hero.windows(3).filter(|w| *w == [0x21, 0xF9, 0x04]).count();
        assert!(
            frames > 1,
            "the README's hero carries {frames} frame control block(s), so it is a still \
             rather than a recording of a run"
        );
        assert!(
            hero.windows(11).any(|w| w == b"NETSCAPE2.0"),
            "the README's hero carries no loop extension, so it plays once and stops \
             wherever the reader's renderer leaves it"
        );
    }

    /// Run the committed pre-push hook the way git runs it — the pushed refs on
    /// its stdin, the remote and its URL as arguments — with `screencomp` off
    /// `PATH`, which is the branch where `SCREENCOMP_GUARD_REQUIRE` decides the
    /// answer. The environment is cleared rather than inherited so that a `CI`
    /// or a `SCREENCOMP_GUARD_REQUIRE` in the runner's own environment cannot
    /// decide what this journey observes.
    fn pre_push(require: Option<&str>) -> Output {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut hook = Command::new("bash");
        hook.arg(".githooks/pre-push")
            .arg("origin")
            .arg("https://example.invalid/onepipeline.git")
            .current_dir(root)
            .env_clear()
            // `/usr/bin:/bin` carries the coreutils the hook runs and not the
            // `screencomp` this host installs under its home.
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(value) = require {
            hook.env("SCREENCOMP_GUARD_REQUIRE", value);
        }
        let mut running = hook.spawn().expect("the pre-push hook runs");
        let zero = "0".repeat(40);
        running
            .stdin
            .as_mut()
            .expect("the hook's stdin is a pipe")
            .write_all(format!("refs/heads/topic {zero} refs/heads/topic {zero}\n").as_bytes())
            .expect("git's ref line is written to the hook");
        running
            .wait_with_output()
            .expect("the pre-push hook is waited on")
    }

    /// `SCREENCOMP_GUARD_REQUIRE` says which way the guard answers when it
    /// cannot evaluate a push, so reading it as present-means-on would turn the
    /// `=0` of someone asking for the lenient answer into the strict one.
    #[test]
    fn the_guard_holds_its_strictness_switch_to_a_value_it_can_have() {
        let lenient = pre_push(Some("0"));
        assert!(
            lenient.status.success(),
            "`SCREENCOMP_GUARD_REQUIRE=0` blocked a push the guard could not evaluate, \
             which is the opposite of what it asks for:\n{}",
            String::from_utf8_lossy(&lenient.stderr)
        );

        let strict = pre_push(Some("1"));
        assert_eq!(
            strict.status.code(),
            Some(1),
            "`SCREENCOMP_GUARD_REQUIRE=1` let a push through that the guard could not \
             evaluate:\n{}",
            String::from_utf8_lossy(&strict.stderr)
        );

        let typo = pre_push(Some("ture"));
        let said = String::from_utf8_lossy(&typo.stderr).into_owned();
        assert_eq!(
            typo.status.code(),
            Some(1),
            "a misspelt `SCREENCOMP_GUARD_REQUIRE` was read as one of the two answers \
             rather than refused, so the push went whichever way the typo happened to \
             fall:\n{said}"
        );
        assert!(
            said.contains("SCREENCOMP_GUARD_REQUIRE is 'ture'"),
            "the guard refused the misspelt value without quoting it back, so the \
             reader cannot see what it read:\n{said}"
        );
    }

    /// `SCREENSHOTS_NO_BUILD` decides whether the scenes are rendered from the
    /// binaries in this tree, so a value it guesses at is a baseline blessed for
    /// code nobody here has.
    #[test]
    fn the_capture_holds_its_build_switch_to_a_value_it_can_have() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let refused = Command::new("bash")
            .arg("screenshots/capture.sh")
            .current_dir(root)
            .env("SCREENSHOTS_NO_BUILD", "flase")
            .output()
            .expect("the capture entry point runs");
        let said = String::from_utf8_lossy(&refused.stderr).into_owned();
        assert_eq!(
            refused.status.code(),
            Some(2),
            "a misspelt `SCREENSHOTS_NO_BUILD` was read as 'skip the build', so the \
             capture would have photographed whatever binary was lying in target/:\n{said}"
        );
        assert!(
            said.contains("SCREENSHOTS_NO_BUILD is 'flase'"),
            "the capture refused the misspelt value without quoting it back:\n{said}"
        );
        assert!(
            !said.contains("Compiling") && !said.contains("Finished"),
            "the capture started building before it had decided whether to, so the \
             refusal costs a release build:\n{said}"
        );
    }
}
