# One pull request's own run, as its `budgets` job reads it

`jobs.json` is `GET repos/nickderobertis/onepipeline/actions/runs/42237631071/jobs?per_page=100&filter=latest`,
pull request #1428's real run, recorded on 2026-11-07 and kept to the fields
`scripts/actions-budget-figures.sh` reads. One thing is rewritten: `budgets` is put back to
`in_progress`, as that job sees itself when its command runs. The figures it gives are its longest
finished job, `test-linux` at 6,549 s, and 5 jobs.
