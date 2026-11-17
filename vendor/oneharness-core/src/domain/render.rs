//! Text renderings for a person reading along: one line (or a few) per
//! normalized event, and the per-run views `oneharness run --format text` and
//! `oneharness history show --format text` print. Pure: no I/O.
//!
//! These are public so an embedder prints exactly what the CLI prints — the
//! `oneharness` binary calls them rather than keeping copies of its own. Every
//! string a harness wrote passes through [`printable`] on its way out, so no
//! rendering can carry an ANSI escape, a carriage return or a bell to the
//! reader's terminal.

use serde_json::Value;

use crate::domain::events::{ActionEvent, ToolCallStatus};
use crate::domain::report::{RunReport, RunResult, Status};

/// `text` with every control character except the newline flattened to a
/// space, for a text view.
///
/// Harness output reaches the text views verbatim — a result's `text`, its
/// `error`, a command an agent ran — and an ANSI escape, a carriage return or
/// a bell inside it could move the cursor, recolour, or overwrite part of a
/// view whose whole point is to be read at a glance. The JSON contract carries
/// the bytes as they were; the text view is the one that draws them, so it is
/// the one that flattens. Newlines survive because a multi-line value is laid
/// out by the renderer, one row per line.
#[must_use]
pub fn printable(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() && c != '\n' { ' ' } else { c })
        .collect()
}

