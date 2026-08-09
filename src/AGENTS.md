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
- **Exit codes are spent.** `0` / `1` / `2` are `reply`'s applied / queued /
  refused verdicts, `3` is "nothing is driving the run", and a round carries `0`
  for a complete graph and `1` for one that settled unfinished. Do not mint a
  fifth without the contract naming it.

## Where the engine lives

- `plan.rs` reads a plan file; `graph.rs` decides whether the graph it describes
  is legal and what may run now. Both halves of "is this input acceptable" are
  there, at the trust boundary.
- `engine.rs` is the reconcile loop and the round transition — the **single
  writer**, holding the run's ownership lock.
- `edits.rs` is the one validator both the submission check and the reconciler
  run, which is what makes "applied or rejected with a reason" true.
- `projection.rs` folds the journal into the plan of record. A round's
  `plan.json` is its launch record and is never rewritten.
- `agentgraph.rs` and `vcs.rs` are the sibling CLIs, reached as subprocesses.
  Nothing here reimplements what they own.

Two ordering rules in `engine.rs` are load-bearing and easy to undo:

- `start_ready` runs **before** the terminal check. A ready human action derives
  as `waiting`, a settled status, so a terminal check that ran first would end
  the round with that settlement unrecorded — and a later `attest` would have
  nothing to validate against.
- `attest` and `complete` are legal at a round boundary; every other command
  needs a live round. Refusing an attestation between rounds strands every
  human-gated run, because no later round can open until the action is recorded.

## The siblings

`oneagentgraph` and `onevcs` types appear in this crate's own signatures — a
`ConfigRef` on a dispatch request and a plan node, a `SessionRequest` in a
workspace spec, `RepoType` / `Workflow` / `MergePolicy` on a lifecycle node. That
is deliberate: the seam is where the cross-repo wiring is proven at compile time.
Do not re-declare a sibling's vocabulary here, and do not add a dependency in the
other direction.

Never invent a local stand-in for a sibling type the contract names: record the
divergence instead.

Two rules govern what crosses the subprocess boundary:

- **Every label sent to a sibling is namespaced under `onepipeline.`.** That
  library's run is not this one's, so it reserves the keys it stamps itself —
  `run_id`, `member`, `persona` — and refuses a `--label` naming one. A label
  added later joins the namespace; none is ever sent bare. Coming back,
  `agentgraph::adopt_labels` reads them off a relayed envelope without rewriting
  what the producer stamped, so both identities stay on the one line.
- **A launcher confirms what it launched.** `start` and `adopt` do not wait for
  the driver, so `launch_graph` watches it long enough to catch a refusal and
  fails with the graph's own words. An exit 0 and a pid for a process that had
  already died is the failure that rule exists to prevent.

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

A double is honest only where the thing it replaces cannot be run: this crate
*is* a composition layer, so a test that stubbed the seam would be testing
nothing. `tests/e2e/dispatch.rs` therefore runs the **real** `oneagentgraph`,
built from the pinned dependency by `harness::sibling_binary`, and substitutes
only the paid model turn — at that library's own `ONEAGENTGRAPH_ONEHARNESS_BIN`
override, by `fake-oneharness`. The scripted doubles stay for the scenarios a
real sibling would need paid turns to produce, and they refuse what the real CLI
refuses by *calling* it (`oneagentgraph::run::parse_label`) rather than by
copying its rules. A double that accepted a label the sibling reserves is what
let every dispatch be refused while this suite stayed green.

Give the `onevcs` seam the same journey when that sibling implements its surface.
