// GitHub's Actions cache REST listing at its HTTP boundary, over the recorded listing in
// ../fixtures/github-cache-listing/. It answers what the real one answers for the calls
// the warm legs' cleanup and the headroom budget make: `key` is a prefix match, entries sort
// as asked, every page is `per_page` long, and a deleted entry is gone from the next read.
import { readFileSync } from "node:fs";
import { createServer } from "node:http";
import { gunzipSync } from "node:zlib";

const recording = (page) => new URL(`../fixtures/github-cache-listing/page-${page}.json.gz`, import.meta.url);
export const recordedEntries = () =>
  [1, 2].flatMap((page) => JSON.parse(gunzipSync(readFileSync(recording(page)))).actions_caches);

export async function githubListing(entries = recordedEntries(), repository = "nickderobertis/onepipeline") {
  const held = [...entries];
  let nextId = Math.max(...held.map((entry) => entry.id)) + 1;
  let clock = Date.parse("2026-11-08T00:00:00Z");
  const requests = [];
  const server = createServer((request, response) => {
    const url = new URL(request.url, "http://127.0.0.1");
    requests.push(`${request.method} ${url.pathname}${url.search}`);
    const answer = (status, body) => {
      response.writeHead(status, { "content-type": "application/json", "x-ratelimit-remaining": "4999" });
      response.end(body === undefined ? undefined : JSON.stringify(body));
    };
    if (!request.headers.authorization) return answer(401, { message: "Requires authentication" });
    const listing = `/repos/${repository}/actions/caches`;
    if (request.method === "GET" && url.pathname === `/repos/${repository}/actions/cache/storage-limit`)
      return answer(200, { max_cache_size_gb: 10 });
    if (request.method === "GET" && url.pathname === listing) {
      const prefix = url.searchParams.get("key") ?? "";
      const size = Number(url.searchParams.get("per_page") ?? 30);
      const page = Number(url.searchParams.get("page") ?? 1);
      if (!Number.isInteger(size) || size < 1 || size > 100 || !Number.isInteger(page) || page < 1)
        return answer(422, { message: "Validation Failed" });
      const found = held
        .filter((entry) => entry.key.startsWith(prefix))
        .sort((a, b) =>
          url.searchParams.get("sort") === "size_in_bytes"
            ? b.size_in_bytes - a.size_in_bytes
            : Date.parse(b.created_at) - Date.parse(a.created_at),
        );
      return answer(200, { total_count: found.length, actions_caches: found.slice((page - 1) * size, page * size) });
    }
    if (request.method === "DELETE" && url.pathname.startsWith(`${listing}/`)) {
      const at = held.findIndex((entry) => String(entry.id) === url.pathname.split("/").at(-1));
      if (at < 0) return answer(404, { message: "Not Found" });
      held.splice(at, 1);
      return answer(204);
    }
    return answer(404, { message: `no route for ${url.pathname}` });
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  return {
    url: `http://127.0.0.1:${server.address().port}`,
    held: () => [...held],
    requests,
    // What a save adds, the way GitHub lists it: on the ref the run was on.
    save(key, ref = "refs/heads/main", size = 1000) {
      clock += 60000;
      const at = new Date(clock).toISOString();
      held.push({ id: nextId++, key, ref, size_in_bytes: size, created_at: at, last_accessed_at: at, version: "fixture" });
    },
    close: () => new Promise((resolve) => server.close(resolve)),
  };
}
