//! `scripts/record-channel-answers.sh`, run as a person runs it.
//!
//! The script's product is the answers `tests/e2e/recorded_channel.rs` holds this
//! build to, captured with a published release. What it runs is `uv`, which
//! fetches that release, the release's own `onepipeline`, and `just`, which runs
//! the capturing journeys. Those three stand first on `PATH` here as programs that
//! record what they were asked, so each refusal and the capture itself are proven
//! through the real script without the network or a second run of the suite.

#[cfg(unix)]
mod unix {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::{Command, Output};

    const SCRIPT: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/scripts/record-channel-answers.sh"
    );

    /// The exit the script refuses with.
    const REFUSED: i32 = 2;

    /// A directory of its own, holding the three programs and what each was asked.
    struct Scratch(PathBuf);

    impl Scratch {
        /// The three programs, with `onepipeline` reporting `reports`.
        fn new(name: &str, reports: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "onepipeline-record-channel-answers-{name}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("bin")).expect("the scratch bin directory is made");
            let scratch = Self(root);
            scratch.executable(
                "uv",
                "printf '%s\\n' \"$*\" >> \"$SCRATCH/uv.asked\"\n\
                 [ ! -f \"$SCRATCH/uv.fails\" ] || exit 1\n\
                 printf '%s\\n' \"$SCRATCH/bin/onepipeline\"\n",
            );
            scratch.executable("onepipeline", &format!("printf '%s\\n' '{reports}'\n"));
            scratch.executable(
                "just",
                "printf 'args=%s with=%s\\n' \"$*\" \"$ONEPIPELINE_RECORD_CHANNEL_ANSWERS_WITH\" \
                 >> \"$SCRATCH/just.asked\"\n",
            );
            scratch
        }

        fn executable(&self, name: &str, body: &str) {
            let path = self.0.join("bin").join(name);
            fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}"))
                .expect("the program is written");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                .expect("the program is runnable");
        }

        fn run(&self, args: &[&str]) -> Output {
            let path = format!(
                "{}:{}",
                self.0.join("bin").display(),
                std::env::var("PATH").unwrap_or_default()
            );
            Command::new("bash")
                .arg(SCRIPT)
                .args(args)
                .env("PATH", path)
                .env("SCRATCH", &self.0)
                .env_remove("ONEPIPELINE_RECORD_CHANNEL_ANSWERS_WITH")
                .output()
                .expect("the script runs")
        }

        /// Everything `tool` was asked, one invocation per line.
        fn asked(&self, tool: &str) -> String {
            fs::read_to_string(self.0.join(format!("{tool}.asked"))).unwrap_or_default()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn streams(output: &Output) -> (String, String) {
        (
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }

    /// A release that is not a version is refused naming it, before anything is
    /// fetched or captured.
    #[test]
    fn a_release_that_is_not_a_version_is_refused_before_anything_is_fetched() {
        let scratch = Scratch::new("not-a-version", "onepipeline 0.28.2");
        let output = scratch.run(&["latest"]);
        let (stdout, stderr) = streams(&output);
        assert_eq!(output.status.code(), Some(REFUSED), "{stdout}\n{stderr}");
        assert!(
            stderr.contains("RELEASE 'latest' is not a MAJOR.MINOR.PATCH version"),
            "{stderr}"
        );
        assert_eq!(
            scratch.asked("uv"),
            "",
            "a release that is no version was fetched"
        );
        assert_eq!(
            scratch.asked("just"),
            "",
            "a release that is no version was captured"
        );
    }

    /// A release `uv` cannot install is refused naming it, and nothing is captured.
    #[test]
    fn a_release_uv_cannot_install_is_refused_and_nothing_is_captured() {
        let scratch = Scratch::new("uv-fails", "onepipeline 0.28.2");
        fs::write(scratch.0.join("uv.fails"), "").expect("the failure is scripted");
        let output = scratch.run(&[]);
        let (stdout, stderr) = streams(&output);
        assert_eq!(output.status.code(), Some(REFUSED), "{stdout}\n{stderr}");
        assert!(
            stderr.contains("onepipeline-cli 0.28.2 could not be installed through uv"),
            "{stderr}"
        );
        assert!(
            scratch
                .asked("uv")
                .contains("tool run --from onepipeline-cli==0.28.2"),
            "uv was not asked for the default release: {}",
            scratch.asked("uv")
        );
        assert_eq!(
            scratch.asked("just"),
            "",
            "a release uv could not install was captured"
        );
    }

    /// An executable reporting another release than the one asked for is refused
    /// naming both, and nothing is captured.
    #[test]
    fn an_executable_reporting_another_release_is_refused_and_nothing_is_captured() {
        let scratch = Scratch::new("mismatch", "onepipeline 0.27.0");
        let output = scratch.run(&[]);
        let (stdout, stderr) = streams(&output);
        assert_eq!(output.status.code(), Some(REFUSED), "{stdout}\n{stderr}");
        assert!(
            stderr.contains("reports 'onepipeline 0.27.0', not onepipeline 0.28.2"),
            "{stderr}"
        );
        assert_eq!(
            scratch.asked("just"),
            "",
            "another release's answers were captured"
        );
    }

    /// The release named is fetched, and its executable captures through the
    /// recorded-channel journeys and no others.
    #[test]
    fn the_named_release_captures_through_the_recorded_channel_journeys_alone() {
        let scratch = Scratch::new("captures", "onepipeline 0.29.0");
        let output = scratch.run(&["0.29.0"]);
        let (stdout, stderr) = streams(&output);
        assert_eq!(output.status.code(), Some(0), "{stdout}\n{stderr}");
        assert!(
            scratch
                .asked("uv")
                .contains("--from onepipeline-cli==0.29.0"),
            "uv was not asked for the named release: {}",
            scratch.asked("uv")
        );
        assert_eq!(
            scratch.asked("just"),
            format!(
                "args=test-e2e test(/^recorded_channel::/) with={}\n",
                scratch.0.join("bin").join("onepipeline").display()
            ),
            "the capture ran other journeys, or with another executable"
        );
        assert!(
            stdout.contains("captured onepipeline 0.29.0's answers under tests/recorded/answers/"),
            "{stdout}"
        );
    }
}
