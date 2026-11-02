# Recorded GitHub answers to `onevcs::release_holdup`

Copied verbatim from `onevcs`'s own recordings at the revision this tree links
(`crates/onevcs/tests/fixtures/graphql-recordings/` at
`fcdfab1f93ff04660253f288e17e11de8680e1ed`, whose README says how they were taken):
the real `onevcs release holdup` against the public `nickderobertis/printobserver`,
whose `release_change: {head: "release-plz-*"}` rule matched one open release-plz pull
request, #298.

- `printobserver-holdup-listing.json` — the listing of open change requests by head
  branch, replayed as recorded.
- `printobserver-holdup-standings.json` — the one standings read of #298. When it was
  recorded three of its checks were red (`gate (windows-x86_64)`, `supply-chain` and
  `gate-shard (windows-x86_64, 1/5)`) and three skipped. `tests/e2e/check_visibility.rs`
  sets those three red ones green on its own copy, then sets the `status` and
  `conclusion` of the contexts a journey is about — a failing required check, a failing
  check that is not required, a pending required check — before replaying it, and
  changes nothing else about the answer.

`fake-gh` replays them from `gh.holdup-listing` and `gh.holdup-standings`, and records
each call, with the `rateLimit.cost` the answer reports, in `gh/holdup-reads.jsonl`.
