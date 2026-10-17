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
fn release_packaging_retains_every_target_and_installs_apple_silicon_before_publication() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(root.join(".github/workflows/release.yml")).unwrap();
    let release: serde_json::Value = onepipeline_testfakes::yaml::from_str(&text).unwrap();
    let jobs = &release["jobs"];
    let plan = |platform: &str| -> Vec<String> {
        let out = std::process::Command::new("python3")
            .args(["scripts/release.py", "plan", platform])
            .current_dir(&root)
            .output()
            .expect("python3 runs scripts/release.py");
        assert!(
            out.status.success(),
            "plan {platform} failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let plan: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        plan["targets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap().to_owned())
            .collect()
    };
    let mut built = Vec::new();
    for (platform, runner) in [
        ("linux", "ubuntu-latest"),
        ("macos", "macos-latest"),
        ("windows", "windows-latest"),
    ] {
        let job = &jobs[format!("build-{platform}").as_str()];
        assert_eq!(job["runs-on"].as_str(), Some(runner));
        assert!(job.get("strategy").is_none(), "build-{platform} is one job");
        let runs: Vec<&str> = job["steps"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|s| s["run"].as_str())
            .collect();
        // The build phase test-installs every artifact before the publish phase
        // can run: scripts/release.py's `build` refuses to finish otherwise.
        let at = |phase: &str| {
            runs.iter()
                .position(|run| *run == format!("just release-{platform} {phase}"))
                .unwrap_or_else(|| panic!("build-{platform} has no {phase} step"))
        };
        assert!(at("build") < at("publish"));
        built.extend(plan(platform));
    }
    // Intel macOS builds are discontinued: the one Mac job builds, packs and
    // installs the Apple silicon target natively, with no Rosetta and no x86_64
    // cross-compilation. Dispatch-local proof runs on Linux, so these structural
    // assertions hold the Mac job's commands without simulating Darwin execution.
    assert_eq!(plan("macos"), ["aarch64-apple-darwin"]);
    for (path, forbidden) in [
        (".github/workflows/release.yml", &text),
        (
            "scripts/build-staged-release.py",
            &std::fs::read_to_string(root.join("scripts/build-staged-release.py")).unwrap(),
        ),
        (
            "scripts/release.py",
            &std::fs::read_to_string(root.join("scripts/release.py")).unwrap(),
        ),
    ] {
        for word in [
            "x86_64-apple-darwin",
            "rosetta",
            "arch -x86_64",
            "darwin-x64",
        ] {
            assert!(
                !forbidden.to_lowercase().contains(word),
                "{path} still carries Intel macOS: {word}"
            );
        }
    }
    let script = std::fs::read_to_string(root.join("scripts/npm-build.mjs")).unwrap();
    let mut declared: Vec<&str> = script
        .lines()
        .filter_map(|line| {
            let (target, _) = line.trim().strip_prefix('"')?.split_once("\": {")?;
            target.contains('-').then_some(target)
        })
        .collect();
    declared.sort_unstable();
    built.sort_unstable();
    assert_eq!(
        built, declared,
        "every released platform is built by exactly one build job"
    );
    let needs: Vec<&str> = jobs["publish"]["needs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(needs, ["build-linux", "build-macos", "build-windows"]);
}
