#!/usr/bin/env node
// Measurement-only edits to `.github/workflows/ci.yml`, for a throwaway "DO NOT MERGE"
// demonstration run whose timings `scripts/ci-time-profile.mjs` reads back. Spike tooling
// (spike-onepipeline-profile): nothing in the repository's own CI runs this.
//
//   node scripts/ci-time-workflow.mjs <ci.yml> [--overhead-sample hash:K/N]
//
// Rewrites the file in place, so that a run of it:
//   - writes no Actions-cache or Nx remote-cache entry: every Nx sidecar is
//     `CACHE_READ_ONLY: '1'` (the sidecar answers every PUT 403) and setup-uv's cache is
//     off; rust-cache is already `save-if: false` and refused here otherwise;
//   - publishes no visual docs and runs no judged lint in `lint`;
//   - samples CPU and non-loopback network every two seconds in `test-linux` and
//     `test-macos`, from the job's first step, into `$RUNNER_TEMP/measure`;
//   - times every rustc invocation there through a `RUSTC_WRAPPER` set after the
//     rust-cache step (so its key is unchanged, and cargo-llvm-cov chains it);
//   - keeps Nx's own task trace (`NX_PROFILE`), which a failed gate writes too, and the
//     Nx task graph the job ran, and uploads all of it as `measure-<job>`;
//   - with `--overhead-sample`, runs that partition of the e2e tier twice after the
//     gate in `test-linux`, instrumented and then plain, for coverage's runtime overhead.
// The job targets the graph step plans are read from `scripts/ci-job.sh` beside the
// workflow, the one copy of what each job runs. The same input always produces the same
// output, so both sides of an A/B carry byte-identical edits. Exit 0 rewritten; 1 the file
// is not the shape these edits expect, with the reason and an ACTION line (nothing is
// written); 2 a usage error.
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const MARK = "measurement-only (spike-onepipeline-profile)";
export const JOBS = ["test-linux", "test-macos"];

/** A refusal: why, and what to do about it. */
export class Refusal extends Error {
  constructor(message, action) {
    super(message);
    this.action = action;
  }
}
const SHAPE =
  "run this on main's .github/workflows/ci.yml as committed (`git show origin/main:.github/workflows/ci.yml`), or update the anchors here to the workflow's new shape";
function fail(message, action = SHAPE) {
  throw new Refusal(message, action);
}

/** Each job's Nx targets, from `scripts/ci-job.sh`'s `case` over the job names. */
export function jobTargets(ciJob) {
  const targets = {};
  for (const job of JOBS) {
    const line = ciJob
      .split("\n")
      .find((l) => new RegExp(`^\\s*[\\w| -]*\\b${job}\\b[\\w| -]*\\) targets="([^"]+)"`).test(l));
    if (!line)
      fail(
        `scripts/ci-job.sh names no targets for ${job}`,
        "check that scripts/ci-job.sh still sets `targets=` for each job in its case",
      );
    targets[job] = /targets="([^"]+)"/.exec(line)[1];
  }
  return targets;
}

function replaceAll(text, from, to, expected) {
  const count = text.split(from).length - 1;
  if (count !== expected) fail(`expected ${expected} of ${JSON.stringify(from)}, found ${count}`);
  return text.split(from).join(to);
}

// One job's block: from its `  <job>:` line to the next top-level job.
function jobBlock(text, job) {
  const start = text.indexOf(`\n  ${job}:\n`);
  if (start < 0) fail(`no job ${job}`);
  const rest = text.slice(start + 1);
  const next = rest.slice(1).search(/\n {2}[a-z][\w-]*:\n/);
  const end = next < 0 ? text.length : start + 1 + next + 2;
  return [start + 1, end];
}

function editJob(text, job, edit) {
  const [start, end] = jobBlock(text, job);
  return text.slice(0, start) + edit(text.slice(start, end)) + text.slice(end);
}

function insertBefore(block, anchor, insertion) {
  const at = block.indexOf(anchor);
  if (at < 0 || block.indexOf(anchor, at + 1) >= 0)
    fail(`expected exactly one ${JSON.stringify(anchor.trim())}`);
  return block.slice(0, at) + insertion + block.slice(at);
}

const indent = (lines, by) =>
  lines
    .split("\n")
    .map((line) => (line ? " ".repeat(by) + line : line))
    .join("\n");

