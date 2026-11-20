//! The build configuration every cargo invocation inside this clone reads, held
//! to what `.cargo/config.toml` promises.
//!
//! That file carries three keys, and each is a promise about *every* build under
//! this clone rather than about the workspace: `build.target-dir` puts a crate
//! outside the workspace into the same `<clone>/target` the members use, so one
//! clone never compiles its dependency graph into two trees,
//! `profile.dev.debug = 0` builds every dev and test build without debuginfo, and
//! the Linux GNU targets' `linker` links every dev and test build with the
//! toolchain's own rust-lld while a release links as it would without it. None can be read off the manifest — a `Cargo.toml` profile
//! reaches only its own workspace — and none is proven by the file parsing,
//! because cargo merges config files from the working directory *upward* and
//! resolves each `target-dir` against its own file's location, which is exactly
//! where a copy of the file goes wrong.
//!
//! So these ask cargo. A probe crate is written under `target/build-config/`,
//! outside the workspace and inside the clone, and what is asserted is what cargo
//! reports for it and what it hands `rustc` when it builds it. The overrides the
//! environment may carry (`CARGO_TARGET_DIR`, `RUSTFLAGS`) are stripped from the
//! probe's environment on purpose: each outranks the file, and what is under
//! test is the file. The linker probe sets `RUSTFLAGS` back, because the file's
//! promise there is that the `-D warnings` the recipes set does not undo it.
//!
//! Nothing a probe runs may wait on a lock the rest of the clone's cargo work
//! takes. The clone's `target/debug` and `target/release` are locked for the
//! whole of every build into them — the clippy, doc and build tasks Nx runs
//! beside this suite hold them for minutes — and the cargo home's package cache
//! for every resolution. So each case runs under a cargo home of its own, a
//! probe that builds does so in a checkout of its own carrying a copy of the
//! file, and a probe that would block on a lock fails naming it rather than
//! waiting out the runner's bound.
//!
//! That is proven against a fixture rather than this clone: a clone and an
//! inherited cargo home of the test's own, whose build-directory and
//! package-cache locks it alone creates, so it holds them for the whole of the
//! probe's builds without waiting on, or holding up, any other cargo on the host.

use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// The repository root: `CARGO_MANIFEST_DIR` is the root crate's directory.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn build_config() -> String {
    let path = repo_root().join(".cargo/config.toml");
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("{} reads: {error}", path.display()))
}

/// The table the file names the Linux linker under, and the script it names,
/// relative to the clone root the way cargo resolves it.
const LINUX_TARGETS: &str = r#"cfg(all(target_os = "linux", target_env = "gnu"))"#;
const LINUX_LINKER: &str = "scripts/cc-lld-debug.sh";

/// A copy of the file at `root/.cargo/config.toml`, and of the linker script it
/// names at the path it names it by, so a build under `root` links as one in
/// the clone does.
fn copy_build_config(root: &Path) {
    fs::create_dir_all(root.join(".cargo")).expect("a checkout config directory");
    fs::write(root.join(".cargo/config.toml"), build_config()).expect("a copied config");
    let linker = root.join(LINUX_LINKER);
    fs::create_dir_all(linker.parent().expect("the linker has a directory"))
        .expect("a checkout scripts directory");
    fs::copy(repo_root().join(LINUX_LINKER), &linker).expect("a copied linker script");
}

/// The clone a case runs in, and the cargo home its environment hands cargo:
/// `None` is whatever this test process inherited.
struct Host {
    clone: PathBuf,
    inherited_home: Option<PathBuf>,
}

impl Host {
    fn this_clone() -> Self {
        Self {
            clone: repo_root(),
            inherited_home: None,
        }
    }
}

/// The directory one case writes everything it runs under, inside `host`'s
/// clone, and the cargo home its cargo runs with at `cargo-home/` inside it.
///
/// Written fresh on every call, and under `target/` for the reason
/// `linked_engines` puts its fixtures there: a failed run leaves the tree that
/// failed where the other build artifacts are. One per case, because two cases
/// sharing a cargo home would contend for its package cache.
fn case_dir(host: &Host, case: &str) -> PathBuf {
    let dir = host.clone.join(format!("target/build-config/{case}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a case directory");
    dir
}

/// A crate at `root` that is inside the clone but outside the workspace: its own
/// `[workspace]` table stops cargo from folding it into the root's, which is the
/// case a manifest-declared profile could never reach.
fn probe_crate(root: &Path) {
    fs::create_dir_all(root.join("src")).expect("a probe crate directory");
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"build-config-probe\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[workspace]\n",
    )
    .expect("a probe manifest");
    fs::write(root.join("src/lib.rs"), "pub fn probe() {}\n").expect("a probe crate");
}

