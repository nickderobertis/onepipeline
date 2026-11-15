# Spike `spike-engine-startup`

- **Branch:** `nick/process-launch-speed-2026-11-14/spike-engine-startup`

## Harness

The branch carries scripts/engine-startup.mjs, engine-startup-cpu.py, engine-startup-profile.sh and engine-startup-verify.sh. Fold a demonstration with `node scripts/engine-startup.mjs fold before/1=BASE_JOB contributors/1=AFTER_JOB`. It prints per-job start/adopt completed trace means and e2e minutes, then per-side means, spreads, differences and typical-PR estimates. See spikes/engine-startup/README.md for profiling and equivalence commands.

## Method

### Live hosted handoff — 2026-11-15 22:28 UTC

This handoff supersedes earlier dated checkpoint status statements below. Two startup demonstrations are in flight; neither active head is pushed.

| Point | PR | Run/attempt | Head | Provisional merge SHA | Status |
| --- | --- | --- | --- | --- | --- |
| 1 | 1628 | 43751624389/1 | dd64b7d141e0326cfeb4991b420bdb25387e0b53 | 9852d29ba162d2c7810b4abe217fc9b1a8d7ed1d (checkout confirmed by lint log) | Linux complete; Windows/macOS running; lint cancelled at its thirty-minute limit |
| 2 | 1635 | 43756326338/1 | ecf2eb593848478fbb97423e49c6f2ad5cfb42a5 | 591dd36224fb3402f9f48130175a8c690b4eb524 (checkout confirmed by lint log) | Linux complete; Windows running, macOS queued; lint cancelled after its judged tier failed |

Both execute workflow blob db2bbe081a7b9319eb38bd014e72350b389e8b01. Point2 incorporates the native-path parser-test repair, which leaves the Rust prototype and workflow unchanged. Its local parser journeys passed all fifteen; the local trace/fold rehearsal preceded the round. Point2 run metadata, job metadata, PR merge-ref response and workflow-blob response are committed under evidence/hosted/43756326338/attempt-1. The capacity query immediately before opening it found six plan demonstrations, below a55’s cap of seven; this is the second startup demonstration, at the per-spike cap. Remaining under a67: finish collecting these two points only, audit all outcomes/cache/coverage, run the finished-source comparison and final judged pass. No third run starts. No hosted after timing or saving is claimed yet.

IN PROGRESS. Ruling c13.0 replaces hosted sampling with local attribution and existing hosted launch-trace/tier measurements. Ruling c11.1 accepts run 43684231509 attempts 1 and 2 as the before side. Rulings c7.0/c9.1 require reporting Linux merged coverage as not produced because the vendored patch audit fails test-rest and Nx skips coverage merge; floor remains 95.74 LCOV.

Earlier pre-host checkpoint omitted; the live handoff above is authoritative.



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

<!-- llmlint: ignore[least_privilege_grants] This is the exact historical local capture command required to attribute the recorded figures, not a deployed permission configuration. Rewriting the command would falsify its provenance; the disposable container was removed, and no hosted profiler is proposed (manager a1). -->
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

Manager a45 explicitly marks the hosted CPU/filesystem/wait/process-creation partition unavailable. Local CPU and trace intervals overlap; residual filesystem latency, descheduling and untraced waits are named rather than assigned invented additive figures. Trace means also exclude pre-first-trace initialization and final teardown. Manager a48 accepts the inherited ab27ccee inventory verdict plus this dispatch’s full new delta (including Cargo.toml vs 13eb521a), excluding unchanged copied vendor lines. Manager a49 caps remaining judged lint to one more pass: remaining non-measurement-correctness findings are limitations, not another repair round.

### Completed local capture set (diagnostic, not pricing)

The cut-enabled debug capture and both release captures passed the same kept-branch real e2e journey. Debug before/after share engine build ID BE1EC92630BD1D4C6A1A90F35B7F66840. Release before/after share 5E17C5E3EE0A65A16CFE5D4DEC85DCF60. These IDs were read from each sampler profile’s libs entry for onepipeline. Release producer command was `cargo build --release --locked --offline --bin onepipeline --config 'profile.release.strip="none"'`; its e2e binary was built with the same release profile. Real subprocess helpers were built with `cargo build --release --locked --offline --package onepipeline-testfakes --package onevcs --bins --config 'profile.release.strip="none"'`. The harness also makes its normal debug freshness calls. Symbol retention differs from the stripped distribution; launch tracing remains enabled, so these are instrumented release-code observations, not uninstrumented user-latency measurements.

The recording command is the historical Docker command above, changing the binary argument to target/release/deps/e2e-2ff01934f6bdc286 and the output directory to scratch/local-release-preserve-before or -after. Cut captures add `-e ONEPIPELINE_STARTUP_CUT=contributors`. Their source commit and complete dirty-source patch are recorded by the collector: debug-after is based on 8bee35d0; both release captures are based on 0113138a. The engine implementation itself is unchanged from a792bafc. See local-*-source.diff.gz and profile evidence for exact provenance. No local before/after wall-clock difference or projected PR saving is computed.

Raw profiles, symbols, source patches, launch trace and complete CPU fold output are under spikes/engine-startup/evidence/local-debug-after.*, local-release-before.* and local-release-after.*. The two primary process IDs remain 90=start and 583=adopt in all captures. Each number below is read by `python scripts/engine-startup-cpu.py PROFILE SYMBOLS` from those recorded files; the raw originals and logs remain in scratch/3469976-4/local-BUILD-preserve-SIDE.

#### debug after

