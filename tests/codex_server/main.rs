//! The Codex double's own process-boundary contract.
use std::path::PathBuf;
use std::process::Command;

#[cfg(unix)]
mod wire;

fn double() -> PathBuf {
    static BUILT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    BUILT
        .get_or_init(|| {
            let running = std::env::current_exe().unwrap();
            let debug = running.parent().unwrap().parent().unwrap();
            let mut cargo =
                Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
            cargo
                .args([
                    "build",
                    "--offline",
                    "--locked",
                    "--package",
                    "onepipeline-testfakes",
                    "--bin",
                    "fake-codex",
                    "--target-dir",
                ])
                .arg(debug.parent().unwrap());
            // The nested build uses the enclosing tier's instrumentation, but not
            // Cargo's metadata about this test crate (which build scripts track).
            for (name, _) in std::env::vars_os() {
                let key = name.to_string_lossy();
                if key.starts_with("CARGO_PKG_")
                    || key.starts_with("CARGO_BIN_EXE_")
                    || [
                        "CARGO_MANIFEST_DIR",
                        "CARGO_MANIFEST_PATH",
                        "CARGO_CRATE_NAME",
                        "CARGO_BIN_NAME",
                        "CARGO_PRIMARY_PACKAGE",
                        "CARGO_TARGET_TMPDIR",
                        "CARGO_RUSTC_CURRENT_DIR",
                        "OUT_DIR",
                    ]
                    .contains(&key.as_ref())
                {
                    cargo.env_remove(name);
                }
            }
            let built = cargo.output().unwrap();
            assert!(
                built.status.success(),
                "{}",
                String::from_utf8_lossy(&built.stderr)
            );
            debug.join(format!("fake-codex{}", std::env::consts::EXE_SUFFIX))
        })
        .clone()
}

#[test]
fn the_codex_double_answers_its_version_probe() {
    let output = Command::new(double()).arg("--version").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "codex-cli 1.0.0 (fake-codex)"
    );
    assert!(output.stderr.is_empty());
}
