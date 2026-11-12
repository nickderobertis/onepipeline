# `released-rulings`

A run root the **released** `onepipeline` 0.93.1 (the `onepipeline-cli` 0.93.1
wheel, the newest on PyPI when this was recorded) wrote, before reply envelopes,
journal records and recorded notes carried a `provenance`. Its plan is `slow`
and `later` (which depends on `slow`). While `slow` was held running, the release
was sent a version 3 `amend` of `later`, a version 2 `note` to `later` with
`deliver: next` (recorded `carried`), and a version 3 verdict. Its driver was then
ended, so the run is unsettled and waiting to be adopted.

`@REPO@` stands for the checkout the run was launched from and `@WORLD@` for the
harness world it ran in. `tests/e2e/rulings.rs` substitutes both when it seeds a
world with this root, and it names no process as the driver, as
`recorded_support::seeded_with` does. Nothing here is edited by hand otherwise.
