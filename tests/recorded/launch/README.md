# Recorded launch records

Launch records an older build of this crate wrote, byte for byte as they were on
disk. Nothing was edited or redacted.

## `otg-closed-state-writes-status.json`

The launch record of the run `otg-closed-state-writes-status` on this host
(`/home/whoop/ai-orchestrator/runs/otg-closed-state-writes-status/launch.json`),
launched 2026-09-17T11:43:21Z and last written that day at 15:03 local time,
after its second adoption. Its `bus_config` names one codec, `onejudge`, as the
engine it was launched under wrote codecs: a `queue`, a `reply_window_seconds`,
a `session_env` and an `asker_env`, and no `select` or `frames` — the two fields
`onemessagebus` 0.7 made every codec carry. Every `onepipeline` from 0.37.0 until
the build that reads it best-effort refused a record like it with
``missing field `select` `` (issue #380); 0.40.0 still does.

`tests/e2e/older_launch_record.rs` splices that `bus_config`, unedited, into its
copy of the recorded run root `../run-root/onemessagebus-repair-2/` and drives
`status`, `results`, `adopt` and the success hook over it; `src/ledger.rs`'s tests
read the whole record, and hold a `serve` of its codec to the refusal that names
the missing field.
