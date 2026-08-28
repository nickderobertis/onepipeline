// What this repository publishes, reconciled against the release configuration.
//
// The declaration is `scripts/release-probe.sh`'s `TARGETS`; the published set
// here is *derived* — from the publish steps in `release.yml`, the workspace
// members, the wheel's manifest, the launcher's manifest, and the platform
// matrix — because an inventory transcribed into a check is the thing that goes
// stale in silence. Each reconciliation therefore runs over
// `releaseConfiguration()`, and the mutated-configuration journeys prove the
// derivation notices a change rather than agreeing with itself.
//
// The probe journeys spawn the real script the way `src/release.rs` spawns one,
// under that environment exactly: a search path and a home directory. The
// registry is the one collaborator substituted, and it is substituted *on that
// search path* — a `curl` that points the probe's own endpoints at a fixture
// server, since the probe reads no variable one could be named in. What a public
// registry serves cannot be asked offline, and a fake registry standing in for
// itself is what would then be under test; the real ones are asked weekly by
// `.github/workflows/published-smoke.yml`.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import { mkdtempSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { after, describe, it } from "node:test";
import { fileURLToPath } from "node:url";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");

/** The probe, at the path the release-targets contract fixes it to. */
const PROBE = join(REPO_ROOT, "scripts", "release-probe.sh");

/** The sixty seconds the contract allows one answer. */
const BOUND_MS = 60_000;

function read(...parts) {
  return readFileSync(join(REPO_ROOT, ...parts), "utf8");
}

/**
 * The targets the probe declares, read out of the probe itself.
 *
 * One list, in the file that answers for it: a copy here would be a second
 * inventory to drift, which is what this check exists to prevent.
 */
function declaredTargets(script = read("scripts", "release-probe.sh")) {
  const block = script.match(/^TARGETS=\(\n([\s\S]*?)^\)$/m);
  assert.ok(block, "no `TARGETS=(` array in scripts/release-probe.sh");
  return block[1].split(/\s+/).filter(Boolean);
}

/**
 * The registry endpoints the probe asks, read out of the probe itself.
 *
 * They are constants in that file and settable from nowhere else: the host hands
 * a probe `PATH` and `HOME`, so a variable it read would be a way for a caller's
 * environment to decide which registry an answer came from.
 */
function endpoints(script = read("scripts", "release-probe.sh")) {
  const named = [...script.matchAll(/^[A-Z_]+="(https:\/\/[^"]+)"$/gm)].map((match) => match[1]);
  assert.equal(
    named.length,
    3,
    "the probe names one public endpoint per registry it answers for, as a constant",
  );
  return named;
}

function parseTarget(identifier) {
  const match = identifier.match(/^(crate|pypi|npm):([A-Za-z0-9][A-Za-z0-9._-]*)$/);
  return match ? { registry: match[1], name: match[2] } : null;
}

