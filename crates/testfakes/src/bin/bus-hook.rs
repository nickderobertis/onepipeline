//! A command hook, as an `onemessagebus` configuration's `hooks` block names one.
//!
//! **Not a double for anything in this stack**: the promise is that a command
//! the *host* names decides what is offered to a queue, so what stands here is a
//! real hook. It reads the message off its stdin the way the bus hands it one and
//! answers on stdout, by the argv its configuration names:
//!
//!   `keep`                    print the message back
//!   `drop`                    print nothing
//!   `refuse <reason>`         exit 1 with `<reason>` on stderr
//!   `unjudged`                exit 3
//!   `scripted`                refuse with the text of `bus-hook.refuse` in the
//!                             script directory while that file stands, and
//!                             keep the message once it is gone — a refusal a
//!                             journey can lift without relaunching its run
//!   `set-message <text>`      rewrite `message`, on a surface or a framed reply
//!   `set-blocking`            mark a surface `blocking: true`
//!   `set-kind <kind>`         rewrite a surface's `kind`
//!   `remove-field <name>`     take the top-level field `<name>` off the message
//!   `set-field <name> <json>` set the top-level field `<name>` to the JSON `<json>`
//!   `keep-first-command`      leave only the first of the envelope's commands
//!   `strip-verdict`           take the verdict half off a bare envelope
//!   `rewrite-message <text>`  rewrite the `message` of every command carrying one
//!   `set-author <word>`       rewrite the envelope's `author`
//!   `declare-complete`        add `completion: true` to the envelope
//!   `set-finding-command`     replace the commands with one `finding`
//!   `unreadable-command`      leave a command this build cannot decode
//!   `reroute-as-commands`     rewrite to the `commands` record shape, and reroute
//!                             there
//!   `reroute-as-verdict`      rewrite to a bare verdict, and reroute to `replies`
//!   `reroute-as-surface`      rewrite to a surface, and reroute to `surfaces`
//!   `judge <argv...>`         hand the message to `<argv...>` on its stdin, and
//!                             keep it when that exits 0; any other exit is
//!                             answered as it is, its stderr passed through — so a
//!                             command that only passes or refuses, as
//!                             `onepipeline boundary validate-envelope` does,
//!                             judges a queue as a hook
//!
//! Every invocation is recorded to `bus-hook.jsonl`, with the queue and the
//! message as they arrived.

