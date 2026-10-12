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

    // And the one template the library embeds, which it reads at compile time: a crate
    // without it does not build.
    let base = Command::new("tar")
        .args(["-xOzf"])
        .arg(&archive)
        .arg(format!(
            "onepipeline-{version}/src/templates/onepipeline/plan-task.md.j2"
        ))
        .output()
        .expect("the crate archive is readable");
    assert!(
        base.status.success(),
        "the crate omitted plan-task's base: {}",
        String::from_utf8_lossy(&base.stderr)
    );
    assert_eq!(
        base.stdout,
        onepipeline::templates::BASE_SOURCE.as_bytes(),
        "the packaged base is not the one the library embeds"
    );
}

#[test]
fn release_packaging_retains_every_target_and_installs_darwin_before_publication() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(root.join(".github/workflows/release.yml")).unwrap();
    let release: serde_norway::Value = serde_norway::from_str(&text).unwrap();
    let jobs = &release["jobs"];
    let darwin = &jobs["darwin"];
    assert_eq!(darwin["runs-on"].as_str(), Some("macos-latest"));
    assert!(darwin.get("strategy").is_none());
    let runs: Vec<&str> = darwin["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .filter_map(|s| s["run"].as_str())
        .collect();
    let builds: Vec<&str> = runs
        .iter()
        .flat_map(|s| s.lines())
        .filter(|s| s.contains("scripts/build-staged-release.py --target"))
        .collect();
    let darwin_targets: Vec<&str> = builds
        .iter()
        .map(|s| {
            assert!(
                s.contains("--install"),
                "Darwin must be installed before publication"
            );
            s.split("--target ")
                .nth(1)
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap()
        })
        .collect();
    assert!(runs.iter().any(|s| s.contains("--install-rosetta")));
    assert!(builds.iter().any(|s| s.contains("--node \"$X64_NODE\"")));
    let script = std::fs::read_to_string(root.join("scripts/npm-build.mjs")).unwrap();
    let mut declared: Vec<&str> = script
        .lines()
        .filter_map(|line| {
            let (target, _) = line.trim().strip_prefix('"')?.split_once("\": {")?;
            target.contains('-').then_some(target)
        })
        .collect();
    declared.sort_unstable();
    for job in ["upload", "build-wheels", "build-npm"] {
        let mut targets: Vec<&str> = jobs[job]["strategy"]["matrix"]["include"]
            .as_sequence()
            .unwrap()
            .iter()
            .map(|leg| leg["target"].as_str().unwrap())
            .collect();
        targets.extend(darwin_targets.iter().copied());
        targets.sort_unstable();
        assert_eq!(targets, declared, "{job} omitted a released platform");
    }
    for (publisher, builder) in [
        ("publish-pypi", "build-wheels"),
        ("publish-npm", "build-npm"),
    ] {
        let needs: Vec<&str> = jobs[publisher]["needs"]
            .as_sequence()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(needs, [builder, "darwin"]);
        assert_eq!(jobs[publisher]["runs-on"].as_str(), Some("ubuntu-latest"));
    }
    assert_eq!(jobs["upload-darwin"]["needs"].as_str(), Some("darwin"));
    assert_eq!(
        jobs["upload-darwin"]["runs-on"].as_str(),
        Some("ubuntu-latest")
    );
    for artifact in [
        "wheels-darwin",
        "npm-darwin",
        "archives-darwin",
        "payload-hashes-darwin",
    ] {
        assert!(darwin["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .any(|step| step["with"]["name"].as_str() == Some(artifact)));
    }
}
