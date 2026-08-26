//! A `onetaskgraph` executable that acts out an install answering badly.
//!
//! **Not a double for the store.** Every plan in this suite is read out of a
//! real `onetaskgraph`, against a real folder of Markdown, because a plan read
//! through a stand-in would prove the stand-in. What this stands in for is the
//! one thing a real binary cannot be asked to be: an install of the *wrong
//! version*, one that cannot answer at all, and one whose answers this build
//! has to refuse.
//!
//! Every journey that reaches it asserts a **refusal**, so it can never make a
//! plan read look like it worked: what it is for is the branches on the other
//! side of "the store answered something this build will not act on", which no
//! correct binary reaches and no fixture inside the crate under test could
//! reach either.
//!
//! Scripted from the directory every double reads its scenario out of:
//!
//! * `onetaskgraph.version` — what `--version` prints, verbatim. Absent, it
//!   prints nothing at all, which is an install this build cannot read a version
//!   off.
//! * `onetaskgraph.refuse` — makes **every** invocation exit 1, saying the file's
//!   contents on stderr. An install that is broken is not broken per verb.
//! * `onetaskgraph.refuse-reads` — the same, for every invocation **except**
//!   `--version`: an install that says what it is and then cannot answer a query,
//!   which is the one shape that gets past the launch's version check.
//! * `onetaskgraph.<verb>` — what a read answers with on stdout, verbatim, where
//!   `<verb>` is the command line's words joined by `-`: `project-show`,
//!   `task-list`, `task-deps`. A read nothing scripts is refused, because a
//!   double that answered a query no journey stated would be inventing a store.
//! * `onetaskgraph.<verb>.2` — what the **second** call to that verb answers,
//!   and `.3` the third, so a journey can state a walk of several pages.

use onepipeline_testfakes as fake;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = fake::script_dir();
    fake::record(&dir, "onetaskgraph", &args);

    if let Ok(reason) = std::fs::read_to_string(dir.join("onetaskgraph.refuse")) {
        eprintln!("{}", reason.trim());
        return ExitCode::from(1);
    }
    if args == ["--version"] {
        let printed = std::fs::read_to_string(dir.join("onetaskgraph.version")).unwrap_or_default();
        print!("{printed}");
        return ExitCode::SUCCESS;
    }
    if let Ok(reason) = std::fs::read_to_string(dir.join("onetaskgraph.refuse-reads")) {
        eprintln!("{}", reason.trim());
        return ExitCode::from(1);
    }

    // The verb is the leading words that are not flags or their values, which is
    // how this binary's own surface is shaped: `project show ID --json`.
    let verb: Vec<&str> = args
        .iter()
        .take_while(|word| !word.starts_with("--"))
        .take(2)
        .map(String::as_str)
        .collect();
    if verb.len() < 2 {
        return fake::refuse(&format!("this double speaks no such command: {args:?}"));
    }
    let name = format!("onetaskgraph.{}", verb.join("-"));

    // Which call of this verb it is, so a journey can state a walk of pages.
    let nth = fake::count(&dir, &name);
    let answer = (nth > 1)
        .then(|| std::fs::read_to_string(dir.join(format!("{name}.{nth}"))).ok())
        .flatten()
        .or_else(|| std::fs::read_to_string(dir.join(&name)).ok());
    match answer {
        Some(scripted) => {
            print!("{scripted}");
            ExitCode::SUCCESS
        }
        None => fake::refuse(&format!(
            "no scenario scripts `{name}`, so this double has no store to answer for"
        )),
    }
}
