// GitHub's HTTP boundary, adapted from create-repo's pinned cache journey.
// Scope rules match Actions: a PR reads its merge ref and base; writes stay in its ref.
import { createServer } from "node:http";
export async function githubCache() {
  const entries = new Map();
  const uploads = new Map();
  const bases = new Map();
  const metadata = new Map();
  const calls = [];
  let sequence = 0;
  const server = createServer(async (req, res) => {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    const bytes = Buffer.concat(chunks);
    const url = new URL(req.url, "http://localhost");
    const reply = (value, status = 200) => {
      const body = Buffer.isBuffer(value) ? value : Buffer.from(JSON.stringify(value));
      res.writeHead(status, { "content-length": body.length, "content-type": Buffer.isBuffer(value) ? "application/octet-stream" : "application/json" });
      res.end(body);
    };
    try {
      if (req.method === "POST") {
        const body = JSON.parse(bytes);
        const scope = req.headers.authorization?.match(/^Bearer (refs\/(?:pull\/[1-9][0-9]*\/merge|heads\/[A-Za-z0-9][A-Za-z0-9._/-]*))$/)?.[1];
        if (!scope) return reply({ message: "cache request requires an authenticated ref scope" }, 401);
        if (!body || typeof body !== "object" || Array.isArray(body) ||
            typeof body.key !== "string" || !body.key || body.key.length > 1024 ||
            typeof body.version !== "string" || !body.version || body.version.length > 256)
          return reply({ message: "cache request requires a key and version" }, 400);
        const method = url.pathname.split("/").at(-1);
        calls.push({ method, scope, body });
        const key = JSON.stringify([body.key, body.version]);
        if (method === "GetCacheEntryDownloadURL") {
          const found = entries.get(`${scope}:${key}`) ?? entries.get(`${bases.get(scope) ?? "refs/heads/main"}:${key}`);
          calls.at(-1).found = Boolean(found);
          if (!found) return reply({ ok: false, signedDownloadUrl: "", matchedKey: "" });
          const token = String(sequence++); uploads.set(token, found);
          return reply({ ok: true, signedDownloadUrl: `${api.url}download/${token}`, matchedKey: body.key });
        }
        if (method === "CreateCacheEntry") {
          const refusal = api.createStatus?.(scope);
          if (refusal) {
            calls.at(-1).status = refusal;
            return reply({ message: `recorded cache service refusal ${refusal}` }, refusal);
          }
          const token = String(sequence++);
          uploads.set(token, { scope, key, blocks: new Map() });
          return reply({ ok: true, signedUploadUrl: `${api.url}upload/${token}` });
        }
        if (method === "FinalizeCacheEntryUpload") {
          const upload = [...uploads.values()].find((u) => u.scope === scope && u.key === key && Buffer.isBuffer(u.body));
          if (!upload) return reply({ message: "no completed upload for this key" }, 400);
          entries.set(`${scope}:${key}`, upload.body);
          return reply({ ok: true, entryId: "1" });
        }
        return reply({ error: method }, 404);
      }
      if (req.method === "PUT") {
        const upload = uploads.get(url.pathname.split("/").at(-1));
        if (!url.pathname.startsWith("/upload/") || !(upload?.blocks instanceof Map))
          return reply({ message: "unknown upload" }, 404);
        const component = url.searchParams.get("comp");
        if (component === "block") {
          const id = url.searchParams.get("blockid");
          if (!id || !/^[A-Za-z0-9+/]+={0,2}$/.test(id)) return reply({ message: "invalid block identifier" }, 400);
          upload.blocks.set(id, bytes);
        } else if (component === "blocklist") {
          const xml = bytes.toString().replace(/^<\?xml[^>]*>\s*/, "");
          const list = xml.match(/^<BlockList>([\s\S]*)<\/BlockList>$/)?.[1];
          if (list === undefined) return reply({ message: "invalid block list" }, 400);
          const blocks = [...list.matchAll(/<(Latest|Committed|Uncommitted)>([A-Za-z0-9+/]+={0,2})<\/\1>/g)];
          if (!blocks.length || blocks.map((m) => m[0]).join("") !== list.replace(/\s/g, "") || blocks.some((m) => !upload.blocks.has(m[2])))
            return reply({ message: "block list references missing or malformed blocks" }, 400);
          upload.body = Buffer.concat(blocks.map((m) => upload.blocks.get(m[2])));
        } else if (component !== null) return reply({ message: "unknown upload component" }, 400);
        else upload.body = bytes;
        return reply(Buffer.alloc(0), 201);
      }
      if (req.method === "DELETE") { calls.push({ method: "DELETE" }); return reply(Buffer.alloc(0), 405); }
      if (url.pathname.startsWith("/repos/")) {
        const value = metadata.get(req.url) ?? metadata.get(url.pathname);
        return value === undefined ? reply({ message: `No fixture route: ${req.url}` }, 404) : reply(value);
      }
      const value = uploads.get(url.pathname.split("/").at(-1));
      return value === undefined ? reply({}, 404) : reply(value);
    } catch (error) { reply({ error: error.message }, error instanceof SyntaxError ? 400 : 500); }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const api = { entries, uploads, bases, metadata, calls, url: `http://127.0.0.1:${server.address().port}/`, close: () => new Promise((resolve) => server.close(resolve)) };
  return api;
}
