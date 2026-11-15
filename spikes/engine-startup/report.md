# Spike `spike-engine-startup`

- **Branch:** `nick/process-launch-speed-2026-11-14/spike-engine-startup`

## Harness

The branch carries scripts/engine-startup.mjs, engine-startup-cpu.py, engine-startup-profile.sh and engine-startup-verify.sh. Fold a demonstration with `node scripts/engine-startup.mjs fold before/1=BASE_JOB contributors/1=AFTER_JOB`. It prints per-job start/adopt completed trace means and e2e minutes, then per-side means, spreads, differences and typical-PR estimates. See spikes/engine-startup/README.md for profiling and equivalence commands.

## Method

IN PROGRESS. Ruling c13.0 replaces hosted sampling with local attribution and existing hosted launch-trace/tier measurements. Ruling c11.1 accepts run 43684231509 attempts 1 and 2 as the before side. Rulings c7.0/c9.1 require reporting Linux merged coverage as not produced because the vendored patch audit fails test-rest and Nx skips coverage merge; floor remains 95.74 LCOV.

Handoff: no hosted runs started by this dispatch; none in flight. Local prototype and checks are underway. Demonstration capacity is held by child-waits under c11.1; wait for a slot, then start round peers together. Remaining: source-identified local sampling on current debug/release builds, hosted after rounds, outcomes and cache audit, final judged lint and comparison. No performance saving is claimed yet.



### Baseline fold checkpoint (source 8f51bf37)

The following output was read from `/home/whoop/ai-orchestrator/runs/brief-process-launch-speed-2026-11-14-spikes/scratch/3469976-4/baseline-finished-8f51bf37.txt`, produced on commit 8f51bf37 by `node scripts/engine-startup.mjs fold` with six `before/POINT/OS=/home/whoop/ai-orchestrator/runs/brief-process-launch-speed-2026-11-14-spikes/scratch/1794352-0/runs/baseline-POINT/test-OS` arguments (POINT=1,2; OS=linux,macos,windows). These are trace lifetimes, not unavailable per-verb test-profile program records. The command was run after that commit; later validation-only source repairs do not establish a new measurement. A finished-source fold remains owed.

Hosted source: run 43684231509 attempts 1 and 2, head 36237191db78133b709b3d893ccc11d91d2a5272, GITHUB_SHA 37341b0a57e77d18f6db6c11c1b32d7b30fb8dac; executed workflow blob db2bbe081a7b9319eb38bd014e72350b389e8b01. Baseline facts and full test-name catalogs are committed under spikes/launch-inventory/evidence/baseline-{1,2}.facts.json.gz.

| Attempt | Job | Job id | Artifact id(s) | Job min | E2e min |

| --- | --- | --- | --- | ---: | ---: |

| 1 | test-macos | 132721606835 | 14159457280 | 201.850000 | 90.112450 |

| 1 | test-windows | 132721606851 | 14153829104 | 144.700000 | 87.112333 |

| 1 | test-linux | 132721606859 | 14153153329 | 136.300000 | 37.919033 |

| 2 | test-linux | 132799383400 | 14165992711 | 171.466667 | 41.055683 |

| 2 | test-windows | 132799383582 | 14167358776 | 251.516667 | 132.993667 |

| 2 | test-macos | 132799383592 | 14166951348 | 198.550000 | 94.377033 |

```text

before/1/linux (linux)
runner: Intel(R) Xeon(R) 6975P-C; cores: 4
e2e minutes: 37.919033
start: 820.300928 ms mean; 2351 completed / 2371 observed
adopt: 1396.245420 ms mean; 340 completed / 345 observed
clock: completed launch-trace lifetime; excludes process creation; unfinished processes excluded
test profiles: 2155; names digest: 83531b68a69ebaf7; scope: {"id":"21fd59ea-9add-43aa-abae-0fd219c703a7","start":1794728919.828,"end":1794731194.969,"seconds":2275.142,"run":2155,"passed":2155,"line":"Summary [2275.142s] 2155 tests run: 2155 passed (23 slow), 1 skipped","records":174,"starts":56,"os":0,"otherProfiles":1}

before/2/linux (linux)
runner: INTEL(R) XEON(R) PLATINUM 8573C; cores: 4
e2e minutes: 41.055683
start: 891.417304 ms mean; 2351 completed / 2371 observed
adopt: 1476.363930 ms mean; 340 completed / 346 observed
clock: completed launch-trace lifetime; excludes process creation; unfinished processes excluded
test profiles: 2155; names digest: 83531b68a69ebaf7; scope: {"id":"2077affc-b3f8-4207-9d22-fc387055d3ae","start":1794750371.568,"end":1794752834.909,"seconds":2463.341,"run":2155,"passed":2155,"line":"Summary [2463.341s] 2155 tests run: 2155 passed (24 slow), 1 skipped","records":19,"starts":5,"os":0,"otherProfiles":1}

before/1/macos (macos)
runner: Apple M1 (Virtual); cores: 3
e2e minutes: 90.112450
start: 1847.244006 ms mean; 2328 completed / 2348 observed
adopt: 2601.639218 ms mean; 349 completed / 354 observed
clock: completed launch-trace lifetime; excludes process creation; unfinished processes excluded
test profiles: 2121; names digest: 224faf8a6439b9ed; scope: {"id":"b0545ab0-aab0-419c-bd08-8def1645ea5d","start":1794736078.613,"end":1794741485.336,"seconds":5406.747,"run":2121,"passed":2121,"line":"Summary [5406.747s] 2121 tests run: 2121 passed (40 slow), 1 skipped","records":0,"starts":0,"os":0,"otherProfiles":0}

before/2/macos (macos)
runner: Apple M1 (Virtual); cores: 3
e2e minutes: 94.377033
start: 1978.519577 ms mean; 2328 completed / 2348 observed
adopt: 2796.896188 ms mean; 349 completed / 354 observed
clock: completed launch-trace lifetime; excludes process creation; unfinished processes excluded
test profiles: 2121; names digest: 224faf8a6439b9ed; scope: {"id":"be48b42d-fa3b-44c3-8a1c-13e5ef37bf20","start":1794748917.563,"end":1794754580.181,"seconds":5662.622,"run":2121,"passed":2121,"line":"Summary [5662.622s] 2121 tests run: 2121 passed (41 slow), 1 skipped","records":0,"starts":0,"os":0,"otherProfiles":0}

before/1/windows (windows)
runner: AMD EPYC 9V45 96-Core Processor                ; cores: 4
e2e minutes: 87.112333
start: 3375.080794 ms mean; 2083 completed / 2089 observed
adopt: 4658.311521 ms mean; 302 completed / 305 observed
clock: completed launch-trace lifetime; excludes process creation; unfinished processes excluded
test profiles: 1832; names digest: a91139c2b34c8c42; scope: {"id":"60af6ffa-12c2-429c-848b-1cffb3033ad3","start":1794726570.108,"end":1794731796.94,"seconds":5226.74,"run":1832,"passed":1832,"line":"Summary [5226.740s] 1832 tests run: 1832 passed (54 slow), 1 skipped","records":0,"starts":0,"os":0,"otherProfiles":0}

before/2/windows (windows)
runner: AMD EPYC 7763 64-Core Processor                ; cores: 4
e2e minutes: 132.993667
start: 5663.412835 ms mean; 2083 completed / 2089 observed
adopt: 8031.223044 ms mean; 302 completed / 305 observed
clock: completed launch-trace lifetime; excludes process creation; unfinished processes excluded
test profiles: 1832; names digest: a91139c2b34c8c42; scope: {"id":"68cf7a38-6e72-43ac-ba8c-0d9e9a850d56","start":1794748390.004,"end":1794756369.667,"seconds":7979.62,"run":1832,"passed":1832,"line":"Summary [7979.620s] 1832 tests run: 1832 passed (134 slow), 1 skipped","records":0,"starts":0,"os":0,"otherProfiles":0}

before/linux: e2e mean 39.487358 min; range 37.919033..41.055683; n=2

before/macos: e2e mean 92.244742 min; range 90.112450..94.377033; n=2

before/windows: e2e mean 110.053000 min; range 87.112333..132.993667; n=2


```

For example the command prints Windows baseline e2e mean 110.053000 minutes (two accepted points). This is a measured baseline, not a saving. All source and job citations above apply to every number in that quoted output.

### Prototype, proof and adoption (in progress)

8f51bf37 commits the first contributor-prefilter bypass and harness. Its initial local checks: `cargo check --offline --bin onepipeline` passed; `ONEPIPELINE_STARTUP_CUT=contributors just test-e2e` with the two exact driver journey names in scratch/e2e-cut.log passed both; `bash scripts/engine-startup-verify.sh` passed 6,588 public-API inputs (find, hits, replace, locator screening). Those pre-commit runs exercise the same source committed in 8f51bf37; they are not a claim about the later uncommitted selector/input repairs. Five initial fold tests and the shared reader’s 46 tests passed. The new API equivalence check was observed failing on an intentional mutation returning no matches on the cut path: SECRET yielded baseline [0], cut []; the mutation was restored.

The first judged diff lint, history 20261115T171137Z-8baa9, failed seven rules. The working repair validates the selector, shares its names with the verifier, validates profile tables and cycles, adds nonzero/spread/missing-profile and malformed-profile journeys, and documents command exits. Final judged status is pending.

Cut implementation removes calls to privateterms::normalize::Contributors::new and Contributors::possible in the selected engine processes, including the global source-map initializer and per-matcher mask-table allocation. Full Matcher::scan normalization, candidate stability, folded/exact automata, public-phrase exemptions, hits span mapping, replace and locator screening remain exercised. Added experimental branches include StartupCut::read baseline/contributors/invalid input and the optional prefilter branch; no source function is claimed covered by merged coverage. A long-lived matcher with many negative Unicode texts may lose throughput by doing more full scans; this prototype alone does not justify removing that filter in production.

Production effort if supported by hosted results: one privateterms implementation/validation dispatch, one consumer lock/adoption dispatch in onepipeline. privateterms would release a patch for a perf-only compatible change. onevcs’s privateterms requirement is ^0.1.2, and onetaskgraph-core’s is ^0.1.0, so both admit a privateterms patch without a release of either consumer library; onepipeline updates the privateterms lock entry and releases a perf patch. No change to a serialized schema is proposed. Existing e2e journeys prove the same user behavior; they exercise less of the contributor prefilter and more of the full scanner on its formerly rejected candidates.

Linux merged line coverage is not produced on the baseline sides under c7.0/c9.1: the path-patch audit fails test-rest and Nx skips the coverage merge. Unchanged floor: 95.74 LCOV. Exact log excerpts and the after side remain to be included with the hosted comparison.

### Current handoff

Runs started by this dispatch: none. Runs in flight: none. Session draft: https://github.com/nickderobertis/onepipeline/pull/1621. Baseline means above are read from inherited artifacts; no hosted quota was consumed to remeasure them. Remaining: final local debug/release samples, capacity for three after runs/platform, outcome/cache/coverage evidence, final-source fold and judged lint. No expected saving is claimed before after measurements.

Manager a37 accepts the identically computed trace means on both sides in place of unavailable per-verb test-profile keys, with completed/observed counts and exclusions as printed.

### Local optimized-dependency attribution checkpoint

This capture used the real journey `publish_preserve::a_live_preserve_add_starts_from_the_already_settled_kept_branch`: a start that preserves H, a live add depending on its kept branch, then adopt preserving A and checking its ancestry. It passed under samply 0.13.1. Producer source was the working tree based on 8f51bf37 with the complete staged diff saved in `evidence/local-debug-baseline.source.diff.gz`; its engine/library implementation was subsequently committed in a792bafc. Later parser/test/document changes are not claims of a new engine measurement. The debug binary was built by `cargo test --locked --offline --test e2e --no-run --config 'profile.dev.package."*".opt-level=2' --message-format=json` (scratch optimized-final-build.jsonl): the privateterms artifact reports opt_level=2, while onepipeline reports opt_level=0.

Local recording command: `docker run --rm --privileged -v /home/whoop:/home/whoop -w "$PWD" -e PATH=/home/whoop/.local/share/uv/python/cpython-3.14.7-linux-x86_64-gnu/bin:/home/whoop/.cargo/bin:/usr/local/bin:/usr/bin:/bin -e HOME=/home/whoop -e RUSTC_WRAPPER= -e GIT_CONFIG_COUNT=1 -e GIT_CONFIG_KEY_0=safe.directory -e GIT_CONFIG_VALUE_0="$PWD" -e ONEPIPELINE_PROFILE_UID="$(id -u)" -e ONEPIPELINE_PROFILE_GID="$(id -g)" engine-startup-samply-retry:local bash scripts/engine-startup-profile.sh target/debug/deps/e2e-30df03cb67e6a4b5 "$ONEPIPELINE_NODE_SCRATCH_DIR/local-debug-preserve-before"`. The image digest is 7f7e48035227b4ef282855ede22c65463a28c8c6aeeab3e2f2440ea4f5ded574 (Dockerfile in scratch profiler-image). It adds build-essential, pkg-config, libssl-dev and zlib1g-dev to the earlier profiler image. `setpriv` runs the test as UID/GID 1000; only the sampler needs root/perf privilege. RUSTC_WRAPPER is empty so nested helper compilation does not leave a compiler-cache daemon inside the sampled process tree.

Fold command: `python scripts/engine-startup-cpu.py spikes/engine-startup/evidence/local-debug-baseline.profile.json.gz spikes/engine-startup/evidence/local-debug-baseline.syms.json.gz`. The following excerpt is from that command's output, scratch local-debug-preserve-before.cpu.txt (saved compressed in evidence), for primary engine pids joined to the trace's verbs: pid90=start, pid583=adopt. Forked children may temporarily retain onepipeline's process name; their separate PIDs are not included in these primary-process CPU totals.

