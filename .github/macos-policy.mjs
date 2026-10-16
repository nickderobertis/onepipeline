import { readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { pathToFileURL } from "node:url";

function git(...args) {
  return execFileSync("git", args, { encoding: "utf8" });
}
const versionSource = String.raw`(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?`;
const versionPattern = new RegExp(`^(?:${versionSource})$`);
function removeVersion(object) {
  if (Object.hasOwn(object, "version")) {
    if (
      typeof object.version !== "string" ||
      versionPattern.exec(object.version)?.[0] !== object.version
    )
      throw new Error("invalid version metadata");
    delete object.version;
  }
}
function canonical(object) {
  if (Array.isArray(object)) return object.map(canonical);
  if (object && typeof object === "object")
    return Object.fromEntries(
      Object.keys(object)
        .sort()
        .map((key) => [key, canonical(object[key])]),
    );
  return object;
}
function semantic(path, text) {
  if (path.endsWith(".json")) {
    const value = JSON.parse(text);
    const versionPresence = [Object.hasOwn(value, "version")];
    if (path === "package-lock.json") {
      removeVersion(value);
      versionPresence.push(Object.hasOwn(value.packages?.[""] ?? {}, "version"));
      if (value.packages?.[""]) removeVersion(value.packages[""]);
    } else removeVersion(value);
    return JSON.stringify([versionPresence, canonical(value)]);
  }
  // Python's standard TOML parser proves that dependencies and all other fields
  // are unchanged; deleting arbitrary lines containing "version" would not.
  const script = `import json,sys,tomllib,re
v=tomllib.loads(sys.stdin.read())
p=sys.argv[1]
presence=[]
def remove_version(obj):
 presence.append('version' in obj)
 if 'version' in obj:
  if not isinstance(obj['version'],str) or not re.fullmatch(sys.argv[2],obj['version']): raise ValueError('invalid version metadata')
  del obj['version']
if p=='Cargo.lock':
 for package in v.get('package',[]):
  if package.get('name')=='onepipeline' and 'source' not in package: remove_version(package)
elif p=='Cargo.toml': remove_version(v.get('package',{}))
else: remove_version(v.get('project',{}))
print(json.dumps([presence,v],sort_keys=True))`;
  return execFileSync("python3", ["-c", script, path, versionSource], {
    input: text,
    encoding: "utf8",
  });
}
// The non-PR events this repository's workflows are triggered by. None of them
// requests the Mac lane; the manual diagnostic opts in through its own input.
export const automaticNonPullRequestEvents = new Set([
  "push",
  "release",
  "workflow_run",
  "repository_dispatch",
  "schedule",
  "workflow_dispatch",
]);
export function classify(
  event,
  base,
  head,
  eventName = process.env.GITHUB_EVENT_NAME ?? "pull_request",
) {
  const required = (reason) => ({ isReleaseOnly: false, macosRequired: true, reason });
  if (automaticNonPullRequestEvents.has(eventName))
    return { isReleaseOnly: false, macosRequired: false, reason: `non-PR event: ${eventName}` };
  if (eventName !== "pull_request") return required(`unrecognised event: ${eventName}`);
  try {
    const pr = event.pull_request;
    if (
      [pr?.head?.repo?.full_name, event.repository?.full_name, pr?.head?.ref].some(
        (value) => typeof value !== "string" || !value,
      )
    )
      return required("unreadable PR identity");
    if (
      pr.head.repo.full_name !== event.repository.full_name ||
      !pr.head.ref.startsWith("release-plz-")
    )
      return required("ordinary or fork PR");
    git("check-ref-format", "--branch", pr.head.ref);
    base = git("rev-parse", "--verify", "--end-of-options", `${base}^{commit}`).trim();
    head = git("rev-parse", "--verify", "--end-of-options", `${head}^{commit}`).trim();
    const paths = git("diff", "--name-only", "--no-renames", base, head)
      .trim()
      .split("\n")
      .filter(Boolean);
    if (!paths.length) return required("empty release diff");
    const admitted = new Set([
      "Cargo.toml",
      "Cargo.lock",
      "pyproject.toml",
      "package.json",
      "package-lock.json",
      "npm/onepipeline-cli/package.json",
    ]);
    for (const path of paths) {
      const before = git("show", `${base}:${path}`);
      const after = git("show", `${head}:${path}`);
      if (path === "CHANGELOG.md") continue;
      if (!admitted.has(path) || semantic(path, before) !== semantic(path, after))
        return required(`non-version content: ${path}`);
    }
    return {
      isReleaseOnly: true,
      macosRequired: false,
      reason: "same-repository release-plz branch; semantic version/changelog-only diff",
    };
  } catch (error) {
    return required(`unreadable classification: ${error.message}`);
  }
}
export function carrier({ changes, macos, required, crate, proof }) {
  if (!["cross", "install", "install-documented"].includes(proof))
    throw new Error("unknown Mac proof");
  if (
    changes !== "success" ||
    !["true", "false"].includes(required) ||
    !["true", "false"].includes(crate)
  )
    throw new Error("classification prerequisite failed, cancelled, or missing");
  if (required === "false") {
    if (macos !== "skipped") throw new Error("authorized omission requires a skipped Mac lane");
    return "Mac lane intentionally omitted by classifier; no Mac tests claimed";
  }
  if (proof === "cross" && crate === "false") {
    if (macos !== "success")
      throw new Error("Mac installs must still succeed on a path-sensitive cross skip");
    return "cross omitted by affected-project selection; Mac install proofs succeeded";
  }
  if (macos !== "success")
    throw new Error("required Mac lane failed, cancelled, skipped, or missing");
  return `Mac ${proof} proof succeeded`;
}
// The verdict a Linux carrier reports for one leg of a path-selected matrix
// proof: that leg's own recorded success, or an omission the affected-project
// selection authorized. Anything failed, cancelled or missing fails.
export function pathCarrier({ changes, crate, proof, leg, verdict }) {
  if (typeof leg !== "string" || !leg) throw new Error("path carrier names no leg");
  if (changes !== "success" || !["true", "false"].includes(crate))
    throw new Error("affected-project prerequisite failed, cancelled, or missing");
  if (crate === "false") {
    if (proof !== "skipped" || verdict)
      throw new Error("an authorized path omission requires a skipped proof");
    return `${leg} proof omitted by affected-project selection; no tests claimed`;
  }
  if (!["success", "failure"].includes(proof) || verdict !== "success")
    throw new Error(`${leg} proof failed, cancelled, skipped, or missing`);
  return `${leg} succeeded`;
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const [mode, ...args] = process.argv.slice(2);
    if (mode === "classify") {
      const options = {};
      for (let i = 0; i < args.length; i += 2) {
        if (!["--event", "--base", "--head"].includes(args[i]) || !args[i + 1])
          throw new Error("usage: classify --event JSON --base REF --head REF");
        const key = args[i].slice(2);
        if (Object.hasOwn(options, key)) throw new Error(`duplicate option --${key}`);
        options[key] = args[i + 1];
      }
      if (!["event", "base", "head"].every((key) => Object.hasOwn(options, key)))
        throw new Error("usage: classify --event JSON --base REF --head REF");
      let event;
      try {
        event = JSON.parse(readFileSync(options.event, "utf8"));
      } catch {
        event = {};
      }
      console.log(JSON.stringify(classify(event, options.base, options.head)));
    } else if (mode === "carrier") {
      if (args.length) throw new Error("usage: carrier takes no arguments");
      console.log(
        carrier({
          changes: process.env.CHANGES_RESULT,
          macos: process.env.MACOS_RESULT,
          required: process.env.MACOS_REQUIRED,
          crate: process.env.CRATE_AFFECTED,
          proof: process.env.PROOF,
        }),
      );
    } else if (mode === "path-carrier") {
      if (args.length) throw new Error("usage: path-carrier takes no arguments");
      console.log(
        pathCarrier({
          changes: process.env.CHANGES_RESULT,
          crate: process.env.CRATE_AFFECTED,
          proof: process.env.PROOF_RESULT,
          leg: process.env.LEG,
          verdict: process.env.LEG_VERDICT,
        }),
      );
    } else throw new Error("expected classify, carrier or path-carrier");
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