function tomlString(text, table, key) {
  const section = text.split(`[${table}]`)[1];
  assert.ok(section, `no [${table}] table`);
  const line = section
    .split(/^\[/m)[0]
    .split("\n")
    .find((candidate) => candidate.startsWith(`${key} = `));
  assert.ok(line, `[${table}] has no ${key}`);
  return JSON.parse(line.slice(`${key} = `.length));
}

function unpublishable(manifest) {
  const section = manifest.split("[package]")[1] ?? "";
  return /^publish\s*=\s*false\s*$/m.test(section.split(/^\[/m)[0]);
}

/** The workspace members, from the one place `--workspace` takes them. */
function workspaceMembers(cargo) {
  const line = cargo.match(/^members = (\[.*\])$/m);
  assert.ok(line, "no single-line `members = [...]` in the workspace manifest");
  return JSON.parse(line[1]);
}

function objectLiteral(source, name) {
  const start = source.indexOf(`const ${name} = {`);
  assert.notEqual(start, -1, `no \`const ${name} = {\` in the source`);
  const end = source.indexOf("\n};", start);
  assert.notEqual(end, -1, `\`${name}\` is not terminated by a \`};\` line`);
  return source.slice(start, end);
}

function workflowTargets(workflow, job) {
  const start = workflow.indexOf(`\n  ${job}:\n`);
  assert.notEqual(start, -1, `no \`${job}\` job in release.yml`);
  const rest = workflow.slice(start + 1);
  const nextJob = rest.slice(1).search(/\n {2}[a-z][a-z0-9-]*:\n/);
  const body = nextJob === -1 ? rest : rest.slice(0, nextJob + 1);
  return [...body.matchAll(/^\s*- target: (\S+)$/gm)].map((match) => match[1]);
}

/**
 * How each registry is *reached* by the release workflow, and what to look for
 * to know that it still is.
 *
 * A publish step is what makes a name a name this repository publishes: with the
 * step gone, the name is not published and a target declaring it is stale. Each
 * marker is the step's own irreplaceable part — the publishing command or the
 * publishing action — rather than a job name a rename would quietly break.
 */
const PUBLISH_STEPS = {
  crate: { marker: "cargo publish", where: "the crates.io publish step" },
  pypi: { marker: "pypa/gh-action-pypi-publish", where: "the PyPI publish action" },
  npm: { marker: "scripts/publish-npm.sh", where: "the npm publish step" },
};

/** Everything the reconciliation reads, as text, so a mutation can replace one. */
function releaseConfiguration() {
  const cargo = read("Cargo.toml");
  return {
    workflow: read(".github", "workflows", "release.yml"),
    cargo,
    members: Object.fromEntries(
      workspaceMembers(cargo).map((member) => [
        member,
        member === "." ? cargo : read(member, "Cargo.toml"),
      ]),
    ),
    pyproject: read("pyproject.toml"),
    buildScript: read("scripts", "npm-build.mjs"),
    launcher: JSON.parse(read("npm", "onepipeline-cli", "package.json")),
  };
}

/**
 * Every name this repository publishes, by registry, derived from the
 * configuration that publishes it.
 *
 * The npm side is the launcher plus one package per Rust target a release
 * builds, because that is what `publish-npm` publishes: `scripts/npm-build.mjs`
 * names each `onepipeline-cli-<platform>-<arch>` from the same matrix.
 */
function publishedNames(config) {
  const published = { crate: [], pypi: [], npm: [] };
  if (config.workflow.includes(PUBLISH_STEPS.crate.marker)) {
    for (const [member, manifest] of Object.entries(config.members)) {
      assert.ok(member, "every workspace member is named by its own path");
      if (unpublishable(manifest)) continue;
      published.crate.push(tomlString(manifest, "package", "name"));
    }
  }
  if (config.workflow.includes(PUBLISH_STEPS.pypi.marker)) {
    published.pypi.push(tomlString(config.pyproject, "project", "name"));
  }
  if (config.workflow.includes(PUBLISH_STEPS.npm.marker)) {
    published.npm.push(config.launcher.name);
    const facts = new Map(
      [
        ...objectLiteral(config.buildScript, "TARGETS").matchAll(
          /"([^"]+)": \{ platform: "([^"]+)", arch: "([^"]+)"/g,
        ),
      ].map(([, triple, platform, arch]) => [triple, `onepipeline-cli-${platform}-${arch}`]),
    );
    for (const triple of workflowTargets(config.workflow, "build-npm")) {
      const name = facts.get(triple);
      assert.ok(name, `scripts/npm-build.mjs names no npm package for the target ${triple}`);
      published.npm.push(name);
    }
  }
  return published;
}

/**
 * The two ways the declaration and the release configuration can disagree.
 *
 * `undeclared` is a name this repository publishes that no target covers — the
 * one that degrades a consumer to launch-now with nothing said. `unpublished` is
 * a target naming something this repository does not publish, which holds a
 * consumer on an answer that will never come.
 *
 * A per-platform npm package is *covered* when the launcher that resolves it is
 * itself a declared target: nobody depends on `onepipeline-cli-linux-x64`
 * directly, and npm installs it only as the launcher's own exact-version
 * optional dependency.
 */
