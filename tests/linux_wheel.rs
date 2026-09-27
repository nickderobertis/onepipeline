//! The one Linux wheel build, `scripts/build-linux-wheel.sh`, driven as `just
//! wheel-linux` drives it.
//!
//! What is substituted is `docker`, and only it: a double stood first on PATH
//! records the invocation and answers with the exit status a scenario names.
//! The real build — an image pull, a toolchain and a release compile — is
//! minutes of network-bound work, so it is proven where it runs, by ci.yml's
//! `wheel` legs and release.yml's `build-wheels`; what these hold is everything
//! the script decides before and after handing over to the container, and that
//! the container it hands over to is the one each workflow means.
//!
//! The two workflows are read as the source of truth rather than restated:
//! every Linux target release.yml's `build-wheels` matrix builds is built here,
//! in its own manylinux image, with the maturin release.yml pins; and ci.yml's
//! `wheel` legs build exactly those targets, so a pull request cannot drift
//! back to building something the release does not.

#![cfg(target_os = "linux")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use onepipeline_testfakes::executable;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A scratch directory under the checkout's ignored `target/`, because `--out`
/// must resolve inside the checkout.
fn scratch(name: &str) -> (Scratch, PathBuf) {
    let dir = repo_root()
        .join("target")
        .join(format!("linux-wheel-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("bin")).expect("the scratch directory is created");
    (Scratch(dir.clone()), dir)
}

/// A `docker` that writes each argument on its own line to `docker.log` beside
/// it, then exits with `status`.
fn docker_double(dir: &Path, status: i32) {
    executable(
        &dir.join("bin/docker"),
        format!(
            "#!/usr/bin/env bash\nprintf '%s\\n' \"$@\" > '{}'\nexit {status}\n",
            dir.join("docker.log").display()
        ),
    );
}

fn run(dir: &Path, path: &std::ffi::OsStr, args: &[&str]) -> Output {
    let output = Command::new("just")
        .current_dir(repo_root())
        .arg("wheel-linux")
        .args(args)
        .env("PATH", path)
        .output()
        .expect("just runs");
    logged(output, dir)
}

/// Echoes a run's outcome, so a failing assertion reads beside what produced it.
fn logged(output: Output, dir: &Path) -> Output {
    eprintln!(
        "[{}] exit {:?}\nstdout:\n{}\nstderr:\n{}",
        dir.display(),
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// The double's directory ahead of the host's PATH.
fn path_with_double(dir: &Path) -> std::ffi::OsString {
    let host = std::env::var_os("PATH").expect("the host has a PATH");
    let mut dirs = vec![dir.join("bin")];
    dirs.extend(std::env::split_paths(&host));
    std::env::join_paths(dirs).expect("the PATH joins")
}

fn out_arg(dir: &Path) -> String {
    dir.join("dist")
        .strip_prefix(repo_root())
        .expect("the scratch directory is inside the checkout")
        .display()
        .to_string()
}

/// One job's block of a workflow: from `  <job>:` to the next job key at the
/// same indentation.
fn job_block(workflow: &str, job: &str) -> String {
    let text = fs::read_to_string(repo_root().join(".github/workflows").join(workflow))
        .unwrap_or_else(|error| panic!("{workflow} is readable: {error}"));
    let header = format!("  {job}:");
    let mut lines = text.lines().skip_while(|line| *line != header);
    let first = lines
        .next()
        .unwrap_or_else(|| panic!("{workflow} has a `{job}` job"));
    let body = lines.take_while(|line| {
        let inner = line.strip_prefix("  ").unwrap_or("");
        !(line.starts_with("  ") && !inner.starts_with([' ', '#']) && inner.ends_with(':'))
    });
    std::iter::once(first)
        .chain(body)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// Each `- target:` in a job's matrix, with the `os:` beside it.
fn matrix_legs(block: &str) -> Vec<(String, String)> {
    let mut legs = Vec::new();
    let mut target = None;
    for line in block.lines().map(str::trim) {
        let line = line.trim_start_matches("- ");
        if let Some(value) = line.strip_prefix("target: ") {
            if !value.contains("${{") {
                target = Some(value.to_string());
            }
        } else if let Some(os) = line.strip_prefix("os: ") {
            if let Some(target) = target.take() {
                legs.push((target, os.to_string()));
            }
        }
    }
    legs
}

fn release_linux_targets() -> Vec<String> {
    let legs = matrix_legs(&job_block("release.yml", "build-wheels"));
    assert!(
        !legs.is_empty(),
        "release.yml's build-wheels matrix has legs"
    );
    legs.into_iter()
        .filter(|(_, os)| os.starts_with("ubuntu"))
        .map(|(target, _)| target)
        .collect()
}

fn release_maturin_version() -> String {
    let text = fs::read_to_string(repo_root().join(".github/workflows/release.yml"))
        .expect("release.yml is readable");
    let pins: Vec<&str> = text
        .lines()
        .filter_map(|line| line.trim().strip_prefix("maturin-version: v"))
        .collect();
    assert!(!pins.is_empty(), "release.yml pins a maturin version");
    assert!(
        pins.iter().all(|pin| *pin == pins[0]),
        "release.yml pins one maturin version, found {pins:?}"
    );
    pins[0].to_string()
}

#[test]
fn every_linux_target_the_release_builds_is_built_in_its_own_manylinux_image() {
    let targets = release_linux_targets();
    assert_eq!(
        targets,
        ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"],
        "release.yml's Linux wheel targets"
    );
    let maturin = release_maturin_version();
    for target in &targets {
        let (_scratch, dir) = scratch(target);
        docker_double(&dir, 0);
        let out = out_arg(&dir);
        let output = run(&dir, &path_with_double(&dir), &[target, &out]);
        assert!(output.status.success(), "{target}: the build succeeds");
        let invocation = fs::read_to_string(dir.join("docker.log")).expect("docker was run");
        let arch = target.split('-').next().expect("a triple names its arch");
        let platform = if arch == "x86_64" {
            "linux/amd64"
        } else {
            "linux/arm64"
        };
        let args: Vec<&str> = invocation.lines().collect();
        for expected in [
            platform,
            &format!("quay.io/pypa/manylinux2014_{arch}")[..],
            &format!("TARGET={target}")[..],
            &format!("MATURIN_VERSION={maturin}")[..],
            "RUSTFLAGS=-D warnings",
        ] {
            assert!(
                args.contains(&expected),
                "{target}: docker got {expected}; it got {args:?}"
            );
        }
        assert!(
            invocation.contains("yum install -y -q perl-IPC-Cmd perl-Time-Piece"),
            "{target}: the container installs OpenSSL's Perl prerequisites"
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains(&format!("built the {target} wheel")),
            "{target}: success names the wheel it built"
        );
    }
}

#[test]
fn the_pull_request_check_builds_exactly_the_release_linux_targets_through_the_recipe() {
    let block = job_block("ci.yml", "wheel");
    let ci: Vec<String> = matrix_legs(&block)
        .into_iter()
        .map(|(target, _)| target)
        .collect();
    assert_eq!(
        ci,
        release_linux_targets(),
        "ci.yml's wheel legs build the release's Linux targets"
    );
    assert!(
        block.contains("run: just wheel-linux \"$TARGET\""),
        "ci.yml's wheel builds through the recipe"
    );
    assert!(
        block.contains("- check: wheel\n"),
        "one ci.yml wheel leg keeps the bare `wheel` name branch protection requires"
    );
    let release = job_block("release.yml", "build-wheels");
    assert!(
        release.contains("run: just wheel-linux \"$TARGET\"")
            && release.contains("if: runner.os == 'Linux'"),
        "release.yml's Linux legs build through the recipe"
    );
}

#[test]
fn an_undefined_target_is_refused_before_docker_is_reached() {
    let (_scratch, dir) = scratch("undefined-target");
    docker_double(&dir, 0);
    let output = run(
        &dir,
        &path_with_double(&dir),
        &["riscv64gc-unknown-linux-gnu"],
    );
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no manylinux build is defined for riscv64gc-unknown-linux-gnu"),
        "{stderr}"
    );
    assert!(
        stderr.contains("ACTION: run 'build-linux-wheel.sh --target"),
        "{stderr}"
    );
    assert!(!dir.join("docker.log").exists(), "docker was not run");
}

#[test]
fn an_out_directory_that_leaves_the_checkout_through_a_symlink_is_refused() {
    let (_scratch, dir) = scratch("escaping-out");
    docker_double(&dir, 0);
    let elsewhere =
        std::env::temp_dir().join(format!("linux-wheel-elsewhere-{}", std::process::id()));
    fs::create_dir_all(&elsewhere).expect("a directory outside the checkout");
    let _elsewhere = Scratch(elsewhere.clone());
    std::os::unix::fs::symlink(&elsewhere, dir.join("dist")).expect("the escaping symlink");
    let output = logged(
        Command::new("bash")
            .current_dir(repo_root())
            .args([
                "scripts/build-linux-wheel.sh",
                "--target",
                "x86_64-unknown-linux-gnu",
                "--out",
            ])
            .arg(out_arg(&dir))
            .env("PATH", path_with_double(&dir))
            .output()
            .expect("the script runs"),
        &dir,
    );
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("outside the repository"), "{stderr}");
    assert!(!dir.join("docker.log").exists(), "docker was not run");
}

#[test]
fn a_build_that_fails_in_the_container_exits_one_naming_the_image() {
    let (_scratch, dir) = scratch("failed-build");
    docker_double(&dir, 101);
    let output = run(&dir, &path_with_double(&dir), &["x86_64-unknown-linux-gnu"]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            "the x86_64-unknown-linux-gnu wheel did not build in quay.io/pypa/manylinux2014_x86_64"
        ),
        "{stderr}"
    );
    assert!(
        stderr.contains("ACTION: follow the ACTION above"),
        "{stderr}"
    );
}

#[test]
fn a_host_without_docker_is_told_to_install_it() {
    let (_scratch, dir) = scratch("no-docker");
    let bin = dir.join("bin");
    let host = std::env::var_os("PATH").expect("the host has a PATH");
    for tool in ["just", "bash", "dirname", "realpath", "sed", "id", "mkdir"] {
        let found = std::env::split_paths(&host)
            .map(|candidate| candidate.join(tool))
            .find(|candidate| candidate.is_file())
            .unwrap_or_else(|| panic!("{tool} is on this host's PATH"));
        std::os::unix::fs::symlink(&found, bin.join(tool)).expect("the tool is linked");
    }
    let output = run(&dir, bin.as_os_str(), &["x86_64-unknown-linux-gnu"]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("docker not found"), "{stderr}");
    assert!(
        stderr.contains("ACTION: install Docker, then re-run"),
        "{stderr}"
    );
}

#[test]
fn an_absolute_out_directory_is_refused() {
    let (_scratch, dir) = scratch("absolute-out");
    docker_double(&dir, 0);
    let absolute = dir.join("dist").display().to_string();
    let output = run(
        &dir,
        &path_with_double(&dir),
        &["x86_64-unknown-linux-gnu", &absolute],
    );
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("is absolute"), "{stderr}");
    assert!(!dir.join("docker.log").exists(), "docker was not run");
}

/// ci.yml's `wheel` legs run only when `changes` reports the crate affected,
/// which is `nx.json`'s `crateSource`; a wheel-build input missing from it is a
/// pull request that changes the build and never runs it.
#[test]
fn a_change_to_what_the_wheel_build_reads_selects_the_wheel_check() {
    let block = job_block("ci.yml", "wheel");
    assert!(
        block.contains("if: needs.changes.outputs.crate == 'true'"),
        "ci.yml's wheel legs are gated on the crate being affected"
    );
    let nx: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(repo_root().join("nx.json")).expect("nx.json is readable"),
    )
    .expect("nx.json is JSON");
    let inputs = nx["namedInputs"]["crateSource"]
        .as_array()
        .expect("nx.json names crateSource");
    for read in [
        "scripts/build-linux-wheel.sh",
        "pyproject.toml",
        "rust-toolchain.toml",
        "Cargo.toml",
        "Cargo.lock",
        "src/**/*",
    ] {
        let input = format!("{{workspaceRoot}}/{read}");
        assert!(
            inputs
                .iter()
                .any(|entry| entry.as_str() == Some(input.as_str())),
            "crateSource carries {input}"
        );
    }
}