export const SAMPLER = `m="$1"
if [ "$(uname -s)" = Linux ]; then
  while :; do
    echo "$(date +%s) $(head -1 /proc/stat) | $(cut -d' ' -f4 /proc/loadavg) | $(sed 's/:/ /' /proc/net/dev | awk 'NR > 2 && $1 != "lo" {rx += $2; tx += $10} END {print rx + 0, tx + 0}')" >> "$m/samples.txt"
    sleep 2
  done
else
  while :; do
    cpu="$(iostat -n 0 -c 2 -w 1 | tail -1)"
    echo "$(date +%s) $cpu | $(sysctl -n vm.loadavg | tr -d '{}') | $(netstat -ibn | awk '$3 ~ /^<Link/ && $1 !~ /^lo/ {if (NF >= 11) {rx += $7; tx += $10} else {rx += $6; tx += $9}} END {print rx + 0, tx + 0}')" >> "$m/samples.txt"
    sleep 1
  done
fi
`;

export const TIMER = `#!/usr/bin/env perl
# Times one rustc invocation and appends one tab-separated line to ONEPIPELINE_RUSTC_TIMINGS:
# start, end, crate name, crate types, instrumented (1/0), out dir, exit code, compiler path.
use strict;
use warnings;
use Time::HiRes qw(time);
my $t0 = time;
my $rc = system { $ARGV[0] } @ARGV;
my $t1 = time;
my $code = $rc == -1 ? 127 : ($? & 127) ? 128 + ($? & 127) : $? >> 8;
my ($name, $types, $instrumented, $out) = ('', '', 0, '');
for (my $i = 1; $i < @ARGV; $i++) {
  my $arg = $ARGV[$i];
  my $next = $i + 1 < @ARGV ? $ARGV[$i + 1] : '';
  if ($arg eq '--crate-name') { $name = $next; }
  elsif ($arg eq '--crate-type') { $types .= ($types ? ',' : '') . $next; }
  elsif ($arg eq '--test') { $types .= ($types ? ',' : '') . 'test'; }
  elsif ($arg eq '--out-dir') { $out = $next; }
  if ($arg =~ /instrument-coverage/) { $instrumented = 1; }
}
if ($name ne '' && $ENV{ONEPIPELINE_RUSTC_TIMINGS}) {
  if (open(my $f, '>>', $ENV{ONEPIPELINE_RUSTC_TIMINGS})) {
    printf $f "%.3f\\t%.3f\\t%s\\t%s\\t%d\\t%s\\t%d\\t%s\\n", $t0, $t1, $name, $types, $instrumented, $out, $code, $ARGV[0];
    close $f;
  } else {
    # The compile itself is unaffected; only its timing is lost, and the analysis would
    # undercount compile without saying so.
    print STDERR "rustc-timer: cannot append to $ENV{ONEPIPELINE_RUSTC_TIMINGS}: $! (this compile is not timed)\\n";
  }
}
exit $code;
`;

function heredoc(path, body) {
  return `cat > "${path}" <<'MEASURE_EOF'\n${body}MEASURE_EOF\n`;
}

function samplerStep() {
  const run =
    'm="$RUNNER_TEMP/measure"; mkdir -p "$m"\n' +
    '{ uname -a; getconf _NPROCESSORS_ONLN; date -u +%s; } > "$m/host.txt"\n' +
    heredoc("$m/sampler.sh", SAMPLER) +
    'nohup bash "$m/sampler.sh" "$m" > /dev/null 2>&1 &\n';
  return `      - name: 'Measure: sample CPU and network every two seconds (${MARK})'\n        shell: bash\n        run: |\n${indent(run, 10)}`;
}

function timerStep() {
  const run =
    'm="$RUNNER_TEMP/measure"; mkdir -p "$m"\n' +
    heredoc("$m/rustc-timer", TIMER) +
    'chmod +x "$m/rustc-timer"\n' +
    'echo "RUSTC_WRAPPER=$m/rustc-timer" >> "$GITHUB_ENV"\n' +
    'echo "ONEPIPELINE_RUSTC_TIMINGS=$m/rustc.tsv" >> "$GITHUB_ENV"\n' +
    "# Nx's own trace of every task's start and end, written whatever the tasks' outcome. Nx\n" +
    "# joins this onto its working directory, so it is relative to the checkout.\n" +
    'echo "NX_PROFILE=target/ci-time-nx-profile.json" >> "$GITHUB_ENV"\n';
  return `      - name: 'Measure: time every rustc invocation, after rust-cache keyed (${MARK})'\n        shell: bash\n        run: |\n${indent(run, 10)}`;
}