function reconcile(declared, config) {
  const published = publishedNames(config);
  const targets = declared.map((identifier) => ({ identifier, ...parseTarget(identifier) }));
  const covered = new Set();
  for (const target of targets) {
    if (target.registry !== "npm" || target.name !== config.launcher.name) continue;
    for (const dependency of Object.keys(config.launcher.optionalDependencies ?? {})) {
      covered.add(dependency);
    }
  }

  const undeclared = [];
  for (const [registry, names] of Object.entries(published)) {
    for (const name of names) {
      if (targets.some((target) => target.registry === registry && target.name === name)) continue;
      if (registry === "npm" && covered.has(name)) continue;
      undeclared.push(`${registry}:${name}`);
    }
  }
  const unpublished = targets
    .filter((target) => !published[target.registry]?.includes(target.name))
    .map((target) => target.identifier);
  return { undeclared, unpublished };
}

/**
 * A registry, over real HTTP, answering from a route table.
 *
 * A route is a function of the request count, so a registry that hiccups once
 * and then answers is expressible — which is the recovery path a probe run on a
 * held node has to survive.
 */
async function registry(routes) {
  const asked = new Map();
  const server = createServer((request, response) => {
    const count = (asked.get(request.url) ?? 0) + 1;
    asked.set(request.url, count);
    const route = routes[request.url];
    if (!route) {
      response.writeHead(404, { "content-type": "application/json" });
      response.end('"Not Found"');
      return;
    }
    const { status, body } =
      typeof route === "string" ? { status: 200, body: route } : route(count);
    response.writeHead(status, { "content-type": "application/json" });
    response.end(body);
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  return {
    get base() {
      return `http://127.0.0.1:${server.address().port}`;
    },
    asked,
    close: () => new Promise((closed) => server.close(closed)),
  };
}

const always = (status, body) => () => ({ status, body });

function indexRecord(name, version, yanked) {
  return `${JSON.stringify({
    name,
    vers: version,
    // A populated `deps` array is the fixture rather than decoration around it:
    // a real record embeds one `"name"` per dependency, so a reader that took
    // the first match would answer with a dependency's name where the crate's
    // belongs, and one that counted the string would call every real record
    // unreadable.
    deps: [
      { name: "onevcs", req: "^0.15", features: [], optional: false, kind: "normal" },
      { name: "clap", req: "^4.6", features: ["derive"], optional: false, kind: "normal" },
    ],
    cksum: "0",
    features: {},
    yanked,
    rust_version: "1.88",
  })}\n`;
}

function hostTool(tool) {
  const resolved = spawnSync("sh", ["-c", `command -v ${tool}`], {
    encoding: "utf8",
  }).stdout.trim();
  assert.ok(resolved, `${tool} is not on this host's PATH, so the probe cannot run at all`);
  return resolved;
}

/**
 * A search path holding exactly the programs the probe reaches for, with a
 * `curl` that asks the fixture registry rather than the public ones.
 *
 * The substitution rides on the search path because that — with `HOME` — is the
 * whole environment the host gives a probe: there is no variable to point it
 * elsewhere with, and deliberately none to add. What the fixture `curl` decides
 * is only *which host answers*; the arguments, the status code, the body file,
 * the timeout, and the transport failure a closed port produces are the real
 * curl's, over real HTTP.
 *
 * `without` leaves one program off, which is how a host that cannot run what the
 * probe needs is driven.
 */
const scratchPaths = [];
function fixturePath(base, { without } = {}) {
  const dir = mkdtempSync(join(tmpdir(), "release-probe-path-"));
  scratchPaths.push(dir);
  for (const tool of ["bash", "sleep", "awk"]) {
    if (tool === without) continue;
    symlinkSync(hostTool(tool), join(dir, tool));
  }
  // Read out of the probe rather than written here, so an endpoint it moves to
  // reaches this fixture rather than quietly reaching the public registry and
  // passing.
  const rewrites = endpoints()
    .map((endpoint) => `    ${endpoint}/*) asked+=("${base}/\${arg#${endpoint}/}") ;;`)
    .join("\n");
  writeFileSync(
    join(dir, "curl"),
    `#!/usr/bin/env bash
set -euo pipefail
asked=()
for arg in "$@"; do
  case "$arg" in
${rewrites}
    http://*|https://*)
      echo "fixture curl: nothing here serves '$arg' — the probe asked an endpoint this fixture does not rewrite" >&2
      exit 6
      ;;
    *) asked+=("$arg") ;;
  esac
done
exec ${hostTool("curl")} "\${asked[@]}"
`,
    { mode: 0o755 },
  );
  return dir;
}

/**
 * The environment the host hands a probe, and nothing else: a search path and a
 * home directory. No credential, no registry setting, nothing the caller
 * happened to be holding — so a probe that had come to need one would fail every
 * journey below rather than passing on this machine's ambient environment.
 */
function contractEnv(path) {
  return { PATH: path, HOME: process.env.HOME };
}

/**
 * The probe, run as `src/release.rs` runs one: the file itself, no shell, one
 * argument, this repository's root, and the environment the host gives it.
 *
 * Spawned asynchronously rather than synchronously because the registry these
 * journeys point it at is served from this process — a blocking spawn would hold
 * the event loop that has to answer the request the probe is waiting on.
 */
function probe(identifier, { env = contractEnv(), args } = {}) {
  const argv = args ?? (identifier === undefined ? [] : [identifier]);
  const started = Date.now();
  const child = spawn(PROBE, argv, { cwd: REPO_ROOT, env });
  let stdout = "";
  let stderr = "";
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stdout.on("data", (chunk) => {
    stdout += chunk;
  });
  child.stderr.on("data", (chunk) => {
    stderr += chunk;
  });
  return new Promise((settle, fail) => {
    child.on("error", fail);
    child.on("close", (status) =>
      settle({ status, stdout, stderr, elapsed: Date.now() - started }),
    );
  });
}

function said(run) {
  return `exit ${run.status}\n--- stdout ---\n${run.stdout}\n--- stderr ---\n${run.stderr}`;
}

/** A registry serving one released version of every target this repository has. */
function releasedAt(version) {
  return registry({
    "/on/ep/onepipeline": always(
      200,
      // Out of publication order, with a newer yanked release and a newer
      // prerelease that are not candidates: an answer taken from the last line,
      // or from the highest number on the page, is a different answer from this.
      [
        indexRecord("onepipeline", "0.1.0", false),
        indexRecord("onepipeline", version, false),
        indexRecord("onepipeline", "0.2.0", false),
        indexRecord("onepipeline", "99.0.0", true),
        indexRecord("onepipeline", "99.1.0-rc.1", false),
      ].join(""),
    ),
    "/onepipeline-cli/json": always(
      200,
      JSON.stringify({ info: { name: "onepipeline-cli", version }, releases: {} }),
    ),
    "/onepipeline-cli/latest": always(
      200,
      JSON.stringify({ name: "onepipeline-cli", version, dist: { tarball: "https://x/y.tgz" } }),
    ),
  });
}

describe("the release targets this repository declares", () => {
  const config = releaseConfiguration();
  const declared = declaredTargets();

  it("names every target as a registry-qualified identifier", () => {
    assert.ok(declared.length > 0, "scripts/release-probe.sh declares no release targets");
    for (const identifier of declared) {
      assert.ok(
        parseTarget(identifier),
        `'${identifier}' is not <registry>:<name> for a registry a consumer can resolve — ` +
          "an unqualified name is two artifacts where one name is served twice",
      );
    }
    assert.equal(new Set(declared).size, declared.length, "a target is declared twice");
  });

  it("reads a published set out of the release configuration rather than a list", () => {
    // The derivation has to find something in every file it reads, or the
    // reconciliation below would agree with the declaration by finding nothing.
    const published = publishedNames(config);
    for (const [registry, step] of Object.entries(PUBLISH_STEPS)) {
      assert.ok(
        config.workflow.includes(step.marker),
        `release.yml no longer carries ${step.where}; if this repository has stopped ` +
          `publishing to ${registry}, drop its target from scripts/release-probe.sh`,
      );
      assert.ok(published[registry].length > 0, `no ${registry} name derived from the workflow`);
    }
    assert.ok(
      Object.keys(config.members).length > 1,
      "the workspace has more than one member, and each is either published or `publish = false`",
    );
    assert.ok(
      published.npm.length > 1,
      "the npm side is the launcher plus one package per target a release builds",
    );
  });

  it("covers every name this repository publishes", () => {
    const { undeclared } = reconcile(declared, config);
    assert.deepEqual(
      undeclared,
      [],
      `this repository publishes ${undeclared.join(", ")}, which no declared release target ` +
        "covers — declare it in scripts/release-probe.sh's TARGETS, or, for a per-platform " +
        "package, resolve it from the launcher that already is a target",
    );
  });

  it("declares nothing this repository does not publish", () => {
    const { unpublished } = reconcile(declared, config);
    assert.deepEqual(
      unpublished,
      [],
      `${unpublished.join(", ")} is declared but nothing in the release configuration ` +
        "publishes it — a consumer holding for that release would wait forever",
    );
  });

  it("fails on a crate that starts publishing without being declared", () => {
    const withMember = {
      ...config,
      members: {
        ...config.members,
        "crates/newthing": '[package]\nname = "onepipeline-newthing"\nversion = "0.1.0"\n',
      },
    };
    assert.deepEqual(reconcile(declared, withMember), {
      undeclared: ["crate:onepipeline-newthing"],
      unpublished: [],
    });
  });

  it("fails on a platform package the launcher does not resolve", () => {
    const orphaned = {
      ...config,
      launcher: {
        ...config.launcher,
        optionalDependencies: Object.fromEntries(
          Object.entries(config.launcher.optionalDependencies).filter(
            ([name]) => name !== "onepipeline-cli-win32-x64",
          ),
        ),
      },
    };
    assert.deepEqual(reconcile(declared, orphaned), {
      undeclared: ["npm:onepipeline-cli-win32-x64"],
      unpublished: [],
    });
  });

  it("fails on a target naming something no registry serves", () => {
    assert.deepEqual(reconcile([...declared, "pypi:onepipeline-extra"], config), {
      undeclared: [],
      unpublished: ["pypi:onepipeline-extra"],
    });
  });

  it("fails on a target whose registry the release workflow no longer reaches", () => {
    const withoutNpm = {
      ...config,
      workflow: config.workflow.replaceAll(PUBLISH_STEPS.npm.marker, "scripts/nothing.sh"),
    };
    assert.deepEqual(reconcile(declared, withoutNpm), {
      undeclared: [],
      unpublished: ["npm:onepipeline-cli"],
    });
  });
});

describe("the release probe", () => {
  const declared = declaredTargets();
  const servers = [];
  /** A fixture registry, closed when the suite ends however a journey leaves it. */
  const serving = async (pending) => {
    const server = await pending;
    servers.push(server);
    return server;
  };
  after(() => {
    for (const server of servers) server.close();
    for (const dir of scratchPaths) rmSync(dir, { recursive: true, force: true });
  });

  it("is an executable file at the path the contract fixes", () => {
    assert.ok(
      statSync(PROBE).mode & 0o111,
      "scripts/release-probe.sh is not executable, so the host cannot spawn it directly",
    );
  });

  it("answers the version each registry currently serves", async () => {
    // A version no public registry has ever served, so a journey whose fixture
    // `curl` had stopped rewriting — and had reached the real registry instead —
    // fails here rather than passing on what is really published.
    const server = await serving(releasedAt("7.8.9"));
    const env = contractEnv(fixturePath(server.base));
    for (const identifier of declared) {
      const run = await probe(identifier, { env });
      assert.equal(run.status, 0, `${identifier} was not answered:\n${said(run)}`);
      assert.equal(run.stdout, "7.8.9\n", `${identifier} answered wrongly:\n${said(run)}`);
      assert.equal(run.stderr, "", `${identifier} answered with noise:\n${said(run)}`);
      assert.ok(run.elapsed < BOUND_MS, `${identifier} took ${run.elapsed}ms to answer`);
    }
    assert.deepEqual(
      [...server.asked.values()],
      [1, 1, 1],
      "one ask per target reached the fixture registry, and no target was asked twice",
    );
  });

  it("takes no direction from the environment but a search path and a home directory", async () => {
    const server = await serving(releasedAt("1.2.3"));
    const env = contractEnv(fixturePath(server.base));
    assert.deepEqual(
      Object.keys(env).sort(),
      ["HOME", "PATH"],
      "the environment these journeys hand the probe carries something the host does not",
    );
    const bare = await probe("pypi:onepipeline-cli", { env });
    assert.equal(bare.status, 0, `the probe wanted more than the host gives it:\n${said(bare)}`);
    assert.equal(bare.stdout, "1.2.3\n", said(bare));

    // The same question under an environment holding a credential and a registry
    // setting for every endpoint, each pointed at a port nothing listens on. A
    // probe that read any of them would answer from somewhere else or refuse;
    // this one answers the same version, because where it asks is a constant in
    // its own source and nothing outside the file can move it.
    const noisy = await probe("pypi:onepipeline-cli", {
      env: {
        ...env,
        ONEPIPELINE_CRATES_INDEX: "http://127.0.0.1:1",
        ONEPIPELINE_PYPI_API: "http://127.0.0.1:1",
        ONEPIPELINE_NPM_REGISTRY: "http://127.0.0.1:1",
        CARGO_REGISTRY_TOKEN: "not-a-token",
        PYPI_TOKEN: "not-a-token",
        NPM_TOKEN: "",
      },
    });
    assert.equal(noisy.status, 0, `an ambient environment changed the answer:\n${said(noisy)}`);
    assert.equal(
      noisy.stdout,
      "1.2.3\n",
      `an ambient environment changed the answer:\n${said(noisy)}`,
    );
  });

  it("answers nothing at all, and succeeds, when a registry has no release yet", async () => {
    // Nothing is routed, so every path is the registry's own 404 — which is how
    // each of the three says it serves no such artifact.
    const server = await serving(registry({}));
    for (const identifier of declared) {
      const run = await probe(identifier, { env: contractEnv(fixturePath(server.base)) });
      assert.equal(
        run.status,
        0,
        `${identifier} reported "no release yet" as a failure:\n${said(run)}`,
      );
      assert.equal(
        run.stdout,
        "",
        `${identifier} answered a version nobody published:\n${said(run)}`,
      );
    }
  });

  it("does not answer for an identifier it does not recognise", async () => {
    const server = await serving(releasedAt("0.16.3"));
    const env = contractEnv(fixturePath(server.base));
    const strangers = [
      // A real artifact of a sibling repository, which this one cannot speak for.
      "crate:onevcs",
      // The right name under the wrong registry: this repository publishes
      // `onepipeline` to crates.io and `onepipeline-cli` to PyPI and npm.
      "pypi:onepipeline",
      // A per-platform package, which the launcher covers and which no consumer
      // names in order to depend on it.
      "npm:onepipeline-cli-linux-x64",
      // Unqualified — the shape that names two artifacts at once.
      "onepipeline-cli",
      "",
    ];
    for (const stranger of strangers) {
      const run = await probe(stranger, { env });
      assert.notEqual(
        run.status,
        0,
        `'${stranger}' was answered rather than refused:\n${said(run)}`,
      );
      assert.equal(
        run.stdout,
        "",
        `'${stranger}' printed on stdout, which a consumer reads as an answer:\n${said(run)}`,
      );
      assert.match(run.stderr, /release-probe: [\s\S]*\nACTION: /, said(run));
    }
    for (const argv of [[], ["crate:onepipeline", "pypi:onepipeline-cli"]]) {
      const run = await probe(undefined, { env, args: argv });
      assert.notEqual(run.status, 0, `${argv.length} arguments was answered:\n${said(run)}`);
      assert.equal(run.stdout, "", said(run));
    }
  });

  it("does not read a registry that will not answer as a registry with no release", async () => {
    const server = await serving(
      registry({
        "/on/ep/onepipeline": always(500, '{"error":"internal"}'),
        "/onepipeline-cli/json": always(503, '{"error":"unavailable"}'),
        "/onepipeline-cli/latest": always(403, '{"error":"forbidden"}'),
      }),
    );
    for (const identifier of declared) {
      const run = await probe(identifier, { env: contractEnv(fixturePath(server.base)) });
      assert.notEqual(
        run.status,
        0,
        `${identifier} reported an unread registry as an answer:\n${said(run)}`,
      );
      assert.equal(
        run.stdout,
        "",
        `${identifier} printed an answer for a registry that never gave one:\n${said(run)}`,
      );
      assert.match(run.stderr, /ACTION: /, said(run));
      assert.ok(
        run.elapsed < BOUND_MS,
        `${identifier} spent ${run.elapsed}ms on a registry that would not answer, which is ` +
          "past the bound the release-target contract sets",
      );
    }
  });

  it("retries a registry that hiccups and answers once it recovers", async () => {
    // Each status a registry says "not now" with, rather than "no such thing":
    // unavailable, rate-limited, and timed out are all answers a later attempt
    // gets past, and reporting any of them as a verdict would hold — or release
    // — a node on a hiccup.
    const recovering = (statuses, body) => (count) =>
      count <= statuses.length
        ? { status: statuses[count - 1], body: '{"busy":true}' }
        : { status: 200, body };
    const server = await serving(
      registry({
        "/onepipeline-cli/latest": recovering(
          [503],
          JSON.stringify({ name: "onepipeline-cli", version: "0.16.3" }),
        ),
        "/onepipeline-cli/json": recovering(
          [429, 408],
          JSON.stringify({ info: { name: "onepipeline-cli", version: "0.16.3" } }),
        ),
      }),
    );
    const env = contractEnv(fixturePath(server.base));
    for (const [identifier, path, asks] of [
      ["npm:onepipeline-cli", "/onepipeline-cli/latest", 2],
      ["pypi:onepipeline-cli", "/onepipeline-cli/json", 3],
    ]) {
      const run = await probe(identifier, { env });
      assert.equal(
        run.status,
        0,
        `${identifier}: a registry that recovered was reported as unread:\n${said(run)}`,
      );
      assert.equal(run.stdout, "0.16.3\n", said(run));
      assert.equal(
        server.asked.get(path),
        asks,
        `${identifier}: the probe gave up before the registry recovered`,
      );
    }
  });

  it("does not read a registry it cannot reach at all as one with no release", async () => {
    // A port nothing is listening on: the transport fails before any status
    // exists to read, which is the shape an outage or a bad endpoint takes.
    const closed = await registry({});
    const base = closed.base;
    await closed.close();
    const run = await probe("crate:onepipeline", { env: contractEnv(fixturePath(base)) });
    assert.notEqual(run.status, 0, `an unreachable registry was answered for:\n${said(run)}`);
    assert.equal(run.stdout, "", said(run));
    assert.match(run.stderr, /did not answer in 3 attempts[\s\S]*ACTION: /, said(run));
    assert.ok(run.elapsed < BOUND_MS, `an unreachable registry took ${run.elapsed}ms`);
  });

  it("refuses a document that answers twice rather than picking a copy", async () => {
    // Duplicate members are legal JSON and no parser has to prefer either copy,
    // so which release such a document names depends on who reads it — which is
    // not an answer a node can be held or launched on.
    const server = await serving(
      registry({
        "/on/ep/onepipeline":
          '{"name":"onepipeline","vers":"0.16.3","vers":"0.2.0","deps":[],"cksum":"0",' +
          '"features":{},"yanked":false}\n',
        "/onepipeline-cli/json": '{"info":{"version":"0.16.3","version":"0.2.0"}}',
        "/onepipeline-cli/latest":
          '{"name":"onepipeline-cli","version":"0.16.3","version":"0.2.0"}',
      }),
    );
    for (const identifier of declared) {
      const run = await probe(identifier, { env: contractEnv(fixturePath(server.base)) });
      assert.notEqual(
        run.status,
        0,
        `${identifier} picked one of two answers a document gave:\n${said(run)}`,
      );
      assert.equal(run.stdout, "", said(run));
    }
  });

  it("refuses a version string that is not one rather than passing it on", async () => {
    const cases = [
      ["pypi:onepipeline-cli", "/onepipeline-cli/json", "the latest one"],
      ["npm:onepipeline-cli", "/onepipeline-cli/latest", "1abc.2"],
      // Longer than any release any of these registries has ever served: an
      // answer a consumer carries into a plan is held to a length as well as to
      // a shape.
      ["npm:onepipeline-cli", "/onepipeline-cli/latest", `0.${"1".repeat(70)}.0`],
    ];
    for (const [identifier, path, version] of cases) {
      const body = path.endsWith("/json")
        ? JSON.stringify({ info: { name: "onepipeline-cli", version } })
        : JSON.stringify({ name: "onepipeline-cli", version });
      const server = await serving(registry({ [path]: always(200, body) }));
      const run = await probe(identifier, { env: contractEnv(fixturePath(server.base)) });
      assert.notEqual(
        run.status,
        0,
        `${identifier} passed on '${version}', which no consumer can hold a node against:\n${said(run)}`,
      );
      assert.equal(run.stdout, "", said(run));
      assert.match(run.stderr, /ACTION: /, said(run));
    }
  });

  it("does not answer when the reader of a registry's document cannot run", async () => {
    const server = await serving(releasedAt("0.16.3"));
    const env = contractEnv(fixturePath(server.base, { without: "awk" }));
    for (const identifier of declared) {
      const run = await probe(identifier, { env });
      assert.notEqual(
        run.status,
        0,
        `${identifier} was answered on a host that could not read the document:\n${said(run)}`,
      );
      assert.equal(run.stdout, "", said(run));
      assert.match(run.stderr, /ACTION: /, said(run));
    }
  });

  it("does not answer when it cannot wait before asking a hiccuping registry again", async () => {
    // The registry says "not now", so the probe has to pause before asking
    // again. A host that cannot pause has not been told anything about a
    // release, and three immediate asks would be a retry that never waited.
    const server = await serving(
      registry({ "/onepipeline-cli/latest": always(503, '{"error":"unavailable"}') }),
    );
    const run = await probe("npm:onepipeline-cli", {
      env: contractEnv(fixturePath(server.base, { without: "sleep" })),
    });
    assert.notEqual(run.status, 0, `a retry that never waited was answered:\n${said(run)}`);
    assert.equal(run.stdout, "", said(run));
    assert.match(
      run.stderr,
      /cannot wait 1 second\(s\) before asking[\s\S]*ACTION: /,
      `the refusal blames the registry for a pause this host could not take:\n${said(run)}`,
    );
    assert.equal(
      server.asked.get("/onepipeline-cli/latest"),
      1,
      "a host that cannot pause asked the registry again anyway, which is a retry that never waited",
    );
  });

  it("answers nothing when every release a registry files is one nothing resolves to", async () => {
    // The index has the crate on it, so this is not a 404 — but a yanked release
    // and a prerelease are not what `cargo add onepipeline` gets, so there is no
    // release currently served, which is the empty answer rather than a refusal.
    const server = await serving(
      registry({
        "/on/ep/onepipeline": always(
          200,
          indexRecord("onepipeline", "0.16.3", true) +
            indexRecord("onepipeline", "0.17.0-rc.1", false),
        ),
      }),
    );
    const run = await probe("crate:onepipeline", { env: contractEnv(fixturePath(server.base)) });
    assert.equal(
      run.status,
      0,
      `a crate with nothing currently served was a failure:\n${said(run)}`,
    );
    assert.equal(run.stdout, "", `a yanked or prerelease version was answered:\n${said(run)}`);
  });

  it("does not read an answer it cannot parse as a registry with no release", async () => {
    const server = await serving(
      registry({
        "/on/ep/onepipeline": always(200, indexRecord("onevcs", "0.15.2", false)),
        "/onepipeline-cli/json": always(200, JSON.stringify({ info: { name: "onepipeline-cli" } })),
        "/onepipeline-cli/latest": always(200, "<html>not a manifest</html>"),
      }),
    );
    for (const identifier of declared) {
      const run = await probe(identifier, { env: contractEnv(fixturePath(server.base)) });
      assert.notEqual(
        run.status,
        0,
        `${identifier} answered from a document it could not read:\n${said(run)}`,
      );
      assert.equal(run.stdout, "", said(run));
      assert.match(run.stderr, /ACTION: /, said(run));
    }
  });

  it("refuses a version it cannot order rather than naming the wrong release", async () => {
    const server = await serving(
      registry({
        "/on/ep/onepipeline": always(
          200,
          indexRecord("onepipeline", "0.16.3", false) + indexRecord("onepipeline", "0.16", false),
        ),
      }),
    );
    const run = await probe("crate:onepipeline", { env: contractEnv(fixturePath(server.base)) });
    assert.notEqual(
      run.status,
      0,
      `a version this cannot order was compared anyway:\n${said(run)}`,
    );
    assert.equal(run.stdout, "", said(run));
    assert.match(
      run.stderr,
      /version '0\.16', which this cannot order/,
      `the refusal does not name the version that stopped it:\n${said(run)}`,
    );
  });
});
