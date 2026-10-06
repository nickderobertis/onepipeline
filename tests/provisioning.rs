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
        // toolchain with every component and every dev tool on PATH. The cargo
        // double fetches nothing and installs nothing — the store is linked, so a
        // bootstrap that asked for a binary install is one it refuses.
        executable(&bin.join("rustup"), "#!/bin/sh\nexit 0\n");
        for tool in ["cargo-nextest", "cargo-llvm-cov", "onebudgetspec"] {
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
            "_crate-test-rest",
            "test-quick",
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

    /// The committed visual guard is **activated** by provisioning, not merely
    /// committed.
    ///
    /// `core.hooksPath` is per-clone state that is never committed, so the one
    /// thing a fresh clone does not inherit is the setting that makes
    /// `.githooks/pre-push` run at all. Hence a real clone rather than this
    /// worktree, and hence the assertion that it starts without one: without
    /// that, this would pass over a clone that had been configured by hand.
    ///
    /// Only `_hooks` is run, not the whole bootstrap: the rest installs
    /// toolchains onto this host and none of it decides where a hook lives.
    #[test]
    fn provisioning_a_fresh_clone_activates_the_committed_visual_guard() {
        let root = std::env::temp_dir().join(format!(
            "onepipeline-hooks-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("a fresh hook-activation scratch directory");
        let _scratch = Scratch(root.clone());
        let clone = root.join("clone");

        let cloned = Command::new("git")
            .args([
                "clone",
                "--quiet",
                "--no-hardlinks",
                "--depth",
                "1",
                "--no-single-branch",
            ])
            .arg(env!("CARGO_MANIFEST_DIR"))
            .arg(&clone)
            .output()
            .expect("git clones this repository");
        assert!(
            cloned.status.success(),
            "the fresh clone could not be made:\n{}",
            String::from_utf8_lossy(&cloned.stderr)
        );

        // A clone starts with no hooks path of its own. If that ever stopped
        // being true this journey would pass without provisioning anything.
        let before = Command::new("git")
            .args(["config", "--get", "core.hooksPath"])
            .current_dir(&clone)
            .output()
            .expect("git reads the clone's config");
        assert!(
            !before.status.success(),
            "a fresh clone already carries core.hooksPath ({}), so this journey \
             would pass without the recipe having done anything",
            String::from_utf8_lossy(&before.stdout).trim()
        );

        // The committed guard, and only the guard, is what the directory holds:
        // the complete pre-push bar stays unhooked, which is a promise
        // `screenshots/AGENTS.md` makes to anyone whose push this now runs on.
        let hooks: Vec<String> = fs::read_dir(clone.join(".githooks"))
            .expect("the clone carries the committed hooks directory")
            .map(|entry| {
                entry
                    .expect("a hooks directory entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(
            hooks,
            vec!["pre-push".to_string()],
            "the committed hooks directory holds something other than the visual guard"
        );
        let guard =
            fs::read_to_string(clone.join(".githooks/pre-push")).expect("the guard is readable");
        // What it *runs*, not what it says about itself: the hook's own header
        // explains in prose that the complete bar is deliberately left out, so a
        // substring search over the whole file matches that sentence.
        let runs_the_bar = guard
            .lines()
            .map(str::trim)
            .filter(|line| !line.starts_with('#'))
            .any(|line| line.contains("just gate") || line.contains("just check"));
        assert!(
            !runs_the_bar,
            "the visual guard runs the complete pre-push bar, which this repository \
             deliberately leaves unhooked"
        );

        // `just bootstrap` is what a clean clone runs, and it activates the hook
        // through a dependency. Running the whole thing here would install
        // toolchains onto this host, so what is held is the dependency itself —
        // read off `just`'s own plan for that recipe — and then the step it
        // names is run for real below. Dropped from `bootstrap`, the recipe
        // would still work and this journey would still fail.
        let planned = Command::new("just")
            .args(["--dry-run", "bootstrap"])
            .current_dir(&clone)
            .output()
            .expect("just plans the bootstrap recipe");
        let plan = format!(
            "{}{}",
            String::from_utf8_lossy(&planned.stdout),
            String::from_utf8_lossy(&planned.stderr)
        );
        assert!(
            planned.status.success(),
            "`just --dry-run bootstrap` failed, so what a clean clone runs cannot \
             be read:\n{plan}"
        );
        assert!(
            plan.contains("core.hooksPath .githooks"),
            "`just bootstrap` no longer activates the committed guard, so a clean \
             clone would push with nothing watching the screenshots:\n{plan}"
        );

        let provisioned = Command::new("just")
            .arg("_hooks")
            .current_dir(&clone)
            .output()
            .expect("the provisioning recipe runs");
        assert!(
            provisioned.status.success(),
            "the hook-activation recipe exited non-zero:\n{}",
            String::from_utf8_lossy(&provisioned.stderr)
        );

        let after = Command::new("git")
            .args(["config", "--get", "core.hooksPath"])
            .current_dir(&clone)
            .output()
            .expect("git reads the clone's config");
        assert!(
            after.status.success(),
            "provisioning left the clone with no core.hooksPath, so the committed \
             guard would never run"
        );
        assert_eq!(
            String::from_utf8_lossy(&after.stdout).trim(),
            ".githooks",
            "provisioning pointed git at a hooks directory this repository does not commit"
        );

        // And what git *resolves* for that clone, rather than only what the
        // config says: a value naming a directory git would not run is a guard
        // that is configured and inert.
        let resolved = clone.join(String::from_utf8_lossy(&after.stdout).trim());
        assert!(
            resolved.join("pre-push").is_file(),
            "the activated hooks path has no pre-push hook in it: {}",
            resolved.display()
        );
    }
}
