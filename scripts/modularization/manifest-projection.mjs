import { execFileSync } from "node:child_process";

const parsed = new Map();
export function cargoManifest(text) {
  if (!parsed.has(text))
    parsed.set(
      text,
      JSON.parse(
        execFileSync(
          "python3",
          ["-c", "import json,sys,tomllib; print(json.dumps(tomllib.loads(sys.stdin.read())))"],
          { input: text, encoding: "utf8", stdio: ["pipe", "pipe", "pipe"] },
        ),
      ),
    );
  return parsed.get(text);
}

export function dependencyDefinitions(manifest) {
  const rows = [];
  const add = (table, scope) => {
    for (const [alias, value] of Object.entries(table ?? {})) {
      const inherited =
        typeof value === "object" && value.workspace
          ? manifest.workspace?.dependencies?.[alias]
          : undefined;
      const packageName =
        (typeof value === "object" ? value.package : undefined) ??
        (typeof inherited === "object" ? inherited.package : undefined) ??
        alias;
      rows.push({ alias, packageName, value, inherited, scope });
    }
  };
  for (const key of ["dependencies", "dev-dependencies", "build-dependencies"]) {
    add(manifest[key], key);
    for (const [target, table] of Object.entries(manifest.target ?? {}))
      add(table[key], `${target}:${key}`);
  }
  return rows;
}

export function manifestFingerprint(manifest, roots, versioned) {
  const pkg = manifest.package ?? {};
  const inherited = (key) => (pkg[key]?.workspace ? manifest.workspace?.package?.[key] : pkg[key]);
  return JSON.stringify({
    package: {
      edition: inherited("edition"),
      rustVersion: inherited("rust-version"),
      build: pkg.build,
      version: versioned ? pkg.version : undefined,
    },
    dependencies: dependencyDefinitions(manifest).filter((row) => roots.includes(row.packageName)),
    features: manifest.features,
    profile: manifest.profile,
    workspaceProfile: manifest.workspace?.profile,
    resolver: manifest.workspace?.resolver,
  });
}

export function manifestChanges(crates, before, after) {
  const a = cargoManifest(before);
  const b = cargoManifest(after);
  return Object.entries(crates)
    .filter(
      ([name, crate]) =>
        manifestFingerprint(a, crate.external, ["onepipeline", "cli-dispatcher"].includes(name)) !==
        manifestFingerprint(b, crate.external, ["onepipeline", "cli-dispatcher"].includes(name)),
    )
    .map(([name]) => `resolved/${name}/manifest.json`);
}

export function toolchainFingerprint(text) {
  const document = cargoManifest(text);
  const toolchain = document.toolchain;
  if (
    Object.keys(document).some((key) => key !== "toolchain") ||
    !toolchain ||
    typeof toolchain !== "object" ||
    Object.keys(toolchain).some(
      (key) => !["channel", "path", "components", "targets", "profile"].includes(key),
    ) ||
    ![toolchain.channel, toolchain.path].some((value) => typeof value === "string" && value.trim())
  )
    throw new Error("invalid Rust toolchain pin");
  for (const key of ["channel", "path"])
    if (
      toolchain[key] !== undefined &&
      (typeof toolchain[key] !== "string" || !toolchain[key].trim())
    )
      throw new Error(`invalid toolchain ${key}`);
  if (
    toolchain.profile !== undefined &&
    !["minimal", "default", "complete"].includes(toolchain.profile)
  )
    throw new Error("invalid toolchain profile");
  for (const key of ["components", "targets"])
    if (
      toolchain[key] !== undefined &&
      (!Array.isArray(toolchain[key]) ||
        !toolchain[key].every((value) => typeof value === "string" && value.length))
    )
      throw new Error(`invalid toolchain ${key}`);
  return JSON.stringify({
    channel: toolchain.channel,
    path: toolchain.path,
    profile: toolchain.profile,
    components: toolchain.components ? [...toolchain.components].sort() : undefined,
    targets: toolchain.targets ? [...toolchain.targets].sort() : undefined,
  });
}
