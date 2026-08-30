//! What the published-artifact smoke *says* when an installed binary fails it.
//!
//! `scripts/smoke-published.sh` is run by CI's `install` job and by both
//! post-publish verify jobs, so its passing branch is exercised continuously
//! against a real installed binary and needs nothing here. Its refusals are read
//! by somebody looking at one red matrix leg, who has the two lines it printed
//! and no repository in front of them — and the help-surface refusal is the one
//! that can fire on a platform package that resolved to the wrong release, which
//! is the case where "reinstall it" is not by itself an instruction.
//!
//! The command list these journeys drive is parsed out of the script, the same
//! way `tests/contract.rs` parses it, so this is not a second copy of the surface
//! to keep in step with the first.

#[cfg(unix)]
mod unix {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).expect("the smoke scratch directory is removed");
        }
    }

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    fn script() -> String {
        fs::read_to_string(repo_root().join("scripts/smoke-published.sh"))
            .expect("the smoke script ships")
    }

    /// The documented surface, read off the script's own loop rather than
    /// restated here — `tests/contract.rs` is what holds that loop to the binary.
    fn documented_commands() -> Vec<String> {
        script()
            .lines()
            .find_map(|line| {
                line.trim()
                    .strip_prefix("for command in ")?
                    .strip_suffix("; do")
            })
            .expect("the smoke script iterates a `for command in ...; do` list")
            .split_whitespace()
            .map(str::to_string)
            .collect()
    }

    fn scratch(name: &str) -> (Scratch, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "onepipeline-published-smoke-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("a fresh smoke scratch directory");
        let bin = root.join("bin");
        fs::create_dir(&bin).expect("the stub installation has a bin directory");
        (Scratch(root), bin)
    }

    /// A stand-in for an installed artifact: it starts, reports `version`, and
    /// lists exactly `commands` on `--help`. Nothing beyond that, because the
    /// help check is the third thing the script does and every journey here is
    /// about what it says when it gets there.
    fn install_stub(bin: &Path, version: &str, commands: &[String]) {
        let listed = commands.join(" ");
        let contents = format!(
            "#!/bin/sh\ncase \"$1\" in\n  --version) echo 'onepipeline {version}' ;;\n  \
             --help) echo 'Commands: {listed}' ;;\n  *) exit 1 ;;\nesac\n"
        );
        let path = bin.join("onepipeline");
        fs::write(&path, contents).expect("the stub binary is written");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("the stub binary is runnable");
    }

    fn run_smoke(bin: &Path, args: &[&str]) -> Output {
        let host_path = std::env::var_os("PATH").expect("the host has a PATH");
        let mut paths = vec![bin.to_path_buf()];
        paths.extend(std::env::split_paths(&host_path));
        let path = std::env::join_paths(paths).expect("the test PATH joins");
        Command::new(repo_root().join("scripts/smoke-published.sh"))
            .args(args)
            .current_dir(repo_root())
            .env("PATH", path)
            .output()
            .expect("the published-artifact smoke runs")
    }

    /// An install that resolved to a release predating a command has to be told
    /// how to install the one under test, not only that it should.
    ///
    /// The version-mismatch refusal above it in the script already spells the two
    /// registries out; this one said "reinstall the version under test" and left
    /// the operator to guess the spelling, on the one failure whose usual cause is
    /// a platform package that resolved to the wrong release.
    #[test]
    fn a_help_surface_missing_a_command_names_a_pinned_install_for_each_registry() {
        let (_scratch, bin) = scratch("pinned");
        let mut commands = documented_commands();
        let dropped = commands.pop().expect("the script documents commands");
        install_stub(&bin, "9.9.9", &commands);

        let refused = run_smoke(
            &bin,
            &["--expect-version", "9.9.9", "--label", "pinned stub"],
        );
        let stderr = String::from_utf8_lossy(&refused.stderr);

        assert_eq!(
            refused.status.code(),
            Some(1),
            "a published binary missing a documented command is a failed smoke:\n{stderr}"
        );
        assert!(
            stderr.contains(&format!("does not list the '{dropped}' command")),
            "the refusal did not name the missing command:\n{stderr}"
        );
        let action = stderr
            .lines()
            .find_map(|line| line.strip_prefix("ACTION: "))
            .unwrap_or_else(|| panic!("the refusal carried no ACTION line:\n{stderr}"));
        for install in [
            "pip install --force-reinstall onepipeline-cli==9.9.9",
            "npm install -g onepipeline-cli@9.9.9",
        ] {
            assert!(
                action.contains(install),
                "the ACTION does not carry a runnable install for every registry this \
                 repository publishes to; it is missing {install:?}:\n{action}"
            );
        }
        assert!(
            action.contains(&format!("'{dropped}'")),
            "the ACTION's second branch — the surface moved rather than the artifact — \
             did not name the command to drop:\n{action}"
        );
    }

    /// Called without `--expect-version` there is no release to pin to, so the
    /// same guidance has to degrade to the bare install rather than to a command
    /// that would not run.
    #[test]
    fn the_same_refusal_without_an_expected_version_names_an_unpinned_install() {
        let (_scratch, bin) = scratch("unpinned");
        let mut commands = documented_commands();
        commands.pop().expect("the script documents commands");
        install_stub(&bin, "9.9.9", &commands);

        let refused = run_smoke(&bin, &["--label", "unpinned stub"]);
        let stderr = String::from_utf8_lossy(&refused.stderr);
        let action = stderr
            .lines()
            .find_map(|line| line.strip_prefix("ACTION: "))
            .unwrap_or_else(|| panic!("the refusal carried no ACTION line:\n{stderr}"));

        assert!(
            action.contains("pip install --force-reinstall onepipeline-cli'")
                && action.contains("npm install -g onepipeline-cli'"),
            "with no version under test the guidance has to name the bare install:\n{action}"
        );
        assert!(
            !action.contains("==") && !action.contains("onepipeline-cli@"),
            "the guidance pinned to a version nobody gave it:\n{action}"
        );
    }

    /// The contrast, so the journeys above are known to be reading the help
    /// check rather than any refusal: a binary that lists the whole documented
    /// surface gets past it, and fails later on something else.
    #[test]
    fn a_help_surface_carrying_every_documented_command_gets_past_the_help_check() {
        let (_scratch, bin) = scratch("complete");
        install_stub(&bin, "9.9.9", &documented_commands());

        let refused = run_smoke(
            &bin,
            &["--expect-version", "9.9.9", "--label", "complete stub"],
        );
        let stderr = String::from_utf8_lossy(&refused.stderr);

        assert!(
            !stderr.contains("does not list the"),
            "a binary listing every documented command was still refused for its help \
             surface:\n{stderr}"
        );
        assert!(
            stderr.contains("drive-run"),
            "the script did not go on to the hidden verbs, so this journey proves \
             nothing about where it stopped:\n{stderr}"
        );
    }
}
