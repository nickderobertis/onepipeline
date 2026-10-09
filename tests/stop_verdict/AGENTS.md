# Stop-verdict budget journeys

`onepipeline-stop-verdict:test` (`just stop-verdict-journeys`) builds the release
binary, then builds both full host-shaped workloads once from empty with
`onevcs-testing`'s recovery fixture and times that binary's
`stop-guard --format claude-code --unpublished`. Every verdict is checked before
its timing counts; git is counted only in separate calls through a forwarding
shim. Only after every assertion passes are the record, the invocation manifest
and the producing binary written to `target/budget-records/`, which Nx restores
together and `onepipeline:budgets` reads through
`scripts/stop-guard-unpublished-budget.mjs` — a reader that launches nothing and
refuses a missing, incomplete, stale, foreign, debug or load-run record.
`ONEPIPELINE_BUDGET_LOAD_WORKERS=16` runs the same journey under generated load
and writes `stop-guard-unpublished-load.json`, which no budget reads. Keep these
workloads on their own edge: an ordinary e2e edit must not rebuild them.