/// A checkout of its own under `case`, carrying a copy of the file at its root,
/// with a probe crate outside any workspace inside it at `probe/`: the clone's
/// layout, in a tree whose build directories nothing but this case builds into.
///
/// That the copy resolves `target-dir` against its own root, as the clone's file
/// does against the clone's, is what
/// `a_second_checkout_carrying_its_own_copy_of_the_file_builds_into_its_own_root`
/// proves.
fn probe_checkout(case: &Path) -> PathBuf {
    copy_build_config(case);
    let probe = case.join("probe");
    probe_crate(&probe);
    probe
}

/// The cargo that builds the probe: `CARGO`, which cargo sets on every test it
/// runs to the binary performing the build, so the probe is built by the same
/// toolchain the tree is, as the e2e harness does.
///
/// The variable is read from the environment, so a value that names no file is
/// refused by name here rather than as a bare "No such file" from the spawn.
fn cargo_binary() -> PathBuf {
    let Some(cargo) = std::env::var_os("CARGO") else {
        return PathBuf::from("cargo");
    };
    let cargo = PathBuf::from(cargo);
    assert!(
        cargo.is_file(),
        "CARGO names {}, which is not a file",
        cargo.display()
    );
    cargo
}

/// What cargo prints, on its own line, when a lock it needs is held elsewhere.
const LOCK_WAIT: &str = "waiting for file lock";

/// Run cargo from `dir` under `host`'s environment with everything that
/// outranks the config file removed, under `own_home` where one is named, and
/// with `RUSTFLAGS` set to `rustflags` where that is named.
///
/// A cargo that reports it is waiting on a lock is ended there, and what is
/// returned is the line naming the lock.
fn cargo_run<I, S>(
    host: &Host,
    dir: &Path,
    own_home: Option<&Path>,
    rustflags: Option<&str>,
    args: I,
) -> Result<Output, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new(cargo_binary());
    command
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for home in [host.inherited_home.as_deref(), own_home]
        .into_iter()
        .flatten()
    {
        command.env("CARGO_HOME", home);
    }
    for outranking in [
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_TARGET_DIR",
        "CARGO_PROFILE_DEV_DEBUG",
        "CARGO_PROFILE_TEST_DEBUG",
        "CARGO_PROFILE_RELEASE_DEBUG",
        "RUSTFLAGS",
        "CARGO_BUILD_RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
    ] {
        command.env_remove(outranking);
    }
    if let Some(flags) = rustflags {
        command.env("RUSTFLAGS", flags);
    }
    let mut child = command.spawn().expect("cargo runs");
    let mut stdout = child.stdout.take().expect("cargo's stdout is piped");
    let stdout = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let mut stderr = Vec::new();
    let lines = BufReader::new(child.stderr.take().expect("cargo's stderr is piped"));
    for line in lines.split(b'\n') {
        let line = line.expect("cargo's stderr reads");
        let text = String::from_utf8_lossy(&line);
        if text.contains(LOCK_WAIT) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(text.trim().to_owned());
        }
        stderr.extend_from_slice(&line);
        stderr.push(b'\n');
    }
    Ok(Output {
        status: child.wait().expect("cargo exits"),
        stdout: stdout
            .join()
            .expect("cargo's stdout reader")
            .expect("cargo's stdout reads"),
        stderr,
    })
}

/// Run cargo as [`cargo_run`] does, under `case`'s own cargo home, and require
/// that it succeeds.
///
/// A lock wait fails the test naming the lock: every lock a probe takes is one
/// only its own case takes, so a wait means one escaped into what the rest of
/// the clone shares.
fn cargo<I, S>(host: &Host, dir: &Path, case: &Path, args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    cargo_with_rustflags(host, dir, case, None, args)
}

