//! A command validator, as an `onemessagebus` configuration's `validators` block
//! names one.
//!
//! **Not a double for anything in this stack**, for the reason `node-validator`
//! is not: the promise is that a command the *host* names judges what is offered
//! to a queue, so what stands here is a real validator. It reads the message off
//! its stdin the way the bus hands it one — one JSON document, the queue named in
//! `ONEMESSAGEBUS_VALIDATE_QUEUE` — and answers with an exit status and its own
//! words on stderr.
//!
//! It is scripted from the same directory the sibling doubles are:
//!
//!   `bus-validator.refuse`    present → refuse every message, writing this
//!                             file's text on stderr and exiting 1
//!   `bus-validator.unjudged`  present → exit 3, which is neither a pass nor a
//!                             refusal
//!
//! Absent both, it passes. Every invocation is recorded to `bus-validator.jsonl`,
//! carrying the queue and the message as they arrived, which is the only witness
//! there is to what crossed the stdin.

use std::io::Read;

use onepipeline_testfakes as fake;

/// The variable the bus names the queue a message is offered to in.
const QUEUE_ENV: &str = "ONEMESSAGEBUS_VALIDATE_QUEUE";

fn main() -> std::process::ExitCode {
    let dir = fake::script_dir();
    let mut offered = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut offered) {
        fake::fail(&format!("cannot read the message on stdin: {error}"));
    }
    let message: serde_json::Value = match serde_json::from_str(&offered) {
        Ok(message) => message,
        Err(error) => fake::fail(&format!(
            "the message did not cross as one JSON document: {error}: {offered}"
        )),
    };
    let queue = match std::env::var(QUEUE_ENV) {
        Ok(queue) if !queue.trim().is_empty() => queue,
        Ok(_) => fake::fail(&format!(
            "the bus named no queue this message is offered to: {QUEUE_ENV} is blank"
        )),
        Err(error) => fake::fail(&format!(
            "the bus did not name the queue this message is offered to in {QUEUE_ENV}: {error}"
        )),
    };
    fake::append(
        &dir.join("bus-validator.jsonl"),
        &serde_json::json!({
            "queue": queue,
            "message": message,
        })
        .to_string(),
    );
    if dir.join("bus-validator.unjudged").is_file() {
        return std::process::ExitCode::from(3);
    }
    match std::fs::read_to_string(dir.join("bus-validator.refuse")) {
        Ok(reason) => {
            eprintln!("{}", reason.trim());
            std::process::ExitCode::from(1)
        }
        Err(_) => std::process::ExitCode::SUCCESS,
    }
}
