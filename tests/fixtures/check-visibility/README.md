# Recorded GitHub answers to `onevcs::release_holdup`

Copied verbatim from `onevcs`'s own recordings at the revision this tree links
(`crates/onevcs/tests/fixtures/graphql-recordings/` at
`fcdfab1f93ff04660253f288e17e11de8680e1ed`, whose README says how they were taken):
the real `onevcs release holdup` against the public `nickderobertis/printobserver`,
whose `release_change: {head: "release-plz-*"}` rule matched one open release-plz pull
request, #298.

- `printobserver-holdup-listing.json` — the listing of open change requests by head
  branch, replayed as recorded.
- `printobserver-holdup-standings.json` — the one standings read of #298. Every check on
  it passed when it was recorded, so `tests/e2e/check_visibility.rs` sets the `status`
  and `conclusion` of named contexts on its own copy before replaying it — a failing
  required check, a failing check that is not required, a pending required check — and
  changes nothing else about the answer.

`fake-gh` replays them from `gh.holdup-listing` and `gh.holdup-standings`, and records
each call, with the `rateLimit.cost` the answer reports, in `gh/holdup-reads.jsonl`.
