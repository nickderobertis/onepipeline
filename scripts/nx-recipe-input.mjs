// Nx runtime input: one digest of the part of the justfile a task runs, so an edit to
// another recipe leaves the task's hash, and the proven result under it, where they are.
//
//   node scripts/nx-recipe-input.mjs RECIPE... [--variable NAME]...
//
// It covers each named recipe and what it reaches: its dependencies, the variables its
// text uses, any recipe a `just` line in its text names, the same through each script its
// text names, every exported variable (they reach every recipe's environment) and the
// settings. `--variable` adds one the task reads some other way. It is a runtime input and
// not a file input because Nx's affected selection reads a file input as "this project is
// touched"; `justfileSelection` in nx.json is what selects, and a selected task whose digest
// did not move replays.
//
// Nx hashes a failed runtime command's output as an ordinary input, so a failure prints a
// value unique to this attempt: it cannot be mistaken for a digest, and no two attempts
// can share a replayable key.
import { execFileSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";

// A line that runs just: as its command, after a shell separator or an environment
// prefix, or as a program name in quotes. A message naming a recipe does not run it.
const COMMENT = /^\s*(?:#|\/\/)/;
const RUNS_JUST =
  /(?:^[\s@-]*|[;&|(]\s*|\b(?:then|do|else|exec)\s+|=\S*\s+)just\s+(.*)$|["']just["'],\s*\[?(.*)$/;
const SCRIPT_PATH = /[\w./-]+\.(?:sh|mjs|py)\b/g;

function parseArguments(args) {
  const recipes = [];
  const variables = [];
  for (let at = 0; at < args.length; at++) {
    if (args[at] !== "--variable") recipes.push(args[at]);
    else if (args[++at]) variables.push(args[at]);
    else throw new Error("--variable names no variable");
  }
  if (!recipes.length && !variables.length) throw new Error("name a recipe or a --variable");
  return { recipes, variables };
}

// The variables an expression or a recipe body uses, from just's own parse: a use is the
// two-element array ["variable", name], at any depth. Text and an absent value use none;
// anything else is a shape this does not know, which would otherwise go uncounted.
function variablesIn(parsed, found = new Set()) {
  if (parsed === null || typeof parsed === "string") return found;
  if (!Array.isArray(parsed))
    throw new Error(`just's dump holds an unknown expression: ${JSON.stringify(parsed)}`);
  if (parsed[0] === "variable" && typeof parsed[1] === "string") found.add(parsed[1]);
  for (const part of parsed) variablesIn(part, found);
  return found;
}

// A body is its lines, each of text and expressions; the text scans as the file does.
const textOf = (body) =>
  body.map((line) => line.filter((part) => typeof part === "string").join("")).join("\n");

function reached(
  dump,
  { recipes, variables },
  read = (path) => (existsSync(path) ? readFileSync(path, "utf8") : ""),
) {
  const reachedRecipes = new Set();
  const reachedVariables = new Set(variables);
  for (const [name, assignment] of Object.entries(dump.assignments))
    if (assignment.export) reachedVariables.add(name);
  const seenScripts = new Set();
  const namesIn = (text) => {
    for (const line of text.split("\n")) {
      const invoked = COMMENT.test(line) ? null : line.match(RUNS_JUST);
      if (!invoked) continue;
      for (const word of (invoked[1] ?? invoked[2]).split("#")[0].split(/[^\w-]+/)) {
        if (word in dump.recipes) visit(word);
        else if (word in dump.assignments) reachedVariables.add(word);
      }
    }
  };
  const scriptsIn = (text) => {
    for (const [path] of text.matchAll(SCRIPT_PATH)) {
      if (seenScripts.has(path)) continue;
      seenScripts.add(path);
      const script = read(path);
      namesIn(script);
      scriptsIn(script);
    }
  };
  const visit = (name) => {
    const recipe = dump.recipes[name];
    if (!recipe) throw new Error(`the justfile has no recipe '${name}'`);
    if (reachedRecipes.has(name)) return;
    reachedRecipes.add(name);
    for (const dependency of recipe.dependencies) {
      visit(dependency.recipe);
      for (const use of variablesIn(dependency.arguments)) reachedVariables.add(use);
    }
    for (const use of variablesIn(recipe.body)) reachedVariables.add(use);
    for (const parameter of recipe.parameters)
      for (const use of variablesIn(parameter.default)) reachedVariables.add(use);
    const text = textOf(recipe.body);
    namesIn(text);
    scriptsIn(text);
  };
  recipes.forEach(visit);
  // A variable's own value can use another.
  for (let previous = -1; previous !== reachedVariables.size; ) {
    previous = reachedVariables.size;
    for (const name of [...reachedVariables]) {
      const assignment = dump.assignments[name];
      if (!assignment) throw new Error(`the justfile has no variable '${name}'`);
      for (const use of variablesIn(assignment.value)) reachedVariables.add(use);
    }
  }
  return { recipes: [...reachedRecipes].sort(), variables: [...reachedVariables].sort() };
}

// just's dump is another program's output, and the digest only covers what this reads of
// it: a shape it does not know must fail rather than digest less than the task runs.
function validated(dump) {
  const isObject = (value) => typeof value === "object" && value !== null && !Array.isArray(value);
  if (!isObject(dump?.recipes) || !isObject(dump.assignments) || !isObject(dump.settings))
    throw new Error("just's dump carries no recipes, assignments or settings");
  for (const [name, recipe] of Object.entries(dump.recipes)) {
    const lists = [recipe?.body, recipe?.dependencies, recipe?.parameters];
    if (!lists.every(Array.isArray) || !recipe.body.every(Array.isArray))
      throw new Error(`just's dump gives recipe '${name}' an unknown shape`);
  }
  for (const [name, assignment] of Object.entries(dump.assignments))
    if (!isObject(assignment) || !("value" in assignment) || typeof assignment.export !== "boolean")
      throw new Error(`just's dump gives variable '${name}' an unknown shape`);
  return dump;
}

function digestOf(dump, reach) {
  const describe = (recipe) => ({ ...recipe, doc: undefined, namepath: undefined });
  const content = {
    settings: dump.settings,
    recipes: reach.recipes.map((name) => [name, describe(dump.recipes[name])]),
    variables: reach.variables.map((name) => [name, dump.assignments[name]]),
  };
  return createHash("sha256").update(JSON.stringify(content)).digest("hex");
}

try {
  const wanted = parseArguments(process.argv.slice(2));
  const dump = validated(
    JSON.parse(
      execFileSync("just", ["--dump", "--dump-format", "json"], {
        encoding: "utf8",
        maxBuffer: 64 * 1024 * 1024,
      }),
    ),
  );
  process.stdout.write(`${digestOf(dump, reached(dump, wanted))}\n`);
} catch (error) {
  const unique = randomUUID();
  process.stdout.write(`recipe-input-failed-${unique}\n`);
  process.stderr.write(
    `nx-recipe-input: ${error.message} (${unique})\nACTION: install just and name recipes the justfile defines\n`,
  );
  process.exitCode = 1;
}