```text
pid 90.1: sampled CPU 595.995 ms; lifetime 2049.874 ms; threads 274
nearest first-party module (otherwise leaf):
  118.114 ms (19.82%; cumulative 19.82%) onevcs::git
  47.935 ms (8.04%; cumulative 27.86%) onepipeline::ledger
  37.770 ms (6.34%; cumulative 34.20%) onevcs::launch_trace
  35.351 ms (5.93%; cumulative 40.13%) libc.so.6!unresolved
  33.113 ms (5.56%; cumulative 45.69%) poll
  25.250 ms (4.24%; cumulative 49.92%) onepipeline::writeback
  22.171 ms (3.72%; cumulative 53.64%) onemessagebus::schema
  18.018 ms (3.02%; cumulative 56.67%) onemessagebus::envelope
  15.608 ms (2.62%; cumulative 59.28%) onevcs::home
  14.919 ms (2.50%; cumulative 61.79%) onepipeline::projection
  13.007 ms (2.18%; cumulative 63.97%) onepipeline::engine
  12.442 ms (2.09%; cumulative 66.06%) onevcs::lock
  9.182 ms (1.54%; cumulative 67.60%) onepipeline::plan
  8.563 ms (1.44%; cumulative 69.03%) onepipeline::summary
  7.725 ms (1.30%; cumulative 70.33%) onepipeline::checkpoint
  7.717 ms (1.29%; cumulative 71.63%) fun_f230
  5.932 ms (1.00%; cumulative 72.62%) onemessagebus::transport
  5.214 ms (0.87%; cumulative 73.50%) onepipeline::vocabulary
  4.976 ms (0.83%; cumulative 74.33%) _close
  4.896 ms (0.82%; cumulative 75.15%) onetaskgraph_local_md::yaml
  4.649 ms (0.78%; cumulative 75.93%) onepipeline::payload
  4.635 ms (0.78%; cumulative 76.71%) onepipeline::report
  4.094 ms (0.69%; cumulative 77.40%) onevcs::store
  4.051 ms (0.68%; cumulative 78.08%) onepipeline::telemetry
  4.047 ms (0.68%; cumulative 78.76%) onepipeline::executor
  3.840 ms (0.64%; cumulative 79.40%) onetaskgraph_local_md::staged
  3.643 ms (0.61%; cumulative 80.01%) onepipeline::vcs
nearest first-party function (otherwise leaf):
  40.815 ms (6.85%; cumulative 6.85%) onevcs::git::bounded::<onevcs::git::run_fed::{closure#4}::{closure#3}>
  35.351 ms (5.93%; cumulative 12.78%) libc.so.6!unresolved
  33.113 ms (5.56%; cumulative 18.34%) poll
  20.830 ms (3.49%; cumulative 21.83%) <onemessagebus::schema::Registry>::register_schema
  18.753 ms (3.15%; cumulative 24.98%) <onevcs::git::PipeCapture>::start::<std::process::ChildStdout>
  17.381 ms (2.92%; cumulative 27.89%) onevcs::git::wait_for_exit
  17.322 ms (2.91%; cumulative 30.80%) <onevcs::git::PipeCapture>::start::<std::process::ChildStderr>
  14.934 ms (2.51%; cumulative 33.31%) onepipeline::ledger::publish
  14.554 ms (2.44%; cumulative 35.75%) onevcs::git::run_fed
  13.147 ms (2.21%; cumulative 37.95%) onevcs::home::ensure_dir
  9.104 ms (1.53%; cumulative 39.48%) onevcs::lock::wait_for
  7.758 ms (1.30%; cumulative 40.78%) onevcs::launch_trace::sleep
  7.717 ms (1.29%; cumulative 42.08%) fun_f230
  6.946 ms (1.17%; cumulative 43.24%) <onevcs::launch_trace::Tracer>::write
  6.695 ms (1.12%; cumulative 44.37%) onepipeline::ledger::disk::publish
  6.365 ms (1.07%; cumulative 45.43%) <onevcs::launch_trace::Launch>::spawned
  6.341 ms (1.06%; cumulative 46.50%) <onemessagebus::envelope::Envelope<onepipeline::vocabulary::Agent> as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::SliceRead>>
  4.976 ms (0.83%; cumulative 47.33%) _close
  4.896 ms (0.82%; cumulative 48.15%) onetaskgraph_local_md::yaml::from_str::<onetaskgraph_local_md::FrontMatter>
  4.847 ms (0.81%; cumulative 48.97%) onevcs::launch_trace::output
  4.358 ms (0.73%; cumulative 49.70%) <onevcs::launch_trace::Wait>::into_launch
  4.187 ms (0.70%; cumulative 50.40%) <onevcs::git::PipeCapture>::finish
  4.167 ms (0.70%; cumulative 51.10%) <onepipeline::ledger::RunPaths>::create
  3.867 ms (0.65%; cumulative 51.75%) onevcs::launch_trace::launch
  3.840 ms (0.64%; cumulative 52.39%) onetaskgraph_local_md::staged
  3.623 ms (0.61%; cumulative 53.00%) onepipeline::ledger::disk::read
  3.609 ms (0.61%; cumulative 53.61%) onepipeline::engine::journalling_relay::{closure#0}
  3.447 ms (0.58%; cumulative 54.19%) <onepipeline::projection::RunState as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>>
  3.444 ms (0.58%; cumulative 54.76%) onetaskgraph_core::resolve::validate_sources
  3.209 ms (0.54%; cumulative 55.30%) syscall
  3.077 ms (0.52%; cumulative 55.82%) <onemessagebus::transport::LocalTransport as onemessagebus::transport::Transport>::fingerprint
  3.030 ms (0.51%; cumulative 56.33%) _munmap
  3.025 ms (0.51%; cumulative 56.83%) <onepipeline::vocabulary::Labels as serde_core::de::Deserialize>::deserialize::<serde_json::value::Value>
  2.942 ms (0.49%; cumulative 57.33%) onepipeline::ledger::read_range
  2.763 ms (0.46%; cumulative 57.79%) onepipeline::driver::attach
  2.695 ms (0.45%; cumulative 58.24%) onepipeline::engine::converge
  2.583 ms (0.43%; cumulative 58.68%) onepipeline::ledger::disk::sync_contents
  2.567 ms (0.43%; cumulative 59.11%) onevcs::policy::parse
  2.563 ms (0.43%; cumulative 59.54%) <onepipeline::projection::RunState as core::clone::Clone>::clone
  2.393 ms (0.40%; cumulative 59.94%) <onepipeline::summary::RunSummary>::derive
  2.339 ms (0.39%; cumulative 60.33%) onemessagebus::envelope::take_or_default::<serde_json::map::Map<alloc::string::String, serde_json::value::Value>, serde_json::error::Error>
  2.336 ms (0.39%; cumulative 60.72%) <onepipeline::summary::RunSummary as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>, serde_json::ser::PrettyFormatter>>
  2.317 ms (0.39%; cumulative 61.11%) onevcs::launch_trace::wait
  2.307 ms (0.39%; cumulative 61.50%) onepipeline::release::ask_until_dropped
  2.305 ms (0.39%; cumulative 61.89%) onepipeline::writeback::worker
  2.296 ms (0.39%; cumulative 62.27%) <jsonschema::paths::LOCATION_BUFFER::{K#0}::{closure#0} as core::ops::function::FnOnce<(core::option::Option<&mut core::option::Option<core::cell::RefCell<alloc::string::String>>>,)>>::call_once
  2.295 ms (0.39%; cumulative 62.66%) onepipeline::writeback::task_document
  2.277 ms (0.38%; cumulative 63.04%) onepipeline::ledger::disk::synced
  2.226 ms (0.37%; cumulative 63.41%) <onepipeline::checkpoint::Projected>::take_with
  2.172 ms (0.36%; cumulative 63.78%) _madvise
  2.107 ms (0.35%; cumulative 64.13%) <onemessagebus::envelope::Envelope<onepipeline::vocabulary::Agent> as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::StrRead>>
  2.077 ms (0.35%; cumulative 64.48%) onevcs::lock::open_at
  2.010 ms (0.34%; cumulative 64.82%) onevcs::native_refs::local_value
  1.954 ms (0.33%; cumulative 65.14%) fun_188d80
  1.913 ms (0.32%; cumulative 65.46%) onepipeline::writeback::validated_within::{closure#0}
  1.902 ms (0.32%; cumulative 65.78%) <onepipeline::plan::Node as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>>
  1.867 ms (0.31%; cumulative 66.10%) onepipeline::writeback::refresh_shadow
  1.833 ms (0.31%; cumulative 66.40%) onepipeline::executor::make_node_scratch_dir
  1.714 ms (0.29%; cumulative 66.69%) read
  1.660 ms (0.28%; cumulative 66.97%) onevcs::store::load
  1.642 ms (0.28%; cumulative 67.25%) <&mut serde_json::de::Deserializer<serde_json::read::SliceRead> as serde_core::de::Deserializer>::deserialize_any::<<serde_json::value::Value as serde_core::de::Deserialize>::deserialize::ValueVisitor>
  1.631 ms (0.27%; cumulative 67.52%) onepipeline::writeback::carry_the_difference
  1.576 ms (0.26%; cumulative 67.78%) onevcs::boundary::manifests::committed_source
  1.521 ms (0.26%; cumulative 68.04%) <alloc::vec::Vec<regex_automata::nfa::thompson::map::Utf8BoundedEntry>>::extend_with
  1.513 ms (0.25%; cumulative 68.29%) <onemessagebus::envelope::Envelope<onepipeline::vocabulary::Agent> as schemars::JsonSchema>::json_schema
  1.513 ms (0.25%; cumulative 68.55%) onevcs::workspace::existing
  1.499 ms (0.25%; cumulative 68.80%) <onemessagebus::transport::LocalTransport>::lock
  1.492 ms (0.25%; cumulative 69.05%) onepipeline::report::open_no_follow
  1.455 ms (0.24%; cumulative 69.29%) privateterms::levels::read_dir_level
  1.418 ms (0.24%; cumulative 69.53%) onepipeline::loopstats::journal_read
  1.401 ms (0.24%; cumulative 69.77%) <onepipeline::ledger::LaunchRecord as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::StrRead>>
  1.371 ms (0.23%; cumulative 70.00%) onepipeline::projection::millis_of
  1.361 ms (0.23%; cumulative 70.22%) onepipeline::vcs::helpers_for
  1.360 ms (0.23%; cumulative 70.45%) onepipeline::engine::drain
  1.356 ms (0.23%; cumulative 70.68%) onepipeline::report::retain
  1.348 ms (0.23%; cumulative 70.91%) <onemessagebus::envelope::Envelope<onepipeline::vocabulary::Agent> as serde_core::de::Deserialize>::deserialize::<serde_json::value::Value>
  1.278 ms (0.21%; cumulative 71.12%) <onetaskgraph_local_md::LocalMdConfig as schemars::JsonSchema>::json_schema
  1.273 ms (0.21%; cumulative 71.33%) <onepipeline::plan::Node as core::cmp::PartialEq>::eq
  1.261 ms (0.21%; cumulative 71.55%) onevcs::lock::record_owner
  1.238 ms (0.21%; cumulative 71.75%) onepipeline::ledger::temporary
  1.235 ms (0.21%; cumulative 71.96%) onevcs::home::root
  1.230 ms (0.21%; cumulative 72.17%) onevcs::boundary::derive::derive
  1.226 ms (0.21%; cumulative 72.37%) onevcs::home::atomic_write_before_replace::<onevcs::home::atomic_write::{closure#0}>
  1.220 ms (0.20%; cumulative 72.58%) onepipeline::ledger::read_appended_envelope_lines::{closure#0}::{closure#0}
  1.220 ms (0.20%; cumulative 72.78%) onevcs::git::lfs::repository_configuration
  1.193 ms (0.20%; cumulative 72.98%) <onevcs::checkout_reads::Hold as core::ops::drop::Drop>::drop
  1.183 ms (0.20%; cumulative 73.18%) onepipeline::writeback::project
  1.166 ms (0.20%; cumulative 73.38%) onepipeline::executor::available_memory
  1.155 ms (0.19%; cumulative 73.57%) onepipeline::journal::stream_queues::<onepipeline::journal::merged_order::{closure#0}>
  1.145 ms (0.19%; cumulative 73.76%) onevcs::store::migrate
  1.081 ms (0.18%; cumulative 73.94%) onepipeline::ledger::open_healed
  1.048 ms (0.18%; cumulative 74.12%) <onepipeline::executor::LocalExecutor as onepipeline::executor::Executor>::capacity
  1.040 ms (0.17%; cumulative 74.29%) <onepipeline::telemetry::Bucket as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>, serde_json::ser::PrettyFormatter>>
  1.020 ms (0.17%; cumulative 74.47%) onetaskgraph_core::config::discovery::nearest_project_document
  1.008 ms (0.17%; cumulative 74.63%) <core::hash::sip::Hasher<core::hash::sip::Sip13Rounds> as core::hash::Hasher>::write
  0.983 ms (0.16%; cumulative 74.80%) <onepipeline::cli::Cli as clap_builder::derive::Parser>::parse
  0.970 ms (0.16%; cumulative 74.96%) onepipeline::journal::merge_queues::<onepipeline::journal::merged_order::{closure#0}>
  0.944 ms (0.16%; cumulative 75.12%) onepipeline::checkwatch::ending
  0.927 ms (0.16%; cumulative 75.28%) <onepipeline::summary::Maintainer>::derived
  0.923 ms (0.15%; cumulative 75.43%) onepipeline::agentgraph::relayed_lines::<std::io::buffered::bufreader::BufReader<std::process::ChildStdout>, <onepipeline::agentgraph::ProcessGraphRun>::events::{closure#0}>::{closure#1}
  0.910 ms (0.15%; cumulative 75.58%) onepipeline::checkpoint::length_of
  0.907 ms (0.15%; cumulative 75.74%) onepipeline::report::failed_verdicts
  0.902 ms (0.15%; cumulative 75.89%) onepipeline::channel::layout::registry
  0.900 ms (0.15%; cumulative 76.04%) onepipeline::checkpoint::fold_in_merge_order
  0.899 ms (0.15%; cumulative 76.19%) <onepipeline::ledger::Handover>::hold_within
  0.894 ms (0.15%; cumulative 76.34%) <onetaskgraph_local_md::LocalMdSource>::locate
  0.894 ms (0.15%; cumulative 76.49%) <onepipeline::appendstamp::Stamp as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::StrRead>>
  0.890 ms (0.15%; cumulative 76.64%) <onepipeline::projection::RunState>::awaiting_human_action
  0.885 ms (0.15%; cumulative 76.79%) <onepipeline::projection::RunState>::settled
  0.884 ms (0.15%; cumulative 76.94%) <onepipeline::summary::Maintainer>::write
  0.883 ms (0.15%; cumulative 77.08%) fun_11ca00
  0.880 ms (0.15%; cumulative 77.23%) onepipeline::report::read
  0.875 ms (0.15%; cumulative 77.38%) <onevcs::launch_trace::Launch>::exited
  0.865 ms (0.15%; cumulative 77.52%) onepipeline::sys::platform_append_all
  0.862 ms (0.14%; cumulative 77.67%) onepipeline::summary::journal_stamp
  0.860 ms (0.14%; cumulative 77.81%) std::sys::backtrace::__rust_begin_short_backtrace::<<onevcs::git::PipeCapture>::start<std::process::ChildStdout>::{closure#0}, onevcs::git::Captured>
  0.854 ms (0.14%; cumulative 77.96%) onepipeline::writeback::update
  0.833 ms (0.14%; cumulative 78.09%) onepipeline::projection::fold_activity
  0.822 ms (0.14%; cumulative 78.23%) onepipeline::sys::platform_process_start_token
  0.817 ms (0.14%; cumulative 78.37%) <onepipeline::writeback::Writeback>::closing_out_until::<<onepipeline::writeback::Pending>::unlanded>
  0.804 ms (0.13%; cumulative 78.50%) onevcs::git::make_executable
  0.800 ms (0.13%; cumulative 78.64%) core::ptr::drop_glue::<core::option::Option<core::result::Result<(), alloc::boxed::Box<dyn core::any::Any + core::marker::Send>>>>
  0.797 ms (0.13%; cumulative 78.77%) <onemessagebus::envelope::Envelope<onevcs::vocabulary::VcsEvents> as serde_core::de::Deserialize>::deserialize::<serde_json::value::Value>
  0.793 ms (0.13%; cumulative 78.91%) onepipeline::writeback::validated_within
  0.784 ms (0.13%; cumulative 79.04%) <onepipeline::checkpoint::Projected>::write
  0.783 ms (0.13%; cumulative 79.17%) <onepipeline::plan::TaskRecord as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::StrRead>>
  0.773 ms (0.13%; cumulative 79.30%) <onepipeline::vcs::DispatchSession>::read_from
  0.759 ms (0.13%; cumulative 79.43%) <onepipeline::writeback::Attempt>::open
  0.748 ms (0.13%; cumulative 79.55%) <onepipeline::vocabulary::Labels as serde_core::ser::Serialize>::serialize::<serde_json::value::ser::Serializer>
  0.731 ms (0.12%; cumulative 79.67%) <<onepipeline::plan::Node as serde_core::de::Deserialize>::deserialize::__Visitor as serde_core::de::Visitor>::visit_map::<&mut serde_json::value::de::MapDeserializer>
  0.725 ms (0.12%; cumulative 79.80%) <onevcs::workspace::Record as serde_core::ser::Serialize>::serialize::<serde_json::value::ser::Serializer>
  0.675 ms (0.11%; cumulative 79.91%) <onemessagebus::redact::Redactor>::from_env
  0.672 ms (0.11%; cumulative 80.02%) fun_ab630
self function:
  105.815 ms (17.75%; cumulative 17.75%) libc.so.6!unresolved
  33.921 ms (5.69%; cumulative 23.45%) poll
  31.777 ms (5.33%; cumulative 28.78%) _open64
  27.687 ms (4.65%; cumulative 33.42%) syscall
  27.468 ms (4.61%; cumulative 38.03%) mkdir
  12.902 ms (2.16%; cumulative 40.20%) statx
  10.955 ms (1.84%; cumulative 42.03%) _close
  9.948 ms (1.67%; cumulative 43.70%) fun_ab630
  9.564 ms (1.60%; cumulative 45.31%) fun_188d80
  7.976 ms (1.34%; cumulative 46.65%) _write
  7.963 ms (1.34%; cumulative 47.98%) rename
  7.717 ms (1.29%; cumulative 49.28%) fun_f230
  6.252 ms (1.05%; cumulative 50.33%) read
  5.515 ms (0.93%; cumulative 51.25%) _munmap
  5.357 ms (0.90%; cumulative 52.15%) malloc
  5.299 ms (0.89%; cumulative 53.04%) clock_nanosleep
  4.274 ms (0.72%; cumulative 53.76%) fsync
  4.110 ms (0.69%; cumulative 54.45%) fun_188cf0
  4.107 ms (0.69%; cumulative 55.14%) <core::hash::sip::Hasher<core::hash::sip::Sip13Rounds> as core::hash::Hasher>::write
  3.569 ms (0.60%; cumulative 55.73%) core::slice::raw::from_raw_parts::precondition_check
  3.124 ms (0.52%; cumulative 56.26%) wait4
  3.073 ms (0.52%; cumulative 56.77%) _mmap
  3.048 ms (0.51%; cumulative 57.29%) _libc_free
  2.900 ms (0.49%; cumulative 57.77%) pipe2
  2.757 ms (0.46%; cumulative 58.24%) <jsonschema::paths::LOCATION_BUFFER::{K#0}::{closure#0} as core::ops::function::FnOnce<(core::option::Option<&mut core::option::Option<core::cell::RefCell<alloc::string::String>>>,)>>::call_once
  2.585 ms (0.43%; cumulative 58.67%) referencing::spec::draft202012::maybe_in_subresource
  2.583 ms (0.43%; cumulative 59.10%) fun_98d50
  2.172 ms (0.36%; cumulative 59.47%) _madvise
  2.156 ms (0.36%; cumulative 59.83%) <*const ()>::is_aligned_to
  2.008 ms (0.34%; cumulative 60.17%) <onepipeline::summary::RunSummary>::derive
  1.937 ms (0.33%; cumulative 60.49%) fun_ab230
  1.891 ms (0.32%; cumulative 60.81%) fun_188620
  1.703 ms (0.29%; cumulative 61.09%) <alloc::collections::btree::node::Handle<alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Mut, (&str, u32), (&str, u64, u64, u64), alloc::collections::btree::node::marker::Leaf>, alloc::collections::btree::node::marker::Edge>>::insert_recursing::<alloc::alloc::Global, <alloc::collections::btree::map::entry::VacantEntry<(&str, u32), (&str, u64, u64, u64)>>::insert_entry::{closure#0}>
  1.672 ms (0.28%; cumulative 61.37%) clock_gettime
  1.658 ms (0.28%; cumulative 61.65%) _mprotect
  1.658 ms (0.28%; cumulative 61.93%) unlinkat
  1.642 ms (0.28%; cumulative 62.21%) <&mut serde_json::de::Deserializer<serde_json::read::SliceRead> as serde_core::de::Deserializer>::deserialize_any::<<serde_json::value::Value as serde_core::de::Deserialize>::deserialize::ValueVisitor>
  1.596 ms (0.27%; cumulative 62.47%) core::ptr::drop_glue::<indexmap::Bucket<alloc::string::String, serde_json::value::Value>>
  1.530 ms (0.26%; cumulative 62.73%) <std::hash::random::RandomState as core::hash::BuildHasher>::hash_one::<&str>
  1.521 ms (0.26%; cumulative 62.99%) <alloc::vec::Vec<regex_automata::nfa::thompson::map::Utf8BoundedEntry>>::extend_with
  1.491 ms (0.25%; cumulative 63.24%) <hashbrown::raw::RawTableInner>::find_insert_index
  1.412 ms (0.24%; cumulative 63.47%) <std::path::Components>::parse_next_component_back
  1.368 ms (0.23%; cumulative 63.70%) <hashbrown::raw::RawTable<usize>>::reserve_rehash::<indexmap::inner::get_hash<alloc::string::String, serde_json::value::Value>::{closure#0}>
  1.367 ms (0.23%; cumulative 63.93%) <serde_path_to_error::de::TrackedSeed<core::marker::PhantomData<alloc::collections::btree::map::BTreeMap<alloc::string::String, serde_json::value::Value>>> as serde_core::de::DeserializeSeed>::deserialize::<serde_saphyr::de::deserializer::YamlDeserializer>
  1.334 ms (0.22%; cumulative 64.16%) pthread_create
  1.298 ms (0.22%; cumulative 64.37%) <serde_json::read::SliceRead>::skip_to_escape
  1.296 ms (0.22%; cumulative 64.59%) <core::slice::iter::Iter<std::sync::mpmc::waker::Entry> as core::iter::traits::iterator::Iterator>::position::<<std::sync::mpmc::waker::Waker>::try_select::{closure#0}>
  1.239 ms (0.21%; cumulative 64.80%) fstatat
  1.217 ms (0.20%; cumulative 65.00%) core::intrinsics::rotate_left::<u64>
  1.204 ms (0.20%; cumulative 65.20%) fun_283e0
  1.185 ms (0.20%; cumulative 65.40%) <std::sync::mpmc::list::Channel<std::fs::File>>::recv::{closure#1}
  1.142 ms (0.19%; cumulative 65.60%) <std::sync::mpmc::list::Channel<onepipeline::engine::Message>>::recv
  1.116 ms (0.19%; cumulative 65.78%) <regex_syntax::hir::Hir>::class
  1.101 ms (0.18%; cumulative 65.97%) <jsonschema::compiler::Context>::in_subresource
  1.082 ms (0.18%; cumulative 66.15%) fun_a9c70
  1.065 ms (0.18%; cumulative 66.33%) <alloc::raw_vec::RawVecInner>::finish_grow
  1.052 ms (0.18%; cumulative 66.50%) readlink
  0.989 ms (0.17%; cumulative 66.67%) <std::thread::lifecycle::JoinInner<onevcs::git::Captured>>::join
  0.958 ms (0.16%; cumulative 66.83%) core::ptr::drop_glue::<alloc::raw_vec::RawVec<u8>>
  0.927 ms (0.16%; cumulative 66.99%) <alloc::collections::btree::navigate::LazyLeafRange<alloc::collections::btree::node::marker::Dying, alloc::string::String, alloc::string::String>>::take_front
  0.921 ms (0.15%; cumulative 67.14%) brk
  0.916 ms (0.15%; cumulative 67.29%) <hashbrown::raw::RawTableInner>::ctrl
  0.906 ms (0.15%; cumulative 67.45%) <alloc::vec::Vec<u8> as core::cmp::PartialEq>::eq
  0.898 ms (0.15%; cumulative 67.60%) <std::sync::mpmc::waker::SyncWaker>::notify
  0.890 ms (0.15%; cumulative 67.75%) ftruncate
  0.883 ms (0.15%; cumulative 67.89%) fun_11ca00
  0.869 ms (0.15%; cumulative 68.04%) <<serde_saphyr::de::deserializer::YamlDeserializer as serde_core::de::Deserializer>::deserialize_map::MA as serde_core::de::MapAccess>::next_key_seed::<core::marker::PhantomData<serde_core::de::ignored_any::IgnoredAny>>
  0.860 ms (0.14%; cumulative 68.18%) std::sys::backtrace::__rust_begin_short_backtrace::<<onevcs::git::PipeCapture>::start<std::process::ChildStdout>::{closure#0}, onevcs::git::Captured>
  0.835 ms (0.14%; cumulative 68.32%) <&mut serde_json::value::de::MapDeserializer as serde_core::de::MapAccess>::next_entry::<alloc::string::String, serde_json::value::Value>
  0.822 ms (0.14%; cumulative 68.46%) core::str::converts::from_utf8
  0.821 ms (0.14%; cumulative 68.60%) core::ptr::drop_glue::<serde_json::value::Value>
  0.811 ms (0.14%; cumulative 68.74%) onevcs::git::bounded::<onevcs::git::run_fed::{closure#4}::{closure#3}>
  0.804 ms (0.13%; cumulative 68.87%) chmod
  0.800 ms (0.13%; cumulative 69.01%) core::ptr::drop_glue::<core::option::Option<core::result::Result<(), alloc::boxed::Box<dyn core::any::Any + core::marker::Send>>>>
  0.783 ms (0.13%; cumulative 69.14%) core::ptr::drop_glue::<referencing::resolver::Resolver>
  0.783 ms (0.13%; cumulative 69.27%) <std::process::Command>::get_args
  0.782 ms (0.13%; cumulative 69.40%) <alloc::vec::Vec<serde_json::value::Value> as core::clone::Clone>::clone
  0.778 ms (0.13%; cumulative 69.53%) core::ptr::copy_nonoverlapping::precondition_check
  0.768 ms (0.13%; cumulative 69.66%) serde_json::ser::format_escaped_str_contents::<&mut alloc::vec::Vec<u8>, serde_json::ser::CompactFormatter>
  0.758 ms (0.13%; cumulative 69.79%) <u64 as core::fmt::Display>::fmt
  0.734 ms (0.12%; cumulative 69.91%) <std::fs::DirBuilder>::_create
  0.732 ms (0.12%; cumulative 70.03%) <usize as core::convert::TryFrom<u64>>::try_from
  0.731 ms (0.12%; cumulative 70.15%) <<onepipeline::plan::Node as serde_core::de::Deserialize>::deserialize::__Visitor as serde_core::de::Visitor>::visit_map::<&mut serde_json::value::de::MapDeserializer>
  0.729 ms (0.12%; cumulative 70.28%) <alloc::alloc::Global>::alloc_impl_runtime
  0.718 ms (0.12%; cumulative 70.40%) <jsonschema::keywords::all_of::AllOfValidator<jsonschema_value::serde_json::SerdeJson> as jsonschema::validator::Validate>::validate
  0.711 ms (0.12%; cumulative 70.52%) <hashbrown::raw::RawTableInner>::find_or_find_insert_index_inner
  0.707 ms (0.12%; cumulative 70.64%) <hashbrown::map::HashMap<alloc::string::String, jsonschema::node::SchemaNode, ahash::random_state::RandomState>>::get::<str>
  0.687 ms (0.12%; cumulative 70.75%) _close_nocancel
  0.686 ms (0.12%; cumulative 70.87%) <indexmap::map::entry::Entry<alloc::string::String, serde_json::value::Value>>::new
  0.658 ms (0.11%; cumulative 70.98%) <alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Immut, alloc::string::String, onepipeline::plan::Node, alloc::collections::btree::node::marker::LeafOrInternal>>::find_key_index::<alloc::string::String>
  0.657 ms (0.11%; cumulative 71.09%) <&mut alloc::vec::Vec<u8> as std::io::Write>::write_all
  0.654 ms (0.11%; cumulative 71.20%) <std::hash::random::RandomState as core::hash::BuildHasher>::hash_one::<&serde_saphyr::de::key_nodes::KeyFingerprint>
  0.642 ms (0.11%; cumulative 71.30%) onevcs::native_refs::pinned
  0.640 ms (0.11%; cumulative 71.41%) <alloc::collections::btree::map::BTreeMap<onemessagebus::schema::SchemaId, onemessagebus::schema::Registered>>::insert
  0.628 ms (0.11%; cumulative 71.52%) <schemars::generate::SchemaGenerator>::subschema_for::<core::option::Option<alloc::string::String>>
  0.627 ms (0.11%; cumulative 71.62%) _open64_nocancel
  0.621 ms (0.10%; cumulative 71.73%) <jsonschema::keywords::enum_::SmallStringEnumValidator<jsonschema::keywords::NotNullable> as jsonschema::validator::Validate>::is_valid
  0.611 ms (0.10%; cumulative 71.83%) <std::time::Instant>::elapsed
  0.610 ms (0.10%; cumulative 71.93%) access
  0.607 ms (0.10%; cumulative 72.03%) onevcs::git::bound_for
  0.607 ms (0.10%; cumulative 72.13%) fun_10f4e0
  0.602 ms (0.10%; cumulative 72.24%) fun_9c7e0
  0.598 ms (0.10%; cumulative 72.34%) <serde_json::value::de::MapDeserializer as serde_core::de::MapAccess>::next_key_seed::<core::marker::PhantomData<<onepipeline::vocabulary::Labels as serde_core::de::Deserialize>::deserialize::__Field>>
  0.590 ms (0.10%; cumulative 72.43%) onevcs::git::default_branch
  0.586 ms (0.10%; cumulative 72.53%) <referencing::draft::Draft>::walk_children::<referencing::registry::build::crawl_schema::{closure#0}>
  0.586 ms (0.10%; cumulative 72.63%) <indexmap::inner::Core<alloc::string::String, serde_json::value::Value>>::push_entry
  0.584 ms (0.10%; cumulative 72.73%) <std::thread::thread::Thread>::park_timeout
  0.578 ms (0.10%; cumulative 72.83%) <core::fmt::Formatter>::write_str
  0.576 ms (0.10%; cumulative 72.92%) <alloc::raw_vec::RawVec<std::sync::mpmc::waker::Entry>>::grow_one
  0.575 ms (0.10%; cumulative 73.02%) fun_aa960
  0.560 ms (0.09%; cumulative 73.11%) <alloc::collections::btree::dedup_sorted_iter::DedupSortedIter<alloc::string::String, serde_json::value::Value, alloc::vec::into_iter::IntoIter<(alloc::string::String, serde_json::value::Value)>> as core::iter::traits::iterator::Iterator>::next
  0.558 ms (0.09%; cumulative 73.21%) <std::thread::local::LocalKey<core::cell::RefCell<core::option::Option<onepipeline::rendercost::Inside>>>>::try_with::<<std::thread::local::LocalKey<core::cell::RefCell<core::option::Option<onepipeline::rendercost::Inside>>>>::with_borrow<onepipeline::rendercost::inside_a_decision::{closure#0}, bool>::{closure#0}, bool>
  0.556 ms (0.09%; cumulative 73.30%) <&std::path::Path as core::convert::AsRef<std::path::Path>>::as_ref
  0.553 ms (0.09%; cumulative 73.39%) <<<onepipeline::checkpoint::Checkpoint as serde_core::de::Deserialize>::deserialize::__Visitor as serde_core::de::Visitor>::visit_map::__DeserializeWith as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::StrRead>>
  0.552 ms (0.09%; cumulative 73.49%) onevcs::launch_trace::wait
  0.551 ms (0.09%; cumulative 73.58%) <schemars::generate::SchemaGenerator>::add_definitions
  0.546 ms (0.09%; cumulative 73.67%) std::env::_var_os
  0.544 ms (0.09%; cumulative 73.76%) core::ptr::drop_glue::<<alloc::vec::into_iter::IntoIter<_, _> as core::ops::drop::Drop>::drop::DropGuard<&str, alloc::alloc::Global>>
  0.543 ms (0.09%; cumulative 73.85%) core::ptr::drop_glue::<alloc::vec::Vec<*const i8>>
  0.539 ms (0.09%; cumulative 73.94%) fun_189710
  0.537 ms (0.09%; cumulative 74.03%) <alloc::vec::Vec<clap_builder::builder::str::Str> as core::ops::drop::Drop>::drop
  0.535 ms (0.09%; cumulative 74.12%) <onevcs::store::RESOLVED::{K#0}::{closure#0} as core::ops::function::FnOnce<(core::option::Option<&mut core::option::Option<core::cell::RefCell<core::option::Option<(usize, alloc::collections::btree::map::BTreeMap<alloc::string::String, onevcs::store::Resolution>)>>>>,)>>::call_once
  0.533 ms (0.09%; cumulative 74.21%) <alloc::collections::btree::map::BTreeMap<alloc::string::String, onepipeline::graph::NodeStatus> as core::ops::drop::Drop>::drop
  0.531 ms (0.09%; cumulative 74.30%) <alloc::vec::Vec<indexmap::Bucket<alloc::string::String, serde_json::value::Value>> as alloc::vec::spec_extend::SpecExtend<&indexmap::Bucket<alloc::string::String, serde_json::value::Value>, core::slice::iter::Iter<indexmap::Bucket<alloc::string::String, serde_json::value::Value>>>>::spec_extend
  0.529 ms (0.09%; cumulative 74.39%) _read_nocancel
  0.529 ms (0.09%; cumulative 74.48%) core::ptr::drop_glue::<alloc::raw_vec::RawVec<serde_json::value::Value>>
  0.528 ms (0.09%; cumulative 74.57%) <alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Mut, schemars::generate::SchemaUid, alloc::collections::btree::set_val::SetValZST, alloc::collections::btree::node::marker::LeafOrInternal>>::search_tree::<schemars::generate::SchemaUid>
  0.528 ms (0.09%; cumulative 74.66%) <regex_automata::nfa::thompson::nfa::Inner>::add
  0.526 ms (0.09%; cumulative 74.74%) onepipeline::channel::registry::{closure#0}
  0.525 ms (0.09%; cumulative 74.83%) <std::sync::poison::Flag>::done
  0.524 ms (0.09%; cumulative 74.92%) core::ptr::drop_glue::<alloc::vec::Vec<u8>>
  0.523 ms (0.09%; cumulative 75.01%) <regex_syntax::ast::parse::ParserI<&mut regex_syntax::ast::parse::Parser>>::parse_with_comments
  0.522 ms (0.09%; cumulative 75.10%) <std::io::error::Error>::kind
  0.521 ms (0.09%; cumulative 75.18%) <std::path::PathBuf as core::convert::Into<std::path::PathBuf>>::into
  0.521 ms (0.09%; cumulative 75.27%) <serde_path_to_error::de::TrackedSeed<core::marker::PhantomData<serde_json::value::Value>> as serde_core::de::DeserializeSeed>::deserialize::<serde_saphyr::de::deserializer::YamlDeserializer>
  0.517 ms (0.09%; cumulative 75.36%) <core::str::pattern::MultiCharEqSearcher<<char>::is_whitespace> as core::str::pattern::Searcher>::next
  0.516 ms (0.09%; cumulative 75.44%) <alloc::collections::btree::node::Handle<alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Immut, alloc::string::String, onepipeline::summary::NodePreserved, alloc::collections::btree::node::marker::LeafOrInternal>, alloc::collections::btree::node::marker::KV>>::next_leaf_edge
  0.515 ms (0.09%; cumulative 75.53%) <usize as core::slice::index::SliceIndex<[_]>>::get_unchecked::precondition_check
  0.514 ms (0.09%; cumulative 75.62%) <jsonschema::keywords::additional_properties::AdditionalPropertiesValidator as jsonschema::validator::Validate>::validate
  0.513 ms (0.09%; cumulative 75.70%) fluent_uri::normalize::normalize_estr
  0.512 ms (0.09%; cumulative 75.79%) <usize>::unchecked_add::precondition_check
  0.511 ms (0.09%; cumulative 75.87%) fun_adf20
  0.508 ms (0.09%; cumulative 75.96%) <std::sync::mpmc::list::Channel<()>>::recv
  0.508 ms (0.09%; cumulative 76.04%) getrlimit
  0.508 ms (0.09%; cumulative 76.13%) alloc::fmt::format::format_inner
  0.507 ms (0.09%; cumulative 76.21%) <&serde_json::map::Map<alloc::string::String, serde_json::value::Value> as jsonschema_value::Object<jsonschema_value::serde_json::SerdeJson>>::get
  0.507 ms (0.09%; cumulative 76.30%) <alloc::collections::btree::map::BTreeMap<alloc::string::String, onepipeline::plan::Node>>::contains_key::<alloc::string::String>
  0.506 ms (0.08%; cumulative 76.38%) core::ptr::drop_glue::<core::array::iter::IntoIter<&str, 7>>
  0.505 ms (0.08%; cumulative 76.47%) <core::option::Option<onevcs::rules::MergePolicy>>::is_none
  0.504 ms (0.08%; cumulative 76.55%) core::ptr::drop_glue::<std::io::error::Error>
  0.504 ms (0.08%; cumulative 76.64%) <core::option::Option<alloc::string::String>>::unwrap_or_else::<onepipeline::vcs::kind_of::{closure#1}>
  0.503 ms (0.08%; cumulative 76.72%) <serde_json::map::Map<alloc::string::String, serde_json::value::Value> as serde_core::de::Deserializer>::deserialize_any::<<alloc::collections::btree::map::BTreeMap<_, _> as serde_core::de::Deserialize>::deserialize::MapVisitor<alloc::string::String, onevcs::registry::Identity>>
  0.503 ms (0.08%; cumulative 76.81%) <core::option::Option<alloc::vec::Vec<onepipeline::plan::Node>>>::is_some
  0.502 ms (0.08%; cumulative 76.89%) core::ptr::drop_glue::<onepipeline::projection::Doing>
  0.501 ms (0.08%; cumulative 76.98%) <hashbrown::raw::RawTable<(jsonschema::compiler::LocationCacheKey, alloc::sync::Arc<std::sync::once_lock::OnceLock<jsonschema::keywords::unevaluated_items::ItemsValidators>>)>>::remove_entry::<hashbrown::map::equivalent_key<jsonschema::compiler::LocationCacheKey, jsonschema::compiler::LocationCacheKey, alloc::sync::Arc<std::sync::once_lock::OnceLock<jsonschema::keywords::unevaluated_items::ItemsValidators>>>::{closure#0}>
  0.501 ms (0.08%; cumulative 77.06%) <std::path::Components as core::iter::traits::double_ended::DoubleEndedIterator>::next_back
  0.500 ms (0.08%; cumulative 77.14%) <core::ops::control_flow::ControlFlow<core::cmp::Reverse<(&str, &str, usize)>> as core::ops::try_trait::FromResidual<core::ops::control_flow::ControlFlow<core::cmp::Reverse<(&str, &str, usize)>, core::convert::Infallible>>>::from_residual
  0.500 ms (0.08%; cumulative 77.23%) <granit_parser::parser::Parser<granit_parser::input::str::StrInput> as granit_parser::parser::ParserTrait>::next_event
  0.500 ms (0.08%; cumulative 77.31%) <std::fs::File as fs4::file_ext::sync_impl::std_impl::FileExt>::unlock
  0.499 ms (0.08%; cumulative 77.40%) core::ptr::drop_glue::<[std::ffi::os_str::OsString]>
  0.499 ms (0.08%; cumulative 77.48%) <serde_json::value::de::SeqDeserializer as serde_core::de::SeqAccess>::next_element_seed::<core::marker::PhantomData<onemessagebus::envelope::ArtifactRef>>
  0.499 ms (0.08%; cumulative 77.56%) <alloc::vec::into_iter::IntoIter<indexmap::Bucket<alloc::string::String, serde_json::value::Value>>>::as_raw_mut_slice
  0.499 ms (0.08%; cumulative 77.65%) <std::sys::process::unix::common::cstring_array::CStringArray as core::ops::drop::Drop>::drop
  0.498 ms (0.08%; cumulative 77.73%) <core::num::nonzero::NonZero<u32> as core::str::traits::FromStr>::from_str
  0.496 ms (0.08%; cumulative 77.81%) <std::hash::random::RandomState as core::hash::BuildHasher>::hash_one::<&alloc::string::String>
  0.495 ms (0.08%; cumulative 77.90%) <core::slice::iter::Iter<u8> as core::iter::traits::double_ended::DoubleEndedIterator>::next_back
  0.494 ms (0.08%; cumulative 77.98%) <core::result::Result<(), std::io::error::Error>>::map_err::<onepipeline::error::Error, <onepipeline::ledger::RunPaths>::create::{closure#0}>
  0.493 ms (0.08%; cumulative 78.06%) <std::collections::hash::map::Entry<&str, alloc::vec::Vec<usize>>>::or_default
  0.493 ms (0.08%; cumulative 78.14%) <alloc::collections::btree::map::BTreeMap<alloc::string::String, onepipeline::vcs::DispatchSession>>::keys
  0.493 ms (0.08%; cumulative 78.23%) std::sys::pal::unix::stack_overflow::imp::drop_handler
  0.492 ms (0.08%; cumulative 78.31%) fun_25da0
  0.491 ms (0.08%; cumulative 78.39%) <onetaskgraph_core::engine::Engine>::known
  0.490 ms (0.08%; cumulative 78.47%) <[_]>::split_at_unchecked::precondition_check
  0.489 ms (0.08%; cumulative 78.56%) <fluent_uri::parse::Reader>::read
  0.489 ms (0.08%; cumulative 78.64%) <core::option::Option<onepipeline::hooks::FiredOver>>::is_none
  0.489 ms (0.08%; cumulative 78.72%) <serde_json::value::ser::Serializer as serde_core::ser::Serializer>::serialize_map
  0.488 ms (0.08%; cumulative 78.80%) <alloc::vec::Vec<onemessagebus::envelope::ArtifactRef> as core::default::Default>::default
  0.488 ms (0.08%; cumulative 78.88%) <alloc::collections::btree::navigate::LazyLeafRange<alloc::collections::btree::node::marker::Dying, alloc::string::String, onepipeline::graph::NodeStatus>>::init_front
  0.487 ms (0.08%; cumulative 78.97%) core::core_arch::x86::sse2::_mm_movemask_epi8
  0.486 ms (0.08%; cumulative 79.05%) <[usize]>::sort_by_key::<core::cmp::Reverse<core::option::Option<u64>>, onepipeline::telemetry::drain::{closure#1}>::{closure#0}
  0.485 ms (0.08%; cumulative 79.13%) core::ptr::drop_glue::<onepipeline::projection::MemberLabel>
  0.485 ms (0.08%; cumulative 79.21%) <<onepipeline::telemetry::Usage as serde_core::de::Deserialize>::deserialize::__FieldVisitor as serde_core::de::Visitor>::visit_str::<serde_json::error::Error>
  0.484 ms (0.08%; cumulative 79.29%) core::ptr::drop_glue::<alloc::vec::Vec<clap_builder::util::any_value::AnyValue>>
  0.484 ms (0.08%; cumulative 79.37%) jsonschema::keywords::type_::compile::<jsonschema_value::serde_json::SerdeJson>
  0.482 ms (0.08%; cumulative 79.45%) <onepipeline::cli::Command as clap_builder::derive::Subcommand>::augment_subcommands
  0.482 ms (0.08%; cumulative 79.53%) <<onepipeline::vocabulary::Labels as serde_core::de::Deserialize>::deserialize::__Visitor as serde_core::de::Visitor>::visit_map::<&mut serde_json::value::de::MapDeserializer>
  0.481 ms (0.08%; cumulative 79.62%) <std::path::PathBuf as core::ops::deref::Deref>::deref
  0.480 ms (0.08%; cumulative 79.70%) <core::fmt::Formatter>::write_fmt
  0.480 ms (0.08%; cumulative 79.78%) <alloc::collections::btree::map::entry::VacantEntry<schemars::generate::SchemaUid, alloc::collections::btree::set_val::SetValZST>>::insert_entry
  0.480 ms (0.08%; cumulative 79.86%) <onepipeline::event::PipelineKind as core::cmp::PartialEq>::eq
  0.479 ms (0.08%; cumulative 79.94%) <alloc::collections::btree::map::Iter<(alloc::string::String, alloc::string::String), alloc::collections::btree::set_val::SetValZST> as core::iter::traits::iterator::Iterator>::next
  0.479 ms (0.08%; cumulative 80.02%) <alloc::collections::btree::map::BTreeMap<alloc::string::String, onepipeline::agentgraph::TurnAddress> as core::clone::Clone>::clone
CPU attribution only: child lifetimes, filesystem latency, waits and process creation are not disjointly measured here.

pid 583.1: sampled CPU 658.103 ms; lifetime 1993.949 ms; threads 332
nearest first-party module (otherwise leaf):
  131.713 ms (20.01%; cumulative 20.01%) onevcs::git
  50.768 ms (7.71%; cumulative 27.73%) onepipeline::ledger
  49.408 ms (7.51%; cumulative 35.24%) poll
  42.282 ms (6.42%; cumulative 41.66%) onevcs::launch_trace
  41.879 ms (6.36%; cumulative 48.02%) libc.so.6!unresolved
  33.250 ms (5.05%; cumulative 53.08%) onemessagebus::envelope
  25.661 ms (3.90%; cumulative 56.98%) onepipeline::projection
  20.477 ms (3.11%; cumulative 60.09%) onepipeline::writeback
  17.339 ms (2.63%; cumulative 62.72%) onemessagebus::schema
  14.529 ms (2.21%; cumulative 64.93%) onepipeline::checkpoint
  12.982 ms (1.97%; cumulative 66.90%) onepipeline::plan
  12.025 ms (1.83%; cumulative 68.73%) onevcs::lock
  12.012 ms (1.83%; cumulative 70.56%) onepipeline::telemetry
  9.354 ms (1.42%; cumulative 71.98%) onepipeline::summary
  6.499 ms (0.99%; cumulative 72.96%) onepipeline::vocabulary
  6.051 ms (0.92%; cumulative 73.88%) onetaskgraph_local_md::staged
  5.802 ms (0.88%; cumulative 74.77%) _close
  5.218 ms (0.79%; cumulative 75.56%) onepipeline::engine
  5.143 ms (0.78%; cumulative 76.34%) fun_f230
  5.018 ms (0.76%; cumulative 77.10%) onevcs::home
  5.012 ms (0.76%; cumulative 77.86%) onepipeline::driver
  4.493 ms (0.68%; cumulative 78.55%) onevcs::workspace
  4.402 ms (0.67%; cumulative 79.22%) onetaskgraph_local_md::yaml
  4.279 ms (0.65%; cumulative 79.87%) onepipeline::graph
  4.225 ms (0.64%; cumulative 80.51%) onetaskgraph_local_md::LocalMdSource>
nearest first-party function (otherwise leaf):
  50.094 ms (7.61%; cumulative 7.61%) onevcs::git::bounded::<onevcs::git::run_fed::{closure#4}::{closure#3}>
  49.408 ms (7.51%; cumulative 15.12%) poll
  41.879 ms (6.36%; cumulative 21.48%) libc.so.6!unresolved
  22.065 ms (3.35%; cumulative 24.84%) onevcs::git::wait_for_exit
  19.943 ms (3.03%; cumulative 27.87%) <onevcs::git::PipeCapture>::start::<std::process::ChildStdout>
  16.862 ms (2.56%; cumulative 30.43%) <onemessagebus::schema::Registry>::register_schema
  16.695 ms (2.54%; cumulative 32.97%) <onevcs::git::PipeCapture>::start::<std::process::ChildStderr>
  14.228 ms (2.16%; cumulative 35.13%) onevcs::git::run_fed
  11.833 ms (1.80%; cumulative 36.93%) onepipeline::ledger::disk::publish
  10.853 ms (1.65%; cumulative 38.57%) onevcs::launch_trace::sleep
  10.678 ms (1.62%; cumulative 40.20%) onepipeline::ledger::publish
  9.385 ms (1.43%; cumulative 41.62%) <onevcs::launch_trace::Tracer>::write
  8.472 ms (1.29%; cumulative 42.91%) onevcs::lock::wait_for
  8.380 ms (1.27%; cumulative 44.18%) <onemessagebus::envelope::Envelope<onepipeline::vocabulary::Agent> as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::SliceRead>>
  7.502 ms (1.14%; cumulative 45.32%) <onepipeline::projection::RunState as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>>
  6.051 ms (0.92%; cumulative 46.24%) onetaskgraph_local_md::staged
  5.856 ms (0.89%; cumulative 47.13%) onevcs::launch_trace::wait
  5.802 ms (0.88%; cumulative 48.01%) _close
  5.655 ms (0.86%; cumulative 48.87%) onepipeline::ledger::disk::sync_contents
  5.555 ms (0.84%; cumulative 49.72%) <onemessagebus::envelope::Envelope<onepipeline::vocabulary::Agent> as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::StrRead>>
  5.528 ms (0.84%; cumulative 50.56%) <onevcs::launch_trace::Launch>::spawned
  5.375 ms (0.82%; cumulative 51.37%) <onepipeline::plan::Node as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::StrRead>>
  5.143 ms (0.78%; cumulative 52.16%) fun_f230
  5.137 ms (0.78%; cumulative 52.94%) <onevcs::git::PipeCapture>::finish
  5.006 ms (0.76%; cumulative 53.70%) <onemessagebus::envelope::Envelope<onepipeline::vocabulary::Agent> as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>>
  4.583 ms (0.70%; cumulative 54.39%) <onepipeline::checkpoint::Projected>::take_with
  4.277 ms (0.65%; cumulative 55.04%) <onepipeline::telemetry::Aggregate as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::StrRead>>
  4.246 ms (0.65%; cumulative 55.69%) <onepipeline::plan::Node as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>>
  4.230 ms (0.64%; cumulative 56.33%) onepipeline::ledger::disk::read
  3.998 ms (0.61%; cumulative 56.94%) <onepipeline::summary::Maintainer>::derived
  3.840 ms (0.58%; cumulative 57.52%) onepipeline::driver::attach
  3.788 ms (0.58%; cumulative 58.10%) onepipeline::projection::millis_of
  3.626 ms (0.55%; cumulative 58.65%) syscall
  3.600 ms (0.55%; cumulative 59.20%) _munmap
  3.347 ms (0.51%; cumulative 59.70%) onetaskgraph_local_md::yaml::from_str::<onetaskgraph_local_md::FrontMatter>
  3.260 ms (0.50%; cumulative 60.20%) <onevcs::launch_trace::Wait>::into_launch
  3.096 ms (0.47%; cumulative 60.67%) onevcs::policy::parse
  3.047 ms (0.46%; cumulative 61.13%) onevcs::launch_trace::launch
  2.965 ms (0.45%; cumulative 61.58%) onevcs::native_refs::local_value
  2.887 ms (0.44%; cumulative 62.02%) onevcs::home::ensure_dir
  2.723 ms (0.41%; cumulative 62.44%) <onemessagebus::transport::LocalTransport as onemessagebus::transport::Transport>::fingerprint
  2.709 ms (0.41%; cumulative 62.85%) <onepipeline::vocabulary::Labels as serde_core::de::Deserialize>::deserialize::<serde_json::value::Value>
  2.607 ms (0.40%; cumulative 63.24%) onepipeline::ledger::disk::synced
  2.573 ms (0.39%; cumulative 63.64%) <*const ()>::is_aligned_to
  2.512 ms (0.38%; cumulative 64.02%) onepipeline::ledger::read_range
  2.411 ms (0.37%; cumulative 64.38%) <onepipeline::ledger::LaunchRecord as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::StrRead>>
  2.389 ms (0.36%; cumulative 64.75%) onepipeline::ledger::write_atomic
  2.324 ms (0.35%; cumulative 65.10%) <onetaskgraph_local_md::LocalMdSource>::directory
  2.323 ms (0.35%; cumulative 65.45%) <onepipeline::channel::layout::CommandResult as schemars::JsonSchema>::json_schema
  2.275 ms (0.35%; cumulative 65.80%) onevcs::launch_trace::output
  2.268 ms (0.34%; cumulative 66.14%) onepipeline::release::ask_until_dropped
  2.258 ms (0.34%; cumulative 66.49%) onemessagebus::envelope::take_or_default::<serde_json::map::Map<alloc::string::String, serde_json::value::Value>, serde_json::error::Error>
  2.251 ms (0.34%; cumulative 66.83%) <onepipeline::vocabulary::Labels as core::clone::Clone>::clone
  2.224 ms (0.34%; cumulative 67.17%) <onepipeline::projection::RunState as core::clone::Clone>::clone
  2.177 ms (0.33%; cumulative 67.50%) onetaskgraph_local_md::compact
  2.164 ms (0.33%; cumulative 67.83%) onepipeline::projection::fold_record
  2.163 ms (0.33%; cumulative 68.15%) onevcs::workspace::existing
  2.102 ms (0.32%; cumulative 68.47%) <onepipeline::writeback::Writeback>::is_handing_over::{closure#0}
  2.092 ms (0.32%; cumulative 68.79%) <onemessagebus::envelope::Envelope<onepipeline::vocabulary::Agent> as core::clone::Clone>::clone
  2.078 ms (0.32%; cumulative 69.11%) <onevcs::launch_trace::Launch>::exited
  1.954 ms (0.30%; cumulative 69.40%) <onepipeline::telemetry::Aggregate>::fold
  1.902 ms (0.29%; cumulative 69.69%) onetaskgraph_core::resolve::validate_sources
  1.895 ms (0.29%; cumulative 69.98%) onevcs::checkout_reads::managed
  1.757 ms (0.27%; cumulative 70.25%) read
  1.748 ms (0.27%; cumulative 70.51%) onepipeline::writeback::closer
  1.683 ms (0.26%; cumulative 70.77%) <onepipeline::graph::Graph>::iter
  1.671 ms (0.25%; cumulative 71.02%) <onepipeline::summary::RunSummary as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>, serde_json::ser::PrettyFormatter>>
  1.632 ms (0.25%; cumulative 71.27%) <onepipeline::plan::TaskRecord as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::StrRead>>
  1.619 ms (0.25%; cumulative 71.52%) onepipeline::writeback::worker
  1.560 ms (0.24%; cumulative 71.75%) fun_9c7e0
  1.559 ms (0.24%; cumulative 71.99%) <onemessagebus::envelope::Envelope<onevcs::vocabulary::VcsEvents> as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::StrRead>>
  1.556 ms (0.24%; cumulative 72.23%) <onepipeline::plan::Node as core::clone::Clone>::clone
  1.492 ms (0.23%; cumulative 72.45%) onepipeline::checkpoint::fold_in_merge_order
  1.479 ms (0.22%; cumulative 72.68%) <onepipeline::checkpoint::Projected>::write
  1.472 ms (0.22%; cumulative 72.90%) onepipeline::writeback::shadow_front_metadata
  1.456 ms (0.22%; cumulative 73.12%) onepipeline::journal::stream_queues::<onepipeline::journal::merged_order::{closure#0}>
  1.438 ms (0.22%; cumulative 73.34%) onepipeline::writeback::opened_within
  1.365 ms (0.21%; cumulative 73.55%) onepipeline::executor::make_node_scratch_dir
  1.348 ms (0.20%; cumulative 73.76%) <onemessagebus::envelope::Envelope<onepipeline::vocabulary::Agent> as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>, serde_json::ser::PrettyFormatter>>
  1.343 ms (0.20%; cumulative 73.96%) onepipeline::ledger::records_of
  1.341 ms (0.20%; cumulative 74.16%) onepipeline::ledger::read_json_opt::<serde_json::value::Value>::{closure#1}
  1.302 ms (0.20%; cumulative 74.36%) _madvise
  1.276 ms (0.19%; cumulative 74.55%) <onemessagebus::redact::Redactor>::from_env
  1.244 ms (0.19%; cumulative 74.74%) onevcs::lock::open_at
  1.238 ms (0.19%; cumulative 74.93%) onevcs::lock::record_owner
  1.206 ms (0.18%; cumulative 75.11%) onepipeline::writeback::opened_within::{closure#0}
  1.182 ms (0.18%; cumulative 75.29%) _libc_calloc
  1.180 ms (0.18%; cumulative 75.47%) onevcs::home::root
  1.171 ms (0.18%; cumulative 75.65%) <std::sync::mpmc::list::Channel<()>>::send
  1.162 ms (0.18%; cumulative 75.83%) onepipeline::writeback::opened_by::<core::result::Result<onetaskgraph_core::config::Config, onetaskgraph_core::config::error::ConfigError>>
  1.148 ms (0.17%; cumulative 76.00%) onepipeline::checkpoint::digested
  1.115 ms (0.17%; cumulative 76.17%) onevcs::workspace::all
  1.085 ms (0.16%; cumulative 76.34%) <onepipeline::telemetry::Aggregate as core::clone::Clone>::clone
  1.081 ms (0.16%; cumulative 76.50%) <onemessagebus::envelope::Envelope<onepipeline::vocabulary::Agent> as serde_core::de::Deserialize>::deserialize::<serde_json::value::Value>
  1.076 ms (0.16%; cumulative 76.66%) onepipeline::checkpoint::length_of
  1.038 ms (0.16%; cumulative 76.82%) onevcs::policy::load
  1.022 ms (0.16%; cumulative 76.98%) onepipeline::summary::journal_stamp
  1.016 ms (0.15%; cumulative 77.13%) onepipeline::engine::wait_for_work
  0.995 ms (0.15%; cumulative 77.28%) onepipeline::executor::load_average
  0.992 ms (0.15%; cumulative 77.43%) <onetaskgraph_local_md::LocalMdSource>::confined
  0.989 ms (0.15%; cumulative 77.58%) regex_automata::util::determinize::epsilon_closure
  0.984 ms (0.15%; cumulative 77.73%) onepipeline::agentgraph::relayed_lines::<std::io::buffered::bufreader::BufReader<std::process::ChildStdout>, <onepipeline::agentgraph::ProcessGraphRun>::events::{closure#0}>::{closure#0}
  0.966 ms (0.15%; cumulative 77.88%) <onepipeline::continuation::ClaimWatch>::until_fence_retry
  0.964 ms (0.15%; cumulative 78.03%) <onepipeline::checkpoint::Coverage>::absorb
  0.954 ms (0.14%; cumulative 78.17%) <onepipeline::writeback::Lineages>::walk
  0.949 ms (0.14%; cumulative 78.32%) onepipeline::projection::derived_already
  0.933 ms (0.14%; cumulative 78.46%) onevcs::git::reads::remember
  0.931 ms (0.14%; cumulative 78.60%) onepipeline::vcs::token_of
  0.930 ms (0.14%; cumulative 78.74%) <onepipeline::projection::Doing as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::StrRead>>
  0.924 ms (0.14%; cumulative 78.88%) onevcs::recovery_cache::has_git_overrides
  0.921 ms (0.14%; cumulative 79.02%) fun_ab630
  0.921 ms (0.14%; cumulative 79.16%) <onepipeline::appendstamp::Stamp as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::StrRead>>
  0.909 ms (0.14%; cumulative 79.30%) <onetaskgraph_local_md::LocalMdSource>::read_text
  0.899 ms (0.14%; cumulative 79.44%) <onepipeline::human_items::HumanItemsCommand as clap_builder::derive::Subcommand>::augment_subcommands
  0.891 ms (0.14%; cumulative 79.57%) onepipeline::engine::journalling_relay::{closure#0}
  0.885 ms (0.13%; cumulative 79.71%) fun_188cf0
  0.885 ms (0.13%; cumulative 79.84%) onepipeline::ledger::open_healed
  0.885 ms (0.13%; cumulative 79.97%) onepipeline::crossdag::resolve_with
  0.884 ms (0.13%; cumulative 80.11%) <onepipeline::projection::RunState as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>, serde_json::ser::PrettyFormatter>>
self function:
  114.489 ms (17.40%; cumulative 17.40%) libc.so.6!unresolved
  50.101 ms (7.61%; cumulative 25.01%) poll
  28.195 ms (4.28%; cumulative 29.29%) _open64
  23.530 ms (3.58%; cumulative 32.87%) syscall
  13.078 ms (1.99%; cumulative 34.86%) rename
  11.727 ms (1.78%; cumulative 36.64%) _close
  11.387 ms (1.73%; cumulative 38.37%) fun_ab630
  10.576 ms (1.61%; cumulative 39.98%) fsync
  10.001 ms (1.52%; cumulative 41.50%) clock_nanosleep
  9.476 ms (1.44%; cumulative 42.94%) statx
  9.412 ms (1.43%; cumulative 44.37%) _write
  8.464 ms (1.29%; cumulative 45.65%) mkdir
  7.995 ms (1.21%; cumulative 46.87%) read
  7.202 ms (1.09%; cumulative 47.96%) wait4
  6.995 ms (1.06%; cumulative 49.02%) _munmap
  6.633 ms (1.01%; cumulative 50.03%) malloc
  6.440 ms (0.98%; cumulative 51.01%) fun_188d80
  5.143 ms (0.78%; cumulative 51.79%) fun_f230
  5.008 ms (0.76%; cumulative 52.55%) core::slice::raw::from_raw_parts::precondition_check
  3.950 ms (0.60%; cumulative 53.15%) pipe2
  3.652 ms (0.55%; cumulative 53.71%) fun_188cf0
  3.362 ms (0.51%; cumulative 54.22%) <*const ()>::is_aligned_to
  3.351 ms (0.51%; cumulative 54.73%) core::ptr::drop_glue::<jsonschema::compiler::LocationCacheKey>
  3.307 ms (0.50%; cumulative 55.23%) _libc_free
  3.247 ms (0.49%; cumulative 55.72%) fun_98d50
  2.982 ms (0.45%; cumulative 56.18%) <serde_json::read::SliceRead as serde_json::read::Read>::peek
  2.812 ms (0.43%; cumulative 56.60%) <alloc::raw_vec::RawVecInner<_>>::reserve::do_reserve_and_handle::<alloc::alloc::Global>
  2.517 ms (0.38%; cumulative 56.99%) <serde_core::de::impls::OptionVisitor<onepipeline::plan::TaskRecord> as serde_core::de::Visitor>::visit_some::<&mut serde_json::de::Deserializer<serde_json::read::StrRead>>
  2.504 ms (0.38%; cumulative 57.37%) fun_ab230
  2.277 ms (0.35%; cumulative 57.71%) <core::hash::sip::Hasher<core::hash::sip::Sip13Rounds> as core::hash::Hasher>::write
  2.250 ms (0.34%; cumulative 58.06%) readlink
  2.224 ms (0.34%; cumulative 58.39%) <serde_json::de::MapAccess<serde_json::read::SliceRead> as serde_core::de::MapAccess>::next_entry::<alloc::string::String, serde_json::value::Value>
  2.182 ms (0.33%; cumulative 58.72%) serde_json::ser::format_escaped_str_contents::<&mut alloc::vec::Vec<u8>, serde_json::ser::CompactFormatter>
  2.144 ms (0.33%; cumulative 59.05%) _mmap
  2.128 ms (0.32%; cumulative 59.37%) <serde_json::map::Entry>::or_insert
  2.109 ms (0.32%; cumulative 59.69%) fun_a9c70
  2.102 ms (0.32%; cumulative 60.01%) <onepipeline::writeback::Writeback>::is_handing_over::{closure#0}
  2.040 ms (0.31%; cumulative 60.32%) <alloc::collections::btree::map::BTreeMap<alloc::string::String, alloc::string::String>>::entry
  2.015 ms (0.31%; cumulative 60.63%) unlinkat
  1.954 ms (0.30%; cumulative 60.93%) <alloc::collections::btree::map::BTreeMap<alloc::string::String, onepipeline::telemetry::Phase>>::remove_entry::<alloc::string::String>
  1.954 ms (0.30%; cumulative 61.22%) <core::iter::adapters::zip::Zip<core::iter::adapters::zip::Zip<core::slice::iter::Iter<u8>, core::slice::iter::Iter<u8>>, core::slice::iter::IterMut<i64>> as core::iter::adapters::zip::ZipImpl<core::iter::adapters::zip::Zip<core::slice::iter::Iter<u8>, core::slice::iter::Iter<u8>>, core::slice::iter::IterMut<i64>>>::next
  1.887 ms (0.29%; cumulative 61.51%) <hashbrown::raw::RawTable<usize>>::reserve_rehash::<indexmap::inner::get_hash<alloc::string::String, serde_json::value::Value>::{closure#0}>
  1.820 ms (0.28%; cumulative 61.79%) <core::hash::sip::Sip13Rounds as core::hash::sip::Sip>::c_rounds
  1.796 ms (0.27%; cumulative 62.06%) <alloc::raw_vec::RawVecInner>::finish_grow
  1.729 ms (0.26%; cumulative 62.32%) <alloc::vec::Vec<u8>>::extend_from_slice
  1.634 ms (0.25%; cumulative 62.57%) core::ptr::copy_nonoverlapping::precondition_check
  1.604 ms (0.24%; cumulative 62.81%) __rustc::__rust_no_alloc_shim_is_unstable_v2
  1.560 ms (0.24%; cumulative 63.05%) fun_9c7e0
  1.546 ms (0.23%; cumulative 63.29%) <alloc::raw_vec::RawVecInner>::deallocate
  1.522 ms (0.23%; cumulative 63.52%) pthread_create
  1.459 ms (0.22%; cumulative 63.74%) _open64_nocancel
  1.424 ms (0.22%; cumulative 63.96%) <serde_json::value::Value as core::fmt::Display>::fmt
  1.412 ms (0.21%; cumulative 64.17%) <std::path::Components as core::iter::traits::iterator::Iterator>::next
  1.409 ms (0.21%; cumulative 64.38%) <serde_json::de::Deserializer<serde_json::read::StrRead>>::parse_whitespace
  1.386 ms (0.21%; cumulative 64.60%) fun_11ca00
  1.368 ms (0.21%; cumulative 64.80%) <std::sync::mpmc::Receiver<core::result::Result<onepipeline::taskgraph::Built, onetaskgraph_core::config::error::ConfigError>> as core::ops::drop::Drop>::drop::{closure#1}
  1.330 ms (0.20%; cumulative 65.01%) <std::time::SystemTime>::now
  1.324 ms (0.20%; cumulative 65.21%) <serde_json::de::MapAccess<serde_json::read::StrRead> as serde_core::de::MapAccess>::next_value_seed::<core::marker::PhantomData<alloc::collections::btree::map::BTreeMap<alloc::string::String, onepipeline::telemetry::Phase>>>
  1.302 ms (0.20%; cumulative 65.40%) _madvise
  1.301 ms (0.20%; cumulative 65.60%) <alloc::vec::Vec<indexmap::Bucket<alloc::string::String, serde_json::value::Value>>>::extend_trusted::<core::iter::adapters::cloned::Cloned<core::slice::iter::Iter<indexmap::Bucket<alloc::string::String, serde_json::value::Value>>>>
  1.299 ms (0.20%; cumulative 65.80%) onevcs::launch_trace::wait
  1.283 ms (0.19%; cumulative 65.99%) <alloc::raw_vec::RawVecInner>::with_capacity_in
  1.275 ms (0.19%; cumulative 66.19%) <[alloc::string::String]>::iter
  1.246 ms (0.19%; cumulative 66.38%) <alloc::string::String as core::clone::Clone>::clone
  1.212 ms (0.18%; cumulative 66.56%) <alloc::collections::btree::map::IntoIter<alloc::string::String, onepipeline::projection::PendingDecision>>::dying_next
  1.211 ms (0.18%; cumulative 66.75%) jsonschema::compiler::compile_with_internal::<jsonschema_value::serde_json::SerdeJson>
  1.206 ms (0.18%; cumulative 66.93%) <alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Dying, alloc::string::String, alloc::string::String, alloc::collections::btree::node::marker::LeafOrInternal>>::deallocate_and_ascend::<alloc::alloc::Global>
  1.187 ms (0.18%; cumulative 67.11%) <alloc::vec::Vec<u8> as alloc::vec::spec_extend::SpecExtend<&u8, core::slice::iter::Iter<u8>>>::spec_extend
  1.182 ms (0.18%; cumulative 67.29%) _libc_calloc
  1.171 ms (0.18%; cumulative 67.47%) <std::sync::mpmc::list::Channel<()>>::send
  1.170 ms (0.18%; cumulative 67.64%) core::ub_checks::maybe_is_nonoverlapping::runtime
  1.148 ms (0.17%; cumulative 67.82%) <core::slice::iter::Iter<u8> as core::iter::traits::iterator::Iterator>::fold::<u128, onepipeline::checkpoint::digested::{closure#0}>
  1.103 ms (0.17%; cumulative 67.99%) core::ptr::drop_glue::<alloc::vec::Vec<indexmap::Bucket<alloc::string::String, serde_json::value::Value>>>
  1.039 ms (0.16%; cumulative 68.14%) _mprotect
  1.019 ms (0.15%; cumulative 68.30%) fun_a9b90
  0.989 ms (0.15%; cumulative 68.45%) regex_automata::util::determinize::epsilon_closure
  0.982 ms (0.15%; cumulative 68.60%) <alloc::collections::btree::node::Handle<alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Mut, onepipeline::telemetry::BucketName, u64, alloc::collections::btree::node::marker::Leaf>, alloc::collections::btree::node::marker::Edge>>::insert_recursing::<alloc::alloc::Global, <alloc::collections::btree::map::entry::VacantEntry<onepipeline::telemetry::BucketName, u64>>::insert_entry::{closure#0}>
  0.981 ms (0.15%; cumulative 68.75%) fun_98690
  0.963 ms (0.15%; cumulative 68.89%) ftruncate
  0.962 ms (0.15%; cumulative 69.04%) _close_nocancel
  0.949 ms (0.14%; cumulative 69.18%) <core::slice::iter::Iter<onepipeline::projection::Derivation>>::new
  0.943 ms (0.14%; cumulative 69.33%) fun_ab050
  0.938 ms (0.14%; cumulative 69.47%) <onepipeline::plan::Node as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>>
  0.933 ms (0.14%; cumulative 69.61%) <serde_json::de::Deserializer<serde_json::read::SliceRead>>::parse_whitespace
  0.931 ms (0.14%; cumulative 69.75%) <[char; 2] as core::str::pattern::Pattern>::is_contained_in
  0.925 ms (0.14%; cumulative 69.89%) <serde_json::ser::Compound<&mut alloc::vec::Vec<u8>, serde_json::ser::CompactFormatter> as serde_core::ser::SerializeMap>::serialize_key::<str>
  0.924 ms (0.14%; cumulative 70.03%) <alloc::vec::Vec<u8> as core::cmp::Ord>::cmp
  0.924 ms (0.14%; cumulative 70.17%) fstatat
  0.906 ms (0.14%; cumulative 70.31%) <serde::private::de::content::ContentRefDeserializer<serde_json::error::Error> as serde_core::de::Deserializer>::deserialize_string::<serde_core::de::impls::StringVisitor>
  0.899 ms (0.14%; cumulative 70.45%) <clap_builder::util::id::Id as clap_builder::builder::resettable::IntoResettable<clap_builder::util::id::Id>>::into_resettable
  0.897 ms (0.14%; cumulative 70.59%) <serde_json::value::Value as core::convert::From<&str>>::from
  0.891 ms (0.14%; cumulative 70.72%) <alloc::string::String as core::fmt::Write>::write_str
  0.885 ms (0.13%; cumulative 70.86%) core::core_arch::x86::sse2::_mm_loadu_si128
  0.875 ms (0.13%; cumulative 70.99%) <alloc::vec::Vec<regex_automata::nfa::thompson::map::Utf8BoundedEntry>>::extend_with
  0.849 ms (0.13%; cumulative 71.12%) alloc::fmt::format::format_inner
  0.828 ms (0.13%; cumulative 71.24%) sysconf
  0.822 ms (0.12%; cumulative 71.37%) unresolved
  0.814 ms (0.12%; cumulative 71.49%) <alloc::vec::Vec<serde_json::value::Value> as core::clone::Clone>::clone
  0.803 ms (0.12%; cumulative 71.61%) core::ptr::drop_glue::<serde_json::value::Value>
  0.802 ms (0.12%; cumulative 71.74%) <std::sync::mpmc::list::Channel<std::fs::File>>::recv::{closure#1}
  0.801 ms (0.12%; cumulative 71.86%) _getauxval
  0.793 ms (0.12%; cumulative 71.98%) referencing::spec::draft202012::walk_children::<referencing::registry::build::crawl_schema::{closure#0}>
  0.786 ms (0.12%; cumulative 72.10%) onevcs::git::bounded::<onevcs::git::run_fed::{closure#4}::{closure#3}>
  0.784 ms (0.12%; cumulative 72.22%) fun_9e730
  0.773 ms (0.12%; cumulative 72.33%) <core::ops::range::RangeInclusive<i64> as core::ops::range::RangeBounds<i64>>::contains::<i64>
  0.768 ms (0.12%; cumulative 72.45%) <std::sync::mpmc::list::Channel<()>>::recv
  0.757 ms (0.12%; cumulative 72.57%) fun_ada50
  0.749 ms (0.11%; cumulative 72.68%) __rustc::__rdl_dealloc
  0.746 ms (0.11%; cumulative 72.79%) core::ptr::drop_glue::<alloc::vec::Vec<u8>>
  0.744 ms (0.11%; cumulative 72.91%) getrlimit
  0.742 ms (0.11%; cumulative 73.02%) __rustc::__rust_dealloc
  0.739 ms (0.11%; cumulative 73.13%) access
  0.737 ms (0.11%; cumulative 73.24%) <serde_json::de::Deserializer<serde_json::read::StrRead>>::parse_object_colon
  0.726 ms (0.11%; cumulative 73.35%) <core::result::Result<serde_json::map::Map<alloc::string::String, serde_json::value::Value>, serde_json::error::Error> as core::ops::try_trait::Try>::branch
  0.726 ms (0.11%; cumulative 73.46%) fun_189710
  0.707 ms (0.11%; cumulative 73.57%) core::ptr::drop_glue::<std::io::error::Error>
  0.698 ms (0.11%; cumulative 73.68%) core::ptr::drop_glue::<indexmap::Bucket<alloc::string::String, serde_json::value::Value>>
  0.687 ms (0.10%; cumulative 73.78%) <core::str::pattern::CharSearcher as core::str::pattern::Searcher>::next_match
  0.687 ms (0.10%; cumulative 73.89%) sigaltstack
  0.674 ms (0.10%; cumulative 73.99%) <serde_json::de::Deserializer<serde_json::read::StrRead>>::parse_integer
  0.672 ms (0.10%; cumulative 74.09%) <core::str::lossy::Utf8Chunks as core::iter::traits::iterator::Iterator>::next
  0.655 ms (0.10%; cumulative 74.19%) <indexmap::inner::Core<alloc::string::String, serde_json::value::Value>>::insert_full
  0.645 ms (0.10%; cumulative 74.29%) <std::hash::random::DefaultHasher as core::hash::Hasher>::write
  0.626 ms (0.10%; cumulative 74.38%) <onevcs::launch_trace::Wait>::into_launch
  0.626 ms (0.10%; cumulative 74.48%) <onepipeline::telemetry::Aggregate as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::StrRead>>
  0.625 ms (0.09%; cumulative 74.57%) <alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Owned, onepipeline::telemetry::BucketName, u64, alloc::collections::btree::node::marker::Leaf>>::new_leaf::<alloc::alloc::Global>
  0.624 ms (0.09%; cumulative 74.67%) core::sync::atomic::atomic_load::<*mut std::sync::mpmc::list::Block<onepipeline::engine::Message>>
  0.610 ms (0.09%; cumulative 74.76%) fun_991e0
  0.610 ms (0.09%; cumulative 74.85%) <onemessagebus::redact::Redactor>::from_env
  0.608 ms (0.09%; cumulative 74.95%) <std::sys::pal::unix::time::Timespec>::now
  0.605 ms (0.09%; cumulative 75.04%) <alloc::collections::btree::node::Handle<alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Dying, alloc::string::String, u64, alloc::collections::btree::node::marker::Leaf>, alloc::collections::btree::node::marker::Edge>>::deallocating_end::<alloc::alloc::Global>
  0.597 ms (0.09%; cumulative 75.13%) <<serde_json::map::Map<alloc::string::String, serde_json::value::Value> as serde_core::de::Deserialize>::deserialize::Visitor as serde_core::de::Visitor>::visit_map::<serde_json::de::MapAccess<serde_json::read::StrRead>>
  0.594 ms (0.09%; cumulative 75.22%) <indexmap::inner::Core<alloc::string::String, serde_json::value::Value> as core::clone::Clone>::clone
  0.591 ms (0.09%; cumulative 75.31%) <core::option::Option<onepipeline::storeops::Context>>::is_none
  0.590 ms (0.09%; cumulative 75.40%) <serde_json::ser::Compound<&mut alloc::vec::Vec<u8>, serde_json::ser::CompactFormatter> as serde_core::ser::SerializeMap>::serialize_entry::<str, alloc::string::String>
  0.584 ms (0.09%; cumulative 75.49%) std::sys::backtrace::__rust_begin_short_backtrace::<<onevcs::git::PipeCapture>::start<std::process::ChildStderr>::{closure#0}, onevcs::git::Captured>
  0.581 ms (0.09%; cumulative 75.57%) <serde_json::de::MapKey<serde_json::read::StrRead> as serde_core::de::Deserializer>::deserialize_str::<serde_json::value::de::KeyClassifier>
  0.578 ms (0.09%; cumulative 75.66%) <core::option::Option<onepipeline::rendercost::Inside>>::as_ref
  0.578 ms (0.09%; cumulative 75.75%) <core::fmt::Formatter>::pad_integral::write_prefix
  0.569 ms (0.09%; cumulative 75.84%) onevcs::ids::unique
  0.565 ms (0.09%; cumulative 75.92%) <serde_json::de::MapKey<serde_json::read::StrRead> as serde_core::de::Deserializer>::deserialize_any::<<onepipeline::agentgraph::WrittenAddress as serde_core::de::Deserialize>::deserialize::__FieldVisitor>
  0.561 ms (0.09%; cumulative 76.01%) fun_98610
  0.551 ms (0.08%; cumulative 76.09%) <serde_json::value::Value as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>>
  0.551 ms (0.08%; cumulative 76.18%) getdents64
  0.549 ms (0.08%; cumulative 76.26%) <u8 as core::slice::cmp::SlicePartialEq<u8>>::equal_same_length
  0.548 ms (0.08%; cumulative 76.34%) <alloc::string::String as serde_core::de::Deserialize>::deserialize::<serde_json::de::MapKey<serde_json::read::SliceRead>>
  0.545 ms (0.08%; cumulative 76.43%) core::sync::atomic::atomic_load::<usize>
  0.544 ms (0.08%; cumulative 76.51%) <&mut serde_json::de::Deserializer<serde_json::read::StrRead> as serde_core::de::Deserializer>::deserialize_map::<<alloc::collections::btree::map::BTreeMap<_, _> as serde_core::de::Deserialize>::deserialize::MapVisitor<alloc::string::String, onevcs::preserve::Preservation>>
  0.542 ms (0.08%; cumulative 76.59%) <core::option::Option<alloc::string::String>>::is_none
  0.541 ms (0.08%; cumulative 76.67%) <alloc::collections::btree::node::Handle<alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Dying, onepipeline::telemetry::BucketName, u64, alloc::collections::btree::node::marker::LeafOrInternal>, alloc::collections::btree::node::marker::KV>>::next_leaf_edge
  0.539 ms (0.08%; cumulative 76.75%) core::ptr::drop_glue::<core::option::Option<alloc::string::String>>
  0.539 ms (0.08%; cumulative 76.84%) <indexmap::Bucket<alloc::string::String, serde_json::value::Value> as core::clone::Clone>::clone
  0.537 ms (0.08%; cumulative 76.92%) <core::num::nonzero::NonZero<u64> as schemars::JsonSchema>::_schemars_private_is_option
  0.537 ms (0.08%; cumulative 77.00%) realloc
  0.531 ms (0.08%; cumulative 77.08%) <std::hash::random::RandomState as core::hash::BuildHasher>::hash_one::<&(core::option::Option<std::path::PathBuf>, alloc::vec::Vec<alloc::string::String>, alloc::vec::Vec<(alloc::string::String, alloc::string::String)>)>
  0.530 ms (0.08%; cumulative 77.16%) <serde_json::ser::Compound<&mut alloc::vec::Vec<u8>, serde_json::ser::CompactFormatter> as serde_core::ser::SerializeMap>::serialize_value::<onepipeline::vocabulary::Source>
  0.529 ms (0.08%; cumulative 77.24%) <hashbrown::map::HashMap<alloc::string::String, jsonschema::node::SchemaNode, ahash::random_state::RandomState>>::get::<str>
  0.529 ms (0.08%; cumulative 77.32%) <&mut serde_json::de::Deserializer<serde_json::read::StrRead> as serde_core::de::Deserializer>::deserialize_enum::<<onepipeline::projection::MemberLabel as serde_core::de::Deserialize>::deserialize::__Visitor>
  0.528 ms (0.08%; cumulative 77.40%) linkat
  0.524 ms (0.08%; cumulative 77.48%) <core::array::iter::IntoIter<core::option::Option<std::time::Instant>, 3> as core::ops::drop::Drop>::drop
  0.524 ms (0.08%; cumulative 77.56%) <alloc::collections::btree::navigate::LazyLeafRange<alloc::collections::btree::node::marker::Dying, alloc::string::String, alloc::collections::btree::set_val::SetValZST>>::deallocating_end::<alloc::alloc::Global>
  0.524 ms (0.08%; cumulative 77.64%) <core::option::Option<&u64>>::copied
  0.523 ms (0.08%; cumulative 77.72%) <serde_json::de::MapAccess<serde_json::read::StrRead> as serde_core::de::MapAccess>::next_key_seed::<core::marker::PhantomData<<onepipeline::telemetry::Aggregate as serde_core::de::Deserialize>::deserialize::__Field>>
  0.520 ms (0.08%; cumulative 77.80%) regex_syntax::ast::visitor::visit::<regex_syntax::ast::parse::NestLimiter<&mut regex_syntax::ast::parse::Parser>>
  0.520 ms (0.08%; cumulative 77.88%) <schemars::schema::Schema as core::convert::From<serde_json::map::Map<alloc::string::String, serde_json::value::Value>>>::from
  0.520 ms (0.08%; cumulative 77.96%) <hashbrown::raw::RawTable<usize>>::insert_tagged_at_index
  0.520 ms (0.08%; cumulative 78.04%) <std::thread::thread::Thread>::park_timeout
  0.516 ms (0.08%; cumulative 78.11%) <serde_json::read::SliceRead as serde_json::read::Read>::next
  0.515 ms (0.08%; cumulative 78.19%) <alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Mut, alloc::string::String, onepipeline::projection::Recorded, alloc::collections::btree::node::marker::Leaf>>::push_with_handle
  0.514 ms (0.08%; cumulative 78.27%) onepipeline::ledger::write_atomic
  0.514 ms (0.08%; cumulative 78.35%) <alloc::vec::Vec<u8>>::append_elements
  0.514 ms (0.08%; cumulative 78.43%) onepipeline::review::failpoint
  0.514 ms (0.08%; cumulative 78.51%) <alloc::collections::btree::map::Iter<alloc::string::String, onepipeline::projection::Recorded> as core::iter::traits::iterator::Iterator>::size_hint
  0.512 ms (0.08%; cumulative 78.58%) core::ptr::drop_glue::<core::option::Option<onepipeline::human_items::HumanItemMetadata>>
  0.512 ms (0.08%; cumulative 78.66%) fun_adf20
  0.511 ms (0.08%; cumulative 78.74%) core::ptr::drop_glue::<alloc::collections::btree::map::IntoIter<alloc::string::String, alloc::string::String>>
  0.508 ms (0.08%; cumulative 78.82%) core::ptr::copy::precondition_check
  0.508 ms (0.08%; cumulative 78.89%) <alloc::collections::btree::map::BTreeMap<alloc::string::String, onepipeline::graph::NodeStatus>>::get::<alloc::string::String>
  0.508 ms (0.08%; cumulative 78.97%) core::str::validations::next_code_point::<core::slice::iter::Iter<u8>>
  0.508 ms (0.08%; cumulative 79.05%) <f64 as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>>
  0.506 ms (0.08%; cumulative 79.12%) <onepipeline::projection::DriverClaim as core::convert::TryFrom<onepipeline::projection::ClaimAsWritten>>::try_from
  0.506 ms (0.08%; cumulative 79.20%) <str as core::cmp::PartialEq>::eq
  0.504 ms (0.08%; cumulative 79.28%) std::thread::spawnhook::run_spawn_hooks
  0.501 ms (0.08%; cumulative 79.35%) <alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Dying, alloc::string::String, onepipeline::plan::Node, alloc::collections::btree::node::marker::LeafOrInternal>>::deallocate_and_ascend::<alloc::alloc::Global>
  0.500 ms (0.08%; cumulative 79.43%) core::char::convert::from_u32_unchecked::precondition_check
  0.498 ms (0.08%; cumulative 79.51%) <serde::private::de::content::ContentRefDeserializer<serde_json::error::Error> as serde_core::de::Deserializer>::deserialize_str::<serde_core::de::impls::StringVisitor>
  0.498 ms (0.08%; cumulative 79.58%) alloc::collections::btree::mem::replace::<alloc::collections::btree::node::Handle<alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Dying, onetaskgraph_plugin_api::metadata::MetadataKey, serde_json::value::Value, alloc::collections::btree::node::marker::Leaf>, alloc::collections::btree::node::marker::Edge>, alloc::collections::btree::node::Handle<alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Dying, onetaskgraph_plugin_api::metadata::MetadataKey, serde_json::value::Value, alloc::collections::btree::node::marker::LeafOrInternal>, alloc::collections::btree::node::marker::KV>, <alloc::collections::btree::node::Handle<alloc::collections::btree::node::NodeRef<alloc::collections::btree::node::marker::Dying, onetaskgraph_plugin_api::metadata::MetadataKey, serde_json::value::Value, alloc::collections::btree::node::marker::Leaf>, alloc::collections::btree::node::marker::Edge>>::deallocating_next_unchecked<alloc::alloc::Global>::{closure#0}>
  0.498 ms (0.08%; cumulative 79.66%) <serde_json::ser::PrettyFormatter as serde_json::ser::Formatter>::begin_object::<&mut alloc::vec::Vec<u8>>
  0.497 ms (0.08%; cumulative 79.73%) <core::option::Option<&core::option::Option<onevcs::checkout_reads::Managed>>>::cloned
  0.496 ms (0.08%; cumulative 79.81%) <alloc::collections::btree::map::entry::VacantEntry<alloc::string::String, serde_json::value::Value>>::insert_entry
  0.495 ms (0.08%; cumulative 79.88%) onepipeline::executor::load_average
  0.494 ms (0.08%; cumulative 79.96%) core::ptr::drop_glue::<core::option::Option<serde_json::value::Value>>
  0.494 ms (0.08%; cumulative 80.03%) <<onepipeline::plan::Node as serde_core::de::Deserialize>::deserialize::__Visitor as serde_core::de::Visitor>::visit_map::<serde_json::de::MapAccess<serde_json::read::StrRead>>
CPU attribution only: child lifetimes, filesystem latency, waits and process creation are not disjointly measured here.
```

