import { readFileSync } from "node:fs";
import { join } from "node:path";
import { declarations } from "./source-projection.mjs";
import { calls, functions } from "./rust-syntax.mjs";
import { codeOnly } from "./graph.mjs";

const cache = new Map();
const phases = new Map();
const snippets = new Map();
let argvSlots;
function slots() {
  if (!argvSlots) {
    argvSlots = {};
    for (const fn of functions(readFileSync("tests/e2e/harness.rs", "utf8"))) {
      if (!/^(?:run(?:_|$)|cmd(?:_|$))/.test(fn.name)) continue;
      if (/\bself\.agentgraph_cmd\s*\(/.test(fn.body)) continue;
      const args = calls(fn.signature)[0]?.args ?? [];
      if (!/\bself\b/.test(args[0] ?? "")) continue;
      const index = args.slice(1).findIndex((arg) => /^\s*args\s*:/.test(arg));
      if (index < 0) continue;
      if (Object.hasOwn(argvSlots, fn.name) && argvSlots[fn.name] !== index)
        throw new Error(`ambiguous harness argv position for ${fn.name}`);
      argvSlots[fn.name] = index;
    }
    if (!Object.keys(argvSlots).length)
      throw new Error("harness exposes no argv method signatures");
  }
  return argvSlots;
}
const callCache = new Map();
const parsedCalls = (body) => {
  if (!callCache.has(body)) callCache.set(body, calls(body));
  return callCache.get(body);
};
function argvValue(expression, body) {
  if (!expression) return undefined;
  const literal = /^\s*&?\s*(?:vec!\s*)?\[\s*"([a-z][a-z-]*)"(?:\s*,\s*"([a-z-]+)")?/.exec(
    expression,
  );
  if (literal) return [literal[1], literal[2]];
  const variable = /^\s*&?\s*(\w+)/.exec(expression)?.[1];
  if (!variable) return undefined;
  if (
    new RegExp(
      `\\b${variable}\\s*\\[\\s*0\\s*\\]\\s*=|\\b${variable}\\.(?:insert|remove|drain|clear|splice)\\s*\\(`,
    ).test(body)
  )
    return undefined;
  const assigned = new RegExp(
    `\\blet\\s+(?:mut\\s+)?${variable}\\s*=\\s*(?:vec!\\s*)?\\[\\s*"([a-z][a-z-]*)"(?:\\s*,\\s*"([a-z-]+)")?`,
  ).exec(body);
  return assigned ? [assigned[1], assigned[2]] : undefined;
}

export function commandCalls(body, commands) {
  return parsedCalls(body)
    .filter((call) => call.method && Object.hasOwn(slots(), call.name))
    .flatMap((call) => {
      const value = argvValue(call.args[slots()[call.name]], body);
      const command = value && (commands[`${value[0]} ${value[1]}`] ?? commands[value[0]]);
      return command ? [{ at: call.at, command }] : [];
    });
}

function moduleSource(module) {
  if (!/^[a-z][a-z0-9_]*$/.test(module)) throw new Error(`invalid fixture module ${module}`);
  if (!cache.has(module)) {
    const text = readFileSync(`tests/e2e/${module}.rs`, "utf8");
    const parsed = functions(text);
    const erased = codeOnly(text);
    const impls = [...erased.matchAll(/\bimpl[^{};]*\{/g)].map((match) => {
      let end = match.index + match[0].length;
      let depth = 1;
      while (end < erased.length && depth) {
        if (erased[end] === "{") depth++;
        if (erased[end] === "}") depth--;
        end++;
      }
      let header = match[0].slice(0, -1).replace(/\bwhere[\s\S]*$/, "");
      while (/<[^<>]*>/.test(header)) header = header.replace(/<[^<>]*>/g, "");
      const type = header
        .split(/\bfor\b/)
        .at(-1)
        .replace(/^impl\s*/, "")
        .trim()
        .split("::")
        .at(-1);
      return { start: match.index, end, type };
    });
    const parent = (at) =>
      parsed
        .filter((fn) => fn.start < at && fn.end >= at)
        .sort((a, b) => a.end - a.start - (b.end - b.start))[0];
    const helpers = new Map();
    for (const fn of parsed) {
      fn.key = `${module}::${fn.name}@${fn.start}`;
      fn.parentKey = parent(fn.start)?.key;
      fn.methodOf = impls
        .filter((entry) => entry.start < fn.start && entry.end >= fn.end)
        .sort((a, b) => a.end - a.start - (b.end - b.start))[0]?.type;
      if (!fn.test) {
        if (!helpers.has(fn.name)) helpers.set(fn.name, []);
        helpers.get(fn.name).push(fn);
      }
    }
    for (const fn of parsed.filter((entry) => !entry.test && !entry.methodOf)) {
      fn.parameters =
        parsedCalls(fn.signature)[0]?.args.map((arg) => /^\s*(\w+)\s*:/.exec(arg)?.[1]) ?? [];
      fn.argvRoles = [];
      for (const call of parsedCalls(fn.body))
        if (call.method && Object.hasOwn(slots(), call.name)) {
          const argument = call.args[slots()[call.name]]?.replace(/^&\s*/, "");
          const index = fn.parameters.indexOf(argument);
          if (index >= 0) fn.argvRoles.push(index);
        }
    }
    const imports = [];
    for (const imported of text.matchAll(/use\s+([a-z][a-z0-9_]*)::([^;]+);/g)) {
      if (["std", "crate", "super", "self"].includes(imported[1])) continue;
      const names = imported[2].includes("{")
        ? imported[2]
            .slice(imported[2].indexOf("{") + 1)
            .replaceAll("}", "")
            .split(",")
        : [imported[2]];
      imports.push({
        parentKey: parent(imported.index)?.key,
        root: imported[1],
        bindings: names.map((name) =>
          name
            .trim()
            .split(/\s+as\s+/)
            .at(-1)
            .split("::")
            .at(-1),
        ),
      });
    }
    cache.set(module, {
      text,
      functions: parsed,
      helpers,
      imports,
      constants: declarations(text)
        .filter((entry) => entry.kind === "const")
        .map((entry) => ({
          ...entry,
          parentKey: parsed
            .filter((fn) => fn.start < entry.offset && fn.end >= entry.offset)
            .sort((a, b) => a.end - a.start - (b.end - b.start))[0]?.key,
        })),
      borrowed: [...text.matchAll(/use crate::(\w+)::/g)].map((m) => ({
        module: m[1],
        parentKey: parent(m.index)?.key,
      })),
      strings: new Map(
        [...text.matchAll(/const (\w+): &str =\s*"([^"]+)";/g)].map((m) => [m[1], m[2]]),
      ),
    });
  }
  return cache.get(module);
}

function selectedBody(body, owner, originalCommands, commands) {
  if (!owner.startsWith("cmd-") || originalCommands.length < 2) return body;
  const calls = parsedCalls(body)
    .filter((call) => call.method && Object.hasOwn(slots(), call.name))
    .map((call) => ({ call, value: argvValue(call.args[slots()[call.name]], body) }))
    .filter((entry) => entry.value)
    .map(({ call, value }) => ({
      at: body.lastIndexOf("\n", call.at) + 1,
      owner: commands[`${value[0]} ${value[1]}`] ?? commands[value[0]],
    }))
    .filter((call) => call.owner);
  if (!calls.some((call) => call.owner === owner)) return body;
  const pieces = [];
  if (calls[0]?.owner === owner) pieces.push(body.slice(0, calls[0].at));
  for (const [index, call] of calls.entries())
    if (call.owner === owner) pieces.push(body.slice(call.at, calls[index + 1]?.at ?? body.length));
  return pieces.join("\n");
}

function snippet(module, body, scope, methodOf) {
  const cacheKey = `${module}:${scope}:${methodOf}:${body}`;
  if (snippets.has(cacheKey)) return snippets.get(cacheKey);
  const owner = moduleSource(module);
  const erased = codeOnly(body);
  const words = new Set(erased.match(/\b\w+\b/g));
  const pending = owner.constants
    .filter((entry) => words.has(entry.name))
    .map((entry) => ({ module, body: entry.raw, key: `${module}::$${entry.name}` }));
  const candidates = new Set([
    module,
    "harness",
    ...owner.borrowed
      .filter((entry) => !entry.parentKey || entry.parentKey === scope)
      .map((entry) => entry.module),
  ]);
  for (const match of erased.matchAll(/crate::(\w+)::/g)) candidates.add(match[1]);
  const calls = [...erased.matchAll(/((?:[A-Za-z_]\w*::)*)([A-Za-z_]\w*)\s*(?:!\s*)?\(/g)].map(
    (match) => ({
      at: match.index + match[1].length,
      name: match[2],
      qualifier: match[1].replace(/::$/, ""),
      method: erased[match.index - 1] === ".",
      receiver: /([A-Za-z_]\w*)\s*$/.exec(erased.slice(0, match.index - 1))?.[1],
    }),
  );
  const adapterValues = [];
  for (const candidate of candidates) {
    let data;
    try {
      data = moduleSource(candidate);
    } catch (error) {
      if (error.code === "ENOENT") continue;
      throw error;
    }
    for (const constant of data.constants)
      if ((!constant.parentKey || constant.parentKey === scope) && words.has(constant.name))
        pending.push({
          module: candidate,
          body: constant.raw,
          key: `${candidate}::$${constant.name}`,
          scope,
        });
    for (const call of calls)
      for (const helper of data.helpers.get(call.name) ?? []) {
        if (helper.parentKey && helper.parentKey !== scope) continue;
        if (call.qualifier) {
          const parts = call.qualifier.split("::");
          const type = parts.at(-1) === "Self" ? methodOf : parts.at(-1);
          if (
            ((parts[0] === "crate" && parts[1] === candidate) ||
              (parts.length === 1 && parts[0] === candidate)) &&
            !helper.methodOf
          ) {
            if (call.method) continue;
          } else if (helper.methodOf !== type) continue;
        } else if (call.method) {
          if (!helper.methodOf) continue;
          const type =
            call.receiver === "world" ? "World" : call.receiver === "self" ? methodOf : undefined;
          if (type && helper.methodOf !== type) continue;
        } else if (helper.methodOf) continue;
        const invoked = parsedCalls(body).find(
          (entry) => entry.name === call.name && entry.at === call.at,
        );
        const supplied = (helper.argvRoles ?? []).map((index) =>
          argvValue(invoked?.args[index], body),
        );
        for (const value of supplied.filter(Boolean)) adapterValues.push(value);
        pending.push({
          resolvedArgv: supplied.length > 0 && supplied.every(Boolean),
          module: candidate,
          body: helper.body,
          key: helper.key,
          scope: helper.key,
          methodOf: helper.methodOf,
        });
      }
  }
  const reads = [
    ...[...body.matchAll(/repo_file\(\s*"([^"]+)"/g)].map((m) => m[1]),
    ...[...body.matchAll(/repo_file\(\s*(\w+)\s*\)/g)].map((m) => owner.strings.get(m[1])),
    ...[...body.matchAll(/CARGO_MANIFEST_DIR"\)\)\s*\.join\("([^"]+)"/g)].map((m) => m[1]),
    ...[...body.matchAll(/include_str!\("([^"]+)"/g)].map((m) =>
      join("tests/e2e", m[1]).replaceAll("\\", "/"),
    ),
  ]
    .filter(Boolean)
    .filter((path) => path !== "." && !path.startsWith("target"))
    .flatMap((path) => [path.replace(/\/$/, ""), `${path.replace(/\/$/, "")}/**`]);
  const imports = owner.imports
    .filter((entry) => !entry.parentKey || entry.parentKey === scope)
    .filter((entry) => entry.bindings.some((binding) => words.has(binding) || /Ext$/.test(binding)))
    .map((entry) => `${entry.root}::`);
  const direct = parsedCalls(body).filter(
    (call) => call.method && Object.hasOwn(slots(), call.name),
  );
  const argumentCalls = [
    ...adapterValues,
    ...(/\bbinary\s*\(|CARGO_BIN_EXE_onepipeline/.test(body)
      ? parsedCalls(body)
          .filter((call) => call.name === "args")
          .map((call) => argvValue(call.args[0], body))
          .filter(Boolean)
      : []),
    ...direct.map((call) => argvValue(call.args[slots()[call.name]], body)).filter(Boolean),
  ];
  const bindings = [];
  const publicApi =
    /\bonepipeline::/.test(erased) || imports.some((entry) => entry === "onepipeline::");
  const dynamicCli =
    module !== "harness" && direct.some((call) => !argvValue(call.args[slots()[call.name]], body));
  const result = {
    pending,
    paths: reads,
    argumentCalls,
    bindings,
    publicApi,
    dynamicCli,
    pipeline: /\bbinary\s*\(/.test(erased) || /\benv!\(\s*"CARGO_BIN_EXE_onepipeline"/.test(body),
    external: [
      ...new Set(
        [...erased.matchAll(/\b([a-z][a-z0-9_]*)::/g)]
          .map((match) => match[1])
          .concat(imports.map((root) => root.slice(0, -2))),
      ),
    ],
  };
  snippets.set(cacheKey, result);
  return result;
}

/** Inputs for the future phase's fixture snippets, excluding other tests that share today's source module. */
export function phaseInputs(piece, disposition, commands) {
  const cacheKey = `${piece.original}:${piece.suite}:${disposition.kind}`;
  if (phases.has(cacheKey)) return phases.get(cacheKey);
  const [module, name] = piece.original.split("::");
  const fn = moduleSource(module).functions.find((entry) => entry.test && entry.name === name);
  if (!fn) throw new Error(`missing phase source ${piece.original}`);
  const body =
    disposition.kind === "decompose"
      ? selectedBody(fn.body, piece.suite, disposition.originalCommands, commands)
      : fn.body;
  const pending = [{ module, body, key: piece.original, scope: fn.key, methodOf: fn.methodOf }];
  const reached = new Set();
  const consumers = new Set();
  const paths = new Set([`migrated-tests/${piece.suite}/**`]);
  const external = new Set();
  const commandRoots = new Set();
  let publicApi = false;
  let dynamicCli = false;
  let pipeline = false;
  for (const held of pending) {
    const diagnostic = held.diagnostic || /::dump(?:@|$)/.test(held.key);
    const visitKey = `${held.key}:${diagnostic}:${held.resolvedArgv ?? false}`;
    if (reached.has(visitKey)) continue;
    reached.add(visitKey);
    const analysis = snippet(held.module, held.body, held.scope, held.methodOf);
    for (const root of analysis.external) if (root !== "onepipeline") external.add(root);
    publicApi ||= analysis.publicApi;
    const bound = analysis.bindings
      .map(([parent, child]) => commands[`${parent} ${child}`] ?? commands[parent])
      .filter(Boolean);
    if (!diagnostic) {
      pipeline ||=
        analysis.pipeline ||
        (held.methodOf === "World" &&
          /::(?:cmd|run|run_from|run_in|run_with_stdin|run_recording_renders)@/.test(held.key));
      dynamicCli ||= analysis.dynamicCli && !bound.length && !held.resolvedArgv;
      for (const owner of bound) commandRoots.add(owner);
      for (const [parent, child] of analysis.argumentCalls) {
        const owner = commands[`${parent} ${child}`] ?? commands[parent];
        if (owner) commandRoots.add(owner);
      }
    }
    for (const path of analysis.paths) paths.add(path);
    for (const next of analysis.pending)
      if (!reached.has(`${next.key}:${diagnostic}:${next.resolvedArgv ?? false}`)) {
        pending.push({ ...next, diagnostic });
        consumers.add(next.key);
      }
  }
  const result = {
    paths: [...paths].sort(),
    external: [...external].sort(),
    commands: [...commandRoots].sort(),
    publicApi,
    dynamicCli: dynamicCli && (pipeline || commandRoots.size > 0),
    pipeline,
    helpers: [...new Set([...consumers].map((key) => key.split("@")[0]))].sort(),
  };
  phases.set(cacheKey, result);
  return result;
}
