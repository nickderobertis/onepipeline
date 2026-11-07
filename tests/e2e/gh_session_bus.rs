//! The real `gh` under the environment a [`World`] leaves its test process with, against a
//! recording `dbus-launch` that starts nothing, so a failing guard leaks nothing.

// llmlint: ignore-file[e2e_not_mocked] the layer under test is this harness's environment and
// the real `gh` and `onevcs` it reaches, both driven for real. The one stand-in is `dbus-launch`,
// because the real one would start the very daemon this guard exists to keep from outliving it.

use std::path::Path;

use onepipeline_testfakes::profile::process::Command;

use crate::harness::{git, onevcs_binary, path_leading_with, World, NO_SESSION_BUS};

/// How many times one `onevcs register` of a GitHub identity has `gh` ask for a bus.
///
/// Registering asks the host for the repository's visibility. A signed-in account with no
/// token in its configuration sends `gh` to the keyring, and it stops there; the closed
/// proxy is there so that nothing could reach GitHub even if a later `gh` went on.
fn asks_for_a_bus(world: &World, bin: &Path, checkout: &Path, without_the_variable: bool) -> usize {
    let record = bin.join("bus-calls");
    let _ = std::fs::remove_file(&record);
    let mut register = Command::new(onevcs_binary());
    register
        .args(["register", &checkout.to_string_lossy()])
        .args(["--origin", "https://github.com/owner/service.git"])
        .env("ONEVCS_HOME", world.onevcs_home())
        .env("GIT_CONFIG_GLOBAL", world.gitconfig())
        .env("GH_CONFIG_DIR", bin)
        .env("BUS_RECORD", &record)
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("PATH", path_leading_with(&[bin.to_path_buf()]))
        .env_remove("NO_PROXY")
        .env_remove("no_proxy")
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN");
    if without_the_variable {
        register.env_remove(NO_SESSION_BUS.0);
    }
    let registered = register.output().expect("the onevcs binary runs");
    assert!(
        registered.status.success(),
        "`onevcs register` failed: {}",
        String::from_utf8_lossy(&registered.stderr)
    );
    std::fs::read_to_string(&record)
        .map(|calls| calls.lines().count())
        .unwrap_or(0)
}

#[test]
fn a_worlds_environment_keeps_the_real_gh_from_asking_for_a_session_bus() {
    let world = World::new("gh-session-bus");
    let no_real_gh = Command::new("gh").arg("--version").output().is_err();
    if !cfg!(target_os = "linux") || no_real_gh {
        eprintln!(
            "skipped: no session bus can start here, as this guard needs the real gh on Linux"
        );
        return;
    }
    let bin = world.root.join("bus-recording");
    std::fs::create_dir_all(&bin).expect("a directory for the recording");
    onepipeline_testfakes::executable(
        &bin.join("dbus-launch"),
        "#!/bin/sh\necho called >> \"$BUS_RECORD\"\nexit 1\n",
    );
    std::fs::write(
        bin.join("hosts.yml"),
        "github.com:\n    user: fixture\n    git_protocol: https\n",
    )
    .expect("a signed-in account for gh to look up");
    let checkout = world.root.join("bus-checkout");
    std::fs::create_dir_all(&checkout).expect("a checkout");
    git(&world, &checkout, &["init", "-q", "--initial-branch=main"]);

    if asks_for_a_bus(&world, &bin, &checkout, true) == 0 {
        eprintln!(
            "skipped: this gh asks for no session bus even without the variable, so none can leak"
        );
        return;
    }
    assert_eq!(
        asks_for_a_bus(&world, &bin, &checkout, false),
        0,
        "a world's environment lets gh ask for a session bus"
    );
    assert_eq!(
        std::env::var(NO_SESSION_BUS.0).as_deref(),
        Ok(NO_SESSION_BUS.1)
    );
}
