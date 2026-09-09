// A real npm registry, in this process, that can be told to lag.
//
// The registry is the *environment* for `publish-gate.test.mjs`, not the layer
// under test: that suite drives the real `npm` CLI and the real
// `scripts/publish-npm.sh` against this. npmjs.org cannot stand in — a test may
// not publish there, and the behaviour under test is one it exhibits only
// sometimes.
//
// `lagFor` and `holdUntilRead` are the two non-standard powers, and both make
// one shape: a package whose publish is acknowledged at once and whose version
// becomes resolvable later. Why that is the shape to reproduce is in
// `scripts/publish-npm.sh`. They differ in what ends the lag — a clock, or the
// reader's own polling — and a journey picks whichever its assertion is about.
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

/// The most a publish body may be before this answers rather than buffers.
/// Generous next to what the journeys send — one stripped debug binary, base64'd
/// — and finite, which is the whole point.
const BODY_LIMIT = 512 * 1024 * 1024;

export class Registry {
  constructor() {
    this.packages = new Map();
    /// package name -> milliseconds between accepting a publish and serving it.
    this.lag = new Map();
    /// package name -> how many more packument reads must find this package's
    /// versions unserved before they are served. See `holdUntilRead`.
    this.reads = new Map();
    /// Every accepted publish and every version becoming visible, in order.
    /// This is what a test reads to say the launcher was not offered early.
    this.timeline = [];
    this.server = null;
    this.port = 0;
    /// An HTTP status to answer everything with, or null to serve normally.
    this.refusing = null;
  }

  /// Acknowledge publishes of `name` at once but serve them `ms` later.
  ///
  /// The lag a journey uses when what it asserts on is an *order* — that the
  /// launcher was not offered before a package it pins was resolvable — because
  /// that comparison is between two instants and needs instants to compare.
  lagFor(name, ms) {
    this.lag.set(name, ms);
  }

  /// The same held publish, ended by the reader rather than by a clock: `name`'s
  /// versions are unserved for the next `reads` packument reads that find them,
  /// and served from the one after.
  ///
  /// The lag a journey uses when what it asserts on is that the caller *waited*.
  /// Measured against the real npm CLI: `npm view --prefer-online` is exactly one
  /// packument read, and every read `npm publish` makes precedes its own PUT — so
  /// `holdUntilRead(name, 1)` means the poll after the publish finds nothing and
  /// the next one finds it, whatever else the host is running. Said with a clock
  /// instead, the same journey asserts that npm answered faster than the lag, and
  /// on a loaded machine it does not: a 1000ms lag left `publish-npm.sh` finding
  /// the version already served, waiting for nothing, and printing no propagation
  /// warning for this to match.
  ///
  /// One or the other per package. A held package ignores any `lagFor` on it,
  /// because two answers to "when does this become visible" is not a registry
  /// anything can reason about.
  holdUntilRead(name, reads) {
    this.reads.set(name, reads);
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

  /// Time-gated: a version published under a lag is not here until it elapses.
  visible(name) {
    const entry = this.packages.get(name);
    if (!entry) return [];
    const now = Date.now();
    return [...entry.versions.entries()]
      .filter(([, held]) => held.visibleAt <= now)
      .map(([version]) => version);
  }

  /// The entry's own `visibleAt` rather than the instant the timer that recorded
  /// it happened to run, so a caller comparing this against another event
  /// compares two instants rather than two observations.
  visibleAt(identity) {
    return this.timeline.find((e) => e.kind === "visible" && e.identity === identity)?.at;
  }

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
    this.#spendRead(name, entry);
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
      let held = 0;
      req.on("data", (chunk) => {
        held += chunk.length;
        // A publish body is a base64'd tarball, so it is large by design and
        // bounded anyway: the largest this suite sends is one stripped debug
        // binary. Past the bound the request is answered rather than buffered,
        // so a body that never ends is a 413 instead of this process's heap.
        if (held > BODY_LIMIT) {
          req.destroy();
          answer(413, { error: `publish body exceeds ${BODY_LIMIT} bytes` });
          return;
        }
        chunks.push(chunk);
      });
      req.on("end", () => {
        if (held > BODY_LIMIT) return;
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

  /// One packument read against a read-held package. While there are reads left
  /// this only spends one and serves nothing; the read after the last one serves
  /// every version that was being held, and records each becoming visible at the
  /// instant it did.
  ///
  /// Counted here rather than in `visible` so that only reads that came over the
  /// wire count — a test asking this registry what it holds is not the reader the
  /// hold is about.
  #spendRead(name, entry) {
    const left = this.reads.get(name);
    if (left === undefined) return;
    const waiting = [...entry.versions.entries()].filter(
      ([, version]) => !Number.isFinite(version.visibleAt),
    );
    if (waiting.length === 0) return;
    if (left > 0) {
      this.reads.set(name, left - 1);
      return;
    }
    const at = Date.now();
    for (const [version, held] of waiting) {
      held.visibleAt = at;
      this.timeline.push({ kind: "visible", identity: `${name}@${version}`, at });
    }
    this.reads.delete(name);
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
      // The key becomes an identity, a filename and a URL below, so it is held
      // to the shape a version has before any of the three are built from it.
      if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)*$/.test(version)) {
        refuse(`'${version}' is not a version this registry will file`);
      }
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
      // `Buffer.from(_, "base64")` discards anything it cannot decode rather
      // than failing, so a body that is not base64 would be filed as a short
      // tarball nobody could install. Checked, then checked again by length.
      if (!/^[A-Za-z0-9+/]*={0,2}$/.test(attached.data)) {
        refuse(`the tarball attached for ${version} is not base64`);
      }
      const tarball = Buffer.from(attached.data, "base64");
      if (tarball.length === 0) refuse(`the tarball attached for ${version} is empty`);
      const identity = `${name}@${version}`;
      // A read-held package has no clock at all, so it is filed at a `visibleAt`
      // no `Date.now()` reaches and `#packument` moves it when the reads run out.
      const held = this.reads.has(name);
      const lag = held ? Number.POSITIVE_INFINITY : (this.lag.get(name) ?? 0);
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
      } else if (Number.isFinite(lag)) {
        setTimeout(() => {
          this.timeline.push({ kind: "visible", identity, at: at + lag });
        }, lag).unref();
      }
    }
  }
}