#### release before

```text
pid 90.1: sampled CPU 404.486 ms; lifetime 1839.286 ms; threads 274
nearest first-party module (otherwise leaf):
  90.912 ms (22.48%; cumulative 22.48%) onevcs::git
  57.249 ms (14.15%; cumulative 36.63%) privateterms::matcher
  36.020 ms (8.91%; cumulative 45.53%) onepipeline::ledger
  27.126 ms (6.71%; cumulative 52.24%) libc.so.6!unresolved
  26.735 ms (6.61%; cumulative 58.85%) onevcs::launch_trace
  17.759 ms (4.39%; cumulative 63.24%) poll
  12.633 ms (3.12%; cumulative 66.36%) onevcs::lock
  9.872 ms (2.44%; cumulative 68.80%) onepipeline::writeback
  9.707 ms (2.40%; cumulative 71.20%) onemessagebus::schema
  7.059 ms (1.75%; cumulative 72.95%) _close
  6.652 ms (1.64%; cumulative 74.59%) onevcs::home
  5.087 ms (1.26%; cumulative 75.85%) syscall
  5.006 ms (1.24%; cumulative 77.09%) onevcs::store
  4.490 ms (1.11%; cumulative 78.20%) _munmap
  4.032 ms (1.00%; cumulative 79.20%) onepipeline::engine
  3.605 ms (0.89%; cumulative 80.09%) onetaskgraph_core::resolve
nearest first-party function (otherwise leaf):
  57.249 ms (14.15%; cumulative 14.15%) <privateterms::matcher::Matcher>::new
  31.409 ms (7.77%; cumulative 21.92%) onevcs::git::run_fed::{closure#4}
  27.126 ms (6.71%; cumulative 28.62%) libc.so.6!unresolved
  23.799 ms (5.88%; cumulative 34.51%) onepipeline::ledger::write_atomic
  17.759 ms (4.39%; cumulative 38.90%) poll
  14.538 ms (3.59%; cumulative 42.49%) <onevcs::git::PipeCapture>::start::<std::process::ChildStdout>
  12.779 ms (3.16%; cumulative 45.65%) <onevcs::git::PipeCapture>::start::<std::process::ChildStderr>
  9.897 ms (2.45%; cumulative 48.10%) onevcs::git::run_fed
  9.736 ms (2.41%; cumulative 50.51%) <onevcs::launch_trace::Tracer>::write
  9.707 ms (2.40%; cumulative 52.91%) <onemessagebus::schema::Registry>::register_schema
  9.513 ms (2.35%; cumulative 55.26%) onevcs::lock::wait_for
  7.059 ms (1.75%; cumulative 57.00%) _close
  6.890 ms (1.70%; cumulative 58.71%) onevcs::git::wait_for_exit
  6.667 ms (1.65%; cumulative 60.36%) onevcs::git::await_an_ending
  5.460 ms (1.35%; cumulative 61.70%) onevcs::home::ensure_dir
  5.087 ms (1.26%; cumulative 62.96%) syscall
  4.502 ms (1.11%; cumulative 64.08%) onepipeline::ledger::read_published
  4.490 ms (1.11%; cumulative 65.19%) _munmap
  4.120 ms (1.02%; cumulative 66.20%) onevcs::store::load
  3.605 ms (0.89%; cumulative 67.10%) onetaskgraph_core::resolve::validate_sources
  3.295 ms (0.81%; cumulative 67.91%) onevcs::launch_trace::output
  3.261 ms (0.81%; cumulative 68.72%) onevcs::launch_trace::launch
  3.261 ms (0.81%; cumulative 69.52%) <onejudge::note::NoteWire as schemars::JsonSchema>::json_schema
  3.078 ms (0.76%; cumulative 70.28%) onetaskgraph_local_md::staged
  2.677 ms (0.66%; cumulative 70.95%) onevcs::boundary::scope::derive_with
  2.539 ms (0.63%; cumulative 71.57%) onevcs::policy::read
  2.530 ms (0.63%; cumulative 72.20%) onevcs::launch_trace::sleep
  2.520 ms (0.62%; cumulative 72.82%) onetaskgraph_core::config::discovery::documents
  2.346 ms (0.58%; cumulative 73.40%) <onepipeline::ledger::RunPaths>::create
  2.223 ms (0.55%; cumulative 73.95%) onetaskgraph_local_md::yaml::from_str::<onetaskgraph_local_md::FrontMatter>
  2.195 ms (0.54%; cumulative 74.49%) onevcs::git::local_setting
  2.187 ms (0.54%; cumulative 75.03%) onepipeline::writeback::project
  2.179 ms (0.54%; cumulative 75.57%) <onemessagebus::transport::LocalTransport as onemessagebus::transport::Transport>::fingerprint
  2.085 ms (0.52%; cumulative 76.09%) <onevcs::git::PipeCapture>::finish
  2.071 ms (0.51%; cumulative 76.60%) <onevcs::launch_trace::Launch>::spawned
  1.991 ms (0.49%; cumulative 77.09%) <onepipeline::executor::LocalExecutor as onepipeline::executor::Executor>::dispatch
  1.952 ms (0.48%; cumulative 77.58%) <onepipeline::projection::RunState as core::clone::Clone>::clone
  1.921 ms (0.47%; cumulative 78.05%) onevcs::lock::record_owner
  1.846 ms (0.46%; cumulative 78.51%) <onevcs::launch_trace::Launch>::exited
  1.821 ms (0.45%; cumulative 78.96%) <onetaskgraph_local_md::LocalMdSource as onetaskgraph_plugin_api::source::TaskSource>::update_task::{closure#0}::{closure#0}
  1.807 ms (0.45%; cumulative 79.40%) <onevcs::launch_trace::Wait>::into_launch
  1.776 ms (0.44%; cumulative 79.84%) onepipeline::writeback::worker
  1.755 ms (0.43%; cumulative 80.28%) onevcs::launch_trace::wait
self function:
  81.428 ms (20.13%; cumulative 20.13%) libc.so.6!unresolved
  35.837 ms (8.86%; cumulative 28.99%) <std::sync::once::Once>::call_once_force::<<std::sync::once_lock::OnceLock<std::collections::hash::map::HashMap<char, alloc::vec::Vec<usize>>>>::initialize<<std::sync::once_lock::OnceLock<std::collections::hash::map::HashMap<char, alloc::vec::Vec<usize>>>>::get_or_init<<privateterms::normalize::Contributors>::new<core::iter::adapters::map::Map<core::slice::iter::Iter<privateterms::resolve::ResolvedRule>, <privateterms::matcher::Matcher>::new::{closure#2}::{closure#0}>>::{closure#0}>::{closure#0}, !>::{closure#0}>::{closure#0}
  18.810 ms (4.65%; cumulative 33.64%) poll
  17.532 ms (4.33%; cumulative 37.98%) mkdir
  16.190 ms (4.00%; cumulative 41.98%) syscall
  14.187 ms (3.51%; cumulative 45.49%) _open64
  12.476 ms (3.08%; cumulative 48.57%) _close
  10.378 ms (2.57%; cumulative 51.14%) fun_188cf0
  10.005 ms (2.47%; cumulative 53.61%) _write
  8.709 ms (2.15%; cumulative 55.76%) fsync
  8.398 ms (2.08%; cumulative 57.84%) <icu_normalizer::Decomposition<core::iter::sources::once::Once<char>> as core::iter::traits::iterator::Iterator>::next
  7.623 ms (1.88%; cumulative 59.72%) _munmap
  7.493 ms (1.85%; cumulative 61.58%) fun_188d80
  7.232 ms (1.79%; cumulative 63.36%) rename
  7.060 ms (1.75%; cumulative 65.11%) wait4
  6.345 ms (1.57%; cumulative 66.68%) read
  6.162 ms (1.52%; cumulative 68.20%) fun_ab630
  6.126 ms (1.51%; cumulative 69.72%) statx
  4.416 ms (1.09%; cumulative 70.81%) core::unicode::unicode_data::conversions::lookup
  3.857 ms (0.95%; cumulative 71.76%) malloc
  3.369 ms (0.83%; cumulative 72.59%) <serde_json::value::ser::Serializer as serde_core::ser::Serializer>::serialize_map
  2.649 ms (0.65%; cumulative 73.25%) _libc_free
  1.933 ms (0.48%; cumulative 73.73%) fun_98d50
  1.907 ms (0.47%; cumulative 74.20%) _mmap
  1.865 ms (0.46%; cumulative 74.66%) ftruncate
  1.862 ms (0.46%; cumulative 75.12%) clock_nanosleep
  1.677 ms (0.41%; cumulative 75.53%) fun_f230
  1.500 ms (0.37%; cumulative 75.91%) <serde_json::value::Value as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut <serde_json::value::Value as core::fmt::Display>::fmt::WriterFormatter>>
  1.496 ms (0.37%; cumulative 76.28%) <std::sync::mpmc::list::Channel<std::fs::File>>::recv::{closure#1}
  1.410 ms (0.35%; cumulative 76.62%) <core::hash::sip::Hasher<core::hash::sip::Sip13Rounds> as core::hash::Hasher>::write
  1.392 ms (0.34%; cumulative 76.97%) <indexmap::map::IndexMap<alloc::string::String, serde_json::value::Value> as core::clone::Clone>::clone
  1.339 ms (0.33%; cumulative 77.30%) _madvise
  1.319 ms (0.33%; cumulative 77.62%) fun_189780
  1.287 ms (0.32%; cumulative 77.94%) alloc::fmt::format::format_inner
  1.282 ms (0.32%; cumulative 78.26%) fun_188620
  1.261 ms (0.31%; cumulative 78.57%) access
  1.221 ms (0.30%; cumulative 78.87%) readlink
  1.220 ms (0.30%; cumulative 79.18%) <hashbrown::raw::RawTable<usize>>::reserve_rehash::<indexmap::inner::get_hash<alloc::string::String, serde_json::value::Value>::{closure#0}>
  1.213 ms (0.30%; cumulative 79.48%) fun_ab230
  1.210 ms (0.30%; cumulative 79.77%) <hashbrown::map::HashMap<&str, (), std::hash::random::RandomState>>::insert
  1.192 ms (0.29%; cumulative 80.07%) unlinkat
CPU attribution only: child lifetimes, filesystem latency, waits and process creation are not disjointly measured here.

pid 583.1: sampled CPU 370.232 ms; lifetime 2011.289 ms; threads 332
nearest first-party module (otherwise leaf):
  104.227 ms (28.15%; cumulative 28.15%) onevcs::git
  35.930 ms (9.70%; cumulative 37.86%) libc.so.6!unresolved
  30.021 ms (8.11%; cumulative 45.97%) onepipeline::ledger
  29.539 ms (7.98%; cumulative 53.94%) onevcs::launch_trace
  19.303 ms (5.21%; cumulative 59.16%) poll
  10.550 ms (2.85%; cumulative 62.01%) _close
  10.176 ms (2.75%; cumulative 64.76%) onemessagebus::schema
  8.795 ms (2.38%; cumulative 67.13%) onevcs::lock
  7.516 ms (2.03%; cumulative 69.16%) onepipeline::writeback
  5.532 ms (1.49%; cumulative 70.66%) _munmap
  5.516 ms (1.49%; cumulative 72.15%) onepipeline::checkpoint
  5.274 ms (1.42%; cumulative 73.57%) onetaskgraph_local_md::staged
  4.979 ms (1.34%; cumulative 74.91%) onemessagebus::envelope
  4.489 ms (1.21%; cumulative 76.13%) onevcs::workspace
  4.178 ms (1.13%; cumulative 77.26%) onetaskgraph_local_md::yaml
  3.812 ms (1.03%; cumulative 78.29%) onepipeline::executor
  3.805 ms (1.03%; cumulative 79.31%) syscall
  3.464 ms (0.94%; cumulative 80.25%) onetaskgraph_local_md::LocalMdSource>
nearest first-party function (otherwise leaf):
  38.245 ms (10.33%; cumulative 10.33%) onevcs::git::run_fed::{closure#4}
  35.930 ms (9.70%; cumulative 20.03%) libc.so.6!unresolved
  21.516 ms (5.81%; cumulative 25.85%) <onevcs::git::PipeCapture>::start::<std::process::ChildStdout>
  20.580 ms (5.56%; cumulative 31.40%) onepipeline::ledger::write_atomic
  19.303 ms (5.21%; cumulative 36.62%) poll
  16.808 ms (4.54%; cumulative 41.16%) <onevcs::git::PipeCapture>::start::<std::process::ChildStderr>
  10.550 ms (2.85%; cumulative 44.01%) _close
  10.176 ms (2.75%; cumulative 46.76%) <onemessagebus::schema::Registry>::register_schema
  10.103 ms (2.73%; cumulative 49.49%) <onevcs::launch_trace::Tracer>::write
  8.086 ms (2.18%; cumulative 51.67%) onevcs::git::run_fed
  6.091 ms (1.65%; cumulative 53.31%) onevcs::lock::wait_for
  5.532 ms (1.49%; cumulative 54.81%) _munmap
  5.274 ms (1.42%; cumulative 56.23%) onetaskgraph_local_md::staged
  5.078 ms (1.37%; cumulative 57.60%) onevcs::launch_trace::launch
  5.073 ms (1.37%; cumulative 58.98%) onevcs::git::wait_for_exit
  4.526 ms (1.22%; cumulative 60.20%) onevcs::git::await_an_ending
  4.178 ms (1.13%; cumulative 61.33%) onetaskgraph_local_md::yaml::from_str::<onetaskgraph_local_md::FrontMatter>
  3.805 ms (1.03%; cumulative 62.35%) syscall
  3.042 ms (0.82%; cumulative 63.18%) <onevcs::launch_trace::Wait>::into_launch
  2.987 ms (0.81%; cumulative 63.98%) onepipeline::ledger::read_published
  2.872 ms (0.78%; cumulative 64.76%) <onevcs::launch_trace::Launch>::spawned
  2.596 ms (0.70%; cumulative 65.46%) <onevcs::git::PipeCapture>::finish
  2.584 ms (0.70%; cumulative 66.16%) onepipeline::checkpoint::readable
  2.526 ms (0.68%; cumulative 66.84%) onevcs::policy::read
  2.411 ms (0.65%; cumulative 67.49%) onevcs::launch_trace::sleep
  2.393 ms (0.65%; cumulative 68.14%) <onevcs::launch_trace::Launch>::exited
  2.339 ms (0.63%; cumulative 68.77%) onepipeline::driver::attach
  2.336 ms (0.63%; cumulative 69.40%) _madvise
  2.047 ms (0.55%; cumulative 69.95%) onevcs::workspace::existing
  2.027 ms (0.55%; cumulative 70.50%) <onemessagebus::transport::LocalTransport>::open_records
  2.024 ms (0.55%; cumulative 71.05%) onevcs::lock::record_owner
  2.007 ms (0.54%; cumulative 71.59%) onepipeline::ledger::read_range
  1.905 ms (0.51%; cumulative 72.10%) <onepipeline::executor::LocalExecutor as onepipeline::executor::Executor>::capacity
  1.789 ms (0.48%; cumulative 72.59%) onevcs::store::load
  1.682 ms (0.45%; cumulative 73.04%) onetaskgraph_core::resolve::validate_sources
  1.651 ms (0.45%; cumulative 73.49%) onepipeline::writeback::project
  1.645 ms (0.44%; cumulative 73.93%) onevcs::git::local_setting
  1.634 ms (0.44%; cumulative 74.37%) onevcs::launch_trace::output
  1.568 ms (0.42%; cumulative 74.80%) <alloc::collections::btree::map::IntoIter<alloc::string::String, onepipeline::payload::ContinuedRework>>::dying_next
  1.543 ms (0.42%; cumulative 75.21%) <onemessagebus::envelope::Envelope<onepipeline::vocabulary::Agent> as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::StrRead>>
  1.534 ms (0.41%; cumulative 75.63%) onevcs::workspace::all
  1.491 ms (0.40%; cumulative 76.03%) <onepipeline::executor::LocalExecutor as onepipeline::executor::Executor>::dispatch
  1.409 ms (0.38%; cumulative 76.41%) onevcs::home::ensure_dir
  1.378 ms (0.37%; cumulative 76.78%) onepipeline::engine::wait_for_work
  1.352 ms (0.37%; cumulative 77.15%) onepipeline::vcs::landings_now
  1.348 ms (0.36%; cumulative 77.51%) onepipeline::writeback::create
  1.315 ms (0.36%; cumulative 77.87%) fun_188620
  1.174 ms (0.32%; cumulative 78.18%) onepipeline::ledger::read_envelope_lines
  1.160 ms (0.31%; cumulative 78.50%) read
  1.157 ms (0.31%; cumulative 78.81%) <onetaskgraph_local_md::LocalMdSource>::confined
  1.122 ms (0.30%; cumulative 79.11%) onepipeline::ledger::read_appended_envelope_lines
  1.095 ms (0.30%; cumulative 79.41%) <onemessagebus::envelope::Envelope<onepipeline::vocabulary::Agent> as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>>
  1.031 ms (0.28%; cumulative 79.69%) onevcs::home::atomic_write
  1.029 ms (0.28%; cumulative 79.97%) <std::sys::sync::mutex::futex::Mutex>::lock_contended
  1.028 ms (0.28%; cumulative 80.24%) <onevcs::checkout_reads::Hold as core::ops::drop::Drop>::drop
self function:
  98.488 ms (26.60%; cumulative 26.60%) libc.so.6!unresolved
  19.303 ms (5.21%; cumulative 31.82%) poll
  17.377 ms (4.69%; cumulative 36.51%) _close
  16.536 ms (4.47%; cumulative 40.98%) _open64
  11.363 ms (3.07%; cumulative 44.04%) rename
  11.343 ms (3.06%; cumulative 47.11%) syscall
  10.574 ms (2.86%; cumulative 49.96%) _write
  10.048 ms (2.71%; cumulative 52.68%) fsync
  9.272 ms (2.50%; cumulative 55.18%) statx
  7.412 ms (2.00%; cumulative 57.18%) mkdir
  7.257 ms (1.96%; cumulative 59.14%) _munmap
  5.666 ms (1.53%; cumulative 60.68%) wait4
  4.880 ms (1.32%; cumulative 61.99%) fun_ab630
  4.415 ms (1.19%; cumulative 63.19%) _mmap
  4.369 ms (1.18%; cumulative 64.37%) read
  4.324 ms (1.17%; cumulative 65.53%) malloc
  4.031 ms (1.09%; cumulative 66.62%) pipe2
  3.946 ms (1.07%; cumulative 67.69%) fun_188620
  3.032 ms (0.82%; cumulative 68.51%) <core::hash::sip::Hasher<core::hash::sip::Sip13Rounds> as core::hash::Hasher>::write
  2.804 ms (0.76%; cumulative 69.26%) fun_188d80
  2.671 ms (0.72%; cumulative 69.99%) <indexmap::map::IndexMap<alloc::string::String, serde_json::value::Value>>::insert_full
  2.336 ms (0.63%; cumulative 70.62%) _madvise
  2.137 ms (0.58%; cumulative 71.19%) fun_a9c70
  1.961 ms (0.53%; cumulative 71.72%) _libc_free
  1.793 ms (0.48%; cumulative 72.21%) <std::hash::random::RandomState as core::hash::BuildHasher>::hash_one::<&alloc::string::String>
  1.770 ms (0.48%; cumulative 72.69%) fun_98d50
  1.736 ms (0.47%; cumulative 73.16%) fun_ab230
  1.708 ms (0.46%; cumulative 73.62%) clock_gettime
  1.652 ms (0.45%; cumulative 74.06%) clock_nanosleep
  1.568 ms (0.42%; cumulative 74.49%) <alloc::collections::btree::map::IntoIter<alloc::string::String, onepipeline::payload::ContinuedRework>>::dying_next
  1.543 ms (0.42%; cumulative 74.90%) readlink
  1.530 ms (0.41%; cumulative 75.32%) serde_json::ser::format_escaped_str_contents::<&mut alloc::vec::Vec<u8>, serde_json::ser::PrettyFormatter>
  1.474 ms (0.40%; cumulative 75.71%) pthread_create
  1.471 ms (0.40%; cumulative 76.11%) unlinkat
  1.354 ms (0.37%; cumulative 76.48%) <indexmap::inner::Core<alloc::string::String, serde_json::value::Value>>::push_entry
  1.284 ms (0.35%; cumulative 76.82%) <core::str::pattern::CharSearcher as core::str::pattern::Searcher>::next_match
  1.227 ms (0.33%; cumulative 77.16%) _mprotect
  1.180 ms (0.32%; cumulative 77.47%) <indexmap::map::IndexMap<alloc::string::String, serde_json::value::Value> as core::clone::Clone>::clone
  1.082 ms (0.29%; cumulative 77.77%) fun_188cf0
  1.029 ms (0.28%; cumulative 78.04%) <std::sys::sync::mutex::futex::Mutex>::lock_contended
  1.018 ms (0.27%; cumulative 78.32%) fun_a9b90
  0.979 ms (0.26%; cumulative 78.58%) ftruncate
  0.974 ms (0.26%; cumulative 78.85%) <hashbrown::raw::RawTable<usize>>::reserve_rehash::<indexmap::inner::get_hash<alloc::string::String, serde_json::value::Value>::{closure#0}>
  0.953 ms (0.26%; cumulative 79.10%) _open64_nocancel
  0.951 ms (0.26%; cumulative 79.36%) <jsonschema::keywords::ref_::DirectRefValidator<jsonschema_value::serde_json::SerdeJson> as jsonschema::validator::Validate>::validate
  0.933 ms (0.25%; cumulative 79.61%) <regex_automata::nfa::thompson::map::Utf8BoundedEntry as alloc::vec::spec_from_elem::SpecFromElem>::from_elem::<alloc::alloc::Global>
  0.931 ms (0.25%; cumulative 79.86%) <u32 as core::fmt::Display>::fmt
  0.918 ms (0.25%; cumulative 80.11%) <indexmap::map::IndexMap<alloc::string::String, serde_json::value::Value>>::get_index_of::<str>
CPU attribution only: child lifetimes, filesystem latency, waits and process creation are not disjointly measured here.
```

