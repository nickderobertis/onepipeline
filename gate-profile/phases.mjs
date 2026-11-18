// The phases the `just` shim runs a recipe as: its prior dependencies, depth first and each once,
// in the order `just` itself runs them, so that running each with `--no-deps` and then the recipe
// with `--no-deps` runs exactly what `just <recipe> <args>` would.
//
//   node gate-profile/phases.mjs <real just> <recipe> [args...]
//
// Prints the dependencies to run first, one per line (none is an empty plan). Exit 3 when the
// invocation cannot be split that way without changing what runs — a dependency given arguments,
// a subsequent (`&&`) dependency, a `[parallel]` recipe, or more arguments than the recipe takes,
// which `just` reads as further recipes — and the shim then runs it whole as one phase. Exit 2 for
// a usage error or a justfile `just --dump` cannot read.
import { execFileSync } from "node:child_process";

const NOT_SPLITTABLE = 3;

function fail(message) {
  console.error(`gate-profile: ${message}`);
  process.exit(2);
}

const [real, recipe, ...args] = process.argv.slice(2);
if (!real || !recipe) fail("usage: phases.mjs <real just> <recipe> [args...]");

let recipes;
try {
  const dump = JSON.parse(
    execFileSync(real, ["--dump", "--dump-format", "json"], {
      encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024,
      stdio: ["ignore", "pipe", "inherit"],
    }),
  );
  recipes = dump?.recipes;
  if (!recipes || typeof recipes !== "object") throw new Error("it has no `recipes` object");
} catch (error) {
  fail(`cannot read the justfile through \`just --dump\`: ${error.message}`);
}

const isObject = (value) => value !== null && typeof value === "object" && !Array.isArray(value);

/** The recipe `name` as the dump declares it, with every field the plan reads checked. */
function declared(name) {
  const entry = recipes[name];
  if (entry === undefined) return null;
  const valid =
    isObject(entry) &&
    Array.isArray(entry.dependencies) &&
    entry.dependencies.every(
      (d) => isObject(d) && typeof d.recipe === "string" && Array.isArray(d.arguments),
    ) &&
    Array.isArray(entry.parameters) &&
    entry.parameters.every((p) => isObject(p) && typeof p.kind === "string") &&
    (entry.priors === undefined || Number.isInteger(entry.priors)) &&
    (entry.attributes === undefined || Array.isArray(entry.attributes));
  if (!valid) fail(`\`just --dump\` describes recipe ${name} in a shape this does not know`);
  return entry;
}

function splittable(entry) {
  const priors = entry.priors ?? entry.dependencies.length;
  if (priors !== entry.dependencies.length) return false;
  if ((entry.attributes ?? []).some((a) => a === "parallel" || (isObject(a) && "parallel" in a)))
    return false;
  return entry.dependencies.every((d) => d.arguments.length === 0);
}

const top = declared(recipe);
if (!top) process.exit(NOT_SPLITTABLE);
const variadic = top.parameters.some((p) => p.kind !== "singular");
if (!variadic && args.length > top.parameters.length) process.exit(NOT_SPLITTABLE);

const order = [];
const seen = new Set([recipe]);
function visit(entry) {
  if (!splittable(entry)) process.exit(NOT_SPLITTABLE);
  for (const { recipe: name } of entry.dependencies) {
    if (seen.has(name)) continue;
    seen.add(name);
    const dependency = declared(name);
    if (!dependency) fail(`recipe ${name} is named as a dependency but \`just --dump\` lacks it`);
    visit(dependency);
    order.push(name);
  }
}
visit(top);
if (order.length) console.log(order.join("\n"));
