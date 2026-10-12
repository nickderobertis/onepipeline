import { createHash } from "node:crypto";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { parse } from "yaml";
import { Lexer, Parser, Evaluator, data } from "@actions/expressions";

export const reusable =
  "nickderobertis/screencomp/.github/workflows/visual-docs-reusable.yml@v0.4.8";
export function workflows(root = process.cwd()) {
  return Object.fromEntries(
    readdirSync(join(root, ".github/workflows"))
      .filter((n) => /\.ya?ml$/.test(n))
      .sort()
      .map((n) => [n, parse(readFileSync(join(root, ".github/workflows", n), "utf8"))]),
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
export function matrix(spec, context) {
  if (!spec) return [{}];
  spec = resolve(spec, context);
  if (!spec || typeof spec !== "object" || Array.isArray(spec))
    throw new Error("unresolved matrix");
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
  for (const inc of spec.include ?? []) {
    const compatible = axes.length
      ? legs.filter((leg) => axes.every(([key]) => !(key in inc) || leg[key] === inc[key]))
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
  if (settings?.types && !settings.types.includes(scenario.event.action)) return false;
  if (scenario.eventName === "push") {
    const branch = scenario.ref.replace("refs/heads/", "");
    if (settings?.branches && !settings.branches.includes(branch)) return false;
    if (scenario.ref.startsWith("refs/tags/") && !settings?.tags) return false;
  }
  if (
    scenario.eventName === "workflow_run" &&
    settings?.workflows &&
    !settings.workflows.includes(scenario.event.workflow_run.name)
  )
    return false;
  return true;
}
export function runGraph(workflow, scenario, root = process.cwd(), prefix = "") {
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
      let outputs = {};
      if (key === "changes")
        outputs = {
          crate: scenario.crate ?? "true",
          macos: String(scenario.classification.macosRequired),
        };
      if (key === "guard") outputs = { publish: "true", enabled: "true" };
      if (key === "arches") {
        const config = readFileSync(join(root, "screencomp.toml"), "utf8");
        const arches = JSON.parse(config.match(/arches\s*=\s*(\[[^\]]+\])/)[1]);
        const projects = JSON.parse(context.inputs.projects ?? "[]");
        const actual = projects.length
          ? projects
          : [{ id: "", current: "shots/current", verify: "shots/verify" }];
        outputs.matrix = JSON.stringify({
          include: arches.flatMap((arch) =>
            actual.map((p) => ({
              arch,
              project: p.id,
              current: p.current ?? `shots/current/${p.id}`,
              verify: p.verify ?? `shots/verify/${p.id}`,
              manifest: p.manifest ?? (p.id ? `shots/baseline/${p.id}/${arch}.json` : ""),
              gallery_title: p["gallery-title"] ?? context.inputs["gallery-title"],
            })),
          ),
        });
      }
      const result = scenario.results?.[key] ?? (selected ? "success" : "skipped");
      // Declaration-derived outputs retain matrix inventories on intentional skips;
      // dependency status still prevents those rows from requesting a runner.
      context.needs[key] = { result, outputs };
      if (job.uses) {
        if (job.uses !== reusable) throw new Error(`unresolved reusable workflow ${job.uses}`);
        const nested = parse(
          readFileSync(join(root, ".github/fixtures/visual-docs-reusable.yml"), "utf8"),
        );
        const inputs = Object.fromEntries(
          Object.entries(nested.on.workflow_call.inputs).map(([k, v]) => [k, v.default ?? ""]),
        );
        Object.assign(inputs, resolve(job.with ?? {}, context));
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
