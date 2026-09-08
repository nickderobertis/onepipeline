// A real npm registry, in this process, that can be told to lag.
//
// The registry is the *environment* for `publish-gate.test.mjs`, not the layer
// under test: that suite drives the real `npm` CLI and the real
// `scripts/publish-npm.sh` against this. npmjs.org cannot stand in — a test may
// not publish there, and the behaviour under test is one it exhibits only
// sometimes.
//
// `lagFor` is the one non-standard power: a package whose publish is
// acknowledged at once and whose version becomes resolvable later. Why that is
// the shape to reproduce is in `scripts/publish-npm.sh`.
//
// Not a `*.test.mjs`, so the suite glob does not run it.

import { createHash } from "node:crypto";
import { createServer } from "node:http";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

/// One package's state: the versions published to it, and when each becomes
/// visible to a reader.
class Entry {
  constructor() {
    /// version -> { manifest, tarball: Buffer, visibleAt: epoch ms }
    this.versions = new Map();
    this.filenames = new Map();
  }
}

export class Registry {
  constructor() {
    this.packages = new Map();
    /// package name -> milliseconds between accepting a publish and serving it.
    this.lag = new Map();
    /// Every accepted publish and every version becoming visible, in order.
    /// This is what a test reads to say the launcher was not offered early.
    this.timeline = [];
    this.server = null;
    this.port = 0;
    /// An HTTP status to answer everything with, or null to serve normally.
    this.refusing = null;
  }

  /// Acknowledge publishes of `name` at once but serve them `ms` later.
  lagFor(name, ms) {
    this.lag.set(name, ms);
  }

  /// Answer every request with `status` instead of serving. A registry that is
  /// failing is not a registry saying "no such version", and a caller that
  /// cannot tell them apart publishes over things.
  refuseWith(status) {
    this.refusing = status;
  }

  get url() {
    return `http://127.0.0.1:${this.port}/`;
  }

  /// The versions of `name` a reader can see right now.
  visible(name) {
    const entry = this.packages.get(name);
    if (!entry) return [];
    const now = Date.now();
    return [...entry.versions.entries()]
      .filter(([, held]) => held.visibleAt <= now)
      .map(([version]) => version);
  }

  /// The instant `identity` (`name@version`) became resolvable, or undefined
  /// while it still is not. The value is the entry's own `visibleAt`, not when
  /// the timer that recorded it happened to run, so a caller comparing it
  /// against another event compares two instants rather than two observations.
  visibleAt(identity) {
    return this.timeline.find((e) => e.kind === "visible" && e.identity === identity)?.at;
  }

  /// When `identity` was accepted for publication, or undefined.
  acceptedAt(identity) {
    return this.timeline.find((e) => e.kind === "accepted" && e.identity === identity)?.at;
  }

  /// Write the `.npmrc` an npm invocation needs to reach this registry, and
  /// return the environment that points npm at it. `npm publish` refuses
  /// outright without an auth token, so one is written even though nothing here
  /// checks it.
  npmEnv(dir) {
    mkdirSync(dir, { recursive: true });
    const rc = join(dir, ".npmrc");
    writeFileSync(
      rc,
      `registry=${this.url}\n//127.0.0.1:${this.port}/:_authToken=not-a-real-token\n`,
    );
    return {
      ...process.env,
      NPM_CONFIG_USERCONFIG: rc,
      // A cache of this run's own, so a packument read here can never be
      // answered out of a developer's ~/.npm — and so the poll under test is
      // asking the registry rather than a disk.
      npm_config_cache: join(dir, "npm-cache"),
      npm_config_registry: this.url,
      npm_config_audit: "false",
      npm_config_fund: "false",
    };
  }

  async start() {
    this.server = createServer((req, res) => this.#handle(req, res));
    await new Promise((resolve, reject) => {
      this.server.once("error", reject);
      this.server.listen(0, "127.0.0.1", resolve);
    });
    this.port = this.server.address().port;
  }

  async stop() {
    if (!this.server) return;
    await new Promise((resolve) => this.server.close(resolve));
    this.server = null;
  }

  #packument(name) {
    const entry = this.packages.get(name);
    if (!entry) return null;
    const versions = {};
    for (const version of this.visible(name)) {
      versions[version] = entry.versions.get(version).manifest;
    }
    const numbers = Object.keys(versions);
    if (numbers.length === 0) return null;
    return {
      _id: name,
      name,
      "dist-tags": { latest: numbers[numbers.length - 1] },
      versions,
    };
  }

