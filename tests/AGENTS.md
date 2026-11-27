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
- **Every world runs the binary's waits at `harness::TIME_SCALE`.** The debug
  build multiplies each shipped default and fixed wait by
  `ONEPIPELINE_TEST_TIME_SCALE` (`src/timescale.rs`), and a value a journey names
  through a wait's own variable is applied as named. A journey states an expected
  wait as `World::scaled` of the shipped figure, never a literal; one proving a
  shipped value, or recording telemetry a budget reads, says
  `World::at_shipped_time` and why. Hang guards (`until`'s deadline, nextest's
  timeouts) are not waits the binary runs and are never scaled.
- **A journey whose time is a wait moves a clock instead of sitting it out.**
  `World::on_journey_clock` puts every process the world starts on a
  `JourneyClock` (`ONEPIPELINE_TEST_CLOCK`, debug builds only): the clock-bound
  waits `src/timescale.rs` names are measured on the host's clock plus what the
  journey moved it on by, and each one the engine schedules is recorded. The
  journey moves the clock (`pass`, `passing`, `advance`) and proves the timing
  from that record and the records stamped on the same clock — each wait the
  engine scheduled, in order — never from a gap it observed, so a busy host
  cannot fail a correct schedule. Moving it passes a window just as waiting
  would, so a "nothing happened in that window" proof moves it on through the
  window rather than sleeping. Backstops on real work stay on the host's clock.

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
- **One coverage floor.** An offline tier's `test` runs through
  `scripts/coverage-unit.sh`, declares its `target/coverage-units/<project>.lcov`
  as its `outputs`, and replays that report when its key did not move;
  `onepipeline:test` merges every tier's report through
  `scripts/coverage-merge.mjs` and `scripts/coverage-tiers.mjs` refuses a tier it
  would not count. A tier's key names the external tools through `coverageTierKey`.
- **The live tiers have no `test`.** Their journey is an uncached target of
  its own that no `check` reaches, and refuses — never skips — without its
  credential or network.
- **An exclusive tier runs with nothing else.** A `test` or `test-quick` target
  marked `parallelism: false` starts only once no other Nx task is running, and
  nothing starts beside it. The tiers whose binaries share the `e2e` nextest
  group carry it so one such nextest process runs at a time (#783): the group's
  process-tree cap and the overrides that take the whole group are per nextest
  process, so a second one beside them is the oversubscription the cap exists to
  prevent. The budget and live tiers carry it because they time themselves or
  failed under load (`status-read`, `human-items`, `smoke`, `release-compat`).
- **The e2e tier's coverage `test` is the exception.** It is the longest task of
  the Linux job, so it starts as soon as `coverage-clean` has run and the job's
  builds and node suites run beside it, through `scripts/nx-precedence.sh`: they
  take only the CPU it leaves idle, so its deadlines are not starved, and
  `install-smoke`, which times host-view-time's reads, keeps its own priority.
  Its `test-quick` on macOS and Windows still runs alone.