#### release after

```text
pid 90.1: sampled CPU 355.412 ms; lifetime 1300.908 ms; threads 274
nearest first-party module (otherwise leaf):
  98.151 ms (27.62%; cumulative 27.62%) onevcs::git
  40.047 ms (11.27%; cumulative 38.88%) onepipeline::ledger
  31.686 ms (8.92%; cumulative 47.80%) libc.so.6!unresolved
  23.307 ms (6.56%; cumulative 54.36%) onevcs::launch_trace
  15.891 ms (4.47%; cumulative 58.83%) poll
  9.968 ms (2.80%; cumulative 61.63%) onevcs::lock
  9.391 ms (2.64%; cumulative 64.27%) onemessagebus::schema
  9.370 ms (2.64%; cumulative 66.91%) onepipeline::writeback
  8.974 ms (2.52%; cumulative 69.44%) onevcs::home
  7.087 ms (1.99%; cumulative 71.43%) _close
  5.378 ms (1.51%; cumulative 72.94%) onepipeline::engine
  4.849 ms (1.36%; cumulative 74.31%) _munmap
  4.757 ms (1.34%; cumulative 75.65%) onetaskgraph_core::resolve
  4.277 ms (1.20%; cumulative 76.85%) syscall
  3.794 ms (1.07%; cumulative 77.92%) onevcs::boundary
  3.493 ms (0.98%; cumulative 78.90%) onetaskgraph_local_md::LocalMdSource>
  3.491 ms (0.98%; cumulative 79.88%) onevcs::policy
  3.439 ms (0.97%; cumulative 80.85%) onevcs::store
nearest first-party function (otherwise leaf):
  36.597 ms (10.30%; cumulative 10.30%) onevcs::git::run_fed::{closure#4}
  31.686 ms (8.92%; cumulative 19.21%) libc.so.6!unresolved
  28.871 ms (8.12%; cumulative 27.34%) onepipeline::ledger::write_atomic
  15.891 ms (4.47%; cumulative 31.81%) poll
  14.483 ms (4.07%; cumulative 35.88%) <onevcs::git::PipeCapture>::start::<std::process::ChildStdout>
  10.822 ms (3.04%; cumulative 38.93%) <onevcs::git::PipeCapture>::start::<std::process::ChildStderr>
  10.279 ms (2.89%; cumulative 41.82%) onevcs::git::await_an_ending
  9.559 ms (2.69%; cumulative 44.51%) onevcs::git::run_fed
  9.068 ms (2.55%; cumulative 47.06%) <onemessagebus::schema::Registry>::register_schema
  8.822 ms (2.48%; cumulative 49.54%) <onevcs::launch_trace::Tracer>::write
  7.414 ms (2.09%; cumulative 51.63%) onevcs::lock::wait_for
  7.216 ms (2.03%; cumulative 53.66%) onevcs::home::ensure_dir
  7.087 ms (1.99%; cumulative 55.65%) _close
  6.449 ms (1.81%; cumulative 57.47%) onevcs::git::wait_for_exit
  4.849 ms (1.36%; cumulative 58.83%) _munmap
  4.757 ms (1.34%; cumulative 60.17%) onetaskgraph_core::resolve::validate_sources
  4.277 ms (1.20%; cumulative 61.37%) syscall
  3.476 ms (0.98%; cumulative 62.35%) onevcs::launch_trace::output
  3.285 ms (0.92%; cumulative 63.28%) <onepipeline::ledger::RunPaths>::create
  3.161 ms (0.89%; cumulative 64.16%) onepipeline::writeback::project
  3.094 ms (0.87%; cumulative 65.04%) onevcs::policy::read
  2.971 ms (0.84%; cumulative 65.87%) onetaskgraph_local_md::staged
  2.932 ms (0.82%; cumulative 66.70%) onevcs::boundary::scope::derive_with
  2.896 ms (0.81%; cumulative 67.51%) onetaskgraph_local_md::yaml::from_str::<onetaskgraph_local_md::FrontMatter>
  2.796 ms (0.79%; cumulative 68.30%) <onevcs::checkout_reads::Hold as core::ops::drop::Drop>::drop
  2.732 ms (0.77%; cumulative 69.07%) <onevcs::git::PipeCapture>::finish
  2.729 ms (0.77%; cumulative 69.83%) onepipeline::ledger::read_published
  2.572 ms (0.72%; cumulative 70.56%) <onevcs::launch_trace::Launch>::exited
  2.453 ms (0.69%; cumulative 71.25%) onevcs::launch_trace::launch
  2.217 ms (0.62%; cumulative 71.87%) onevcs::git::local_setting
  2.185 ms (0.61%; cumulative 72.49%) read
  2.112 ms (0.59%; cumulative 73.08%) onevcs::store::migrate
  2.016 ms (0.57%; cumulative 73.65%) onepipeline::ledger::read_appended_envelope_lines
  1.998 ms (0.56%; cumulative 74.21%) <onevcs::launch_trace::Launch>::spawned
  1.893 ms (0.53%; cumulative 74.74%) <onetaskgraph_local_md::LocalMdSource>::directory
  1.859 ms (0.52%; cumulative 75.27%) onepipeline::driver::attach
  1.804 ms (0.51%; cumulative 75.77%) onevcs::launch_trace::sleep
  1.579 ms (0.44%; cumulative 76.22%) onevcs::lock::record_owner
  1.569 ms (0.44%; cumulative 76.66%) onepipeline::engine::watch_releases
  1.528 ms (0.43%; cumulative 77.09%) <onemessagebus::transport::LocalTransport>::lock
  1.469 ms (0.41%; cumulative 77.50%) <onevcs::launch_trace::Wait>::into_launch
  1.422 ms (0.40%; cumulative 77.90%) fun_9c7e0
  1.410 ms (0.40%; cumulative 78.30%) <onepipeline::executor::LocalExecutor as onepipeline::executor::Executor>::dispatch
  1.377 ms (0.39%; cumulative 78.69%) onepipeline::engine::converge
  1.334 ms (0.38%; cumulative 79.06%) onepipeline::vcs::landings_now
  1.327 ms (0.37%; cumulative 79.44%) onevcs::store::load
  1.265 ms (0.36%; cumulative 79.79%) onepipeline::writeback::closer
  1.155 ms (0.32%; cumulative 80.12%) _madvise
self function:
  80.552 ms (22.66%; cumulative 22.66%) libc.so.6!unresolved
  20.735 ms (5.83%; cumulative 28.50%) _close
  19.600 ms (5.51%; cumulative 34.01%) _open64
  18.703 ms (5.26%; cumulative 39.28%) syscall
  17.819 ms (5.01%; cumulative 44.29%) mkdir
  16.158 ms (4.55%; cumulative 48.84%) poll
  10.810 ms (3.04%; cumulative 51.88%) rename
  9.619 ms (2.71%; cumulative 54.58%) _write
  8.389 ms (2.36%; cumulative 56.94%) _munmap
  6.225 ms (1.75%; cumulative 58.70%) fsync
  6.022 ms (1.69%; cumulative 60.39%) fun_ab630
  5.667 ms (1.59%; cumulative 61.98%) pipe2
  5.477 ms (1.54%; cumulative 63.53%) wait4
  5.176 ms (1.46%; cumulative 64.98%) statx
  5.006 ms (1.41%; cumulative 66.39%) jsonschema::compiler::compile_with_internal::<jsonschema_value::serde_json::SerdeJson>
  4.208 ms (1.18%; cumulative 67.57%) fun_188d80
  3.715 ms (1.05%; cumulative 68.62%) fun_a9c70
  3.331 ms (0.94%; cumulative 69.56%) _libc_free
  2.974 ms (0.84%; cumulative 70.39%) fun_ab230
  2.961 ms (0.83%; cumulative 71.23%) read
  2.935 ms (0.83%; cumulative 72.05%) fun_98d50
  2.489 ms (0.70%; cumulative 72.75%) malloc
  2.233 ms (0.63%; cumulative 73.38%) _mmap
  2.221 ms (0.62%; cumulative 74.01%) fstatat
  2.030 ms (0.57%; cumulative 74.58%) fun_188cf0
  1.804 ms (0.51%; cumulative 75.08%) clock_nanosleep
  1.572 ms (0.44%; cumulative 75.53%) <onevcs::launch_trace::Launch>::exited
  1.569 ms (0.44%; cumulative 75.97%) onepipeline::engine::watch_releases
  1.422 ms (0.40%; cumulative 76.37%) fun_9c7e0
  1.375 ms (0.39%; cumulative 76.76%) readlink
  1.364 ms (0.38%; cumulative 77.14%) foldhash::hash_bytes_long
  1.332 ms (0.37%; cumulative 77.51%) <hashbrown::raw::RawTable<(&str, ())>>::reserve_rehash::<hashbrown::map::make_hasher<&str, (), std::hash::random::RandomState>::{closure#0}>
  1.155 ms (0.32%; cumulative 77.84%) _madvise
  1.126 ms (0.32%; cumulative 78.16%) unlinkat
  0.993 ms (0.28%; cumulative 78.43%) <alloc::raw_vec::RawVecInner>::finish_grow
  0.990 ms (0.28%; cumulative 78.71%) <regex_automata::nfa::thompson::map::Utf8BoundedEntry as alloc::vec::spec_from_elem::SpecFromElem>::from_elem::<alloc::alloc::Global>
  0.979 ms (0.28%; cumulative 78.99%) core::ptr::drop_glue::<indexmap::Bucket<alloc::string::String, serde_json::value::Value>>
  0.977 ms (0.27%; cumulative 79.26%) getdents64
  0.894 ms (0.25%; cumulative 79.52%) referencing::registry::build::index_resources::<ahash::hash_map::AHashMap<fluent_uri::imp::Uri<alloc::string::String>, referencing::registry::input::PendingResource>>
  0.887 ms (0.25%; cumulative 79.76%) <core::hash::sip::Hasher<core::hash::sip::Sip13Rounds> as core::hash::Hasher>::write
  0.876 ms (0.25%; cumulative 80.01%) chmod
CPU attribution only: child lifetimes, filesystem latency, waits and process creation are not disjointly measured here.

pid 583.1: sampled CPU 457.114 ms; lifetime 2166.491 ms; threads 332
nearest first-party module (otherwise leaf):
  126.825 ms (27.74%; cumulative 27.74%) onevcs::git
  57.214 ms (12.52%; cumulative 40.26%) poll
  41.429 ms (9.06%; cumulative 49.32%) libc.so.6!unresolved
  38.404 ms (8.40%; cumulative 57.73%) onevcs::launch_trace
  29.031 ms (6.35%; cumulative 64.08%) onepipeline::ledger
  11.794 ms (2.58%; cumulative 66.66%) onetaskgraph_local_md::yaml
  10.989 ms (2.40%; cumulative 69.06%) onevcs::lock
  10.578 ms (2.31%; cumulative 71.37%) onemessagebus::schema
  9.516 ms (2.08%; cumulative 73.46%) onepipeline::writeback
  8.525 ms (1.86%; cumulative 75.32%) _close
  5.302 ms (1.16%; cumulative 76.48%) syscall
  4.626 ms (1.01%; cumulative 77.49%) onepipeline::checkpoint
  4.384 ms (0.96%; cumulative 78.45%) onepipeline::engine
  4.187 ms (0.92%; cumulative 79.37%) onemessagebus::envelope
  3.753 ms (0.82%; cumulative 80.19%) onepipeline::driver
nearest first-party function (otherwise leaf):
  57.214 ms (12.52%; cumulative 12.52%) poll
  46.648 ms (10.20%; cumulative 22.72%) onevcs::git::run_fed::{closure#4}
  41.429 ms (9.06%; cumulative 31.78%) libc.so.6!unresolved
  24.153 ms (5.28%; cumulative 37.07%) <onevcs::git::PipeCapture>::start::<std::process::ChildStdout>
  21.540 ms (4.71%; cumulative 41.78%) onepipeline::ledger::write_atomic
  14.453 ms (3.16%; cumulative 44.94%) <onevcs::git::PipeCapture>::start::<std::process::ChildStderr>
  14.062 ms (3.08%; cumulative 48.02%) <onevcs::launch_trace::Tracer>::write
  11.431 ms (2.50%; cumulative 50.52%) onetaskgraph_local_md::yaml::from_str::<onetaskgraph_local_md::FrontMatter>
  11.337 ms (2.48%; cumulative 53.00%) onevcs::git::await_an_ending
  10.578 ms (2.31%; cumulative 55.31%) <onemessagebus::schema::Registry>::register_schema
  10.383 ms (2.27%; cumulative 57.58%) onevcs::git::run_fed
  9.678 ms (2.12%; cumulative 59.70%) onevcs::git::wait_for_exit
  8.525 ms (1.86%; cumulative 61.57%) _close
  7.702 ms (1.68%; cumulative 63.25%) onevcs::lock::wait_for
  6.172 ms (1.35%; cumulative 64.60%) onevcs::launch_trace::sleep
  5.302 ms (1.16%; cumulative 65.76%) syscall
  4.635 ms (1.01%; cumulative 66.78%) <onevcs::git::PipeCapture>::finish
  4.245 ms (0.93%; cumulative 67.70%) onevcs::launch_trace::launch
  4.125 ms (0.90%; cumulative 68.61%) <onevcs::launch_trace::Launch>::exited
  3.704 ms (0.81%; cumulative 69.42%) onepipeline::driver::attach
  3.660 ms (0.80%; cumulative 70.22%) _munmap
  3.460 ms (0.76%; cumulative 70.97%) onetaskgraph_local_md::staged
  3.142 ms (0.69%; cumulative 71.66%) <onevcs::launch_trace::Launch>::spawned
  2.743 ms (0.60%; cumulative 72.26%) jsonschema::compiler::compile_with_internal::<jsonschema_value::serde_json::SerdeJson>
  2.539 ms (0.56%; cumulative 72.82%) <onevcs::launch_trace::Wait>::into_launch
  2.473 ms (0.54%; cumulative 73.36%) onevcs::workspace::existing
  2.451 ms (0.54%; cumulative 73.89%) <onepipeline::checkpoint::Coverage>::sealed_with
  2.299 ms (0.50%; cumulative 74.40%) onetaskgraph_core::resolve::validate_sources
  2.210 ms (0.48%; cumulative 74.88%) onevcs::policy::read
  2.203 ms (0.48%; cumulative 75.36%) onevcs::launch_trace::wait
  2.165 ms (0.47%; cumulative 75.84%) onepipeline::verbs::render_adopted
  2.069 ms (0.45%; cumulative 76.29%) onepipeline::ledger::read_published
  1.942 ms (0.42%; cumulative 76.71%) onepipeline::writeback::project
  1.898 ms (0.42%; cumulative 77.13%) _madvise
  1.888 ms (0.41%; cumulative 77.54%) <onepipeline::executor::LocalExecutor as onepipeline::executor::Executor>::dispatch
  1.842 ms (0.40%; cumulative 77.95%) <onemessagebus::envelope::Envelope<onepipeline::vocabulary::Agent> as serde_core::de::Deserialize>::deserialize::<&mut serde_json::de::Deserializer<serde_json::read::StrRead>>
  1.840 ms (0.40%; cumulative 78.35%) onevcs::lock::open_at
  1.717 ms (0.38%; cumulative 78.72%) onepipeline::engine::wait_for_work
  1.642 ms (0.36%; cumulative 79.08%) onevcs::home::ensure_dir
  1.633 ms (0.36%; cumulative 79.44%) read
  1.581 ms (0.35%; cumulative 79.79%) <onepipeline::projection::RunState>::settled
  1.456 ms (0.32%; cumulative 80.10%) onevcs::launch_trace::output
self function:
  113.894 ms (24.92%; cumulative 24.92%) libc.so.6!unresolved
  57.809 ms (12.65%; cumulative 37.56%) poll
  21.838 ms (4.78%; cumulative 42.34%) syscall
  19.746 ms (4.32%; cumulative 46.66%) _open64
  12.952 ms (2.83%; cumulative 49.49%) _write
  12.344 ms (2.70%; cumulative 52.19%) _close
  9.149 ms (2.00%; cumulative 54.19%) rename
  9.101 ms (1.99%; cumulative 56.19%) fun_ab630
  8.989 ms (1.97%; cumulative 58.15%) wait4
  8.231 ms (1.80%; cumulative 59.95%) mkdir
  7.249 ms (1.59%; cumulative 61.54%) fsync
  7.208 ms (1.58%; cumulative 63.12%) _munmap
  6.860 ms (1.50%; cumulative 64.62%) statx
  6.721 ms (1.47%; cumulative 66.09%) fun_188d80
  6.046 ms (1.32%; cumulative 67.41%) <str>::trim_matches::<<char>::is_whitespace>
  5.735 ms (1.25%; cumulative 68.66%) clock_nanosleep
  4.926 ms (1.08%; cumulative 69.74%) malloc
  4.327 ms (0.95%; cumulative 70.69%) fun_98d50
  4.125 ms (0.90%; cumulative 71.59%) _libc_free
  3.925 ms (0.86%; cumulative 72.45%) read
  3.468 ms (0.76%; cumulative 73.21%) _mmap
  3.391 ms (0.74%; cumulative 73.95%) core::slice::sort::shared::smallsort::insertion_sort_shift_left::<(jsonschema::keywords::Keyword, alloc::boxed::Box<dyn jsonschema::validator::Validate>), <[(jsonschema::keywords::Keyword, alloc::boxed::Box<dyn jsonschema::validator::Validate>)]>::sort_by_key<u8, <jsonschema::node::SchemaNode>::from_keywords::{closure#0}>::{closure#0}>
  3.311 ms (0.72%; cumulative 74.67%) jsonschema::compiler::compile_with_internal::<jsonschema_value::serde_json::SerdeJson>
  3.276 ms (0.72%; cumulative 75.39%) pipe2
  3.238 ms (0.71%; cumulative 76.10%) fun_ab230
  2.683 ms (0.59%; cumulative 76.69%) fun_a9c70
  2.569 ms (0.56%; cumulative 77.25%) <core::hash::sip::Hasher<core::hash::sip::Sip13Rounds> as core::hash::Hasher>::write
  2.416 ms (0.53%; cumulative 77.78%) fun_a9b90
  2.165 ms (0.47%; cumulative 78.25%) onepipeline::verbs::render_adopted
  2.144 ms (0.47%; cumulative 78.72%) _open64_nocancel
  2.051 ms (0.45%; cumulative 79.17%) unlinkat
  1.898 ms (0.42%; cumulative 79.58%) _madvise
  1.581 ms (0.35%; cumulative 79.93%) <onepipeline::projection::RunState>::settled
  1.564 ms (0.34%; cumulative 80.27%) fun_188cf0
CPU attribution only: child lifetimes, filesystem latency, waits and process creation are not disjointly measured here.
```

