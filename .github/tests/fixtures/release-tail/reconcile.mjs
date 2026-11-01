// Re-fetch every recording under recordings/ from GitHub, read-only, and diff it against the
// committed fixture. It runs by hand, never in a gate — it reaches GitHub, and what GitHub answers
// moves — and says, for each recording, whether GitHub still answers what was recorded:
//
//   node .github/tests/fixtures/release-tail/reconcile.mjs
//
// Needs `gh` signed in with read access to this repository. The recorded 401 is re-fetched under
// an invalid token, as it was recorded. Ignored, and only these, because GitHub rewrites them on
// every answer or with no change to the record: the response's date, etag, last-modified,
// rate-limit and request-id headers, and the `download_count` and `updated_at` fields in a body.
// Exit 0 when every recording matches, 1 naming each that does not, and 2 when a read cannot be
// made at all.
import { spawnSync } from "node:child_process";
import { readdirSync, readFileSync } from "node:fs";

const RECORDINGS = new URL("./recordings/", import.meta.url);
const IGNORED_HEADERS = /^(date|etag|last-modified|x-ratelimit-.*|x-github-request-id)$/;
const IGNORED_FIELDS = new Set(["download_count", "updated_at"]);
const INVALID_TOKEN = "ghp_invalidreplaytoken000000000000000000000";

// `gh api --include` output: the status line, the headers, a blank line and the body.
function parse(text, what) {
  const split = text.indexOf("\r\n\r\n");
  if (split === -1) throw Error(`${what} holds no response`);
  const [statusLine, ...headerLines] = text.slice(0, split).split(/\r?\n/);
  const status = statusLine.match(/^HTTP\/[0-9.]+ ([1-5][0-9]{2})(?: |$)/)?.[1];
  if (!status) throw Error(`${what} starts with no HTTP status line: ${statusLine.slice(0, 80)}`);
  const headers = {};
  for (const line of headerLines) {
    const at = line.indexOf(": ");
    if (at > 0) headers[line.slice(0, at).toLowerCase()] = line.slice(at + 2);
  }
  return { status: Number(status), headers, body: JSON.parse(text.slice(split + 4)) };
}

function fetch(path, invalid) {
  const ran = spawnSync("gh", ["api", "--include", "--method", "GET", path], {
    encoding: "utf8",
    maxBuffer: 256 * 1024 * 1024,
    env: invalid ? { ...process.env, GH_TOKEN: INVALID_TOKEN } : process.env,
  });
  if (ran.error) throw ran.error;
  return parse(ran.stdout, `gh's answer for ${path} (${ran.stderr.trim()})`);
}

function differences(recorded, fetched, at = "body") {
  if (
    recorded === null ||
    fetched === null ||
    typeof recorded !== "object" ||
    typeof fetched !== "object"
  )
    return JSON.stringify(recorded) === JSON.stringify(fetched) ? [] : [at];
  if (Array.isArray(recorded) !== Array.isArray(fetched)) return [at];
  const keys = new Set([...Object.keys(recorded), ...Object.keys(fetched)]);
  return [...keys]
    .filter((key) => !IGNORED_FIELDS.has(key))
    .flatMap((key) => differences(recorded[key], fetched[key], `${at}.${key}`));
}

let names = [];
try {
  names = readdirSync(RECORDINGS).filter((name) => name.endsWith(".http"));
} catch {}
if (!names.length) {
  console.error(
    "reconcile: recordings/ holds no recording; next: restore the `*.http` recordings beside this script",
  );
  process.exit(2);
}
let mismatched = 0;
try {
  for (const name of names) {
    const recorded = parse(readFileSync(new URL(name, RECORDINGS), "utf8"), name);
    const { path } = JSON.parse(readFileSync(new URL(`${name}.source.json`, RECORDINGS), "utf8"));
    const fetched = fetch(path, recorded.status !== 200);
    const found = [
      ...(recorded.status === fetched.status
        ? []
        : [`status ${recorded.status} is now ${fetched.status}`]),
      ...Object.keys(recorded.headers)
        .filter((header) => !IGNORED_HEADERS.test(header))
        .filter((header) => recorded.headers[header] !== fetched.headers[header])
        .map((header) => `header ${header}`),
      ...differences(recorded.body, fetched.body),
    ];
    if (found.length) {
      mismatched++;
      console.error(
        `${name}: GitHub now answers otherwise at ${found.slice(0, 20).join(", ")}${found.length > 20 ? ", …" : ""}`,
      );
    }
  }
} catch (error) {
  console.error(
    `reconcile: ${error.message}; next: sign gh in with read access to this repository (gh auth status) and re-run`,
  );
  process.exit(2);
}
if (mismatched) {
  console.error(
    `${mismatched} recording(s) no longer match GitHub. Re-record them read-only with \`gh api --include --method GET <path> > <name>.http\` and re-run .github/tests/release-tail.test.mjs.`,
  );
  process.exitCode = 1;
}
