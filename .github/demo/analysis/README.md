DEMONSTRATION ONLY: the analysis behind docs/ci-e2e-profile.md, as run from the measuring
dispatch's scratch directory over the downloaded artifacts (paths there are that layout).

- `tier.py`: one platform's run of records, and the tier's concurrency.
- `split.py`: each test's split, pairing the spawn log with the span records.
- `gitsplit.py`: git processes per test of a serial run, from `GIT_TRACE2_EVENT`.
- `tierwide.py`: the tier-wide split, engine git at the runner's floor.
- `final.py`, `render.py`: the floors and the per-journey tables.