/// [`cargo`], with `RUSTFLAGS` set to `rustflags` where that is named.
fn cargo_with_rustflags<I, S>(
    host: &Host,
    dir: &Path,
    case: &Path,
    rustflags: Option<&str>,
    args: I,
) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = cargo_run(host, dir, Some(&case.join("cargo-home")), rustflags, args)
        .unwrap_or_else(|wait| {
            panic!(
                "cargo in {} waited on a lock the rest of the clone shares: {wait}",
                dir.display()
            )
        });
    assert!(
        output.status.success(),
        "cargo failed in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn reported_target_directory(dir: &Path, case: &Path) -> PathBuf {
    let output = cargo(
        &Host::this_clone(),
        dir,
        case,
        ["metadata", "--format-version", "1", "--no-deps"],
    );
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata is JSON");
    PathBuf::from(
        metadata["target_directory"]
            .as_str()
            .expect("cargo metadata reports target_directory"),
    )
}

/// The `rustc` command line a verbose build of the probe crate handed rustc,
/// chosen by `marker` where one build compiles the crate twice.
///
/// Cargo prints it only when it compiles, so a probe cargo judged fresh fails
/// here rather than passing on a line from an earlier run.
fn rustc_invocation(output: &Output, marker: &str) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    stderr
        .lines()
        .find(|line| {
            line.contains("Running")
                && line.contains("--crate-name build_config_probe")
                && line.contains(marker)
        })
        .unwrap_or_else(|| {
            panic!("no rustc invocation for the probe crate ({marker}) in:\n{stderr}")
        })
        .to_owned()
}

/// The `--out-dir` a rustc command line names, as a path rather than as text.
///
/// Cargo prints each argument shell-quoted when it needs to be — a Windows
/// path's backslashes, a space in the clone's path — and writes the platform's
/// own separator, so the text of the argument differs from `Path::display` of
/// the directory it names on exactly the platforms where the check matters.
/// Comparing paths compares components, which is what the assertion means.
fn out_dir(invocation: &str) -> PathBuf {
    let rest = invocation
        .split_once("--out-dir ")
        .unwrap_or_else(|| panic!("no --out-dir in: {invocation}"))
        .1;
    let value = match rest.chars().next() {
        Some(quote @ ('\'' | '"')) => rest[1..].split(quote).next(),
        _ => rest.split(' ').next(),
    };
    PathBuf::from(value.expect("--out-dir names a directory"))
}

/// The keys under a table, so a table holding more than the contract's is named
/// by what it holds rather than by a missing key.
fn keys(table: &toml::Value, name: &str) -> Vec<String> {
    table
        .as_table()
        .unwrap_or_else(|| panic!("[{name}] is a table"))
        .keys()
        .cloned()
        .collect()
}

#[test]
fn the_config_file_carries_the_contracts_three_keys_and_nothing_else() {
    let config: toml::Value = toml::from_str(&build_config()).expect(".cargo/config.toml parses");
    assert_eq!(
        keys(&config, ""),
        ["build", "profile", "target"],
        "the file holds the three tables the contract names and no other"
    );
    assert_eq!(keys(&config["build"], "build"), ["target-dir"]);
    assert_eq!(
        config["build"]["target-dir"].as_str(),
        Some("target"),
        "build.target-dir is the string \"target\""
    );
    assert_eq!(
        keys(&config["profile"], "profile"),
        ["dev"],
        "the file touches no profile but dev; release stays Cargo.toml's"
    );
    assert_eq!(keys(&config["profile"]["dev"], "profile.dev"), ["debug"]);
    assert_eq!(
        config["profile"]["dev"]["debug"].as_integer(),
        Some(0),
        "profile.dev.debug is the integer 0"
    );
    assert_eq!(
        keys(&config["target"], "target"),
        [LINUX_TARGETS],
        "the file names a linker for the Linux GNU targets alone, so macOS and Windows read none"
    );
    assert_eq!(
        keys(&config["target"][LINUX_TARGETS], LINUX_TARGETS),
        ["linker"]
    );
    assert_eq!(
        config["target"][LINUX_TARGETS]["linker"].as_str(),
        Some(LINUX_LINKER),
        "the Linux linker is the clone's own script"
    );
}

