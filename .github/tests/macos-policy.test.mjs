import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";
import { test, after } from "node:test";
import { workflows, runGraph, triggered, digest, matrix, evaluate } from "../workflow-graph.mjs";

const root = resolve(".");
const scratch = mkdtempSync(
  join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? process.env.TMPDIR ?? "/tmp", "macos-contract-"),
);
after(() => rmSync(scratch, { recursive: true, force: true }));
const repo = join(scratch, "repo");
mkdirSync(repo);
function git(...args) {
  return execFileSync("git", args, { cwd: repo, encoding: "utf8" }).trim();
}
git("init", "-q");
git("config", "user.email", "fixture@example.com");
git("config", "user.name", "Fixture");
writeFileSync(
  join(repo, "Cargo.toml"),
  '[package]\nname="onepipeline"\nversion="1.2.3"\n[dependencies]\nsibling="0.1"\n',
);
writeFileSync(
  join(repo, "Cargo.lock"),
  'version=4\n[[package]]\nname="onepipeline"\nversion="1.2.3"\n[[package]]\nname="sibling"\nversion="0.1.0"\nsource="registry+https://example.com"\n',
);
writeFileSync(join(repo, "CHANGELOG.md"), "# Changelog\n");
writeFileSync(join(repo, "package.json"), ' {"version":"1.2.3","dependencies":{"sibling":"0.1"}}');
writeFileSync(join(repo, "source.rs"), "fn main() {}\n");
git("add", ".");
git("commit", "-qm", "fixture");
const base = git("rev-parse", "HEAD");
const heads = {};
for (const kind of ["source", "version", "dependency"]) {
  git("checkout", "-q", "--detach", base);
  if (kind === "source") writeFileSync(join(repo, "source.rs"), "fn main() { panic!(); }\n");
  if (kind === "version") {
    for (const path of ["Cargo.toml", "Cargo.lock", "package.json"])
      writeFileSync(
        join(repo, path),
        readFileSync(join(repo, path), "utf8").replaceAll("1.2.3", "1.2.4"),
      );
    writeFileSync(join(repo, "CHANGELOG.md"), "# Changelog\n## 1.2.4\n");
  }
  if (kind === "dependency")
    writeFileSync(
      join(repo, "Cargo.toml"),
      readFileSync(join(repo, "Cargo.toml"), "utf8").replace('sibling="0.1"', 'sibling="0.2"'),
    );
  git("add", ".");
  git("commit", "-qm", kind);
  heads[kind] = git("rev-parse", "HEAD");
}
const eventPath = join(scratch, "event.json");
function event(fork = false, branch = "release-plz-fixture") {
  return {
    action: "synchronize",
    repository: { full_name: "nickderobertis/onepipeline" },
    pull_request: {
      title: "chore: release",
      user: { login: "nickderobertis" },
      head: {
        ref: branch,
        repo: { full_name: fork ? "fork/onepipeline" : "nickderobertis/onepipeline" },
      },
      base: { ref: "main" },
    },
  };
}
function classify(payload, head, eventName = "pull_request") {
  writeFileSync(eventPath, typeof payload === "string" ? payload : JSON.stringify(payload));
  return JSON.parse(
    execFileSync(
      "node",
      [
        join(root, ".github/macos-policy.mjs"),
        "classify",
        "--event",
        eventPath,
        "--base",
        base,
        "--head",
        head,
      ],
      { cwd: repo, env: { ...process.env, GITHUB_EVENT_NAME: eventName }, encoding: "utf8" },
    ),
  );
}
test("real classifier admits only semantically verified same-repository automation branches", () => {
  assert.equal(classify(event(), heads.version).isReleaseOnly, true);
  for (const [payload, head] of [
    [event(), heads.source],
    [event(), heads.dependency],
    [event(true), heads.version],
    [event(false, "feature"), heads.version],
    [
      {
        ...event(),
        pull_request: {
          ...event().pull_request,
          head: { ref: "feature", repo: event().repository },
          user: { login: "github-actions[bot]" },
        },
      },
      heads.version,
    ],
    ["{malformed", heads.version],
    [{}, heads.version],
    [event(), "missing-ref"],
  ]) {
    const result = classify(payload, head);
    assert.equal(result.isReleaseOnly, false);
    assert.equal(result.macosRequired, true);
  }
  for (const kind of ["source", "dependency"]) {
    git("checkout", "-q", "--detach", heads[kind]);
    writeFileSync(join(repo, "CHANGELOG.md"), "release");
    git("add", ".");
    git("commit", "-qm", "mixed");
    assert.equal(classify(event(), git("rev-parse", "HEAD")).macosRequired, true);
  }
  assert.equal(classify("{malformed", heads.source, "push").macosRequired, false);
});
const required = [
  "cross (macos-latest)",
  "cross (windows-latest)",
  "deny",
  "gate",
  "install (macos-latest)",
  "install (ubuntu-latest)",
  "install (windows-latest)",
  "install-documented (macos-latest)",
  "install-documented (ubuntu-latest)",
  "install-documented (windows-latest)",
  "llmlint",
  "msrv",
  "pr-title",
  "release-compat",
  "smoke",
  "visual-docs / report (x86_64, shots/current, shots/verify, Visual docs)",
  "wheel",
];
const classes = [
  ["pr-opened", "pull_request", "opened"],
  ["pr-synchronize", "pull_request", "synchronize"],
  ["pr-reopened", "pull_request", "reopened"],
  ["base-push", "push", null],
  ["release-pr-merge", "push", null],
  ["release-tag", "push", null],
  ["release-published", "release", "published"],
  ["downstream", "workflow_run", "completed"],
  ["repository-dispatch", "repository_dispatch", null],
  ["schedule", "schedule", null],
  ["manual-default", "workflow_dispatch", null],
  ["manual-opt-in", "workflow_dispatch", null],
];
function scenario(id, eventName, action, kind, fork, opt, branch = "release-plz-fixture") {
  const payload = event(fork, branch);
  payload.action = action;
  if (eventName === "workflow_run")
    payload.workflow_run = { name: "Release", head_sha: heads[kind], conclusion: "success" };
  if (eventName === "release")
    payload.release = { tag_name: "v1.2.4", target_commitish: heads[kind] };
  return {
    id,
    eventName,
    event: payload,
    head: heads[kind],
    ref: id === "release-tag" ? "refs/tags/v1.2.4" : "refs/heads/main",
    inputs: { diagnostic: id === "manual-opt-in" },
    vars: { PYPI_PUBLISH: opt.pypi, NPM_PUBLISH: opt.npm },
    classification: classify(payload, heads[kind], eventName),
  };
}
function runs(all, s) {
  return Object.entries(all)
    .filter(([, w]) => triggered(w, s))
    .map(([workflow, w]) => ({
      workflow,
      event: s.eventName,
      head: s.head,
      tag: s.event.release?.tag_name ?? null,
      jobs: runGraph(w, s),
    }));
}
function automaticFanout(all, initial, ancestry = []) {
  return runs(all, initial).flatMap((run) => {
    if (ancestry.includes(run.workflow))
      throw new Error(`cyclic automatic workflow fan-out: ${run.workflow}`);
    const downstream = {
      ...initial,
      eventName: "workflow_run",
      event: {
        action: "completed",
        workflow_run: {
          name: all[run.workflow].name,
          head_sha: initial.head,
          conclusion: "success",
        },
      },
      classification: { isReleaseOnly: false, macosRequired: false, reason: "non-PR event" },
      inputs: {},
    };
    return [run, ...automaticFanout(all, downstream, [...ancestry, run.workflow])];
  });
}
test("actual workflow graph routes all events, preserves contexts and records lifecycle telemetry", () => {
  const all = workflows();
  const scenarios = [];
  for (const [id, eventName, action] of classes)
    for (const kind of ["source", "version", "dependency"])
      for (const fork of [false, true])
        for (const pypi of ["false", "true"])
          for (const npm of ["false", "true"]) {
            const s = scenario(id, eventName, action, kind, fork, { pypi, npm });
            const resolved = runs(all, s);
            const mac = resolved.flatMap((r) =>
              r.jobs
                .filter((j) => j.selected && j.macos)
                .map((j) => ({ workflow: r.workflow, ...j })),
            );
            if (eventName === "pull_request") {
              assert.deepEqual(
                mac.map((j) => j.job),
                s.classification.macosRequired ? ["macos"] : [],
              );
              for (const name of required)
                assert.equal(
                  resolved.flatMap((r) => r.jobs).filter((j) => j.name === name).length,
                  1,
                  `${id}: required context ${name}`,
                );
            } else if (eventName === "release")
              assert.deepEqual(
                mac.map((j) => j.job),
                ["darwin"],
              );
            else if (id === "manual-opt-in")
              assert.deepEqual(
                mac.map((j) => j.job),
                ["macos-diagnostic"],
              );
            else assert.deepEqual(mac, [], `${id} never requests Mac automatically`);
            scenarios.push({
              id: `${id}/${kind}/${fork ? "fork" : "same"}/pypi-${pypi}/npm-${npm}`,
              runs: resolved,
            });
          }
  // An ordinary source change -> merge -> release-only PR -> merge -> Release -> smoke.
  // Each event fans out into the actual workflows whose trigger accepts it.
  const lifecycles = [];
  for (const fork of [false, true])
    for (const pypi of ["false", "true"])
      for (const npm of ["false", "true"]) {
        const opt = { pypi, npm };
        const events = [
          scenario("pr-synchronize", "pull_request", "synchronize", "source", fork, opt, "feature"),
          scenario("base-push", "push", null, "source", false, opt),
          scenario("pr-opened", "pull_request", "opened", "version", false, opt),
          scenario("release-pr-merge", "push", null, "version", false, opt),
          scenario("release-tag", "push", null, "version", false, opt),
          scenario("release-published", "release", "published", "version", false, opt),
        ];
        lifecycles.push({
          id: `${fork ? "fork" : "same"}/pypi-${pypi}/npm-${npm}`,
          runs: events.flatMap((s) => automaticFanout(all, s)),
        });
        lifecycles.push({
          id: `${fork ? "fork" : "same"}/pypi-${pypi}/npm-${npm}/without-release-pr`,
          runs: events
            .filter((s) => !["pr-opened", "release-pr-merge"].includes(s.id))
            .flatMap((s) => automaticFanout(all, s)),
        });
      }
  mkdirSync(".github/.telemetry", { recursive: true });
  writeFileSync(
    ".github/.telemetry/macos-policy.json",
    `${JSON.stringify(
      {
        schema_version: 1,
        source_digest: digest(),
        event_classes: classes.map((c) => c[0]),
        scenarios,
        lifecycles,
      },
      null,
      2,
    )}\n`,
  );
});
test("expression and matrix resolution fail clearly for unsupported routing and calls", () => {
  assert.throws(() => evaluate("unknown.field", {}), /Unrecognized named-value|unknown/i);
  assert.throws(
    () =>
      runGraph(
        { jobs: { bad: { "runs-on": `\${{ inputs.missing }}` } } },
        {
          eventName: "push",
          event: {},
          ref: "refs/heads/main",
          classification: { macosRequired: false },
        },
      ),
    /unresolved runner/,
  );
  assert.throws(
    () =>
      runGraph(
        { jobs: { bad: { uses: "unknown/workflow@v1" } } },
        { eventName: "push", event: {}, ref: "refs/heads/main" },
      ),
    /unresolved reusable/,
  );
  assert.deepEqual(
    matrix(
      {
        os: ["ubuntu-latest", "windows-latest"],
        exclude: [{ os: "windows-latest" }],
        include: [{ os: "macos-latest" }],
      },
      {},
    ),
    [{ os: "ubuntu-latest" }, { os: "macos-latest" }],
  );
});
function driveCarrier(proof, changes, macos, required, crate = "true") {
  const job = workflows()["ci.yml"].jobs[`${proof}-macos`];
  assert.equal(job.if, "always()");
  const step = job.steps.find((step) => step.run);
  const context = {
    needs: {
      changes: { result: changes, outputs: { macos: required, crate } },
      macos: { result: macos },
    },
  };
  const env = Object.fromEntries(
    Object.entries(step.env).map(([key, expression]) => [
      key,
      expression.includes("${{") ? String(evaluate(expression, context)) : expression,
    ]),
  );
  return spawnSync("bash", ["-eo", "pipefail", "-c", step.run], {
    encoding: "utf8",
    env: { ...process.env, ...env },
  });
}
test("real carrier command refuses every failed prerequisite and unauthorized omission", () => {
  for (const proof of ["cross", "install", "install-documented"])
    for (const changes of ["success", "failure", "cancelled", "skipped", ""])
      for (const macos of ["success", "failure", "cancelled", "skipped", ""])
        for (const required of ["true", "false", ""]) {
          const result = driveCarrier(proof, changes, macos, required);
          const accepted =
            changes === "success" &&
            ((required === "true" && macos === "success") ||
              (required === "false" && macos === "skipped"));
          assert.equal(
            result.status === 0,
            accepted,
            `${proof}/${changes}/${macos}/${required}: ${result.stderr}`,
          );
          if (accepted && required === "false") assert.match(result.stdout, /no Mac tests claimed/);
        }
  for (const crate of ["false", ""]) {
    const r = driveCarrier("cross", "success", "success", "true", crate);
    assert.equal(r.status === 0, crate === "false");
  }
});

