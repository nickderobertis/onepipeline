//! A live `onevcs` session that carries no labels, as `session-holder REPO`.
//!
//! **Not a double for anything in this stack**: it opens a real session through
//! the linked `onevcs` library — the same call its own `session open` verb makes —
//! and then stays alive, so the record's owner is a running process. That is the
//! shape a session opened by hand, or by an engine that predates the `run`/`node`
//! labels, has while its owner works, and the one `onevcs session open` cannot
//! leave behind: that verb exits as soon as it prints, so its record answers
//! stale from that instant.
//!
//! Prints the token the library minted once the session is open, then holds until its stdin closes.

use std::io::{Read, Write};
use std::process::ExitCode;

use onevcs::Vcs;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [repo] = args.as_slice() else {
        eprintln!("usage: session-holder REPO");
        return ExitCode::from(2);
    };
    let request: onevcs::SessionRequest = match serde_json::from_value(serde_json::json!({
        "repo": repo,
        // Minted by the library as it opens the session, whatever a request says.
        "token": "",
        "labels": {},
    })) {
        Ok(request) => request,
        Err(error) => {
            eprintln!("session-holder: {error}");
            return ExitCode::from(2);
        }
    };
    match onevcs::Git.open_session(request) {
        Ok(session) => {
            let mut stdout = std::io::stdout();
            let held = writeln!(stdout, "{}", session.token.0)
                .and_then(|()| stdout.flush())
                .and_then(|()| std::io::stdin().read_to_end(&mut Vec::new()));
            match held {
                Ok(_) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("session-holder: {error}");
                    ExitCode::from(1)
                }
            }
        }
        Err(error) => {
            eprintln!("session-holder: {error}");
            ExitCode::from(1)
        }
    }
}
