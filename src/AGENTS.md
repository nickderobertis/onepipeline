# The crate

Every **public** item here exists because `docs/contract.md` names it. The
engine behind that surface is private — `mod engine`, `mod driver`, `mod edits`,
`mod ledger`, and the rest — so a consumer can only reach what the contract
promised.

Rules:

- **Add no public item the contract does not name.** A `RunId` newtype, a
  builder, a convenience accessor, an extra enum variant: each is interface
  drift, and a consumer that pins to it gets a breaking change. When the engine
  needs a type, it goes in a private module.
- **Optionality is a decision, not a default.** Where the contract states a
  default or shows a field as optional, that reading is encoded. Where it neither
  states a default nor marks a field optional, the field is *required* — do not
  quietly relax one to make a plan or a rules file parse.
- **`#![warn(missing_docs)]` with `clippy -D warnings` means undocumented public
  items fail the gate.**
- **The task a graph is launched with says what the run *is*, never who does
  what.** `oneagentgraph` gives that one `--task` to every member carrying none
  of its own, so a role stated there is stated to members whose job it is not.
  Roles belong to the consuming graph: a member's persona, or its own `task`
  composed from `{task}`.
- **Exit codes are spent.** `0` / `1` / `2` are `reply`'s applied / queued /
  refused verdicts, `3` is "nothing is driving the run", and a driver carries `0`
  for a complete graph and `1` for one that settled unfinished. Do not mint a
  fifth without the contract naming it.

## Where the engine lives

- `plan.rs` is the plan **schema**; `taskgraph.rs` reads one project of a
  `onetaskgraph` store as a plan of that shape, driving that binary as a
  subprocess; `graph.rs` decides whether the graph it describes is legal and
  what may run now. All of "is this input acceptable" is there, at the trust
  boundary a project crosses.
- `engine.rs` is the one continuous reconcile loop — the **single writer**,
  holding the run's ownership lock for as long as it drives. There are no
  rounds: a node dispatches on the pass that observed its last dependency
  settle, and the only thing that pauses anything is a decision point, which
  pauses only the subtree depending on it.
- `edits.rs` is the one validator both the submission check and the reconciler
  run, which is what makes "applied or rejected with a reason" true.
  It also holds the two host hooks. The per-node validator fires inside
  `compile`, so an accepted edit is offered twice — once at submission and once
  at reconcile — and the envelope reviewer fires once, at the submission check
  only, which is the one place a refusal can still turn a whole envelope away.
- `projection.rs` folds the journal into the plan of record. The run's
  `plan.json` is its launch record and is never rewritten, and the store the
  plan was read from is never re-read to decide what the run is doing.
- `writeback.rs` projects that folded graph back onto the `onetaskgraph` project
  it was launched from, through that binary's own `project copy` out of a shadow
  `local-md` store under the run. The shadow names each far end of an edge
  `<project>/<task>`: a copy rewrites an edge to the destination's own id only
  where its far end is a **member of the copied set**, and a bare file name —
  which resolves to `<shadow-source>:<file>` — is not one.
- `agentgraph.rs` and `vcs.rs` are the sibling CLIs, reached as subprocesses.
  Nothing here reimplements what they own.
- `report.rs` is **half public**: `retain` and the constants an accepted
  settlement is built from are the contract's retention path, published so a
  consumer writes a report through the same promise it resolves one back
  through — `RunPaths::report_for`, re-exported from `views`. Everything else
  there is `pub(crate)`, because what this crate's views render out of a
  retained report is a rendering rather than a promise, and the segment
  sanitiser behind `report_for` stays private so nobody restates it.

The loop **waits**; it does not poll. A pass runs when a dispatch says
something, when the planner's channel moves, when something outside this run
moves — an upstream ledger, an answered release probe, a failed projection — or
when the longest interval it may go without one comes due, and on nothing else.
So **whole-state work belongs on a change, never on a pass**: a per-pass
`state.statuses()`, `writeback.publish`, `upstreams.resolve` or
`projection::fold` puts back a CPU sink big enough to make this host unusable.

