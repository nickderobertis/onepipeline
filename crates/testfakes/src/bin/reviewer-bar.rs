//! A bar fingerprint, as `start --envelope-reviewer-bar` names the command that
//! prints one.
//!
//! **Not a double for anything in this stack**: a host's bar is whatever it
//! judges an envelope against — a reviewer prompt, a criteria revision — and the
//! command it names prints something that changes when that does. What stands
//! here prints the fingerprint a journey scripts:
//!
//!   `reviewer-bar.fingerprint`  present → print this file's text and exit 0;
//!                               absent → print nothing and exit 1, which is a
//!                               bar that cannot be read
//!
//!   `reviewer-bar.burn`         present → before answering, spend this many
//!                               milliseconds of CPU, then write the CPU this process
//!                               spent, user and system, in milliseconds, to
//!                               `reviewer-bar.cpu` — `null` where the platform has
//!                               no `getrusage` — which is a host's command that does
//!                               real work and says what it cost
//!
//! Every invocation is recorded to `reviewer-bar.jsonl`, so a journey can tell a
//! bar that was read from one nobody asked.

use onepipeline_testfakes as fake;

/// This process's own CPU, user and system, from `getrusage`.
#[cfg(unix)]
fn own_cpu() -> Option<std::time::Duration> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: `getrusage` fills the one `rusage` the pointer names, which lives for
    // the call; it is read only when the call says it filled it.
    let usage = unsafe {
        if libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) != 0 {
            return None;
        }
        usage.assume_init()
    };
    let time = |value: libc::timeval| {
        std::time::Duration::new(
            u64::try_from(value.tv_sec).unwrap_or(0),
            u32::try_from(value.tv_usec).unwrap_or(0) * 1000,
        )
    };
    Some(time(usage.ru_utime) + time(usage.ru_stime))
}

#[cfg(not(unix))]
fn own_cpu() -> Option<std::time::Duration> {
    None
}

/// Spend `ms` of CPU — of wall clock where there is no CPU clock to read — and
/// write what was spent.
fn burn(dir: &std::path::Path, ms: u64) {
    let wanted = std::time::Duration::from_millis(ms);
    let began = std::time::Instant::now();
    let mut spun: u64 = 0;
    while own_cpu().unwrap_or_else(|| began.elapsed()) < wanted {
        for _ in 0..10_000 {
            spun =
                std::hint::black_box(spun.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1));
        }
    }
    let spent = own_cpu().map(|cpu| cpu.as_secs_f64() * 1e3);
    std::fs::write(
        dir.join("reviewer-bar.cpu"),
        serde_json::json!(spent).to_string(),
    )
    .unwrap_or_else(|error| fake::fail(&format!("reviewer-bar.cpu is not written: {error}")));
}

fn main() -> std::process::ExitCode {
    let dir = fake::script_dir();
    if let Ok(ms) = std::fs::read_to_string(dir.join("reviewer-bar.burn")) {
        let ms = ms
            .trim()
            .parse()
            .unwrap_or_else(|_| fake::fail("reviewer-bar.burn is not a count of milliseconds"));
        burn(&dir, ms);
    }
    let fingerprint = std::fs::read_to_string(dir.join("reviewer-bar.fingerprint")).ok();
    fake::append(
        &dir.join("reviewer-bar.jsonl"),
        &serde_json::json!({ "printed": fingerprint.as_deref().map(str::trim) }).to_string(),
    );
    match fingerprint {
        Some(fingerprint) => {
            println!("{}", fingerprint.trim());
            std::process::ExitCode::SUCCESS
        }
        None => std::process::ExitCode::from(1),
    }
}