  #handle(req, res) {
    const answer = (code, body) => {
      const payload = Buffer.isBuffer(body) ? body : Buffer.from(JSON.stringify(body));
      res.writeHead(code, {
        "content-type": Buffer.isBuffer(body) ? "application/octet-stream" : "application/json",
        "content-length": payload.length,
      });
      res.end(payload);
    };

    // A request line is external input even here: npm is not the only thing that
    // can reach this socket, and a path that is not valid percent-encoding must
    // be an answer rather than an exception thrown past the response.
    let path;
    try {
      path = decodeURIComponent(new URL(req.url, this.url).pathname).replace(/^\/+/, "");
    } catch {
      return answer(400, { error: "unreadable request path" });
    }

    if (this.refusing !== null) {
      return answer(this.refusing, { error: "the registry is not answering" });
    }

    if (req.method === "PUT") {
      const chunks = [];
      req.on("data", (chunk) => chunks.push(chunk));
      req.on("end", () => {
        try {
          this.#publish(path, JSON.parse(Buffer.concat(chunks).toString("utf8")));
          answer(201, { ok: true });
        } catch (error) {
          answer(error.status ?? 500, { error: error.message });
        }
      });
      return;
    }

    if (req.method !== "GET" && req.method !== "HEAD") {
      return answer(405, { error: "method not allowed" });
    }

    // `<name>/-/<file>.tgz` is the tarball; anything else is a packument.
    const tarball = path.match(/^(.+)\/-\/([^/]+\.tgz)$/);
    if (tarball) {
      const entry = this.packages.get(tarball[1]);
      const version = entry?.filenames.get(tarball[2]);
      const held = version ? entry.versions.get(version) : undefined;
      if (!held || held.visibleAt > Date.now()) {
        return answer(404, { error: "Not found" });
      }
      return answer(200, held.tarball);
    }

    const packument = this.#packument(path);
    if (!packument) return answer(404, { error: "Not found" });
    return answer(200, packument);
  }

  #publish(name, body) {
    // Everything read below comes off the wire, so its shape is checked here
    // rather than assumed: a body that is not an object, or whose `versions` or
    // `_attachments` are not, is a 400 and not a TypeError.
    const refuse = (why) => {
      throw Object.assign(new Error(why), { status: 400 });
    };
    const isRecord = (value) =>
      typeof value === "object" && value !== null && !Array.isArray(value);
    if (!isRecord(body)) refuse("publish body is not an object");
    if (body.name !== name) refuse("name mismatch");
    if (!isRecord(body.versions)) refuse("publish body has no `versions` object");
    if (body._attachments !== undefined && !isRecord(body._attachments)) {
      refuse("publish body's `_attachments` is not an object");
    }
    const entry = this.packages.get(name) ?? new Entry();
    this.packages.set(name, entry);

    const attachments = Object.entries(body._attachments ?? {});
    for (const [version, manifest] of Object.entries(body.versions)) {
      if (!isRecord(manifest)) refuse(`the manifest for ${version} is not an object`);
      if (entry.versions.has(version)) {
        throw Object.assign(new Error("cannot publish over the previously published version"), {
          status: 403,
        });
      }
      const filename = `${name.replace(/^@[^/]+\//, "")}-${version}.tgz`;
      const attached = attachments.find(([key]) => key.endsWith(filename))?.[1];
      if (!isRecord(attached) || typeof attached.data !== "string") {
        refuse(`no tarball attached for ${version}`);
      }
      const tarball = Buffer.from(attached.data, "base64");
      const identity = `${name}@${version}`;
      const lag = this.lag.get(name) ?? 0;
      const at = Date.now();
      entry.versions.set(version, {
        manifest: {
          ...manifest,
          dist: {
            ...manifest.dist,
            tarball: `${this.url}${name}/-/${filename}`,
            shasum: createHash("sha1").update(tarball).digest("hex"),
          },
        },
        tarball,
        visibleAt: at + lag,
      });
      entry.filenames.set(filename, version);
      this.timeline.push({ kind: "accepted", identity, at });
      // Recorded only once it is true, and carrying the instant it became true,
      // so a reader before then gets `undefined` rather than a future time.
      if (lag === 0) {
        this.timeline.push({ kind: "visible", identity, at });
      } else {
        setTimeout(() => {
          this.timeline.push({ kind: "visible", identity, at: at + lag });
        }, lag).unref();
      }
    }
  }
}