/// `text` as an indented block: every line prefixed with `indent`, each
/// terminated, so a multi-line value sits under its label rather than beside
/// it. Flattened through [`printable`] on the way.
#[must_use]
pub fn indented(text: &str, indent: &str) -> String {
    let mut out = String::new();
    for line in printable(text).lines() {
        out.push_str(indent);
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn or_null(value: Option<&str>) -> String {
    value.map_or_else(|| "null".to_string(), printable)
}

const MESSAGE_MARK: &str = "› ";
/// The prefix of a `reasoning` line — a label rather than a glyph, so it cannot
/// be mistaken for the agent's own text.
const REASONING_MARK: &str = "(thinking) ";
const COMMAND_MARK: &str = "$ ";
const FILE_MARK: &str = "✎ ";
const TOOL_MARK: &str = "▸ ";
const FAILED_MARK: &str = "✗ ";
/// The widest a tool call's argument summary is drawn before it is cut.
const SUMMARY_MAX_CHARS: usize = 120;

/// The readable form of one normalized event, as `run --stream --format text`
/// and `history watch --format text` print it: plain text with no ANSI escapes
/// and no trailing newline, or `None` for an event this view deliberately does
/// not draw.
///
/// - `message` → `› <text>`; `reasoning` → `(thinking) <text>`. A multi-line
///   text continues on lines indented to sit under the first.
/// - A tool call that ran a shell command → `$ <command>`, with the harness's
///   shell wrapper (`/bin/zsh -lc '…'`, `bash -lc '…'`) removed.
/// - A file change → `✎ <path>`, one line per path.
/// - Any other tool call → `▸ <name> <its main argument>`.
/// - A failed call is marked `✗` with its exit code when the harness reported
///   one (`✗ exit 2`), else `✗ failed`, followed by the first line of its
///   output; a call cut short is marked `✗ timed out` / `✗ interrupted`.
/// - A `tool_result` is `None`: the call it answers was already drawn, and the
///   observation is the call's output, not a line of its own — unless the
///   result reports the call `failed` (a Claude Code `is_error` block), which
///   the already-drawn call could not show: then one `✗ <name>  failed` line,
///   naming the tool the call named (`tool` when unknown).
#[must_use]
pub fn render_event(event: &ActionEvent) -> Option<String> {
    match event.kind.as_str() {
        "message" => text_lines(MESSAGE_MARK, event.output.as_deref()?),
        "reasoning" => text_lines(REASONING_MARK, event.output.as_deref()?),
        "tool_call" => Some(render_tool_call(event)),
        "tool_result" if event.status == Some(ToolCallStatus::Failed) => Some(format!(
            "{FAILED_MARK}{}  failed",
            printable(event.name.as_deref().unwrap_or("tool"))
        )),
        _ => None,
    }
}

/// `text` under `mark`, continuation lines indented to sit under the first;
/// `None` for text with nothing to read.
fn text_lines(mark: &str, text: &str) -> Option<String> {
    let text = printable(text);
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let continuation = " ".repeat(mark.chars().count());
    let mut out = String::new();
    for (i, line) in text.lines().enumerate() {
        if i == 0 {
            out.push_str(mark);
        } else {
            out.push('\n');
            if !line.is_empty() {
                out.push_str(&continuation);
            }
        }
        out.push_str(line.trim_end());
    }
    Some(out)
}

fn render_tool_call(event: &ActionEvent) -> String {
    let input = event.input.as_ref();
    let body = if let Some(command) = input
        .and_then(|input| input.get("command"))
        .and_then(command_text)
    {
        text_lines(COMMAND_MARK, &command).unwrap_or_else(|| COMMAND_MARK.trim_end().to_string())
    } else if let Some(paths) = changed_paths(event).filter(|paths| !paths.is_empty()) {
        paths
            .iter()
            .map(|path| format!("{FILE_MARK}{}", printable(path)))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        let name = printable(event.name.as_deref().unwrap_or("tool"));
        match input.and_then(argument_summary) {
            Some(summary) => format!("{TOOL_MARK}{name} {summary}"),
            None => format!("{TOOL_MARK}{name}"),
        }
    };
    match outcome_mark(event) {
        Some(mark) => format!("{body} {mark}"),
        None => body,
    }
}

/// The `✗ …` suffix of a call that did not complete, or `None` for one that
/// did (or whose outcome is not known yet).
fn outcome_mark(event: &ActionEvent) -> Option<String> {
    let exit_code = event
        .input
        .as_ref()
        .and_then(|input| input.get("exit_code"))
        .and_then(Value::as_i64);
    match event.status {
        Some(ToolCallStatus::Timeout) => return Some("✗ timed out".to_string()),
        Some(ToolCallStatus::Interrupted) => return Some("✗ interrupted".to_string()),
        Some(ToolCallStatus::Failed) => {}
        Some(ToolCallStatus::Completed) | None if exit_code.is_some_and(|code| code != 0) => {}
        Some(ToolCallStatus::Completed) | None => return None,
    }
    let mut mark = match exit_code {
        Some(code) => format!("✗ exit {code}"),
        None => "✗ failed".to_string(),
    };
    if let Some(first) = event.output.as_deref().map(printable).and_then(|output| {
        output
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_string)
    }) {
        mark.push_str(": ");
        mark.push_str(&first);
    }
    Some(mark)
}

/// The command a tool call ran, with its shell wrapper removed: a string, or
/// an argv array (`["bash", "-lc", "…"]`).
fn command_text(command: &Value) -> Option<String> {
    match command {
        Value::String(text) => Some(strip_shell_wrapper(text)),
        Value::Array(words) => {
            let words: Vec<&str> = words.iter().map(Value::as_str).collect::<Option<_>>()?;
            if let [shell, flag, script] = words.as_slice() {
                if is_shell(shell) && is_command_flag(flag) {
                    return Some((*script).to_string());
                }
            }
            Some(words.join(" "))
        }
        _ => None,
    }
}

/// `/bin/zsh -lc 'cat x'` → `cat x`: a POSIX shell (or PowerShell) invoked
/// with a command flag and exactly one quoted script word is the harness's
/// wrapper, not what the agent asked to run. Anything else is returned as it
/// was, since a guess at unwrapping would misreport the command.
fn strip_shell_wrapper(command: &str) -> String {
    let trimmed = command.trim();
    let mut rest = trimmed;
    let Some((shell, after)) = split_word(rest) else {
        return command.to_string();
    };
    if !is_shell(&shell) {
        return command.to_string();
    }
    rest = after;
    let mut saw_command_flag = false;
    loop {
        let Some((word, after)) = split_word(rest) else {
            return command.to_string();
        };
        if !word.starts_with('-') {
            break;
        }
        if is_command_flag(&word) {
            saw_command_flag = true;
            rest = after;
            break;
        }
        if !matches!(
            word.to_ascii_lowercase().as_str(),
            "-l" | "--login" | "-noprofile" | "-nologo" | "-noninteractive"
        ) {
            return command.to_string();
        }
        rest = after;
    }
    if !saw_command_flag {
        return command.to_string();
    }
    match split_word(rest) {
        Some((script, after)) if after.trim().is_empty() => script,
        _ => command.to_string(),
    }
}

fn is_shell(word: &str) -> bool {
    let base = word
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(word)
        .to_ascii_lowercase();
    let base = base.strip_suffix(".exe").unwrap_or(&base);
    matches!(
        base,
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "fish" | "pwsh" | "powershell"
    )
}

fn is_command_flag(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "-c" | "-lc" | "-cl" | "-command"
    )
}

/// The first shell word of `text` with its quoting removed, and what follows
/// it; `None` when the text is empty or a quote is left open.
fn split_word(text: &str) -> Option<(String, &str)> {
    let text = text.trim_start();
    if text.is_empty() {
        return None;
    }
    let mut word = String::new();
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        match c {
            c if c.is_whitespace() => return Some((word, &text[at..])),
            '\'' => loop {
                match chars.next()? {
                    (_, '\'') => break,
                    (_, c) => word.push(c),
                }
            },
            '"' => loop {
                match chars.next()? {
                    (_, '"') => break,
                    (_, '\\') => match chars.next()? {
                        (_, c @ ('"' | '\\' | '$' | '`')) => word.push(c),
                        (_, c) => {
                            word.push('\\');
                            word.push(c);
                        }
                    },
                    (_, c) => word.push(c),
                }
            },
            '\\' => word.push(chars.next()?.1),
            c => word.push(c),
        }
    }
    Some((word, ""))
}

