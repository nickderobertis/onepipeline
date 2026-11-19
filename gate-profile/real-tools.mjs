// The genuine tool behind each profiling shim, resolved once, when `just gate-profile` starts and
// before anything it runs can change PATH. run.mjs hands each to the shims as
// ONEPIPELINE_GATE_PROFILE_REAL_<TOOL>, and a shim execs that path rather than looking one up on
// the PATH it was reached through: a test that puts its own fake of the tool first on PATH, and
// whose fake execs the tool it found behind itself — the shim — would otherwise be exec'd by the
// shim in turn, and the two would alternate forever.
import { spawnSync } from "node:child_process";
import { readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
export const SHIMS = join(here, "shims");

/** The variable a shim reads its tool's resolved path from; lib.sh derives the same name. */
export const realToolVariable = (tool) =>
  `ONEPIPELINE_GATE_PROFILE_REAL_${tool.toUpperCase().replace(/[^A-Z0-9_]/g, "_")}`;

/**
 * Each shimmed tool's variable and the tool it resolves to on `path`, past every run's shims, by
 * lib.sh's own lookup. A tool `path` does not carry is left out: its shim then says so when used.
 */
export function realTools(path) {
  const found = {};
  for (const tool of readdirSync(SHIMS)) {
    if (tool.startsWith(".")) continue;
    const lookup = spawnSync(
      "bash",
      ["-c", '. "$1" && gate_profile_real_tool "$2"', "gate-profile", join(here, "lib.sh"), tool],
      { encoding: "utf8", env: { PATH: path } },
    );
    if (lookup.status === 0 && lookup.stdout.trim())
      found[realToolVariable(tool)] = lookup.stdout.trim();
  }
  return found;
}
