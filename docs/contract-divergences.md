# Where the code and the contract diverge

The contract is committed **verbatim as approved** and is never edited to match
the code. Where it cannot be compiled exactly as written, the
code takes the nearest thing that does exist, and the divergence is recorded
here as a proposal for the planner who owns the contract. Nothing on this list is
resolved unilaterally.

Every entry below has since been **ruled on by the planner who owns the
contract**, and `docs/contract.md` was amended to carry each ruling. The entries
stay for the record: each states what diverged, what was ruled, and where the
amended contract now says it.

## 1. `ResolvedGraphRef` is not a type `oneagentgraph` exports — RESOLVED

**Ruling: accept `ConfigRef`; the contract names it. No new type in
`oneagentgraph`.**

The contract's `DispatchRequest` declared:

```rust
pub graph: ResolvedGraphRef,   // content-addressed node-scope agent-graph config (oneagentgraph type)
```

`oneagentgraph` exports no `ResolvedGraphRef`. The type matching the comment is
[`oneagentgraph::config::ConfigRef`](https://github.com/nickderobertis/oneagentgraph/blob/main/src/config.rs)
— "a filesystem path, or an `https` URL that is fetched, checksummed, and
recorded content-addressed in the run record so replay never depends on the URL
staying stable." `ConfigRef` is what the code uses, in `DispatchRequest::graph`
and in a plan node's `agent_graph`.

The proposal asked whether a `ResolvedGraphRef` — the fetched content plus its
digest, as distinct from the *reference* a config is written as — was intended
as a second type. It was not: the contract's seam sketch now declares
`pub graph: ConfigRef`, and neither repository grows another type for it.

## 2. `SessionSpec` is not a type `onevcs` exports — RESOLVED

**Ruling: accept `SessionRequest`; the caller asks, the executing machine
opens.**

The contract's `WorkspaceSpec` declared:

```rust
pub workspace: WorkspaceSpec,  // Path(PathBuf) | VcsSession(SessionSpec: onevcs type)
```

`onevcs` exports no `SessionSpec`. Its type for *asking* for a session is
[`onevcs::SessionRequest`](https://github.com/nickderobertis/onevcs/blob/main/crates/onevcs/src/session.rs)
— repo, branch, base, execution checkout — which is exactly what
`WorkspaceSpec::VcsSession` carries, since the contract says the machine running
the dispatch is the one that opens the session. (`onevcs::Session` is the *opened*
session, which the dispatching machine never holds.) `SessionRequest` is what the
code uses, and what the amended contract names; the surrounding sentence now says
the request carries the ask and never an opened session.

## 3. `DispatchOutcome` has no specified fields — RESOLVED

**Ruling: accept the four fields and state them; keep the struct
`#[non_exhaustive]`, and keep `tests/contract.rs` gating the prose.**

The contract named `DispatchOutcome` as `DispatchHandle::wait`'s success value
but said nothing about what it carries.

It carries four fields, each one a thing a caller **cannot** recover from the
relayed event stream:

```rust
pub struct DispatchOutcome {
    pub succeeded: bool,
    pub detail: String,
    pub session: Option<String>,
    pub branch: Option<String>,
}
```

`succeeded` and `detail` are the settlement itself — a stream of turns does not
say whether the dispatch ended well, and a node has to settle `done` or
`failed`. `session` and `branch` follow from the contract's own statement that
`WorkspaceSpec::VcsSession` means *the machine running the dispatch opens the
session there*: the caller never opened it, so publication has no token unless
the outcome hands one back.

The contract's seam sketch now declares all four under `#[non_exhaustive]`, so
naming more later stays additive. `tests/contract.rs` reads the struct's
declaration and gates it against both documents, so neither can drift from what
`DispatchOutcome` actually carries.

## 4. The rules grammar spells one predicate but describes two families — RESOLVED

**Ruling: state both predicate families explicitly, and what each matches on.**

The contract called the rules "ordered predicates over capacity **and node
labels**", and its example spelled exactly one:

```yaml
rules:
  - when: {executor_has_capacity: local}
    use: local
```

`when` is a *mapping*, so `Predicate` is compiled as a struct rather than a
one-variant enum — which also makes a second condition a second field, read as
"all of these hold". The label family's key name and matching semantics were the
contract's to settle, and it has settled them: `node_label: {KEY: VALUE, ...}`
matches the node's own reserved labels (`run_id`, `round`, `node`, `persona`) by
exact string equality, never a glob or a pattern; several conditions in one
`when` conjoin; and a `when` naming neither family — or naming `step`, which no
executor choice can see, because the choice is made once per node before any step
runs — is refused at load rather than left as a rule that silently never fires. `Predicate` carries both fields,
and the shipped default rules are unchanged — the capacity family alone is what
a single-executor host needs.

## 5. `min_free_mem: 2GiB` is carried as its string — RESOLVED

**Ruling: accept `String` plus a validated unit list; the contract states the
units. Refusing an unknown unit at load is correct.**

`ExecutorEntry::min_free_mem` is a `String`, holding `2GiB` as the contract's
example writes it, and `rules::bytes_of` reads it as a byte count where the
evaluator needs one.

The field stays a `String` on the type rather than becoming a `u64`, because the
contract fixes the *wire* syntax and a parsed field would make the type unable to
round-trip what a rules file wrote. `bytes_of` accepts exactly the units the
contract now spells — `B`, `KiB`, `MiB`, `GiB`, `TiB`, and a bare byte count.

A unit outside that list is **refused when the rules file loads**, naming the
executor and the list. Read leniently it would mean *no limit at all*, so the one
file written to keep dispatches off an exhausted host would be the file that
removed the bound — and a rules file is external input, which this crate
validates at its boundary rather than at the first dispatch. The contract now
states both the unit list and the refusal, so a rules file naming `2GB` is
refused against a documented vocabulary rather than against this
implementation's reading of the one example.

## 6. `onepipeline`'s own event kinds are not enumerated — RESOLVED

**Ruling: enumerate them, and narrow the type to an enum. Cross-source kinds stay
strings.**

The contract fixed the merged stream as "envelope NDJSON" and named the three
sources it merges, but enumerated no event kinds for this library — unlike the
sibling contracts, which each enumerate their own. `EventKind` was therefore the
wire string for everything this crate wrote as well as everything it relayed.

The contract now lists this library's nineteen kinds, and
[`event::PipelineKind`](../src/event.rs) is the enum of exactly those:
`Journal::emit` takes one, so a kind this crate emits cannot be a typo, and every
reader folds through `PipelineKind::from_wire`. A **relayed** envelope keeps its
producer's kind as `EventKind`'s wire string, because an enum there would reject
a kind a newer sibling already produces.

## 7. `resume` cannot say how much of a preserved branch survives — RESOLVED

**Ruling: accept the proposal. `resume` names the completed steps a preserved
branch carries, and a checkpoint must be a commit reachable on the remote.**

The plan schema is "v7 node shapes unchanged", and `resume` is one of them. The
shape originally compiled here was the two fields a continuation cannot do
without:

```rust
pub struct Resume {
    pub branch: String,
    pub checkpoint: Option<String>,
}
```

`ai-orchestrator`'s own `Resume` carries four more — `completed_steps`, `mode`,
`base_branch`, and `pr_base` — and one of them decides real behaviour. Its
lifecycle splits the preserved branch by *what the merge path refused*
(`orchestrator/lifecycle.py`, `REJECTED_CONTENT_OUTCOMES`): a gate that rejected
the **content** clears `completed_steps`, because republishing the identical tree
would be refused identically, while a publication that failed for any other
reason keeps them and re-runs only what is left.

`Resume` now carries `completed_steps`, and a continuation skips exactly the
steps it names. An absent or empty list re-runs the whole workstream, which is
the safe direction: work is repeated, never skipped or lost. The contract also
now states what a `checkpoint` is — a commit reachable on the remote, because the
machine that continues a node is not the machine that made it. The
content/non-content split that decides *when* `completed_steps` is cleared stays
with the library that knows what its merge path refused, which is `onevcs`; this
crate carries forward what the round it folded actually finished.

## 8. Cross-DAG edges are named by the schema and nowhere else — RESOLVED

**Ruling: adopt `ai-orchestrator`'s semantics and state them, including
blocked-not-failed and reported-not-rerun.**

The contract named the feature once, in the plan-schema sentence: "cross-DAG
`run:<id>#<node>` refs". That fixed the *syntax* and nothing else. It did not
say what resolves an edge, what an unresolvable one does, or what either records
— and an edge that is only syntax is inert, because a reference nothing resolves
blocks its consumer for ever.

The semantics implemented are `ai-orchestrator`'s, which
`docs/orchestration.md` states and which this crate's task named as the source
being preserved. The contract now states all four:

- An edge resolves by reading the referenced run's ledger. Only a `node-settled`
  of `done` satisfies it; an unknown run, a node that has not settled, and one
  that settled `failed` or `skipped` all leave the consumer **blocked**, never
  failed, because the upstream may still arrive.
- Resolution is re-read on every reconcile pass and afresh in every later round,
  so an upstream that arrives after a consumer was blocked starts it in the next
  round rather than parking the run.
- On first resolution the consumer records how far the upstream had got
  (`cross-dag-satisfied`, `{dependency, last_seq}`). If the upstream passes that
  point afterwards the consumer reports it once per consumer
  (`upstream-modified`, `{dependency, captured_last_seq, observed_last_seq}`)
  and **does not re-run**: the work was correct when it was done, and repeating
  it is the planner's judgement.
- `last_seq` is the count of records in the upstream's merged store. A run is
  written by several processes, so no single stream's `seq` describes it.

Both kinds are `onepipeline`'s own, and divergence 6 now enumerates them.