/// The paths a file-changing call touched: codex's `changes` list, or the one
/// path an edit/write tool names. `None` for a call that changes no file.
fn changed_paths(event: &ActionEvent) -> Option<Vec<String>> {
    let input = event.input.as_ref()?;
    if let Some(changes) = input.get("changes").and_then(Value::as_array) {
        return Some(
            changes
                .iter()
                .filter_map(|change| change.get("path").and_then(Value::as_str))
                .map(str::to_string)
                .collect(),
        );
    }
    let edits_a_file = matches!(
        event.name.as_deref()?,
        "Edit"
            | "Write"
            | "MultiEdit"
            | "NotebookEdit"
            | "edit"
            | "write"
            | "patch"
            | "apply_patch"
            | "replace"
            | "write_file"
            | "edit_file"
    );
    if !edits_a_file {
        return None;
    }
    ["file_path", "filePath", "path", "notebook_path"]
        .iter()
        .find_map(|key| input.get(*key).and_then(Value::as_str))
        .map(|path| vec![path.to_string()])
}

/// One argument that says what a call was about — the path it read, the
/// pattern it searched — else the whole input, compact and cut to width.
fn argument_summary(input: &Value) -> Option<String> {
    const KEYS: [&str; 9] = [
        "file_path",
        "filePath",
        "path",
        "notebook_path",
        "pattern",
        "query",
        "url",
        "description",
        "prompt",
    ];
    let summary = KEYS
        .iter()
        .find_map(|key| input.get(*key).and_then(Value::as_str).map(str::to_string))
        .or_else(|| match input {
            Value::Null => None,
            Value::Object(map) if map.is_empty() => None,
            other => Some(other.to_string()),
        })?;
    let flat = printable(&summary).replace('\n', " ");
    Some(if flat.chars().count() > SUMMARY_MAX_CHARS {
        let cut: String = flat.chars().take(SUMMARY_MAX_CHARS - 1).collect();
        format!("{cut}…")
    } else {
        flat
    })
}

/// The `run` report for a person at a terminal — what `oneharness run --format
/// text` prints: the run's own settings first (what
/// was asked, the mode, the session handle, the batch/fallback blocks), then
/// one block per result. Every value is the JSON's own — a `null` there is said
/// to be null here, with the `text_source` that explains it — and every string
/// a harness wrote is flattened before it is drawn.
pub fn render_report_text(report: &RunReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "prompt: {}\n",
        printable(first_line(&report.prompt))
    ));
    out.push_str(&format!(
        "mode: {}{}\n",
        report.permission_mode.as_str(),
        if report.dry_run { " · dry run" } else { "" }
    ));
    if let Some(models) = &report.models {
        out.push_str(&format!("models: {}\n", printable(&models.join(", "))));
    } else if let Some(model) = &report.model {
        out.push_str(&format!("model: {}\n", printable(model)));
    }
    if let Some(resume) = &report.resume {
        out.push_str(&format!(
            "resume: {}{}\n",
            printable(resume),
            if report.fork { " (forked)" } else { "" }
        ));
    }
    if let Some(session) = &report.session {
        out.push_str(&format!(
            "session: {} ({}) · token {} · store {}\n",
            printable(&session.name),
            session.phase.as_str(),
            or_null(session.token.as_deref()),
            or_null(session.store_file.as_deref()),
        ));
    }
    if let Some(batch) = &report.batch {
        out.push_str(&format!(
            "batch: {} · {} prompts · forked {}\n",
            batch.strategy.as_str(),
            batch.prompt_count,
            if batch.forked { "yes" } else { "no" }
        ));
    }
    if report.schema.is_some() {
        out.push_str(&format!(
            "schema: applied · max retries {}\n",
            report
                .schema_max_retries
                .map_or_else(|| "null".to_string(), |n| n.to_string())
        ));
    }
    if let Some(fallback) = &report.fallback {
        out.push_str(&format!(
            "fallback: ran {}{}\n",
            or_null(fallback.ran.as_deref()),
            if fallback.stopped_without_work {
                " (stopped without work evidence)"
            } else {
                ""
            }
        ));
        for fell in &fallback.fell_through {
            out.push_str(&format!(
                "  fell through {}: {}{}\n",
                printable(&fell.harness),
                fell.reason.as_str(),
                fell.detail
                    .as_deref()
                    .map_or_else(String::new, |d| format!(" — {}", printable(d)))
            ));
        }
    }
    if let Some(control) = &report.control {
        out.push_str(&format!(
            "control: {} · socket {} · {} interrupt{}\n",
            control.mechanism.as_str(),
            printable(&control.socket.to_string()),
            control.interrupts.len(),
            if control.interrupts.len() == 1 {
                ""
            } else {
                "s"
            }
        ));
    }
    if let Some(file) = &report.history_file {
        out.push_str(&format!("history: {}\n", printable(file)));
    }
    if let Some(file) = &report.spy_file {
        out.push_str(&format!("spy log: {}\n", printable(file)));
    }
    for result in &report.results {
        out.push('\n');
        out.push_str(&render_result(result, report.batch.is_some()));
    }
    out
}

