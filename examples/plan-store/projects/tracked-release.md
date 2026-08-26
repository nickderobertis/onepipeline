---
title: "tracked-release"
metadata:
  "onepipeline.schema_version": 3
  "onepipeline.goal":
    "text": "Deliver the tracked release safely across its target projects"
  "onepipeline.concurrency": 3
---

A plan with every node shape in it — a direct agent node, a `kind: human`
approval, a lifecycle node running two steps on one branch, a node that
expects no diff, and a node that names its own agent graph.
