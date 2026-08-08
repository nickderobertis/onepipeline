# Where the code and the contract diverge

The contract is committed **verbatim as approved** and is never edited to match
the code. Where it cannot be compiled exactly as written, the
code takes the nearest thing that does exist, and the divergence is recorded
here as a proposal for the planner who owns the contract. Nothing on this list is
resolved unilaterally.

## 1. `ResolvedGraphRef` is not a type `oneagentgraph` exports

The contract's `DispatchRequest` declares:

```rust
pub graph: ResolvedGraphRef,   // content-addressed node-scope agent-graph config (oneagentgraph type)
```

`oneagentgraph` exports no `ResolvedGraphRef`. The type matching the comment is
[`oneagentgraph::config::ConfigRef`](https://github.com/nickderobertis/oneagentgraph/blob/main/src/config.rs)
— "a filesystem path, or an `https` URL that is fetched, checksummed, and
recorded content-addressed in the run record so replay never depends on the URL
staying stable." `ConfigRef` is what the code uses, in `DispatchRequest::graph`
and in a plan node's `agent_graph`.

The two names may describe two things: a `ConfigRef` is the *reference* a config
is written as, and a `ResolvedGraphRef` reads as the *result* of resolving one —
the fetched content plus its digest. If that distinction is intended, it is
`oneagentgraph`'s type to add and this crate's to consume; the planner decides
which repository grows it.

## 2. `SessionSpec` is not a type `onevcs` exports

The contract's `WorkspaceSpec` declares:

```rust
pub workspace: WorkspaceSpec,  // Path(PathBuf) | VcsSession(SessionSpec: onevcs type)
```

`onevcs` exports no `SessionSpec`. Its type for *asking* for a session is
[`onevcs::SessionRequest`](https://github.com/nickderobertis/onevcs/blob/main/crates/onevcs/src/session.rs)
— repo, branch, base, execution checkout — which is exactly what
`WorkspaceSpec::VcsSession` carries, since the contract says the machine running
the dispatch is the one that opens the session. (`onevcs::Session` is the *opened*
session, which the dispatching machine never holds.) `SessionRequest` is what the
code uses. This looks like a naming difference rather than a design one.

## 3. `DispatchOutcome` has no specified fields

The contract names `DispatchOutcome` as `DispatchHandle::wait`'s success value
but says nothing about what it carries.

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

The struct is `#[non_exhaustive]`, so naming more fields later is additive. The
proposal for the planner is to state these four in the contract, or to say
instead that `wait` reports settlement only and that a session token reaches the
caller some other way.

`tests/contract.rs` gates this list against the type, so the prose above cannot
drift from what `DispatchOutcome` actually carries.

## 4. The rules grammar spells one predicate but describes two families

The contract calls the rules "ordered predicates over capacity **and node
labels**", and its example spells exactly one:

```yaml
rules:
  - when: {executor_has_capacity: local}
    use: local
```

`when` is a *mapping*, so `Predicate` is compiled as a struct with that one
required field rather than as a one-variant enum — which also makes a second
condition a second field, read as "all of these hold". The label predicates are
not invented here: their key names and matching semantics (equality? glob? a
mapping of label to pattern?) are the contract's to settle, as is whether several
conditions in one `when` conjoin.

## 5. `min_free_mem: 2GiB` is carried as its string

`ExecutorEntry::min_free_mem` is a `String`, holding `2GiB` as the contract's
example writes it, and `rules::bytes_of` reads it as a byte count where the
evaluator needs one.

The field stays a `String` on the type rather than becoming a `u64`, because the
contract fixes the *wire* syntax and a parsed field would make the type unable to
round-trip what a rules file wrote. `bytes_of` accepts exactly the units the
contract spells — `B`, `KiB`, `MiB`, `GiB`, `TiB`, and a bare byte count — and
answers `None` for anything else rather than guessing, which resolves toward
"has capacity" the way every other unreadable input does.

The proposal for the planner is to state that unit list in the contract, since a
rules file naming `2GB` today is silently treated as "no limit".

## 6. `onepipeline`'s own event kinds are not enumerated

The contract fixes the merged stream as "envelope NDJSON" and names the three
sources it merges, but enumerates no event kinds for this library — unlike the
sibling contracts, which each enumerate their own. `EventKind` is therefore the
wire string: this crate both relays another library's kinds and emits its own,
and an enum here would reject a kind a sibling already produces.

It narrows to an enum once the contract names this library's kinds. Until then
the structural boundary still holds — `v`, `ts`, `stream`, `seq`, `source`, and
`labels` are all typed, and `source` rejects anything but the three libraries.
