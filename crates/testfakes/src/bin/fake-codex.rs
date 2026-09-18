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
//!
//! It speaks the `codex exec` line oneharness builds — `codex exec [resume ID]
//! [--dangerously-bypass-approvals-and-sandbox | --sandbox read-only|
//! workspace-write] [--model M] [--json] (PROMPT | -)` — and refuses any other,
//! so a line oneharness starts sending that this double does not speak is a
//! refusal a journey reads rather than an argument it silently swallows.

use onepipeline_testfakes as fake;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Before the script directory is required, for the reason `fake-claude`
    // gives: the `--version` probe decides whether the identity is installed,
    // and is not a turn. Exactly the argv oneharness probes with — `--version`
    // and nothing else — so a `--version` reaching an `exec` line is refused
    // below as an argument Codex does not take, rather than answering a probe.
    if args == ["--version"] {
        println!("codex-cli 1.0.0 (fake-codex)");
        return ExitCode::SUCCESS;
    }
    let dir = fake::script_dir();
    // Recorded first, so an argv this double refuses is still readable by the
    // journey that has to explain why the candidate was stepped past.
    fake::record(&dir, "codex", &args);
    if let Err(refusal) = declared(&args) {
        return fake::refuse(&refusal);
    }
    println!("{}", fake::CODEX_SERVER_OVERLOADED);
    ExitCode::SUCCESS
}

/// The sandboxes `codex exec --sandbox` takes, as oneharness maps its modes
/// onto them.
// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] a copy of a *provider's*
// CLI, which no crate in this dependency graph declares as data — oneharness's
// mapping onto it is a private function in `domain::harness` — and gated the way
// `fake-claude`'s `MODES` is: `tests/e2e/dispatch.rs` drives the real
// `oneharness_core` against this binary, so a sandbox oneharness starts sending
// that is not below is a refusal there rather than a double that reads differently.
const SANDBOXES: [&str; 2] = ["read-only", "workspace-write"];

/// Whether `args` is a `codex exec` line oneharness builds, or why it is not.
///
/// The verb first, then its options in any order, then exactly one prompt —
/// the positional, or `-` for one piped on stdin — and nothing after it.
// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] the grammar of a
// *provider's* CLI, which no crate in this dependency graph declares as data, and
// gated the way `SANDBOXES` above is: the real `oneharness_core` builds the argv
// `tests/e2e/dispatch.rs` drives this binary on, so a line it starts sending that
// this does not speak is a refusal that journey reads, and the double's own tests
// there hold each form it accepts and each it refuses.
fn declared(args: &[String]) -> Result<(), String> {
    let mut rest = args.iter().map(String::as_str).peekable();
    if rest.next() != Some("exec") {
        return Err("codex is only driven as `codex exec` here".to_owned());
    }
    if rest.peek() == Some(&"resume") {
        rest.next();
        rest.next()
            .filter(|id| !id.starts_with('-'))
            .ok_or("codex's `exec resume` takes a thread id, and nothing followed it")?;
    }
    let mut prompt = None;
    while let Some(arg) = rest.next() {
        match arg {
            "--dangerously-bypass-approvals-and-sandbox" | "--json" => {}
            "--sandbox" => {
                let sandbox = rest
                    .next()
                    .ok_or("codex's --sandbox takes a value, and nothing followed it")?;
                if !SANDBOXES.contains(&sandbox) {
                    return Err(format!("codex takes no sandbox {sandbox:?}"));
                }
            }
            "--model" => {
                let model = rest
                    .next()
                    .ok_or("codex's --model takes a value, and nothing followed it")?;
                if model.starts_with('-') {
                    return Err(format!(
                        "codex's --model takes a model name, and {model:?} is an option"
                    ));
                }
            }
            flag if flag.starts_with('-') && flag != "-" => {
                return Err(format!("codex takes no argument {flag:?}"));
            }
            positional => {
                if prompt.replace(positional).is_some() {
                    return Err("codex exec takes one prompt, and two were sent".to_owned());
                }
            }
        }
    }
    prompt
        .map(|_| ())
        .ok_or_else(|| "codex exec was sent no prompt".to_owned())
}
