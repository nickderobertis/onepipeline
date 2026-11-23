# Human-items journeys

- `counted_human_items_reads` is the pull request's: it times nothing and holds each warm
  read to `main.rs`'s `EXPECTED_FULL`. A change that moves a count updates it in its own
  diff; never loosen a count to a bound.
- `timed_human_items_reads` is `human-items-e2e:timed`'s, on main alone: regenerate its
  record with `just human-items-journey`. `report.mjs` times nothing.
- Both read only through `plan`; add a read there, never beside it.
- The fixture's key (`fixture.rs`'s `KEY`) names every input that decides what the
  fixture holds. An input the generator starts to depend on joins it, and
  `lifecycle.rs`'s `EDITS` gains the edit that proves it moves the key.
- `github-store.sh` saves only where `HUMAN_ITEMS_FIXTURE_SAVE_DIR` is set: ci.yml's
  push-to-main `timed` job. A pull request restores, and never saves.
- Keep this tier on its own Nx edge: an ordinary e2e edit must not run it.