function graphStep(targets) {
  const run =
    'm="$RUNNER_TEMP/measure"; mkdir -p "$m"\n' +
    'if [ -f target/ci-time-nx-profile.json ]; then cp target/ci-time-nx-profile.json "$m/nx-profile.json"; fi\n' +
    `if ! ONEPIPELINE_NX_SHOW_OUTPUT=1 env -u NX_PROFILE bash scripts/nx-affected.sh -t "${targets}" --graph=stdout > "$m/graph.json" 2> "$m/graph.err"; then\n` +
    '  echo "::warning::measurement: Nx could not write the task graph, so this run has no critical path; graph.err in the measure artifact says why"\n' +
    "fi\n" +
    'date -u +%s > "$m/ended.txt"\n';
  return `      - name: 'Measure: the Nx task graph this job ran (${MARK})'\n        if: always()\n        shell: bash\n        working-directory: \${{ github.workspace }}\n        run: |\n${indent(run, 10)}`;
}

// llmlint: ignore[work_goes_through_command_surface] One partition of one tier has to run both ways on the same runner after the gate; `_tier-test` takes no partition and `_tier-test-quick` partitions by count, not hash, so neither recipe can run this sample, and the commands are the ones those recipes run (coverage-unit.sh, nextest-run.sh).
function overheadStep(partition) {
  const run =
    'm="$RUNNER_TEMP/measure/overhead"; mkdir -p "$m"\n' +
    'filter="$(just --evaluate e2e-tier)"\n' +
    "run() {\n" +
    '  label="$1"; shift\n' +
    '  start="$(date +%s)"; status=0\n' +
    '  "$@" > "$m/$label.log" 2>&1 || status=$?\n' +
    '  echo "$label $start $(date +%s) $status" >> "$m/times.txt"\n' +
    '  if [ "$status" -ne 0 ]; then echo "::warning::measurement: $label exited $status; $label.log in the measure artifact holds its output, and the overhead figure leaves out what it did not run"; fi\n' +
    '  if [ -d target/test-profile ]; then mv target/test-profile "$m/$label-records"; fi\n' +
    "}\n" +
    "# The gate's records were uploaded by the step before; set them aside so each run\n" +
    "# below leaves only its own.\n" +
    'if [ -d target/test-profile ]; then mv target/test-profile "$m/gate-records"; fi\n' +
    `run instrumented env RUSTFLAGS="-D warnings" COVERAGE_UNIT_IGNORE_REGEX=/crates/testfakes/ bash scripts/coverage-unit.sh onepipeline-e2e-sample -- nextest --locked -E "$filter" --partition ${partition} --no-fail-fast --final-status-level fail\n` +
    `run plain-build cargo nextest run --locked --no-run -E "$filter"\n` +
    `run plain bash scripts/nextest-run.sh cargo nextest run --locked -E "$filter" --partition ${partition} --no-fail-fast --final-status-level fail\n`;
  return `      - name: 'Measure: one e2e partition instrumented, then plain (${MARK})'\n        if: \${{ !cancelled() }}\n        continue-on-error: true\n        timeout-minutes: 60\n        run: |\n${indent(run, 10)}`;
}

function uploadStep() {
  return `      - name: 'Measure: keep the samples, compile timings and graph (${MARK})'\n        if: always()\n        uses: actions/upload-artifact@v4\n        with:\n          name: measure-\${{ github.job }}\n          path: \${{ runner.temp }}/measure/\n          if-no-files-found: warn\n          retention-days: 30\n`;
}

/**
 * The measurement-only edits applied to one workflow's text, given `scripts/ci-job.sh`'s
 * text and an optional e2e partition (`hash:K/N`) to run twice after the gate.
 */