test("unknown PR metadata and ordinary version edits route through the actual Mac lane", () => {
  const all = workflows();
  for (const payload of [{}, event(false, "feature"), event(true)]) {
    const s = {
      eventName: "pull_request",
      event: payload,
      ref: "refs/heads/main",
      head: heads.version,
      classification: classify(payload, heads.version),
      vars: {},
    };
    const jobs = runGraph(all["ci.yml"], s);
    assert.deepEqual(
      jobs.filter((j) => j.selected && j.macos).map((j) => j.job),
      ["macos"],
    );
  }
});

test("budget readers report existing telemetry and refuse missing or stale records", () => {
  const telemetry = JSON.parse(readFileSync(".github/.telemetry/macos-policy.json", "utf8"));
  for (const metric of ["jobs-per-run", "runs-per-change"]) {
    const output = join(scratch, `${metric}.json`);
    const result = spawnSync(
      "node",
      [
        ".github/macos-budget.mjs",
        "--metric",
        metric,
        "--telemetry",
        ".github/.telemetry/macos-policy.json",
      ],
      {
        encoding: "utf8",
        env: { ...process.env, ONEBUDGETSPEC_RESULT: output },
      },
    );
    assert.equal(result.status, 0, result.stderr);
    const reported = JSON.parse(readFileSync(output, "utf8"));
    const expected =
      metric === "jobs-per-run"
        ? Math.max(
            ...telemetry.scenarios.flatMap((s) =>
              s.runs.map((r) => r.jobs.filter((j) => j.selected && j.macos).length),
            ),
          )
        : Math.max(
            ...telemetry.lifecycles.map(
              (l) => l.runs.filter((r) => r.jobs.some((j) => j.selected && j.macos)).length,
            ),
          );
    assert.equal(reported.value, expected);
    assert.match(reported.detail, /macos|darwin/);
  }
  const stale = join(scratch, "stale.json");
  writeFileSync(stale, JSON.stringify({ ...telemetry, source_digest: "stale" }));
  for (const path of [join(scratch, "missing.json"), stale]) {
    const result = spawnSync(
      "node",
      [".github/macos-budget.mjs", "--metric", "jobs-per-run", "--telemetry", path],
      { encoding: "utf8" },
    );
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /ENOENT|stale/);
  }
});

