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

- `plan.rs` reads a plan file; `graph.rs` decides whether the graph it describes
  is legal and what may run now. Both halves of "is this input acceptable" are
  there, at the trust boundary.
- `engine.rs` is the one continuous reconcile loop — the **single writer**,
  holding the run's ownership lock for as long as it drives. There are no
  rounds: a node dispatches on the pass that observed its last dependency
  settle, and the only thing that pauses anything is a decision point, which
  pauses only the subtree depending on it.
- `edits.rs` is the one validator both the submission check and the reconciler
  run, which is what makes "applied or rejected with a reason" true.
- `projection.rs` folds the journal into the plan of record. The run's
  `plan.json` is its launch record and is never rewritten.
- `agentgraph.rs` and `vcs.rs` are the sibling CLIs, reached as subprocesses.
  Nothing here reimplements what they own.

Two ordering rules in `engine.rs` are load-bearing and easy to undo:

- `start_ready` runs **before** the terminal check. A ready human action derives
  as `waiting`, a settled status, so a terminal check that ran first would end
  the loop with that settlement unrecorded — and a later `attest` would have
  nothing to validate against.
- `drive_run` takes the ownership lock **before** it claims the run in the launch
  record. A driver that wrote its pid there and then lost the race for the lock
  would leave the record naming a process that is gone, and every reader would
  call the run undriven while the driver that won was still working.

## The siblings

`oneagentgraph` and `onevcs` types appear in this crate's own signatures — a
`ConfigRef` on a dispatch request and a plan node, a `SessionRequest` in a
workspace spec, `RepoType` / `Workflow` / `MergePolicy` on a lifecycle node. That
is deliberate: the seam is where the cross-repo wiring is proven at compile time.
Do not re-declare a sibling's vocabulary here, and do not add a dependency in the
other direction.

Never invent a local stand-in for a sibling type the contract names: record the
divergence instead.

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

`tests/e2e/dispatch.rs` drives the **real** `oneagentgraph` and substitutes only
the paid turn — and that substitution is `fake-claude` at oneharness's own
`ONEHARNESS_BIN_CLAUDE_CODE`, not `oneharness` itself: from `oneagentgraph
0.2.18` a single-sided member's turn is an `oneharness_core` library call, so it
has no argv, no exit status and no stderr for a journey to read. What a turn was
asked to do is therefore readable only from the turn's own record
(`World::turns`), and which member it was rides that member's harness config
`[env]` block; `member-started` carries the composed `config` and the `worktree`
instead.

Each sibling also has a journey driving its **real** binary, built from the
version `Cargo.lock` pins: `tests/e2e/dispatch.rs` for `oneagentgraph`,
`tests/e2e/real_vcs.rs` for `onevcs`, and `tests/smoke/` for both at once plus a
real GitHub. A double states a scenario; it is never a stand-in for a sibling
nobody has run. When a double's answer changes, check it against the real one,
and when a journey lands, its real e2e lands with it.
