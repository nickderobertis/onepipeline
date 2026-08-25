# onepipeline

Execute a task DAG over [`oneagentgraph`](https://github.com/nickderobertis/oneagentgraph)
and [`onevcs`](https://github.com/nickderobertis/onevcs), merging their event
streams into one.

`onepipeline` is the composition layer. It owns the plan — a dependency graph
mixing direct agent nodes, repository lifecycle nodes, and explicit human actions
— executes it continuously, dispatching each node the moment its dependencies
settle, through a pluggable **executor seam**, and keeps a live channel open to
the planner supervising the run. The
agents come from `oneagentgraph`; the clones, worktrees, publications, and
change requests come from `onevcs`. Nothing here verifies a change: that is the
repository's own merge path — the host's required checks where a change
publishes remotely, and the repository's `pre-push` hook at the publishing push
where it publishes locally. Dependency direction is one-way: neither sibling
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
does not get there costs the change request its body and nothing else. It costs no
visibility either: a drafting dispatch that was configured, attempted, and produced
no body is recorded against the node under one of three endings — `dispatch-failed`
for one that could not be run or ran without succeeding, `schema-refused` for one
whose every answer the schema rejected, and `no-body` for one that answered inside
the schema and put nothing in it, which are three different fixes — and the node's
own settlement says the same thing, so `results` shows it. Naming no graph and
writing the `body` yourself spend no dispatch and are not reported.

A publication that fails does not always finish the node. `onevcs` says which
failure it was, and five of them settle under a word of their own — `checks-failed`
for a required check the host reports concluded red, `checks-unsettled` for a bound
that elapsed with the change still outstanding, `push-rejected` for a push the merge
path refused, `sync-conflict` for a base that moved under the publication, and
`pushed-unverified` for a push that reached the remote with the merge path unreadable
behind it. Each of the five leaves the rejected tree on the branch the session
handed back, so the node is **dispatched again on that branch**, with no step
recorded as completed and
with the failure's reason and the id of every artifact its publication recorded
delivered as that dispatch's own context — the worker meets the diagnosis, on the
tree that has to change. Everything else settles `publication-failed` as it always
did and is not retried: the repository's own gate, a request refused at a trust
boundary, and a seam with no implementation behind it all answer the same way
however many times they are asked. The loop is bounded by
`ONEPIPELINE_PUBLICATION_ATTEMPTS`, three by default, and a node that spends it
settles `failed` under the last failure's word, saying how many attempts were made
and what each one ended with.

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

Two of those ops reach a node that is already running, and they are deliberately
not the same lever:

```bash
onepipeline reply run-1 <<<'{"version":1,"commands":[    # steer the worker
  {"op":"context","id":"build","note":"the fixture moved to tests/data"}]}'
onepipeline reply run-1 <<<'{"version":1,"commands":[    # move the bar
  {"op":"amend","id":"build","text":"The comment lines are out of scope: leave them."}]}'
```

A `context` note **steers the worker only**. It is rendered under
`## Planner context` saying of itself that it reports observed state and adds no
acceptance criteria, it carries exactly one dispatch, and it does not change what
the node is judged against. An `amend` **does** change that: its text becomes part
of the node's effective task, rendered under `## Amendment` above the task's
operational notes and claiming precedence over them, so the worker and the judge
reviewing it read the same ruling — on that dispatch and on every later one,
until another `amend` replaces it. A node's current amendment is readable from
`status` and from `results` before anything replaces it. Without the second lever
a manager's mid-dispatch ruling reaches the worker and not its judge, and the
node's own judge can tell it to undo what the manager decided.

`amend` is the planner's; an observing monitor may not issue one, because moving a
bar is a decomposition decision rather than an observation.

A launch may also name a **node validator** — a command of the host's own, which
every op that introduces or changes a node's task (`add`, `retry`, a `requeue`
whose amendment touches `task`, and `amend`) is offered the resulting node to, as
JSON on its stdin. Exit `0` accepts the edit; a non-zero exit refuses it with the
command's own stderr as the reason. It is named by `--node-validator COMMAND`, by
`ONEPIPELINE_NODE_VALIDATOR`, or by a launch config's `node_validator`, in that
order of precedence; naming none is the default and runs no validator at all.

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
