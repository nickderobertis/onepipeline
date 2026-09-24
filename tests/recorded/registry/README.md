# A capture of what another build's registry held

`onemessagebus-agent-0.8.0.json` is every schema id
`onemessagebus_agent::registry()` listed at `onemessagebus-agent` **0.8.0**, and
the document it held under each — the last release of the crate this repository
took the agent stack's event vocabulary from. It was captured once, by running
that released crate's own `registry()` and printing `ids()` and `schema(&id)`
for each, and it is not regenerated: the point of it is to be a record of a
build this tree no longer contains.

`tests/registry.rs` holds both registries this crate constructs —
`payload::registry()` and `channel::layout::registry()` — to it: the same ids,
each under the same document. A record written by any build against one of those
ids validates against the same document here, which is what the move of the
vocabulary out of that crate promised and what this file is the evidence for.
