# Recorded streams, from the bus's agent profile crate

One stream from each producer of the agent stack, and a fourth holding three
relayed lines with a drifted label order. Each is **copied byte for byte** from
`crates/onemessagebus-agent/tests/recorded/` in
[`onemessagebus`](https://github.com/nickderobertis/onemessagebus) at tag
`onemessagebus-agent-v0.8.0` — the last release of the crate that held the agent
stack's event vocabulary before `onepipeline` took it over
(`src/vocabulary.rs`). Nothing in these files was edited; a fixture that had been
touched would prove the touch rather than the producer.

They live here now because this is the crate that declares the vocabulary they
are written in, and `tests/recorded_bus.rs` holds every line of them to
round-tripping through this crate's own `Reader` and `serde_json::to_string`
with no byte changed. That is the whole of what the move promised: the bytes on
the wire did not move when the types did.

The provenance of each, carried over from that crate's own `README.md`:

- `oneagentgraph-run.ndjson` — the whole `events.jsonl` of the `oneagentgraph`
  run `review-bar-probe-1789277572872-1933894` (9 envelopes, `v: 1`, source
  `agentgraph`, the `session` extra label on the turn kinds).
- `onevcs-session.ndjson` — the whole stream of the `onevcs` publication session
  `publish-branch-onevcs-s-b5c195333f94` (7 envelopes, `v: 1`, source `vcs`,
  `phase` stamped on every line).
- `onepipeline-events.jsonl` — lines 2–5 and 59–64 of the `onepipeline` run
  `otg-issue-repo-design`'s `events.jsonl`, selected by line number and
  otherwise untouched: the run's own `v: 2` `pipeline` envelopes beside the
  `v: 1` `agentgraph` and `vcs` envelopes it relayed, so one file carries every
  source and both envelope versions. The lines left out are the same shapes
  with long turn payloads — and the three below.
- `onepipeline-relayed-drift.jsonl` — lines 6, 8 and 9 of that same file: three
  `agentgraph` envelopes `onepipeline` relayed whose labels it wrote as
  `run_id, node, persona, member, …`. That order is drift this crate resolved
  rather than a shape to preserve: `onepipeline`'s copy of `Labels` had no
  `member` field, so a relayed `member` fell among the extras and came back out
  after `persona`, while `oneagentgraph` — the producer — writes `member` before
  `persona`, as the contract lists the reserved keys.
  `tests/recorded_bus.rs` holds these three to reading whole and re-serializing
  in the contract's order, which is what this build writes. They are kept apart
  from the file above so the byte-identity test stays exact rather than carrying
  an exception list.

`tests/golden/envelope-v1.json` and `tests/golden/envelope-v2.json` are this
crate's own and stay where they are; that crate had copies of them.
