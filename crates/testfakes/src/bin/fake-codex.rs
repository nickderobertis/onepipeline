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
//! [-c features.plugins=false|true] [-c features.apps=false|true]
//! [-c analytics.enabled=false|true] [-c model_catalog_json=PATH] [--dangerously-bypass-approvals-and-sandbox |
//! --sandbox read-only|workspace-write] [--model M] [--json] (PROMPT | -)` — and
//! refuses any other, so a line oneharness starts sending that this double does
//! not speak is a refusal a journey reads rather than an argument it silently
//! swallows.

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
    let dir = script_dir();
    // Recorded first, so an argv this double refuses is still readable by the
    // journey that has to explain why the candidate was stepped past.
    fake::record(&dir, "codex", &args);
    if let Err(refusal) = declared(&args) {
        return fake::refuse(&refusal);
    }
    println!("{}", fake::CODEX_SERVER_OVERLOADED);
    ExitCode::SUCCESS
}

/// The file beside this executable that names the script directory, for a
/// copy oneharness starts with an environment that does not carry
/// [`fake::SCRIPT_DIR_ENV`].
///
/// oneharness from 0.28 starts `codex app-server` as a **shared server**, with
/// the environment cleared down to its base variables and Codex's own, so the
/// variable every other double reads is not there. A journey that puts this
/// binary on `PATH` as `codex` writes the directory here instead.
const SCRIPT_DIR_BESIDE: &str = "onepipeline-fake-dir";

/// Where this double reads its script from and records into: the variable,
/// else the file a journey wrote beside this copy, else refused as unset.
fn script_dir() -> std::path::PathBuf {
    if std::env::var_os(fake::SCRIPT_DIR_ENV).is_none() {
        let beside = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join(SCRIPT_DIR_BESIDE)))
            .and_then(|file| std::fs::read_to_string(file).ok());
        if let Some(dir) = beside.filter(|dir| !dir.trim().is_empty()) {
            return std::path::PathBuf::from(dir.trim());
        }
    }
    fake::script_dir()
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

/// The `-c` settings oneharness starts every Codex launch with, in either state.
const SETTINGS: [&str; 6] = [
    "features.plugins=false",
    "features.plugins=true",
    "features.apps=false",
    "features.apps=true",
    "analytics.enabled=false",
    "analytics.enabled=true",
];

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
            // The config overrides oneharness sends: Codex's plugins, apps and
            // analytics, each off unless the run opted in, and the shared model catalog.
            "-c" => {
                let setting = rest
                    .next()
                    .ok_or("codex's -c takes a setting, and nothing followed it")?;
                if !SETTINGS.contains(&setting) && !setting.starts_with("model_catalog_json=") {
                    return Err(format!("codex takes no setting {setting:?} here"));
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
