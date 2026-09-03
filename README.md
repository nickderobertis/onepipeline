# onepipeline

Execute a task DAG over [`oneagentgraph`](https://github.com/nickderobertis/oneagentgraph)
and [`onevcs`](https://github.com/nickderobertis/onevcs), merging their event
streams into one.

`onepipeline` is the composition layer. It executes a plan — a dependency graph
mixing direct agent nodes, repository lifecycle nodes, and explicit human actions
— continuously, dispatching each node the moment its dependencies
settle, through a pluggable **executor seam**, and keeps a live channel open to
the planner supervising the run. The plan itself lives in
[`onetaskgraph`](https://github.com/nickderobertis/onetaskgraph): a run is
launched by naming a project of whichever backend you already track work in, so
a plan is something you can open, edit and share without this harness in the
loop. The
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

## Where a plan lives

A plan is one **onetaskgraph project**, and a node is one task in it. A run is
launched by naming that project's qualified id:

```bash
onepipeline start plans:tracked-release --heartbeat-interval 1800
```

Which backend that store is — a folder of Markdown, Linear, GitHub Projects — is
onetaskgraph's own configuration, discovered from the directory you launch in:

```yaml
# onetaskgraph.yaml
sources:
  plans:
    plugin: local-md
    config: { root: ./plans }
```

`examples/plan-store/` is a complete store of that shape, holding the two example
plans this repository ships. Nothing here special-cases a remote source, so a
`local-md` project runs directly — author locally, run it, and copy it up only
when it should become durable.

The mapping is [`docs/contract.md`](docs/contract.md)'s, and it is one rule per
field: the plan-level settings (`schema_version`, `goal`, `name`, `concurrency`)
are reserved `onepipeline.<field>` metadata keys on the **project**; a node's id
is `onepipeline.id` on its task; its prose is the task's `content`, its title the
task's `title`, and its repository the first of the task's `repositories`; its
dependencies are real onetaskgraph dependency edges; and every other node field
is `onepipeline.<field>` carrying the same JSON value a plan document carried
under that name. One task of the example store:

```markdown
---
title: "feat: implement approved release"
project: "tracked-release"
repositories:
  - "github.com/nickderobertis/some-service"
depends_on:
  - "tracked-release/design-approval"
metadata:
  "onepipeline.id": "service"
  "onepipeline.persona": "engineer"
  "onepipeline.max_turns": 24
---
## What
Implement the approved API and rollout behaviour.
```

onepipeline **drives the onetaskgraph binary** rather than linking the crate, so
one has to be installed: from `ONETASKGRAPH_BIN` when that names one, and from
`onetaskgraph` on the `PATH` otherwise. Its version is checked before anything is
dispatched, and an absent, unusable, or too-old install refuses the launch —
naming the path, the version, the minimum, and how to install one — rather than
becoming a run that fails on its first node.

A plan is checkable before it is launched, and the check is the engine's own
loader — every refusal `start` makes before it dispatches anything, and no other
rule — so a consumer never has to re-implement it:

```bash
onepipeline plan check plans:tracked-release --check ./checks/review-bar --json
```

Each repeatable `--check <PATH>` names an executable, resolved against the
directory the verb ran in. It is handed the **loaded** plan as one JSON document
on its stdin — every default resolved, each node carrying its task's own metadata
map verbatim — with `ONEPIPELINE_PLAN_CHECK_SCHEMA=1` in its environment, and
answers on stdout with `{"refusals": [...]}` and exit 0, `node` and `field`
present on each and null where it is about neither. Engine refusals come first
and carry `"source": "engine"`; each check's follow in the order its flags were
given, under the path as it was given, and `--json` prints them as one object
carrying `project`, `accepted`, `refusals` and `unrunnable`, always all four.
Exit `0` is the loader and every check accepting, `1` is at least one refusal
from either source, and `2` is a project that could not be read or a check that
could not be run — which is reported separately from a refusal and never read as
an accept. A loader refusal short-circuits: there is no loaded plan to hand a
check, so each is reported as not run.

The run's own record does not move: the journal, the ledger, and the graph a run
is executing are still this crate's, projected from that journal under the run's
ownership lock. Node status, settlement metadata, and accepted live graph edits
are projected back onto the onetaskgraph project in the background. A failed
write is reported and retried; it never changes execution or an edit ruling.

## What it does

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
`onepipeline adopt RUN` attaches a fresh driver to the intact ledger, and takes
the same `--attach`/`--detach` pair `start` does: detached, it prints the launch
record and returns once the driver it retained has the run, so recovering one run
does not hold the session supervising the others.

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
behind it. The first four leave the rejected tree on the branch the session handed
back, so the node is **dispatched again on that branch**, with no step
recorded as completed and
with the failure's reason and the id of every artifact its publication recorded
delivered as that dispatch's own context — the worker meets the diagnosis, on the
tree that has to change. `pushed-unverified` is answered differently, because
nothing about its tree was rejected: the work is already on the origin, so the
**merge path is read again** — bounded by `ONEPIPELINE_MERGE_PATH_READS`, three by
default — rather than the agent re-dispatched for a fresh clone and a fresh gate to
re-push what the remote already carries. A verdict that arrives during those reads
settles the node; reads that never get one settle it `failed` saying where the work
is, what commit it is at, and what stopped the read. Everything else settles
`publication-failed` as it always
did and is not retried: the repository's own gate, a request refused at a trust
boundary, and a seam with no implementation behind it all answer the same way
however many times they are asked. The loop is bounded by
`ONEPIPELINE_PUBLICATION_ATTEMPTS`, three by default, and a node that spends it
settles `failed` under the last failure's word, saying how many attempts were made
and what each one ended with.

A dispatch that ends for a reason that is **not the agent's verdict on its task**
settles `dispatch-died` rather than `task-failed`: a rate limit twenty seconds after
the final report, a harness that lost its credential, a run root deleted underneath
a live turn. The word is chosen by classifying the failure's own detail and never by
inspecting the branch, so a dispatch that died holding finished work and one that
produced nothing at all reach the same word. The settlement carries `cause` — the
producer's own classification, `rate_limit`, `quota`, `auth`, `spawn-error` — and
`head`, the commit the node's branch was left at, and `results` and `status` say in
one sentence that the branch may carry finished work and name that commit. It is not
`infrastructure-failure`, which is the dispatch layer refusing **before any work
began** and is retried for exactly that reason.

Where the producer's own liveness rule was `provider-failure` the word narrows to
`provider-failed`, carrying the same `cause`, `branch` and `head`: a node whose
provider went is a node with nothing wrong with its work, and `task-failed` sent
the reader looking for what the work got wrong. That classification is reconciled
against the record of the turn it names before anything acts on it — a turn that
opened, closed, and was billed says the dispatch produced what the death says it
did not, and where the two disagree the record wins and the node is not settled as
a provider death. Not in the approved contract yet: open divergence 49.

Every node dispatch carries `ONEPIPELINE_NODE_SCRATCH_DIR` in its own
environment: an absolute path to a directory that exists and is writable before
the dispatch's first turn, unique to that dispatch — a retry, a requeue and a
resumed pin of the same node each get their own — and never removed while that
dispatch runs. Nothing beyond that is promised, and the spelling of the path least
of all: no consumer may derive one path from another. Not in the approved contract
yet either: open divergence 48.

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
reviewing it read the same ruling — on the dispatch that follows it and on every
later one, until another `amend` replaces it. A turn already in flight is not
reached: its task was composed before the ruling existed, and so was the one its
judge reads. A node's current amendment is readable from
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

A launch may also name an **envelope reviewer** — a second command of the host's
own, offered the whole reply envelope once, after every command in it has passed
this crate's own validation and the node validator above and before any of its
edits is committed. One JSON document crosses its stdin: the run's goal, every
node the envelope introduces or changes with the op that produced each, and the
plan they are being edited into, as the envelope leaves it. That is the review no
per-node check can make — two added nodes that duplicate each other, a contract
seam between two nodes of one edit, the dependency edges the edit introduces, or
whether the edited graph still delivers the goal. Exit `0` accepts it; a non-zero
exit **refuses the whole envelope**, so no command of it half-applies, with the
command's own stderr as the reason and every op and node the envelope carried
named beside it. The refusal also names the node the reviewer **objected to**,
which is not the same set: the reviewer declares it on a line of its stderr
reading `objection: cover` — one line per node, repeatable — and a reviewer that
declares none is reported as having declared none rather than as objecting to
everything. A reviewer that cannot be started refuses the envelope rather than
letting it through **reviewed by nothing**. An accepted envelope is offered
**once per envelope** — unlike the node validator's two offers, because this hook
exists for a review a host plausibly answers with an agent, and because the
submission check is the only place a refusal is still whole. It is named by
`--envelope-reviewer COMMAND`, by `ONEPIPELINE_ENVELOPE_REVIEWER`, or by a launch
config's `envelope_reviewer`, in that order of precedence; naming none is the
default and runs no reviewer at all.

Read-only views — `runs`, `status`, `host`, `monitor`, `results`, `goals`,
`transcript`, `telemetry` — report unread surfaces, driver liveness, and
provider health without touching a run. `status` says what each in-flight node
is doing right now, with an event count and an age; `transcript RUN [NODE]`
renders a dispatched turn's tools and its words; `telemetry` reports what each
party spent and where the wall clock went, in eight buckets that sum exactly.
Anything nothing in the stack measures is reported absent, never as a zero.

`onepipeline watch RUN` is the bounded wait a supervisor puts in a wake loop, and
it takes the same `--filter NAME|SPEC` / `--all` profile selection `monitor` does.
It blocks, writing one line per event a supervisor acts on — a graph edit whichever
author issued it, a node settling at any outcome, a surface being raised, a decision
beginning or clearing to hold a subtree, and the run being stopped — and one
heartbeat line per `--tick-interval SECONDS` of silence, so a quiet run and a dead
stream are not the same thing seen from outside. **Every heartbeat says how many
planner surfaces are unread and of which kinds**, so a caller matching only on event
lines cannot lose the one signal that a question is waiting. The human lines go to
standard error and one NDJSON record per line to standard output — the events, the
heartbeats, and a final record naming the condition, the exit code and the cursor —
so nothing has to match prose.

It returns on four conditions, each with a status of its own: exit `0` when the
run settled complete, `3` when nothing is driving it — the same code every other
verb here uses for that — `4` when a blocking surface is waiting to be answered,
and `5` when the `--timeout SECONDS` wait elapsed with the run still live. It
prints a cursor on exit, and `--cursor` resumes from one without re-emitting what
the earlier watch already did. `--until settled` waits through a blocking surface
rather than returning on it, still counting it on every heartbeat.
`--tick-interval` is **this stream's** clock and is not `start`'s
`--heartbeat-interval`, which sets the pacemaker agent's cadence; neither verb
accepts the other's flag.

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
