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
//! Every invocation is recorded to `reviewer-bar.jsonl`, so a journey can tell a
//! bar that was read from one nobody asked.

use onepipeline_testfakes as fake;

fn main() -> std::process::ExitCode {
    let dir = fake::script_dir();
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