test("actual carriers remain reachable through prerequisite failures and path-sensitive skips", () => {
  const ci = workflows()["ci.yml"];
  const s = scenario(
    "pr-synchronize",
    "pull_request",
    "synchronize",
    "source",
    false,
    { pypi: "false", npm: "false" },
    "feature",
  );
  const skipped = runGraph(ci, { ...s, crate: "false" });
  assert.equal(skipped.find((j) => j.job === "cross").selected, false);
  assert.equal(skipped.find((j) => j.job === "macos").selected, true);
  for (const result of ["failure", "cancelled", "skipped"]) {
    const rows = runGraph(ci, { ...s, results: { changes: result } });
    assert.equal(rows.find((j) => j.job === "macos").selected, false);
    for (const proof of ["cross", "install", "install-documented"]) {
      assert.equal(rows.find((j) => j.job === `${proof}-macos`).selected, true);
      assert.notEqual(driveCarrier(proof, result, "skipped", "").status, 0);
    }
  }
});

test("semantic manifest comparison rejects dependency, lock content and malformed version edits", () => {
  for (const [path, contents] of [
    ["package.json", '{"version":"1.2.4","dependencies":{"sibling":"0.2"}}'],
    ["package.json", '{"version":{"untrusted":true},"dependencies":{"sibling":"0.1"}}'],
    ["Cargo.toml", '[package]\nname="onepipeline"\nversion="1.2.4"\nmalformed = ['],
    [
      "Cargo.lock",
      'version=4\n[[package]]\nname="onepipeline"\nversion="1.2.4"\n[[package]]\nname="sibling"\nversion="0.2.0"\nsource="registry+https://example.com"\n',
    ],
  ]) {
    git("checkout", "-q", "--detach", base);
    writeFileSync(join(repo, path), contents);
    git("add", ".");
    git("commit", "-qm", "adversarial release metadata");
    assert.equal(classify(event(), git("rev-parse", "HEAD")).macosRequired, true);
  }
});

