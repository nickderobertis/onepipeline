Private workflow contracts run with `just nx run github-workflows:test`.
`just nx run github-workflows:check` reaches the focused lint, syntax checks,
telemetry-producing tests and repository-pinned budget checker. Bootstrap installs
this directory's locked dependencies; checks do not install them.

`macos-policy.mjs classify --event JSON --base REF --head REF` prints the agreed
JSON decision. `GITHUB_EVENT_NAME` supplies Actions' event kind (defaults to PR
outside Actions). The non-PR events the workflows use never request Mac; an
unrecognised event name does, with a reason naming it. Exit 0 includes
conservative decisions requiring Mac coverage when PR input is unreadable. Invalid command syntax/mode exits 1 on stderr.
`carrier` exits 0 only for a successful proof or classifier-authorized omission;
failed, cancelled, skipped or missing required work exits 1.
`path-carrier` reports one leg of a path-selected Linux/Windows matrix proof
under its protected name: exit 0 for that leg's recorded success, or for a proof
the affected-project selection skipped; a failed, cancelled or missing
prerequisite, proof or leg verdict exits 1.

`macos-budget.mjs --metric jobs-per-run|runs-per-change --telemetry JSON` reads
existing telemetry and reports through `ONEBUDGETSPEC_RESULT`. Exit 0 means the
measurement was reported; exit 1 means invalid invocation, unreadable/malformed/
stale telemetry, or missing/unwritable reporting configuration. It compares no
threshold. `just nx run github-workflows:budgets` uses the checker pinned by the justfile;
its nonzero status means a budget exceeded its bound or could not be measured.

The reusable snapshot is locked to the existing caller's Git blob. The canonical
comparison preserves the upstream resolver; only screencomp's CLI boundary is
doubled when the actual guard/architecture shell steps run locally.