```text
pid 90.1: sampled CPU 961.846 ms; lifetime 2492.638 ms; threads 274
nearest first-party module (otherwise leaf):
  193.155 ms (20.08%; cumulative 20.08%) privateterms::normalize
  158.516 ms (16.48%; cumulative 36.56%) onevcs::git
  55.347 ms (5.75%; cumulative 42.32%) onepipeline::ledger
  49.857 ms (5.18%; cumulative 47.50%) onevcs::launch_trace
  46.347 ms (4.82%; cumulative 52.32%) libc.so.6!unresolved
  40.111 ms (4.17%; cumulative 56.49%) poll
  29.802 ms (3.10%; cumulative 59.59%) onemessagebus::schema
  25.352 ms (2.64%; cumulative 62.22%) onepipeline::writeback
  24.916 ms (2.59%; cumulative 64.81%) onepipeline::projection
  24.465 ms (2.54%; cumulative 67.36%) onemessagebus::envelope
  16.153 ms (1.68%; cumulative 69.04%) onepipeline::summary
  14.700 ms (1.53%; cumulative 70.56%) onevcs::home
  14.533 ms (1.51%; cumulative 72.08%) onevcs::lock
  12.303 ms (1.28%; cumulative 73.35%) onepipeline::engine
  11.043 ms (1.15%; cumulative 74.50%) onepipeline::checkpoint
  9.984 ms (1.04%; cumulative 75.54%) onepipeline::plan
  9.071 ms (0.94%; cumulative 76.48%) onepipeline::payload
  7.882 ms (0.82%; cumulative 77.30%) onepipeline::telemetry
  6.953 ms (0.72%; cumulative 78.03%) onepipeline::appendstamp
  6.825 ms (0.71%; cumulative 78.74%) onepipeline::vocabulary
  6.787 ms (0.71%; cumulative 79.44%) onetaskgraph_local_md::yaml
  6.540 ms (0.68%; cumulative 80.12%) onepipeline::driver
nearest first-party function (otherwise leaf):
  193.155 ms (20.08%; cumulative 20.08%) <privateterms::normalize::Contributors>::new::<core::iter::adapters::map::Map<core::slice::iter::Iter<privateterms::resolve::ResolvedRule>, <privateterms::matcher::Matcher>::new::{closure#2}::{closure#0}>>
  61.539 ms (6.40%; cumulative 26.48%) onevcs::git::bounded::<onevcs::git::run_fed::{closure#4}::{closure#3}>
  46.347 ms (4.82%; cumulative 31.30%) libc.so.6!unresolved
  40.111 ms (4.17%; cumulative 35.47%) poll
  27.983 ms (2.91%; cumulative 38.38%) <onevcs::git::PipeCapture>::start::<std::process::ChildStdout>
  27.654 ms (2.88%; cumulative 41.25%) <onemessagebus::schema::Registry>::register_schema
  24.031 ms (2.50%; cumulative 43.75%) onevcs::git::wait_for_exit
  18.661 ms (1.94%; cumulative 45.69%) <onevcs::git::PipeCapture>::start::<std::process::ChildStderr>
  14.714 ms (1.53%; cumulative 47.22%) onevcs::git::run_fed
  14.711 ms (1.53%; cumulative 48.75%) <onevcs::launch_trace::Tracer>::write
  12.149 ms (1.26%; cumulative 50.01%) onevcs::home::ensure_dir
  11.945 ms (1.24%; cumulative 51.26%) onepipeline::ledger::disk::publish
  10.020 ms (1.04%; cumulative 52.30%) onevcs::lock::wait_for
  9.606 ms (1.00%; cumulative 53.30%) <onepipeline::summary::RunSummary>::derive
  8.383 ms (0.87%; cumulative 54.17%) onepipeline::ledger::publish
  8.233 ms (0.86%; cumulative 55.02%) <onepipeline::projection::RunState as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>>
  7.519 ms (0.78%; cumulative 55.81%) <onemessagebus::envelope::Envelope<onepipeline::vocabulary::Agent> as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::SliceRead>>
  7.465 ms (0.78%; cumulative 56.58%) onevcs::launch_trace::sleep
  6.340 ms (0.66%; cumulative 57.24%) onevcs::launch_trace::launch
  6.272 ms (0.65%; cumulative 57.89%) fun_f230
  5.973 ms (0.62%; cumulative 58.51%) <onevcs::launch_trace::Launch>::spawned
  5.806 ms (0.60%; cumulative 59.12%) onetaskgraph_local_md::yaml::from_str::<onetaskgraph_local_md::FrontMatter>
  5.707 ms (0.59%; cumulative 59.71%) onevcs::launch_trace::output
  5.680 ms (0.59%; cumulative 60.30%) <onevcs::git::PipeCapture>::finish
  5.565 ms (0.58%; cumulative 60.88%) onepipeline::writeback::refresh_shadow
  5.304 ms (0.55%; cumulative 61.43%) onepipeline::ledger::disk::sync_contents
  5.161 ms (0.54%; cumulative 61.97%) onetaskgraph_core::resolve::validate_sources
  5.004 ms (0.52%; cumulative 62.49%) onepipeline::driver::attach
  4.628 ms (0.48%; cumulative 62.97%) onemessagebus::envelope::take_or_default::<serde_json::map::Map<alloc::string::String, serde_json::value::Value>, serde_json::error::Error>
  4.509 ms (0.47%; cumulative 63.44%) onepipeline::ledger::disk::read
  4.429 ms (0.46%; cumulative 63.90%) onepipeline::ledger::open_healed
  4.402 ms (0.46%; cumulative 64.36%) onetaskgraph_local_md::staged
  3.819 ms (0.40%; cumulative 64.75%) _munmap
  3.813 ms (0.40%; cumulative 65.15%) _close
  3.642 ms (0.38%; cumulative 65.53%) <onevcs::launch_trace::Wait>::into_launch
  3.312 ms (0.34%; cumulative 65.87%) onevcs::launch_trace::wait
  3.293 ms (0.34%; cumulative 66.21%) onepipeline::ledger::disk::synced
  3.286 ms (0.34%; cumulative 66.56%) <onepipeline::projection::RunState as core::clone::Clone>::clone
  3.282 ms (0.34%; cumulative 66.90%) <onepipeline::vocabulary::Labels as serde_core::de::Deserialize>::deserialize::<serde_json::value::Value>
  3.091 ms (0.32%; cumulative 67.22%) <onepipeline::appendstamp::Stamp as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::StrRead>>
  2.993 ms (0.31%; cumulative 67.53%) onevcs::boundary::derive::derive
  2.891 ms (0.30%; cumulative 67.83%) onepipeline::ledger::records_of
  2.883 ms (0.30%; cumulative 68.13%) onepipeline::journal::stream_queues::<onepipeline::journal::merged_order::{closure#0}>
  2.822 ms (0.29%; cumulative 68.42%) onepipeline::executor::make_node_scratch_dir
  2.810 ms (0.29%; cumulative 68.72%) onepipeline::storeops::record
  2.653 ms (0.28%; cumulative 68.99%) <onepipeline::checkpoint::Projected>::take_with
  2.634 ms (0.27%; cumulative 69.27%) onevcs::native_refs::local_value
  2.628 ms (0.27%; cumulative 69.54%) <onepipeline::ledger::RunPaths>::create
  2.626 ms (0.27%; cumulative 69.81%) <onemessagebus::transport::LocalTransport>::lock
  2.607 ms (0.27%; cumulative 70.08%) <onetaskgraph_local_md::LocalMdSource>::locate
  2.592 ms (0.27%; cumulative 70.35%) onepipeline::storeops::append
  2.583 ms (0.27%; cumulative 70.62%) onevcs::lock::record_owner
  2.568 ms (0.27%; cumulative 70.89%) <onemessagebus::transport::LocalTransport as onemessagebus::transport::Transport>::fingerprint
  2.554 ms (0.27%; cumulative 71.15%) <onepipeline::summary::Maintainer>::derived
  2.514 ms (0.26%; cumulative 71.41%) onepipeline::projection::millis_of
  2.409 ms (0.25%; cumulative 71.67%) <onetaskgraph_local_md::LocalMdSource>::directory
  2.352 ms (0.24%; cumulative 71.91%) <onepipeline::plan::Node as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>>
  2.337 ms (0.24%; cumulative 72.15%) onepipeline::engine::journalling_relay::{closure#0}
  2.282 ms (0.24%; cumulative 72.39%) syscall
  2.257 ms (0.23%; cumulative 72.62%) <onepipeline::writeback::ProjectionWire as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>>
  2.170 ms (0.23%; cumulative 72.85%) <<alloc::collections::btree::map::BTreeMap<_, _> as serde_core::de::Deserialize>::deserialize::MapVisitor<alloc::string::String, alloc::string::String> as serde_core::de::Visitor>::visit_map::<serde_json::de::MapAccess<serde_json::read::StrRead>>
  2.100 ms (0.22%; cumulative 73.07%) onepipeline::report::retain
  2.091 ms (0.22%; cumulative 73.29%) onepipeline::channel::layout::registry
  2.089 ms (0.22%; cumulative 73.50%) <onepipeline::projection::RunState>::settled
  2.053 ms (0.21%; cumulative 73.72%) read
  2.011 ms (0.21%; cumulative 73.93%) _madvise
  1.988 ms (0.21%; cumulative 74.13%) <onetaskgraph_core::engine::Engine>::refuse_private_references::{closure#0}
  1.955 ms (0.20%; cumulative 74.34%) <onevcs::launch_trace::Launch>::exited
  1.928 ms (0.20%; cumulative 74.54%) onepipeline::writeback::opened_by::<core::result::Result<onetaskgraph_core::config::Config, onetaskgraph_core::config::error::ConfigError>>
  1.901 ms (0.20%; cumulative 74.73%) onemessagebus::envelope::take_or_default::<onepipeline::vocabulary::Labels, serde_json::error::Error>
  1.863 ms (0.19%; cumulative 74.93%) <onepipeline::summary::RunSummary as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>, serde_json::ser::PrettyFormatter>>
  1.836 ms (0.19%; cumulative 75.12%) onevcs::native_refs::keep_turn
  1.821 ms (0.19%; cumulative 75.31%) onevcs::policy::parse
  1.816 ms (0.19%; cumulative 75.50%) <onepipeline::appendstamp::Stamp as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>, serde_json::ser::PrettyFormatter>>
  1.747 ms (0.18%; cumulative 75.68%) <onemessagebus::envelope::Envelope<onepipeline::vocabulary::Agent> as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::StrRead>>
  1.724 ms (0.18%; cumulative 75.86%) <onepipeline::payload::HomeStatus as serde_core::ser::Serialize>::serialize::<serde_json::value::ser::Serializer>
  1.712 ms (0.18%; cumulative 76.04%) onepipeline::engine::wait_for_work
  1.702 ms (0.18%; cumulative 76.21%) onepipeline::report::read
  1.683 ms (0.17%; cumulative 76.39%) onepipeline::release::ask_until_dropped
  1.634 ms (0.17%; cumulative 76.56%) <onemessagebus::envelope::Envelope<onepipeline::vocabulary::Agent> as core::clone::Clone>::clone
  1.620 ms (0.17%; cumulative 76.73%) onevcs::lock::open_at
  1.611 ms (0.17%; cumulative 76.89%) <onepipeline::vocabulary::Labels as core::clone::Clone>::clone
  1.597 ms (0.17%; cumulative 77.06%) <alloc::vec::Vec<regex_automata::nfa::thompson::map::Utf8BoundedEntry>>::extend_with
  1.590 ms (0.17%; cumulative 77.22%) onepipeline::engine::converge
  1.571 ms (0.16%; cumulative 77.39%) fun_9c7e0
  1.568 ms (0.16%; cumulative 77.55%) <onemessagebus::envelope::Envelope<onepipeline::vocabulary::Agent> as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>>
  1.535 ms (0.16%; cumulative 77.71%) <onepipeline::telemetry::Aggregate>::fold
  1.513 ms (0.16%; cumulative 77.87%) onepipeline::ledger::read_range
  1.482 ms (0.15%; cumulative 78.02%) <onepipeline::telemetry::Aggregate>::finish
  1.417 ms (0.15%; cumulative 78.17%) onepipeline::projection::fold_record
  1.409 ms (0.15%; cumulative 78.32%) <oneagentgraph::event::OneharnessSession as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>>
  1.392 ms (0.14%; cumulative 78.46%) <onepipeline::executor::LocalExecutor as onepipeline::executor::Executor>::capacity
  1.371 ms (0.14%; cumulative 78.60%) onevcs::home::atomic_write_before_replace::<onevcs::home::atomic_write::{closure#0}>
  1.370 ms (0.14%; cumulative 78.75%) <onepipeline::ledger::Handover>::hold_within
  1.361 ms (0.14%; cumulative 78.89%) <onepipeline::checkpoint::Coverage>::absorb
  1.347 ms (0.14%; cumulative 79.03%) <onepipeline::checkpoint::Projected>::write
  1.345 ms (0.14%; cumulative 79.17%) onepipeline::sys::platform_process_start_token
  1.335 ms (0.14%; cumulative 79.31%) onevcs::boundary::manifests::committed_source
  1.330 ms (0.14%; cumulative 79.44%) onepipeline::checkpoint::fold_in_merge_order
  1.299 ms (0.14%; cumulative 79.58%) onepipeline::projection::derived_already::{closure#0}
  1.286 ms (0.13%; cumulative 79.71%) onepipeline::writeback::snapshot_of::{closure#2}
  1.279 ms (0.13%; cumulative 79.85%) onemessagebus::envelope::take::<u32, serde_json::error::Error>
  1.278 ms (0.13%; cumulative 79.98%) onepipeline::ledger::temporary
  1.262 ms (0.13%; cumulative 80.11%) onevcs::store::load
self function:
  134.023 ms (13.93%; cumulative 13.93%) libc.so.6!unresolved
  50.350 ms (5.23%; cumulative 19.17%) <icu_normalizer::Decomposition<core::iter::sources::once::Once<char>> as core::iter::traits::iterator::Iterator>::next
  45.104 ms (4.69%; cumulative 23.86%) <icu_collections::codepointinvlist::cpinvlist::CodePointInversionList>::contains_query
  41.518 ms (4.32%; cumulative 28.17%) poll
  36.844 ms (3.83%; cumulative 32.01%) mkdir
  27.979 ms (2.91%; cumulative 34.91%) syscall
  27.078 ms (2.82%; cumulative 37.73%) _open64
  25.207 ms (2.62%; cumulative 40.35%) fun_188d80
  18.690 ms (1.94%; cumulative 42.29%) <std::sync::once::Once>::call_once_force::<<std::sync::once_lock::OnceLock<std::collections::hash::map::HashMap<char, alloc::vec::Vec<usize>>>>::initialize<<std::sync::once_lock::OnceLock<std::collections::hash::map::HashMap<char, alloc::vec::Vec<usize>>>>::get_or_init<<privateterms::normalize::Contributors>::new<core::iter::adapters::map::Map<core::slice::iter::Iter<privateterms::resolve::ResolvedRule>, <privateterms::matcher::Matcher>::new::{closure#2}::{closure#0}>>::{closure#0}>::{closure#0}, !>::{closure#0}>::{closure#0}
  17.050 ms (1.77%; cumulative 44.07%) fun_ab630
  14.857 ms (1.54%; cumulative 45.61%) _write
  14.044 ms (1.46%; cumulative 47.07%) rename
  13.777 ms (1.43%; cumulative 48.50%) fun_188cf0
  12.701 ms (1.32%; cumulative 49.82%) statx
  11.403 ms (1.19%; cumulative 51.01%) <icu_normalizer::DecomposingNormalizerBorrowed>::normalize_iter::<core::iter::sources::once::Once<char>>
  9.025 ms (0.94%; cumulative 51.95%) <icu_collections::codepointinvlist::cpinvlist::CodePointInversionList>::contains
  8.939 ms (0.93%; cumulative 52.88%) alloc::vec::in_place_collect::from_iter_in_place::<core::iter::adapters::filter::Filter<core::iter::adapters::filter::Filter<alloc::vec::into_iter::IntoIter<alloc::string::String>, <onepipeline::summary::RunSummary>::derive::{closure#1}>, <onepipeline::summary::RunSummary>::derive::{closure#2}>, alloc::string::String>
  8.603 ms (0.89%; cumulative 53.77%) core::unicode::unicode_data::conversions::lookup
  8.509 ms (0.88%; cumulative 54.66%) fsync
  8.476 ms (0.88%; cumulative 55.54%) _close
  8.128 ms (0.85%; cumulative 56.38%) core::unicode::unicode_data::conversions::to_lower
  7.739 ms (0.80%; cumulative 57.19%) <icu_collections::codepointtrie::cptrie::CodePointTrie<u32>>::get32_by_small_index
  7.383 ms (0.77%; cumulative 57.95%) core::ptr::drop_glue::<std::process::Child>
  6.978 ms (0.73%; cumulative 58.68%) _munmap
  6.844 ms (0.71%; cumulative 59.39%) clock_nanosleep
  6.580 ms (0.68%; cumulative 60.08%) <core::hash::sip::Hasher<core::hash::sip::Sip13Rounds> as core::hash::Hasher>::write
  6.272 ms (0.65%; cumulative 60.73%) fun_f230
  6.124 ms (0.64%; cumulative 61.36%) wait4
  5.961 ms (0.62%; cumulative 61.98%) malloc
  5.793 ms (0.60%; cumulative 62.59%) read
  4.972 ms (0.52%; cumulative 63.10%) _libc_free
  4.696 ms (0.49%; cumulative 63.59%) <core::char::ToLowercase as core::iter::traits::iterator::Iterator>::next
  4.660 ms (0.48%; cumulative 64.08%) _mmap
  4.564 ms (0.47%; cumulative 64.55%) fun_98d50
  3.901 ms (0.41%; cumulative 64.96%) <*const ()>::is_aligned_to
  3.668 ms (0.38%; cumulative 65.34%) <std::path::Components>::parse_next_component_back
  3.416 ms (0.36%; cumulative 65.69%) pipe2
  3.415 ms (0.36%; cumulative 66.05%) core::slice::raw::from_raw_parts::precondition_check
  3.392 ms (0.35%; cumulative 66.40%) pthread_create
  3.117 ms (0.32%; cumulative 66.72%) fun_188620
  3.007 ms (0.31%; cumulative 67.04%) _mprotect
  2.717 ms (0.28%; cumulative 67.32%) jsonschema::keywords::ref_::extract_ref_target_base
  2.544 ms (0.26%; cumulative 67.58%) serde_json::ser::format_escaped_str_contents::<&mut alloc::vec::Vec<u8>, serde_json::ser::CompactFormatter>
  2.541 ms (0.26%; cumulative 67.85%) fun_ab230
  2.520 ms (0.26%; cumulative 68.11%) getdents64
  2.478 ms (0.26%; cumulative 68.37%) core::sync::atomic::atomic_compare_exchange::<u32>
  2.460 ms (0.26%; cumulative 68.62%) fun_a9c70
  2.436 ms (0.25%; cumulative 68.88%) _open64_nocancel
  2.239 ms (0.23%; cumulative 69.11%) core::fmt::write
  2.197 ms (0.23%; cumulative 69.34%) core::ptr::copy_nonoverlapping::precondition_check
  2.185 ms (0.23%; cumulative 69.56%) <serde_json::de::MapAccess<serde_json::read::StrRead> as serde_core::de::MapAccess>::next_value::<core::option::Option<(u64, u64, i64, i64)>>
  2.170 ms (0.23%; cumulative 69.79%) <<alloc::collections::btree::map::BTreeMap<_, _> as serde_core::de::Deserialize>::deserialize::MapVisitor<alloc::string::String, alloc::string::String> as serde_core::de::Visitor>::visit_map::<serde_json::de::MapAccess<serde_json::read::StrRead>>
  2.011 ms (0.21%; cumulative 70.00%) _madvise
  1.988 ms (0.21%; cumulative 70.21%) <onetaskgraph_core::engine::Engine>::refuse_private_references::{closure#0}
  1.975 ms (0.21%; cumulative 70.41%) ftruncate
  1.812 ms (0.19%; cumulative 70.60%) _cxa_thread_atexit_impl
  1.744 ms (0.18%; cumulative 70.78%) fstatat
  1.724 ms (0.18%; cumulative 70.96%) <onepipeline::payload::HomeStatus as serde_core::ser::Serialize>::serialize::<serde_json::value::ser::Serializer>
  1.597 ms (0.17%; cumulative 71.13%) <alloc::vec::Vec<regex_automata::nfa::thompson::map::Utf8BoundedEntry>>::extend_with
  1.571 ms (0.16%; cumulative 71.29%) fun_9c7e0
  1.524 ms (0.16%; cumulative 71.45%) unlinkat
  1.464 ms (0.15%; cumulative 71.60%) core::sync::atomic::atomic_load::<usize>
  1.455 ms (0.15%; cumulative 71.75%) serde_json::ser::format_escaped_str_contents::<&mut alloc::vec::Vec<u8>, serde_json::ser::PrettyFormatter>
  1.448 ms (0.15%; cumulative 71.90%) <core::iter::adapters::zip::Zip<core::slice::iter::Iter<u8>, core::slice::iter::Iter<u8>> as core::iter::adapters::zip::TrustedRandomAccessNoCoerce>::size
  1.366 ms (0.14%; cumulative 72.04%) <alloc::vec::Vec<u8> as alloc::vec::spec_extend::SpecExtend<&u8, core::slice::iter::Iter<u8>>>::spec_extend
  1.361 ms (0.14%; cumulative 72.19%) <core::option::Option<onepipeline::checkpoint::Placed>>::as_ref
  1.349 ms (0.14%; cumulative 72.33%) core::ptr::drop_glue::<alloc::vec::Vec<u8>>
  1.343 ms (0.14%; cumulative 72.47%) <usize>::unchecked_add::precondition_check
  1.328 ms (0.14%; cumulative 72.60%) unlink
  1.320 ms (0.14%; cumulative 72.74%) <serde_saphyr::de::deserializer::YamlDeserializer>::peek_scalar_view
  1.299 ms (0.14%; cumulative 72.88%) <alloc::collections::btree::node::Handle<alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Immut, onepipeline::graph::NodeRef, onepipeline::graph::Settled, alloc::collections::btree::node::marker::Leaf>, alloc::collections::btree::node::marker::Edge>>::next_unchecked::{closure#0}
  1.294 ms (0.13%; cumulative 73.01%) <hashbrown::raw::RawTable<usize>>::insert_tagged_at_index
  1.261 ms (0.13%; cumulative 73.14%) unresolved
  1.247 ms (0.13%; cumulative 73.27%) core::slice::sort::shared::smallsort::insert_tail::<onetaskgraph_core::config::layer::Setting, <[onetaskgraph_core::config::layer::Setting]>::sort_by<<onetaskgraph_core::config::effective::EffectiveConfig>::new::{closure#1}>::{closure#0}>
  1.216 ms (0.13%; cumulative 73.40%) core::hash::sip::u8to64_le
  1.216 ms (0.13%; cumulative 73.52%) <indexmap::inner::Core<alloc::string::String, serde_json::value::Value>>::push_entry
  1.204 ms (0.13%; cumulative 73.65%) <hashbrown::raw::RawTableInner>::ctrl_slice
  1.197 ms (0.12%; cumulative 73.77%) <str as core::fmt::Display>::fmt
  1.195 ms (0.12%; cumulative 73.90%) <alloc::raw_vec::RawVecInner>::finish_grow
  1.170 ms (0.12%; cumulative 74.02%) <hashbrown::raw::RawTableInner>::fallible_with_capacity::<allocator_api2::stable::alloc::global::Global>
  1.146 ms (0.12%; cumulative 74.14%) <std::sys::process::unix::common::Command>::spawn
  1.134 ms (0.12%; cumulative 74.26%) fun_a9b90
  1.129 ms (0.12%; cumulative 74.37%) <std::thread::spawnhook::SpawnHooks as core::ops::drop::Drop>::drop
  1.125 ms (0.12%; cumulative 74.49%) onevcs::git::run_fed
  1.123 ms (0.12%; cumulative 74.61%) fun_ad010
  1.070 ms (0.11%; cumulative 74.72%) <&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>> as serde_core::ser::Serializer>::collect_map::<&onepipeline::writeback::StoreCall, &core::num::nonzero::NonZero<u64>, &alloc::collections::btree::map::BTreeMap<onepipeline::writeback::StoreCall, core::num::nonzero::NonZero<u64>>>
  1.068 ms (0.11%; cumulative 74.83%) <serde_json::read::SliceRead>::skip_to_escape
  1.066 ms (0.11%; cumulative 74.94%) clock_gettime
  1.059 ms (0.11%; cumulative 75.05%) <fluent_uri::parse::Reader>::read
  1.030 ms (0.11%; cumulative 75.16%) readlink
  1.025 ms (0.11%; cumulative 75.26%) core::core_arch::x86::sse2::_mm_movemask_epi8
  0.960 ms (0.10%; cumulative 75.36%) std::sys::backtrace::__rust_begin_short_backtrace::<<onevcs::git::PipeCapture>::start<std::process::ChildStdout>::{closure#0}, onevcs::git::Captured>
  0.958 ms (0.10%; cumulative 75.46%) <alloc::vec::Vec<onepipeline::release::Dependency>>::is_empty
  0.957 ms (0.10%; cumulative 75.56%) <&str as core::fmt::Display>::fmt
  0.950 ms (0.10%; cumulative 75.66%) <indexmap::inner::Core<alloc::string::String, serde_json::value::Value>>::get_index_of::<str>
  0.945 ms (0.10%; cumulative 75.76%) <alloc::string::String as core::fmt::Write>::write_str
  0.944 ms (0.10%; cumulative 75.86%) clap_builder::builder::debug_asserts::assert_app
  0.941 ms (0.10%; cumulative 75.96%) onevcs::launch_trace::launch
  0.939 ms (0.10%; cumulative 76.05%) <core::hash::sip::Sip13Rounds as core::hash::sip::Sip>::c_rounds
  0.931 ms (0.10%; cumulative 76.15%) <&mut serde_json::de::Deserializer<serde_json::read::SliceRead> as serde_core::de::Deserializer>::deserialize_any::<<serde_json::value::Value as serde_core::de::Deserialize>::deserialize::ValueVisitor>
  0.925 ms (0.10%; cumulative 76.25%) <indexmap::map::entry::Entry<alloc::string::String, serde_json::value::Value>>::new
  0.918 ms (0.10%; cumulative 76.34%) <core::str::lossy::Utf8Chunks as core::iter::traits::iterator::Iterator>::next
  0.918 ms (0.10%; cumulative 76.44%) <hashbrown::map::HashMap<alloc::string::String, jsonschema::node::SchemaNode, ahash::random_state::RandomState>>::get::<str>
  0.909 ms (0.09%; cumulative 76.53%) __rustc::__rdl_alloc
  0.900 ms (0.09%; cumulative 76.63%) __rustc::__rust_no_alloc_shim_is_unstable_v2
  0.895 ms (0.09%; cumulative 76.72%) _libc_calloc
  0.892 ms (0.09%; cumulative 76.81%) <alloc::raw_vec::RawVecInner>::try_allocate_in
  0.881 ms (0.09%; cumulative 76.90%) _getauxval
  0.861 ms (0.09%; cumulative 76.99%) <jsonschema::keywords::properties::SmallPropertiesValidator as jsonschema::validator::Validate>::validate
  0.854 ms (0.09%; cumulative 77.08%) getenv
  0.852 ms (0.09%; cumulative 77.17%) <referencing::resource::ResourceRef>::pointer
  0.838 ms (0.09%; cumulative 77.26%) <&mut alloc::vec::Vec<u8> as std::io::Write>::write_all
  0.832 ms (0.09%; cumulative 77.34%) <alloc::raw_vec::RawVecInner>::deallocate
  0.827 ms (0.09%; cumulative 77.43%) <core::str::pattern::CharSearcher as core::str::pattern::Searcher>::next_match
  0.809 ms (0.08%; cumulative 77.51%) flock
  0.803 ms (0.08%; cumulative 77.60%) core::ub_checks::maybe_is_nonoverlapping::runtime
  0.802 ms (0.08%; cumulative 77.68%) _close_nocancel
  0.796 ms (0.08%; cumulative 77.76%) <hashbrown::raw::RawTableInner>::drop_inner_table::<usize, allocator_api2::stable::alloc::global::Global>
  0.792 ms (0.08%; cumulative 77.85%) <onepipeline::projection::RunState as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>>
  0.785 ms (0.08%; cumulative 77.93%) <<onepipeline::ledger::LaunchRecord as serde_core::de::Deserialize>::deserialize::__Visitor as serde_core::de::Visitor>::visit_map::<serde_json::de::MapAccess<serde_json::read::StrRead>>
  0.768 ms (0.08%; cumulative 78.01%) <onepipeline::cli::Command as clap_builder::derive::Subcommand>::augment_subcommands
  0.768 ms (0.08%; cumulative 78.09%) <<serde_json::value::Value as serde_core::de::Deserialize>::deserialize::ValueVisitor as serde_core::de::Visitor>::visit_map::<serde_json::de::MapAccess<serde_json::read::StrRead>>
  0.756 ms (0.08%; cumulative 78.17%) core::ptr::drop_glue::<std::io::error::Error>
  0.743 ms (0.08%; cumulative 78.24%) realpath
  0.734 ms (0.08%; cumulative 78.32%) fun_18bac0
  0.725 ms (0.08%; cumulative 78.39%) <alloc::collections::btree::node::Handle<alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Dying, alloc::string::String, alloc::string::String, alloc::collections::btree::node::marker::Leaf>, alloc::collections::btree::node::marker::Edge>>::deallocating_next::<alloc::alloc::Global>
  0.717 ms (0.07%; cumulative 78.47%) <u64 as itoa::Unsigned>::fmt
  0.703 ms (0.07%; cumulative 78.54%) fun_177ce0
  0.702 ms (0.07%; cumulative 78.62%) fun_25da0
  0.699 ms (0.07%; cumulative 78.69%) <std::time::Instant>::now
  0.698 ms (0.07%; cumulative 78.76%) <jsonschema::node::SchemaNode as jsonschema::validator::Validate>::validate
  0.697 ms (0.07%; cumulative 78.83%) onepipeline::writeback::task_document
  0.694 ms (0.07%; cumulative 78.91%) <serde::private::de::FlatMapDeserializer<serde_json::error::Error> as serde_core::de::Deserializer>::deserialize_map::<<serde_json::map::Map<alloc::string::String, serde_json::value::Value> as serde_core::de::Deserialize>::deserialize::Visitor>
  0.692 ms (0.07%; cumulative 78.98%) onepipeline::driver::start
  0.691 ms (0.07%; cumulative 79.05%) <str as serde_json::value::index::Index>::index_into
  0.687 ms (0.07%; cumulative 79.12%) <core::fmt::Formatter>::write_str
  0.687 ms (0.07%; cumulative 79.19%) fun_11ca00
  0.685 ms (0.07%; cumulative 79.26%) <onevcs::checkout_reads::Lease>::acquire
  0.672 ms (0.07%; cumulative 79.33%) <core::result::Result<serde_json::value::Value, serde_json::error::Error>>::map_err::<onepipeline::error::Error, onepipeline::vcs::relayed::{closure#0}>
  0.670 ms (0.07%; cumulative 79.40%) pthread_mutex_lock
  0.665 ms (0.07%; cumulative 79.47%) <u32 as core::fmt::Display>::fmt
  0.648 ms (0.07%; cumulative 79.54%) <jsonschema::keywords::ref_::DirectRefValidator<jsonschema_value::serde_json::SerdeJson> as jsonschema::validator::Validate>::validate
  0.648 ms (0.07%; cumulative 79.61%) <serde_json::value::ser::Serializer as serde_core::ser::Serializer>::serialize_map
  0.641 ms (0.07%; cumulative 79.67%) <alloc::collections::btree::map::BTreeMap<alloc::string::String, alloc::string::String> as core::ops::drop::Drop>::drop
  0.634 ms (0.07%; cumulative 79.74%) fun_98690
  0.629 ms (0.07%; cumulative 79.80%) <hashbrown::raw::RawTable<usize> as core::clone::Clone>::clone_from
  0.628 ms (0.07%; cumulative 79.87%) core::ptr::drop_glue::<core::option::Option<alloc::string::String>>
  0.613 ms (0.06%; cumulative 79.93%) fun_189710
  0.610 ms (0.06%; cumulative 80.00%) core::str::converts::from_utf8
  0.590 ms (0.06%; cumulative 80.06%) <std::sync::poison::Flag>::done
CPU attribution only: child lifetimes, filesystem latency, waits and process creation are not disjointly measured here.

pid 583.1: sampled CPU 873.577 ms; lifetime 2950.168 ms; threads 332
nearest first-party module (otherwise leaf):
  174.754 ms (20.00%; cumulative 20.00%) onevcs::git
  64.765 ms (7.41%; cumulative 27.42%) onepipeline::ledger
  57.239 ms (6.55%; cumulative 33.97%) libc.so.6!unresolved
  48.344 ms (5.53%; cumulative 39.50%) onepipeline::projection
  46.352 ms (5.31%; cumulative 44.81%) onevcs::launch_trace
  43.772 ms (5.01%; cumulative 49.82%) poll
  40.731 ms (4.66%; cumulative 54.48%) onemessagebus::envelope
  29.772 ms (3.41%; cumulative 57.89%) onepipeline::writeback
  20.464 ms (2.34%; cumulative 60.23%) onemessagebus::schema
  17.779 ms (2.04%; cumulative 62.27%) onevcs::lock
  17.684 ms (2.02%; cumulative 64.29%) onepipeline::checkpoint
  16.706 ms (1.91%; cumulative 66.21%) onepipeline::plan
  14.159 ms (1.62%; cumulative 67.83%) onepipeline::summary
  13.100 ms (1.50%; cumulative 69.33%) onepipeline::graph
  12.755 ms (1.46%; cumulative 70.79%) onepipeline::engine
  12.297 ms (1.41%; cumulative 72.19%) onepipeline::vocabulary
  11.840 ms (1.36%; cumulative 73.55%) onevcs::checkout_reads
  10.387 ms (1.19%; cumulative 74.74%) onetaskgraph_local_md::yaml
  9.743 ms (1.12%; cumulative 75.85%) onepipeline::driver
  8.929 ms (1.02%; cumulative 76.88%) onetaskgraph_local_md::LocalMdSource>
  8.352 ms (0.96%; cumulative 77.83%) onepipeline::telemetry
  7.910 ms (0.91%; cumulative 78.74%) onepipeline::payload
  7.572 ms (0.87%; cumulative 79.60%) onetaskgraph_core::engine
  7.143 ms (0.82%; cumulative 80.42%) onetaskgraph_local_md::staged
nearest first-party function (otherwise leaf):
  66.021 ms (7.56%; cumulative 7.56%) onevcs::git::bounded::<onevcs::git::run_fed::{closure#4}::{closure#3}>
  57.239 ms (6.55%; cumulative 14.11%) libc.so.6!unresolved
  43.772 ms (5.01%; cumulative 19.12%) poll
  31.984 ms (3.66%; cumulative 22.78%) <onevcs::git::PipeCapture>::start::<std::process::ChildStdout>
  25.017 ms (2.86%; cumulative 25.65%) onevcs::git::wait_for_exit
  20.418 ms (2.34%; cumulative 27.98%) <onevcs::git::PipeCapture>::start::<std::process::ChildStderr>
  18.529 ms (2.12%; cumulative 30.10%) <onemessagebus::schema::Registry>::register_schema
  17.342 ms (1.99%; cumulative 32.09%) onevcs::git::run_fed
  15.514 ms (1.78%; cumulative 33.86%) <onevcs::launch_trace::Tracer>::write
  15.088 ms (1.73%; cumulative 35.59%) <onepipeline::projection::RunState as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>>
  14.225 ms (1.63%; cumulative 37.22%) onepipeline::ledger::disk::publish
  11.725 ms (1.34%; cumulative 38.56%) onepipeline::ledger::publish
  10.815 ms (1.24%; cumulative 39.80%) onevcs::lock::wait_for
  9.977 ms (1.14%; cumulative 40.94%) onetaskgraph_local_md::yaml::from_str::<onetaskgraph_local_md::FrontMatter>
  9.275 ms (1.06%; cumulative 42.00%) onepipeline::driver::attach
  8.776 ms (1.00%; cumulative 43.01%) <onepipeline::projection::RunState as core::clone::Clone>::clone
  8.020 ms (0.92%; cumulative 43.93%) <onemessagebus::envelope::Envelope<onepipeline::vocabulary::Agent> as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::SliceRead>>
  7.951 ms (0.91%; cumulative 44.84%) <onemessagebus::envelope::Envelope<onepipeline::vocabulary::Agent> as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::StrRead>>
  7.143 ms (0.82%; cumulative 45.65%) onetaskgraph_local_md::staged
  7.134 ms (0.82%; cumulative 46.47%) onepipeline::ledger::disk::read
  6.931 ms (0.79%; cumulative 47.26%) <onevcs::git::PipeCapture>::finish
  6.891 ms (0.79%; cumulative 48.05%) <onevcs::launch_trace::Launch>::spawned
  6.743 ms (0.77%; cumulative 48.83%) fun_f230
  6.066 ms (0.69%; cumulative 49.52%) <onepipeline::checkpoint::Projected>::take_with
  5.953 ms (0.68%; cumulative 50.20%) onevcs::checkout_reads::managed
  5.761 ms (0.66%; cumulative 50.86%) <onevcs::launch_trace::Wait>::into_launch
  5.472 ms (0.63%; cumulative 51.49%) <onepipeline::plan::Node as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>>
  5.297 ms (0.61%; cumulative 52.09%) <onepipeline::vocabulary::Labels as serde_core::de::Deserialize>::deserialize::<serde_json::value::Value>
  5.146 ms (0.59%; cumulative 52.68%) onepipeline::ledger::write_atomic
  5.093 ms (0.58%; cumulative 53.27%) _munmap
  4.873 ms (0.56%; cumulative 53.82%) onepipeline::ledger::disk::sync_contents
  4.651 ms (0.53%; cumulative 54.36%) onepipeline::ledger::read_range
  4.278 ms (0.49%; cumulative 54.85%) <onepipeline::projection::RunState>::settled
  4.235 ms (0.48%; cumulative 55.33%) onevcs::launch_trace::sleep
  4.046 ms (0.46%; cumulative 55.79%) <onemessagebus::envelope::ArtifactRef as schemars::JsonSchema>::json_schema
  4.022 ms (0.46%; cumulative 56.25%) onepipeline::executor::make_node_scratch_dir
  4.000 ms (0.46%; cumulative 56.71%) <onemessagebus::envelope::Envelope<onepipeline::vocabulary::Agent> as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>>
  3.862 ms (0.44%; cumulative 57.15%) <onepipeline::summary::RunSummary as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>, serde_json::ser::PrettyFormatter>>
  3.770 ms (0.43%; cumulative 57.59%) onevcs::launch_trace::launch
  3.687 ms (0.42%; cumulative 58.01%) onevcs::native_refs::local_value
  3.673 ms (0.42%; cumulative 58.43%) onemessagebus::envelope::take_or_default::<serde_json::map::Map<alloc::string::String, serde_json::value::Value>, serde_json::error::Error>
  3.660 ms (0.42%; cumulative 58.85%) <onetaskgraph_local_md::LocalMdSource>::directory
  3.638 ms (0.42%; cumulative 59.26%) <onepipeline::summary::Maintainer>::derived
  3.578 ms (0.41%; cumulative 59.67%) _close
  3.540 ms (0.41%; cumulative 60.08%) onevcs::lock::record_owner
  3.490 ms (0.40%; cumulative 60.48%) syscall
  3.425 ms (0.39%; cumulative 60.87%) onevcs::policy::parse
  3.241 ms (0.37%; cumulative 61.24%) <onepipeline::taskgraph::Store>::project_metadata_asking
  3.160 ms (0.36%; cumulative 61.60%) onetaskgraph_core::resolve::validate_sources
  3.149 ms (0.36%; cumulative 61.96%) <onepipeline::ledger::LaunchRecord as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::StrRead>>
  3.082 ms (0.35%; cumulative 62.32%) <onepipeline::plan::Node as serde_core::ser::Serialize>::serialize::<serde_json::value::ser::Serializer>
  3.071 ms (0.35%; cumulative 62.67%) onevcs::home::ensure_dir
  2.976 ms (0.34%; cumulative 63.01%) <onepipeline::graph::Graph as core::cmp::PartialEq>::eq
  2.937 ms (0.34%; cumulative 63.34%) onevcs::lock::open_at
  2.885 ms (0.33%; cumulative 63.67%) <onetaskgraph_local_md::LocalMdSource>::read_text
  2.855 ms (0.33%; cumulative 64.00%) onepipeline::writeback::worker
  2.802 ms (0.32%; cumulative 64.32%) onevcs::launch_trace::wait
  2.798 ms (0.32%; cumulative 64.64%) <onevcs::checkout_reads::Hold as core::ops::drop::Drop>::drop
  2.704 ms (0.31%; cumulative 64.95%) <onevcs::launch_trace::Launch>::exited
  2.649 ms (0.30%; cumulative 65.26%) onevcs::launch_trace::output
  2.576 ms (0.29%; cumulative 65.55%) <onepipeline::summary::RunSummary>::derive
  2.559 ms (0.29%; cumulative 65.84%) <onepipeline::vocabulary::Labels as core::clone::Clone>::clone
  2.536 ms (0.29%; cumulative 66.13%) onepipeline::ledger::disk::synced
  2.375 ms (0.27%; cumulative 66.41%) <onepipeline::graph::AsWritten as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>>
  2.361 ms (0.27%; cumulative 66.68%) onepipeline::writeback::write_shadow
  2.347 ms (0.27%; cumulative 66.94%) onepipeline::engine::wait_for_work
  2.203 ms (0.25%; cumulative 67.20%) <onepipeline::ledger::RunPaths>::journal
  2.140 ms (0.24%; cumulative 67.44%) onepipeline::projection::fold_recall
  2.133 ms (0.24%; cumulative 67.69%) onepipeline::engine::converge
  2.111 ms (0.24%; cumulative 67.93%) onevcs::workspace::existing
  2.066 ms (0.24%; cumulative 68.16%) onemessagebus::envelope::take::<u32, serde_json::error::Error>
  2.017 ms (0.23%; cumulative 68.39%) onepipeline::projection::millis_of
  1.916 ms (0.22%; cumulative 68.61%) <onepipeline::graph::Graph as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>>
  1.887 ms (0.22%; cumulative 68.83%) onepipeline::checkpoint::length_of
  1.879 ms (0.22%; cumulative 69.04%) <alloc::vec::Vec<regex_automata::nfa::thompson::map::Utf8BoundedEntry>>::extend_with
  1.849 ms (0.21%; cumulative 69.26%) _madvise
  1.832 ms (0.21%; cumulative 69.47%) onevcs::home::atomic_write_before_replace::<onevcs::home::atomic_write::{closure#0}>
  1.814 ms (0.21%; cumulative 69.67%) <onepipeline::plan::Node as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::StrRead>>
  1.807 ms (0.21%; cumulative 69.88%) onepipeline::writeback::task_document
  1.740 ms (0.20%; cumulative 70.08%) onepipeline::journal::merge_queues::<onepipeline::journal::merged_order::{closure#0}>
  1.714 ms (0.20%; cumulative 70.28%) <onemessagebus::envelope::Envelope<onepipeline::vocabulary::Agent> as core::clone::Clone>::clone
  1.689 ms (0.19%; cumulative 70.47%) onepipeline::writeback::carry_the_difference
  1.672 ms (0.19%; cumulative 70.66%) read
  1.649 ms (0.19%; cumulative 70.85%) onepipeline::projection::fold_record
  1.601 ms (0.18%; cumulative 71.03%) onepipeline::journal::stream_queues::<onepipeline::journal::merged_order::{closure#0}>
  1.547 ms (0.18%; cumulative 71.21%) onepipeline::checkpoint::fold_in_merge_order
  1.547 ms (0.18%; cumulative 71.39%) <onepipeline::storeops::line::Line as serde_core::ser::Serialize>::serialize::<serde_json::value::ser::Serializer>
  1.543 ms (0.18%; cumulative 71.56%) <onemessagebus::transport::LocalTransport as onemessagebus::transport::Transport>::fingerprint
  1.536 ms (0.18%; cumulative 71.74%) <onepipeline::graph::Graph as core::clone::Clone>::clone
  1.532 ms (0.18%; cumulative 71.92%) onepipeline::writeback::opened_by::<core::result::Result<onetaskgraph_core::config::Config, onetaskgraph_core::config::error::ConfigError>>
  1.520 ms (0.17%; cumulative 72.09%) onevcs::store::load
  1.510 ms (0.17%; cumulative 72.26%) onevcs::checkout_reads::relative_checkout
  1.484 ms (0.17%; cumulative 72.43%) onepipeline::crossdag::resolve_with
  1.473 ms (0.17%; cumulative 72.60%) <onetaskgraph_local_md::LocalMdSource>::confined
  1.470 ms (0.17%; cumulative 72.77%) <oneagentgraph::event::OneharnessSession as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>>
  1.440 ms (0.16%; cumulative 72.93%) onepipeline::release::ask_until_dropped
  1.438 ms (0.16%; cumulative 73.10%) onepipeline::writeback::refresh_shadow
  1.434 ms (0.16%; cumulative 73.26%) onepipeline::channel::layout::registry
  1.412 ms (0.16%; cumulative 73.42%) <onemessagebus::envelope::Envelope<onevcs::vocabulary::VcsEvents> as serde_core::ser::Serialize>::serialize::<serde_json::value::ser::Serializer>
  1.408 ms (0.16%; cumulative 73.59%) onevcs::git::lfs::repository_configuration
  1.401 ms (0.16%; cumulative 73.75%) <onepipeline::summary::Maintainer>::opened_for
  1.391 ms (0.16%; cumulative 73.90%) <onepipeline::plan::Node as core::clone::Clone>::clone
  1.380 ms (0.16%; cumulative 74.06%) <onepipeline::telemetry::Usage as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>, serde_json::ser::PrettyFormatter>>
  1.373 ms (0.16%; cumulative 74.22%) <oneagentgraph::event::OneharnessSession as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::StrRead>>
  1.372 ms (0.16%; cumulative 74.38%) <onepipeline::projection::RunState as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>, serde_json::ser::PrettyFormatter>>
  1.354 ms (0.15%; cumulative 74.53%) onepipeline::yaml::from_str::<serde_json::value::Value>
  1.340 ms (0.15%; cumulative 74.69%) onepipeline::scratch::graph_ending
  1.329 ms (0.15%; cumulative 74.84%) onepipeline::projection::fold_activity
  1.324 ms (0.15%; cumulative 74.99%) <onemessagebus::redact::Redactor>::from_env
  1.316 ms (0.15%; cumulative 75.14%) onepipeline::home::recorded
  1.288 ms (0.15%; cumulative 75.29%) <onepipeline::plan::TaskRecord as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::StrRead>>
  1.283 ms (0.15%; cumulative 75.43%) onepipeline::report::failed_verdicts
  1.277 ms (0.15%; cumulative 75.58%) <onepipeline::projection::SessionOpened as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>>
  1.274 ms (0.15%; cumulative 75.73%) <onepipeline::writeback::Snapshot as core::clone::Clone>::clone
  1.261 ms (0.14%; cumulative 75.87%) <onepipeline::checkpoint::Tally as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>>
  1.257 ms (0.14%; cumulative 76.01%) onevcs::git::make_executable
  1.256 ms (0.14%; cumulative 76.16%) <onepipeline::plan::Node as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>, serde_json::ser::PrettyFormatter>>
  1.217 ms (0.14%; cumulative 76.30%) fun_ab630
  1.197 ms (0.14%; cumulative 76.43%) onevcs::yaml::convert::<u64>
  1.158 ms (0.13%; cumulative 76.57%) onepipeline::engine::journalling_relay::{closure#0}
  1.152 ms (0.13%; cumulative 76.70%) <<onepipeline::projection::RunState as serde_core::de::Deserialize>::deserialize::__Visitor as serde_core::de::Visitor>::visit_map::<serde_json::de::MapAccess<serde_json::read::StrRead>>
  1.135 ms (0.13%; cumulative 76.83%) <onepipeline::telemetry::RunTelemetry as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>, serde_json::ser::PrettyFormatter>>
  1.135 ms (0.13%; cumulative 76.96%) <onepipeline::appendstamp::Receipt as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::StrRead>>
  1.112 ms (0.13%; cumulative 77.09%) onemessagebus::envelope::take_or_default::<alloc::vec::Vec<onemessagebus::envelope::ArtifactRef>, serde_json::error::Error>
  1.075 ms (0.12%; cumulative 77.21%) _mmap
  1.070 ms (0.12%; cumulative 77.33%) <onepipeline::graph::Graph>::get
  1.049 ms (0.12%; cumulative 77.45%) fun_9c7e0
  1.047 ms (0.12%; cumulative 77.57%) onepipeline::writeback::document
  1.030 ms (0.12%; cumulative 77.69%) <onepipeline::agentgraph::WrittenAddress as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>>
  1.027 ms (0.12%; cumulative 77.81%) <onepipeline::vocabulary::Dimensions as serde_core::de::Deserialize>::deserialize::<serde_json::value::Value>
  1.023 ms (0.12%; cumulative 77.92%) onepipeline::summary::seal
  1.015 ms (0.12%; cumulative 78.04%) onepipeline::checkpoint::digested
  1.013 ms (0.12%; cumulative 78.16%) <onepipeline::vocabulary::Source as serde_core::de::Deserialize>::deserialize::<serde_json::value::Value>
  1.012 ms (0.12%; cumulative 78.27%) <onepipeline::graph::Graph>::iter::{closure#0}
  1.010 ms (0.12%; cumulative 78.39%) _mprotect
  1.002 ms (0.11%; cumulative 78.50%) <str as core::cmp::PartialEq>::eq
  0.989 ms (0.11%; cumulative 78.62%) onepipeline::views::summarize
  0.988 ms (0.11%; cumulative 78.73%) <onepipeline::plan::Node as core::cmp::PartialEq>::eq
  0.986 ms (0.11%; cumulative 78.84%) onepipeline::ledger::read_lock_file
  0.983 ms (0.11%; cumulative 78.95%) <onepipeline::checkpoint::Projected>::write
  0.969 ms (0.11%; cumulative 79.06%) <onetaskgraph_core::engine::Engine>::copy::{closure#0}
  0.963 ms (0.11%; cumulative 79.18%) <onepipeline::projection::ServiceRecord as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::StrRead>>
  0.962 ms (0.11%; cumulative 79.29%) <serde_json::read::SliceRead as serde_json::read::Read>::peek
  0.942 ms (0.11%; cumulative 79.39%) <onepipeline::telemetry::Aggregate as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::StrRead>>
  0.942 ms (0.11%; cumulative 79.50%) <onepipeline::unanswered::SourceHolds>::record
  0.942 ms (0.11%; cumulative 79.61%) core::ptr::drop_glue::<std::io::error::Error>
  0.941 ms (0.11%; cumulative 79.72%) onepipeline::projection::fold_addresses
  0.941 ms (0.11%; cumulative 79.82%) onepipeline::ledger::read_json_opt::<serde_json::value::Value>::{closure#1}
  0.941 ms (0.11%; cumulative 79.93%) <onepipeline::telemetry::Usage as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>>
  0.940 ms (0.11%; cumulative 80.04%) onepipeline::ledger::records_of
self function:
  156.050 ms (17.86%; cumulative 17.86%) libc.so.6!unresolved
  44.265 ms (5.07%; cumulative 22.93%) poll
  31.456 ms (3.60%; cumulative 26.53%) _open64
  29.131 ms (3.33%; cumulative 29.87%) syscall
  17.124 ms (1.96%; cumulative 31.83%) rename
  16.687 ms (1.91%; cumulative 33.74%) _write
  15.788 ms (1.81%; cumulative 35.54%) statx
  15.011 ms (1.72%; cumulative 37.26%) mkdir
  12.504 ms (1.43%; cumulative 38.69%) _close
  12.128 ms (1.39%; cumulative 40.08%) core::slice::raw::from_raw_parts::precondition_check
  10.464 ms (1.20%; cumulative 41.28%) fun_ab630
  9.600 ms (1.10%; cumulative 42.38%) _munmap
  9.105 ms (1.04%; cumulative 43.42%) wait4
  9.063 ms (1.04%; cumulative 44.46%) read
  8.955 ms (1.03%; cumulative 45.48%) fsync
  8.070 ms (0.92%; cumulative 46.41%) malloc
  7.974 ms (0.91%; cumulative 47.32%) fun_188d80
  6.743 ms (0.77%; cumulative 48.09%) fun_f230
  6.588 ms (0.75%; cumulative 48.85%) <*const ()>::is_aligned_to
  6.350 ms (0.73%; cumulative 49.57%) fun_188cf0
  6.074 ms (0.70%; cumulative 50.27%) _mmap
  5.742 ms (0.66%; cumulative 50.93%) <indexmap::inner::Core<alloc::string::String, serde_json::value::Value>>::insert_full
  5.626 ms (0.64%; cumulative 51.57%) _libc_free
  5.101 ms (0.58%; cumulative 52.15%) fun_98d50
  4.486 ms (0.51%; cumulative 52.67%) core::ptr::copy_nonoverlapping::precondition_check
  4.200 ms (0.48%; cumulative 53.15%) <std::thread::local::LocalKey<core::cell::RefCell<(std::path::PathBuf, std::collections::hash::map::HashMap<std::path::PathBuf, core::option::Option<onevcs::checkout_reads::Managed>>)>>>::with::<onevcs::checkout_reads::managed::{closure#0}, core::option::Option<core::option::Option<onevcs::checkout_reads::Managed>>>
  3.999 ms (0.46%; cumulative 53.61%) clock_nanosleep
  3.651 ms (0.42%; cumulative 54.02%) pipe2
  3.621 ms (0.41%; cumulative 54.44%) fun_ab230
  3.582 ms (0.41%; cumulative 54.85%) readlink
  3.316 ms (0.38%; cumulative 55.23%) fun_a9c70
  3.285 ms (0.38%; cumulative 55.60%) fun_11ca00
  3.186 ms (0.36%; cumulative 55.97%) unlinkat
  3.184 ms (0.36%; cumulative 56.33%) core::hash::sip::u8to64_le
  3.178 ms (0.36%; cumulative 56.70%) fun_188620
  3.122 ms (0.36%; cumulative 57.05%) _mprotect
  3.056 ms (0.35%; cumulative 57.40%) serde_json::ser::format_escaped_str_contents::<&mut alloc::vec::Vec<u8>, serde_json::ser::CompactFormatter>
  2.784 ms (0.32%; cumulative 57.72%) fun_a9b90
  2.771 ms (0.32%; cumulative 58.04%) <alloc::raw_vec::RawVecInner>::finish_grow
  2.565 ms (0.29%; cumulative 58.33%) <serde_json::de::Deserializer<serde_json::read::StrRead>>::parse_whitespace
  2.508 ms (0.29%; cumulative 58.62%) <jsonschema::paths::Location>::join::<&jsonschema::keywords::Keyword>
  2.227 ms (0.25%; cumulative 58.88%) <onepipeline::summary::RunSummary>::derive
  2.196 ms (0.25%; cumulative 59.13%) ftruncate
  2.161 ms (0.25%; cumulative 59.37%) core::ub_checks::maybe_is_nonoverlapping::runtime
  2.155 ms (0.25%; cumulative 59.62%) <core::hash::sip::Hasher<core::hash::sip::Sip13Rounds> as core::hash::Hasher>::write
  2.017 ms (0.23%; cumulative 59.85%) getrlimit
  1.965 ms (0.22%; cumulative 60.08%) <hashbrown::raw::RawTable<usize>>::reserve_rehash::<indexmap::inner::get_hash<alloc::string::String, serde_json::value::Value>::{closure#0}>
  1.891 ms (0.22%; cumulative 60.29%) <core::str::lossy::Utf8Chunks as core::iter::traits::iterator::Iterator>::next
  1.879 ms (0.22%; cumulative 60.51%) <alloc::vec::Vec<regex_automata::nfa::thompson::map::Utf8BoundedEntry>>::extend_with
  1.849 ms (0.21%; cumulative 60.72%) _madvise
  1.796 ms (0.21%; cumulative 60.93%) clock_gettime
  1.782 ms (0.20%; cumulative 61.13%) core::fmt::write
  1.616 ms (0.18%; cumulative 61.31%) <<onepipeline::projection::RunState as serde_core::de::Deserialize>::deserialize::__Visitor as serde_core::de::Visitor>::visit_map::<serde_json::de::MapAccess<serde_json::read::StrRead>>
  1.587 ms (0.18%; cumulative 61.50%) __rustc::__rdl_alloc
  1.573 ms (0.18%; cumulative 61.68%) <jsonschema::keywords::properties::SmallPropertiesValidator as jsonschema::validator::Validate>::validate
  1.562 ms (0.18%; cumulative 61.85%) <hashbrown::raw::RawTableInner>::find_or_find_insert_index_inner
  1.526 ms (0.17%; cumulative 62.03%) brk
  1.515 ms (0.17%; cumulative 62.20%) core::str::converts::from_utf8
  1.510 ms (0.17%; cumulative 62.38%) <std::path::Components>::as_path
  1.494 ms (0.17%; cumulative 62.55%) <alloc::raw_vec::RawVecInner>::deallocate
  1.478 ms (0.17%; cumulative 62.72%) <str as core::cmp::PartialEq>::eq
  1.465 ms (0.17%; cumulative 62.88%) realloc
  1.460 ms (0.17%; cumulative 63.05%) <&mut serde_json::value::de::MapDeserializer as serde_core::de::MapAccess>::next_value_seed::<serde::private::de::content::ContentVisitor>
  1.443 ms (0.17%; cumulative 63.22%) <alloc::vec::Vec<u8> as core::cmp::Ord>::cmp
  1.420 ms (0.16%; cumulative 63.38%) <[u8]>::split_at_unchecked
  1.419 ms (0.16%; cumulative 63.54%) <granit_parser::parser::Parser<granit_parser::input::str::StrInput>>::parse_node
  1.387 ms (0.16%; cumulative 63.70%) <serde_json::read::SliceRead as serde_json::read::Read>::peek
  1.369 ms (0.16%; cumulative 63.86%) jsonschema::compiler::compile_with_internal::<jsonschema_value::serde_json::SerdeJson>
  1.365 ms (0.16%; cumulative 64.01%) <std::hash::random::RandomState as core::hash::BuildHasher>::hash_one::<&str>
  1.342 ms (0.15%; cumulative 64.17%) <alloc::vec::Vec<u8>>::extend_from_slice
  1.329 ms (0.15%; cumulative 64.32%) <std::thread::thread::Thread>::park_timeout
  1.316 ms (0.15%; cumulative 64.47%) <alloc::vec::Vec<onepipeline::plan::Node> as core::ops::drop::Drop>::drop
  1.286 ms (0.15%; cumulative 64.62%) <hashbrown::map::HashMap<jsonschema::compiler::LocationCacheKey, jsonschema::node::PendingSchemaNode, ahash::random_state::RandomState>>::insert
  1.257 ms (0.14%; cumulative 64.76%) chmod
  1.199 ms (0.14%; cumulative 64.90%) <serde_json::read::StrRead as serde_json::read::Read>::discard
  1.184 ms (0.14%; cumulative 65.03%) <core::fmt::Formatter>::write_str
  1.148 ms (0.13%; cumulative 65.16%) _open64_nocancel
  1.134 ms (0.13%; cumulative 65.29%) <granit_parser::scanner::Scanner<granit_parser::input::str::StrInput>>::fetch_value
  1.063 ms (0.12%; cumulative 65.42%) core::ptr::drop_glue::<alloc::vec::Vec<(alloc::string::String, onepipeline::graph::NodeStatus)>>
  1.049 ms (0.12%; cumulative 65.54%) fun_9c7e0
  1.035 ms (0.12%; cumulative 65.65%) getdents64
  1.006 ms (0.12%; cumulative 65.77%) <alloc::vec::Vec<onemessagebus::envelope::Envelope<onepipeline::vocabulary::Agent>>>::extend_desugared::<core::iter::adapters::filter_map::FilterMap<core::slice::iter::Iter<onepipeline::journal::FinishedLine>, onepipeline::checkpoint::fold_in_merge_order::{closure#0}>>
  0.978 ms (0.11%; cumulative 65.88%) alloc::fmt::format::format_inner
  0.976 ms (0.11%; cumulative 65.99%) <serde_json::de::MapAccess<serde_json::read::StrRead> as serde_core::de::MapAccess>::next_value::<core::option::Option<std::path::PathBuf>>
  0.974 ms (0.11%; cumulative 66.10%) <&mut <core::time::Duration as core::cmp::Ord>::cmp as core::ops::function::FnOnce<(&core::time::Duration, &core::time::Duration)>>::call_once
  0.971 ms (0.11%; cumulative 66.22%) <alloc::vec::Vec<&std::path::Path> as core::iter::traits::collect::IntoIterator>::into_iter
  0.969 ms (0.11%; cumulative 66.33%) <onetaskgraph_core::engine::Engine>::copy::{closure#0}
  0.961 ms (0.11%; cumulative 66.44%) core::ptr::drop_glue::<[indexmap::Bucket<alloc::string::String, serde_json::value::Value>]>
  0.951 ms (0.11%; cumulative 66.55%) <serde_json::ser::Compound<&mut alloc::vec::Vec<u8>, serde_json::ser::CompactFormatter> as serde_core::ser::SerializeMap>::serialize_key::<str>
  0.943 ms (0.11%; cumulative 66.65%) core::str::validations::next_code_point::<core::slice::iter::Iter<u8>>
  0.942 ms (0.11%; cumulative 66.76%) core::ptr::drop_glue::<std::io::error::Error>
  0.937 ms (0.11%; cumulative 66.87%) <<onepipeline::ledger::LaunchRecord as serde_core::de::Deserialize>::deserialize::__Visitor as serde_core::de::Visitor>::visit_map::<serde_json::de::MapAccess<serde_json::read::StrRead>>
  0.935 ms (0.11%; cumulative 66.98%) std::sys::fs::unix::try_statx
  0.931 ms (0.11%; cumulative 67.08%) onepipeline::ledger::read_range
  0.925 ms (0.11%; cumulative 67.19%) <u64 as itoa::Unsigned>::fmt
  0.920 ms (0.11%; cumulative 67.29%) pthread_mutex_lock
  0.919 ms (0.11%; cumulative 67.40%) <core::ops::control_flow::ControlFlow<()> as core::cmp::PartialEq>::eq
  0.909 ms (0.10%; cumulative 67.50%) <core::slice::iter::Iter<alloc::string::String> as core::iter::traits::iterator::Iterator>::find_map::<&onepipeline::plan::Node, &mut <onepipeline::graph::Graph>::iter::{closure#0}>
  0.905 ms (0.10%; cumulative 67.61%) <alloc::collections::btree::map::Iter<alloc::string::String, onepipeline::projection::Recorded> as core::iter::traits::iterator::Iterator>::next
  0.874 ms (0.10%; cumulative 67.71%) pthread_create
  0.873 ms (0.10%; cumulative 67.81%) <str as serde_json::value::index::Index>::index_into
  0.871 ms (0.10%; cumulative 67.91%) <core::option::Option<&[i64]> as core::ops::try_trait::Try>::branch
  0.868 ms (0.10%; cumulative 68.01%) <granit_parser::parser::Parser<granit_parser::input::str::StrInput>>::block_mapping_key_node
  0.863 ms (0.10%; cumulative 68.10%) <alloc::collections::btree::map::BTreeMap<_, _, _> as core::clone::Clone>::clone::clone_subtree::<alloc::string::String, alloc::string::String, alloc::alloc::Global>
  0.830 ms (0.10%; cumulative 68.20%) std::sys::pal::unix::conf::glibc_version
  0.829 ms (0.09%; cumulative 68.29%) <core::option::Option<alloc::string::String> as core::clone::Clone>::clone
  0.823 ms (0.09%; cumulative 68.39%) core::ptr::drop_glue::<std::sys::os_str::bytes::Buf>
  0.819 ms (0.09%; cumulative 68.48%) <serde_json::read::SliceRead>::skip_to_escape
  0.818 ms (0.09%; cumulative 68.58%) unlink
  0.816 ms (0.09%; cumulative 68.67%) fun_ab050
  0.807 ms (0.09%; cumulative 68.76%) <hashbrown::raw::RawTable<usize>>::insert_tagged_at_index
  0.805 ms (0.09%; cumulative 68.85%) <<serde_json::value::Value as serde_core::de::Deserialize>::deserialize::ValueVisitor as serde_core::de::Visitor>::visit_map::<serde_json::de::MapAccess<serde_json::read::StrRead>>
  0.800 ms (0.09%; cumulative 68.95%) <granit_parser::scanner::Scanner<granit_parser::input::str::StrInput>>::fetch_plain_scalar
  0.795 ms (0.09%; cumulative 69.04%) <core::hash::sip::Hasher<core::hash::sip::Sip13Rounds> as core::hash::Hasher>::write_str
  0.790 ms (0.09%; cumulative 69.13%) onepipeline::writeback::carry_the_difference
  0.788 ms (0.09%; cumulative 69.22%) <indexmap::map::IndexMap<alloc::string::String, serde_json::value::Value>>::get_index_of::<str>
  0.783 ms (0.09%; cumulative 69.31%) <alloc::raw_vec::RawVecInner>::grow_amortized
  0.780 ms (0.09%; cumulative 69.40%) <hashbrown::raw::RawTable<(alloc::sync::Arc<fluent_uri::imp::Uri<alloc::string::String>>, alloc::sync::Arc<referencing::registry::build::StoredResource>)>>::reserve_rehash::<hashbrown::map::make_hasher<alloc::sync::Arc<fluent_uri::imp::Uri<alloc::string::String>>, alloc::sync::Arc<referencing::registry::build::StoredResource>, ahash::random_state::RandomState>::{closure#0}>
  0.780 ms (0.09%; cumulative 69.49%) posix_spawn_file_actions_destroy
  0.775 ms (0.09%; cumulative 69.57%) <alloc::raw_vec::RawVec<std::sync::mpmc::waker::Entry>>::grow_one
  0.769 ms (0.09%; cumulative 69.66%) <core::option::Option<u64> as core::cmp::PartialEq>::eq
  0.767 ms (0.09%; cumulative 69.75%) <std::sync::mpmc::waker::SyncWaker>::notify
  0.767 ms (0.09%; cumulative 69.84%) <std::time::Instant>::now
  0.759 ms (0.09%; cumulative 69.92%) _waitpid
  0.748 ms (0.09%; cumulative 70.01%) <indexmap::inner::Core<alloc::string::String, serde_json::value::Value>>::push_entry
  0.745 ms (0.09%; cumulative 70.10%) <<str as alloc::borrow::ToOwned>::to_owned as core::ops::function::FnOnce<(&str,)>>::call_once
  0.744 ms (0.09%; cumulative 70.18%) <std::hash::random::RandomState as core::hash::BuildHasher>::build_hasher
  0.738 ms (0.08%; cumulative 70.26%) <onepipeline::taskgraph::QualifiedId as core::convert::TryFrom<alloc::string::String>>::try_from
  0.737 ms (0.08%; cumulative 70.35%) <core::slice::iter::Iter<alloc::string::String> as core::iter::traits::iterator::Iterator>::next
  0.736 ms (0.08%; cumulative 70.43%) std::thread::lifecycle::spawn_unchecked::<<onevcs::git::PipeCapture>::start<std::process::ChildStdout>::{closure#0}, onevcs::git::Captured>
  0.730 ms (0.08%; cumulative 70.52%) <alloc::vec::Vec<u8>>::reserve
  0.727 ms (0.08%; cumulative 70.60%) <alloc::vec::Vec<u8> as alloc::vec::spec_extend::SpecExtend<&u8, core::slice::iter::Iter<u8>>>::spec_extend
  0.724 ms (0.08%; cumulative 70.68%) <hashbrown::raw::RawTableInner>::allocation_info
  0.722 ms (0.08%; cumulative 70.77%) onepipeline::driver::attach
  0.720 ms (0.08%; cumulative 70.85%) <alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::ValMut, alloc::string::String, onepipeline::projection::Recorded, alloc::collections::btree::node::marker::LeafOrInternal>>::into_key_val_mut_at
  0.716 ms (0.08%; cumulative 70.93%) fun_ada50
  0.710 ms (0.08%; cumulative 71.01%) <core::ptr::non_null::NonNull<_>>::new_unchecked::precondition_check
  0.707 ms (0.08%; cumulative 71.09%) fun_10f4e0
  0.699 ms (0.08%; cumulative 71.17%) fun_189710
  0.696 ms (0.08%; cumulative 71.25%) std::thread::lifecycle::spawn_unchecked::<<onevcs::git::PipeCapture>::start<std::process::ChildStderr>::{closure#0}, onevcs::git::Captured>
  0.692 ms (0.08%; cumulative 71.33%) <&str as core::slice::cmp::SlicePartialEq<&str>>::equal_same_length
  0.689 ms (0.08%; cumulative 71.41%) <serde_json::value::de::MapDeserializer>::new
  0.681 ms (0.08%; cumulative 71.49%) <serde_json::de::MapAccess<serde_json::read::StrRead> as serde_core::de::MapAccess>::next_value_seed::<core::marker::PhantomData<serde_json::value::Value>>
  0.681 ms (0.08%; cumulative 71.57%) <serde_json::de::MapAccess<serde_json::read::StrRead> as serde_core::de::MapAccess>::next_key_seed::<core::marker::PhantomData<<onepipeline::plan::Node as serde_core::de::Deserialize>::deserialize::__Field>>
  0.678 ms (0.08%; cumulative 71.64%) fun_18bac0
  0.670 ms (0.08%; cumulative 71.72%) <granit_parser::parser::Parser<granit_parser::input::str::StrInput> as granit_parser::parser::ParserTrait>::next_event
  0.668 ms (0.08%; cumulative 71.80%) <hashbrown::raw::RawTableInner>::fallible_with_capacity::<allocator_api2::stable::alloc::global::Global>
  0.649 ms (0.07%; cumulative 71.87%) <core::option::Option<&core::option::Option<onevcs::checkout_reads::Managed>>>::cloned
  0.643 ms (0.07%; cumulative 71.94%) core::ptr::drop_glue::<indexmap::inner::Core<alloc::string::String, serde_json::value::Value>>
  0.638 ms (0.07%; cumulative 72.02%) <std::sync::mpmc::list::Channel<()>>::recv
  0.636 ms (0.07%; cumulative 72.09%) fun_9e730
  0.636 ms (0.07%; cumulative 72.16%) <std::path::Path as core::hash::Hash>::hash::<std::hash::random::DefaultHasher>
  0.627 ms (0.07%; cumulative 72.24%) <alloc::collections::btree::node::Handle<alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Dying, alloc::string::String, serde_json::map::Map<alloc::string::String, serde_json::value::Value>, alloc::collections::btree::node::marker::LeafOrInternal>, alloc::collections::btree::node::marker::KV>>::next_leaf_edge
  0.618 ms (0.07%; cumulative 72.31%) onepipeline::journal::stream_queues::<onepipeline::journal::merged_order::{closure#0}>
  0.611 ms (0.07%; cumulative 72.38%) <onepipeline::graph::Graph>::get
  0.598 ms (0.07%; cumulative 72.44%) <regex_automata::meta::regex::Builder>::build_from_hir
  0.584 ms (0.07%; cumulative 72.51%) referencing::uri::encode_to
  0.580 ms (0.07%; cumulative 72.58%) <u8 as core::slice::cmp::SlicePartialEq<u8>>::equal_same_length
  0.572 ms (0.07%; cumulative 72.64%) <fluent_uri::parse::Parser>::parse_from_authority
  0.572 ms (0.07%; cumulative 72.71%) alloc::collections::btree::mem::replace::<alloc::collections::btree::node::Handle<alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Dying, alloc::string::String, u64, alloc::collections::btree::node::marker::Leaf>, alloc::collections::btree::node::marker::Edge>, alloc::collections::btree::node::Handle<alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Dying, alloc::string::String, u64, alloc::collections::btree::node::marker::LeafOrInternal>, alloc::collections::btree::node::marker::KV>, <alloc::collections::btree::node::Handle<alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Dying, alloc::string::String, u64, alloc::collections::btree::node::marker::Leaf>, alloc::collections::btree::node::marker::Edge>>::deallocating_next_unchecked<alloc::alloc::Global>::{closure#0}>
  0.565 ms (0.06%; cumulative 72.77%) <core::iter::adapters::enumerate::Enumerate<core::slice::iter::Iter<alloc::string::String>> as core::iter::traits::iterator::Iterator>::next
  0.563 ms (0.06%; cumulative 72.84%) <alloc::collections::btree::map::BTreeMap<alloc::string::String, onevcs::preserve::Preservation>>::is_empty
  0.562 ms (0.06%; cumulative 72.90%) <serde_json::value::Value as serde_core::de::Deserializer>::deserialize_struct::<<onepipeline::vocabulary::Dimensions as serde_core::de::Deserialize>::deserialize::__Visitor>
  0.560 ms (0.06%; cumulative 72.97%) core::sync::atomic::atomic_compare_exchange::<u32>
  0.560 ms (0.06%; cumulative 73.03%) <alloc::boxed::Box<alloc::collections::btree::node::LeafNode<alloc::string::String, onepipeline::projection::SessionOpened>>>::try_new_uninit_in
  0.559 ms (0.06%; cumulative 73.09%) serde_json::de::from_str::<onepipeline::ledger::LaunchRecord>
  0.558 ms (0.06%; cumulative 73.16%) <alloc::vec::Vec<onepipeline::ledger::Record> as core::iter::traits::collect::IntoIterator>::into_iter
  0.558 ms (0.06%; cumulative 73.22%) core::ptr::drop_glue::<alloc::string::String>
  0.558 ms (0.06%; cumulative 73.29%) <core::hash::sip::Sip13Rounds as core::hash::sip::Sip>::c_rounds
  0.557 ms (0.06%; cumulative 73.35%) <alloc::collections::btree::map::IntoIter<alloc::string::String, onevcs::releases::TargetName> as core::ops::drop::Drop>::drop
  0.555 ms (0.06%; cumulative 73.41%) <<onepipeline::vocabulary::Source as serde_core::de::Deserialize>::deserialize::__FieldVisitor as serde_core::de::Visitor>::visit_string::<serde_json::error::Error>
  0.553 ms (0.06%; cumulative 73.48%) <<tokio::runtime::context::scoped::Scoped<_>>::set::Reset<tokio::runtime::scheduler::Context> as core::ops::drop::Drop>::drop
  0.552 ms (0.06%; cumulative 73.54%) <core::result::Result<(), serde_json::error::Error> as core::ops::try_trait::Try>::branch
  0.550 ms (0.06%; cumulative 73.60%) <onepipeline::projection::Closure as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>>
  0.550 ms (0.06%; cumulative 73.67%) getpagesize
  0.547 ms (0.06%; cumulative 73.73%) onepipeline::release::ask_until_dropped
  0.544 ms (0.06%; cumulative 73.79%) <<alloc::vec::into_iter::IntoIter<_, _> as core::ops::drop::Drop>::drop::DropGuard<serde_json::value::Value, alloc::alloc::Global> as core::ops::drop::Drop>::drop
  0.540 ms (0.06%; cumulative 73.85%) <<onepipeline::telemetry::Usage as serde_core::de::Deserialize>::deserialize::__Visitor as serde_core::de::Visitor>::visit_map::<serde_json::de::MapAccess<serde_json::read::StrRead>>
  0.536 ms (0.06%; cumulative 73.91%) core::ptr::drop_glue::<jsonschema::keywords::unique_items::UniqueItemsValidator>
  0.534 ms (0.06%; cumulative 73.97%) <hashbrown::raw::RawTableInner>::ctrl_slice
  0.533 ms (0.06%; cumulative 74.04%) <alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Immut, alloc::string::String, onepipeline::plan::Node, alloc::collections::btree::node::marker::LeafOrInternal>>::find_key_index::<alloc::string::String>
  0.533 ms (0.06%; cumulative 74.10%) <core::hash::sip::Hasher<core::hash::sip::Sip13Rounds>>::reset
  0.532 ms (0.06%; cumulative 74.16%) std::sys::backtrace::__rust_begin_short_backtrace::<<onevcs::git::PipeCapture>::start<std::process::ChildStderr>::{closure#0}, onevcs::git::Captured>
  0.530 ms (0.06%; cumulative 74.22%) <std::thread::local::LocalKey<core::cell::Cell<(u64, u64)>>>::try_with::<<std::hash::random::RandomState>::new::{closure#0}, std::hash::random::RandomState>
  0.530 ms (0.06%; cumulative 74.28%) <u32 as itoa::private::Sealed>::write
  0.529 ms (0.06%; cumulative 74.34%) <core::option::Option<onepipeline::plan::RepoType> as core::cmp::PartialEq>::eq
  0.528 ms (0.06%; cumulative 74.40%) <onepipeline::ledger::RunPaths>::launch
  0.528 ms (0.06%; cumulative 74.46%) <serde_json::number::Number as serde_core::de::Deserializer>::deserialize_any::<<u32 as serde_core::de::Deserialize>::deserialize::PrimitiveVisitor>
  0.527 ms (0.06%; cumulative 74.52%) <serde_json::ser::Compound<&mut alloc::vec::Vec<u8>, serde_json::ser::PrettyFormatter> as serde_core::ser::SerializeMap>::serialize_entry::<str, u32>
  0.527 ms (0.06%; cumulative 74.58%) <serde_json::value::Value as core::fmt::Display>::fmt
  0.526 ms (0.06%; cumulative 74.64%) <alloc::collections::btree::map::BTreeMap<alloc::string::String, onepipeline::graph::NodeStatus> as core::ops::drop::Drop>::drop
  0.526 ms (0.06%; cumulative 74.70%) <core::slice::iter::Iter<u8> as core::iter::traits::iterator::Iterator>::fold::<u128, onepipeline::checkpoint::digested::{closure#0}>
  0.525 ms (0.06%; cumulative 74.76%) <alloc::collections::btree::node::Handle<alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Dying, onepipeline::telemetry::BucketName, u64, alloc::collections::btree::node::marker::LeafOrInternal>, alloc::collections::btree::node::marker::KV>>::drop_key_val
  0.525 ms (0.06%; cumulative 74.82%) core::ptr::drop_glue::<alloc::vec::Vec<u8>>
  0.524 ms (0.06%; cumulative 74.88%) core::ptr::drop_glue::<core::option::Option<oneagentgraph::event::OneharnessSession>>
  0.524 ms (0.06%; cumulative 74.94%) fun_adf20
  0.523 ms (0.06%; cumulative 75.00%) onepipeline::vocabulary::registry
  0.523 ms (0.06%; cumulative 75.06%) <core::iter::adapters::zip::Zip<core::iter::adapters::zip::Zip<core::slice::iter::Iter<u8>, core::slice::iter::Iter<u8>>, core::slice::iter::IterMut<i64>> as core::iter::adapters::zip::ZipImpl<core::iter::adapters::zip::Zip<core::slice::iter::Iter<u8>, core::slice::iter::Iter<u8>>, core::slice::iter::IterMut<i64>>>::next
  0.521 ms (0.06%; cumulative 75.12%) <alloc::vec::Vec<serde_json::value::Value>>::push
  0.520 ms (0.06%; cumulative 75.18%) <&alloc::vec::Vec<onepipeline::release::Dependency> as core::iter::traits::collect::IntoIterator>::into_iter
  0.519 ms (0.06%; cumulative 75.24%) <&str as core::hash::Hash>::hash::<std::hash::random::DefaultHasher>
  0.518 ms (0.06%; cumulative 75.30%) <alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Dying, alloc::string::String, alloc::string::String, alloc::collections::btree::node::marker::LeafOrInternal>>::ascend
  0.517 ms (0.06%; cumulative 75.36%) <onepipeline::views::RunView>::of_state
  0.517 ms (0.06%; cumulative 75.42%) <alloc::collections::btree::map::BTreeMap<alloc::string::String, alloc::string::String>>::is_empty
  0.515 ms (0.06%; cumulative 75.48%) onepipeline::projection::millis_of
  0.515 ms (0.06%; cumulative 75.54%) <chrono::format::OffsetFormat>::format::<alloc::string::String>
  0.514 ms (0.06%; cumulative 75.59%) <serde_json::value::Value as <[_]>::to_vec_in::ConvertVec>::to_vec::<alloc::alloc::Global>
  0.514 ms (0.06%; cumulative 75.65%) <serde_json::ser::Compound<&mut alloc::vec::Vec<u8>, serde_json::ser::PrettyFormatter> as serde_core::ser::SerializeMap>::serialize_entry::<str, core::option::Option<onepipeline::writeback::ProjectedStatus>>
  0.512 ms (0.06%; cumulative 75.71%) _read_nocancel
  0.512 ms (0.06%; cumulative 75.77%) <alloc::collections::btree::node::Handle<alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Immut, alloc::string::String, alloc::collections::btree::set_val::SetValZST, alloc::collections::btree::node::marker::Leaf>, alloc::collections::btree::node::marker::Edge>>::next_kv
  0.512 ms (0.06%; cumulative 75.83%) fun_aabc0
  0.510 ms (0.06%; cumulative 75.89%) <core::iter::adapters::zip::Zip<core::iter::adapters::zip::Zip<core::slice::iter::Iter<u8>, core::slice::iter::Iter<u8>>, core::slice::iter::IterMut<i64>> as core::iter::adapters::zip::ZipImpl<core::iter::adapters::zip::Zip<core::slice::iter::Iter<u8>, core::slice::iter::Iter<u8>>, core::slice::iter::IterMut<i64>>>::new
  0.510 ms (0.06%; cumulative 75.95%) core::option::Option::<alloc::string::String>::Some
  0.510 ms (0.06%; cumulative 76.00%) <core::result::Result<serde_json::value::Value, serde_json::error::Error> as core::ops::try_trait::Try>::branch
  0.510 ms (0.06%; cumulative 76.06%) <serde_json::ser::Compound<&mut alloc::vec::Vec<u8>, serde_json::ser::PrettyFormatter> as serde_core::ser::SerializeStruct>::serialize_field::<alloc::collections::btree::map::BTreeMap<alloc::string::String, alloc::string::String>>
  0.509 ms (0.06%; cumulative 76.12%) <alloc::collections::btree::navigate::LazyLeafRange<alloc::collections::btree::node::marker::Dying, alloc::string::String, onevcs::preserve::Preservation>>::deallocating_end::<alloc::alloc::Global>
  0.509 ms (0.06%; cumulative 76.18%) <alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Mut, alloc::string::String, alloc::string::String, alloc::collections::btree::node::marker::Leaf>>::push_with_handle
  0.508 ms (0.06%; cumulative 76.24%) <core::option::Option<alloc::string::String>>::is_none
  0.507 ms (0.06%; cumulative 76.30%) <core::fmt::Formatter>::write_fmt
  0.507 ms (0.06%; cumulative 76.35%) <std::hash::random::RandomState as core::hash::BuildHasher>::hash_one::<&[u8]>
  0.507 ms (0.06%; cumulative 76.41%) <alloc::collections::btree::map::BTreeMap<alloc::string::String, u64>>::entry
  0.506 ms (0.06%; cumulative 76.47%) linkat
  0.506 ms (0.06%; cumulative 76.53%) <onepipeline::ledger::RunPaths>::journal
  0.506 ms (0.06%; cumulative 76.59%) <serde_saphyr::de::deserializer::YamlDeserializer>::take_scalar_view
  0.505 ms (0.06%; cumulative 76.64%) <serde_json::map::Map<alloc::string::String, serde_json::value::Value>>::entry::<&str>
  0.505 ms (0.06%; cumulative 76.70%) <core::ops::range::Range<usize> as core::iter::traits::iterator::Iterator>::next
  0.505 ms (0.06%; cumulative 76.76%) <alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Dying, alloc::string::String, alloc::collections::btree::set_val::SetValZST, alloc::collections::btree::node::marker::LeafOrInternal>>::ascend
  0.505 ms (0.06%; cumulative 76.82%) <serde_json::ser::Compound<&mut alloc::vec::Vec<u8>, serde_json::ser::CompactFormatter> as serde_core::ser::SerializeMap>::serialize_value::<core::option::Option<onepipeline::plan::TaskRecord>>
  0.504 ms (0.06%; cumulative 76.87%) <&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>> as serde_core::ser::Serializer>::serialize_str
  0.504 ms (0.06%; cumulative 76.93%) <onepipeline::projection::RunState as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>>
  0.503 ms (0.06%; cumulative 76.99%) core::iter::adapters::filter_map::filter_map_try_fold::<&alloc::string::String, &onepipeline::plan::Node, (), core::ops::control_flow::ControlFlow<(onepipeline::graph::NodeRef, onepipeline::graph::Settled)>, <onepipeline::graph::Graph>::iter::{closure#0}, core::iter::traits::iterator::Iterator::find_map::check<&onepipeline::plan::Node, (onepipeline::graph::NodeRef, onepipeline::graph::Settled), &mut <onepipeline::projection::RunState>::settled::{closure#0}>::{closure#0}>::{closure#0}
  0.502 ms (0.06%; cumulative 77.05%) <alloc::collections::btree::node::Handle<alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Immut, alloc::string::String, onepipeline::projection::NodeActivity, alloc::collections::btree::node::marker::Leaf>, alloc::collections::btree::node::marker::Edge>>::next_kv
  0.501 ms (0.06%; cumulative 77.10%) <regex_syntax::hir::ClassBytesRange as regex_syntax::hir::interval::Interval>::create
  0.501 ms (0.06%; cumulative 77.16%) <&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>> as serde_core::ser::Serializer>::serialize_unit
  0.500 ms (0.06%; cumulative 77.22%) <core::str::pattern::CharSearcher as core::str::pattern::ReverseSearcher>::next_reject_back
  0.500 ms (0.06%; cumulative 77.28%) <alloc::collections::btree::node::Handle<alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Dying, alloc::string::String, onepipeline::projection::NodeActivity, alloc::collections::btree::node::marker::Leaf>, alloc::collections::btree::node::marker::Edge>>::deallocating_next_unchecked::<alloc::alloc::Global>::{closure#0}
  0.499 ms (0.06%; cumulative 77.33%) <alloc::borrow::Cow<str> as core::clone::Clone>::clone
  0.498 ms (0.06%; cumulative 77.39%) <onepipeline::human_items::HumanItemFilter as clap_builder::derive::Args>::augment_args
  0.498 ms (0.06%; cumulative 77.45%) <<onepipeline::plan::TaskRecord as serde_core::de::Deserialize>::deserialize::__FieldVisitor as serde_core::de::Visitor>::visit_str::<serde_json::error::Error>
  0.498 ms (0.06%; cumulative 77.50%) <indexmap::map::iter::Iter<alloc::string::String, serde_json::value::Value> as core::iter::traits::iterator::Iterator>::next
  0.497 ms (0.06%; cumulative 77.56%) <alloc::collections::btree::map::Iter<alloc::string::String, onepipeline::agentgraph::TurnAddress> as core::iter::traits::iterator::Iterator>::try_fold::<(), core::iter::traits::iterator::Iterator::try_for_each::call<(&alloc::string::String, &onepipeline::agentgraph::TurnAddress), core::result::Result<(), serde_json::error::Error>, <&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>> as serde_core::ser::Serializer>::collect_map<&alloc::string::String, &onepipeline::agentgraph::TurnAddress, &alloc::collections::btree::map::BTreeMap<alloc::string::String, onepipeline::agentgraph::TurnAddress>>::{closure#0}>::{closure#0}, core::result::Result<(), serde_json::error::Error>>
  0.496 ms (0.06%; cumulative 77.62%) <jsonschema::compiler::Context>::new
  0.496 ms (0.06%; cumulative 77.67%) <core::slice::sort::shared::smallsort::CopyOnDrop<(onepipeline::graph::NodeRef, onepipeline::graph::Settled)> as core::ops::drop::Drop>::drop
  0.496 ms (0.06%; cumulative 77.73%) <u32 as itoa::Unsigned>::fmt
  0.495 ms (0.06%; cumulative 77.79%) <serde_json::de::MapKey<serde_json::read::StrRead> as serde_core::de::Deserializer>::deserialize_any::<<onepipeline::projection::RunState as serde_core::de::Deserialize>::deserialize::__FieldVisitor>
  0.494 ms (0.06%; cumulative 77.84%) <indexmap::map::IndexMap<alloc::string::String, serde_json::value::Value>>::insert_full
  0.494 ms (0.06%; cumulative 77.90%) <hashbrown::raw::RawTableInner>::drop_elements::<usize>
  0.493 ms (0.06%; cumulative 77.96%) <regex_automata::nfa::thompson::compiler::Compiler>::new
  0.493 ms (0.06%; cumulative 78.01%) <<onepipeline::graph::NodeStatus as serde_core::de::Deserialize>::deserialize::__Visitor as serde_core::de::Visitor>::visit_enum::<serde_json::de::UnitVariantAccess<serde_json::read::StrRead>>
  0.493 ms (0.06%; cumulative 78.07%) core::iter::traits::iterator::Iterator::find_map::check::<&onepipeline::plan::Node, (onepipeline::graph::NodeRef, onepipeline::graph::Settled), &mut <onepipeline::projection::RunState>::settled::{closure#0}>::{closure#0}
  0.493 ms (0.06%; cumulative 78.13%) <serde_saphyr::de::deserializer::YamlDeserializer as serde_core::de::Deserializer>::deserialize_any::<serde_path_to_error::wrap::Wrap<<serde_json::value::Value as serde_core::de::Deserialize>::deserialize::ValueVisitor>>
  0.493 ms (0.06%; cumulative 78.18%) std::sys::fs::rename
  0.493 ms (0.06%; cumulative 78.24%) <serde_json::ser::MapKeySerializer<&mut alloc::vec::Vec<u8>, serde_json::ser::CompactFormatter> as serde_core::ser::Serializer>::serialize_str
  0.492 ms (0.06%; cumulative 78.30%) <str as core::cmp::Ord>::cmp
  0.492 ms (0.06%; cumulative 78.35%) <alloc::vec::Vec<(onepipeline::graph::NodeRef, onepipeline::graph::Settled)> as core::iter::traits::collect::IntoIterator>::into_iter
  0.492 ms (0.06%; cumulative 78.41%) <serde_json::ser::Compound<&mut alloc::vec::Vec<u8>, serde_json::ser::CompactFormatter> as serde_core::ser::SerializeStruct>::serialize_field::<alloc::string::String>
  0.492 ms (0.06%; cumulative 78.46%) <alloc::collections::btree::set::Iter<alloc::string::String> as core::iter::traits::iterator::Iterator>::next
  0.491 ms (0.06%; cumulative 78.52%) <alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Mut, schemars::generate::SchemaUid, alloc::collections::btree::set_val::SetValZST, alloc::collections::btree::node::marker::LeafOrInternal>>::search_tree::<schemars::generate::SchemaUid>
  0.491 ms (0.06%; cumulative 78.58%) <alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Dying, alloc::string::String, alloc::vec::Vec<onepipeline::projection::ServiceRecord>, alloc::collections::btree::node::marker::LeafOrInternal>>::ascend
  0.491 ms (0.06%; cumulative 78.63%) <alloc::vec::Vec<u8> as std::io::Write>::write_all
  0.491 ms (0.06%; cumulative 78.69%) <&mut serde_json::de::Deserializer<serde_json::read::SliceRead> as serde_core::de::Deserializer>::deserialize_any::<<serde_json::value::Value as serde_core::de::Deserialize>::deserialize::ValueVisitor>
  0.490 ms (0.06%; cumulative 78.75%) <&serde_json::map::Map<alloc::string::String, serde_json::value::Value> as jsonschema_value::Object<jsonschema_value::serde_json::SerdeJson>>::get
  0.490 ms (0.06%; cumulative 78.80%) <core::option::Option<onepipeline::graph::Landing> as core::cmp::PartialEq>::eq
  0.490 ms (0.06%; cumulative 78.86%) core::intrinsics::rotate_left::<u64>
  0.490 ms (0.06%; cumulative 78.91%) <alloc::raw_vec::RawVec<alloc::borrow::Cow<str>> as core::ops::drop::Drop>::drop
  0.489 ms (0.06%; cumulative 78.97%) <serde_json::de::Deserializer<serde_json::read::SliceRead>>::parse_whitespace
  0.489 ms (0.06%; cumulative 79.03%) <usize>::unchecked_add::precondition_check
  0.489 ms (0.06%; cumulative 79.08%) core::ptr::drop_glue::<onepipeline::unanswered::ScheduleFile>
  0.489 ms (0.06%; cumulative 79.14%) <onevcs::launch_trace::Wait>::into_launch
  0.489 ms (0.06%; cumulative 79.19%) _errno_location
  0.488 ms (0.06%; cumulative 79.25%) core::option::Option::<onepipeline::checkpoint::Placed>::Some
  0.487 ms (0.06%; cumulative 79.31%) <referencing::vocabularies::VocabularySet>::contains
  0.487 ms (0.06%; cumulative 79.36%) <alloc::collections::btree::node::Handle<alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Dying, alloc::string::String, u64, alloc::collections::btree::node::marker::Leaf>, alloc::collections::btree::node::marker::Edge>>::deallocating_next_unchecked::<alloc::alloc::Global>::{closure#0}
  0.487 ms (0.06%; cumulative 79.42%) <alloc::collections::btree::node::Handle<alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Dying, onepipeline::telemetry::BucketName, u64, alloc::collections::btree::node::marker::Leaf>, alloc::collections::btree::node::marker::Edge>>::deallocating_next::<alloc::alloc::Global>
  0.487 ms (0.06%; cumulative 79.47%) onevcs::lock::exclusive
  0.487 ms (0.06%; cumulative 79.53%) <<serde_json::map::Map<alloc::string::String, serde_json::value::Value> as serde_core::de::Deserialize>::deserialize::Visitor as serde_core::de::Visitor>::visit_map::<serde_json::de::MapAccess<serde_json::read::SliceRead>>
  0.487 ms (0.06%; cumulative 79.58%) <alloc::vec::into_iter::IntoIter<(alloc::string::String, alloc::vec::Vec<alloc::string::String>, core::result::Result<onevcs::standings::FollowedChecks, alloc::string::String>)>>::as_raw_mut_slice
  0.487 ms (0.06%; cumulative 79.64%) core::iter::adapters::map::map_fold::<&indexmap::Bucket<alloc::string::String, serde_json::value::Value>, indexmap::Bucket<alloc::string::String, serde_json::value::Value>, (), <indexmap::Bucket<alloc::string::String, serde_json::value::Value> as core::clone::Clone>::clone, core::iter::traits::iterator::Iterator::for_each::call<indexmap::Bucket<alloc::string::String, serde_json::value::Value>, <alloc::vec::Vec<indexmap::Bucket<alloc::string::String, serde_json::value::Value>>>::extend_trusted<core::iter::adapters::cloned::Cloned<core::slice::iter::Iter<indexmap::Bucket<alloc::string::String, serde_json::value::Value>>>>::{closure#0}>::{closure#0}>::{closure#0}
  0.486 ms (0.06%; cumulative 79.70%) alloc::collections::btree::node::slice_remove::<alloc::collections::btree::set_val::SetValZST>
  0.486 ms (0.06%; cumulative 79.75%) core::slice::index::range::<core::ops::range::RangeFull>
  0.486 ms (0.06%; cumulative 79.81%) <std::fs::OpenOptions>::_open
  0.485 ms (0.06%; cumulative 79.86%) <referencing::cache::SharedUriCache>::resolve_against::<&str>
  0.485 ms (0.06%; cumulative 79.92%) <alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Owned, schemars::generate::SchemaUid, alloc::collections::btree::set_val::SetValZST, alloc::collections::btree::node::marker::Leaf>>::new_leaf::<alloc::alloc::Global>
  0.485 ms (0.06%; cumulative 79.97%) <onepipeline::projection::SessionOpened as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>>
  0.484 ms (0.06%; cumulative 80.03%) <onepipeline::projection::RunState as core::clone::Clone>::clone
CPU attribution only: child lifetimes, filesystem latency, waits and process creation are not disjointly measured here.
```