On release before, the largest attributed start module is onevcs::git (90.912 ms, 22.48% of 404.486 ms observed CPU). privateterms::matcher accounts for 57.249 ms (14.15%); release optimization inlines the contributor initializer into matcher construction, so that figure is not an initializer-only measurement. Release adopt is led by onevcs::git (104.227 ms, 28.15% of 370.232 ms observed CPU). The cut’s source omits the initializer; it is absent from the reported dominant cut stacks. Observed CPU varies between captures even for adopt, which had no initializer before, so these CPU observations are not a causal saving estimate. Hosted means alone price the option.

### Local interval accounting and residual

Command: `node scripts/engine-startup.mjs phases LABEL=LOCAL_CAPTURE_DIR`. It consumes the unchanged shared launch trace through its existing reader. The following command outputs are from scratch/local-debug-baseline-partition.txt and local-release-partition.txt, on source 7d949434 plus the subsequently recorded fixture test. Each figure cites the corresponding local capture and recorded trace above. Intervals are clipped to the first-proc-to-final-end trace window, with overlap removed: known spawn-call intervals take precedence, then other known operation-live intervals, then unallocated time. Program rows and wait overlays are deliberately not added to the partition or to sampled CPU.

#### debug baseline trace

```text
before: local trace intervals (not a hosted wall-clock estimate)
start pid 90: trace window 2432.921 ms; 119 timed / 119 observed child or opaque operations
partition: spawn-call 346.662 ms + operation-live outside spawn 1065.186 ms + unallocated 1021.073 ms = trace window 2432.921 ms
  git: 116/116 timed; inclusive operation 1206.399 ms; spawn-call 343.355 ms (program rows can overlap)
  onevcs: 2/2 timed; inclusive operation 170.753 ms; spawn-call 0.000 ms (program rows can overlap)
  fake-oneagentgraph: 1/1 timed; inclusive operation 34.696 ms; spawn-call 3.308 ms (program rows can overlap)
recorded child-wait loops: 768.402 ms over 113 records (overlays, not an additive partition)
adopt pid 583: trace window 2832.135 ms; 144 timed / 144 observed child or opaque operations
partition: spawn-call 411.823 ms + operation-live outside spawn 1069.268 ms + unallocated 1351.045 ms = trace window 2832.135 ms
  git: 143/143 timed; inclusive operation 1462.177 ms; spawn-call 409.132 ms (program rows can overlap)
  fake-oneagentgraph: 1/1 timed; inclusive operation 18.914 ms; spawn-call 2.690 ms (program rows can overlap)
recorded child-wait loops: 951.300 ms over 141 records (overlays, not an additive partition)
Unallocated wall includes own CPU, filesystem latency, descheduling, non-child waits and unmeasured children, not separately measured. Operation-live means observed child/opaque-call presence, not proof the engine was waiting. Separately sampled engine CPU overlaps all timeline buckets and must not be added to them. Untimed operations remain uncertainty; clocks exclude creation of the engine, pre-first-trace initialization and teardown.
```