export function edit(original, ciJob, overhead = null) {
  const targets = jobTargets(ciJob);
  let text = original;
  if (text.includes(MARK))
    fail(
      "the workflow already carries the measurement edits",
      "start again from the workflow as committed; these edits are applied once",
    );
  text = replaceAll(
    text,
    // biome-ignore lint/suspicious/noTemplateCurlyInString: a GitHub Actions expression matched literally, not a JS template
    "CACHE_READ_ONLY: ${{ steps.nx-cache-trust.outputs.read_only == 'true' && '1' || '' }}",
    `CACHE_READ_ONLY: '1' # ${MARK}: every PUT answered 403`,
    4,
  );
  text = replaceAll(
    text,
    // biome-ignore lint/suspicious/noTemplateCurlyInString: a GitHub Actions expression matched literally, not a JS template
    "enable-cache: ${{ github.event.pull_request.head.repo.full_name == github.repository }}",
    `enable-cache: false # ${MARK}: no Actions-cache write`,
    4,
  );
  if (
    /save-if: (?!false\b)/.test(text) ||
    /uses: actions\/cache@|uses: actions\/cache\/save@/.test(text)
  )
    fail(
      "a step of this workflow can save an Actions cache, which a measurement run may not",
      "make every rust-cache step `save-if: false` and use `actions/cache/restore` only, then run this again",
    );
  // The jobs' own conditions stay as they are: tests/ci_workflows.rs holds every required
  // context to them, and a failing test there would end a fail-fast Linux tier early.
  text = replaceAll(
    text,
    '      VISUAL_DOCS_PUBLISH: "true"\n',
    `      VISUAL_DOCS_PUBLISH: "false" # ${MARK}: no gallery deploy or comment\n`,
    1,
  );
  text = replaceAll(
    text,
    // biome-ignore lint/suspicious/noTemplateCurlyInString: a GitHub Actions expression matched literally, not a JS template
    "      - name: Lint changed files\n        if: ${{ !cancelled() }}\n",
    `      - name: Lint changed files\n        if: false # ${MARK}: no judged lint over a measurement diff\n`,
    1,
  );
  for (const job of JOBS) {
    text = editJob(text, job, (block) => {
      let b = insertBefore(
        block,
        "      - name: Decide cache trust and configure the sidecar\n",
        samplerStep(),
      );
      const cache = b.indexOf("      - uses: Swatinem/rust-cache@v2.9.2\n");
      const saveIf = b.indexOf("          save-if: false\n", cache);
      if (cache < 0 || saveIf < 0) fail(`${job} has no read-only rust-cache step`);
      const after = saveIf + "          save-if: false\n".length;
      b = b.slice(0, after) + timerStep() + b.slice(after);
      let tail = graphStep(targets[job]);
      if (overhead && job === "test-linux") tail += overheadStep(overhead);
      tail += uploadStep();
      return insertBefore(b, "      - name: Report cache entries and task executions\n", tail);
    });
  }
  return text;
}

function usage(message) {
  process.stderr.write(
    `ci-time-workflow: ${message}\nusage: ci-time-workflow.mjs <ci.yml> [--overhead-sample hash:K/N]\n`,
  );
  process.exit(2);
}

function main(argv) {
  const args = [...argv];
  const file = args.shift();
  if (!file || file.startsWith("-")) usage("name the workflow file to edit");
  let overhead = null;
  while (args.length) {
    const flag = args.shift();
    const value = args.shift();
    if (flag !== "--overhead-sample") usage(`unknown argument ${flag}`);
    const m = /^hash:(\d+)\/(\d+)$/.exec(value ?? "");
    if (!m || Number(m[1]) < 1 || Number(m[1]) > Number(m[2]))
      usage("--overhead-sample takes hash:K/N with 1 <= K <= N");
    overhead = value;
  }
  // The workflow's own checkout: .github/workflows/ci.yml -> scripts/ci-job.sh.
  const ciJobPath = join(dirname(resolve(file)), "..", "..", "scripts", "ci-job.sh");
  let ciJob;
  try {
    ciJob = readFileSync(ciJobPath, "utf8");
  } catch {
    fail(
      `cannot read ${ciJobPath}, which names each job's Nx targets`,
      "run this on .github/workflows/ci.yml inside a checkout of the repository",
    );
  }
  writeFileSync(file, edit(readFileSync(file, "utf8"), ciJob, overhead));
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    if (!(error instanceof Refusal)) throw error;
    process.stderr.write(`ci-time-workflow: ${error.message}\nACTION: ${error.action}\n`);
    process.exit(1);
  }
}