Its other half: a pass that **moved the run's own state re-passes at once**. A
settlement the pass itself made — an `expects_no_diff` node, a human action
recorded as waiting — has no dispatch thread to report it and writes nothing to
the channel, so what it readied has no wake of its own to be started on.

Two ordering rules in `engine.rs` are load-bearing and easy to undo:

- `start_ready` runs **before** the terminal check. A ready human action derives
  as `waiting`, a settled status, so a terminal check that ran first would end
  the loop with that settlement unrecorded — and a later `attest` would have
  nothing to validate against.
- `drive_run` takes the ownership lock **before** it claims the run in the launch
  record. A driver that wrote its pid there and then lost the race for the lock
  would leave the record naming a process that is gone, and every reader would
  call the run undriven while the driver that won was still working.
- `holds_now` runs **after** `start_ready`, so what a concurrency hold names as
  ahead of it is what is really in flight and a node the pass just dispatched is
  not reported as held by the run it just joined.

## The siblings

`oneagentgraph` and `onevcs` types appear in this crate's own signatures — a
`ConfigRef` on a dispatch request and a plan node, a `SessionRequest` in a
workspace spec, `RepoType` / `Workflow` / `MergePolicy` on a lifecycle node. That
is deliberate: the seam is where the cross-repo wiring is proven at compile time.
Do not re-declare a sibling's vocabulary here, and do not add a dependency in the
other direction.

Never invent a local stand-in for a sibling type the contract names: record the
divergence instead.

`onejudge` is a dependency too, and is **not** a third sibling this crate
composes — nothing here launches, calls, or spawns it. It is one vocabulary: the
verdicts a two-party member settles on, which `oneagentgraph` copies onto the
`member-settled` this crate relays. Reading them by field name would be the
re-declaration above, so `report::failed_verdicts` deserializes into that
library's own `NamedVerdict`. Its pin must resolve to the one `oneagentgraph`
carries, and `src/report.rs` fails the build where it does not.

Both are ordinary crates.io dependencies, pinned to a published version. **Keep
them that way** — a `git`/`rev` source makes the graph unreproducible from the
registries alone, hides which released version carries a given API, and leaves
the crate unbuildable for anyone without access to that revision. When a sibling
grows an API this crate needs, the answer is a release of the sibling, not a
revision pin here.

## Deliberately absent

There is no `typecheck` target. The clippy pass `lint` already runs, over every
target and denying warnings, *is* this crate's type check, so a separate target
would re-compile the tree to learn nothing new.

## Tests

`tests/contract.rs` reads `docs/contract.md` itself, so a type added here without
a matching assertion there leaves the document unproven. Extend it in the same
change.

`tests/e2e/` drives the compiled binary against **real executables** standing in
for the two siblings, built from `crates/testfakes` and scripted per test from a
directory (`build.fail`, `build.wait` + `build.go` to hold a dispatch open,
`service.pr-author.fail` to fail one persona's dispatch only). A separate
workspace member so they can never ship; `tests/e2e/harness.rs` builds them on
demand, because a package-scoped build — `cargo llvm-cov` runs one — does not
build another member's binaries.

The doubles are the only honest way to test this crate: it *is* a composition
layer, so a test that stubbed the seam would be testing nothing.

A single-sided member's turn is a library call inside `oneagentgraph`, with no
argv, exit status or stderr of its own. So the paid turn is substituted a layer
further in — `fake-claude`, at oneharness's `ONEHARNESS_BIN_CLAUDE_CODE` — and
what a member was asked to do is read from that turn's own record
(`World::turns`), never off `member-started`, which carries the composed config
and worktree.

Each sibling also has a journey driving its **real** binary, built from the
version `Cargo.lock` pins: `tests/e2e/dispatch.rs` for `oneagentgraph`,
`tests/e2e/real_vcs.rs` for `onevcs`, and `tests/smoke/` for both at once plus a
real GitHub. A double states a scenario; it is never a stand-in for a sibling
nobody has run. When a double's answer changes, check it against the real one,
and when a journey lands, its real e2e lands with it.