#### release baseline trace

```text
release: local trace intervals (not a hosted wall-clock estimate)
start pid 90: trace window 1817.150 ms; 119 timed / 119 observed child or opaque operations
partition: spawn-call 83.052 ms + operation-live outside spawn 491.812 ms + unallocated 1242.287 ms = trace window 1817.150 ms
  git: 116/116 timed; inclusive operation 517.272 ms; spawn-call 81.917 ms (program rows can overlap)
  onevcs: 2/2 timed; inclusive operation 38.947 ms; spawn-call 0.000 ms (program rows can overlap)
  fake-oneagentgraph: 1/1 timed; inclusive operation 18.645 ms; spawn-call 1.135 ms (program rows can overlap)
recorded child-wait loops: 381.575 ms over 113 records (overlays, not an additive partition)
adopt pid 583: trace window 1964.572 ms; 144 timed / 144 observed child or opaque operations
partition: spawn-call 105.363 ms + operation-live outside spawn 526.096 ms + unallocated 1333.113 ms = trace window 1964.572 ms
  git: 143/143 timed; inclusive operation 622.786 ms; spawn-call 104.216 ms (program rows can overlap)
  fake-oneagentgraph: 1/1 timed; inclusive operation 8.673 ms; spawn-call 1.147 ms (program rows can overlap)
recorded child-wait loops: 456.129 ms over 141 records (overlays, not an additive partition)
Unallocated wall includes own CPU, filesystem latency, descheduling, non-child waits and unmeasured children, not separately measured. Operation-live means observed child/opaque-call presence, not proof the engine was waiting. Separately sampled engine CPU overlaps all timeline buckets and must not be added to them. Untimed operations remain uncertainty; clocks exclude creation of the engine, pre-first-trace initialization and teardown.
```

