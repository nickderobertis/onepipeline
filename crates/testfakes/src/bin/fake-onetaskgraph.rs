//! A `onetaskgraph` executable that answers `--version` and nothing else.
//!
//! **Not a double for the store.** Every plan in this suite is read out of a
//! real `onetaskgraph`, against a real folder of Markdown, because a plan read
//! through a stand-in would prove the stand-in. What this stands in for is the
//! one thing a real binary cannot be asked to be: an install of the *wrong
//! version*, and one that cannot answer at all.
//!
//! So it speaks exactly `--version`, scripted from the directory every double
//! reads its scenario out of:
//!
//! * `onetaskgraph.version` — what it prints, verbatim. Absent, it prints
//!   nothing at all, which is an install this build cannot read a version off.
//! * `onetaskgraph.refuse` — its exit code for `--version`, when it is to refuse
//!   one. The file's contents are what it says on stderr.
//!
//! Anything else is refused, argument by argument, for the reason every double
//! here refuses: one that shrugged off a verb the real binary has never had
//! would let this crate reach a real install with it and stay green.

use onepipeline_testfakes as fake;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = fake::script_dir();
    fake::record(&dir, "onetaskgraph", &args);

    if args != ["--version"] {
        return fake::refuse(&format!(
            "this double answers `--version` only, and was asked for {args:?}"
        ));
    }
    if let Ok(reason) = std::fs::read_to_string(dir.join("onetaskgraph.refuse")) {
        eprintln!("{}", reason.trim());
        return ExitCode::from(1);
    }
    let printed = std::fs::read_to_string(dir.join("onetaskgraph.version")).unwrap_or_default();
    print!("{printed}");
    ExitCode::SUCCESS
}
