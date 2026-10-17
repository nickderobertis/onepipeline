// Local stand-ins for the services a release writes to, each a real HTTP server
// in this process that answers the requests the real clients make: GitHub's
// REST and GraphQL APIs (Releases, their assets and notes, pull requests,
// auto-merge), PyPI's upload, simple and JSON APIs, and a crates.io sparse
// registry. npm's is `npm/test/registry-support.mjs`.
//
// They are the *environment* the release journeys run in, not the layer under
// test: the journeys drive the real release/release.py, `uv`, `pip`, `npm` and
// `cargo` against them, because the public services may not be written to by a
// test and cannot be told to fail on cue. Each records every request, so a
// journey can say what was — and what was not — written.

import { createHash } from "node:crypto";
import { createServer } from "node:http";

const BODY_LIMIT = 512 * 1024 * 1024;

function sha256(buffer) {
  return createHash("sha256").update(buffer).digest("hex");
}

/// One HTTP server, a request log, and one-shot refusals by method and path.
class Double {
  constructor() {
    this.server = null;
    this.port = 0;
    this.requests = [];
    this.refusals = [];
  }

  get url() {
    return `http://127.0.0.1:${this.port}`;
  }

  /// Answer the next request matching `method` and `pattern` with `status`:
  /// `{ message }` as JSON, or `message` itself as the body when `raw`.
  refuseNext(method, pattern, status, message = "refused by the test", raw = false) {
    this.refusals.push({ method, pattern, status, message, raw });
  }

  /// Every request that could have written something.
  writes() {
    return this.requests.filter((r) => !["GET", "HEAD"].includes(r.method));
  }

  async start() {
    this.server = createServer((req, res) => {
      const chunks = [];
      let held = 0;
      req.on("data", (chunk) => {
        held += chunk.length;
        if (held <= BODY_LIMIT) chunks.push(chunk);
      });
      req.on("end", () => {
        const url = new URL(req.url, this.url);
        const body = Buffer.concat(chunks);
        this.requests.push({ method: req.method, path: url.pathname, query: url.search });
        const answer = (status, payload, type) => {
          if (res.headersSent) return;
          const data = Buffer.isBuffer(payload)
            ? payload
            : Buffer.from(typeof payload === "string" ? payload : JSON.stringify(payload));
          res.writeHead(status, {
            "content-type":
              type ?? (Buffer.isBuffer(payload) ? "application/octet-stream" : "application/json"),
            "content-length": data.length,
          });
          res.end(req.method === "HEAD" ? undefined : data);
        };
        if (held > BODY_LIMIT) return answer(413, { message: `body exceeds ${BODY_LIMIT} bytes` });
        const refusal = this.refusals.findIndex(
          (r) => r.method === req.method && r.pattern.test(url.pathname),
        );
        if (refusal !== -1) {
          const [r] = this.refusals.splice(refusal, 1);
          return r.raw
            ? answer(r.status, r.message, "text/plain")
            : answer(r.status, { message: r.message });
        }
        Promise.resolve()
          .then(() => this.handle(req, url, body, answer))
          .catch((error) => answer(500, { message: String(error?.stack ?? error) }));
      });
    });
    await new Promise((resolve, reject) => {
      this.server.once("error", reject);
      this.server.listen(0, "127.0.0.1", resolve);
    });
    this.port = this.server.address().port;
    return this;
  }

  async stop() {
    if (!this.server) return;
    this.server.closeAllConnections?.();
    await new Promise((resolve) => this.server.close(resolve));
    this.server = null;
  }
}

/// GitHub: one repository's Releases and pull requests, REST and GraphQL.
///
/// Also answers under `/api/v3`, the prefix GitHub Enterprise Server serves its
/// REST API at, which is how release-plz reaches a forge named by URL.
export class GitHubDouble extends Double {
  constructor(repository = "nickderobertis/onepipeline") {
    super();
    this.repository = repository;
    this.releases = [];
    this.pulls = [];
    this.graphql = [];
    this.nextId = 1;
    /// What the next auto-merge request is answered with: null accepts it.
    this.autoMergeRefusal = null;
    /// Raw bodies to answer the pull request list and GraphQL with instead.
    this.pullsBody = null;
    this.graphqlBody = null;
    /// A raw body to answer the Release asset list with instead.
    this.assetsBody = null;
    /// Called with the Release before each asset listing is answered.
    this.onAssetsListed = null;
  }