test("release artifact downloads use this run's exact tagged builders and successful Darwin proof", () => {
  const release = workflows()["release.yml"];
  const produced = [];
  const ancestors = (key) => {
    const need = release.jobs[key].needs;
    const direct = need ? (Array.isArray(need) ? need : [need]) : [];
    return [...direct, ...direct.flatMap(ancestors)];
  };
  for (const [key, job] of Object.entries(release.jobs)) {
    for (const leg of matrix(job.strategy?.matrix, {})) {
      for (const step of job.steps ?? []) {
        if (step.uses === "actions/checkout@v4") {
          assert.ok(
            !step.with?.ref || step.with.ref === `\${{ github.sha }}`,
            `${key} must build the event's tagged tree`,
          );
        }
        if (step.uses === "actions/upload-artifact@v4") {
          const expression = step.with.name;
          const name = expression.replace(/\$\{\{(.*?)\}\}/g, (_, e) =>
            String(evaluate(e, { matrix: leg })),
          );
          produced.push({ key, name });
        }
      }
    }
  }
  for (const [key, job] of Object.entries(release.jobs)) {
    for (const step of job.steps ?? [])
      if (step.uses === "actions/download-artifact@v4") {
        assert.equal(step.with["run-id"], undefined, `${key} must consume its own workflow run`);
        const name = step.with.name ?? step.with.pattern;
        const pattern = new RegExp(`^${name.replaceAll("*", ".*")}$`);
        const inputs = produced.filter((p) => pattern.test(p.name));
        assert.ok(inputs.length, `${key} has unresolved artifact ${name}`);
        for (const input of inputs)
          assert.ok(ancestors(key).includes(input.key), `${key} must wait for ${input.key}`);
      }
  }
  for (const key of ["publish-crate", "publish-pypi", "publish-npm", "upload", "upload-darwin"]) {
    assert.ok(
      ancestors(key).includes("darwin"),
      `${key} cannot publish before Darwin installs pass`,
    );
  }
});

