---
title: "test: cover the reporting service's failure paths"
project: "single-node"
repositories:
  - "github.com/nickderobertis/some-service"
metadata:
  "onepipeline.id": "cover-report-failures"
  "onepipeline.persona": "engineer"
  "onepipeline.max_turns": 24
---

## What
Add realistic tests for the reporting endpoint's failure and recovery paths.

## Why
The endpoint's failures are currently unproven, so a regression there reaches users before anyone sees it.

## Acceptance criteria
- A test drives the real endpoint and asserts its behaviour when the upstream store is unavailable.
- A test drives the real endpoint and asserts it recovers once the store returns.
- Each new test is observed failing for the intended reason before it passes.
- The repository's own full gate is green, without lowering enforced coverage.