#[test]
fn a_shell_quoted_out_dir_is_read_as_the_path_it_names() {
    // The line a Windows runner printed: cargo quoted the path for its
    // backslashes, and the crate path beside it for the same reason.
    let quoted = "     Running `rustc.exe --crate-name build_config_probe --edition=2021 \
                  'src\\lib.rs' --crate-type lib -C debuginfo=1 \
                  --out-dir 'D:\\a\\onepipeline\\onepipeline\\target\\debug\\deps' \
                  -L 'dependency=D:\\a\\onepipeline\\onepipeline\\target\\debug\\deps'`";
    assert_eq!(
        out_dir(quoted),
        PathBuf::from("D:\\a\\onepipeline\\onepipeline\\target\\debug\\deps")
    );
    let bare =
        "     Running `rustc --crate-name build_config_probe --out-dir /clone/target/debug/deps \
                -L dependency=/clone/target/debug/deps`";
    assert_eq!(out_dir(bare), PathBuf::from("/clone/target/debug/deps"));
}

#[test]
fn every_crate_in_the_clone_builds_into_the_clones_own_target_directory() {
    let case = case_dir(&Host::this_clone(), "outside-workspace");
    let clone_target = repo_root().join("target");
    assert_eq!(
        reported_target_directory(&repo_root(), &case),
        clone_target,
        "the workspace root builds into <clone>/target"
    );
    let outside = case.join("probe");
    probe_crate(&outside);
    assert_eq!(
        reported_target_directory(&outside, &case),
        clone_target,
        "a crate outside the workspace, inside the clone, builds into the same <clone>/target"
    );
}

#[test]
fn a_second_checkout_carrying_its_own_copy_of_the_file_builds_into_its_own_root() {
    // A worktree nested inside a clone is the case that matters: cargo reads
    // both files, and the deeper one's `target-dir` has to win *and* resolve
    // against the deeper root, or two checkouts silently share one target.
    let nested_root = case_dir(&Host::this_clone(), "nested-checkout");
    probe_crate(&nested_root);
    copy_build_config(&nested_root);
    assert_eq!(
        reported_target_directory(&nested_root, &nested_root),
        nested_root.join("target"),
        "a checkout with its own copy of the file resolves target-dir against its own root"
    );
}

/// Build the probe under the dev, test and release profiles, in a checkout of
/// its own under `case` in `host`'s clone, and hold each build to the debuginfo
/// the file promises.
fn builds_carry_no_debuginfo(host: &Host, case: &str) {
    let checkout = case_dir(host, case);
    let probe = probe_checkout(&checkout);
    let deps = checkout.join("target/debug/deps");

    let dev = rustc_invocation(
        &cargo(host, &probe, &checkout, ["build", "-v", "--offline"]),
        "--crate-type lib",
    );
    // At `debug = 0` cargo passes no `-C debuginfo` level and strips the standard
    // library's debuginfo instead.
    assert!(
        !dev.contains("-C debuginfo=") && dev.contains("-C strip=debuginfo"),
        "a dev build carries no debuginfo: {dev}"
    );
    assert_eq!(
        out_dir(&dev),
        deps,
        "a dev build of a crate outside the workspace lands in <checkout>/target/debug: {dev}"
    );

    let test = rustc_invocation(
        &cargo(
            host,
            &probe,
            &checkout,
            ["test", "-v", "--no-run", "--offline"],
        ),
        "--test",
    );
    assert!(
        !test.contains("-C debuginfo=") && test.contains("-C strip=debuginfo"),
        "the test profile inherits dev's lack of debuginfo: {test}"
    );

    let release = rustc_invocation(
        &cargo(
            host,
            &probe,
            &checkout,
            ["build", "-v", "--offline", "--release"],
        ),
        "--crate-type lib",
    );
    // `-C strip=debuginfo` is cargo's own release default and names no level;
    // what the dev profile would have added is a `-C debuginfo=` level.
    assert!(
        !release.contains("-C debuginfo="),
        "a release build is untouched by the dev profile: {release}"
    );
}

#[test]
fn dev_test_and_release_builds_carry_no_debuginfo() {
    builds_carry_no_debuginfo(&Host::this_clone(), "debuginfo");
}