test("the actual CI classification step writes the omission-controlling output", () => {
  const step = workflows()["ci.yml"].jobs.changes.steps.find((step) => step.id === "macos");
  const tooling = join(repo, ".github");
  mkdirSync(tooling, { recursive: true });
  writeFileSync(join(tooling, "macos-policy.mjs"), readFileSync(".github/macos-policy.mjs"));
  for (const [payload, head, eventName, want] of [
    [event(), heads.source, "pull_request", "true"],
    [event(), heads.version, "pull_request", "false"],
    [event(), heads.dependency, "pull_request", "true"],
    [event(true), heads.version, "pull_request", "true"],
    ["malformed", heads.version, "pull_request", "true"],
    [{}, heads.source, "push", "false"],
  ]) {
    const input =
      typeof payload === "string"
        ? payload
        : {
            ...payload,
            before: base,
            pull_request: payload.pull_request
              ? {
                  ...payload.pull_request,
                  base: { ...payload.pull_request.base, sha: base },
                  head: { ...payload.pull_request.head, sha: head },
                }
              : undefined,
          };
    writeFileSync(eventPath, typeof input === "string" ? input : JSON.stringify(input));
    const output = join(scratch, "github-output");
    writeFileSync(output, "");
    const github = { event: typeof input === "string" ? {} : input, sha: head };
    const env = Object.fromEntries(
      Object.entries(step.env).map(([key, expression]) => [
        key,
        String(evaluate(expression, { github })),
      ]),
    );
    const result = spawnSync("bash", ["-eo", "pipefail", "-c", step.run], {
      cwd: repo,
      encoding: "utf8",
      env: {
        ...process.env,
        ...env,
        GITHUB_EVENT_PATH: eventPath,
        GITHUB_EVENT_NAME: eventName,
        GITHUB_OUTPUT: output,
      },
    });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(readFileSync(output, "utf8"), `required=${want}\n`);
  }
});

