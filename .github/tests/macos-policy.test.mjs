import {
  eventClasses as classes,
  diffClasses,
  ownership,
  publishFlags,
  publicationId,
  scenarioId,
} from "../workload.mjs";
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
  'version=4\n[[package]]\nname="onepipeline"\nversion="1.2.3"\n[[package]]\nname="onepipeline"\nversion="0.8.0"\nsource="registry+https://example.com"\n[[package]]\nname="sibling"\nversion="0.1.0"\nsource="registry+https://example.com"\n',
);
writeFileSync(join(repo, "CHANGELOG.md"), "# Changelog\n");
writeFileSync(join(repo, "package.json"), ' {"version":"1.2.3","dependencies":{"sibling":"0.1"}}');
writeFileSync(
  join(repo, "pyproject.toml"),
  '[project]\nname="onepipeline-cli"\nversion="1.2.3"\ndescription="unchanged"\n',
);
writeFileSync(
  join(repo, "package-lock.json"),
  JSON.stringify({
    name: "onepipeline",
    version: "1.2.3",
    lockfileVersion: 3,
    packages: {
      "": { version: "1.2.3" },
      "node_modules/sibling": { version: "0.1.0", integrity: "unchanged" },
    },
  }),
);
mkdirSync(join(repo, "npm/onepipeline-cli"), { recursive: true });
writeFileSync(
  join(repo, "npm/onepipeline-cli/package.json"),
  JSON.stringify({
    name: "onepipeline-cli",
    version: "1.2.3",
    bin: { onepipeline: "bin/onepipeline.js" },
  }),
);
writeFileSync(join(repo, "source.rs"), "fn main() {}\n");
git("add", ".");
git("commit", "-qm", "fixture");
const base = git("rev-parse", "HEAD");
const heads = {};
for (const kind of diffClasses) {
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
const classifications = new Map();
function classify(payload, head, eventName = "pull_request") {
  const key = JSON.stringify([payload, head, eventName]);
  if (classifications.has(key)) return classifications.get(key);
  writeFileSync(eventPath, typeof payload === "string" ? payload : JSON.stringify(payload));
  const result = JSON.parse(
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
  classifications.set(key, result);
  return result;
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
    [
      {
        repository: { full_name: 42 },
        pull_request: { head: { ref: "release-plz-fixture", repo: { full_name: 42 } } },
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
test("the real classifier keeps known non-PR events Mac-free and fails closed on unknown ones", () => {
  const known = [...new Set(classes.map(([, eventName]) => eventName))].filter(
    (name) => name !== "pull_request",
  );
  assert.deepEqual(known.sort(), [
    "push",
    "release",
    "repository_dispatch",
    "schedule",
    "workflow_dispatch",
    "workflow_run",
  ]);
  for (const eventName of known)
    for (const head of Object.values(heads)) {
      const result = classify(event(), head, eventName);
      assert.equal(result.macosRequired, false, eventName);
      assert.match(result.reason, new RegExp(`non-PR event: ${eventName}$`));
    }
  for (const eventName of ["merge_group", "pull_request_target", "issue_comment"]) {
    const result = classify(event(), heads.version, eventName);
    assert.equal(result.isReleaseOnly, false);
    assert.equal(result.macosRequired, true, eventName);
    assert.equal(result.reason, `unrecognised event: ${eventName}`);
  }
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
    cratePublish: opt.crate ?? "true",
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
      classification: classify({}, initial.head, "workflow_run"),
      inputs: {},
    };
    return [run, ...automaticFanout(all, downstream, [...ancestry, run.workflow])];
  });
}
test("actual workflow graph routes all events, preserves contexts and records lifecycle telemetry", () => {
  const sourceDigest = digest();
  const all = workflows();
  const scenarios = [];
  for (const [id, eventName, action] of classes)
    for (const kind of diffClasses)
      for (const fork of ownership)
        for (const pypi of publishFlags)
          for (const npm of publishFlags)
            for (const crate of publishFlags) {
              const s = scenario(id, eventName, action, kind, fork, { pypi, npm, crate });
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
                id: scenarioId(id, kind, fork, { pypi, npm, crate }),
                runs: resolved,
              });
            }
  // An ordinary source change -> merge -> release-only PR -> merge -> Release -> smoke.
  // Each event fans out into the actual workflows whose trigger accepts it.
  const lifecycles = [];
  for (const fork of ownership)
    for (const pypi of publishFlags)
      for (const npm of publishFlags)
        for (const crate of publishFlags) {
          const opt = { pypi, npm, crate };
          const events = [
            scenario(
              "pr-synchronize",
              "pull_request",
              "synchronize",
              "source",
              fork,
              opt,
              "feature",
            ),
            scenario("base-push", "push", null, "source", false, opt),
            scenario("pr-opened", "pull_request", "opened", "version", false, opt),
            scenario("release-pr-merge", "push", null, "version", false, opt),
            scenario("release-tag", "push", null, "version", false, opt),
            scenario("release-published", "release", "published", "version", false, opt),
          ];
          lifecycles.push({
            id: publicationId(fork, opt),
            runs: events.flatMap((s) => automaticFanout(all, s)),
          });
          lifecycles.push({
            id: `${publicationId(fork, opt)}/without-release-pr`,
            runs: events
              .filter((s) => !["pr-opened", "release-pr-merge"].includes(s.id))
              .flatMap((s) => automaticFanout(all, s)),
          });
        }
  assert.equal(
    digest(),
    sourceDigest,
    "workflow source changed during measurement; rerun on a frozen tree",
  );
  mkdirSync(".github/.telemetry", { recursive: true });
  writeFileSync(
    ".github/.telemetry/macos-policy.json",
    `${JSON.stringify(
      {
        schema_version: 1,
        source_digest: sourceDigest,
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
        { on: "push", jobs: { bad: { "runs-on": `\${{ inputs.missing }}` } } },
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
        { on: "push", jobs: { bad: { uses: "unknown/workflow@v1" } } },
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
// The contexts a run reports: every selected row under its rendered name, and a
// job skipped outright under its own name only when it has no matrix — GitHub
// reports a skipped matrix job under its bare key, never under a leg's name.
function emittedContexts(workflow, rows) {
  return rows
    .filter((row) => row.selected || !workflow.jobs[row.job]?.strategy?.matrix)
    .map((row) => ({ name: row.name, selected: row.selected }));
}
test("an ordinary pull request emits every required context, never skipped, whatever its jobs conclude", () => {
  const all = workflows();
  const opt = { pypi: "true", npm: "true", crate: "true" };
  for (const results of [
    {},
    { gate: "failure" },
    { macos: "failure" },
    { cross: "failure" },
    { gate: "failure", macos: "failure", cross: "failure" },
  ])
    for (const [kind, branch] of [
      ["source", "feature"],
      ["dependency", "feature"],
      ["version", "release-plz-fixture"],
    ]) {
      const s = {
        ...scenario("pr-synchronize", "pull_request", "synchronize", kind, false, opt, branch),
        results,
      };
      const contexts = runs(all, s).flatMap((run) =>
        emittedContexts(all[run.workflow], run.jobs),
      );
      for (const name of required) {
        const emitted = contexts.filter((context) => context.name === name);
        assert.equal(emitted.length, 1, `${kind}/${JSON.stringify(results)}: ${name} reports once`);
        assert.equal(
          emitted[0].selected,
          true,
          `${kind}/${JSON.stringify(results)}: ${name} runs rather than being skipped`,
        );
      }
    }
});
test("the real install prerequisite step fails every leg whose gate did not succeed", () => {
  const job = workflows()["ci.yml"].jobs.install;
  const step = job.steps[0];
  assert.equal(step.env.GATE_RESULT, "${{ needs.gate.result }}");
  for (const gate of ["success", "failure", "cancelled", "skipped", ""]) {
    const result = spawnSync("bash", ["-eo", "pipefail", "-c", step.run], {
      encoding: "utf8",
      env: {
        ...process.env,
        GATE_RESULT: String(evaluate(step.env.GATE_RESULT, { needs: { gate: { result: gate } } })),
      },
    });
    assert.equal(result.status === 0, gate === "success", `${gate}: ${result.stderr}`);
    if (gate !== "success") assert.match(result.stderr, /install is not proven/);
  }
});
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
  writeFileSync(stale, JSON.stringify({ ...telemetry, source_digest: "0".repeat(64) }));
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

test("same-named registry dependency version changes require Mac coverage", () => {
  git("checkout", "-q", "--detach", base);
  writeFileSync(
    join(repo, "Cargo.lock"),
    readFileSync(join(repo, "Cargo.lock"), "utf8")
      .replace('version="1.2.3"', 'version="1.2.4"')
      .replace('version="0.8.0"', 'version="0.8.1"'),
  );
  git("add", "Cargo.lock");
  git("commit", "-qm", "local release and same-named registry dependency update");
  const result = classify(event(), git("rev-parse", "HEAD"));
  assert.equal(result.isReleaseOnly, false);
  assert.equal(result.macosRequired, true);
  assert.equal(result.reason, "non-version content: Cargo.lock");
  assert.equal(classify(event(), heads.version).isReleaseOnly, true);
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
        // Nx sets FORCE_COLOR for the tasks the CI gate runs; the step must
        // still write a plain boolean then.
        FORCE_COLOR: "1",
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

test("deleting version metadata is unreadable release input and requires the Mac lane", () => {
  git("checkout", "-q", "--detach", base);
  writeFileSync(
    join(repo, "Cargo.toml"),
    readFileSync(join(repo, "Cargo.toml"), "utf8").replace('version="1.2.3"\n', ""),
  );
  git("add", "Cargo.toml");
  git("commit", "-qm", "missing version metadata");
  assert.equal(classify(event(), git("rev-parse", "HEAD")).macosRequired, true);
});

test("package-lock and Python metadata admit only existing version-field edits", () => {
  const lock = {
    name: "onepipeline",
    version: "1.2.4",
    lockfileVersion: 3,
    packages: {
      "": { version: "1.2.4" },
      "node_modules/sibling": { version: "0.1.0", integrity: "unchanged" },
    },
  };
  const cases = [
    ["package-lock.json", JSON.stringify(lock), false],
    [
      "package-lock.json",
      JSON.stringify({
        ...lock,
        packages: {
          ...lock.packages,
          "node_modules/sibling": { version: "0.2.0", integrity: "unchanged" },
        },
      }),
      true,
    ],
    [
      "package-lock.json",
      JSON.stringify({ ...lock, packages: { ...lock.packages, "": {} } }),
      true,
    ],
    [
      "pyproject.toml",
      '[project]\nname="onepipeline-cli"\nversion="1.2.4"\ndescription="unchanged"\n',
      false,
    ],
    [
      "pyproject.toml",
      '[project]\nname="onepipeline-cli"\nversion="1.2.4"\ndescription="changed"\n',
      true,
    ],
    ["pyproject.toml", '[project]\nname="onepipeline-cli"\ndescription="unchanged"\n', true],
  ];
  for (const [path, contents, required] of cases) {
    git("checkout", "-q", "--detach", base);
    writeFileSync(join(repo, path), contents);
    git("add", path);
    git("commit", "-qm", "metadata boundary fixture");
    assert.equal(classify(event(), git("rev-parse", "HEAD")).macosRequired, required);
  }
});

test("budget readers reject missing, mistyped and inconsistent nested job flags", () => {
  const record = JSON.parse(readFileSync(".github/.telemetry/macos-policy.json", "utf8"));
  for (const mutate of [
    (job) => {
      delete job.selected;
    },
    (job) => {
      job.macos = "false";
    },
    (job) => {
      job.macos = !job.macos;
    },
    (job) => {
      job.unknown = true;
    },
  ]) {
    const bad = structuredClone(record);
    mutate(bad.scenarios.find((s) => s.runs.length).runs[0].jobs[0]);
    const path = join(scratch, "invalid-flags.json");
    writeFileSync(path, JSON.stringify(bad));
    const result = spawnSync(
      "node",
      [".github/macos-budget.mjs", "--metric", "jobs-per-run", "--telemetry", path],
      {
        encoding: "utf8",
        env: { ...process.env, ONEBUDGETSPEC_RESULT: join(scratch, "invalid-result.json") },
      },
    );
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /invalid telemetry|disagrees/);
  }
});

test("actual producer commands govern credential guards and architecture overrides", () => {
  const release = workflows()["release.yml"];
  const s = scenario("release-published", "release", "published", "version", false, {
    pypi: "false",
    npm: "false",
    crate: "false",
  });
  assert.equal(runGraph(release, s).find((j) => j.job === "publish-crate").selected, false);
  assert.equal(
    runGraph(release, { ...s, cratePublish: "true" }).find((j) => j.job === "publish-crate")
      .selected,
    true,
  );
  const snapshot = JSON.parse(
    readFileSync(".github/fixtures/visual-docs-reusable.origin.json", "utf8"),
  );
  const original = workflows()["visual-docs.yml"];
  const modified = structuredClone(original);
  modified.jobs["visual-docs"].with.arches = '["arm64"]';
  const pr = scenario(
    "pr-opened",
    "pull_request",
    "opened",
    "source",
    false,
    { pypi: "false", npm: "false" },
    "feature",
  );
  const rows = runGraph(modified, pr);
  assert.ok(rows.some((j) => j.job === "visual-docs / capture" && j.runner === "ubuntu-24.04-arm"));
  modified.jobs["visual-docs"].with.projects = '[{"id":"duplicate"},{"id":"duplicate"}]';
  assert.throws(() => runGraph(modified, pr), /projects needs unique/);
  modified.jobs["visual-docs"].with.projects = "[]";
  modified.jobs["visual-docs"].with.arches = '["../../escape"]';
  assert.throws(() => runGraph(modified, pr), /unsupported arches/);
  assert.match(snapshot.blob_sha, /^[a-f0-9]{40}$/);
});

test("pinned reusable snapshots refuse source drift and origin corruption", async () => {
  const { pinnedCallee } = await import("../pinned-callee.mjs");
  const { reusable } = await import("../workflow-graph.mjs");
  const copied = join(scratch, "callee-root");
  mkdirSync(join(copied, ".github/fixtures"), { recursive: true });
  const fixture = ".github/fixtures/visual-docs-reusable.yml";
  const origin = ".github/fixtures/visual-docs-reusable.origin.json";
  writeFileSync(join(copied, fixture), readFileSync(fixture));
  writeFileSync(join(copied, origin), readFileSync(origin));
  assert.equal(pinnedCallee(reusable, copied).name, "Visual docs (reusable)");
  writeFileSync(
    join(copied, fixture),
    readFileSync(fixture, "utf8").replace("runs-on: ubuntu-latest", "runs-on: macos-latest"),
  );
  assert.throws(() => pinnedCallee(reusable, copied), /differs from locked/);
  writeFileSync(join(copied, fixture), readFileSync(fixture));
  const lock = JSON.parse(readFileSync(origin, "utf8"));
  for (const invalid of [
    null,
    [],
    { ...lock, schema_version: 2 },
    { ...lock, reference: "wrong" },
    { ...lock, extra: true },
    { ...lock, source_base64: 42 },
  ]) {
    writeFileSync(join(copied, origin), JSON.stringify(invalid));
    assert.throws(() => pinnedCallee(reusable, copied), /invalid pinned callee origin/);
  }
  for (const encoding of ["not-base64!", `${lock.source_base64}\n`]) {
    writeFileSync(join(copied, origin), JSON.stringify({ ...lock, source_base64: encoding }));
    assert.throws(() => pinnedCallee(reusable, copied), /invalid pinned callee encoding/);
  }
  lock.blob_sha = "0".repeat(40);
  writeFileSync(join(copied, origin), JSON.stringify(lock));
  assert.throws(() => pinnedCallee(reusable, copied), /Git blob mismatch/);
});

test("nested npm version metadata cannot authorize a changed launcher", () => {
  for (const [manifest, required] of [
    [
      { name: "onepipeline-cli", version: "1.2.4", bin: { onepipeline: "bin/onepipeline.js" } },
      false,
    ],
    [{ name: "onepipeline-cli", version: "1.2.4", bin: { onepipeline: "different.js" } }, true],
  ]) {
    git("checkout", "-q", "--detach", base);
    writeFileSync(join(repo, "npm/onepipeline-cli/package.json"), JSON.stringify(manifest));
    git("add", "npm/onepipeline-cli/package.json");
    git("commit", "-qm", "launcher metadata");
    assert.equal(classify(event(), git("rev-parse", "HEAD")).macosRequired, required);
  }
});

test("actual trigger routing rejects unmatched branches, tags, types and unresolved filters", () => {
  const all = workflows();
  const push = scenario("base-push", "push", null, "source", false, {
    pypi: "false",
    npm: "false",
  });
  assert.equal(triggered(all["ci.yml"], { ...push, ref: "refs/heads/other" }), false);
  assert.equal(triggered(all["ci.yml"], { ...push, ref: "refs/tags/v1.2.4" }), false);
  const pr = scenario("pr-opened", "pull_request", "opened", "source", false, {
    pypi: "false",
    npm: "false",
  });
  assert.equal(
    triggered(all["ci.yml"], { ...pr, event: { ...pr.event, action: "closed" } }),
    false,
  );
  const release = scenario("release-published", "release", "published", "version", false, {
    pypi: "false",
    npm: "false",
  });
  assert.equal(
    triggered(all["release.yml"], { ...release, event: { ...release.event, action: "created" } }),
    false,
  );
  const tagged = structuredClone(all["ci.yml"]);
  tagged.on.push = { tags: ["v1.2.4"] };
  assert.equal(triggered(tagged, { ...push, ref: "refs/tags/v1.2.4" }), true);
  assert.equal(triggered(tagged, { ...push, ref: "refs/tags/v1.2.5" }), false);
  assert.equal(triggered(tagged, push), false);
  for (const filter of [
    { paths: ["src/**"] },
    { "branches-ignore": ["main"] },
    { branches: ["main*"] },
    { tags: ["v*"] },
  ]) {
    const bad = structuredClone(all["ci.yml"]);
    bad.on.push = filter;
    assert.throws(() => triggered(bad, push), /unsupported trigger/);
  }
});

test("successive additional matrix inclusions remain separate jobs", () => {
  assert.deepEqual(
    matrix(
      {
        os: ["ubuntu-latest"],
        include: [
          { os: "macos-latest", arch: "arm64" },
          { os: "macos-latest", arch: "x64" },
        ],
      },
      {},
    ),
    [
      { os: "ubuntu-latest" },
      { os: "macos-latest", arch: "arm64" },
      { os: "macos-latest", arch: "x64" },
    ],
  );
});

test("matrix expansion follows GitHub's documented original-combination example", () => {
  // https://docs.github.com/en/actions/how-tos/write-workflows/choose-what-workflows-do/run-job-variations#expanding-or-adding-matrix-configurations
  assert.deepEqual(
    matrix(
      {
        fruit: ["apple", "pear"],
        animal: ["cat", "dog"],
        include: [
          { color: "green" },
          { color: "pink", animal: "cat" },
          { fruit: "apple", shape: "circle" },
          { fruit: "banana" },
          { fruit: "banana", animal: "cat" },
        ],
      },
      {},
    ),
    [
      { fruit: "apple", animal: "cat", color: "pink", shape: "circle" },
      { fruit: "apple", animal: "dog", color: "green", shape: "circle" },
      { fruit: "pear", animal: "cat", color: "pink" },
      { fruit: "pear", animal: "dog", color: "green" },
      { fruit: "banana" },
      { fruit: "banana", animal: "cat" },
    ],
  );
});

test("unrecognized workflow/job/step fields are refused before budget interpretation", () => {
  const ci = workflows()["ci.yml"];
  const s = scenario("pr-opened", "pull_request", "opened", "source", false, {
    pypi: "false",
    npm: "false",
  });
  for (const mutate of [
    (w) => {
      w.unsupported = true;
    },
    (w) => {
      w.jobs.gate.unsupported = true;
    },
    (w) => {
      w.jobs.gate.steps[0].unsupported = true;
    },
  ]) {
    const bad = structuredClone(ci);
    mutate(bad);
    assert.throws(() => runGraph(bad, s), /unsupported workflow field/);
  }
});

test("policy CLI rejects incomplete, duplicate and extra argument invocations", () => {
  for (const args of [
    ["classify"],
    ["classify", "--event", eventPath, "--base", base],
    [
      "classify",
      "--event",
      eventPath,
      "--event",
      eventPath,
      "--base",
      base,
      "--head",
      heads.version,
    ],
    ["carrier", "unexpected"],
  ]) {
    const result = spawnSync("node", [".github/macos-policy.mjs", ...args], {
      encoding: "utf8",
      env: {
        ...process.env,
        CHANGES_RESULT: "success",
        MACOS_RESULT: "success",
        MACOS_REQUIRED: "true",
        CRATE_AFFECTED: "true",
        PROOF: "cross",
      },
    });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /usage|duplicate/);
    assert.equal(result.stdout, "");
  }
});

test("reusable callers reject unknown, missing required and mistyped inputs", () => {
  const original = workflows()["visual-docs.yml"];
  const s = scenario("pr-opened", "pull_request", "opened", "source", false, {
    pypi: "false",
    npm: "false",
  });
  for (const mutate of [
    (w) => {
      w.jobs["visual-docs"].with.unknown = true;
    },
    (w) => {
      delete w.jobs["visual-docs"].with["capture-command"];
    },
    (w) => {
      w.jobs["visual-docs"].with["gh-pages-maintenance"] = "true";
    },
  ]) {
    const bad = structuredClone(original);
    mutate(bad);
    assert.throws(() => runGraph(bad, s), /reusable input/);
  }
});

test("reporting failures and malformed telemetry collections refuse to report a measurement", () => {
  const record = JSON.parse(readFileSync(".github/.telemetry/macos-policy.json", "utf8"));
  for (const output of [undefined, scratch]) {
    const env = { ...process.env };
    delete env.ONEBUDGETSPEC_RESULT;
    if (output) env.ONEBUDGETSPEC_RESULT = output;
    const result = spawnSync(
      "node",
      [
        ".github/macos-budget.mjs",
        "--metric",
        "jobs-per-run",
        "--telemetry",
        ".github/.telemetry/macos-policy.json",
      ],
      { encoding: "utf8", env },
    );
    assert.equal(result.status, 1);
    assert.match(result.stderr, /ONEBUDGETSPEC_RESULT|EISDIR/);
  }
  for (const mutate of [
    (r) => {
      r.schema_version = 2;
    },
    (r) => {
      r.event_classes = [];
    },
    (r) => {
      r.scenarios = {};
    },
    (r) => {
      r.scenarios[0].id = 42;
    },
    (r) => {
      r.lifecycles[0].runs = [];
    },
    (r) => {
      r.unknown = true;
    },
  ]) {
    const bad = structuredClone(record);
    mutate(bad);
    const path = join(scratch, "bad-collection.json");
    writeFileSync(path, JSON.stringify(bad));
    const result = spawnSync(
      "node",
      [".github/macos-budget.mjs", "--metric", "jobs-per-run", "--telemetry", path],
      { encoding: "utf8" },
    );
    assert.equal(result.status, 1);
    assert.match(result.stderr, /telemetry/);
  }
});

test("the budget wrapper diagnoses a fresh clone whose pinned checker is missing", () => {
  const fresh = join(scratch, "fresh-checker-root");
  mkdirSync(join(fresh, ".github"), { recursive: true });
  writeFileSync(join(fresh, ".github/check-budgets.sh"), readFileSync(".github/check-budgets.sh"));
  writeFileSync(join(fresh, "justfile"), readFileSync("justfile"));
  const result = spawnSync("bash", [join(fresh, ".github/check-budgets.sh")], { encoding: "utf8" });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /pinned onebudgetspec is missing; run just bootstrap/);
});

test("empty release diff and malformed matrix combinations fail closed", () => {
  assert.equal(classify(event(), base).macosRequired, true);
  for (const spec of [
    { include: ["macos-latest"] },
    { include: [null] },
    { include: {} },
    { exclude: [42] },
    { exclude: {} },
  ])
    assert.throws(() => matrix(spec, {}), /invalid matrix|unresolved matrix/);
});

test("workflow boundary refuses malformed trigger and command shapes", () => {
  const valid = {
    on: "push",
    jobs: { proof: { "runs-on": "ubuntu-latest", steps: [{ run: "true" }] } },
  };
  for (const on of [
    42,
    true,
    [],
    [42],
    {},
    { push: 42 },
    { push: [], schedule: [{ cron: "0 0 * * *" }] },
  ]) {
    assert.throws(
      () => runGraph({ ...valid, on }, { eventName: "push", event: {} }),
      /invalid workflow trigger/,
    );
  }
  for (const field of ["run", "uses"])
    for (const value of [42, {}, [], ""]) {
      const bad = structuredClone(valid);
      bad.jobs.proof.steps = [{ [field]: value }];
      assert.throws(() => runGraph(bad, { eventName: "push", event: {} }), /invalid step command/);
    }
});

test("semantic version parsing rejects malformed JSON and TOML versions", () => {
  const cases = [
    ["01.2.3", true],
    ["1.2.3-.", true],
    ["1.2.3+.", true],
    ["1.2.3-a..b", true],
    ["1.2.3-01", true],
    ["1.2.3\n", true],
    ["1.2.4", false],
    ["0.0.0", false],
    ["1.2.4-rc.1+build.00", false],
    ["1.2.4-01x", false],
  ];
  for (const path of ["package.json", "Cargo.toml"])
    for (const [version, required] of cases) {
      git("checkout", "-q", "--detach", base);
      const initial = readFileSync(join(repo, path), "utf8");
      const changed = path.endsWith("json")
        ? JSON.stringify({ ...JSON.parse(initial), version })
        : initial.replace('version="1.2.3"', `version=${JSON.stringify(version)}`);
      writeFileSync(join(repo, path), changed);
      git("add", path);
      git("commit", "-qm", "version syntax");
      assert.equal(
        classify(event(), git("rev-parse", "HEAD")).macosRequired,
        required,
        `${path}: ${JSON.stringify(version)}`,
      );
    }
});

test("classifier and staged artifact validator retain identical SemVer grammar", () => {
  const classifier = readFileSync(".github/macos-policy.mjs", "utf8").match(
    /const versionSource = String.raw`([^`]+)`/,
  )[1];
  const builder = readFileSync("scripts/build-staged-release.py", "utf8").match(
    /SEMVER_PATTERN = r'([^']+)'/,
  )[1];
  assert.equal(classifier, builder);
});

test("scheduled routing requires cron mappings with typed required fields", () => {
  const valid = {
    on: { schedule: [{ cron: "0 4 * * *" }] },
    jobs: { proof: { "runs-on": "ubuntu-latest", steps: [{ run: "true" }] } },
  };
  const event = { eventName: "schedule", event: {} };
  assert.equal(runGraph(valid, event)[0].selected, true);
  for (const schedule of [
    null,
    [],
    [42],
    [null],
    [{}],
    [{ cron: 42 }],
    [{ cron: [] }],
    [{ cron: "" }],
    [{ cron: "0 4" }],
    [{ cron: "0 4 * * *", extra: true }],
  ]) {
    assert.throws(
      () => runGraph({ ...valid, on: { schedule } }, event),
      /invalid.*schedule|unsupported workflow field schedule/,
    );
  }
  for (const on of ["schedule", ["schedule"]])
    assert.throws(() => runGraph({ ...valid, on }, event), /invalid.*schedule/);
});

test("real carrier step rejects an unknown proof identity", () => {
  const step = workflows()["ci.yml"].jobs["cross-macos"].steps.find((step) => step.run);
  const result = spawnSync("bash", ["-eo", "pipefail", "-c", step.run], {
    encoding: "utf8",
    env: {
      ...process.env,
      CHANGES_RESULT: "success",
      MACOS_RESULT: "success",
      MACOS_REQUIRED: "true",
      CRATE_AFFECTED: "true",
      PROOF: "unknown-proof",
    },
  });
  assert.equal(result.status, 1);
  assert.equal(result.stdout, "");
  assert.match(result.stderr, /unknown Mac proof/);
});

test("budget CLI rejects missing, extra, misspelled and unsupported arguments", () => {
  const valid = ["--metric", "jobs-per-run", "--telemetry", ".github/.telemetry/macos-policy.json"];
  for (const args of [
    [],
    valid.slice(0, 2),
    [...valid, "extra"],
    ["--wrong", ...valid.slice(1)],
    ["--metric", "unknown", "--telemetry", valid[3]],
    ["--metric", valid[1], "--wrong", valid[3]],
  ]) {
    const result = spawnSync("node", [".github/macos-budget.mjs", ...args], { encoding: "utf8" });
    assert.equal(result.status, 1);
    assert.equal(result.stdout, "");
    assert.match(result.stderr, /usage: --metric/);
  }
});

test("budget reader refuses omitted, duplicated and unknown workload inventory", () => {
  const original = JSON.parse(readFileSync(".github/.telemetry/macos-policy.json", "utf8"));
  for (const mutate of [
    (r) => r.event_classes.pop(),
    (r) => r.scenarios.pop(),
    (r) => r.lifecycles.pop(),
    (r) => {
      r.scenarios[0] = r.scenarios[1];
    },
    (r) => {
      r.lifecycles[0].id = "unknown";
    },
  ]) {
    const incomplete = structuredClone(original);
    incomplete.source_digest = digest();
    mutate(incomplete);
    const path = join(scratch, "incomplete-inventory.json");
    writeFileSync(path, JSON.stringify(incomplete));
    const result = spawnSync(
      "node",
      [".github/macos-budget.mjs", "--metric", "jobs-per-run", "--telemetry", path],
      {
        encoding: "utf8",
        env: { ...process.env, ONEBUDGETSPEC_RESULT: join(scratch, "incomplete-result.json") },
      },
    );
    assert.equal(result.status, 1);
    assert.match(result.stderr, /incomplete telemetry.*inventory/);
  }
});

test("changes outputs follow actual declarations and producer wiring", () => {
  const ci = workflows()["ci.yml"];
  const s = scenario("pr-opened", "pull_request", "opened", "source", false, {
    pypi: "false",
    npm: "false",
  });
  const renamed = structuredClone(ci);
  renamed.jobs.changes.outputs.apple = renamed.jobs.changes.outputs.macos;
  delete renamed.jobs.changes.outputs.macos;
  assert.equal(runGraph(renamed, s).find((j) => j.job === "macos").selected, false);
  renamed.jobs.macos.if = "needs.changes.outputs.apple == 'true'";
  assert.equal(runGraph(renamed, s).find((j) => j.job === "macos").selected, true);
  const broken = structuredClone(ci);
  const producer = broken.jobs.changes.steps.find((step) => step.id === "macos");
  producer.run = producer.run.replace('echo "required=', 'echo "alternate=');
  assert.throws(() => runGraph(broken, s), /unresolved changes output/);
});

test("malformed, emptied matrices and cyclic job dependencies refuse interpretation", () => {
  for (const spec of [false, 0, "", null])
    assert.throws(() => matrix(spec, {}), /invalid matrix|unresolved matrix/);
  assert.throws(
    () => matrix({ os: ["ubuntu-latest"], exclude: [{ os: "ubuntu-latest" }] }, {}),
    /matrix resolved to no jobs/,
  );
  assert.throws(
    () =>
      runGraph(
        {
          on: "push",
          jobs: {
            a: { "runs-on": "ubuntu-latest", needs: "b" },
            b: { "runs-on": "ubuntu-latest", needs: "a" },
          },
        },
        { eventName: "push", event: {}, ref: "refs/heads/main" },
      ),
    /cyclic job/,
  );
});

test("supplied falsy strategy mappings are rejected", () => {
  for (const strategy of [false, 0, "", null])
    assert.throws(
      () =>
        runGraph(
          { on: "push", jobs: { proof: { "runs-on": "ubuntu-latest", strategy } } },
          { eventName: "push", event: {}, ref: "refs/heads/main" },
        ),
      /invalid workflow mapping/,
    );
});
