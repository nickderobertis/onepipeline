//! A node validator, as `onepipeline start --node-validator` names one.
//!
//! **Not a double for anything in this stack.** The hook's whole promise is that
//! a command the *host* names is run, and the host's own is hundreds of lines of
//! rules over documents this crate has never seen. So what stands here is a real
//! validator: it reads the node off its stdin the way the contract says one
//! does, decides, and answers with an exit status and its own words on stderr.
//! A stub inside the crate under test would prove none of that.
//!
//! It is scripted from the same directory the sibling doubles are, so a journey
//! states what the host's rules say the way it states everything else:
//!
//!   `validator.refuse`   present → refuse every node, with this file's text on
//!                        stderr; absent → accept
//!   `validator.silent`   present → refuse without reading stdin and without
//!                        saying anything, which is the answer a caller still
//!                        has to be able to act on
//!   `validator.chatter`  present → write this file's text to **stdout** before
//!                        answering, the way a host's rules engine narrates what
//!                        it checked
//!
//! Every invocation is recorded to `validator.jsonl`, each line carrying the
//! **name the validator was invoked as** and the node it was given — so a
//! journey about which of three names a launch resolved reads the answer off the
//! program that actually ran.

use std::io::Read;

use onepipeline_testfakes as fake;

fn main() -> std::process::ExitCode {
    let dir = fake::script_dir();

    // The name this copy of the program was invoked as, which is the command
    // the launch resolved. Three names for one program is how a journey tells
    // the flag's validator from the environment's and from the config's.
    let invoked_as = std::env::args()
        .next()
        .map(|argv0| {
            std::path::Path::new(&argv0)
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or(argv0)
        })
        .unwrap_or_else(|| "unknown".to_string());

    // A validator that refuses without reading its input is answering rather
    // than failing, and what decides the edit is the status below — so this one
    // exits with stdin still unread, deliberately.
    if dir.join("validator.silent").is_file() {
        fake::append(
            &dir.join("validator.jsonl"),
            &serde_json::json!({"as": invoked_as, "node": serde_json::Value::Null}).to_string(),
        );
        return std::process::ExitCode::from(3);
    }

    let mut document = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut document) {
        fake::fail(&format!("cannot read the node on stdin: {error}"));
    }
    let node: serde_json::Value = match serde_json::from_str(&document) {
        Ok(node) => node,
        Err(error) => {
            // The contract says a node crosses as JSON. One that does not is a
            // seam that broke, and a validator that accepted it anyway would let
            // the journey pass on a node nothing checked.
            eprintln!("the node did not cross as JSON: {error}: {document}");
            return std::process::ExitCode::from(1);
        }
    };
    fake::append(
        &dir.join("validator.jsonl"),
        &serde_json::json!({"as": invoked_as, "node": node}).to_string(),
    );

    // A validator that narrates on stdout is ordinary — a host's rules engine
    // prints what it checked — and none of it is the caller's answer.
    if let Ok(chatter) = std::fs::read_to_string(dir.join("validator.chatter")) {
        println!("{}", chatter.trim());
    }

    match std::fs::read_to_string(dir.join("validator.refuse")) {
        Ok(reason) => {
            eprintln!("{}", reason.trim());
            std::process::ExitCode::from(1)
        }
        Err(_) => std::process::ExitCode::SUCCESS,
    }
}