  env() {
    return {
      GITHUB_API_URL: this.url,
      GITHUB_GRAPHQL_URL: `${this.url}/graphql`,
      GITHUB_REPOSITORY: this.repository,
      GITHUB_TOKEN: "not-a-real-token",
      GITHUB_SERVER_URL: "https://github.com",
      GITHUB_RUN_ID: "4242",
    };
  }

  addRelease(tag, body = "## What changed\n") {
    const release = { id: this.nextId++, tag_name: tag, body, assets: [] };
    this.releases.push(release);
    return release;
  }

  release(tag) {
    return this.releases.find((r) => r.tag_name === tag);
  }

  addPull(number, ref, { fork = false } = {}) {
    this.pulls.push({
      number,
      node_id: `PR_node_${number}`,
      head: { ref, repo: { full_name: fork ? "someone/fork" : this.repository } },
    });
  }

  #view(release) {
    return {
      id: release.id,
      tag_name: release.tag_name,
      body: release.body,
      upload_url: `${this.url}/uploads/repos/${this.repository}/releases/${release.id}/assets{?name,label}`,
      assets: release.assets.map((a) => this.#asset(a)),
    };
  }

  #asset(asset) {
    return {
      id: asset.id,
      name: asset.name,
      size: asset.content.length,
      url: `${this.url}/repos/${this.repository}/releases/assets/${asset.id}`,
      browser_download_url: `${this.url}/download/${asset.id}/${asset.name}`,
    };
  }

  handle(req, url, body, answer) {
    const path = url.pathname.replace(/^\/api\/v3(?=\/)/, "");
    const repo = `/repos/${this.repository}`;
    const uploads = `/uploads${repo}/releases/`;
    if (req.method === "POST" && path === "/graphql") {
      const request = JSON.parse(body.toString("utf8"));
      this.graphql.push(request);
      if (this.graphqlBody !== null) return answer(200, this.graphqlBody, "application/json");
      if (this.autoMergeRefusal) {
        return answer(200, { data: null, errors: [{ message: this.autoMergeRefusal }] });
      }
      const pull = this.pulls.find((p) => p.node_id === request.variables?.id);
      if (!pull || !/enablePullRequestAutoMerge/.test(request.query)) {
        return answer(200, { data: null, errors: [{ message: "Could not resolve to a node" }] });
      }
      pull.autoMerge = /mergeMethod:\s*SQUASH/.test(request.query) ? "SQUASH" : "MERGE";
      return answer(200, {
        data: {
          enablePullRequestAutoMerge: {
            pullRequest: { number: pull.number, autoMergeRequest: { mergeMethod: pull.autoMerge } },
          },
        },
      });
    }
    if (req.method === "GET" && path === `${repo}/pulls`) {
      if (this.pullsBody !== null) return answer(200, this.pullsBody, "application/json");
      return answer(200, this.pulls);
    }
    if (req.method === "GET" && new RegExp(`^${repo}/commits/[0-9a-f]+/pulls$`).test(path)) {
      return answer(200, []);
    }
    let found = path.match(new RegExp(`^${repo}/releases/tags/(.+)$`));
    if (req.method === "GET" && found) {
      const release = this.release(decodeURIComponent(found[1]));
      return release ? answer(200, this.#view(release)) : answer(404, { message: "Not Found" });
    }
    found = path.match(new RegExp(`^${repo}/releases/(\\d+)/assets$`));
    if (req.method === "GET" && found && this.onAssetsListed) {
      const listed = this.releases.find((r) => r.id === Number(found[1]));
      if (listed) this.onAssetsListed(listed);
    }
    if (req.method === "GET" && found && this.assetsBody !== null) {
      return answer(200, this.assetsBody, "application/json");
    }
    if (req.method === "GET" && found) {
      const release = this.releases.find((r) => r.id === Number(found[1]));
      return release
        ? answer(
            200,
            release.assets.map((a) => this.#asset(a)),
          )
        : answer(404, { message: "Not Found" });
    }
    found = path.match(new RegExp(`^${repo}/releases/(\\d+)$`));
    if (req.method === "PATCH" && found) {
      const release = this.releases.find((r) => r.id === Number(found[1]));
      if (!release) return answer(404, { message: "Not Found" });
      const patch = JSON.parse(body.toString("utf8"));
      if (typeof patch?.body !== "string") return answer(422, { message: "body must be a string" });
      release.body = patch.body;
      return answer(200, this.#view(release));
    }
    if (req.method === "POST" && path.startsWith(uploads)) {
      const release = this.releases.find(
        (r) => r.id === Number(path.slice(uploads.length).split("/")[0]),
      );
      const name = url.searchParams.get("name");
      if (!release || !name) return answer(404, { message: "Not Found" });
      if (release.assets.some((a) => a.name === name)) {
        return answer(422, { message: "Validation Failed", errors: [{ code: "already_exists" }] });
      }
      const asset = { id: this.nextId++, name, content: body };
      release.assets.push(asset);
      return answer(201, this.#asset(asset));
    }
    found = path.match(/^\/download\/(\d+)\/[^/]+$/);
    if (req.method === "GET" && found) {
      const asset = this.releases.flatMap((r) => r.assets).find((a) => a.id === Number(found[1]));
      return asset ? answer(200, asset.content) : answer(404, { message: "Not Found" });
    }
    found = path.match(new RegExp(`^${repo}/releases/assets/(\\d+)$`));
    if (found) {
      const id = Number(found[1]);
      const release = this.releases.find((r) => r.assets.some((a) => a.id === id));
      const asset = release?.assets.find((a) => a.id === id);
      if (!asset) return answer(404, { message: "Not Found" });
      if (req.method === "DELETE") {
        release.assets = release.assets.filter((a) => a.id !== id);
        return answer(204, Buffer.alloc(0));
      }
      if (req.method === "GET") return answer(200, asset.content);
    }
    return answer(404, { message: `the GitHub double does not serve ${req.method} ${path}` });
  }
}

/// PyPI: the legacy upload API, the simple index and the JSON API, one project.
export class PyPIDouble extends Double {
  constructor() {
    super();
    /// filename -> { version, content }
    this.files = new Map();
  }

  env() {
    return {
      ONEPIPELINE_PYPI_UPLOAD_URL: `${this.url}/legacy/`,
      ONEPIPELINE_PYPI_INDEX_URL: `${this.url}/simple/`,
      ONEPIPELINE_PYPI_JSON_URL: `${this.url}/pypi`,
      PYPI_TOKEN: "pypi-not-a-real-token",
    };
  }

  /// Serve a file as if a previous release had uploaded it.
  serve(filename, content) {
    const version = filename.split("-")[1];
    this.files.set(filename, { version, content });
  }

  async handle(req, url, body, answer) {
    if (req.method === "POST" && url.pathname === "/legacy/") {
      const form = await new Request(url, {
        method: "POST",
        headers: { "content-type": req.headers["content-type"] },
        body,
      }).formData();
      const file = form.get("content");
      if (form.get(":action") !== "file_upload" || !file || typeof file === "string") {
        return answer(400, "not a file upload", "text/plain");
      }
      const content = Buffer.from(await file.arrayBuffer());
      if (form.get("sha256_digest") && form.get("sha256_digest") !== sha256(content)) {
        return answer(400, "digest mismatch", "text/plain");
      }
      const version = file.name.split("-")[1];
      if (form.get("name") !== "onepipeline-cli" || form.get("version") !== version) {
        return answer(400, "name or version differs from the file's", "text/plain");
      }
      if (this.files.has(file.name)) return answer(400, "File already exists.", "text/plain");
      this.files.set(file.name, { version, content });
      return answer(200, "OK", "text/plain");
    }
    if (req.method === "GET" && /^\/simple\/onepipeline-cli\/?$/.test(url.pathname)) {
      const links = [...this.files.entries()]
        .map(
          ([name, { content }]) =>
            `<a href="${this.url}/files/${name}#sha256=${sha256(content)}">${name}</a>`,
        )
        .join("\n");
      return answer(200, `<!DOCTYPE html><html><body>\n${links}\n</body></html>\n`, "text/html");
    }
    let found = url.pathname.match(/^\/files\/(.+)$/);
    if (req.method === "GET" && found) {
      const held = this.files.get(decodeURIComponent(found[1]));
      return held ? answer(200, held.content) : answer(404, "Not Found", "text/plain");
    }
    found = url.pathname.match(/^\/pypi\/onepipeline-cli\/([^/]+)\/json$/);
    if (req.method === "GET" && found) {
      const urls = [...this.files.entries()]
        .filter(([, held]) => held.version === found[1])
        .map(([filename]) => ({ filename, url: `${this.url}/files/${filename}` }));
      return urls.length ? answer(200, { urls }) : answer(404, { message: "Not Found" });
    }
    return answer(
      404,
      `the PyPI double does not serve ${req.method} ${url.pathname}`,
      "text/plain",
    );
  }
}

/// crates.io: a sparse registry index, its publish API and its downloads.
export class CratesDouble extends Double {
  constructor() {
    super();
    /// name -> [{ line, content }]
    this.crates = new Map();
  }

  env() {
    return {
      ONEPIPELINE_CRATE_REGISTRY: `${this.url}/index/`,
      CARGO_REGISTRY_TOKEN: "crates-not-a-real-token",
    };
  }

  static path(name) {
    const lower = name.toLowerCase();
    if (lower.length <= 2) return `${lower.length}/${lower}`;
    if (lower.length === 3) return `3/${lower[0]}/${lower}`;
    return `${lower.slice(0, 2)}/${lower.slice(2, 4)}/${lower}`;
  }

  handle(req, url, body, answer) {
    if (req.method === "GET" && url.pathname === "/index/config.json") {
      return answer(200, { dl: `${this.url}/api/v1/crates`, api: this.url });
    }
    if (req.method === "PUT" && url.pathname === "/api/v1/crates/new") {
      // Cargo's framing: a little-endian u32 length and the metadata JSON, then
      // a u32 length and the .crate, and nothing after.
      const malformed = (why) =>
        answer(400, { errors: [{ detail: `malformed publish body: ${why}` }] });
      if (body.length < 4) return malformed("no metadata length");
      const length = body.readUInt32LE(0);
      if (body.length < 8 + length) return malformed("truncated metadata");
      const crateLength = body.readUInt32LE(4 + length);
      if (body.length !== 8 + length + crateLength)
        return malformed("the crate length is not the body's");
      let meta;
      try {
        meta = JSON.parse(body.subarray(4, 4 + length).toString("utf8"));
      } catch {
        return malformed("metadata is not JSON");
      }
      if (
        typeof meta?.name !== "string" ||
        typeof meta.vers !== "string" ||
        !Array.isArray(meta.deps)
      ) {
        return malformed("metadata lacks name, vers or deps");
      }
      const content = body.subarray(8 + length);
      const versions = this.crates.get(meta.name) ?? [];
      if (versions.some((v) => v.line.vers === meta.vers)) {
        return answer(200, {
          errors: [{ detail: `crate version \`${meta.vers}\` is already uploaded` }],
        });
      }
      const deps = meta.deps.map((d) => ({
        name: d.explicit_name_in_toml ?? d.name,
        req: d.version_req,
        features: d.features,
        optional: d.optional,
        default_features: d.default_features,
        target: d.target,
        kind: d.kind,
        registry: d.registry,
        ...(d.explicit_name_in_toml ? { package: d.name } : {}),
      }));
      versions.push({
        line: {
          name: meta.name,
          vers: meta.vers,
          deps,
          cksum: sha256(content),
          features: meta.features,
          yanked: false,
          ...(meta.rust_version ? { rust_version: meta.rust_version } : {}),
        },
        content,
      });
      this.crates.set(meta.name, versions);
      return answer(200, { warnings: { invalid_categories: [], invalid_badges: [], other: [] } });
    }
    let found = url.pathname.match(/^\/api\/v1\/crates\/([^/]+)\/([^/]+)\/download$/);
    if (req.method === "GET" && found) {
      const held = this.crates.get(found[1])?.find((v) => v.line.vers === found[2]);
      return held ? answer(200, held.content) : answer(404, { errors: [{ detail: "Not Found" }] });
    }
    found = url.pathname.match(/^\/index\/(.+)$/);
    if (req.method === "GET" && found) {
      const name = found[1].split("/").at(-1);
      const versions = this.crates.get(name);
      if (!versions || CratesDouble.path(name) !== found[1]) return answer(404, "", "text/plain");
      return answer(
        200,
        `${versions.map((v) => JSON.stringify(v.line)).join("\n")}\n`,
        "text/plain",
      );
    }
    return answer(404, {
      errors: [{ detail: `the crates double does not serve ${url.pathname}` }],
    });
  }
}
