# The crate

Every public item here exists because `docs/contract.md` names it, and the crate
is at the **interface-only** stage: the surface compiles, and nothing behind it
does anything.

Rules while that holds:

- **No method bodies** beyond derives, trivial field constructors, and serde
  `default` helpers. A byte-size parse, a predicate evaluation, a capacity probe,
  a graph walk — all of it belongs to the implementation change, not here.
- **Add no public item the contract does not name.** A `RunId` newtype, a
  builder, a convenience accessor, an extra enum variant: each is interface
  drift, and a consumer that pins to it gets a breaking change when the
  implementation lands.
- **`main` parses and refuses.** Exit code 70 (`EX_SOFTWARE`) is scaffolding kept
  clear of every code the contract spends — `0` / `1` / `2` are `reply`'s
  applied / queued / refused verdicts and `3` is "nothing is driving the run". It
  goes away with the implementation.
- **Optionality is a decision, not a default.** Where the contract states a
  default or shows a field as optional, that reading is encoded. Where it neither
  states a default nor marks a field optional, the field is *required* — do not
  quietly relax one to make a plan or a rules file parse.
- **`#![warn(missing_docs)]` with `clippy -D warnings` means undocumented public
  items fail the gate.** That is deliberate: at this stage the docs are most of
  what the crate delivers.

The llmlint directives in these files name which rule above forbids their fix.
They are exemptions for this stage, not permanent ones: revisit each with the
implementation rather than widening its directive.

## The siblings

`oneagentgraph` and `onevcs` types appear in this crate's own signatures — a
`ConfigRef` on a dispatch request and a plan node, a `SessionRequest` in a
workspace spec, `RepoType` / `Workflow` / `MergePolicy` on a lifecycle node. That
is deliberate: the seam is where the cross-repo wiring is proven at compile time.
Do not re-declare a sibling's vocabulary here, and do not add a dependency in the
other direction.

Never invent a local stand-in for a sibling type the contract names: record the
divergence instead.

Both are resolved from git by revision, each carrying the `version` it will
publish under, so the dependency is not a wildcard and the requirement is already
release-shaped. **Do not publish this crate to crates.io before both siblings are
there** — those requirements would resolve to nothing and the published crate
would not build. Cargo will not stop you; the guard is that `release.yml`'s
`publish-crate` job self-activates on `CARGO_REGISTRY_TOKEN`, so leave that
secret unset until the siblings publish.

## Tests

`tests/contract.rs` reads `docs/contract.md` itself, so a type added here without
a matching assertion there leaves the document unproven. Extend it in the same
change. `tests/e2e/main.rs` drives the compiled binary, so a new argument form is
not accepted until a test invokes it the way a user would.