use std::io::{Read, Write};

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
        "scripted" => {
            if let Ok(reason) = std::fs::read_to_string(dir.join("bus-hook.refuse")) {
                eprint!("{}", reason.trim());
                return std::process::ExitCode::from(1);
            }
            print!("{offered}");
        }
        "set-message" => {
            let target = if message.get("reply").is_some() {
                &mut message["reply"]
            } else {
                &mut message
            };
            let Some(fields) = target.as_object_mut() else {
                fake::fail(&format!("set-message was handed no object: {offered}"));
            };
            fields.insert("message".to_owned(), json!(argument));
            println!("{message}");
        }
        "set-blocking" => {
            let Some(fields) = message.as_object_mut() else {
                fake::fail(&format!("set-blocking was handed no object: {offered}"));
            };
            fields.insert("blocking".to_owned(), json!(true));
            println!("{message}");
        }
        "set-kind" => {
            let Some(fields) = message.as_object_mut() else {
                fake::fail(&format!("set-kind was handed no object: {offered}"));
            };
            fields.insert("kind".to_owned(), json!(argument));
            println!("{message}");
        }
        "remove-field" => {
            let Some(fields) = message.as_object_mut() else {
                fake::fail(&format!("remove-field was handed no object: {offered}"));
            };
            fields.remove(&argument);
            println!("{message}");
        }
        "set-field" => {
            let value: Value = serde_json::from_str(args.get(2).map_or("null", String::as_str))
                .unwrap_or_else(|error| {
                    fake::fail(&format!("set-field's value is not JSON: {error}"))
                });
            let Some(fields) = message.as_object_mut() else {
                fake::fail(&format!("set-field was handed no object: {offered}"));
            };
            fields.insert(argument.clone(), value);
            println!("{message}");
        }
        "keep-first-command" => {
            let commands = commands_of(&mut message, &offered);
            commands.truncate(1);
            println!("{message}");
        }
        "strip-verdict" => {
            let Some(fields) = message.as_object_mut() else {
                fake::fail(&format!("strip-verdict was handed no object: {offered}"));
            };
            for half in ["completion", "message", "reason"] {
                fields.remove(half);
            }
            println!("{message}");
        }
        "rewrite-message" => {
            for command in commands_of(&mut message, &offered).iter_mut() {
                if let Some(said) = command.get_mut("message") {
                    *said = json!(argument);
                }
            }
            println!("{message}");
        }
        "set-author" => {
            let Some(fields) = message.as_object_mut() else {
                fake::fail(&format!("set-author was handed no object: {offered}"));
            };
            fields.insert("author".to_owned(), json!(argument));
            println!("{message}");
        }
        "declare-complete" => {
            let Some(fields) = message.as_object_mut() else {
                fake::fail(&format!("declare-complete was handed no object: {offered}"));
            };
            fields.insert("completion".to_owned(), json!(true));
            println!("{message}");
        }
        "set-finding-command" => {
            let Some(fields) = message.as_object_mut() else {
                fake::fail(&format!(
                    "set-finding-command was handed no object: {offered}"
                ));
            };
            fields.insert("version".to_owned(), json!(3));
            fields.insert(
                "commands".to_owned(),
                json!([{"op": "finding", "message": "added by a hook"}]),
            );
            println!("{message}");
        }
        "unreadable-command" => {
            *commands_of(&mut message, &offered) =
                vec![json!({"op": "finding", "no-such-field": true})];
            println!("{message}");
        }
        "reroute-as-commands" => {
            let record = json!({"id": 0, "commands": message.get("commands").cloned()});
            route_to("commands");
            println!("{record}");
        }
        "reroute-as-verdict" => {
            route_to("replies");
            println!("{}", json!({"completion": false, "reason": "rerouted"}));
        }
        "reroute-as-surface" => {
            route_to("surfaces");
            println!(
                "{}",
                json!({"id": 0, "kind": "finding", "message": "rerouted to the planner",
                       "source": "proposal", "blocking": false, "queued_at": 1})
            );
        }
        "judge" => return judged(&args[1..], &offered),
        other => fake::fail(&format!("bus-hook does not know `{other}`")),
    }
    std::process::ExitCode::SUCCESS
}

/// Run `argv` over `offered`: a keep when it exits 0, and its own exit otherwise,
/// its stderr having gone straight through as the refusal's reason.
fn judged(argv: &[String], offered: &str) -> std::process::ExitCode {
    let Some((program, rest)) = argv.split_first() else {
        fake::fail("judge names no command to run");
    };
    let mut child = match std::process::Command::new(program)
        .args(rest)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => fake::fail(&format!("cannot run {program}: {error}")),
    };
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(error) = stdin.write_all(offered.as_bytes()) {
            fake::fail(&format!("cannot hand {program} the message: {error}"));
        }
    }
    match child.wait().map(|status| status.code()) {
        Ok(Some(0)) => {
            print!("{offered}");
            std::process::ExitCode::SUCCESS
        }
        Ok(Some(code)) => std::process::ExitCode::from(u8::try_from(code).unwrap_or(u8::MAX)),
        Ok(None) => fake::fail(&format!("{program} was ended by a signal")),
        Err(error) => fake::fail(&format!("cannot wait for {program}: {error}")),
    }
}

fn commands_of<'a>(message: &'a mut Value, offered: &str) -> &'a mut Vec<Value> {
    match message.get_mut("commands").and_then(Value::as_array_mut) {
        Some(commands) => commands,
        None => fake::fail(&format!("the message carries no commands: {offered}")),
    }
}

fn route_to(queue: &str) {
    let route = match std::env::var(ROUTE_ENV) {
        Ok(route) => route,
        Err(error) => fake::fail(&format!("the bus offered no route file: {error}")),
    };
    if let Err(error) = std::fs::write(&route, queue) {
        fake::fail(&format!("cannot write the route file: {error}"));
    }
}
