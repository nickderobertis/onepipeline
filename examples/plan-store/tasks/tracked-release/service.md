---
title: "feat: implement approved release"
project: "tracked-release"
repositories:
  - "github.com/nickderobertis/some-service"
depends_on:
  - "tracked-release/design-approval"
metadata:
  "onepipeline.id": "service"
  "onepipeline.repo_type": "team"
  "onepipeline.executor": "local"
  "onepipeline.steps":
    - "id": "implement"
      "persona": "engineer"
      "task": |-
        ## What
        Implement the approved API and rollout behaviour and write the tests that prove it.

        ## Why
        Deliver the approved release behaviour to service users without breaking the agreed contract, with the implementation fully proven in its own dispatch.

        ## Acceptance criteria
        - The approved API and rollout behaviour are implemented.
        - Realistic request tests prove the happy path and a failure or recovery path, without opening coverage gaps.
        - Each new test is observed failing for the intended reason before it passes.
        - The repository's own full gate is green.
    - "id": "staging-approval"
      "kind": "human"
      "task": "Exercise the staged service and approve continuation on this branch."
      "deps":
        - "implement"
---
