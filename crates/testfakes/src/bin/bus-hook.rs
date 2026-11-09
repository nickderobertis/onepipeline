//! A command hook, as an `onemessagebus` configuration's `hooks` block names one.
//!
//! **Not a double for anything in this stack**, for the reason `bus-validator` is
//! not: the promise is that a command the *host* names decides what is offered to
//! a queue, so what stands here is a real hook. It reads the message off its stdin
//! the way the bus hands it one and answers on stdout, by the argv its configuration
//! names:
//!
//!   `keep`                    print the message back
//!   `drop`                    print nothing
//!   `refuse <reason>`         exit 1 with `<reason>` on stderr
//!   `unjudged`                exit 3
//!   `set-message <text>`      rewrite `message`, on a surface or a framed reply
//!   `keep-first-command`      leave only the first of the envelope's commands
//!   `rewrite-note <text>`     rewrite the `text` of every command carrying one
//!   `reroute-as-commands`     rewrite to the `commands` record shape, and reroute
//!                             there
//!
//! Every invocation is recorded to `bus-hook.jsonl`, with the queue and the
//! message as they arrived.

use std::io::Read;

use onepipeline_testfakes as fake;
use serde_json::{json, Value};

/// The variable the bus names the queue in.
const QUEUE_ENV: &str = "ONEMESSAGEBUS_HOOK_QUEUE";
/// The variable naming the file a route is written to.
const ROUTE_ENV: &str = "ONEMESSAGEBUS_HOOK_ROUTE";

fn main() -> std::process::ExitCode {
    let dir = fake::script_dir();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut offered = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut offered) {
        fake::fail(&format!("cannot read the message on stdin: {error}"));
    }
    let mut message: Value = match serde_json::from_str(&offered) {
        Ok(message) => message,
        Err(error) => fake::fail(&format!(
            "the message did not cross as one JSON document: {error}: {offered}"
        )),
    };
    let queue = std::env::var(QUEUE_ENV).unwrap_or_default();
    fake::append(
        &dir.join("bus-hook.jsonl"),
        &json!({"queue": queue, "message": message}).to_string(),
    );
    let verb = args.first().map(String::as_str).unwrap_or_default();
    let argument = args.get(1).cloned().unwrap_or_default();
    match verb {
        "keep" => print!("{offered}"),
        "drop" => {}
        "refuse" => {
            eprint!("{argument}");
            return std::process::ExitCode::from(1);
        }
        "unjudged" => return std::process::ExitCode::from(3),
        "set-message" => {
            let target = if message.get("reply").is_some() {
                &mut message["reply"]
            } else {
                &mut message
            };
            target["message"] = json!(argument);
            println!("{message}");
        }
        "keep-first-command" => {
            let kept = message["commands"][0].clone();
            message["commands"] = json!([kept]);
            println!("{message}");
        }
        "rewrite-note" => {
            for command in message["commands"].as_array_mut().into_iter().flatten() {
                if command.get("message").is_some() {
                    command["message"] = json!(argument);
                }
            }
            println!("{message}");
        }
        "reroute-as-commands" => {
            let record = json!({"id": 0, "commands": message["commands"]});
            let route = match std::env::var(ROUTE_ENV) {
                Ok(route) => route,
                Err(error) => fake::fail(&format!("the bus offered no route file: {error}")),
            };
            if let Err(error) = std::fs::write(&route, "commands") {
                fake::fail(&format!("cannot write the route file: {error}"));
            }
            println!("{record}");
        }
        other => fake::fail(&format!("bus-hook does not know `{other}`")),
    }
    std::process::ExitCode::SUCCESS
}
