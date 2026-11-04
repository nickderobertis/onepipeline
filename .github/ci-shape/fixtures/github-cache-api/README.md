# GitHub cache evidence response recordings

These are complete, unedited `gh api` response bodies read from this draft's
run 41780837817, commit eb4468c4 and PR #1391. Each entry in `provenance.json`
names its source URL, fetch date and SHA-256 of the exact response bytes.
Gzip stores those bytes losslessly; `support/github-recordings.mjs` verifies
them before parsing.

The journey's synthetic repository, commit and PR identities are applied only
by that helper's labelled test-only variant builders. Its comparison selection
comes from the fixture workspace's real Git ancestry. The cache CLI and sidecar
remain production code; GitHub's HTTP boundary is the double.
