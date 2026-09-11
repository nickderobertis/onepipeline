# The suite

Every journey here drives the compiled binary, the sibling doubles it runs as
subprocesses, and real repositories; the root `AGENTS.md` says what that
requires of a test. This layer holds the rule the wait sites in
`e2e/harness.rs` — and the fixture trees in `src/sys.rs` — are written to.

- **A wait may not spend what it waits for needs.** Process starts are the
  scarce resource on the hosted cross-platform runners — an order of magnitude
  dearer than on a laptop, and shared by a handful of tests at once — so a wait
  that starts a process per poll competes with the launch it is waiting on and
  loses on exactly the host where the deadline is tightest. A wait polls files;
  a wait that has to ask a process yields between asks (`World::until_store`);
  and a test that needs to know a spawned child is there has the child say so —
  a pid on its own stdout, a double's `<key>.arrived` (`World::held`) — rather
  than asking a process listing. A wall-clock deadline is the backstop for the
  product's own asynchrony, never the signal.
