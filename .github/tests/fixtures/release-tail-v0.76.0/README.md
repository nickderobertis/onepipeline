# Recorded release-cycle selection

This is onepipeline v0.76.0, recorded read-only on 2026-11-04 through
onevcs 0.57.4's `ONEVCS_GH` seam. `selection.json` is its real `host
release-changes` answer. PR #983 changes only `.github/workflows/release-sweep.yml`;
it contributes no published path and its checks are never read. The other seven
carried PRs qualify under the root published-paths contract.

`github.json` records the 47 REST answers used by `release-changes`, each
qualifying PR's `change-checks`, and the release commit's `commit-runs`. Each
record names its endpoint and recording time. It projects fields consumed by
onevcs, removes unconsumed bodies, user details, links and job steps, and retains
the eight carried rows and the final row of each closed-pull page for pagination. Values, order and
pagination/rate headers come from GitHub; no response value is fabricated.
`expected.json` records the first run and last non-skipped, runner-taken job
from the real `change-checks` answers before that projection.

The test runs the copied production figures command under onebudgetspec,
with the real onevcs reading these responses. Its tail clock is the dispatch's
current clock, not the historical release's publish time; this verifies
selection and arithmetic, not a claim about that release's original latency.
