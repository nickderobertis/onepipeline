// Copied from dero-skills' create-repo skill; edit only the source.
// source: github.com/nickderobertis/dero-skills/skills/bootstrap/create-repo/assets/ci/actions-cache-prune.mjs
// version: 1
// sha256: 112878b64182200739b5e0c7e34decc3778f0316f0cb05f9bd40f66445d33031
//
// Deletes a warm leg's superseded Actions cache copies, keeping each family's newest
// entry on main. references/ci.md states why it runs in the leg and what it may delete.
//
//   node scripts/actions-cache-prune.mjs --family PREFIX [--family PREFIX]... [--repo OWNER/NAME]
//
// Reads GH_TOKEN or GITHUB_TOKEN, GITHUB_API_URL and GITHUB_REPOSITORY. A failed read or
// delete warns and leaves the job green: exit 0 except a usage error (2).
import { appendFileSync } from 'node:fs';

const MAIN = 'refs/heads/main';
const PER_PAGE = 100;
const MAX_PAGES = 1000;
const REQUEST_TIMEOUT_MS = 60000;
// Nx's test results share the store, and losing one costs a re-run test.
const RESULT_PREFIX = 'nx-cache-';

class Usage extends Error {}

function warning(message) {
  console.error(`::warning::${String(message).replaceAll('\n', ' ')}`);
}

function summary(line) {
  console.log(line);
  try {
    if (process.env.GITHUB_STEP_SUMMARY)
      appendFileSync(process.env.GITHUB_STEP_SUMMARY, `${line}\n`);
  } catch (error) {
    warning(`Cannot append to the step summary: ${error.message}`);
  }
}

function parseArguments(args) {
  const families = [];
  let repo = process.env.GITHUB_REPOSITORY;
  for (let at = 0; at < args.length; at += 2) {
    const [name, value] = [args[at], args[at + 1]];
    if (name !== '--family' && name !== '--repo')
      throw new Usage(`unknown argument: ${name}`);
    if (value === undefined || value === '' || value.startsWith('--'))
      throw new Usage(`${name} needs a value`);
    if (name === '--repo') repo = value;
    else if (value.startsWith(RESULT_PREFIX))
      throw new Usage(
        `--family ${value} names Nx test results, which this never deletes`,
      );
    else families.push(value);
  }
  if (families.length === 0) throw new Usage('name at least one --family PREFIX');
  if (!/^[\w.-]+\/[\w.-]+$/.test(repo ?? ''))
    throw new Usage(
      'no repository: pass --repo OWNER/NAME or set GITHUB_REPOSITORY',
    );
  return { families, repo };
}

function apiBase() {
  const base = process.env.GITHUB_API_URL || 'https://api.github.com';
  let url;
  try {
    url = new URL(base);
  } catch {
    throw new Usage(`GITHUB_API_URL is not a URL: ${base}`);
  }
  const loopback =
    url.protocol === 'http:' && ['127.0.0.1', 'localhost'].includes(url.hostname);
  if (url.protocol !== 'https:' && !loopback)
    throw new Usage('GITHUB_API_URL must use HTTPS or loopback HTTP');
  return base.replace(/\/$/, '');
}

function client(base) {
  const token = process.env.GH_TOKEN || process.env.GITHUB_TOKEN || '';
  return (path, method = 'GET') =>
    fetch(`${base}/${path}`, {
      method,
      headers: {
        accept: 'application/vnd.github+json',
        'x-github-api-version': '2022-11-28',
        ...(token ? { authorization: `Bearer ${token}` } : {}),
      },
      signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
    });
}

// Deletion builds its path from the id, and the summary adds up the sizes.
const wellFormed = (entry) =>
  Number.isSafeInteger(entry?.id) &&
  entry.id > 0 &&
  typeof entry.key === 'string' &&
  typeof entry.ref === 'string' &&
  Number.isSafeInteger(entry.size_in_bytes);

// Every entry of the family, newest first, or a thrown reason when any page failed:
// what a partial listing would keep cannot be trusted to be the newest copy.
async function listFamily(request, repo, family) {
  const entries = new Map();
  for (let page = 1; page <= MAX_PAGES; page++) {
    const query = new URLSearchParams({
      key: family,
      sort: 'created_at',
      direction: 'desc',
      per_page: String(PER_PAGE),
      page: String(page),
    });
    const answer = await request(`repos/${repo}/actions/caches?${query}`);
    if (!answer.ok) {
      await answer.arrayBuffer();
      throw new Error(`page ${page} answered HTTP ${answer.status}`);
    }
    const listed = (await answer.json()).actions_caches;
    if (!Array.isArray(listed))
      throw new Error(`page ${page} carries no actions_caches`);
    const before = entries.size;
    for (const entry of listed) {
      if (!wellFormed(entry)) throw new Error(`page ${page} holds a malformed entry`);
      entries.set(entry.id, entry);
    }
    if (listed.length < PER_PAGE) return [...entries.values()];
    if (entries.size === before)
      throw new Error(`page ${page} repeated the entries before it`);
  }
  throw new Error(`the listing exceeds ${MAX_PAGES} pages`);
}

async function deleteEntry(request, repo, entry) {
  const answer = await request(`repos/${repo}/actions/caches/${entry.id}`, 'DELETE');
  await answer.arrayBuffer();
  // A 404 is a copy something else already removed: gone is what was asked for.
  if (answer.ok || answer.status === 404) return true;
  throw new Error(`HTTP ${answer.status}`);
}

async function pruneFamily(request, repo, family) {
  let entries;
  try {
    entries = await listFamily(request, repo, family);
  } catch (error) {
    warning(`Cannot list cache family ${family}, deleting nothing: ${error.message}`);
    return summary(`${family}: kept unknown, deleted 0, freed 0 bytes`);
  }
  const kept = entries.find((entry) => entry.ref === MAIN);
  if (!kept) {
    warning(`Cache family ${family} has no copy on main, deleting nothing`);
    return summary(`${family}: kept nothing, deleted 0, freed 0 bytes`);
  }
  let deleted = 0;
  let freed = 0;
  for (const entry of entries.filter((candidate) => candidate !== kept)) {
    try {
      await deleteEntry(request, repo, entry);
      deleted++;
      freed += entry.size_in_bytes;
    } catch (error) {
      warning(`Cannot delete cache ${entry.key} on ${entry.ref}: ${error.message}`);
    }
  }
  summary(`${family}: kept ${kept.key}, deleted ${deleted}, freed ${freed} bytes`);
}

async function main() {
  try {
    const { families, repo } = parseArguments(process.argv.slice(2));
    const request = client(apiBase());
    for (const family of families) await pruneFamily(request, repo, family);
  } catch (error) {
    if (error instanceof Usage) {
      console.error(`actions-cache-prune: ${error.message}`);
      console.error(
        'usage: node scripts/actions-cache-prune.mjs --family PREFIX [--family PREFIX]... [--repo OWNER/NAME]',
      );
      process.exitCode = 2;
    } else {
      warning(`Cache cleanup stopped: ${error.message}`);
    }
  }
}

void main();
