//! The build configuration every cargo invocation inside this clone reads, held
//! to what `.cargo/config.toml` promises.
//!
//! That file carries two keys, and each is a promise about *every* build under
//! this clone rather than about the workspace: `build.target-dir` puts a crate
//! outside the workspace into the same `<clone>/target` the members use, so one
//! clone never compiles its dependency graph into two trees, and
//! `profile.dev.debug = 0` builds every dev and test build without debuginfo. Neither can be read off the manifest — a `Cargo.toml` profile
//! reaches only its own workspace — and neither is proven by the file parsing,
//! because cargo merges config files from the working directory *upward* and
//! resolves each `target-dir` against its own file's location, which is exactly
//! where a copy of the file goes wrong.
//!
//! So these ask cargo. A probe crate is written under `target/build-config/`,
//! outside the workspace and inside the clone, and what is asserted is what cargo
//! reports for it and what it hands `rustc` when it builds it. The overrides the
//! environment may carry (`CARGO_TARGET_DIR`, `RUSTFLAGS`) are stripped from the
//! probe's environment on purpose: each outranks the file, and what is under
//! test is the file.
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
    fs::create_dir_all(case.join(".cargo")).expect("a checkout config directory");
    fs::write(case.join(".cargo/config.toml"), build_config()).expect("a copied config");
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
/// outranks the config file removed, and under `own_home` where one is named.
///
/// A cargo that reports it is waiting on a lock is ended there, and what is
/// returned is the line naming the lock.
fn cargo_run<I, S>(
    host: &Host,
    dir: &Path,
    own_home: Option<&Path>,
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
    let output =
        cargo_run(host, dir, Some(&case.join("cargo-home")), args).unwrap_or_else(|wait| {
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
fn the_config_file_carries_the_contracts_two_keys_and_nothing_else() {
    let config: toml::Value = toml::from_str(&build_config()).expect(".cargo/config.toml parses");
    assert_eq!(
        keys(&config, ""),
        ["build", "profile"],
        "the file holds the two tables the contract names and no other"
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
    fs::create_dir_all(nested_root.join(".cargo")).expect("a nested config directory");
    fs::write(nested_root.join(".cargo/config.toml"), build_config()).expect("a copied config");
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
        fs::create_dir_all(clone.join(".cargo")).expect("a fixture clone");
        fs::write(clone.join(".cargo/config.toml"), build_config()).expect("a copied config");
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
    let wait = cargo_run(&held.host, &probe, None, ["build", "-v", "--offline"])
        .expect_err("a resolution against the held cargo home waits on its package cache");
    assert!(
        wait.contains("package cache"),
        "the wait is on the package cache: {wait}"
    );
}
