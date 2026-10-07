# The suite

- **A wait may not spend what it waits for needs.** Process starts are the
  scarce resource on the hosted cross-platform runners — an order of magnitude
  dearer than on a laptop, and shared by a handful of tests at once — so a wait
  that starts a process per poll competes with the launch it is waiting on and
  loses on exactly the host where the deadline is tightest. A wait polls files;
  a wait that has to ask a process yields between asks (`World::until_store` in
  `e2e/harness.rs`); and a test that needs to know a spawned child is there has
  the child say so — a pid on its own stdout, as the fixture trees in
  `src/sys.rs` do, or a double connecting to the test's `<key>.rendezvous`
  (`World::rendezvous`, `fake::meet`), which holds and releases with no clock on
  either side — rather than asking a process listing. A wall-clock deadline is
  the backstop for the product's own asynchrony, never the signal.

# The test-tier projects

A test binary with a `project.json` beside its sources is its own Nx project;
the crate's project (`onepipeline:test-rest`) runs the unit tests and every
other binary here.

- **The tiers partition the offline suite.** The justfile's `*-tier` filters
  decide which tests each project runs, and `rest-tier` is spelled as the
  complement of the others, so a new binary lands in the crate's tier rather
  than in none. A new tier project adds its filter there, its binary to
  `rest-tier`'s exclusions, and its `test` to `onepipeline:test`'s `dependsOn`.
- **A tier's inputs are what its tests read.** Its named input in `nx.json`
  lists the sources it compiles *and* every file it opens at run time; one it
  misses replays a cached green run over a change it would have failed on.
- **One coverage floor.** An offline tier's `test` runs instrumented with
  `--no-report`, names its profiles `onepipeline-<tier>-*.profraw`, and declares
  exactly those as its `outputs`, so a replay restores what
  `onepipeline:test`'s single report merges.
- **The live tiers have no `test`.** Their journey is an uncached target of
  its own that no `check` reaches, and refuses — never skips — without its
  credential or network.