Filesystem wall latency is not independently measured. The unallocated bucket includes own CPU, filesystem latency, descheduling, non-child waits and children whose duration is unmeasured. Known operation-live intervals can themselves overlap those activities. CPU samples are an overlay, not a disjoint wall bucket. Per-platform hosted partitioning remains unavailable under a45, and no new engine phase instrumentation was added. The trace excludes process creation of the engine, pre-first-trace initialization and final teardown. Attached start/adopt windows include the plan execution they drive; these means are not a time-to-first-output metric.

The new phase command is exercised over the recorded driver fixture and 543 verbatim primary-process records from the kept-branch capture. Its regression test asserts the actual interval totals, program totals and child-wait overlays. At that checkpoint fourteen parser tests passed; the later excluded-record regression brings the current total to fifteen, all passing (exclusion-tests.log). One UTF-8 test was deliberately observed failing with an ASCII input decoder, then the decoder was restored and all tests passed. The strengthened public matcher verifier still passes 6,588 cases; the earlier exhaustive positive-context verifier also completed 6,672,456 comparisons, but is not claimed as exhaustive negative-path coverage.

The existing workspace-nx module had one build-reuse assertion fail while other Cargo builds were active (scratch/workspace-input-tests.log); after compilation became idle, `node --test --test-name-pattern='leaves nothing to compile for the debug build the npm publish journey packs' npm/test/workspace-nx.test.mjs` ran that actual case and passed, with no skipped cases (workspace-build-reuse-recovery.log). No shared test, lint threshold or workflow guard was changed to clear it. The demo workflow blob remains byte-identical to the accepted baseline, and its actual events CI-shape test passed after linking the installed dependency directory (demo-actual-ci-shape-green.log).

### Baseline audit

The companion [baseline job audit](baseline-audit.md) gives every baseline job’s processor, full Nx selection, cache-step restore result/time, pre-e2e time and available compile log lines, coverage refusal lines, and capsule references for complete test-name catalogs. Quiet Cargo archive builds do not print compile-only durations on every platform; those are explicitly unavailable rather than inferred from the tier duration.


### Collection pause — 2026-11-15 19:30 UTC

The read-only collector stopped before reading run status because the shared GitHub rate allowance fell below the a0 floor. No additional run result is claimed. Manager a55 directs resuming after the approximate 19:52 UTC reset, collecting only finished jobs about every fifteen minutes. It raises the plan-wide open demonstration cap to seven; the per-spike two-open limit remains. No run is cancelled and no demonstration head is pushed.


### Suppression inventory for this spike’s diff

The inherited Cargo.toml `crates_stay_in_one_cargo_workspace` block covers the added privateterms path patch: retaining the released dependency layout makes this never-land prototype comparable. The new Cargo.lock marker immediately above the privateterms package gives the same measurement-only reason and preserves upstream package metadata (a1/a49). No workspace enforcement configuration changes.

Three `tool_output_is_signal` blocks in scripts/engine-startup.mjs cover phase rows, per-job fold rows and side-comparison rows; the block in scripts/engine-startup-cpu.py covers the CPU attribution rows. Each explains that the requested measurement deliverable needs multiple rows and explicit uncertainty rather than a mutation-success line (a1).

Two `changed_behavior_has_e2e` blocks in scripts/engine-startup-profile.sh cover sampler/discovery setup and privileged recording. Their evidence is the actual recorded debug/release subprocess journeys; portable parser tests cannot assume a built Rust e2e binary and perf privilege before Rust tiers finish (a1). The README’s `work_goes_through_command_surface` marker covers the narrow Node test command: the shared just recipe has no module filter, and adding a new recipe would expand this measurement-only diff. The report’s `least_privilege_grants` marker covers the exact historical Docker invocation: changing its granted permissions in the quoted provenance would misstate the command that produced the captures; the disposable container was removed. These are all added directives or existing directive regions this spike changes.


### Baseline named outcomes and recovery

The committed evidence/baseline-outcomes.json.gz preserves the existing windows-job-profile.mjs `outcomes` parser’s final status by binary/test for each saved log, plus names that had a failure and later a PASS in that same job. Producer: `node $ONEPIPELINE_NODE_SCRATCH_DIR/baseline-outcomes.mjs`, local source a7eb7ef6. These are log-reported counts, not full suite counts: Linux’s quiet output omits most individual PASS lines, so its e2e name catalog must be paired with its all-passed nextest summary. Profile presence alone is insufficient. Recovered names include other tiers; no omitted name is silently treated as passed.

```text
attempt 1 linux: 52 named results; 51 final passes; recovered []
attempt 1 macos: 3774 named results; 3769 final passes; recovered []
attempt 1 windows: 3405 named results; 3403 final passes; recovered []
attempt 2 linux: 13 named results; 12 final passes; recovered ["onepipeline::narrow_write a_runs_settlement_under_a_rewriting_policy_reads_what_it_reads_under_none","onepipeline::note a_binding_note_enters_the_bar_its_judge_decides_against_as_the_workers_own","onepipeline::note a_cancel_beside_a_live_note_is_applied_as_its_envelope_is_handed","onepipeline::note a_cancel_preempts_the_unanswered_note_to_its_node_and_what_waits_behind_it","onepipeline::note a_cancel_refuses_the_note_offered_to_its_node_and_the_one_queued_behind_it","onepipeline::pooled_slot a_pooled_identity_holds_the_second_node_until_the_first_hands_its_slot_back","onepipeline::release_channel a_channel_the_0_28_2_wheel_writes_is_the_channel_this_build_writes_and_each_reads_the_other","onepipeline::release_channel a_channel_this_build_writes_is_read_by_the_0_28_2_wheel_and_what_it_appends_is_read_back","onepipeline::release_channel extended_checkpoint::a_checkpoint_the_host_view_extended_is_refolded_by_the_preceding_release","onepipeline::smoke a_lifecycle_node_opens_a_real_pull_request_merges_it_and_the_base_advances"]
attempt 2 macos: 3774 named results; 3769 final passes; recovered []
attempt 2 windows: 3405 named results; 3401 final passes; recovered []
```


### Round-two local rehearsal

Before starting the remaining hosted round, source 01a5186d drove the same real debug e2e journey with the contributor environment selector and launch tracing, then folded it with `node scripts/engine-startup.mjs phases contributors-round2=$ONEPIPELINE_NODE_SCRATCH_DIR/round2-local-slice`. Both commands exited zero. The journey, phase output and trace are committed as evidence/round2-local-slice.*.gz. This is a trace-integrity rehearsal, not a local before/after timing comparison. The engine implementation is the same sampled build a792bafc; no rebuild-dependent claim is made. The second demonstration is prepared locally at identical head dd64b7d1 and has not been pushed while collection is paused.


