//! Provisioning journeys driven through the same `just` recipe CI uses.

#[cfg(unix)]
mod unix {
    use std::fs;
    #[cfg(target_os = "linux")]
    use std::path::Path;
    use std::path::PathBuf;
    use std::process::Command;

    #[cfg(target_os = "linux")]
    use onepipeline_testfakes::executable;

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).expect("the provisioning scratch directory is removed");
        }
    }

    /// A PATH holding only the named host tools, so a recipe about what is
    /// *absent* from a host can be driven on one where it is present.
    #[cfg(target_os = "linux")]
    fn tools_only(bin: &Path, tools: &[&str]) {
        let host_path = std::env::var_os("PATH").expect("the host has a PATH");
        for tool in tools {
            let found = std::env::split_paths(&host_path)
                .map(|dir| dir.join(tool))
                .find(|candidate| candidate.is_file())
                .unwrap_or_else(|| panic!("{tool} is not on this host's PATH"));
            std::os::unix::fs::symlink(&found, bin.join(tool))
                .unwrap_or_else(|error| panic!("{tool} is linked into the test PATH: {error}"));
        }
    }

    /// The tracer the Linux-only e2e journeys need is installed where the host
    /// is this repository's to provision — CI, through a recording `sudo` and
    /// `apt-get` stood ahead of the host's — and named with its install command
    /// where it is not, by the provisioning recipe, the preflight, and every
    /// tier recipe that holds those journeys; a host that has it is left alone.
    ///
    /// Driven through the crate's whole bootstrap as well as the one recipe,
    /// because the bootstrap is where a clean clone meets this: its toolchain
    /// and installer are doubles that answer as installed ones do, so what the
    /// bootstrap reaches on a host with no tracer is the tracer's own refusal or
    /// installation, and nothing else it provisions is touched on this host.
    #[test]
    #[cfg(target_os = "linux")]
    fn strace_is_provisioned_in_ci_and_named_with_its_install_command_elsewhere() {
        let root = std::env::temp_dir().join(format!(
            "onepipeline-provisioning-strace-{}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("a fresh provisioning scratch directory");
        let _scratch = Scratch(root.clone());
        let bin = root.join("bin");
        fs::create_dir(&bin).expect("the test PATH has a bin directory");
        // `chmod`, `mkdir` and `cat` are the recording package manager's and
        // installer's own needs, `sed` the justfile's, and `cp` the bootstrap's
        // — none is a recipe under test.
        tools_only(
            &bin,
            &[
                "bash", "cat", "chmod", "cp", "just", "mkdir", "sed", "uname",
            ],
        );
        let asked = root.join("apt-get.asked");
        let refusing = root.join("apt-get.refuses");
        executable(
            &bin.join("sudo"),
            "#!/bin/sh\nset -eu\n[ \"$1\" = -n ] || { echo \"sudo was not asked non-interactively: $*\" >&2; exit 1; }\nshift\nexec \"$@\"\n",
        );
        // Records every request; refuses them all while `apt-get.refuses` stands,
        // as a package manager with no route to its mirror does.
        executable(
            &bin.join("apt-get"),
            format!(
                "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$*\" >> {asked}\n[ ! -e {refusing} ] || {{ echo 'E: Unable to locate package strace' >&2; exit 100; }}\ncase \"$*\" in\n  *install*strace*) printf '#!/bin/sh\\nexit 0\\n' > {bin}/strace; chmod 0755 {bin}/strace ;;\nesac\n",
                asked = asked.display(),
                refusing = refusing.display(),
                bin = bin.display(),
            ),
        );
        // The rest of what `_crate-bootstrap` provisions, already provisioned: a
        // toolchain with every component and both dev tools on PATH. The cargo
        // double fetches nothing and installs nothing — the store is linked, so a
        // bootstrap that asked for a binary install is one it refuses.
        executable(&bin.join("rustup"), "#!/bin/sh\nexit 0\n");
        for tool in ["cargo-nextest", "cargo-llvm-cov"] {
            executable(&bin.join(tool), "#!/bin/sh\nexit 0\n");
        }
        let cargo_home = root.join("cargo-home");
        executable(
            &bin.join("cargo"),
            r#"#!/bin/sh
set -eu
case "$1" in
  fetch) ;;
  *) echo "the bootstrap asked cargo for something its double does not answer: $*" >&2; exit 1 ;;
esac
"#,
        );
        let path = std::env::join_paths([&bin]).expect("the test PATH joins");
        let recipe = |name: &str, ci: Option<&str>| {
            let mut command = Command::new("just");
            command
                .arg(name)
                .current_dir(env!("CARGO_MANIFEST_DIR"))
                .env_clear()
                .env("PATH", &path)
                .env("HOME", &root)
                .env("CARGO_HOME", &cargo_home);
            match ci {
                Some(value) => command.env("CI", value),
                None => command.env_remove("CI"),
            };
            command.output().expect("the recipe runs")
        };
        let refused_naming_the_command = |name: &str, output: &std::process::Output| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                !output.status.success(),
                "`just {name}` passed on a host with no strace:\n{stderr}"
            );
            assert!(
                stderr.contains("strace not installed") && stderr.contains("apt-get install"),
                "`just {name}` refused without naming the tool and its install command:\n{stderr}"
            );
        };

        for name in [
            "_strace-preflight",
            "_ensure-strace",
            "_crate-bootstrap",
            "_e2e-test",
            "_e2e-test-quick",
            "test-e2e",
        ] {
            refused_naming_the_command(name, &recipe(name, None));
        }
        assert!(
            !asked.exists(),
            "the package manager was asked outside CI: {}",
            fs::read_to_string(&asked).unwrap_or_default()
        );
        assert!(
            !bin.join("strace").exists(),
            "strace was installed outside CI"
        );

        // A package manager that cannot install it fails the provisioning, and
        // the failure names the tool rather than leaving only the manager's words.
        fs::write(&refusing, "").expect("the package manager is set to refuse");
        for name in ["_ensure-strace", "_crate-bootstrap"] {
            let refused = recipe(name, Some("true"));
            let stderr = String::from_utf8_lossy(&refused.stderr);
            assert!(
                !refused.status.success(),
                "`just {name}` passed in CI though the package manager refused:\n{stderr}"
            );
            assert!(
                stderr.contains("strace could not be installed"),
                "`just {name}` failed without naming the tool it could not install:\n{stderr}"
            );
        }
        assert!(
            asked.is_file(),
            "the refusal came from somewhere other than the package manager"
        );
        assert!(
            !bin.join("strace").exists(),
            "strace was installed by a package manager that refused"
        );
        fs::remove_file(&refusing).expect("the package manager is set to answer");
        fs::remove_file(&asked).expect("the refused requests are cleared");

        // The whole bootstrap, in CI, reaches the package manager and comes back
        // with the tracer runnable.
        let provisioned = recipe("_crate-bootstrap", Some("true"));
        assert!(
            provisioned.status.success(),
            "the bootstrap failed in CI:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&provisioned.stdout),
            String::from_utf8_lossy(&provisioned.stderr)
        );
        let requests = fs::read_to_string(&asked).expect("the package manager was asked");
        assert!(
            requests
                .lines()
                .any(|line| line.contains("install") && line.contains("strace")),
            "the package manager was not asked for strace: {requests}"
        );
        assert!(
            Command::new("strace")
                .env_clear()
                .env("PATH", &path)
                .status()
                .expect("the provisioned tracer resolves")
                .success(),
            "the provisioned tracer does not run"
        );

        let before = requests.lines().count();
        for name in ["_strace-preflight", "_ensure-strace", "_crate-bootstrap"] {
            let output = recipe(name, Some("true"));
            assert!(
                output.status.success(),
                "`just {name}` refused a host that has strace:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let output = recipe(name, None);
            assert!(
                output.status.success(),
                "`just {name}` refused a host that has strace, outside CI:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert_eq!(
            fs::read_to_string(&asked)
                .expect("the record stands")
                .lines()
                .count(),
            before,
            "the package manager was asked for a tracer the host already had"
        );
    }

    /// The template journeys' preflight refuses where the released `onetaskgraph` is not
    /// installed at the root the recipes install it under, naming its version and how to
    /// install it, and passes once it is there.
    #[test]
    fn the_onetaskgraph_preflight_names_the_missing_release_and_passes_once_installed() {
        let root = std::env::temp_dir().join(format!(
            "onepipeline-provisioning-onetaskgraph-{}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("a fresh provisioning scratch directory");
        let _scratch = Scratch(root.clone());
        let preflight = || {
            Command::new("just")
                .arg("--set")
                .arg("onetaskgraph-root")
                .arg(&root)
                .arg("_onetaskgraph-preflight")
                .current_dir(env!("CARGO_MANIFEST_DIR"))
                .output()
                .expect("the recipe runs")
        };

        let refused = preflight();
        let stderr = String::from_utf8_lossy(&refused.stderr);
        assert!(
            !refused.status.success(),
            "the preflight passed with no onetaskgraph installed:\n{stderr}"
        );
        assert!(
            stderr.contains("is not installed") && stderr.contains("just bootstrap"),
            "the preflight refused without naming the release and its install:\n{stderr}"
        );

        fs::create_dir(root.join("bin")).expect("the install root has a bin directory");
        onepipeline_testfakes::executable(&root.join("bin/onetaskgraph"), "#!/bin/sh\nexit 0\n");
        let passed = preflight();
        assert!(
            passed.status.success(),
            "the preflight refused an installed onetaskgraph:\n{}",
            String::from_utf8_lossy(&passed.stderr)
        );
    }

    /// The session hook reports a missing tracer beside the rest of the
    /// toolchain, naming the install command, and installs nothing itself.
    #[test]
    #[cfg(target_os = "linux")]
    fn session_setup_reports_a_missing_strace_with_its_install_command() {
        let root = std::env::temp_dir().join(format!(
            "onepipeline-provisioning-session-{}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("a fresh provisioning scratch directory");
        let _scratch = Scratch(root.clone());
        let bin = root.join("bin");
        fs::create_dir(&bin).expect("the test PATH has a bin directory");
        // `dirname` is the hook's own need, to find the llmlint installer beside it.
        tools_only(&bin, &["bash", "dirname", "just", "uname"]);
        let path = std::env::join_paths([&bin]).expect("the test PATH joins");
        // `HOME` is the scratch, so nothing the hook would persist reaches this
        // host's; `CI` is unset, because the hook no-ops there.
        let reported = |what: &str| -> String {
            let output = Command::new("bash")
                .arg("scripts/session-setup.sh")
                .current_dir(env!("CARGO_MANIFEST_DIR"))
                .env_clear()
                .env("PATH", &path)
                .env("HOME", &root)
                .output()
                .expect("the session hook runs");
            assert!(
                output.status.success(),
                "the session hook exited non-zero {what}, which would abort a session:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stderr).into_owned()
        };

        let without = reported("without strace");
        assert!(
            without.contains("strace not on PATH") && without.contains("apt-get install"),
            "the hook did not name the missing tracer and its install command:\n{without}"
        );
        assert!(
            !bin.join("strace").exists(),
            "the session hook installed strace"
        );

        executable(&bin.join("strace"), "#!/bin/sh\nexit 0\n");
        let with = reported("with strace");
        assert!(
            !with.contains("strace"),
            "the hook reported a tracer the host has:\n{with}"
        );
    }

    /// On a GitHub Actions runner the llmlint installer puts its install
    /// directory on `GITHUB_PATH`, because the cached `lint-llm-diff` target and
    /// its journeys run `llmlint` from PATH in later steps; off a runner it writes
    /// nothing, and a `GITHUB_PATH` it cannot append to is logged, never fatal.
    #[test]
    #[cfg(target_os = "linux")]
    fn setup_llmlint_puts_its_install_directory_on_a_runner_path() {
        let root = std::env::temp_dir().join(format!(
            "onepipeline-provisioning-llmlint-{}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("a fresh provisioning scratch directory");
        let _scratch = Scratch(root.clone());
        let bin = root.join("bin");
        fs::create_dir(&bin).expect("the test PATH has a bin directory");
        // No `uv`, so nothing is installed: the subject is what the recipe
        // persists, not a registry download.
        // `sed` is the justfile's own need, to read its MSRV when it loads.
        tools_only(&bin, &["bash", "just", "sed"]);
        let path = std::env::join_paths([&bin]).expect("the test PATH joins");
        let setup = |github_path: Option<&Path>| {
            let mut command = Command::new("just");
            command
                .arg("setup-llmlint")
                .current_dir(env!("CARGO_MANIFEST_DIR"))
                .env_clear()
                .env("PATH", &path)
                .env("HOME", &root);
            if let Some(file) = github_path {
                command.env("GITHUB_PATH", file);
            }
            let output = command.output().expect("just setup-llmlint runs");
            assert!(
                output.status.success(),
                "setup-llmlint exited non-zero:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stderr).into_owned()
        };

        let runner_path = root.join("github-path");
        setup(Some(&runner_path));
        assert_eq!(
            fs::read_to_string(&runner_path).expect("setup-llmlint wrote GITHUB_PATH"),
            format!("{}\n", root.join(".local").join("bin").display()),
        );

        fs::remove_file(&runner_path).expect("the runner path file is removed");
        setup(None);
        assert!(
            !runner_path.exists(),
            "setup-llmlint wrote a runner path off a runner"
        );

        let unwritable = root.join("github-path-is-a-directory");
        fs::create_dir(&unwritable).expect("an unwritable GITHUB_PATH");
        let said = setup(Some(&unwritable));
        assert!(
            said.contains("could not append to GITHUB_PATH"),
            "setup-llmlint did not report the GITHUB_PATH it could not append to:\n{said}"
        );
    }

    /// Old clones retain hooksPath, but ordinary pushes must run no visual work.
    #[test]
    fn old_hook_configuration_allows_pushes_without_visual_work() {
        let root = std::env::temp_dir().join(format!(
            "onepipeline-push-{}-{}",
            std::process::id(),
            line!()
        ));
        fs::create_dir_all(&root).unwrap();
        let _scratch = Scratch(root.clone());
        let clone = root.join("clone");
        let remote = root.join("remote.git");
        // Git's automatic GC otherwise detaches: waiting for `git push` alone
        // does not join the writer that can race Scratch's recursive removal.
        // The receiving repository is configured below because the transport
        // can clear command-scoped configuration before starting receive-pack.
        let git_command = || {
            let mut command = Command::new("git");
            command.args(["-c", "gc.autoDetach=false"]);
            command
        };
        let initialized = git_command()
            .args(["init", "--quiet", "--bare"])
            .arg(&remote)
            .output()
            .unwrap();
        assert!(initialized.status.success());
        let configured = git_command()
            .arg("--git-dir")
            .arg(&remote)
            .args(["config", "gc.autoDetach", "false"])
            .output()
            .unwrap();
        assert!(configured.status.success());
        let cloned = git_command()
            .arg("clone")
            .arg(&remote)
            .arg(&clone)
            .output()
            .unwrap();
        assert!(cloned.status.success());
        // Copy the finished working tree, including hook files if restored, rather
        // than HEAD: this test must also prove an uncommitted hook deletion.
        let exported = git_command()
            .args(["ls-files", "-z"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .unwrap();
        assert!(exported.status.success());
        for name in exported
            .stdout
            .split(|byte| *byte == 0)
            .filter(|name| !name.is_empty())
        {
            let name = std::str::from_utf8(name).unwrap();
            let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(name);
            if source.is_file() {
                let destination = clone.join(name);
                fs::create_dir_all(destination.parent().unwrap()).unwrap();
                fs::copy(source, destination).unwrap();
            }
        }
        let git = |args: &[&str]| {
            let out = git_command()
                .args(args)
                .current_dir(&clone)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            out
        };
        git(&["config", "user.name", "Push journey"]);
        git(&["config", "user.email", "push@example.invalid"]);
        git(&["config", "core.hooksPath", ".githooks"]);
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "test: first authored commit"]);
        git(&["repack"]);
        git(&["remote", "rename", "origin", "local"]);
        let artifacts = git(&["ls-files", "-s", "shots", "screenshots/images"]).stdout;
        let push = || {
            let out = git_command()
                .args(["push", "local", "HEAD:refs/heads/journey"])
                .env("GIT_TRACE", "1")
                .current_dir(&clone)
                .output()
                .unwrap();
            let trace = String::from_utf8_lossy(&out.stderr);
            assert!(out.status.success(), "ordinary push failed: {trace}");
            for command in [
                "pre-push",
                "screencomp",
                "capture.sh",
                "just check",
                "just gate",
            ] {
                assert!(!trace.contains(command), "push executed {command}: {trace}");
            }
            let received = git_command()
                .arg("--git-dir")
                .arg(&remote)
                .args(["rev-parse", "refs/heads/journey"])
                .output()
                .unwrap();
            assert!(received.status.success());
            assert_eq!(received.stdout, git(&["rev-parse", "HEAD"]).stdout);
            assert_eq!(
                artifacts,
                git(&["ls-files", "-s", "shots", "screenshots/images"]).stdout
            );
            assert!(git(&[
                "status",
                "--porcelain",
                "--untracked-files=all",
                "--",
                "shots",
                "screenshots/images"
            ])
            .stdout
            .is_empty());
            assert!(!clone.join("shots/review").exists());
        };
        push();
        fs::write(
            clone.join("screenshots/push-journey.txt"),
            "guarded path changed\n",
        )
        .unwrap();
        git(&["add", "screenshots/push-journey.txt"]);
        git(&["commit", "--quiet", "-m", "test: change a guarded path"]);
        push();
        // Up-to-date pushes still resolve the configured hooks directory.
        push();
        // Keep both packs, then cross the real automatic-GC pack threshold.
        // This proves the command joins maintenance, rather than relying on a
        // small fixture never reaching the condition that caused the race.
        git(&["repack"]);
        let collected = git_command()
            .args([
                "-c",
                "gc.auto=1",
                "-c",
                "gc.autoPackLimit=1",
                "gc",
                "--auto",
            ])
            .env("GIT_TRACE", "1")
            .current_dir(&clone)
            .output()
            .unwrap();
        let trace = String::from_utf8_lossy(&collected.stderr);
        assert!(collected.status.success(), "automatic GC failed: {trace}");
        assert!(trace.contains("pack-objects"), "GC did no work: {trace}");
        assert!(
            !trace.contains("--detach"),
            "GC left a writer behind: {trace}"
        );
        assert!(!clone.join(".git/gc.pid").exists(), "GC has not finished");
        let planned = Command::new("just")
            .args(["--dry-run", "bootstrap"])
            .current_dir(&clone)
            .output()
            .unwrap();
        assert!(planned.status.success());
        let plan = format!(
            "{}{}",
            String::from_utf8_lossy(&planned.stdout),
            String::from_utf8_lossy(&planned.stderr)
        );
        for obsolete in ["hooksPath", "_hooks", "pre-push", "screencomp"] {
            assert!(
                !plan.contains(obsolete),
                "bootstrap restores {obsolete}: {plan}"
            );
        }
        assert_eq!(
            git(&["config", "--get", "core.hooksPath"]).stdout,
            b".githooks\n"
        );
        push();
    }
}
