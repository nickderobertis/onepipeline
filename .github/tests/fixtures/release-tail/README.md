# Recorded GitHub responses for the release tail

`../../release-tail.test.mjs` replays these through `ONEVCS_GH`, onevcs's seam for replaying
recorded responses, while it runs `publish`'s last step as release.yml writes it. Every file under
`recordings/` is a real GitHub REST response recorded read-only from this repository with
`gh api --include --method GET <path>` on 2026-11-01, byte for byte as gh printed it: the status
line, every header (each ending in CRLF), a blank line and the body. Its `<name>.source.json`
sidecar names the API path and the release, pull request or run it was fetched for.

The release is v0.97.1 (commit c3f74a07, Release run 41192531055): its own three reads, the runs
at its commit and at each of its three pull requests' heads (#1278, #1320 and release-plz's
#1322), and the jobs of every run the figures select. These are exactly the 31 reads onevcs 0.57.4
made for the seven `release-tail` budgets over that release. v0.97.1 rather than a later release:
the later ones' commit messages name a private repository, and nothing here may carry that. One more is the releases listing
answered 401 under an invalid token, the error status the tail must exit non-zero on.

v0.97.1 predates the four-job CI shape and the main-push Rust cache warmer: its pull request heads
ran six workflows each, and its release commit carries the push runs ci.yml, the npm launcher,
engine currency and visual docs then made, so every figure it reports is over its threshold. That
is the shape the budgets were written to bring down; the test holds each figure to the one it
computes from these jobs' own timestamps and counts, whatever its verdict.

Nothing is hand-written or edited. The one change any response is served with — the `publish`
job of the Release run served in progress, because the tail runs inside it — is the test's,
applied when it serves, and named by that recording's sidecar under `served_in_progress`.

`reconcile.mjs` re-fetches every path and diffs it against its recording, ignoring only what
GitHub rewrites (the date, etag, last-modified, rate-limit and request-id headers, and
`download_count` and `updated_at`). It reaches GitHub, so it runs by hand and never in a gate.