### Hosted checkpoint — 2026-11-15 19:52 UTC

Run 43751624389 attempt 1 remains in flight: Windows and Linux running, macOS queued. Lint job132926636183 was cancelled at its existing thirty-minute limit (19:17:56–19:48:13 UTC), while the judged tier was running; this is not a successful hosted lint claim. The log confirms the actual checkout GITHUB_SHA 9852d29ba162d2c7810b4abe217fc9b1a8d7ed1d (lines113,492,495), head dd64b7d141e0326cfeb4991b420bdb25387e0b53, base 0ddf1b3eb9b5094629a620a0426e893c99a7c32f. The workflow blob remains db2bbe081a7b9319eb38bd014e72350b389e8b01. Its saved log and shared-inventory jobFacts output are under evidence/hosted/43751624389/attempt-1/lint.*.gz; the facts list all 54 selected lint tasks and each cache-step restore/time. They show no cache-save line and a receipt of zero Nx entries/bytes stored or replayed; the workflow’s trust-step override also sets read_only=true/promotable=false and uv save-cache=false. No test-job timing or outcome is available yet.

Capacity query at this checkpoint found seven plan demonstration heads (#1613–1616, #1628, #1633–1634), exactly the a55 cap. The second startup head remains local; no additional run starts until a slot is free. Completed local work is committed/published, and remaining work is the three after points per platform, their audits, final comparison and final judged pass.


### Scope of the contributor option

The demo selects the vendored library’s default at compile time, so all linked privateterms consumers in that build use the bypass, including helper executables that link it. A complete-tier mean difference therefore prices the compatible library option across the selected suite; it does not isolate engine-only initializer savings. The per-verb trace separately measures observed onepipeline start/adopt lifetimes. This is also why the release/adoption path names the library and its consumers, and why the full matcher behavior comparison is required. No full-tier saving is attributed exclusively to onepipeline’s own CPU.


### Parser-test path correction

After source 9902aa4c, inspection found the new parser journeys used URL.pathname for executable/fixture paths, which retains URL escapes and can produce /D:/ drive paths on Windows. They now use Node fileURLToPath consistently. Existing journeys, assertions and fixtures are unchanged; no existing repository test is modified. In a copied workspace named `native path proof`, the earlier tests failed all fifteen on encoded paths (evidence/native-path-induced-red.log.gz). After conversion all fifteen pass there (native-path-spaces-green.log.gz) and in the ordinary checkout (native-path-tests.log.gz); Biome check is green (native-path-static.log.gz). The first copied recovery attempt lacked samply-version.txt, which was added to that disposable copy before the successful run. This is Linux evidence of path handling, not a claim of a Windows run. The active hosted head dd64b7d1 still has the earlier parser tests and remains frozen; subsequent demonstration heads will carry this test-only portability repair. It cannot affect the Rust prototype’s performance, but any hosted failures of those old tests remain reported rather than relabelled as passes.


The 20:23 snapshots (jobs-2023.json.gz for each point) record point1 macOS job132926636261 starting at 20:15:54 UTC. Point2 job ids are Linux132941104723, lint132941104952, macOS132941104978 and Windows132941105006; macOS is still queued. No test job has completed and no after figure is inferred from in-progress logs.


Collection at approximately 20:38 UTC stopped at the shared GitHub allowance floor before any run read; second-point collection was not attempted. Under a55, the next read waits for the approximate 20:52 UTC reset. No newer job state or result is inferred from this pause. The active demo heads remain frozen.


### Adoption effort by file and repository

Estimated implementation effort for this option is two dispatches, not a measured duration. In github.com/nickderobertis/privateterms, change src/matcher.rs and add a public-API equivalence journey (proposed tests/startup_equivalence.rs); validate negative-text throughput before choosing this eager-table/full-scan trade for a release. The existing src/normalize.rs contributor implementation becomes unused by the cut but need not be rewritten to demonstrate it. A compatible performance-only implementation takes a patch release. In github.com/nickderobertis/onepipeline, move Cargo.lock’s privateterms resolution to that patch, keep the existing caret requirements, run the consumer proofs and take a performance patch release. onevcs and onetaskgraph-core already admit the compatible privateterms patch; neither needs a release for this lock adoption. These are proposed production files/dispatches, not changes made by this never-land measurement branch. The prototype’s files are vendor/privateterms/src/matcher.rs and vendor/privateterms/src/lib.rs, plus the root Cargo.toml/Cargo.lock path adoption; the measurement commands, fixtures and report remain measurement artifacts.


### Hosted checkpoint — 2026-11-15 20:52 UTC

Point2 lint job132941104952 ran 20:08:13–20:38:29 UTC and was cancelled at its existing thirty-minute job limit, while waiting for the existing Pages build. Before cancellation its judged tier completed at 20:34:17 UTC: history20261115T203417Z-7802b reports forty rules, twenty-six passed, one failed, eight skipped, five not relevant. The sole finding is comments_earn_their_place at demo-only src/lib.rs:191, the byte-identical no-behavior source touch the comparison requires. Under a49, that non-measurement finding is retained as a limitation rather than starting another style repair/refinement loop or changing the agreed source touch across sides. This hosted finding is not a green local judged-lint claim; the final local pass is still owed. The stock workflow’s later Pages work is unchanged by this spike; the timeout does not establish a timing result for any test tier.

The saved lint log confirms point2 GITHUB_SHA 591dd36224fb3402f9f48130175a8c690b4eb524 (lines471,496,499), head ecf2eb593848478fbb97423e49c6f2ad5cfb42a5 and base 934364b8d2bdd125f81828df1fd09e86b6aaca16. The shared jobFacts output lists all fifty-four selected lint tasks and each cache step’s restore/time; it records no Actions-cache save line and zero Nx entries/bytes stored or replayed (log line3624). Both files and the current job snapshot are under evidence/hosted/43756326338/attempt-1/lint.*.gz and jobs-2052.json.gz. Point1’s three test jobs remain in progress; point2 Linux/Windows run and macOS is queued. No test-job artifact has completed, and no after timing is claimed.


### Comparison scope update — manager a67

Manager a67 directs two valid hosted after runs per platform against the two inventory baseline points, and explicitly no third run anywhere. A prototype-defect-aborted run is not a point. The two startup demonstrations already in flight are therefore the complete planned after side, subject to successful collection and valid e2e execution. It also limits by-name proof to the main e2e tier; other tiers use counts and unchanged Nx selections. This supersedes earlier three-point plans in the dated checkpoints.

The shared reader’s global run-ID join is not reliable for every non-e2e task block. For example, one note profile starts after the summary its ID joins, and narrow_write/pooled_slot IDs have no parsed summary. No shared shape or parser is changed. Follow-up drafted: drafts:brief-process-launch-speed-2026-11-14-spikes/drafts/20261115T214355Z-verify-nextest-run-id-joins-across-interleaved-t. Quiet-log missing individual PASS lines outside e2e are not treated as failures or fabricated as named passes.

### First completed after point: Linux

Hosted source: run43751624389 attempt1, test-linux job132926636190, head dd64b7d141e0326cfeb4991b420bdb25387e0b53, confirmed GITHUB_SHA9852d29ba162d2c7810b4abe217fc9b1a8d7ed1d, workflow db2bbe081a7b9319eb38bd014e72350b389e8b01. Artifact14181940231 has digest sha256:2b8fd5a781acfe39fce6903a8e3fd96b188c01c28a167056461f5faa89e75a22. Log, capsule and fold output are committed under evidence/hosted/43751624389/attempt-1/test-linux.*.gz. Processor Intel(R) Xeon(R)6975P-C, four logical cores. The job ran 19:17:57–21:29:54 UTC, 131.950000 minutes. The e2e tier started 93.215533 minutes into the job, including quiet archive compilation and earlier tiers; this is not compile-only time.

The 2155-name profile catalog exactly equals both baseline Linux catalogs (digest83531b68a69ebaf7), and the nextest summary reports 2155/2155 passed. Thus every named baseline main-e2e test is present and passed on this point; no main-e2e test failed then recovered. The complete names remain in the committed capsules. Both baseline 37-task Nx selections exactly equal this job’s selection. The existing outcomes command also finds all 51 explicitly logged baseline1 passes passing here. Its nine absent baseline2 individual PASS lines are other tiers, not the main e2e proof, and are retained in the raw comparison output without claiming a by-name verdict for those tiers (a67).

Per-tier normal complete-count summaries in baseline1 and after1 have the same sequence: 151/151, 3/3, 1/1, 75/75, 1/1, 3/3, 1/1, 7/7, 2/2, 1/1, 49/49, 2155/2155, 1/1, followed by three patch-audit re-runs at 1/0. Baseline2 has interrupted other tiers, explicitly including 4/75 run with zero passed (job132799383400 log2323), 1373/1443 with1372 passed (log2634), and1/49 withzero passed (log9071); their nominal selections are unchanged, while this after point completes those normal tier counts. Those partial baseline executions and recovered names are not silently normalized into complete runs. The shared nextestRuns helper omits partial N/M forms, so the original log is authoritative for those lines.

Cache steps from this job’s jobFacts capsule:

| Step | Restore | Seconds |
| --- | --- | ---: |
| Nx server startup | no restore | 0.005 |
| rust-cache | partial restore key | 66.172 |
| setup-node | no restore recorded | 4.963 |
| setup-python | miss | 6.168 |
| setup-uv | no restore recorded | 2.368 |
| bootstrap actions/cache/restore | hit | 2.169 |

The trust-step read-only override is unchanged. No Actions-cache save line is recorded; the Nx receipt reports zero entries, zero bytes stored and zero replayed. Merged Linux coverage was not produced: log9924 lists tasks not run, the failed task is onepipeline:test-rest, and log10150 reports patch_pin::every_patched_member_of_one_repository_comes_from_one_revision failing all three automatic re-runs. Under c7.0/c9.1 the LCOV floor remains95.74; no test/workflow/justfile workaround is made. The source functions/branches this option stops exercising or adds are listed in the prototype section above.

This partial comparison was produced at source bc7a13e9 by `node scripts/engine-startup.mjs fold before/1/linux=BASELINE1_LINUX before/2/linux=BASELINE2_LINUX contributors/1/linux=AFTER1_LINUX`, with directories as recorded in the capsule producer and the baseline checkpoint. It is an earlier-commit checkpoint; the final-source fold remains owed. Output:

```text
before/1/linux (linux)
excluded input records: {"malformed":0,"invalid":0,"records":0,"waits":0,"otherBinaries":176}
runner: Intel(R) Xeon(R) 6975P-C; cores: 4
e2e minutes: 37.919033
start: 820.300928 ms mean; 2351 completed / 2371 observed
adopt: 1396.245420 ms mean; 340 completed / 345 observed
clock: completed launch-trace lifetime; excludes process creation; unfinished processes excluded
test profiles: 2155; names digest: 83531b68a69ebaf7; scope: {"id":"21fd59ea-9add-43aa-abae-0fd219c703a7","start":1794728919.828,"end":1794731194.969,"seconds":2275.142,"run":2155,"passed":2155,"line":"Summary [2275.142s] 2155 tests run: 2155 passed (23 slow), 1 skipped","records":174,"starts":56,"os":0,"otherProfiles":1}

before/2/linux (linux)
excluded input records: {"malformed":0,"invalid":0,"records":0,"waits":0,"otherBinaries":18}
runner: INTEL(R) XEON(R) PLATINUM 8573C; cores: 4
e2e minutes: 41.055683
start: 891.417304 ms mean; 2351 completed / 2371 observed
adopt: 1476.363930 ms mean; 340 completed / 346 observed
clock: completed launch-trace lifetime; excludes process creation; unfinished processes excluded
test profiles: 2155; names digest: 83531b68a69ebaf7; scope: {"id":"2077affc-b3f8-4207-9d22-fc387055d3ae","start":1794750371.568,"end":1794752834.909,"seconds":2463.341,"run":2155,"passed":2155,"line":"Summary [2463.341s] 2155 tests run: 2155 passed (24 slow), 1 skipped","records":19,"starts":5,"os":0,"otherProfiles":1}

contributors/1/linux (linux)
excluded input records: {"malformed":0,"invalid":0,"records":0,"waits":0,"otherBinaries":176}
runner: Intel(R) Xeon(R) 6975P-C; cores: 4
e2e minutes: 36.829633
start: 779.037746 ms mean; 2351 completed / 2371 observed
adopt: 1374.532448 ms mean; 340 completed / 346 observed
clock: completed launch-trace lifetime; excludes process creation; unfinished processes excluded
test profiles: 2155; names digest: 83531b68a69ebaf7; scope: {"id":"a725d099-f0a9-4952-baaf-e1f1196f43d3","start":1794775869.932,"end":1794778079.71,"seconds":2209.778,"run":2155,"passed":2155,"line":"Summary [2209.778s] 2155 tests run: 2155 passed (23 slow), 1 skipped","records":174,"starts":56,"os":0,"otherProfiles":1}

before/linux: e2e mean 39.487358 min; range 37.919033..41.055683; n=2

contributors/linux: e2e mean 36.829633 min; range 36.829633..36.829633; n=1

contributors/linux: before-minus-after 2.657725 min; undecided (inside a side's spread); typical PR estimate (27/30) 2.391953 min (undecided, not a saving)

contributors/1/linux: baseline test profiles missing []; profile presence does not establish pass status (read nextest outcomes)
```

The measured difference printed above is2.657725 e2e minutes, inside the baseline spread, hence undecided with only one after point. Its27/30 typical-PR estimate2.391953 minutes is not claimed as a saving. No Windows/macOS after figure is yet available. Both demonstration PRs remain in flight; no new demonstration is authorized or planned under a67.


### Linux two-point comparison complete

Hosted point2 source: run43756326338 attempt1, test-linux job132941104723, head ecf2eb593848478fbb97423e49c6f2ad5cfb42a5, confirmed GITHUB_SHA591dd36224fb3402f9f48130175a8c690b4eb524, workflow db2bbe081a7b9319eb38bd014e72350b389e8b01. Artifact14183665542 has digest sha256:6eba0a7b5b91e3330981602e5f7a06ab3ced6399b13ac49fbc1b5e44583c18f8. Source log, capsule and comparison output are under evidence/hosted/43756326338/attempt-1. Processor AMD EPYC9V45 96-Core Processor, four logical cores. The job ran20:08:13–22:19:09 UTC:130.933333 minutes, with89.705850 minutes before the e2e tier including quiet compilation/earlier tiers.

Both after points have the exact baseline2155-name main-e2e catalog (digest83531b68a69ebaf7) and2155/2155 passed summaries. No main-e2e failure/recovery was observed. Both full37-task Nx selections equal both baseline selections. Point2’s normal complete-count nextest sequence exactly matches point1 and baseline1; interrupted baseline2 executions remain identified in the prior section. No source test or workflow guard changed. Native-path parser journeys are included in point2, adding their requested measurement proof without changing existing journeys.

Point2 cache steps:

| Step | Restore | Seconds |
| --- | --- | ---: |
| Nx server startup | no restore | 0.003 |
| rust-cache | partial restore key | 59.687 |
| setup-node | no restore recorded | 0.592 |
| setup-python | miss | 5.443 |
| setup-uv | no restore recorded | 2.035 |
| bootstrap actions/cache/restore | hit | 1.066 |

The read-only workflow guard is unchanged; no Actions-cache save line appears and the Nx receipt is zero entries/bytes stored and zero replayed. Merged coverage was not produced on this point either: log9899 names tasks not run, log9905 names failed onepipeline:test-rest, and log10125 records patch_pin failing all three re-runs. Floor95.74 remains unchanged under c9.1.

The following current two-point output was produced on commit276c5d44 with `node scripts/engine-startup.mjs fold before/1/linux=BASELINE1_LINUX before/2/linux=BASELINE2_LINUX contributors/1/linux=AFTER1_LINUX contributors/2/linux=AFTER2_LINUX`, using the baseline directories and scratch/runs/{RUN}-attempt-1/test-linux paths already recorded above. It is a checkpoint on that commit, not yet the final-source rerun:

```text
before/1/linux (linux)
excluded input records: {"malformed":0,"invalid":0,"records":0,"waits":0,"otherBinaries":176}
runner: Intel(R) Xeon(R) 6975P-C; cores: 4
e2e minutes: 37.919033
start: 820.300928 ms mean; 2351 completed / 2371 observed
adopt: 1396.245420 ms mean; 340 completed / 345 observed
clock: completed launch-trace lifetime; excludes process creation; unfinished processes excluded
test profiles: 2155; names digest: 83531b68a69ebaf7; scope: {"id":"21fd59ea-9add-43aa-abae-0fd219c703a7","start":1794728919.828,"end":1794731194.969,"seconds":2275.142,"run":2155,"passed":2155,"line":"Summary [2275.142s] 2155 tests run: 2155 passed (23 slow), 1 skipped","records":174,"starts":56,"os":0,"otherProfiles":1}

before/2/linux (linux)
excluded input records: {"malformed":0,"invalid":0,"records":0,"waits":0,"otherBinaries":18}
runner: INTEL(R) XEON(R) PLATINUM 8573C; cores: 4
e2e minutes: 41.055683
start: 891.417304 ms mean; 2351 completed / 2371 observed
adopt: 1476.363930 ms mean; 340 completed / 346 observed
clock: completed launch-trace lifetime; excludes process creation; unfinished processes excluded
test profiles: 2155; names digest: 83531b68a69ebaf7; scope: {"id":"2077affc-b3f8-4207-9d22-fc387055d3ae","start":1794750371.568,"end":1794752834.909,"seconds":2463.341,"run":2155,"passed":2155,"line":"Summary [2463.341s] 2155 tests run: 2155 passed (24 slow), 1 skipped","records":19,"starts":5,"os":0,"otherProfiles":1}

contributors/1/linux (linux)
excluded input records: {"malformed":0,"invalid":0,"records":0,"waits":0,"otherBinaries":176}
runner: Intel(R) Xeon(R) 6975P-C; cores: 4
e2e minutes: 36.829633
start: 779.037746 ms mean; 2351 completed / 2371 observed
adopt: 1374.532448 ms mean; 340 completed / 346 observed
clock: completed launch-trace lifetime; excludes process creation; unfinished processes excluded
test profiles: 2155; names digest: 83531b68a69ebaf7; scope: {"id":"a725d099-f0a9-4952-baaf-e1f1196f43d3","start":1794775869.932,"end":1794778079.71,"seconds":2209.778,"run":2155,"passed":2155,"line":"Summary [2209.778s] 2155 tests run: 2155 passed (23 slow), 1 skipped","records":174,"starts":56,"os":0,"otherProfiles":1}

contributors/2/linux (linux)
excluded input records: {"malformed":0,"invalid":0,"records":0,"waits":0,"otherBinaries":176}
runner: AMD EPYC 9V45 96-Core Processor; cores: 4
e2e minutes: 38.982833
start: 844.160199 ms mean; 2351 completed / 2371 observed
adopt: 1462.730949 ms mean; 340 completed / 345 observed
clock: completed launch-trace lifetime; excludes process creation; unfinished processes excluded
test profiles: 2155; names digest: 83531b68a69ebaf7; scope: {"id":"2b7751fc-b3a2-4958-9d26-d9712e29bf3a","start":1794778675.351,"end":1794781014.321,"seconds":2338.97,"run":2155,"passed":2155,"line":"Summary [2338.970s] 2155 tests run: 2155 passed (24 slow), 1 skipped","records":169,"starts":54,"os":0,"otherProfiles":1}

before/linux: e2e mean 39.487358 min; range 37.919033..41.055683; n=2

contributors/linux: e2e mean 37.906233 min; range 36.829633..38.982833; n=2

contributors/linux: before-minus-after 1.581125 min; undecided (inside a side's spread); typical PR estimate (27/30) 1.423013 min (undecided, not a saving)

contributors/1/linux: baseline test profiles missing []; profile presence does not establish pass status (read nextest outcomes)

contributors/2/linux: baseline test profiles missing []; profile presence does not establish pass status (read nextest outcomes)
```

Linux therefore has measured side means39.487358 before and37.906233 after, a1.581125-minute difference, inside the observed spread. The printed typical-PR estimate1.423013 minutes is undecided, not a saving. Different processor models remain a confound; this small sample does not establish an engine-only causal effect. Linux’s two required after points are complete; Windows and macOS still need collection from the two existing runs. No third run starts under a67.


## Candidate budgets

This spike measured no candidate budget itself.

## Design findings

- Completed start/adopt launch-trace means replace unavailable per-verb test-profile executable totals identically on both sides under manager a37. They exclude engine creation, pre-trace initialization and teardown.
- Only hosted after runs can decide whether the contributor bypass saves e2e minutes; local profiles justify the candidate but do not price it.
