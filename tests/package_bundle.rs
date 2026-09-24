use std::path::PathBuf;
use std::process::Command;

#[test]
fn the_packaged_crate_carries_the_committed_events_bundle() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let package = Command::new(cargo)
        .current_dir(&root)
        .args([
            "package",
            "--package",
            "onepipeline",
            "--locked",
            "--offline",
            "--allow-dirty",
            "--no-verify",
        ])
        .output()
        .expect("cargo package runs");
    assert!(
        package.status.success(),
        "cargo package refused: {}",
        String::from_utf8_lossy(&package.stderr)
    );

    let version = env!("CARGO_PKG_VERSION");
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"));
    let archive = target
        .join("package")
        .join(format!("onepipeline-{version}.crate"));
    let carried = Command::new("tar")
        .args(["-xOzf"])
        .arg(&archive)
        .arg(format!("onepipeline-{version}/schemas/events.json"))
        .output()
        .expect("the crate archive is readable");
    assert!(
        carried.status.success(),
        "the crate omitted schemas/events.json: {}",
        String::from_utf8_lossy(&carried.stderr)
    );
    assert_eq!(carried.stdout, include_bytes!("../schemas/events.json"));
}