/// One result block: the candidate and its envelope on the first line, then
/// what it produced and, when it failed, why.
fn render_result(result: &RunResult, batch: bool) -> String {
    let mut out = format!(
        "{candidate}{model}: {status} · exit {exit} · {duration}\n",
        candidate = printable(&result.harness_id),
        model = result
            .model
            .as_deref()
            .map_or_else(String::new, |m| format!(" [model {}]", printable(m))),
        status = result.status.as_str(),
        exit = result
            .exit_code
            .map_or_else(|| "null".to_string(), |c| c.to_string()),
        duration = result
            .duration_ms
            .map_or_else(|| "not run".to_string(), |ms| format!("{ms} ms")),
    );
    if let Some(observed) = &result.observed_model {
        out.push_str(&format!("  observed model: {}\n", printable(observed)));
    }
    if batch {
        out.push_str(&format!(
            "  prompt: {}\n",
            or_null(result.prompt.as_deref().map(first_line))
        ));
    }
    if result.status == Status::Planned {
        out.push_str(&format!(
            "  command: {}\n",
            printable(&shell_words(&result.command))
        ));
    }
    match &result.text {
        Some(text) => {
            out.push_str(&format!(
                "  text ({}):\n",
                or_null(result.text_source.as_deref())
            ));
            out.push_str(&indented(text, "    "));
        }
        None => out.push_str(&format!(
            "  text: null ({})\n",
            result
                .text_source
                .as_deref()
                .map_or_else(|| text_absence(result), printable)
        )),
    }
    if result.schema_valid.is_some() || result.structured.is_some() {
        out.push_str(&format!(
            "  structured: {} · attempts {}\n",
            match result.schema_valid {
                Some(true) => "valid",
                Some(false) => "invalid",
                None => "not validated",
            },
            result
                .schema_attempts
                .map_or_else(|| "null".to_string(), |n| n.to_string())
        ));
        if let Some(value) = &result.structured {
            out.push_str(&indented(
                &serde_json::to_string_pretty(value).unwrap_or_default(),
                "    ",
            ));
        } else {
            out.push_str("    null (no JSON value could be extracted)\n");
        }
        if let Some(error) = &result.schema_error {
            out.push_str(&format!("  schema error: {}\n", printable(error)));
        }
    }
    if let Some(kind) = result.failure_kind {
        out.push_str(&format!(
            "  failure: {}{}\n",
            kind.as_str(),
            result
                .failure_kind_source
                .as_deref()
                .map_or_else(String::new, |s| format!(" (from {})", printable(s)))
        ));
    }
    if let Some(work) = result.work {
        out.push_str(&format!("  work evidence: {}\n", work.as_str()));
    }
    if let Some(error) = &result.error {
        out.push_str(&format!("  error: {}\n", printable(error)));
    }
    if let Some(id) = &result.session_id {
        out.push_str(&format!("  session id: {}\n", printable(id)));
    }
    if result.usage_source.is_some() {
        out.push_str(&format!(
            "  usage: in {} · out {} · cache read {} · cache write {} · cost {}\n",
            count(result.usage.input_tokens),
            count(result.usage.output_tokens),
            count(result.usage.cache_read_tokens),
            count(result.usage.cache_write_tokens),
            result
                .usage
                .cost_usd
                .map_or_else(|| "null".to_string(), |c| format!("${c}")),
        ));
    }
    if let Some(events) = &result.events {
        out.push_str(&format!(
            "  events: {} ({})\n",
            events.len(),
            or_null(result.events_source.as_deref())
        ));
    }
    out
}

/// Why a result carries no `text` when it also carries no `text_source`: the
/// envelope says whether the harness ran at all.
fn text_absence(result: &RunResult) -> String {
    match result.status {
        Status::Planned => "dry run, nothing executed".to_string(),
        Status::Skipped => "not run".to_string(),
        Status::SpawnError => "the harness could not be spawned".to_string(),
        _ => "no extraction was possible from the harness's output".to_string(),
    }
}

fn count(value: Option<u64>) -> String {
    value.map_or_else(|| "null".to_string(), |n| n.to_string())
}

/// The first line of a prompt, so a multi-line prompt stays one row.
fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("")
}

