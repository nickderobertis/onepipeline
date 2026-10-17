# Measurements the spike report cites

- `results.tsv` — one line per `measure.sh` invocation (`matrix.sh` re-takes every
  one): the configuration, then the run count and how many runs lost the stand-in's
  stdout line, its stderr line, or either. Loaded runs were taken three
  configurations at a time (plus a fourth stream for the `direct=false` ones), each
  with its own `nproc` busy loops, on a 20-core WSL2 host other work was also using
  (load average 95–110 throughout); idle runs one at a time with nothing else from
  this harness running.
- `nx-repro.log` — `nx-repro.sh` against Nx 23.1.1, 23.3.0, 24.0.0-beta.2,
  24.0.0-canary.20261016-5ef7804 and 23.1.2, 50 runs each.
- `lost-run-onepipeline-nxsh-ci.*` — what one lost run printed (stdout is empty;
  stderr is Nx's frame with neither stand-in line).
- `truncated-replay-ai-orchestrator.*` — the first 400 and last 300 bytes of one
  replay through ai-orchestrator's `scripts/nx.sh` that arrived cut at ~63KB.
