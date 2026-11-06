The budgets job's own budget. `test` drives the real `budgets` job of `ci.yml` through the released action at its pin, with
only GitHub, the release host and the package index doubled, and records the job's telemetry; `budgets` reads it back and
reruns nothing. It reaches the release, PyPI and codeload, so no `check` runs it and offline it fails rather than skips.
Budget-only data stays here. Assert behaviour, never a threshold.
