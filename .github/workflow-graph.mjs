import { createHash } from "node:crypto";
import { readFileSync, readdirSync, mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { parse } from "yaml";
import { validateWorkflow } from "./workflow-schema.mjs";
import { pinnedCallee } from "./pinned-callee.mjs";
import { Lexer, Parser, Evaluator, data } from "@actions/expressions";

export const reusable =
  "nickderobertis/screencomp/.github/workflows/visual-docs-reusable.yml@v0.4.8";
export function workflows(root = process.cwd()) {
  return Object.fromEntries(
    readdirSync(join(root, ".github/workflows"))
      .filter((n) => /\.ya?ml$/.test(n))
      .sort()
      .map((n) => [
        n,
        validateWorkflow(parse(readFileSync(join(root, ".github/workflows", n), "utf8"))),
      ]),
  );
}
export function digest(root = process.cwd()) {
  const hash = createHash("sha256");
  function walk(dir) {
    for (const name of readdirSync(join(root, dir), { withFileTypes: true }).sort((a, b) =>
      a.name.localeCompare(b.name),
    )) {
      if (["node_modules", ".telemetry"].includes(name.name)) continue;
      const path = join(dir, name.name);
      if (name.isDirectory()) walk(path);
      else {
        hash.update(path);
        hash.update(readFileSync(join(root, path)));
      }
    }
  }
  walk(".github");
  for (const path of [
    "screencomp.toml",
    "Cargo.toml",
    "release-targets.toml",
    "scripts/build-staged-release.py",
    "scripts/npm-build.mjs",
    "npm/onepipeline-cli/package.json",
  ]) {
    hash.update(path);
    hash.update(readFileSync(join(root, path)));
  }
  return hash.digest("hex");
}
function value(v) {
  if (v === null || v === undefined) return new data.Null();
  if (typeof v === "string") return new data.StringData(v);
  if (typeof v === "boolean") return new data.BooleanData(v);
  if (typeof v === "number") return new data.NumberData(v);
  if (Array.isArray(v)) return new data.Array(...v.map(value));
  return new data.Dictionary(...Object.entries(v).map(([key, v]) => ({ key, value: value(v) })));
}
export function evaluate(expression, context, status = {}) {
  const source = expression.trim().replace(/^\$\{\{\s*|\s*\}\}$/g, "");
  const functions = new Map(
    ["always", "success", "failure", "cancelled"].map((name) => [
      name,
      {
        name,
        minArgs: 0,
        maxArgs: 0,
        call: () => value(name === "always" || (status[name] ?? name === "success")),
      },
    ]),
  );
  const ast = new Parser(new Lexer(source).lex().tokens, Object.keys(context), [
    ...functions.values(),
  ]).parse();
  const result = new Evaluator(ast, value(context), functions).evaluate();
  return JSON.parse(JSON.stringify(result, data.replacer));
}
function resolve(v, context, status) {
  if (typeof v === "string" && v.includes("${{")) {
    if (/^\s*\$\{\{[\s\S]*\}\}\s*$/.test(v) && v.match(/\$\{\{/g)?.length === 1)
      return evaluate(v, context, status);
    return v.replace(/\$\{\{(.*?)\}\}/g, (_, expr) => String(evaluate(expr, context, status)));
  }
  if (Array.isArray(v)) return v.map((x) => resolve(x, context, status));
  if (v && typeof v === "object")
    return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, resolve(x, context, status)]));
  return v;
}
const producerCache = new Map();
function producerOutputs(kind, job, context, root, scenario) {
  const step = job.steps.find((step) => step.id === (kind === "guard" ? "check" : "read"));
  if (!step?.run || !job.outputs) throw new Error(`unresolved producer ${kind}`);
  const secrets = {
    CARGO_REGISTRY_TOKEN: scenario.cratePublish === "false" ? "" : "present",
    RELEASE_PLZ_TOKEN: "present",
  };
  const env = resolve(step.env ?? {}, { ...context, secrets });
  const key = JSON.stringify([
    root,
    step.run,
    env,
    job.outputs,
    kind === "arches" ? readFileSync(join(root, "screencomp.toml"), "utf8") : null,
  ]);
  if (producerCache.has(key)) return producerCache.get(key);
  const scratch = mkdtempSync(
    join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "workflow-producer-"),
  );
  try {
    const output = join(scratch, "output");
    const bin = join(scratch, "bin");
    mkdirSync(bin);
    if (kind === "arches") {
      const arches = JSON.parse(
        execFileSync(
          "python3",
          [
            "-c",
            "import json,tomllib; print(json.dumps(tomllib.load(open('screencomp.toml','rb'))['capture']['arches']))",
          ],
          { cwd: root, encoding: "utf8" },
        ),
      );
      // Only the third-party screencomp CLI is doubled; the pinned workflow's
      // resolver and jq validation run verbatim against the committed config.
      writeFileSync(
        join(bin, "screencomp"),
        `#!/usr/bin/env node\nprocess.stdout.write(${JSON.stringify(JSON.stringify(arches))});\n`,
        { mode: 0o755 },
      );
    }
    execFileSync("bash", ["-eo", "pipefail", "-c", step.run], {
      cwd: root,
      encoding: "utf8",
      env: {
        PATH: `${bin}:${process.env.PATH}`,
        HOME: process.env.HOME,
        ...env,
        GITHUB_OUTPUT: output,
      },
    });
    const values = {};
    for (const line of readFileSync(output, "utf8").trim().split("\n")) {
      const found = line.match(/^([A-Za-z_][A-Za-z0-9_]*)=(.*)$/);
      if (!found || Object.hasOwn(values, found[1]))
        throw new Error(`invalid producer output: ${kind}`);
      values[found[1]] = found[2];
    }
    const outputs = resolve(job.outputs, { ...context, steps: { [step.id]: { outputs: values } } });
    if (
      kind === "guard" &&
      Object.values(outputs).some((value) => !["true", "false"].includes(value))
    )
      throw new Error("invalid guard producer output");
    if (kind === "arches") {
      const combined = JSON.parse(outputs.matrix);
      if (
        !Array.isArray(combined.include) ||
        !combined.include.length ||
        combined.include.some((leg) => !["x86_64", "arm64", "aarch64"].includes(leg.arch))
      )
        throw new Error("unsupported arches producer output");
    }
    producerCache.set(key, outputs);
    return outputs;
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}
export function matrix(spec, context) {
  if (spec === undefined) return [{}];
  spec = resolve(spec, context);
  if (!spec || typeof spec !== "object" || Array.isArray(spec))
    throw new Error("unresolved matrix");
  for (const field of ["include", "exclude"]) {
    if (
      spec[field] !== undefined &&
      (!Array.isArray(spec[field]) ||
        spec[field].some((entry) => !entry || typeof entry !== "object" || Array.isArray(entry)))
    )
      throw new Error(`invalid matrix ${field}: expected mapping objects`);
  }
  let legs = [{}];
  const axes = Object.entries(spec).filter(([key]) => !["include", "exclude"].includes(key));
  for (const [key, choices] of axes) {
    if (!Array.isArray(choices) || !choices.length)
      throw new Error(`unresolved matrix axis ${key}`);
    legs = legs.flatMap((leg) => choices.map((x) => ({ ...leg, [key]: x })));
  }
  legs = legs.filter(
    (leg) => !(spec.exclude ?? []).some((ex) => Object.entries(ex).every(([k, v]) => leg[k] === v)),
  );
  if (!axes.length) legs = [];
  const original = [...legs];
  for (const inc of spec.include ?? []) {
    const compatible = axes.length
      ? original.filter((leg) => axes.every(([key]) => !(key in inc) || leg[key] === inc[key]))
      : [];
    if (compatible.length) for (const leg of compatible) Object.assign(leg, inc);
    else legs.push({ ...inc });
  }
  if (!legs.length) throw new Error("matrix resolved to no jobs");
  return legs;
}
export function triggered(workflow, scenario) {
  const on = workflow.on;
  const trigger = Array.isArray(on)
    ? on.includes(scenario.eventName)
    : typeof on === "string"
      ? on === scenario.eventName
      : Object.hasOwn(on, scenario.eventName);
  if (!trigger) return false;
  const settings = on?.[scenario.eventName];
  if (scenario.eventName === "schedule") return true;
  const allowed = {
    push: ["branches", "tags"],
    pull_request: ["types", "branches"],
    release: ["types"],
    workflow_run: ["types", "workflows"],
    workflow_dispatch: ["inputs"],
    repository_dispatch: ["types"],
  }[scenario.eventName];
  if (!allowed) throw new Error(`unsupported event routing: ${scenario.eventName}`);
  for (const key of Object.keys(settings ?? {})) {
    if (!allowed.includes(key)) throw new Error(`unsupported trigger filter: ${key}`);
    if (key === "inputs") continue;
    if (
      !Array.isArray(settings[key]) ||
      settings[key].some((value) => typeof value !== "string" || !value || /[*?[\]{}!]/.test(value))
    )
      throw new Error(`unsupported trigger pattern: ${key}`);
  }
  if (settings?.types && !settings.types.includes(scenario.event.action)) return false;
  if (scenario.eventName === "pull_request") {
    if (
      !settings?.types &&
      scenario.event.action &&
      !["opened", "synchronize", "reopened"].includes(scenario.event.action)
    )
      return false;
    if (settings?.branches) {
      const branch = scenario.event.pull_request?.base?.ref;
      if (typeof branch !== "string") throw new Error("unresolved PR base branch");
      if (!settings.branches.includes(branch)) return false;
    }
  }
  if (scenario.eventName === "push") {
    const tag = scenario.ref.startsWith("refs/tags/");
    const branch = scenario.ref.startsWith("refs/heads/");
    if (!tag && !branch) throw new Error("unresolved push ref");
    if (tag && settings?.branches && !settings.tags) return false;
    if (branch && settings?.tags && !settings.branches) return false;
    const filter = tag ? settings?.tags : settings?.branches;
    if (filter && !filter.includes(scenario.ref.replace(tag ? "refs/tags/" : "refs/heads/", "")))
      return false;
  }
  if (
    scenario.eventName === "workflow_run" &&
    settings?.workflows &&
    !settings.workflows.includes(scenario.event.workflow_run.name)
  )
    return false;
  return true;
}
function changesOutputs(job, context, scenario) {
  const steps = {};
  for (const step of job.steps ?? []) {
    const affected = step.run?.includes("scripts/nx-affected.sh");
    const classification = step.run?.includes(".github/macos-policy.mjs classify");
    if (!affected && !classification) continue;
    const emitted = [...step.run.matchAll(/echo\s+"([A-Za-z_][A-Za-z0-9_]*)=/g)];
    if (!step.id || emitted.length !== 1) throw new Error("unresolved changes output producer");
    let value = scenario.crate ?? "true";
    if (classification) {
      const field = step.run.match(
        /JSON\.parse\(process\.argv\[1\]\)\.([A-Za-z_][A-Za-z0-9_]*)/,
      )?.[1];
      if (!field || typeof scenario.classification?.[field] !== "boolean")
        throw new Error("unresolved changes output classification");
      value = String(scenario.classification[field]);
    }
    steps[step.id] = { outputs: { [emitted[0][1]]: value } };
  }
  const outputs = resolve(job.outputs ?? {}, { ...context, steps });
  if (Object.values(outputs).some((value) => !["true", "false"].includes(value)))
    throw new Error("unresolved changes output declaration");
  return outputs;
}
export function runGraph(workflow, scenario, root = process.cwd(), prefix = "") {
  validateWorkflow(workflow);
  const github = {
    event_name: scenario.eventName,
    event: scenario.event,
    repository: "nickderobertis/onepipeline",
    ref: scenario.ref,
    head_ref: scenario.event.pull_request?.head.ref ?? "",
    sha: scenario.head,
  };
  const context = { github, inputs: scenario.inputs ?? {}, vars: scenario.vars ?? {}, needs: {} };
  const jobs = workflow.jobs;
  const rows = [];
  const pending = new Set(Object.keys(jobs));
  while (pending.size) {
    let progressed = false;
    for (const key of pending) {
      const job = jobs[key];
      const dependencies = job.needs ? (Array.isArray(job.needs) ? job.needs : [job.needs]) : [];
      if (dependencies.some((n) => pending.has(n))) continue;
      if (dependencies.some((n) => !context.needs[n]))
        throw new Error(`unresolved needs for ${key}`);
      progressed = true;
      pending.delete(key);
      const status = {
        success: dependencies.every((n) => context.needs[n].result === "success"),
        failure: dependencies.some((n) => context.needs[n].result === "failure"),
        cancelled: dependencies.some((n) => context.needs[n].result === "cancelled"),
      };
      const condition = job.if ?? "success()";
      const selected =
        Boolean(evaluate(condition, context, status)) &&
        (/\b(always|failure|cancelled|success)\(/.test(condition) || status.success);
      const result = scenario.results?.[key] ?? (selected ? "success" : "skipped");
      let outputs = {};
      // A producer that did not succeed sets no outputs for its dependents to read.
      if (key === "changes" && result === "success")
        outputs = changesOutputs(job, context, scenario);
      if (["guard", "arches"].includes(key))
        outputs = producerOutputs(key, job, context, root, scenario);
      // Declaration-derived outputs retain matrix inventories on intentional skips;
      // dependency status still prevents those rows from requesting a runner.
      context.needs[key] = { result, outputs };
      if (job.uses) {
        if (job.uses !== reusable) throw new Error(`unresolved reusable workflow ${job.uses}`);
        const nested = pinnedCallee(job.uses, root);
        const inputs = Object.fromEntries(
          Object.entries(nested.on.workflow_call.inputs).map(([k, v]) => [k, v.default ?? ""]),
        );
        const supplied = resolve(job.with ?? {}, context);
        const declarations = nested.on.workflow_call.inputs;
        for (const key of Object.keys(supplied))
          if (!Object.hasOwn(declarations, key)) throw new Error(`unknown reusable input: ${key}`);
        for (const [key, declaration] of Object.entries(declarations)) {
          if (declaration.required && !Object.hasOwn(supplied, key))
            throw new Error(`missing required reusable input: ${key}`);
          const input = Object.hasOwn(supplied, key)
            ? supplied[key]
            : (declaration.default ??
              (declaration.type === "boolean" ? false : declaration.type === "number" ? 0 : ""));
          if (
            typeof input !== declaration.type ||
            (declaration.type === "number" && !Number.isFinite(input))
          )
            throw new Error(`invalid reusable input type: ${key}`);
          inputs[key] = input;
        }
        const children = runGraph(nested, { ...scenario, inputs }, root, `${prefix}${key} / `);
        rows.push(...children.map((row) => ({ ...row, selected: selected && row.selected })));
        continue;
      }
      for (const leg of matrix(job.strategy?.matrix, context)) {
        const ctx = { ...context, matrix: leg };
        const runner = resolve(job["runs-on"], ctx, status);
        const labels = Array.isArray(runner) ? runner : [runner];
        if (
          !labels.length ||
          labels.some(
            (x) =>
              typeof x !== "string" ||
              !x ||
              x.includes("${{") ||
              !/^(ubuntu-|windows-|macos-)/.test(x),
          )
        )
          throw new Error(`unresolved runner for ${prefix}${key}: ${JSON.stringify(runner)}`);
        const name = job.name
          ? resolve(job.name, ctx)
          : key +
            (Object.keys(leg).length
              ? ` (${Object.values(leg)
                  .filter((x) => x !== "")
                  .join(", ")})`
              : "");
        const targets = leg.target
          ? [leg.target]
          : (job.steps ?? []).flatMap((step) =>
              [...(step.run ?? "").matchAll(/build-staged-release.py --target (\S+)/g)].map(
                (match) => match[1],
              ),
            );
        rows.push({
          targets,
          job: `${prefix}${key}`,
          name: `${prefix}${name}`,
          runner,
          selected,
          macos: labels.some((x) => x.startsWith("macos-")),
          target: leg.target ?? leg.arch ?? null,
          needs: dependencies,
        });
      }
    }
    if (!progressed) throw new Error("unresolved/cyclic job graph");
  }
  return rows;
}
