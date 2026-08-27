//! Provisioning journeys driven through the same `just` recipe CI uses.

#[cfg(unix)]
mod unix {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).expect("the provisioning scratch directory is removed");
        }
    }

    fn executable(path: &Path, contents: &str) {
        fs::write(path, contents).expect("the executable is written");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .expect("the executable is runnable");
    }

    /// A cached binary is not evidence that it came from the pinned revision.
    ///
    /// This puts a wrong-revision `onetaskgraph` and Cargo's installer ahead of
    /// the host tools, drives the real provisioning recipe, and then resolves
    /// the binary again. The installer records the requested revision in the
    /// installed executable, so the final invocation proves which pin the
    /// provisioning path replaced the stale binary with.
    #[test]
    fn provisioning_replaces_a_wrong_revision_on_path_with_the_pin() {
        let root =
            std::env::temp_dir().join(format!("onepipeline-provisioning-{}", std::process::id()));
        fs::create_dir(&root).expect("a fresh provisioning scratch directory");
        let _scratch = Scratch(root.clone());
        let bin = root.join("bin");
        fs::create_dir(&bin).expect("the stale installation has a bin directory");

        let binary = bin.join("onetaskgraph");
        executable(&binary, "#!/bin/sh\nprintf '%s\\n' wrong-revision\n");
        executable(
            &bin.join("cargo"),
            r#"#!/bin/sh
set -eu
revision=
root=${CARGO_HOME:?}
force=false
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--rev" ]; then
    revision=$2
    shift 2
    continue
  fi
  if [ "$1" = "--root" ]; then
    root=$2
    shift 2
    continue
  fi
  if [ "$1" = "--force" ]; then
    force=true
  fi
  shift
done
[ -n "$revision" ]
[ "$root" = "$CARGO_HOME" ] || [ "$force" = true ]
mkdir -p "$root/bin"
cat > "$root/bin/onetaskgraph" <<EOF
#!/bin/sh
printf '%s\\n' '$revision'
EOF
chmod +x "$root/bin/onetaskgraph"
"#,
        );

        let host_path = std::env::var_os("PATH").expect("the host has a PATH");
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(&host_path));
        let path = std::env::join_paths(paths).expect("the test PATH joins");
        let provisioned = Command::new("just")
            .arg("_ensure-onetaskgraph")
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("PATH", &path)
            .env("CARGO_HOME", root.join("cargo-home"))
            .output()
            .expect("the provisioning recipe runs");
        assert!(
            provisioned.status.success(),
            "provisioning failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&provisioned.stdout),
            String::from_utf8_lossy(&provisioned.stderr)
        );

        let resolved = Command::new("onetaskgraph")
            .env("PATH", path)
            .output()
            .expect("the provisioned binary resolves");
        let declaration = include_str!("../src/taskgraph.rs")
            .lines()
            .find(|line| line.starts_with("pub const FIRST_REVISION"))
            .expect("taskgraph declares the provisioning revision");
        let pinned = declaration
            .split('"')
            .nth(1)
            .expect("the revision declaration carries a quoted value");
        assert_eq!(
            String::from_utf8_lossy(&resolved.stdout).trim(),
            pinned,
            "the wrong-revision binary survived provisioning"
        );

        let cargo_home = root.join("default-cargo-home");
        let cargo_bin = cargo_home.join("bin");
        let tools = root.join("tools");
        fs::create_dir_all(&cargo_bin).expect("the default Cargo bin exists");
        fs::create_dir(&tools).expect("the installer tools directory exists");
        executable(
            &cargo_bin.join("onetaskgraph"),
            "#!/bin/sh\nprintf '%s\\n' wrong-revision\n",
        );
        fs::copy(root.join("bin/cargo"), tools.join("cargo"))
            .expect("the same recording installer is ahead of Cargo's bin");
        let mut default_paths = vec![tools, cargo_bin];
        default_paths.extend(std::env::split_paths(&host_path));
        let default_path = std::env::join_paths(default_paths).expect("the default PATH joins");
        let default_provisioned = Command::new("just")
            .arg("_ensure-onetaskgraph")
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("PATH", &default_path)
            .env("CARGO_HOME", &cargo_home)
            .output()
            .expect("the default-Cargo provisioning recipe runs");
        assert!(
            default_provisioned.status.success(),
            "default-Cargo provisioning failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&default_provisioned.stdout),
            String::from_utf8_lossy(&default_provisioned.stderr)
        );
        let default_resolved = Command::new("onetaskgraph")
            .env("PATH", default_path)
            .output()
            .expect("the default-Cargo provisioned binary resolves");
        assert_eq!(
            String::from_utf8_lossy(&default_resolved.stdout).trim(),
            pinned,
            "the wrong revision survived in the default Cargo home"
        );
    }
}
