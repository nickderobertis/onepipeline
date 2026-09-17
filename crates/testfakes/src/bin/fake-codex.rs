//! A Codex paid-turn double for journeys through the linked classifier.

use onepipeline_testfakes as fake;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--version") {
        println!("codex-cli 1.0.0");
        return ExitCode::SUCCESS;
    }

    let dir = fake::script_dir();
    fake::record(&dir, "codex", &args);
    fake::count(&dir, "codex-overloaded");

    // The terminal event Codex emits when its server cannot accept a turn. It
    // exits cleanly, so only the structured error can classify the attempt as
    // failed; this is the producer boundary the linked core must read.
    println!(r#"{{"type":"turn.failed","error":{{"codex_error_info":"server_overloaded"}}}}"#);
    ExitCode::SUCCESS
}