/// A clone and an inherited cargo home of the test's own, under `name`, with
/// every lock the rest of a clone's cargo work takes held exclusively until it
/// drops: the clone's dev and release build directories, and both halves of the
/// cargo home's package cache, which is what a cargo mutating it holds.
///
/// The test creates every one of those files, so nothing else on the host knows
/// them: each hold is taken at once, and lasts exactly as long as the fixture.
struct HeldHost {
    host: Host,
    _locks: Vec<File>,
}

impl HeldHost {
    fn hold(name: &str) -> Self {
        let root = case_dir(&Host::this_clone(), name);
        let clone = root.join("clone");
        copy_build_config(&clone);
        let home = root.join("inherited-cargo-home");
        let locks = [
            clone.join("target/debug/.cargo-lock"),
            clone.join("target/release/.cargo-lock"),
            home.join(".package-cache"),
            home.join(".package-cache-mutate"),
        ]
        .into_iter()
        .map(|path| {
            fs::create_dir_all(path.parent().expect("a lock has a directory"))
                .expect("a lock directory");
            let file = File::create_new(&path)
                .unwrap_or_else(|error| panic!("{} is created: {error}", path.display()));
            file.try_lock()
                .unwrap_or_else(|error| panic!("{} is held: {error}", path.display()));
            file
        })
        .collect();
        Self {
            host: Host {
                clone,
                inherited_home: Some(home),
            },
            _locks: locks,
        }
    }
}

#[test]
fn the_debuginfo_builds_wait_on_no_lock_the_rest_of_the_clone_holds() {
    // What the tier's neighbours do for minutes at a time: a build into the
    // clone's target directory, and a resolution against the cargo home the
    // environment hands every cargo. Held across all three builds, so a probe
    // that took any of these locks would report the wait, and `cargo` fails on it.
    let held = HeldHost::hold("held-for-the-debuginfo-builds");
    builds_carry_no_debuginfo(&held.host, "debuginfo");
}

#[test]
fn the_held_fixture_stops_a_probe_that_builds_into_the_clones_target() {
    // The probe as it was before it had a checkout of its own: a crate in the
    // clone, so it builds into the clone's target directory.
    let held = HeldHost::hold("held-against-the-shared-target");
    let probe = case_dir(&held.host, "probe");
    probe_crate(&probe);
    let wait = cargo_run(
        &held.host,
        &probe,
        Some(&probe.join("cargo-home")),
        None,
        ["build", "-v", "--offline"],
    )
    .expect_err("a build into the held clone's target directory waits on its lock");
    assert!(
        wait.contains("artifact directory"),
        "the wait is on the build directory: {wait}"
    );
}

#[test]
fn the_held_fixture_stops_a_probe_that_resolves_against_the_inherited_cargo_home() {
    // The probe as it was before it had a cargo home of its own: whatever the
    // environment handed it.
    let held = HeldHost::hold("held-against-the-inherited-home");
    let probe = probe_checkout(&case_dir(&held.host, "probe"));
    let wait = cargo_run(&held.host, &probe, None, None, ["build", "-v", "--offline"])
        .expect_err("a resolution against the held cargo home waits on its package cache");
    assert!(
        wait.contains("package cache"),
        "the wait is on the package cache: {wait}"
    );
}