The initializer is the start process's largest attributed module: 193.155 ms, 20.08% of its 961.846 ms observed CPU. Adopt has no contributor initializer in this journey; its largest attributed module is onevcs::git, 174.754 ms or 20.00% of 873.577 ms observed CPU. These numbers are local sampled CPU attribution and instrumented lifetimes, not hosted latency or expected PR savings. The sampler log says `Lost 5 events.`; missing-event uncertainty is retained, with no claim of perfect capture or a quantified bound. Unresolved native frames remain explicitly labelled in the tables rather than assigned to a guessed function. Child lifetimes overlap engine CPU and are not a disjoint filesystem/wait/process-creation wall partition.

Manager a41 confirms pricing the contributor cut for both verbs and reporting no further independently justified adopt startup cut here: git reads and PipeCapture/bounded child waits belong to the sibling spikes.

### Updated handoff

No hosted run has been started by this dispatch; none is in flight. Session PR1621 is a draft. The branch has prototype and measurement repairs through b6542b9a; latest corpus-only strengthening is being committed separately. The local baseline attribution above is recorded; the local cut sample and release samples remain, as do hosted after runs, pass-name/cache/coverage audit and final-source judged lint/fold. Four child-waits demonstration heads occupy the plan cap under a41; the next slot goes to git-reads, then engine-startup.


## Candidate budgets

This spike measured no candidate budget itself.

## Design findings

- The baseline test-profile programs are keyed by executable, not by verb. Manager question c-c079002ebedf688e823b2c9cea9a6c46 asks whether completed launch-trace start/adopt lifetimes may replace unavailable per-verb profile means on both sides; these exclude process creation.
