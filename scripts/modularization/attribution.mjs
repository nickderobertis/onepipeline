import { readFileSync } from "node:fs";
import { join } from "node:path";
import { createHash } from "node:crypto";
import { functions, calls } from "./rust-syntax.mjs";

export const sourceHash = (text) => createHash("sha256").update(text).digest("hex");

/** Reviewed exceptions are tied to complete Rust function definitions, not test-area names. */
export function loadAttributions(commands, root = ".") {
  const path = "scripts/modularization/command-attributions.json";
  let audit;
  try {
    audit = JSON.parse(readFileSync(join(root, path), "utf8"));
  } catch (error) {
    if (error.code === "ENOENT") return {};
    throw error;
  }
  if (
    !audit ||
    typeof audit !== "object" ||
    Array.isArray(audit) ||
    audit.schema !== 1 ||
    Object.keys(audit).some((key) => !["schema", "functions"].includes(key)) ||
    !audit.functions ||
    typeof audit.functions !== "object" ||
    Array.isArray(audit.functions)
  )
    throw new Error("invalid command attribution audit");
  const sources = new Map();
  for (const [id, entry] of Object.entries(audit.functions)) {
    if (
      !/^[a-z][a-z0-9_]*::[A-Za-z_]\w*$/.test(id) ||
      !entry ||
      Object.keys(entry).some(
        (key) =>
          !["sha256", "moduleSha256", "commands", "reason", "kind", "forward", "boundary"].includes(
            key,
          ),
      ) ||
      !/^[a-f0-9]{64}$/.test(entry.sha256) ||
      !/^[a-f0-9]{64}$/.test(entry.moduleSha256) ||
      !Array.isArray(entry.commands) ||
      new Set(entry.commands).size !== entry.commands.length ||
      entry.commands.some((name) => !Object.values(commands).includes(name)) ||
      typeof entry.reason !== "string" ||
      entry.reason.trim().length < 20 ||
      (entry.kind !== undefined &&
        !["support", "library", "dispatch", "isolated", "decompose", "integration"].includes(
          entry.kind,
        ))
    )
      throw new Error(`invalid command attribution ${id}`);
    if (
      (["support", "library"].includes(entry.kind) && entry.commands.length) ||
      (entry.kind === "isolated" && entry.commands.length !== 1) ||
      (["integration", "decompose"].includes(entry.kind) && entry.commands.length < 2)
    )
      throw new Error(`attribution disposition has incompatible commands ${id}`);
    if (entry.boundary !== undefined && entry.boundary !== "external")
      throw new Error(`invalid attribution boundary ${id}`);
    if (
      entry.forward !== undefined &&
      (!entry.forward ||
        typeof entry.forward !== "object" ||
        Array.isArray(entry.forward) ||
        !Number.isInteger(entry.forward.argument) ||
        entry.forward.argument < 0 ||
        !Array.isArray(entry.forward.prefix) ||
        entry.forward.prefix.some((part) => typeof part !== "string") ||
        Object.keys(entry.forward).some((key) => !["argument", "prefix"].includes(key)))
    )
      throw new Error(`invalid attribution forwarding ${id}`);
    const [module, name] = id.split("::");
    if (!sources.has(module))
      sources.set(module, { text: readFileSync(join(root, `tests/e2e/${module}.rs`), "utf8") });
    const source = sources.get(module);
    source.functions ??= functions(source.text);
    const definitions = source.functions.filter((fn) => fn.name === name);
    const raw = definitions.map((fn) => source.text.slice(fn.start, fn.end)).join("\n");
    if (
      !definitions.length ||
      sourceHash(raw) !== entry.sha256 ||
      sourceHash(source.text) !== entry.moduleSha256
    )
      throw new Error(`stale attribution source ${id}; review its argv and assertions again`);
    if (
      entry.forward &&
      definitions.some((fn) => {
        const parameters = calls(fn.signature)[0]?.args ?? [];
        return (
          entry.forward.argument >= parameters.length ||
          !/\[\s*&str\s*\]/.test(parameters[entry.forward.argument]) ||
          entry.forward.prefix.some((part) => !fn.body.includes(`"${part}"`))
        );
      })
    )
      throw new Error(`attribution forwarding differs from source signature/prefix ${id}`);
    if (entry.kind && definitions.some((fn) => !fn.test))
      throw new Error(`helper cannot own a test disposition ${id}`);
  }
  return audit.functions;
}