test("automatic workflow_run fan-out cannot hide an extra Mac-spending run", () => {
  const all = workflows();
  const s = scenario(
    "pr-synchronize",
    "pull_request",
    "synchronize",
    "source",
    false,
    { pypi: "false", npm: "false" },
    "feature",
  );
  const mutated = structuredClone(all);
  mutated["regression.yml"] = {
    name: "Regression",
    on: { workflow_run: { workflows: [all["ci.yml"].name], types: ["completed"] } },
    jobs: { regression: { "runs-on": "macos-latest" } },
  };
  const actual = automaticFanout(mutated, s);
  assert.ok(
    actual.some(
      (run) =>
        run.workflow === "regression.yml" && run.jobs.some((job) => job.selected && job.macos),
    ),
  );
  for (const eventName of [
    "pull_request",
    "push",
    "workflow_run",
    "schedule",
    "repository_dispatch",
  ]) {
    const ordinary = { ...s, eventName, inputs: { diagnostic: true } };
    const rows = runGraph(all["ci.yml"], ordinary);
    assert.equal(rows.find((row) => row.job === "macos-diagnostic").selected, false);
  }
});

test("registered budgets run through the existing pinned checker and manifest-directory routing", () => {
  const result = spawnSync("bash", [".github/check-budgets.sh"], { encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /onepipeline-macos-jobs-per-run/);
  assert.match(result.stdout, /onepipeline-macos-runs-per-change/);
});