/// An argv as one line, each word single-quoted when it carries whitespace or
/// a quote — for reading, not for re-executing (the JSON has the exact argv).
fn shell_words(argv: &[String]) -> String {
    argv.iter()
        .map(|word| {
            if word.is_empty()
                || word
                    .chars()
                    .any(|c| c.is_whitespace() || c == '\'' || c == '"')
            {
                format!("'{}'", word.replace('\'', "'\\''"))
            } else {
                word.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The `history show` view for a person at a terminal — what `oneharness
/// history show --format text` prints: one block per record, with the first
/// line of its prompt and answer and each of its events in [`render_event`]'s
/// form. A run still in progress (an `incomplete` entry: events but no
/// closing record yet) is headed `running`.
#[must_use]
pub fn render_history_show_text(records: &[Value]) -> String {
    if records.is_empty() {
        return "no records\n".to_string();
    }
    let mut out = String::new();
    for r in records {
        let get = |key: &str| r.get(key).and_then(|value| value.as_str()).unwrap_or("");
        if get("type") == "incomplete" {
            out.push_str(&printable(&format!(
                "running  [{harness}]\n",
                harness = get("harness"),
            )));
        } else {
            out.push_str(&printable(&format!(
                "{ts}  [{harness}] {status}\n",
                ts = get("timestamp"),
                harness = get("harness"),
                status = get("status"),
            )));
        }
        let prompt = get("prompt");
        if !prompt.is_empty() {
            out.push_str(&format!("  prompt: {}\n", printable(first_line(prompt))));
        }
        if let Some(text) = r.get("text").and_then(|value| value.as_str()) {
            out.push_str(&format!("  text: {}\n", printable(first_line(text))));
        }
        let events = r
            .get("events")
            .cloned()
            .and_then(|events| serde_json::from_value::<Vec<ActionEvent>>(events).ok())
            .unwrap_or_default();
        for event in events.iter().filter_map(render_event) {
            out.push_str(&indented(&event, "  "));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::fallback::{FallThroughReason, RunWork};
    use crate::domain::mode::PermissionMode;
    use crate::domain::report::{FallThrough, FallbackReport, OutputFormat, SessionReport};
    use crate::domain::session::SessionPhase;
    use crate::domain::signals::{FailureKind, Usage};
    use serde_json::json;

    fn event(
        kind: &str,
        name: Option<&str>,
        input: Option<Value>,
        output: Option<&str>,
    ) -> ActionEvent {
        ActionEvent {
            kind: kind.to_string(),
            name: name.map(str::to_string),
            input,
            output: output.map(str::to_string),
            index: 0,
            tool_call_id: None,
            started_at: None,
            finished_at: None,
            duration_ms: None,
            status: None,
            timing_source: None,
        }
    }

    fn command(text: &str) -> ActionEvent {
        event(
            "tool_call",
            Some("command_execution"),
            Some(json!({"command": text})),
            None,
        )
    }

    #[test]
    fn a_failed_tool_result_draws_one_outcome_line_naming_its_tool() {
        let mut failed = event("tool_result", Some("Bash"), None, Some("error: boom"));
        failed.status = Some(ToolCallStatus::Failed);
        assert_eq!(render_event(&failed).unwrap(), "✗ Bash  failed");
        // A result whose call was never seen still says a call failed.
        failed.name = None;
        assert_eq!(render_event(&failed).unwrap(), "✗ tool  failed");
        // A harness-written name cannot carry a control character to the reader.
        failed.name = Some("Ba\u{1b}[2Jsh".to_string());
        assert_eq!(render_event(&failed).unwrap(), "✗ Ba [2Jsh  failed");
    }

    #[test]
    fn a_command_is_drawn_without_the_harness_shell_wrapper() {
        for (wrapped, bare) in [
            (
                "/bin/zsh -lc 'cat docs/ai/prompt-git-workflow.md'",
                "cat docs/ai/prompt-git-workflow.md",
            ),
            ("/usr/bin/bash -lc 'ls does-not-exist'", "ls does-not-exist"),
            ("bash -lc 'echo it'\\''s'", "echo it's"),
            (
                r#"/usr/bin/bash -lc "printf 'done\\n' > out.txt""#,
                r"printf 'done\n' > out.txt",
            ),
            ("sh -c 'a && b'", "a && b"),
            (
                "pwsh.exe -NoProfile -Command 'Get-ChildItem'",
                "Get-ChildItem",
            ),
            // Not a wrapper: the agent's own command, drawn as it was.
            ("git status --short", "git status --short"),
            ("bash script.sh", "bash script.sh"),
            ("bash -lc 'unterminated", "bash -lc 'unterminated"),
            ("bash -lc 'one' 'two'", "bash -lc 'one' 'two'"),
        ] {
            assert_eq!(
                render_event(&command(wrapped)).unwrap(),
                format!("$ {bare}")
            );
        }
        let argv = event(
            "tool_call",
            Some("shell"),
            Some(json!({"command": ["bash", "-lc", "make test"]})),
            None,
        );
        assert_eq!(render_event(&argv).unwrap(), "$ make test");
        let claude = event(
            "tool_call",
            Some("Bash"),
            Some(json!({"command": "cat note.txt"})),
            None,
        );
        assert_eq!(render_event(&claude).unwrap(), "$ cat note.txt");
    }

    #[test]
    fn the_readme_text_stream_example_is_render_events_own_output() {
        // README.md draws a text stream by example; this is that example, so
        // the documented form cannot drift from what `render_event` prints.
        let mut failed = event(
            "tool_call",
            Some("command_execution"),
            Some(json!({"command": "bash -lc 'ls does-not-exist'", "exit_code": 2})),
            Some("ls: cannot access 'does-not-exist': No such file or directory\n"),
        );
        failed.status = Some(ToolCallStatus::Failed);
        let events = [
            event(
                "message",
                None,
                None,
                Some("I'll compare the products, then run both commands."),
            ),
            event("reasoning", None, None, Some("**Proceeding with 391–399**")),
            command("/bin/zsh -lc 'cat note.txt'"),
            event(
                "tool_call",
                Some("Edit"),
                Some(json!({"file_path": "src/lib.rs"})),
                None,
            ),
            failed,
            event(
                "message",
                None,
                None,
                Some("Done: the file holds two lines."),
            ),
        ];
        let drawn: Vec<String> = events.iter().filter_map(render_event).collect();
        let block = format!("```text\n{}\n\nprompt: …\n```", drawn.join("\n"));
        let readme = include_str!("../../../../README.md").replace("\r\n", "\n");
        assert!(
            readme.contains(&block),
            "README.md must carry the text-stream example as `render_event` draws it:\n{block}"
        );
    }

    #[test]
    fn a_file_change_is_one_line_per_path() {
        let codex = event(
            "tool_call",
            Some("file_change"),
            Some(json!({"changes": [
                {"path": "/work/a.md", "kind": "update"},
                {"path": "/work/b.md", "kind": "add"}
            ]})),
            None,
        );
        assert_eq!(render_event(&codex).unwrap(), "✎ /work/a.md\n✎ /work/b.md");
        let claude = event(
            "tool_call",
            Some("Edit"),
            Some(json!({"file_path": "src/lib.rs", "old_string": "a", "new_string": "b"})),
            None,
        );
        assert_eq!(render_event(&claude).unwrap(), "✎ src/lib.rs");
    }

    #[test]
    fn a_failed_call_is_marked_with_its_exit_code_and_first_output_line() {
        let mut failed = event(
            "tool_call",
            Some("command_execution"),
            Some(json!({"command": "/usr/bin/bash -lc 'ls nope'", "exit_code": 2})),
            Some("\nls: cannot access 'nope': No such file or directory\nmore\n"),
        );
        failed.status = Some(ToolCallStatus::Failed);
        assert_eq!(
            render_event(&failed).unwrap(),
            "$ ls nope ✗ exit 2: ls: cannot access 'nope': No such file or directory"
        );
        // A non-zero exit with no status yet is still a failure; a failure with
        // no exit code says so; a completed call carries no mark.
        failed.status = None;
        assert!(render_event(&failed).unwrap().contains("✗ exit 2"));
        let mut no_code = event(
            "tool_call",
            Some("count"),
            Some(json!({"n": 2})),
            Some("cancelled"),
        );
        no_code.status = Some(ToolCallStatus::Failed);
        assert_eq!(
            render_event(&no_code).unwrap(),
            r#"▸ count {"n":2} ✗ failed: cancelled"#
        );
        let mut ok = command("/bin/zsh -lc 'true'");
        ok.input = Some(json!({"command": "/bin/zsh -lc 'true'", "exit_code": 0}));
        ok.status = Some(ToolCallStatus::Completed);
        assert_eq!(render_event(&ok).unwrap(), "$ true");
        let mut cut = command("sleep 99");
        cut.status = Some(ToolCallStatus::Timeout);
        assert_eq!(render_event(&cut).unwrap(), "$ sleep 99 ✗ timed out");
        cut.status = Some(ToolCallStatus::Interrupted);
        assert_eq!(render_event(&cut).unwrap(), "$ sleep 99 ✗ interrupted");
    }

    #[test]
    fn agent_text_and_reasoning_are_told_apart() {
        let message = event("message", None, None, Some("Done.\nSecond line\n"));
        assert_eq!(render_event(&message).unwrap(), "› Done.\n  Second line");
        let reasoning = event("reasoning", None, None, Some("**Comparing** products"));
        let drawn = render_event(&reasoning).unwrap();
        assert_eq!(drawn, "(thinking) **Comparing** products");
        assert!(!drawn.starts_with(MESSAGE_MARK));
        assert_eq!(
            render_event(&event("message", None, None, Some("  \n"))),
            None
        );
        assert_eq!(render_event(&event("message", None, None, None)), None);
    }

    #[test]
    fn other_calls_name_their_main_argument_and_results_are_not_drawn() {
        let read = event(
            "tool_call",
            Some("Read"),
            Some(json!({"file_path": "a.rs"})),
            None,
        );
        assert_eq!(render_event(&read).unwrap(), "▸ Read a.rs");
        let bare = event("tool_call", None, None, None);
        assert_eq!(render_event(&bare).unwrap(), "▸ tool");
        let long = event(
            "tool_call",
            Some("x"),
            Some(json!({"blob": "y".repeat(400)})),
            None,
        );
        let drawn = render_event(&long).unwrap();
        assert!(
            drawn.ends_with('…') && drawn.chars().count() < 140,
            "{drawn}"
        );
        assert_eq!(
            render_event(&event("tool_result", None, None, Some("hi"))),
            None
        );
        let mut completed = event("tool_result", Some("Bash"), None, Some("hi"));
        completed.status = Some(ToolCallStatus::Completed);
        assert_eq!(render_event(&completed), None);
        assert_eq!(
            render_event(&event("future_kind", None, None, Some("x"))),
            None
        );
    }

    #[test]
    fn no_rendering_carries_a_control_character_or_a_trailing_newline() {
        for drawn in [
            render_event(&command("printf '\u{1b}[31mred'")).unwrap(),
            render_event(&event("message", None, None, Some("a\u{1b}[2Jb\r\n"))).unwrap(),
        ] {
            assert!(
                !drawn.chars().any(|c| c.is_control() && c != '\n'),
                "{drawn:?}"
            );
            assert!(!drawn.ends_with('\n'), "{drawn:?}");
        }
    }

    #[test]
    fn printable_keeps_newlines_and_flattens_every_other_control_character() {
        let flattened = printable("a\u{1b}[31mb\r\nc\u{7}d\u{9b}e\tf");
        assert_eq!(flattened, "a [31mb \nc d e f");
    }

    #[test]
    fn indented_lays_a_multi_line_value_out_one_row_per_line() {
        assert_eq!(indented("one\ntwo\u{1b}", "  "), "  one\n  two \n");
        assert_eq!(indented("", "  "), "");
    }

    #[test]
    fn show_text_renders_first_lines() {
        let records = vec![json!({
            "timestamp": "2026-01-01T00:00:00Z",
            "harness": "codex",
            "status": "ok",
            "prompt": "line one\nline two",
            "text": "answer\nmore",
        })];
        let text = render_history_show_text(&records);
        assert!(text.contains("[codex] ok"));
        assert!(text.contains("prompt: line one"));
        assert!(!text.contains("line two"));
        assert!(text.contains("text: answer"));
    }

    #[test]
    fn show_text_draws_a_running_entry_and_every_record_event() {
        let events = json!([
            {"kind": "message", "name": null, "input": null, "output": "on it", "index": 0},
            {"kind": "tool_call", "name": "command_execution",
             "input": {"command": "bash -lc 'ls'"}, "output": null, "index": 1}
        ]);
        let records = vec![json!({
            "type": "incomplete",
            "run_id": "0192b2a0-0000-7000-8000-000000000001",
            "harness": "codex",
            "events": events,
        })];
        assert_eq!(
            render_history_show_text(&records),
            "running  [codex]\n  › on it\n  $ ls\n\n"
        );
    }

    #[test]
    fn show_text_empty_is_labeled() {
        assert_eq!(render_history_show_text(&[]), "no records\n");
    }

    fn result(id: &str, status: Status) -> RunResult {
        RunResult {
            harness: id.to_string(),
            variant: None,
            harness_id: id.to_string(),
            bin: id.to_string(),
            available: true,
            status,
            prompt: None,
            model: None,
            observed_model: None,
            exit_code: None,
            duration_ms: None,
            telemetry: None,
            command: vec![id.to_string(), "-p".to_string(), "say hi".to_string()],
            output_format: OutputFormat::Json,
            text: None,
            text_source: None,
            usage: Usage::default(),
            usage_source: None,
            session_id: None,
            events: None,
            events_source: None,
            structured: None,
            schema_valid: None,
            schema_attempts: None,
            schema_error: None,
            failure_kind: None,
            work: None,
            failure_kind_source: None,
            stdout: String::new(),
            stderr: String::new(),
            error: None,
        }
    }

    fn report(results: Vec<RunResult>) -> RunReport {
        RunReport {
            schema_version: "test".to_string(),
            oneharness_version: "test".to_string(),
            prompt: "say hi\nsecond line".to_string(),
            model: None,
            models: None,
            resume: None,
            fork: false,
            session: None,
            permission_mode: PermissionMode::Default,
            bypass_permissions: false,
            dry_run: false,
            schema: None,
            schema_max_retries: None,
            batch: None,
            fallback: None,
            mock_rules: None,
            spy_file: None,
            history_file: None,
            config_files: vec![],
            control: None,
            results,
        }
    }

    #[test]
    fn text_view_lays_out_a_completed_result_with_its_answer_and_signals() {
        let mut ok = result("claude-code", Status::Ok);
        ok.model = Some("opus".to_string());
        ok.exit_code = Some(0);
        ok.duration_ms = Some(1234);
        ok.text = Some("pong\u{1b}[31m\nline two".to_string());
        ok.text_source = Some("json:result".to_string());
        ok.session_id = Some("sess-1".to_string());
        ok.usage = Usage {
            input_tokens: Some(10),
            output_tokens: Some(2),
            cache_read_tokens: None,
            cache_write_tokens: None,
            cost_usd: Some(0.01),
        };
        ok.usage_source = Some("json".to_string());
        let text = render_report_text(&report(vec![ok]));

        assert!(
            text.starts_with("prompt: say hi\nmode: default\n"),
            "{text}"
        );
        assert!(
            text.contains("claude-code [model opus]: ok · exit 0 · 1234 ms\n"),
            "{text}"
        );
        assert!(
            text.contains("  text (json:result):\n    pong [31m\n    line two\n"),
            "the answer is indented under its label with the escape flattened:\n{text}"
        );
        assert!(text.contains("  session id: sess-1\n"), "{text}");
        assert!(
            text.contains(
                "  usage: in 10 · out 2 · cache read null · cache write null · cost $0.01\n"
            ),
            "{text}"
        );
        assert!(
            serde_json::from_str::<serde_json::Value>(&text).is_err(),
            "the text view is not a JSON document"
        );
    }

    #[test]
    fn text_view_says_why_text_is_null_and_names_a_failure() {
        let mut failed = result("codex", Status::Nonzero);
        failed.exit_code = Some(1);
        failed.duration_ms = Some(5);
        failed.failure_kind = Some(FailureKind::Auth);
        failed.failure_kind_source = Some("stderr".to_string());
        failed.error = Some("codex exited 1: 401\r".to_string());
        let mut skipped = result("goose", Status::Skipped);
        skipped.available = false;
        let mut unclassified = result("crush", Status::Timeout);
        unclassified.work = Some(RunWork::None);
        let text = render_report_text(&report(vec![failed, skipped, unclassified]));

        assert!(text.contains("codex: nonzero · exit 1 · 5 ms\n"), "{text}");
        assert!(
            text.contains("  text: null (no extraction was possible from the harness's output)\n"),
            "{text}"
        );
        assert!(text.contains("  failure: auth (from stderr)\n"), "{text}");
        assert!(text.contains("  error: codex exited 1: 401 \n"), "{text}");
        assert!(
            text.contains("goose: skipped · exit null · not run\n"),
            "{text}"
        );
        assert!(text.contains("  text: null (not run)\n"), "{text}");
        assert!(text.contains("  work evidence: none\n"), "{text}");
    }

    #[test]
    fn text_view_carries_the_session_fallback_batch_and_schema_blocks() {
        let mut rep = report(vec![]);
        rep.session = Some(SessionReport {
            name: "chat".to_string(),
            phase: SessionPhase::Continue,
            token: Some("tok".to_string()),
            store_file: None,
        });
        rep.fallback = Some(FallbackReport {
            ran: Some("codex".to_string()),
            fell_through: vec![FallThrough {
                harness: "claude-code".to_string(),
                reason: FallThroughReason::Auth,
                detail: Some("claude-code exited 1: 401".to_string()),
            }],
            stopped_without_work: false,
        });
        rep.batch = Some(crate::domain::report::BatchReport {
            strategy: crate::domain::batch::BatchStrategy::Speed,
            prompt_count: 2,
            forked: false,
        });
        rep.schema = Some(serde_json::json!({"type": "object"}));
        rep.schema_max_retries = Some(2);
        rep.models = Some(vec!["a".to_string(), "b".to_string()]);
        let mut structured = result("codex", Status::Ok);
        structured.prompt = Some("second prompt".to_string());
        structured.structured = Some(serde_json::json!({"name": "x"}));
        structured.schema_valid = Some(false);
        structured.schema_attempts = Some(3);
        structured.schema_error = Some("missing `age`".to_string());
        rep.results = vec![structured];
        let text = render_report_text(&rep);

        assert!(text.contains("models: a, b\n"), "{text}");
        assert!(
            text.contains("session: chat (continue) · token tok · store null\n"),
            "{text}"
        );
        assert!(
            text.contains("batch: speed · 2 prompts · forked no\n"),
            "{text}"
        );
        assert!(text.contains("schema: applied · max retries 2\n"), "{text}");
        assert!(text.contains("fallback: ran codex\n"), "{text}");
        assert!(
            text.contains("  fell through claude-code: auth — claude-code exited 1: 401\n"),
            "{text}"
        );
        assert!(text.contains("  prompt: second prompt\n"), "{text}");
        assert!(
            text.contains("  structured: invalid · attempts 3\n"),
            "{text}"
        );
        assert!(
            text.contains("    {\n      \"name\": \"x\"\n    }\n"),
            "{text}"
        );
        assert!(text.contains("  schema error: missing `age`\n"), "{text}");
    }

    #[test]
    fn text_view_shows_each_planned_command_under_a_dry_run() {
        let mut rep = report(vec![result("claude-code", Status::Planned)]);
        rep.dry_run = true;
        let text = render_report_text(&rep);
        assert!(text.contains("mode: default · dry run\n"), "{text}");
        assert!(
            text.contains("  command: claude-code -p 'say hi'\n"),
            "{text}"
        );
        assert!(
            text.contains("  text: null (dry run, nothing executed)\n"),
            "{text}"
        );
    }

    #[test]
    fn shell_words_quotes_only_what_needs_it() {
        let argv = ["a", "b c", "", "it's"].map(String::from);
        assert_eq!(shell_words(&argv), "a 'b c' '' 'it'\\''s'");
    }
}
