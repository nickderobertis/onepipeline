//! A change request's review feedback, read through the linked `onevcs`, as
//! `review-read URL`.
//!
//! **Not a double for anything in this stack**: it makes the one call `onevcs
//! change comments URL` makes — [`onevcs::review_comments`] on the real providers —
//! and prints the comments it answers as one JSON array. What it reads is GitHub,
//! which in a journey is `fake-gh` at the library's own `ONEVCS_GH` seam, so a
//! journey asking where a review thread sits asks the question exactly as a
//! consumer of that library does. A program rather than an in-process call because
//! the seam is an environment variable, and a test process runs its journeys side
//! by side under one environment.

// llmlint: ignore-file[new_code_lands_in_a_project] `crates/testfakes` has no `project.json`
// of its own, like every double beside this file: its files are inputs of the
// `onepipeline-note-journeys` project (`noteJourneySource` lists `crates/**/*` in `nx.json`)
// and of the root project's `visualDocsSource`, and the crate is a member of the one Cargo
// workspace those projects build. It ships nowhere, and a project of its own would restate
// their targets over the same build unit.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [url] = args.as_slice() else {
        eprintln!("usage: review-read URL");
        return ExitCode::from(2);
    };
    let change = onevcs::ChangeRef::Url(url.clone());
    match onevcs::review_comments(&onevcs::Providers::real(), &change, None) {
        Ok(read) => match serde_json::to_string(&read.comments) {
            Ok(comments) => {
                println!("{comments}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("review-read: {error}");
                ExitCode::from(1)
            }
        },
        Err(error) => {
            eprintln!("review-read: {error}");
            ExitCode::from(1)
        }
    }
}
