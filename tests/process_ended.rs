//! Whether a process has ended, as the suite's waits ask it of
//! `onepipeline_testfakes::containment::process_ended`.
//!
//! On Unix that probe asks the kernel with signal 0, and a zombie answers it as
//! alive, so a process the probe finds is then looked up in `ps` for its `Z`
//! state. A parent can reap the zombie between those two questions, and the `ps`
//! that runs after it lists nothing at the pid. On a loaded runner, the
//! stop-guard journey read a driver that `end_driver` had just waited out as
//! still running this way. The order is forced here: the `ps` the probe finds
//! first on `PATH` waits until this test has reaped its child before it runs the
//! host's own `ps`.
//!
//! One test in its own binary, because it moves `PATH` for the whole process.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

/// A child that was killed, and then reaped while `ps` was starting, has ended,
/// even though the `ps` that then reads the process table does not list it.
#[test]
fn a_zombie_reaped_before_ps_reads_the_table_has_ended() {
    let scratch = std::env::temp_dir().join(format!("process-ended-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).expect("a scratch directory");
    let asked = scratch.join("asked");
    let reaped = scratch.join("reaped");
    let host_ps = which("ps");
    let delayed = scratch.join("ps");
    std::fs::write(
        &delayed,
        format!(
            "#!/bin/sh\n: > '{asked}'\nwhile [ ! -e '{reaped}' ]; do sleep 0.01; done\nexec '{host_ps}' \"$@\"\n",
            asked = asked.display(),
            reaped = reaped.display(),
            host_ps = host_ps.display(),
        ),
    )
    .expect("the delayed ps");
    std::fs::set_permissions(&delayed, std::fs::Permissions::from_mode(0o755))
        .expect("the delayed ps is executable");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut ahead = vec![scratch.clone()];
    ahead.extend(std::env::split_paths(&path));
    std::env::set_var("PATH", std::env::join_paths(ahead).expect("a PATH"));

    let mut child = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .expect("a child to end");
    let pid = child.id();
    let killed = std::process::Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .status()
        .expect("this host ends a process it owns");
    assert!(killed.success(), "pid {pid} was not signalled");
    // Killed but not reaped: the probe's first question finds it, as a zombie.
    let asking = std::thread::spawn(move || onepipeline_testfakes::containment::process_ended(pid));
    wait_for(&asked);
    child.wait().expect("the zombie is reaped");
    std::fs::write(&reaped, "").expect("the reap is announced");

    let ended = asking.join().expect("the probe returns");
    std::fs::remove_dir_all(&scratch).expect("the scratch directory goes");
    assert!(ended, "pid {pid} was reaped, yet read as still running");
}

/// The host's own `ps`, found before `PATH` is moved in front of it.
fn which(program: &str) -> std::path::PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| panic!("this host has no {program} on PATH"))
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "{} never appeared: the probe did not ask ps",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
