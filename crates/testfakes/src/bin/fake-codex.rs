//! A real `codex` executable, for the journeys that drive the **real**
//! `oneagentgraph` through a Codex candidate that *answers*.
//!
//! It stands where `fake-claude` stands — at the paid model turn, named at
//! oneharness's own `ONEHARNESS_BIN_CODEX` seam — and it is a separate double
//! because the two providers' terminal documents are different contracts: what
//! the linked classifier reads off a Codex turn is Codex's own `--json` event
//! stream, and a journey through that classifier has to hand it Codex's own
//! spelling. Kept to the one answer a journey asks of it: the terminal
//! `turn.failed` a server that cannot accept a turn ends on, which exits
//! cleanly, so only the structured error can classify the attempt as failed.

use onepipeline_testfakes as fake;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Before the script directory is required, for the reason `fake-claude`
    // gives: the `--version` probe decides whether the identity is installed,
    // and is not a turn.
    if args.iter().any(|arg| arg == "--version") {
        println!("codex-cli 1.0.0 (fake-codex)");
        return ExitCode::SUCCESS;
    }
    let dir = fake::script_dir();
    fake::record(&dir, "codex", &args);
    println!(r#"{{"type":"turn.failed","error":{{"codex_error_info":"server_overloaded"}}}}"#);
    ExitCode::SUCCESS
}
