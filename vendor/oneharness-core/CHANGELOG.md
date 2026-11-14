# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.29.1](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.29.0...oneharness-core-v0.29.1) - 2026-10-23

### Fixed

- *(history)* tell a sink each run began when the run does not stream ([#1530](https://github.com/nickderobertis/oneharness/pull/1530))

## [0.29.0](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.28.8...oneharness-core-v0.29.0) - 2026-10-22

### Added

- *(history)* [**breaking**] write each run's prompt and location when it begins ([#1527](https://github.com/nickderobertis/oneharness/pull/1527))

## [0.28.8](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.28.7...oneharness-core-v0.28.8) - 2026-10-21

### Fixed

- *(app-server)* stop a pooled server once its pool entry is gone ([#1520](https://github.com/nickderobertis/oneharness/pull/1520))

## [0.28.7](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.28.6...oneharness-core-v0.28.7) - 2026-10-20

### Fixed

- *(codex)* record token usage for controlled app-server turns ([#1509](https://github.com/nickderobertis/oneharness/pull/1509))

## [0.28.6](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.28.5...oneharness-core-v0.28.6) - 2026-10-20

### Fixed

- *(mock)* run controlled mock-harness Codex turns on the shared app-server ([#1506](https://github.com/nickderobertis/oneharness/pull/1506))

## [0.28.5](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.28.4...oneharness-core-v0.28.5) - 2026-10-19

### Fixed

- *(codex)* fall through when the account cannot use the model ([#1493](https://github.com/nickderobertis/oneharness/pull/1493))

## [0.28.4](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.28.3...oneharness-core-v0.28.4) - 2026-10-19

### Fixed

- *(control)* answer a redirect as served only once its turn opens with it ([#1499](https://github.com/nickderobertis/oneharness/pull/1499))

## [0.28.3](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.28.2...oneharness-core-v0.28.3) - 2026-10-18

### Fixed

- *(live-control)* check pooled-reuse unloading under a short Codex unload delay ([#1495](https://github.com/nickderobertis/oneharness/pull/1495))
- *(app-server)* start the shared Codex app-server on macOS and report why one exits early ([#1488](https://github.com/nickderobertis/oneharness/pull/1488))
- *(app-server)* stop the app-server tests hanging the macOS sweep ([#1481](https://github.com/nickderobertis/oneharness/pull/1481))

## [0.28.2](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.28.1...oneharness-core-v0.28.2) - 2026-10-16

### Fixed

- *(app-server)* stop the shared app-server tests hanging the macOS check ([#1474](https://github.com/nickderobertis/oneharness/pull/1474))

## [0.28.1](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.28.0...oneharness-core-v0.28.1) - 2026-10-15

### Fixed

- *(app-server)* report a refused upgrade and release the caller's output on macOS ([#1470](https://github.com/nickderobertis/oneharness/pull/1470))

## [0.28.0](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.27.0...oneharness-core-v0.28.0) - 2026-10-14

### Added

- *(codex)* [**breaking**] share one pooled app-server across concurrent Codex dispatches ([#1467](https://github.com/nickderobertis/oneharness/pull/1467))

## [0.27.0](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.26.0...oneharness-core-v0.27.0) - 2026-10-13

### Added

- *(codex)* [**breaking**] share one Codex model catalog per home and keep Codex analytics off by default ([#1464](https://github.com/nickderobertis/oneharness/pull/1464))

## [0.26.0](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.25.1...oneharness-core-v0.26.0) - 2026-10-13

### Added

- *(codex)* [**breaking**] launch Codex with plugins off unless a config opts in ([#1457](https://github.com/nickderobertis/oneharness/pull/1457))

## [0.25.1](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.25.0...oneharness-core-v0.25.1) - 2026-10-13

### Added

- *(core)* find and stop a dispatch's marked processes on Linux, macOS and Windows ([#1455](https://github.com/nickderobertis/oneharness/pull/1455))

## [0.25.0](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.24.2...oneharness-core-v0.25.0) - 2026-10-09

### Added

- *(usage)* [**breaking**] report each Codex identity's banked rate-limit reset credits ([#1437](https://github.com/nickderobertis/oneharness/pull/1437))

## [0.24.2](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.24.1...oneharness-core-v0.24.2) - 2026-10-06

### Fixed

- *(events)* turn OpenCode text and reasoning parts into message and reasoning events ([#1422](https://github.com/nickderobertis/oneharness/pull/1422))

## [0.24.1](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.24.0...oneharness-core-v0.24.1) - 2026-10-03

### Fixed

- *(events)* mark a failed Claude tool call in the live text stream ([#1412](https://github.com/nickderobertis/oneharness/pull/1412))

## [0.24.0](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.23.0...oneharness-core-v0.24.0) - 2026-09-30

### Added

- *(history)* [**breaking**] dated pointer index segments, no implicit scans ([#1394](https://github.com/nickderobertis/oneharness/pull/1394))

## [0.23.0](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.22.0...oneharness-core-v0.23.0) - 2026-09-30

### Fixed

- *(sdk)* [**breaking**] accept the in-flight session entry history show prints ([#1393](https://github.com/nickderobertis/oneharness/pull/1393))

## [0.22.0](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.21.0...oneharness-core-v0.22.0) - 2026-09-30

### Added

- *(stream)* [**breaking**] stream readable text by default, NDJSON by an explicit --format json ([#1395](https://github.com/nickderobertis/oneharness/pull/1395))

## [0.21.0](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.20.0...oneharness-core-v0.21.0) - 2026-09-30

### Added

- *(stream)* [**breaking**] human-readable streaming, text history watch, and the event gaps behind it ([#1386](https://github.com/nickderobertis/oneharness/pull/1386))

<!-- llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] A changelog entry records what this release changed, as of this release; it is history a consumer reads when upgrading, not a live copy of the stream, event, history or Rust API contracts to keep in step. Their sources are `ActionEvent`'s doc comments (generated into the SDK schemas), `domain::render`, `commands::run`'s `StreamView` and `HistoryEventLine`, each pinned by its own tests and `just sdk-check`. -->

### Changed

- **Breaking:** `events` — on the live `EventSink`, in `RunReport.events` (and
  so the `--stream` NDJSON event records) and in history — now carries non-tool
  kinds beside `tool_call` / `tool_result`: `message` (the agent's own text) and
  `reasoning`, each with its text in `output`, one event per finished item
  (codex `agent_message` / `reasoning` items, Claude Code assistant `text` /
  `thinking` blocks), never a token delta. A consumer that treats every entry as
  a tool call must filter on `kind` (`ActionEvent::is_tool_activity`). Because
  `ActionEvent.index` numbers every kind, it is no longer contiguous across tool
  calls — count tool calls by `kind`, not by `index`.
- **Breaking:** a codex tool call reaches the event sink (and the `--stream`
  NDJSON, and history) once, when it completes, instead of once per lifecycle
  record; that event carries its terminal `status`, and a command's `input`
  carries the `exit_code` codex reported. Live events and the report's now share
  one numbering, so the live count of calls equals the report's.
- `run --stream --format text` (streaming from `--stream`, config `stream`, or
  `ONEHARNESS_STREAM`) is no longer refused: it prints one readable line per
  event, then the text report. A stream with no `--format` is still NDJSON in
  this release and warns once that the default becomes text in the next one;
  pass `--format json` to keep the NDJSON protocol.
- **Breaking (Rust API):** `HistoryEventLine` gains `session_name`, a
  validated `HistorySessionName` (serialized only when set; a line whose name
  is empty or not in the sanitized shape is refused on read), and `SessionSummary` gains `running` (serialized only when
  true), so struct literals of either must set them. Added:
  `domain::render::{render_event, render_report_text,
  render_history_show_text, printable, indented}`,
  `ActionEvent::is_tool_activity`, `HistoryWatcher::open_session`, and
  `HistoryWatchOptions::session` — a validated `HistorySessionSelector` (a
  minted session id or a sanitized session name), so a selector no session
  could carry is refused at the CLI, SDK and library boundary.

<!-- llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate] -->

## [0.20.0](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.19.1...oneharness-core-v0.20.0) - 2026-09-29

### Added

- *(config)* [**breaking**] repeatable --config, later files overriding earlier ones ([#1384](https://github.com/nickderobertis/oneharness/pull/1384))

### Migration: repeatable config files

<!-- llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] A changelog entry records the call shapes this release changed from and to, as of this release; it is history a consumer reads when bumping core, not a live copy of the API to keep in step. The source is `io::run`/`io::config`, and the compiler holds every caller to it. -->

Explicit configuration is now a list of files, layered in order with each later
file (its own `extends` chain beneath it) overriding the earlier ones.

- `RunRequest`, `ListRequest`, `DetectRequest`, `SyncRequest` and
  `UsageRequest`: `config: Some(path)` becomes `config: vec![path]`, and
  `config: None` becomes `config: Vec::new()` (an empty list still means
  user/project discovery).
- `io::config::load(Some(&path), no_config, start)` becomes
  `load(&[path], no_config, start)`, and `load(None, …)` becomes
  `load(&[], …)`; `load_layers` changes the same way.
<!-- llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate] -->

## [0.19.1](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.19.0...oneharness-core-v0.19.1) - 2026-09-29

### Fixed

- *(codex)* carry the sandbox mode on a resumed exec turn as config ([#1376](https://github.com/nickderobertis/oneharness/pull/1376))

## [0.19.0](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.18.0...oneharness-core-v0.19.0) - 2026-09-29

### Added

- *(sync)* [**breaking**] sync permission rules into Codex's execpolicy file and hold lists exact with --exact ([#1368](https://github.com/nickderobertis/oneharness/pull/1368))

## [0.18.0](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.17.1...oneharness-core-v0.18.0) - 2026-09-22

### Added

- *(config)* let one config extend another, resolved against its own directory ([#1349](https://github.com/nickderobertis/oneharness/pull/1349))

## [0.17.1](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.17.0...oneharness-core-v0.17.1) - 2026-09-21

### Fixed

- *(cli)* exit 1 when a one-candidate selection cannot run, fall a bin override back to its base ([#1344](https://github.com/nickderobertis/oneharness/pull/1344))

## [0.17.0](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.16.1...oneharness-core-v0.17.0) - 2026-09-20

### Added

- *(history)* [**breaking**] write a per-run pointer line for every harness run and read it back typed ([#1326](https://github.com/nickderobertis/oneharness/pull/1326))

## [0.16.1](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.16.0...oneharness-core-v0.16.1) - 2026-09-19

### Fixed

- *(core)* fall through a clean-exit rate limit, honour a base bin override for a variant, and gate rustdoc ([#1319](https://github.com/nickderobertis/oneharness/pull/1319))

## [0.16.0](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.15.0...oneharness-core-v0.16.0) - 2026-09-19

### Added

- [**breaking**] default run_mode to fallback and CLI stdout to text ([#1316](https://github.com/nickderobertis/oneharness/pull/1316))

## [0.15.0](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.14.1...oneharness-core-v0.15.0) - 2026-09-18

### Added

- *(cli)* [**breaking**] add --format <text|json> to every JSON-stdout verb, json still the default ([#1312](https://github.com/nickderobertis/oneharness/pull/1312))

## [0.14.1](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.14.0...oneharness-core-v0.14.1) - 2026-09-17

### Fixed

- *(usage)* read a null claude rate-limit snapshot as a transient to retry, never as drift ([#1304](https://github.com/nickderobertis/oneharness/pull/1304))

## [0.14.0](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.13.2...oneharness-core-v0.14.0) - 2026-09-17

### Fixed

- *(codex)* [**breaking**] retry a server-overloaded turn, then fall through ([#1300](https://github.com/nickderobertis/oneharness/pull/1300))

## [0.13.2](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.13.1...oneharness-core-v0.13.2) - 2026-09-16

### Fixed

- *(signals)* classify Claude's login refusal as an auth failure ([#1293](https://github.com/nickderobertis/oneharness/pull/1293))

## [0.13.1](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.13.0...oneharness-core-v0.13.1) - 2026-09-11

### Fixed

- *(fallback)* carry a stopped candidate's own error, and decide what an unclassified failure stops ([#1286](https://github.com/nickderobertis/oneharness/pull/1286))

## [0.13.0](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.12.3...oneharness-core-v0.13.0) - 2026-09-11

### Fixed

- *(control)* [**breaking**] run a controlled codex turn under its candidate's model, and refuse one the server would run elsewhere ([#1284](https://github.com/nickderobertis/oneharness/pull/1284))

## [0.12.3](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.12.2...oneharness-core-v0.12.3) - 2026-09-08

### Fixed

- *(usage)* answer the claude headroom probe without the project's session-start work ([#1280](https://github.com/nickderobertis/oneharness/pull/1280))

## [0.12.2](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.12.1...oneharness-core-v0.12.2) - 2026-08-30

### Fixed

- stop classifying a completed billed turn as a failure, and report a failed release ([#1277](https://github.com/nickderobertis/oneharness/pull/1277))

## [0.12.1](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.12.0...oneharness-core-v0.12.1) - 2026-08-26

### Fixed

- *(events)* normalize tool events from a controlled codex app-server turn ([#1270](https://github.com/nickderobertis/oneharness/pull/1270))

## [0.12.0](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.11.0...oneharness-core-v0.12.0) - 2026-08-24

### Fixed

- [**breaking**] resume a named session on a turn-driving control mechanism, or refuse it ([#1268](https://github.com/nickderobertis/oneharness/pull/1268))

## [0.11.0](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.10.2...oneharness-core-v0.11.0) - 2026-08-23

### Fixed

- stop an unrecognised startup refusal reading as a completed run ([#1266](https://github.com/nickderobertis/oneharness/pull/1266))

## [0.10.2](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.10.1...oneharness-core-v0.10.2) - 2026-08-19

### Added

- *(core)* own every test scratch directory with a self-removing ScratchDir ([#1264](https://github.com/nickderobertis/oneharness/pull/1264))

## [0.10.1](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.10.0...oneharness-core-v0.10.1) - 2026-08-16

### Added

- let an embedder own the harness child a run spawns ([#1260](https://github.com/nickderobertis/oneharness/pull/1260))

## [0.10.0](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.9.0...oneharness-core-v0.10.0) - 2026-08-16

### Added

- [**breaking**] fall through a precondition refusal and bound a control socket address ([#1256](https://github.com/nickderobertis/oneharness/pull/1256))

## [0.9.0](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.8.0...oneharness-core-v0.9.0) - 2026-08-15

### Added

- [**breaking**] refuse a contradictory option pair instead of editing it out ([#1255](https://github.com/nickderobertis/oneharness/pull/1255))

## [0.8.0](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.7.1...oneharness-core-v0.8.0) - 2026-08-14

### Fixed

- [**breaking**] bind a controlled turn's mechanism to the candidate serving it ([#1253](https://github.com/nickderobertis/oneharness/pull/1253))

## [0.7.1](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.7.0...oneharness-core-v0.7.1) - 2026-08-13

### Fixed

- bind a controllable turn to a fallback chain ([#1251](https://github.com/nickderobertis/oneharness/pull/1251))

## [0.7.0](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.6.14...oneharness-core-v0.7.0) - 2026-08-13

### Added

- [**breaking**] run a turn with no timeout unless one is asked for ([#1246](https://github.com/nickderobertis/oneharness/pull/1246))
- let a run opt out of the per-harness timeout entirely ([#1244](https://github.com/nickderobertis/oneharness/pull/1244))
- close the SDK gaps against the CLI and gate the drift ([#1241](https://github.com/nickderobertis/oneharness/pull/1241))
- carry a redirection with an out-of-band interrupt ([#1230](https://github.com/nickderobertis/oneharness/pull/1230))
- out-of-band turn control via a socket and `oneharness interrupt` ([#1225](https://github.com/nickderobertis/oneharness/pull/1225))
- cancel a run's harness tree on signal and expose telemetry on results ([#1222](https://github.com/nickderobertis/oneharness/pull/1222))
- stream config key and absent-home auth fallthrough ([#1220](https://github.com/nickderobertis/oneharness/pull/1220))
- stream a fallback chain instead of refusing it ([#1213](https://github.com/nickderobertis/oneharness/pull/1213))
- [**breaking**] omit absent usage fields from the wire and pin the v0.1 golden;… ([#1198](https://github.com/nickderobertis/oneharness/pull/1198))
- record observed tool timing for Anthropic-envelope harnesses ([#1196](https://github.com/nickderobertis/oneharness/pull/1196))
- add first-class harness variants ([#1186](https://github.com/nickderobertis/oneharness/pull/1186))
- incremental event persistence and live event-level history watch ([#1175](https://github.com/nickderobertis/oneharness/pull/1175))
- [**breaking**] add event-sourced history migration; define event-sourced histor… ([#1173](https://github.com/nickderobertis/oneharness/pull/1173))
- ship deterministic mock harness for CLI + all SDKs ([#1171](https://github.com/nickderobertis/oneharness/pull/1171))
- add resumable labeled history and SDK parity ([#1146](https://github.com/nickderobertis/oneharness/pull/1146))
- *(sdk)* export generated Zod schemas ([#1139](https://github.com/nickderobertis/oneharness/pull/1139))
- detect and classify claude-code deferred-tool dead-ends (tool_deferred)
- add init subcommand to scaffold a starter config ([#1129](https://github.com/nickderobertis/oneharness/pull/1129))
- configure reasoning/thinking effort per harness ([#1122](https://github.com/nickderobertis/oneharness/pull/1122))
- fan out over multiple models in parallel and fallback modes ([#1120](https://github.com/nickderobertis/oneharness/pull/1120))
- add fallback run mode (--run-mode fallback) ([#1118](https://github.com/nickderobertis/oneharness/pull/1118))
- deliver large prompts/system off the argv to harnesses ([#1115](https://github.com/nickderobertis/oneharness/pull/1115)) ([#1116](https://github.com/nickderobertis/oneharness/pull/1116))
- uniform --session handle ([#1112](https://github.com/nickderobertis/oneharness/pull/1112))
- add --system-file so a large system prompt bypasses the argv limit ([#1109](https://github.com/nickderobertis/oneharness/pull/1109))
- opt-in standardized run history + `history` view/manage verb ([#1101](https://github.com/nickderobertis/oneharness/pull/1101))
- mock/spy responder — per-tool-call deny/rewrite/stub with regex matching ([#1099](https://github.com/nickderobertis/oneharness/pull/1099))
- normalized tool-call events + streaming across the harness matrix ([#1097](https://github.com/nickderobertis/oneharness/pull/1097))
- same-prefix batch run mode (one harness over N prompts, cache-aware) ([#1088](https://github.com/nickderobertis/oneharness/pull/1088))
- surface prompt-cache token counts in normalized usage ([#1086](https://github.com/nickderobertis/oneharness/pull/1086))
- extend session continuation to all harnesses and add --fork
- [**breaking**] normalized --mode approval modes across all harnesses ([#1079](https://github.com/nickderobertis/oneharness/pull/1079))
- ONEHARNESS_&lt;FIELD&gt; environment config overrides ([#1077](https://github.com/nickderobertis/oneharness/pull/1077))
- structured output (JSON Schema) for run ([#1072](https://github.com/nickderobertis/oneharness/pull/1072))
- *(opencode)* forward session id from the plugin shim ([#1070](https://github.com/nickderobertis/oneharness/pull/1070))
- user-global hook install, runtime gate command, and live hook-enforcement e2e

### Fixed

- *(core)* release process-aware readiness API ([#1239](https://github.com/nickderobertis/oneharness/pull/1239))
- record history for a run that failed before it could be measured ([#1217](https://github.com/nickderobertis/oneharness/pull/1217))
- classify a zero-work 429 as quota so a chain falls through ([#1215](https://github.com/nickderobertis/oneharness/pull/1215))
- *(fallback)* fall through a zero-work session-limit rejection ([#1211](https://github.com/nickderobertis/oneharness/pull/1211))
- *(fallback)* classify codex usage limits as quota ([#1208](https://github.com/nickderobertis/oneharness/pull/1208))
- *(fallback)* classify Claude subscription limits as quota ([#1204](https://github.com/nickderobertis/oneharness/pull/1204))
- *(usage)* wait for codex's rate-limit answer instead of closing its stdin ([#1202](https://github.com/nickderobertis/oneharness/pull/1202))
- close harness variant CI regressions ([#1191](https://github.com/nickderobertis/oneharness/pull/1191))
- emit complete v1.0 telemetry for boundaried file_change items; loc… ([#1183](https://github.com/nickderobertis/oneharness/pull/1183))
- *(history)* address llmlint findings in event-sourced history ([#1177](https://github.com/nickderobertis/oneharness/pull/1177))
- *(sdk)* accept unavailable history timing ([#1164](https://github.com/nickderobertis/oneharness/pull/1164))
- degrade unsupported history telemetry gracefully ([#1161](https://github.com/nickderobertis/oneharness/pull/1161))
- *(history)* harden macOS timing test; add e2e; rename fixture ([#1157](https://github.com/nickderobertis/oneharness/pull/1157))
- terminate timed-out process trees and preserve telemetry ([#1147](https://github.com/nickderobertis/oneharness/pull/1147))
- harden SDK release lifecycle ([#1137](https://github.com/nickderobertis/oneharness/pull/1137))
- allow --session in fallback run mode ([#1127](https://github.com/nickderobertis/oneharness/pull/1127))
- *(cursor)* deliver reasoning as a model-id tier suffix ([#1125](https://github.com/nickderobertis/oneharness/pull/1125))
- spawn multi-line args against Windows .cmd-shim harnesses ([#1075](https://github.com/nickderobertis/oneharness/pull/1075))
- *(hooks)* honor Claude hook timeout and make the Goose manifest description caller-brandable

## [0.6.14](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.6.13...oneharness-core-v0.6.14) - 2026-08-13

### Added

- let a run opt out of the per-harness timeout entirely ([#1244](https://github.com/nickderobertis/oneharness/pull/1244))

## [0.6.13](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.6.12...oneharness-core-v0.6.13) - 2026-08-12

### Added

- close the SDK gaps against the CLI and gate the drift ([#1241](https://github.com/nickderobertis/oneharness/pull/1241))

## [0.6.12](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.6.11...oneharness-core-v0.6.12) - 2026-08-11

### Fixed

- *(core)* release process-aware readiness API ([#1239](https://github.com/nickderobertis/oneharness/pull/1239))

## [0.6.11](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.6.10...oneharness-core-v0.6.11) - 2026-08-10

### Added

- carry a redirection with an out-of-band interrupt ([#1230](https://github.com/nickderobertis/oneharness/pull/1230))

## [0.6.10](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.6.9...oneharness-core-v0.6.10) - 2026-08-09

### Added

- out-of-band turn control via a socket and `oneharness interrupt` ([#1225](https://github.com/nickderobertis/oneharness/pull/1225))

## [0.6.9](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.6.8...oneharness-core-v0.6.9) - 2026-08-08

### Added

- cancel a run's harness tree on signal and expose telemetry on results ([#1222](https://github.com/nickderobertis/oneharness/pull/1222))

## [0.6.8](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.6.7...oneharness-core-v0.6.8) - 2026-08-08

### Added

- stream config key and absent-home auth fallthrough ([#1220](https://github.com/nickderobertis/oneharness/pull/1220))

## [0.6.7](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.6.6...oneharness-core-v0.6.7) - 2026-08-05

### Fixed

- record history for a run that failed before it could be measured ([#1217](https://github.com/nickderobertis/oneharness/pull/1217))

## [0.6.6](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.6.5...oneharness-core-v0.6.6) - 2026-08-03

### Fixed

- classify a zero-work 429 as quota so a chain falls through ([#1215](https://github.com/nickderobertis/oneharness/pull/1215))

## [0.6.5](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.6.4...oneharness-core-v0.6.5) - 2026-08-02

### Added

- stream a fallback chain instead of refusing it ([#1213](https://github.com/nickderobertis/oneharness/pull/1213))

## [0.6.4](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.6.3...oneharness-core-v0.6.4) - 2026-08-01

### Fixed

- *(fallback)* fall through a zero-work session-limit rejection ([#1211](https://github.com/nickderobertis/oneharness/pull/1211))

## [0.6.3](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.6.2...oneharness-core-v0.6.3) - 2026-07-31

### Fixed

- *(fallback)* classify codex usage limits as quota ([#1208](https://github.com/nickderobertis/oneharness/pull/1208))

## [0.6.2](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.6.1...oneharness-core-v0.6.2) - 2026-07-31

### Fixed

- *(fallback)* classify Claude subscription limits as quota ([#1204](https://github.com/nickderobertis/oneharness/pull/1204))

## [0.6.1](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.6.0...oneharness-core-v0.6.1) - 2026-07-31

### Fixed

- *(usage)* wait for codex's rate-limit answer instead of closing its stdin ([#1202](https://github.com/nickderobertis/oneharness/pull/1202))

## [0.6.0](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.5.6...oneharness-core-v0.6.0) - 2026-07-30

### Added

- [**breaking**] omit absent usage fields from the wire and pin the v0.1 golden;… ([#1198](https://github.com/nickderobertis/oneharness/pull/1198))

## [0.5.6](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.5.5...oneharness-core-v0.5.6) - 2026-07-26

### Added

- record observed tool timing for Anthropic-envelope harnesses ([#1196](https://github.com/nickderobertis/oneharness/pull/1196))

## [0.5.5](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.5.4...oneharness-core-v0.5.5) - 2026-07-26

### Fixed

- close harness variant CI regressions ([#1191](https://github.com/nickderobertis/oneharness/pull/1191))

## [0.5.4](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.5.3...oneharness-core-v0.5.4) - 2026-07-25

### Added

- add first-class harness variants ([#1186](https://github.com/nickderobertis/oneharness/pull/1186))

## [0.5.3](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.5.2...oneharness-core-v0.5.3) - 2026-07-24

### Fixed

- emit complete v1.0 telemetry for boundaried file_change items; loc… ([#1183](https://github.com/nickderobertis/oneharness/pull/1183))

## [0.5.2](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.5.1...oneharness-core-v0.5.2) - 2026-07-22

### Fixed

- *(history)* address llmlint findings in event-sourced history ([#1177](https://github.com/nickderobertis/oneharness/pull/1177))

## [0.5.1](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.5.0...oneharness-core-v0.5.1) - 2026-07-22

### Added

- incremental event persistence and live event-level history watch ([#1175](https://github.com/nickderobertis/oneharness/pull/1175))

## [0.5.0](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.4.12...oneharness-core-v0.5.0) - 2026-07-22

### Added

- [**breaking**] add event-sourced history migration; define event-sourced histor… ([#1173](https://github.com/nickderobertis/oneharness/pull/1173))

## [0.4.12](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.4.11...oneharness-core-v0.4.12) - 2026-07-21

### Added

- ship deterministic mock harness for CLI + all SDKs ([#1171](https://github.com/nickderobertis/oneharness/pull/1171))

## [0.4.11](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.4.10...oneharness-core-v0.4.11) - 2026-07-20

### Fixed

- *(sdk)* accept unavailable history timing ([#1164](https://github.com/nickderobertis/oneharness/pull/1164))

## [0.4.10](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.4.9...oneharness-core-v0.4.10) - 2026-07-20

### Fixed

- degrade unsupported history telemetry gracefully ([#1161](https://github.com/nickderobertis/oneharness/pull/1161))

## [0.4.9](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.4.8...oneharness-core-v0.4.9) - 2026-07-20

### Fixed

- *(history)* harden macOS timing test; add e2e; rename fixture ([#1157](https://github.com/nickderobertis/oneharness/pull/1157))

## [0.4.8](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.4.7...oneharness-core-v0.4.8) - 2026-07-17

### Fixed

- capture Codex and Qwen session ids so `--session` resumes the prior conversation instead of silently starting a cold one; Codex now defaults to JSON, session capability is derived from each harness's session-bearing formats, and session requests automatically select a compatible format ([#1150](https://github.com/nickderobertis/oneharness/pull/1150))

### Changed

- migration: Codex and Qwen runs that explicitly combine `--session` with `--output-format text` now fail with a usage error; remove the explicit text format to let oneharness select a session-capable format ([#1150](https://github.com/nickderobertis/oneharness/pull/1150))

## [0.4.7](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.4.6...oneharness-core-v0.4.7) - 2026-07-17

### Added

- add resumable labeled history and SDK parity ([#1146](https://github.com/nickderobertis/oneharness/pull/1146))

## [0.4.6](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.4.5...oneharness-core-v0.4.6) - 2026-07-17

### Fixed

- terminate timed-out process trees and preserve telemetry ([#1147](https://github.com/nickderobertis/oneharness/pull/1147))

## [0.4.5](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.4.4...oneharness-core-v0.4.5) - 2026-07-16

### Added

- *(sdk)* export generated Zod schemas ([#1139](https://github.com/nickderobertis/oneharness/pull/1139))

### Fixed

- harden SDK release lifecycle ([#1137](https://github.com/nickderobertis/oneharness/pull/1137))

## [0.4.4](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.4.3...oneharness-core-v0.4.4) - 2026-07-13

### Added

- detect and classify claude-code deferred-tool dead-ends (tool_deferred)

## [0.4.3](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.4.2...oneharness-core-v0.4.3) - 2026-07-12

### Added

- add init subcommand to scaffold a starter config ([#1129](https://github.com/nickderobertis/oneharness/pull/1129))

## [0.4.2](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.4.1...oneharness-core-v0.4.2) - 2026-07-12

### Fixed

- allow --session in fallback run mode ([#1127](https://github.com/nickderobertis/oneharness/pull/1127))

## [0.4.1](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.4.0...oneharness-core-v0.4.1) - 2026-07-11

### Fixed

- *(cursor)* deliver reasoning as a model-id tier suffix ([#1125](https://github.com/nickderobertis/oneharness/pull/1125))

## [0.4.0](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.3.8...oneharness-core-v0.4.0) - 2026-07-11

### Added

- configure reasoning/thinking effort per harness ([#1122](https://github.com/nickderobertis/oneharness/pull/1122))
- fan out over multiple models in parallel and fallback modes ([#1120](https://github.com/nickderobertis/oneharness/pull/1120))
- add fallback run mode (--run-mode fallback) ([#1118](https://github.com/nickderobertis/oneharness/pull/1118))
- deliver large prompts/system off the argv to harnesses ([#1115](https://github.com/nickderobertis/oneharness/pull/1115)) ([#1116](https://github.com/nickderobertis/oneharness/pull/1116))
- uniform --session handle ([#1112](https://github.com/nickderobertis/oneharness/pull/1112))
- add --system-file so a large system prompt bypasses the argv limit ([#1109](https://github.com/nickderobertis/oneharness/pull/1109))
- opt-in standardized run history + `history` view/manage verb ([#1101](https://github.com/nickderobertis/oneharness/pull/1101))
- mock/spy responder — per-tool-call deny/rewrite/stub with regex matching ([#1099](https://github.com/nickderobertis/oneharness/pull/1099))
- normalized tool-call events + streaming across the harness matrix ([#1097](https://github.com/nickderobertis/oneharness/pull/1097))
- same-prefix batch run mode (one harness over N prompts, cache-aware) ([#1088](https://github.com/nickderobertis/oneharness/pull/1088))
- surface prompt-cache token counts in normalized usage ([#1086](https://github.com/nickderobertis/oneharness/pull/1086))
- extend session continuation to all harnesses and add --fork
- [**breaking**] normalized --mode approval modes across all harnesses ([#1079](https://github.com/nickderobertis/oneharness/pull/1079))
- ONEHARNESS_&lt;FIELD&gt; environment config overrides ([#1077](https://github.com/nickderobertis/oneharness/pull/1077))
- structured output (JSON Schema) for run ([#1072](https://github.com/nickderobertis/oneharness/pull/1072))
- *(opencode)* forward session id from the plugin shim ([#1070](https://github.com/nickderobertis/oneharness/pull/1070))
- user-global hook install, runtime gate command, and live hook-enforcement e2e

### Fixed

- spawn multi-line args against Windows .cmd-shim harnesses ([#1075](https://github.com/nickderobertis/oneharness/pull/1075))
- *(hooks)* honor Claude hook timeout and make the Goose manifest description caller-brandable

## [0.3.8](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.3.7...oneharness-core-v0.3.8) - 2026-07-11

### Added

- fan out over multiple models in parallel and fallback modes ([#1120](https://github.com/nickderobertis/oneharness/pull/1120))

## [0.3.7](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.3.6...oneharness-core-v0.3.7) - 2026-07-11

### Added

- add fallback run mode (--run-mode fallback) ([#1118](https://github.com/nickderobertis/oneharness/pull/1118))

## [0.3.6](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.3.5...oneharness-core-v0.3.6) - 2026-07-11

### Added

- deliver large prompts/system off the argv to harnesses ([#1115](https://github.com/nickderobertis/oneharness/pull/1115)) ([#1116](https://github.com/nickderobertis/oneharness/pull/1116))

## [0.3.5](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.3.4...oneharness-core-v0.3.5) - 2026-07-10

### Added

- uniform --session handle ([#1112](https://github.com/nickderobertis/oneharness/pull/1112))

## [0.3.4](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.3.3...oneharness-core-v0.3.4) - 2026-07-10

### Added

- add --system-file so a large system prompt bypasses the argv limit ([#1109](https://github.com/nickderobertis/oneharness/pull/1109))

## [0.3.3](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.3.2...oneharness-core-v0.3.3) - 2026-07-07

### Added

- opt-in standardized run history + `history` view/manage verb ([#1101](https://github.com/nickderobertis/oneharness/pull/1101))

## [0.3.2](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.3.1...oneharness-core-v0.3.2) - 2026-07-06

### Added

- mock/spy responder — per-tool-call deny/rewrite/stub with regex matching ([#1099](https://github.com/nickderobertis/oneharness/pull/1099))

## [0.3.1](https://github.com/nickderobertis/oneharness/compare/oneharness-core-v0.3.0...oneharness-core-v0.3.1) - 2026-07-06

### Added

- normalized tool-call events + streaming across the harness matrix ([#1097](https://github.com/nickderobertis/oneharness/pull/1097))

## [0.3.0](https://github.com/nickderobertis/oneharness/compare/v0.2.38...v0.3.0) - 2026-07-01

### Added

- same-prefix batch run mode (one harness over N prompts, cache-aware) ([#1088](https://github.com/nickderobertis/oneharness/pull/1088))
- surface prompt-cache token counts in normalized usage ([#1086](https://github.com/nickderobertis/oneharness/pull/1086))
- extend session continuation to all harnesses and add --fork
- [**breaking**] normalized --mode approval modes across all harnesses ([#1079](https://github.com/nickderobertis/oneharness/pull/1079))
- ONEHARNESS_&lt;FIELD&gt; environment config overrides ([#1077](https://github.com/nickderobertis/oneharness/pull/1077))
- structured output (JSON Schema) for run ([#1072](https://github.com/nickderobertis/oneharness/pull/1072))
- *(opencode)* forward session id from the plugin shim ([#1070](https://github.com/nickderobertis/oneharness/pull/1070))
- user-global hook install, runtime gate command, and live hook-enforcement e2e

### Fixed

- spawn multi-line args against Windows .cmd-shim harnesses ([#1075](https://github.com/nickderobertis/oneharness/pull/1075))
- *(hooks)* honor Claude hook timeout and make the Goose manifest description caller-brandable
