# onepipeline

Execute a task DAG over [`oneagentgraph`](https://github.com/nickderobertis/oneagentgraph)
and [`onevcs`](https://github.com/nickderobertis/onevcs), merging their event
streams into one.

`onepipeline` is the composition layer. It owns the plan — a dependency graph
mixing direct agent nodes, repository lifecycle nodes, and explicit human actions
— executes it continuously, dispatching each node the moment its dependencies
settle, through a pluggable **executor seam**, and keeps a live channel open to
the planner supervising the run. The
agents come from `oneagentgraph`; the clones, worktrees, gates, and change
requests come from `onevcs`. Dependency direction is one-way: neither sibling
depends on this crate.

The public types, traits, config schemas, and CLI surface are the approved
contract in [`docs/contract.md`](docs/contract.md), compiled — and implemented
behind it. `onevcs` is **linked and called**: sessions, publication, and a
session's event stream are library calls, so what a publication did is a typed
value rather than a line of prose to parse. `oneagentgraph` is still run as a
CLI, so a build of it that refuses will make the dispatches this crate starts
refuse too; the composition layer itself is complete.

## Install

```bash
pip install onepipeline-cli      # prebuilt binary, no Rust toolchain
npm install -g onepipeline-cli   # the same binary, via npm
cargo install onepipeline        # from crates.io, compiled locally
```

To install a revision that has not been released yet — which today is every
revision, since the crate depends on its siblings by git and a git dependency
cannot be published — build it from the repository:

```bash
cargo install --git https://github.com/nickderobertis/onepipeline onepipeline --locked
```

The package name is not optional: `cargo install --git` searches the whole
repository, and this one also carries the `onepipeline-testfakes` test harness,
so an unqualified command fails with `multiple packages with binaries found`.

Prebuilt archives for Linux (x86-64, arm64), macOS (Intel, Apple silicon), and
Windows (x86-64) are attached to every release, with `sha256` checksums.

## What it does

```bash
onepipeline start plan.json --heartbeat-interval 1800
```

`start` **drives the run itself**: a node — and each step within a lifecycle
node — dispatches the moment its dependencies settle, and settlement triggers
integration and publication immediately. No agent is required. The only pauses
are decision points: a ready `kind: human` node, or any surface declared
blocking, holds back the subtree that depends on it while every other branch
carries on, and clearing it with `attest` or `reply` resumes that subtree inside
the running loop.

`--dag-graph REF` attaches an agent graph as an **observer** — the shipped one is
a `monitor` member that watches the stream and raises what does not line up, plus
a resettable-cron `check-in` member that surfaces a status when nobody has
reported one for a while. It never drives the engine. Attached, `start` returns
when the run settles; exit `3` means nothing is driving the run, and
`onepipeline adopt RUN` attaches a fresh driver to the intact ledger.

A lifecycle node states the `title` its change request opens under, and may state
its `body` too. `--pr-author-graph REF` names an agent graph that drafts that body
instead, from the branch's own diff, once the branch is verified and before the
change request is opened; naming none is the default, and a drafting dispatch that
does not get there costs the change request its body and nothing else.

The planner supervises over the channel:

```bash
onepipeline next run-1                                   # read the next surface
onepipeline reply run-1 <<<'{"version":1,"commands":[    # edit the live graph
  {"op":"retry","id":"failed","node":{"id":"retry","task":"..."}}]}'
onepipeline attest run-1 design-approval                 # complete a human action
```

Every edit is applied or rejected with a reason: `reply` exits `0` when the
reconciler applied it, `1` when it is queued but not yet reconciled, and `2` when
it was refused.

Read-only views — `runs`, `status`, `host`, `monitor`, `results`, `goals`,
`transcript`, `telemetry` — report unread surfaces, driver liveness, and
provider health without touching a run. `status` says what each in-flight node
is doing right now, with an event count and an age; `transcript RUN [NODE]`
renders a dispatched turn's tools and its words; `telemetry` reports what each
party spent and where the wall clock went, in eight buckets that sum exactly.
Anything nothing in the stack measures is reported absent, never as a zero.

## Where a dispatch runs

The [executor seam](docs/contract.md) decides. v1 ships the local executor only;
the trait and the rules grammar are shaped so a dispatch-server or Kubernetes
executor is a config change rather than a code change.

```yaml
executors:
  - {name: local, type: local, max_load1: 8.0, min_free_mem: 2GiB}
rules:
  - when: {executor_has_capacity: local}
    use: local
  - use: local
```

Ordered: the first rule whose `when` holds decides, and a rule with no `when` is
the fallback. A `when` tests an executor's capacity, the node's own labels
(`when: {node_label: {persona: reviewer}}`), or both — several conditions in one
`when` all have to hold.

## Development

```bash
just bootstrap   # from a clean clone
just check       # the deterministic gate
just gate        # check + the diff-scoped llmlint tier
```

`just --list` is the full command surface.
[`docs/contract-divergences.md`](docs/contract-divergences.md) records every place
the code could not compile the contract exactly as written, and what the planner
who owns the contract ruled on each.

## License

MIT.