/// The linker and version lld names in an ELF's `.comment` (`LLD 22.1.6`, from
/// its `Linker: LLD 22.1.6 (<source>)`), read off the file's bytes: GNU `ld`
/// writes none, so `None` is a GNU `ld` link.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn recorded_linker(binary: &Path) -> Option<String> {
    let bytes =
        fs::read(binary).unwrap_or_else(|error| panic!("{} reads: {error}", binary.display()));
    let marker = b"Linker: ";
    let start = bytes.windows(marker.len()).position(|w| w == marker)?;
    let start = start + marker.len();
    let end = bytes[start..].iter().position(|&b| b == 0)? + start;
    let line = String::from_utf8_lossy(&bytes[start..end]).into_owned();
    Some(
        line.split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// The version the toolchain's own rust-lld reports (`LLD 22.1.6`), found beside
/// the `rustc` that sits beside [`cargo_binary`], as rustc finds it.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn bundled_lld_version() -> String {
    let cargo = cargo_binary();
    let rustc = cargo.with_file_name("rustc");
    let rustc = if rustc.is_file() {
        rustc
    } else {
        PathBuf::from("rustc")
    };
    let ask = |args: &[&str]| {
        let output = Command::new(&rustc)
            .args(args)
            .output()
            .expect("rustc runs");
        assert!(output.status.success(), "rustc {args:?} fails");
        String::from_utf8(output.stdout).expect("rustc prints UTF-8")
    };
    let sysroot = ask(&["--print", "sysroot"]);
    let host = ask(&["-vV"])
        .lines()
        .find_map(|line| line.strip_prefix("host: ").map(str::to_owned))
        .expect("rustc -vV names its host");
    let lld = Path::new(sysroot.trim()).join(format!("lib/rustlib/{host}/bin/rust-lld"));
    let output = Command::new(&lld)
        .args(["-flavor", "gnu", "--version"])
        .output()
        .unwrap_or_else(|error| panic!("{} runs: {error}", lld.display()));
    let version = String::from_utf8_lossy(&output.stdout);
    version
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The test executables a `cargo test --no-run` of the probe wrote: every file
/// under `deps` named for the crate that carries no extension.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn probe_test_binaries(deps: &Path) -> Vec<PathBuf> {
    fs::read_dir(deps)
        .expect("a deps directory")
        .map(|entry| entry.expect("a deps entry").path())
        .filter(|path| {
            path.file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| name.starts_with("build_config_probe-") && !name.contains('.'))
        })
        .collect()
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[test]
fn linux_dev_and_test_builds_link_with_the_bundled_lld_and_release_as_it_would_without_it() {
    // Under the `-D warnings` the recipes set, which would have replaced a
    // config `rustflags` and does not reach a `linker`.
    let deny = Some("-D warnings");
    let host = Host::this_clone();
    let checkout = case_dir(&host, "linker");
    let probe = probe_checkout(&checkout);
    fs::write(probe.join("src/main.rs"), "fn main() {}\n").expect("a probe binary");
    let lld = Some(bundled_lld_version());

    cargo_with_rustflags(&host, &probe, &checkout, deny, ["build", "--offline"]);
    assert_eq!(
        recorded_linker(&checkout.join("target/debug/build-config-probe")),
        lld,
        "a dev build links with the toolchain's own rust-lld"
    );
    cargo_with_rustflags(
        &host,
        &probe,
        &checkout,
        deny,
        ["test", "--no-run", "--offline"],
    );
    let tests = probe_test_binaries(&checkout.join("target/debug/deps"));
    assert!(!tests.is_empty(), "cargo test --no-run wrote test binaries");
    for test in tests {
        assert_eq!(
            recorded_linker(&test),
            lld,
            "{} links with the toolchain's own rust-lld",
            test.display()
        );
    }

    // What a release links with when the file names no linker, which is the
    // target's default: GNU `ld` on aarch64, rustc's own rust-lld on x86_64.
    // Outside the clone, because cargo merges every config file upward and the
    // clone's would hand this checkout the linker it is meant to lack.
    let unset = std::env::temp_dir().join(format!(
        "onepipeline-build-config-linker-unset-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&unset);
    let unset_probe = probe_checkout(&unset);
    fs::write(unset_probe.join("src/main.rs"), "fn main() {}\n").expect("a probe binary");
    let without_target_table = build_config()
        .split("\n[target.")
        .next()
        .expect("the file has text before its target table")
        .to_owned();
    fs::write(unset.join(".cargo/config.toml"), without_target_table).expect("a trimmed config");
    for (root, dir) in [(&checkout, &probe), (&unset, &unset_probe)] {
        cargo_with_rustflags(&host, dir, root, deny, ["build", "--offline", "--release"]);
    }
    assert_eq!(
        recorded_linker(&checkout.join("target/release/build-config-probe")),
        recorded_linker(&unset.join("target/release/build-config-probe")),
        "a release build links with what it would without the file's linker"
    );
    let _ = fs::remove_dir_all(&unset);
}

/// The linker script run as rustc runs it — `-o <output>` among its arguments,
/// under `path` — with a `cc` ahead of it on `path` that prints the arguments it
/// was handed, one per line.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn linker_script_hands_cc(case: &Path, path: &[&Path], output: &str) -> Vec<String> {
    let stub = case.join("stub-bin");
    fs::create_dir_all(&stub).expect("a stub directory");
    let cc = stub.join("cc");
    fs::write(&cc, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").expect("a stub cc");
    make_executable(&cc);
    let path = std::env::join_paths(path.iter().copied().chain([stub.as_path()])).expect("a PATH");
    // By path: the PATH handed it holds no system directory.
    let ran = Command::new("/bin/sh")
        .arg(repo_root().join(LINUX_LINKER))
        .args(["main.o", "-o", output, "-nodefaultlibs"])
        .env("PATH", path)
        .output()
        .expect("sh runs the linker script");
    assert!(ran.status.success(), "the linker script fails: {ran:?}");
    String::from_utf8(ran.stdout)
        .expect("UTF-8")
        .lines()
        .map(str::to_owned)
        .collect()
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod +x");
}

/// A rustlib `bin` directory as rustc puts one on its linker's PATH: `rust-lld`
/// and the `gcc-ld/ld.lld` that runs it.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn bundled_lld_dir(case: &Path) -> PathBuf {
    let bin = case.join("rustlib-bin");
    fs::create_dir_all(bin.join("gcc-ld")).expect("a gcc-ld directory");
    for tool in [bin.join("rust-lld"), bin.join("gcc-ld/ld.lld")] {
        fs::write(&tool, "#!/bin/sh\n").expect("a stand-in lld");
        make_executable(&tool);
    }
    bin
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[test]
fn the_linker_script_selects_the_bundled_lld_for_dev_and_test_output_alone() {
    let case = case_dir(&Host::this_clone(), "linker-script");
    let bundled = bundled_lld_dir(&case);
    let unchanged = |output: &str| ["main.o", "-o", output, "-nodefaultlibs"].map(str::to_owned);
    for output in [
        "/clone/target/debug/deps/onepipeline-0123",
        "/clone/target/debug/build/serde-0123/build_script_build-0123",
        "/clone/target/debug/examples/demo-0123",
        "/clone/target/aarch64-unknown-linux-gnu/debug/deps/onepipeline-0123",
        "/clone/target/llvm-cov-target/debug/deps/onepipeline-0123",
    ] {
        let mut expected = vec![
            "-fuse-ld=lld".to_owned(),
            format!("-B{}", bundled.join("gcc-ld").display()),
        ];
        expected.extend(unchanged(output));
        assert_eq!(
            linker_script_hands_cc(&case, &[&bundled], output),
            expected,
            "{output} links with the bundled lld"
        );
    }
    for output in [
        "/clone/target/release/deps/onepipeline-0123",
        "/clone/target/release/build/serde-0123/build_script_build-0123",
        // A clone under a directory named `debug` is still a release build.
        "/home/debug/clone/target/release/deps/onepipeline-0123",
    ] {
        assert_eq!(
            linker_script_hands_cc(&case, &[&bundled], output),
            unchanged(output),
            "{output} is handed to cc unchanged"
        );
    }
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[test]
fn the_linker_script_hands_cc_the_link_unchanged_where_the_toolchain_ships_no_lld() {
    let case = case_dir(&Host::this_clone(), "linker-script-no-lld");
    // A rustlib `bin` holding `rust-lld` but no `gcc-ld`, and one holding
    // neither: neither can run the bundled lld, so the build keeps cc's default.
    let partial = case.join("partial-bin");
    fs::create_dir_all(&partial).expect("a partial rustlib bin");
    fs::write(partial.join("rust-lld"), "#!/bin/sh\n").expect("a stand-in rust-lld");
    make_executable(&partial.join("rust-lld"));
    let empty = case.join("empty-bin");
    fs::create_dir_all(&empty).expect("an empty rustlib bin");
    let output = "/clone/target/debug/deps/onepipeline-0123";
    assert_eq!(
        linker_script_hands_cc(&case, &[&partial, &empty], output),
        ["main.o", "-o", output, "-nodefaultlibs"],
        "with no bundled lld the dev link is cc's default"
    );
}
