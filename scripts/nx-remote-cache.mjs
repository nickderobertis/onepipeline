// Copied from dero-skills' create-repo skill; edit only the source.
// source: github.com/nickderobertis/dero-skills/skills/bootstrap/create-repo/assets/ci/nx-remote-cache.mjs
// version: 10
// sha256: f8ee98c7b95c9e2afe7cab4539183dabce85d92c67506215b052782d849923e8
import {
  appendFileSync,
  readFileSync,
  writeFileSync,
  renameSync,
  mkdirSync,
  existsSync,
  readdirSync,
  copyFileSync,
  rmSync,
} from 'node:fs';
import { join, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createServer, request } from 'node:http';
import { spawn, execFileSync } from 'node:child_process';
import { inflateRawSync } from 'node:zlib';
import { randomUUID, createHash } from 'node:crypto';

export const PROMOTION_BRANCH_PATTERN = 'cache-promote/**';
export const PROMOTION_BRANCH_PREFIX = PROMOTION_BRANCH_PATTERN.slice(0, -2);
export const OP_NX_COMMIT = '91839c24885842cac86567c30e23b37fb85f7e33';
const self = fileURLToPath(import.meta.url);
const env = process.env;
const state = () =>
  join(
    env.RUNNER_TEMP || '.nx',
    'nx-cache-recording',
    env.GITHUB_JOB || 'local',
  );
const json = (path) => JSON.parse(readFileSync(path, 'utf8'));
function save(path, value) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, JSON.stringify(value, null, 2) + '\n');
}
function append(path, value) {
  mkdirSync(dirname(path), { recursive: true });
  appendFileSync(path, JSON.stringify(value) + '\n');
}
function lines(path) {
  return existsSync(path)
    ? readFileSync(path, 'utf8')
        .trim()
        .split('\n')
        .filter(Boolean)
        .map(JSON.parse)
    : [];
}
function warning(message) {
  console.error(`::warning::${String(message).replaceAll('\n', ' ')}`);
}
function summary(message) {
  console.log(message);
  try {
    if (env.GITHUB_STEP_SUMMARY)
      appendFileSync(env.GITHUB_STEP_SUMMARY, message + '\n');
  } catch (error) {
    warning(`Cannot append cache summary: ${error.message}`);
  }
}
function usage(message) {
  const error = new Error(message);
  error.usage = true;
  throw error;
}
function check(ok, message) {
  if (!ok) throw new Error(message);
}
const utc = (value) =>
  typeof value === 'string' &&
  /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?Z$/.test(value) &&
  Number.isFinite(Date.parse(value));
const hashOK = (value) =>
  typeof value === 'string' && /^[a-f0-9]{1,512}$/.test(value);
const integer = (value) => Number.isSafeInteger(value) && value > 0;
const taskKey = (t) => `${t.task}\0${t.os}\0${t.hash}`;
// A remote read's outcome at the recording proxy: 200 served it, 404 said it holds nothing,
// and any other status or no answer (null) is a failed read. The pinned sidecar answers its
// own store faults with a 404, so those read as a miss, which can only understate replays.
function remoteRead(gets) {
  if (gets.length === 0) return 'not_attempted';
  if (gets.some((get) => get.http_status === 200)) return 'hit';
  return gets.at(-1).http_status === 404 ? 'miss' : 'failed';
}
const trusted = (pr, repo) =>
  pr.head?.repo?.full_name === repo &&
  pr.base?.repo?.full_name === repo &&
  typeof pr.user?.login === 'string' &&
  pr.user.login !== 'dependabot[bot]';
function options(args, allowed, required = []) {
  const out = {};
  for (let i = 0; i < args.length; i += 2) {
    const name = args[i]?.replace(/^--/, '');
    if (
      !args[i]?.startsWith('--') ||
      !allowed.includes(name) ||
      !args[i + 1] ||
      args[i + 1].startsWith('--') ||
      name in out
    )
      usage(`invalid argument ${args[i]}`);
    out[name] = args[i + 1];
  }
  for (const name of required) if (!out[name]) usage(`--${name} is required`);
  return out;
}

// Nx invokes this hook once for each existing task invocation. Failure records
// an evidence gap without changing the task's exit status.
export async function preTasksExecution(_options, context) {
  if (!env.GITHUB_ACTIONS) return;
  try {
    append(join(state(), 'started.jsonl'), {
      id: context.id,
      argv: context.argv,
    });
  } catch (error) {
    warning(`Nx telemetry start unavailable: ${error.message}`);
  }
}

export async function postTasksExecution(_options, context) {
  if (!env.GITHUB_ACTIONS) return;
  try {
    const tasks = Object.values(context.taskResults).map((result) => {
      const t = result.task;
      check(
        hashOK(t.hash) &&
          Number.isFinite(t.startTime) &&
          Number.isFinite(t.endTime),
        `incomplete Nx telemetry for ${t.id}`,
      );
      check(
        typeof t.cache === 'boolean',
        `Nx omitted cacheability for ${t.id}`,
      );
      return {
        task: t.id,
        hash: t.hash,
        cacheable: t.cache,
        status: result.status,
        code: result.code,
        started_at: new Date(t.startTime).toISOString(),
        finished_at: new Date(t.endTime).toISOString(),
      };
    });
    append(join(state(), 'invocations.jsonl'), {
      id: context.id,
      started_at: new Date(context.startTime).toISOString(),
      finished_at: new Date(context.endTime).toISOString(),
      tasks,
    });
  } catch (error) {
    warning(`Nx telemetry unavailable: ${error.message}`);
    try {
      append(join(state(), 'errors.jsonl'), { error: error.message });
    } catch {
      /* reporter detects the absent evidence */
    }
  }
}

function trust() {
  const payload = json(env.GITHUB_EVENT_PATH);
  const pr = payload.pull_request;
  const readOnly = Boolean(pr && !trusted(pr, env.GITHUB_REPOSITORY));
  check(env.GITHUB_OUTPUT, 'GITHUB_OUTPUT is unset');
  appendFileSync(
    env.GITHUB_OUTPUT,
    `read_only=${readOnly}\npromotable=${Boolean(pr && !readOnly)}\n`,
  );
}

async function probe(url, ms) {
  try {
    const r = await fetch(`${url}/v1/cache/deadbeef`, {
      signal: AbortSignal.timeout(ms),
    });
    await r.arrayBuffer();
    return r.status;
  } catch {
    return null;
  }
}
async function ready(opts) {
  const seconds = Number(opts['timeout-seconds'] ?? 60);
  if (!Number.isFinite(seconds) || seconds < 0 || seconds > 3600)
    usage('--timeout-seconds must be between 0 and 3600');
  rmSync(state(), { recursive: true, force: true });
  mkdirSync(state(), { recursive: true });
  const url = env.NX_SELF_HOSTED_REMOTE_CACHE_SERVER;
  const token = env.NX_SELF_HOSTED_REMOTE_CACHE_ACCESS_TOKEN;
  const begun = Date.now();
  save(join(state(), 'session.json'), { started_at: new Date().toISOString() });
  if (!url || !token || !/^http:\/\/127\.0\.0\.1:\d+$/.test(url)) {
    warning(
      'Nx cache sidecar unavailable; tasks will run without remote cache',
    );
    return;
  }
  let live = false;
  while (Date.now() - begun < seconds * 1000) {
    if (
      (await probe(
        url,
        Math.max(1, Math.min(1000, seconds * 1000 - (Date.now() - begun))),
      )) === 401
    ) {
      live = true;
      break;
    }
    await new Promise((done) =>
      setTimeout(done, Math.min(100, seconds * 1000)),
    );
  }
  if (!live) {
    warning(
      'Nx cache sidecar not live within readiness bound; tasks will run without remote cache',
    );
    return;
  }
  const secret = randomUUID();
  // llmlint: ignore[typescript_and_npm_launched_via_bun] Restart this same portable Node CLI with its current interpreter; consumers do not install a bun package for the proxy.
  const child = spawn(process.execPath, [self, '_proxy'], {
    env: { ...env, NX_RECORDING_SECRET: secret },
    detached: true,
    stdio: 'ignore',
  });
  child.unref();
  const deadline = Date.now() + 5000;
  while (!existsSync(join(state(), 'proxy.json')) && Date.now() < deadline)
    await new Promise((done) => setTimeout(done, 50));
  if (!existsSync(join(state(), 'proxy.json'))) {
    child.kill();
    warning(
      'Nx recording proxy unavailable; tasks will run without remote cache',
    );
    return;
  }
  const proxy = json(join(state(), 'proxy.json'));
  check(env.GITHUB_ENV, 'GITHUB_ENV is unset');
  appendFileSync(
    env.GITHUB_ENV,
    `NX_SELF_HOSTED_REMOTE_CACHE_SERVER=${proxy.url}\nNX_SELF_HOSTED_REMOTE_CACHE_ACCESS_TOKEN=${token}\n`,
  );
}

async function proxy() {
  const target = env.NX_SELF_HOSTED_REMOTE_CACHE_SERVER;
  const events = join(state(), 'requests.jsonl');
  const bodies = join(state(), 'bodies');
  mkdirSync(bodies, { recursive: true });
  const server = createServer((req, res) => {
    if (
      req.url === '/stop' &&
      req.headers.authorization === env.NX_RECORDING_SECRET
    ) {
      res.end('stopped');
      server.close();
      return;
    }
    const hash = req.url.match(/^\/v1\/cache\/([a-f0-9]{1,512})$/)?.[1];
    if (!hash || !['GET', 'PUT'].includes(req.method)) {
      req.resume();
      res.writeHead(404).end();
      return;
    }
    const id = randomUUID();
    const event = {
      id,
      hash,
      method: req.method,
      attempted_at: new Date().toISOString(),
      http_status: null,
    };
    // Persist the attempt before forwarding: an interrupted transport remains
    // a failed attempt, rather than disappearing from eligibility.
    append(events, event);
    const sent = [],
      received = [];
    let ended = false;
    function finish(status) {
      if (ended) return;
      ended = true;
      event.http_status = status;
      event.finished_at = new Date().toISOString();
      if (status === 200) {
        const body = Buffer.concat(req.method === 'PUT' ? sent : received);
        event.bytes = body.length;
        event.body = `${id}.tar`;
        writeFileSync(join(bodies, event.body), body);
      }
      append(events, event);
    }
    const upstream = request(
      `${target}${req.url}`,
      { method: req.method, headers: req.headers },
      (answer) => {
        const status = answer.statusCode;
        // Nx accepts 409 as a non-fatal refusal. Preserve the sidecar's status
        // for telemetry and promotion; a refused upload is never reported stored.
        answer.on('data', (chunk) => received.push(chunk));
        answer.on('end', () => {
          finish(status);
          res.writeHead(req.method === 'PUT' && status !== 200 ? 409 : status, {
            ...answer.headers,
            'x-nx-cache-upstream-status': String(status),
          });
          res.end(Buffer.concat(received));
        });
        answer.on('error', () => {
          finish(null);
          if (!res.headersSent)
            res.writeHead(req.method === 'PUT' ? 409 : 404, {
              'x-nx-cache-upstream-status': 'unavailable',
            });
          res.end();
        });
      },
    );
    upstream.on('error', () => {
      finish(null);
      if (!res.headersSent)
        res.writeHead(req.method === 'PUT' ? 409 : 404, {
          'x-nx-cache-upstream-status': 'unavailable',
        });
      res.end();
    });
    req.on('data', (chunk) => sent.push(chunk));
    req.on('error', () => {
      finish(null);
      upstream.destroy();
    });
    req.pipe(upstream);
  });
  await new Promise((done) => server.listen(0, '127.0.0.1', done));
  const proxyPath = join(state(), 'proxy.json');
  const temporaryPath = `${proxyPath}.${randomUUID()}.tmp`;
  save(temporaryPath, {
    url: `http://127.0.0.1:${server.address().port}`,
    secret: env.NX_RECORDING_SECRET,
  });
  renameSync(temporaryPath, proxyPath);
}

// A schema 2 document is incomplete when it names its unfinished invocations. One
// with no base_sha is the earlier writer's schema 2, which meant incomplete alone.
const incompleteTelemetry = (doc) =>
  doc.schema === 2 && (!('base_sha' in doc) || 'cancelled_invocations' in doc);
// The first parent of the merge commit a pull_request run checked out. Anything else
// at GITHUB_SHA (a push, a shallow checkout) has no base this run can name.
function mergeRefBase() {
  try {
    // llmlint: ignore[scripts_runtime_independent] The run's merge commit is only known to the consumer checkout; git is provided on the GitHub-hosted runner, with no authoring dependency.
    const parents = execFileSync(
      'git',
      ['rev-list', '--parents', '-n', '1', env.GITHUB_SHA],
      { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] },
    )
      .trim()
      .split(' ');
    return parents.length === 3 ? parents[1] : null;
  } catch {
    return null;
  }
}

function validateExecutions(doc) {
  check(
    [1, 2].includes(doc?.schema) &&
      typeof doc.repository === 'string' &&
      typeof doc.ref === 'string' &&
      (doc.pr === null || integer(doc.pr)) &&
      typeof doc.head_sha === 'string' &&
      /^[a-f0-9]{40}$/.test(doc.head_sha) &&
      integer(doc.run_id) &&
      integer(doc.attempt) &&
      typeof doc.job === 'string' &&
      ['linux', 'darwin', 'win32'].includes(doc.os),
    'invalid executions identity',
  );
  check(
    doc.selection &&
      (doc.pr === null
        ? doc.selection.mode === 'all'
        : doc.selection.mode === 'affected' &&
          /^[a-f0-9]{40}$/.test(doc.selection.base_sha) &&
          doc.selection.head_sha === doc.head_sha),
    'invalid selection',
  );
  check(
    Array.isArray(doc.tasks) && Array.isArray(doc.store_attempts),
    'missing tasks or store_attempts',
  );
  if ('base_sha' in doc)
    check(
      doc.base_sha === null ||
        (typeof doc.base_sha === 'string' && /^[a-f0-9]{40}$/.test(doc.base_sha)),
      'invalid base_sha',
    );
  if (doc.schema === 2)
    check(
      Array.isArray(doc.cancelled_invocations ?? []) &&
        (doc.cancelled_invocations ?? []).every(
          (invocation) =>
            typeof invocation.id === 'string' &&
            Array.isArray(invocation.argv) &&
            invocation.argv.every((arg) => typeof arg === 'string'),
        ),
      'invalid cancelled invocations',
    );
  for (const t of doc.tasks)
    check(
      typeof t.task === 'string' &&
        t.task.includes(':') &&
        hashOK(t.hash) &&
        typeof t.cacheable === 'boolean' &&
        ['remote_hit', 'local_hit', 'executed', 'failed'].includes(t.result) &&
        (t.remote_status === null || Number.isInteger(t.remote_status)) &&
        (t.remote_read === undefined ||
          ['hit', 'miss', 'failed', 'not_attempted'].includes(t.remote_read)) &&
        utc(t.started_at) &&
        utc(t.finished_at) &&
        Date.parse(t.started_at) <= Date.parse(t.finished_at),
      'invalid execution task',
    );
  for (const t of doc.tasks)
    check(
      t.result !== 'remote_hit' || t.remote_status === 200,
      'remote_hit has no accepted GET',
    );
  for (const a of doc.store_attempts)
    check(
      typeof a.task === 'string' &&
        hashOK(a.hash) &&
        a.os === doc.os &&
        doc.tasks.some(
          (t) =>
            t.task === a.task &&
            t.hash === a.hash &&
            t.cacheable &&
            t.result !== 'failed',
        ) &&
        utc(a.attempted_at) &&
        ['stored', 'throttled', 'failed'].includes(a.status) &&
        (a.http_status === null || Number.isInteger(a.http_status)),
      'invalid store attempt',
    );
  return doc;
}

async function report(opts) {
  try {
    rmSync(join(opts.out, 'executions.json'), { force: true });
    rmSync(join(opts.out, 'unavailable.json'), { force: true });
    if (existsSync(join(state(), 'proxy.json'))) {
      const p = json(join(state(), 'proxy.json'));
      await fetch(`${p.url}/stop`, {
        headers: { authorization: p.secret },
        signal: AbortSignal.timeout(5000),
      });
    }
    const calls = [
      ...new Map(
        lines(join(state(), 'requests.jsonl')).map((e) => [e.id, e]),
      ).values(),
    ];
    const entries = new Map();
    mkdirSync(opts.out, { recursive: true });
    for (const r of calls.filter((r) => r.http_status === 200 && r.body)) {
      const source = r.method === 'PUT' ? 'stored' : 'replayed';
      const previous = entries.get(r.hash);
      if (!previous || source === 'stored') {
        copyFileSync(
          join(state(), 'bodies', r.body),
          join(opts.out, `${r.hash}.tar`),
        );
        entries.set(r.hash, { hash: r.hash, bytes: r.bytes, source });
      }
    }
    const stored = [...entries.values()].filter((e) => e.source === 'stored');
    save(join(opts.out, 'manifest.json'), {
      schema: 1,
      job: opts.job,
      entries: [...entries.values()],
      stored_bytes: stored.reduce((sum, e) => sum + e.bytes, 0),
    });
    summary(
      `Nx cache: stored ${stored.length} entries, ${stored.reduce((sum, e) => sum + e.bytes, 0)} bytes; replayed ${[...entries.values()].filter((e) => e.source === 'replayed').length} entries`,
    );
    check(
      lines(join(state(), 'errors.jsonl')).length === 0,
      'Nx run telemetry is incomplete',
    );
    const invocations = lines(join(state(), 'invocations.jsonl'));
    const begun = lines(join(state(), 'started.jsonl'));
    check(
      invocations.length > 0 || begun.length > 0,
      'Nx run telemetry is unavailable',
    );
    const incomplete =
      invocations.length === 0 ||
      begun.length !== invocations.length ||
      begun.some((b) => !invocations.some((i) => i.id === b.id));
    const cancelledInvocations = begun
      .filter((b) => !invocations.some((i) => i.id === b.id))
      .map((b) => ({ id: b.id, argv: b.argv || [] }));
    const event = json(env.GITHUB_EVENT_PATH);
    const tasks = [],
      attempts = [];
    for (const invocation of invocations)
      for (const t of invocation.tasks) {
        const gets = calls.filter(
          (r) =>
            !t.status.startsWith('local-cache') &&
            r.hash === t.hash &&
            r.method === 'GET' &&
            r.attempted_at >= invocation.started_at &&
            r.attempted_at <= invocation.finished_at,
        );
        const puts = calls.filter(
          (r) =>
            !t.status.startsWith('local-cache') &&
            t.code === 0 &&
            r.hash === t.hash &&
            r.method === 'PUT' &&
            r.attempted_at >= invocation.started_at &&
            r.attempted_at <= invocation.finished_at,
        );
        const remote = gets.find((r) => r.http_status === 200);
        const result =
          t.code !== 0
            ? 'failed'
            : t.status.startsWith('local-cache')
              ? 'local_hit'
              : t.status === 'remote-cache' && remote
                ? 'remote_hit'
                : 'executed';
        check(
          t.status !== 'remote-cache' || remote,
          `no proxy observation for remote hit ${t.task}`,
        );
        tasks.push({
          task: t.task,
          hash: t.hash,
          cacheable: t.cacheable,
          result,
          remote_status: gets.at(-1)?.http_status ?? null,
          remote_read: remoteRead(gets),
          started_at: t.started_at,
          finished_at: t.finished_at,
        });
        for (const p of puts)
          attempts.push({
            task: t.task,
            hash: t.hash,
            os: process.platform,
            attempted_at: p.attempted_at,
            status:
              p.http_status === 200
                ? 'stored'
                : p.http_status === 429
                  ? 'throttled'
                  : 'failed',
            http_status: p.http_status,
          });
        if (t.cacheable && result === 'executed' && puts.length === 0)
          attempts.push({
            task: t.task,
            hash: t.hash,
            os: process.platform,
            attempted_at: t.finished_at,
            status: 'failed',
            http_status: null,
          });
      }
    const doc = {
      schema: 2,
      ...(incomplete ? { cancelled_invocations: cancelledInvocations } : {}),
      base_sha: event.pull_request ? mergeRefBase() : null,
      repository: env.GITHUB_REPOSITORY,
      ref: env.GITHUB_REF,
      pr: event.pull_request?.number ?? null,
      head_sha: event.pull_request?.head.sha ?? env.GITHUB_SHA,
      run_id: Number(env.GITHUB_RUN_ID),
      attempt: Number(env.GITHUB_RUN_ATTEMPT),
      job: opts.job,
      os: process.platform,
      tasks,
      store_attempts: attempts,
      selection: event.pull_request
        ? { mode: 'affected', base_sha: env.NX_BASE, head_sha: env.NX_HEAD }
        : { mode: 'all' },
    };
    validateExecutions(doc);
    save(join(opts.out, 'executions.json'), doc);
    check(
      !incomplete,
      invocations.length
        ? 'Nx run telemetry is incomplete'
        : 'Nx run telemetry is unavailable',
    );
  } catch (error) {
    warning(`Nx cache report missing data: ${error.message}`);
    try {
      save(join(opts.out, 'unavailable.json'), {
        schema: 1,
        reason: error.message,
      });
    } catch (writeError) {
      warning(`Cannot write unavailable cache evidence: ${writeError.message}`);
    }
  }
}

class Github {
  constructor(repo) {
    check(/^[\w.-]+\/[\w.-]+$/.test(repo), 'repository must be OWNER/NAME');
    this.api = env.GITHUB_API_URL || 'https://api.github.com';
    const url = new URL(this.api);
    check(
      url.protocol === 'https:' ||
        (url.protocol === 'http:' &&
          ['127.0.0.1', 'localhost'].includes(url.hostname)),
      'GitHub API must use HTTPS or loopback HTTP',
    );
    this.repo = repo;
    this.requests = 0;
    this.downloads = 0;
    this.selectionComparisons = new Set();
    this.root = join(
      env.RUNNER_TEMP || '.nx',
      'nx-cache-evidence',
      repo.replace('/', '__'),
    );
  }
  read(endpoint, binary = false) {
    const path = join(
      this.root,
      createHash('sha256').update(endpoint).digest('hex') +
        (binary ? '.zip' : '.json'),
    );
    if (!existsSync(path)) {
      // llmlint: ignore[scripts_runtime_independent] The budget contract requires read-only gh API pagination with the runner authentication; gh is provided on the GitHub-hosted runner, with no authoring dependency.
      const body = execFileSync(
        'gh',
        [
          'api',
          '--method',
          'GET',
          `${this.api.replace(/\/$/, '')}/${endpoint}`,
        ],
        { maxBuffer: 128 * 1024 * 1024, timeout: 60000 },
      );
      mkdirSync(dirname(path), { recursive: true });
      writeFileSync(path, body);
      this.requests++;
      if (binary) this.downloads++;
    }
    return binary ? readFileSync(path) : json(path);
  }
  pages(endpoint, key) {
    let page = 1,
      out = [];
    const seen = new Set();
    while (page <= 1000) {
      const doc = this.read(
        endpoint +
          (endpoint.includes('?') ? '&' : '?') +
          `per_page=100&page=${page}`,
      );
      const values = key ? doc[key] : doc;
      check(Array.isArray(values), `invalid paginated response ${endpoint}`);
      const fingerprint = createHash('sha256')
        .update(JSON.stringify(values))
        .digest('hex');
      check(
        !seen.has(fingerprint),
        `repeated pagination page ${page}: ${endpoint}`,
      );
      seen.add(fingerprint);
      out.push(...values);
      if (values.length < 100) return out;
      page++;
    }
    throw new Error(`pagination exceeds 1000 pages: ${endpoint}`);
  }
}

// GitHub artifacts are ZIP archives. Read the central directory, reject paths
// and duplicate names, and inflate only regular, unencrypted entries in memory.
function zipFiles(buffer) {
  const files = new Map();
  let end = -1;
  for (let i = buffer.length - 22; i >= Math.max(0, buffer.length - 65557); i--)
    if (buffer.readUInt32LE(i) === 0x06054b50) {
      end = i;
      break;
    }
  check(end >= 0, 'artifact is not a ZIP archive');
  const count = buffer.readUInt16LE(end + 10);
  let at = buffer.readUInt32LE(end + 16);
  for (let i = 0; i < count; i++) {
    check(
      buffer.readUInt32LE(at) === 0x02014b50,
      'invalid ZIP central directory',
    );
    const flags = buffer.readUInt16LE(at + 8),
      method = buffer.readUInt16LE(at + 10),
      size = buffer.readUInt32LE(at + 20),
      plainSize = buffer.readUInt32LE(at + 24),
      n = buffer.readUInt16LE(at + 28),
      extra = buffer.readUInt16LE(at + 30),
      comment = buffer.readUInt16LE(at + 32),
      local = buffer.readUInt32LE(at + 42);
    const name = buffer.subarray(at + 46, at + 46 + n).toString('utf8');
    at += 46 + n + extra + comment;
    check(!flags || !(flags & 1), 'encrypted ZIP artifact');
    check(
      !name.includes('..') &&
        !name.startsWith('/') &&
        !name.includes('\\') &&
        !files.has(name),
      'unsafe or duplicate artifact path',
    );
    check(
      plainSize <= 128 * 1024 * 1024 && size <= 128 * 1024 * 1024,
      'artifact entry exceeds size bound',
    );
    check(buffer.readUInt32LE(local) === 0x04034b50, 'invalid ZIP local entry');
    const bodyAt =
      local +
      30 +
      buffer.readUInt16LE(local + 26) +
      buffer.readUInt16LE(local + 28);
    const compressed = buffer.subarray(bodyAt, bodyAt + size);
    const body =
      method === 0
        ? compressed
        : method === 8
          ? inflateRawSync(compressed, { maxOutputLength: 128 * 1024 * 1024 })
          : null;
    check(
      body && body.length === plainSize,
      'invalid ZIP entry size or compression',
    );
    files.set(name, body);
  }
  return files;
}
function fileJson(files, name) {
  check(files.has(name), `artifact missing ${name}`);
  return JSON.parse(files.get(name).toString('utf8'));
}
function validateManifest(doc, files, job) {
  check(
    doc?.schema === 1 &&
      doc.job === job &&
      Array.isArray(doc.entries) &&
      Number.isSafeInteger(doc.stored_bytes),
    'invalid manifest',
  );
  const hashes = new Set();
  let bytes = 0;
  for (const entry of doc.entries) {
    check(
      hashOK(entry.hash) &&
        !hashes.has(entry.hash) &&
        Number.isSafeInteger(entry.bytes) &&
        entry.bytes >= 0 &&
        ['stored', 'replayed'].includes(entry.source),
      'invalid manifest entry',
    );
    hashes.add(entry.hash);
    const body = files.get(`${entry.hash}.tar`);
    check(
      body && body.length === entry.bytes,
      'manifest body missing or has wrong size',
    );
    if (entry.source === 'stored') bytes += entry.bytes;
  }
  check(bytes === doc.stored_bytes, 'manifest stored_bytes mismatch');
  return doc;
}
function artifactIdentity(artifact, run) {
  check(!artifact.expired, `expired artifact ${artifact.name}`);
  check(
    artifact.workflow_run?.id === run.id &&
      artifact.workflow_run.head_sha === run.head_sha,
    `artifact ${artifact.name} does not match source run/head`,
  );
  check(
    artifact.workflow_run.head_repository_id === run.head_repository.id,
    `artifact ${artifact.name} has wrong head repository`,
  );
}
function attemptRun(gh, id, attempt) {
  const run = gh.read(
    `repos/${gh.repo}/actions/runs/${id}/attempts/${attempt}`,
  );
  check(
    run.id === id && run.run_attempt === attempt,
    'workflow attempt identity mismatch',
  );
  return run;
}
function artifactsFor(
  gh,
  run,
  detail = [],
  allowEmptyJobs = false,
  unavailable = { count: 0 },
) {
  const all = gh.pages(
    `repos/${gh.repo}/actions/runs/${run.id}/artifacts`,
    'artifacts',
  );
  const selected = all.filter(
    (a) =>
      a.name.startsWith('nx-cache-entries-') &&
      Date.parse(a.created_at) >= Date.parse(run.run_started_at) &&
      Date.parse(a.created_at) <= Date.parse(run.updated_at),
  );
  const jobs = gh.pages(
    `repos/${gh.repo}/actions/runs/${run.id}/attempts/${run.run_attempt}/jobs`,
    'jobs',
  );
  if (allowEmptyJobs && jobs.length === 0) {
    detail.push(
      `run ${run.id}/${run.run_attempt} started no job (conclusion: ${run.conclusion})`,
    );
    return [];
  }
  // Evidence nothing could have kept: nothing here was uploaded and validated, so
  // there is nothing to distrust, and an artifact that is present is read below.
  if (run.predates_adoption && selected.length === 0) {
    detail.push(
      `unavailable: run ${run.id}/${run.run_attempt} predates this repository's cache telemetry, adopted ${run.predates_adoption}`,
    );
    unavailable.count++;
    return [];
  }
  const expected = jobs.filter(
    (j) => j.name === 'lint' || /^test-(linux|windows|macos)$/.test(j.name),
  );
  check(
    expected.length > 0,
    `run ${run.id}/${run.run_attempt} has no expected CI jobs`,
  );
  check(
    run.event === 'pull_request' && run.head_repository.full_name === gh.repo,
    'untrusted source run event/repository',
  );
  const records = [];
  for (const job of expected) {
    const cancelled =
      job.conclusion === 'cancelled' ||
      (run.conclusion === 'cancelled' &&
        job.conclusion !== 'failure' &&
        job.conclusion !== 'skipped');
    check(job.conclusion !== 'skipped', `missing expected job ${job.name}`);
    const candidates = selected.filter(
      (a) => a.name === `nx-cache-entries-${job.name}`,
    );
    if (cancelled && candidates.length === 0) {
      detail.push(
        `cancelled run ${run.id}/${run.run_attempt} job ${job.name}: no artifact`,
      );
      continue;
    }
    // A whole-workflow re-run removes every artifact of the attempts before it, so one
    // job missing beside another's present artifact is not that, and still refuses.
    if (run.superseded && selected.length === 0) {
      detail.push(
        `unavailable: run ${run.id}/${run.run_attempt} job ${job.name}: a whole-workflow re-run removed its artifact`,
      );
      unavailable.count++;
      continue;
    }
    check(
      candidates.length > 0,
      `missing artifact for ${run.id}/${run.run_attempt}/${job.name}`,
    );
    let record;
    for (const artifact of candidates) {
      artifactIdentity(artifact, run);
      const files = zipFiles(
        gh.read(`repos/${gh.repo}/actions/artifacts/${artifact.id}/zip`, true),
      );
      if (
        cancelled &&
        !files.has('executions.json') &&
        files.has('unavailable.json')
      ) {
        detail.push(
          `cancelled run ${run.id}/${run.run_attempt} job ${job.name}: telemetry unavailable`,
        );
        continue;
      }
      const execution = validateExecutions(fileJson(files, 'executions.json'));
      check(
        !incompleteTelemetry(execution) || cancelled,
        'Nx run telemetry is incomplete',
      );
      if (execution.schema === 2)
        for (const invocation of execution.cancelled_invocations ?? [])
          detail.push(
            `cancelled run ${run.id}/${run.run_attempt} job ${job.name} invocation ${invocation.id} argv ${JSON.stringify(invocation.argv)}: unfinished tasks are not individually known; excluded from numerator and denominator`,
          );
      validateSelection(gh, execution, run);
      check(
        execution.repository === gh.repo &&
          execution.run_id === run.id &&
          execution.attempt === run.run_attempt &&
          execution.head_sha === run.head_sha &&
          execution.job === job.name &&
          execution.ref === `refs/pull/${execution.pr}/merge`,
        'execution identity mismatches GitHub metadata',
      );
      check(
        associatedPrs(gh, run).some(
          (pr) =>
            pr.number === execution.pr,
        ),
        'execution PR mismatches source workflow',
      );
      const started = Date.parse(run.run_started_at),
        completed = Date.parse(run.updated_at);
      check(
        Number.isFinite(started) &&
          Number.isFinite(completed) &&
          started <= completed,
        'invalid workflow attempt timing',
      );
      check(
        execution.tasks.every(
          (task) =>
            Date.parse(task.started_at) >= started &&
            Date.parse(task.finished_at) <= completed,
        ) &&
          execution.store_attempts.every(
            (attempt) =>
              Date.parse(attempt.attempted_at) >= started &&
              Date.parse(attempt.attempted_at) <= completed,
          ),
        'execution timing mismatches workflow attempt',
      );
      const runner = (job.labels || []).join(' ').toLowerCase();
      const os = /windows/.test(runner)
        ? 'win32'
        : /macos|darwin/.test(runner)
          ? 'darwin'
          : /linux|ubuntu/.test(runner)
            ? 'linux'
            : null;
      check(
        os && execution.os === os,
        'execution OS mismatches workflow runner metadata',
      );
      const manifest = validateManifest(
        fileJson(files, 'manifest.json'),
        files,
        job.name,
      );
      for (const t of execution.tasks.filter((t) => t.result === 'remote_hit'))
        check(
          manifest.entries.some((e) => e.hash === t.hash),
          'remote replay has no recorded body',
        );
      for (const e of manifest.entries)
        check(
          execution.tasks.some(
            (t) =>
              t.hash === e.hash &&
              t.cacheable &&
              (e.source === 'replayed'
                ? t.result === 'remote_hit'
                : execution.store_attempts.some(
                    (a) => a.hash === e.hash && a.status === 'stored',
                  )),
          ),
          'manifest has no matching task evidence',
        );
      if (record)
        check(
          JSON.stringify(record.execution) === JSON.stringify(execution) &&
            JSON.stringify(record.manifest) === JSON.stringify(manifest),
          'conflicting duplicate artifact',
        );
      record = { execution, manifest, files };
    }
    if (record) records.push(record);
  }
  return records;
}
function validateReceipt(doc) {
  check(
    doc?.schema === 1 &&
      typeof doc.repository === 'string' &&
      typeof doc.scope === 'string' &&
      /^[a-f0-9]{40}$/.test(doc.merge_sha) &&
      integer(doc.pr) &&
      integer(doc.source_run_id) &&
      integer(doc.source_attempt) &&
      utc(doc.promoted_at) &&
      Array.isArray(doc.entries) &&
      Array.isArray(doc.attempts),
    'invalid promotion receipt',
  );
  for (const a of doc.attempts)
    check(
      typeof a.task === 'string' &&
        hashOK(a.hash) &&
        ['linux', 'darwin', 'win32'].includes(a.os) &&
        utc(a.attempted_at) &&
        [
          'promoted',
          'already_present',
          'throttled',
          'failed',
          'never_promoted',
        ].includes(a.status) &&
        (a.http_status === null || Number.isInteger(a.http_status)),
      'invalid promotion attempt',
    );
  for (const a of doc.attempts)
    check(
      !['promoted', 'already_present'].includes(a.status) ||
        a.http_status === (a.status === 'promoted' ? 200 : 409),
      'promotion status contradicts HTTP status',
    );
  for (const e of doc.entries)
    check(
      doc.attempts.some(
        (a) =>
          taskKey(a) === taskKey(e) &&
          a.status === e.status &&
          ['promoted', 'already_present'].includes(a.status),
      ),
      'successful receipt entry has no accepted attempt',
    );
  return doc;
}
async function storePut(hash, body) {
  const server = env.NX_SELF_HOSTED_REMOTE_CACHE_SERVER,
    token = env.NX_SELF_HOSTED_REMOTE_CACHE_ACCESS_TOKEN;
  if (!server || !token) return { status: null, present: false };
  try {
    const answer = await fetch(`${server}/v1/cache/${hash}`, {
      method: 'PUT',
      headers: {
        authorization: `Bearer ${token}`,
        'content-type': 'application/octet-stream',
      },
      body,
      signal: AbortSignal.timeout(60000),
    });
    await answer.arrayBuffer();
    const upstreamStatus = answer.headers.get('x-nx-cache-upstream-status');
    const status =
      upstreamStatus === null
        ? answer.status
        : upstreamStatus === 'unavailable'
          ? null
          : Number(upstreamStatus);
    if (status === 409) {
      try {
        const existing = await fetch(`${server}/v1/cache/${hash}`, {
          headers: { authorization: `Bearer ${token}` },
          signal: AbortSignal.timeout(60000),
        });
        const bytes = Buffer.from(await existing.arrayBuffer());
        return {
          status,
          present: existing.status === 200 && bytes.equals(body),
        };
      } catch {
        return { status, present: false };
      }
    }
    return { status, present: false };
  } catch {
    return { status: null, present: false };
  }
}
async function promote(opts) {
  const repo = opts.repo || env.GITHUB_REPOSITORY;
  let receipt;
  try {
    check(
      env.GITHUB_EVENT_NAME === 'push',
      'promotion requires a real push event',
    );
    if (opts.pr) {
      check(
        env.GITHUB_REF === `refs/heads/${PROMOTION_BRANCH_PREFIX}${opts.pr}`,
        'manual promotion branch must name the source PR',
      );
    } else
      check(
        env.GITHUB_REF === 'refs/heads/main',
        'merge promotion requires the main push scope',
      );
    const gh = new Github(repo);
    let pr;
    if (opts.pr) pr = gh.read(`repos/${repo}/pulls/${opts.pr}`);
    else {
      const prs = gh.pages(`repos/${repo}/commits/${opts.sha}/pulls`);
      pr = prs.find((p) => p.merge_commit_sha === opts.sha && p.merged_at);
    }
    if (!pr) {
      summary('Nx cache promotion skipped: commit merged no pull request');
      return;
    }
    if (!trusted(pr, repo)) {
      summary(
        `Nx cache promotion skipped: ${pr.user?.login === 'dependabot[bot]' ? 'Dependabot author' : 'fork head'}`,
      );
      return;
    }
    const runs = gh
      .pages(
        `repos/${repo}/actions/workflows/${encodeURIComponent(opts.workflow)}/runs?event=pull_request&head_sha=${pr.head.sha}`,
        'workflow_runs',
      )
      .filter(
        (r) =>
          r.conclusion === 'success' &&
          r.head_sha === pr.head.sha &&
          r.event === 'pull_request' &&
          r.head_repository.full_name === repo &&
          associatedPrs(gh, r).some((p) => p.number === pr.number),
      )
      .sort((a, b) => b.id - a.id);
    check(runs.length > 0, 'no successful trusted pull_request run');
    const run = attemptRun(gh, runs[0].id, runs[0].run_attempt);
    const records = artifactsFor(gh, run);
    receipt = {
      schema: 1,
      repository: repo,
      scope: env.GITHUB_REF,
      merge_sha: opts.sha || env.GITHUB_SHA,
      pr: pr.number,
      source_run_id: run.id,
      source_attempt: run.run_attempt,
      promoted_at: new Date().toISOString(),
      entries: [],
      attempts: [],
    };
    const bodies = new Map(),
      mapping = new Map();
    for (const r of records) {
      for (const t of r.execution.tasks)
        if (
          t.cacheable &&
          (r.execution.store_attempts.some(
            (a) => a.task === t.task && a.hash === t.hash,
          ) ||
            r.manifest.entries.some((e) => e.hash === t.hash))
        )
          mapping.set(taskKey({ ...t, os: r.execution.os }), {
            task: t.task,
            hash: t.hash,
            os: r.execution.os,
          });
      for (const e of r.manifest.entries) {
        const body = r.files.get(`${e.hash}.tar`);
        if (bodies.has(e.hash))
          check(
            bodies.get(e.hash).equals(body),
            'conflicting bodies for same hash',
          );
        bodies.set(e.hash, body);
      }
    }
    const statuses = new Map();
    let promoted = 0,
      present = 0,
      failed = 0,
      bytes = 0;
    for (const [hash, body] of bodies) {
      const attempted_at = new Date().toISOString();
      const { status, present: confirmedPresent } = await storePut(hash, body);
      statuses.set(hash, {
        status:
          status === 200
            ? 'promoted'
            : confirmedPresent
              ? 'already_present'
              : status === 429
                ? 'throttled'
                : 'failed',
        http_status: status,
        attempted_at,
      });
      if (status === 200) {
        promoted++;
        bytes += body.length;
      } else if (confirmedPresent) present++;
      else failed++;
    }
    failed += new Set(
      [...mapping.values()]
        .filter((t) => !statuses.has(t.hash))
        .map((t) => t.hash),
    ).size;
    for (const t of mapping.values()) {
      const a = {
        ...t,
        ...(statuses.get(t.hash) || {
          status: 'never_promoted',
          http_status: null,
          attempted_at: new Date().toISOString(),
        }),
      };
      receipt.attempts.push(a);
      if (['promoted', 'already_present'].includes(a.status))
        receipt.entries.push({ ...t, status: a.status });
    }
    receipt.promoted_at = new Date().toISOString();
    validateReceipt(receipt);
    save(
      join(env.RUNNER_TEMP || '.nx', 'nx-cache-promotion', 'receipt.json'),
      receipt,
    );
    summary(
      `Nx cache promotion: ${promoted} promoted, ${present} already present, ${failed} failed; ${bytes} bytes`,
    );
    if (failed)
      warning(
        `Nx cache promotion could not promote ${failed} expected hash(es); see receipt attempts`,
      );
  } catch (error) {
    warning(`Nx cache promotion skipped: ${error.message}`);
    summary(`Nx cache promotion skipped: ${error.message}`);
  }
}

function associatedPrs(gh, run) {
  check(
    Array.isArray(run.pull_requests),
    `run ${run.id}/${run.run_attempt} has no PR metadata`,
  );
  if (run.pull_requests.length) return run.pull_requests;
  const prs = gh
    .pages(`repos/${gh.repo}/commits/${run.head_sha}/pulls`)
    .filter(
      (pr) =>
        pr.merged_at &&
        trusted(pr, gh.repo) &&
        pr.head.repo.full_name === run.head_repository.full_name &&
        pr.user.login === run.actor?.login,
    );
  check(
    prs.length > 0,
    `run ${run.id}/${run.run_attempt} cannot be associated with a trusted merged pull request`,
  );
  return prs;
}
function validateSelection(gh, doc, run) {
  const selection = doc.selection;
  check(
    selection?.mode === 'affected' &&
      /^[a-f0-9]{40}$/.test(selection.base_sha) &&
      selection.head_sha === doc.head_sha &&
      selection.head_sha === run.head_sha,
    'invalid affected selection',
  );
  const pr = associatedPrs(gh, run).find((pr) => pr.number === doc.pr);
  check(
    pr &&
      (run.pull_requests.length
        ? pr.head?.repo?.url === `https://api.github.com/repos/${gh.repo}` &&
          pr.base?.repo?.url === `https://api.github.com/repos/${gh.repo}`
        : pr.head?.repo?.full_name === gh.repo &&
          pr.base?.repo?.full_name === gh.repo),
    'untrusted source PR repository',
  );
  const membership = gh.pages(`repos/${gh.repo}/commits/${run.head_sha}/pulls`);
  check(
    membership.some(
      (p) =>
        p.number === doc.pr &&
        p.head?.repo?.full_name === gh.repo &&
        p.base?.repo?.full_name === gh.repo,
    ),
    'run head is not a commit of the source PR',
  );
  if (run.pull_requests.length) {
    // The run's commit and the base branch retain ancestry after later pushes;
    // the live association's head/base SHAs do not describe this run anymore.
    for (const [tip, message] of [
      [run.head_sha, 'selection base is not an ancestor of the run head'],
      [pr.base.ref, 'selection base is not on the source PR base branch'],
    ]) {
      check(
        typeof tip === 'string' && tip.length > 0,
        'missing selection ancestry tip',
      );
      const endpoint = `repos/${gh.repo}/compare/${selection.base_sha}...${encodeURIComponent(tip)}`;
      const ancestry = gh.read(endpoint);
      gh.selectionComparisons.add(endpoint);
      if (tip === run.head_sha && ['behind', 'diverged'].includes(ancestry.status)) {
        // Nx selects from this merge-base when the event base is newer than
        // the PR's branch point. GitHub supplies it in the same bounded read.
        check(
          /^[a-f0-9]{40}$/.test(ancestry.merge_base_commit?.sha),
          'missing effective selection merge-base',
        );
      } else {
        check(['ahead', 'identical'].includes(ancestry.status), message);
      }
    }
  } else {
    const merge = gh.read(`repos/${gh.repo}/commits/${pr.merge_commit_sha}`);
    const parent = merge.parents?.[0]?.sha;
    check(
      /^[a-f0-9]{40}$/.test(parent),
      'merge commit has no first parent for selection validation',
    );
    const ancestry = gh.read(
      `repos/${gh.repo}/compare/${selection.base_sha}...${parent}`,
    );
    check(
      ['ahead', 'identical'].includes(ancestry.status),
      'selection base is not an ancestor of the merged PR base',
    );
  }
}

function commitChangesPublished(sha) {
  check(/^[a-f0-9]{40}$/.test(sha), 'missing merge commit SHA');
  const specs = readFileSync('.github/release-published-paths', 'utf8')
    .split(/\r?\n/)
    .filter((line) => line.trim() && !/^\s*#/.test(line));
  check(specs.length > 0, 'empty .github/release-published-paths');
  try {
    return (
      // llmlint: ignore[scripts_runtime_independent] The published-path contract reads merge diffs from the consumer checkout with git; git is provided on the GitHub-hosted runner, with no authoring dependency.
      execFileSync(
        'git',
        ['diff', '--name-only', `${sha}^1`, sha, '--', ...specs],
        { encoding: 'utf8' },
      ).trim().length > 0
    );
  } catch {
    throw new Error(`merge ${sha} unavailable: checkout with fetch-depth: 0`);
  }
}
function receiptRecords(gh, pr, sourceRuns, end, scope, predatesAdoption = false) {
  const pushes = gh.pages(
    `repos/${gh.repo}/actions/workflows/release-plz.yml/runs?head_sha=${pr.merge_commit_sha}`,
    'workflow_runs',
  );
  const receipts = [];
  for (const run of pushes.filter(
    (run) =>
      run.head_repository.full_name === gh.repo &&
      run.head_sha === pr.merge_commit_sha &&
      run.event === 'push' &&
      `refs/heads/${run.head_branch}` === scope &&
      Date.parse(run.updated_at) <= end,
  )) {
    const artifacts = gh
      .pages(`repos/${gh.repo}/actions/runs/${run.id}/artifacts`, 'artifacts')
      .filter((a) => a.name === `nx-cache-promotion-${pr.merge_commit_sha}`);
    if (predatesAdoption && artifacts.length === 0) continue;
    check(artifacts.length > 0, `missing promotion receipt for run ${run.id}`);
    for (const artifact of artifacts) {
      artifactIdentity(artifact, run);
      const receipt = validateReceipt(
        fileJson(
          zipFiles(
            gh.read(
              `repos/${gh.repo}/actions/artifacts/${artifact.id}/zip`,
              true,
            ),
          ),
          'receipt.json',
        ),
      );
      check(
        receipt.repository === gh.repo &&
          receipt.pr === pr.number &&
          receipt.merge_sha === pr.merge_commit_sha &&
          receipt.scope === scope &&
          `refs/heads/${run.head_branch}` === receipt.scope,
        'untrusted promotion scope/repository/PR',
      );
      const source = sourceRuns.find(
        (r) =>
          r.id === receipt.source_run_id &&
          r.run_attempt === receipt.source_attempt,
      );
      check(source, 'receipt references an unavailable source run/attempt');
      const sourceHead = source.head_sha;
      const merge = gh.read(`repos/${gh.repo}/commits/${pr.merge_commit_sha}`);
      check(
        merge.sha === pr.merge_commit_sha && sourceHead === pr.head.sha,
        'promotion source head mismatch',
      );
      receipts.push(receipt);
    }
  }
  return receipts;
}
// When the telemetry reached this repository: the commit adding the script that writes
// it, by committer date. Unknown in a shallow checkout, whose oldest commit is no such
// thing, or for a script history does not hold: nothing then counts as pre-adoption.
function telemetryAdoption() {
  try {
    // llmlint: ignore[scripts_runtime_independent] Adoption is a fact of the consumer checkout's history; git is provided on the GitHub-hosted runner, with no authoring dependency.
    const git = (...args) =>
      execFileSync('git', args, {
        encoding: 'utf8',
        stdio: ['ignore', 'pipe', 'ignore'],
      }).trim();
    if (git('rev-parse', '--is-shallow-repository') === 'true') return null;
    const added = git(
      'log',
      '--format=%cI',
      '--diff-filter=A',
      '--',
      'scripts/nx-remote-cache.mjs',
    )
      .split('\n')
      .at(-1);
    return added ? new Date(added) : null;
  } catch {
    return null;
  }
}
async function cacheUse(opts) {
  const gh = new Github(opts.repo || env.GITHUB_REPOSITORY);
  const releaseTag =
    opts.release ||
    env.GITHUB_REF_NAME ||
    (env.GITHUB_EVENT_PATH
      ? json(env.GITHUB_EVENT_PATH).release?.tag_name
      : undefined);
  check(releaseTag, 'no release: pass --release TAG');
  const releases = gh
    .pages(`repos/${gh.repo}/releases`)
    .filter((r) => !r.draft);
  const release = releases.find((r) => r.tag_name === releaseTag);
  check(
    release && utc(release.published_at),
    'release not found or unpublished',
  );
  const previous = releases
    .filter(
      (r) => Date.parse(r.published_at) < Date.parse(release.published_at),
    )
    .sort((a, b) => Date.parse(b.published_at) - Date.parse(a.published_at))[0];
  const end = Date.parse(release.published_at),
    start = previous ? Date.parse(previous.published_at) : 0;
  const tip = gh.read(
    `repos/${gh.repo}/commits/${encodeURIComponent(releaseTag)}`,
  );
  const commits = previous
    ? gh.pages(
        `repos/${gh.repo}/compare/${encodeURIComponent(previous.tag_name)}...${encodeURIComponent(releaseTag)}`,
        'commits',
      )
    : gh.pages(
        `repos/${gh.repo}/commits?sha=${encodeURIComponent(releaseTag)}`,
      );
  const prs = new Map();
  let releasePr;
  for (const commit of commits)
    for (const pr of gh.pages(`repos/${gh.repo}/commits/${commit.sha}/pulls`)) {
      if (!pr.merged_at || pr.merge_commit_sha !== commit.sha) continue;
      if (pr.merge_commit_sha === tip.sha) releasePr = pr;
      if (
        pr.merge_commit_sha === tip.sha ||
        commitChangesPublished(pr.merge_commit_sha)
      )
        prs.set(pr.number, pr);
    }
  check(releasePr, 'release commit maps to no release pull request');
  const detail = [
    `Observation interval: ${previous?.published_at || 'first CI run'} exclusive to ${release.published_at} inclusive`,
  ];
  const evidence = [],
    receipts = [],
    mergedExpectations = [],
    seen = new Set();
  let storedBytes = 0,
    localHits = 0,
    promotionFailures = 0,
    unknownBase = 0;
  const unavailable = { count: 0 },
    removedAttempts = new Set(),
    latestRuns = new Map(),
    adoption = telemetryAdoption();
  for (const pr of prs.values()) {
    check(trusted(pr, gh.repo), `untrusted counted PR #${pr.number}`);
    const runs = gh
      .pages(
        `repos/${gh.repo}/actions/workflows/ci.yml/runs?event=pull_request&branch=${encodeURIComponent(pr.head.ref)}`,
        'workflow_runs',
      )
      .filter(
        (r) =>
          r.head_repository.full_name === gh.repo &&
          associatedPrs(gh, r).some((p) => p.number === pr.number),
      );
    const attempts = [];
    for (const run of runs)
      for (let attempt = 1; attempt <= run.run_attempt; attempt++) {
        const r = attemptRun(gh, run.id, attempt);
        const completed = Date.parse(r.updated_at);
        if (completed > end) {
          detail.push(
            `left out run ${r.id}/${attempt}: completed after ${release.published_at}`,
          );
          continue;
        }
        r.in_interval = completed > start;
        check(r.status === 'completed', `run ${r.id}/${attempt} incomplete`);
        r.superseded = attempt < run.run_attempt;
        if (adoption && Date.parse(r.created_at ?? r.run_started_at) < adoption)
          r.predates_adoption = adoption.toISOString();
        attempts.push(r);
      }
    const predatesAdoption =
      adoption !== null &&
      pr.number !== releasePr.number &&
      (attempts.length
        ? attempts.every((r) => r.predates_adoption)
        : Date.parse(pr.merged_at) < adoption);
    if (predatesAdoption && attempts.length === 0) {
      detail.push(
        `unavailable: #${pr.number} merged before this repository's cache telemetry, adopted ${adoption.toISOString()}, with no CI run left to read`,
      );
      unavailable.count++;
      continue;
    }
    if (pr.number === releasePr.number) {
      const anchor = attempts
        .filter(
          (r) =>
            r.conclusion === 'success' &&
            r.head_sha === pr.head.sha &&
            Date.parse(r.updated_at) <= Date.parse(pr.merged_at),
        )
        .sort((a, b) => Date.parse(b.updated_at) - Date.parse(a.updated_at))[0];
      check(
        anchor,
        `release PR #${pr.number} has no final successful CI attempt`,
      );
      // Older release-PR runs establish eligibility but do not measure the
      // release tail. All ordinary PR retries/pushes remain observations.
      for (const r of attempts)
        r.observe =
          r.in_interval &&
          Date.parse(r.updated_at) >= Date.parse(anchor.updated_at);
    } else for (const r of attempts) r.observe = r.in_interval;
    check(attempts.length > 0, `missing CI attempts for #${pr.number}`);
    for (const run of attempts) {
      const records = artifactsFor(gh, run, detail, true, unavailable);
      if (run.superseded && records.length === 0)
        removedAttempts.add(`${run.id}/${run.run_attempt}`);
      for (const record of records) {
        validateSelection(gh, record.execution, run);
        storedBytes += record.manifest.stored_bytes;
        evidence.push({
          ...record.execution,
          manifest: record.manifest,
          observe: run.observe,
          release_pr: pr.number === releasePr.number,
        });
      }
    }
    const latest = attempts
      .filter(
        (r) =>
          r.head_sha === pr.head.sha &&
          Date.parse(r.updated_at) <= Date.parse(pr.merged_at),
      )
      .sort((a, b) => Date.parse(b.updated_at) - Date.parse(a.updated_at))[0];
    check(latest, `missing latest merged PR run #${pr.number}`);
    const latestEvidence = evidence.filter(
      (e) =>
        e.pr === pr.number &&
        e.run_id === latest.id &&
        e.attempt === latest.run_attempt,
    );
    if (pr.number !== releasePr.number) latestRuns.set(pr.number, latestEvidence);
    for (const e of latestEvidence) {
      for (const a of e.store_attempts)
        mergedExpectations.push({
          ...a,
          pr: pr.number,
          merge_sha: pr.merge_commit_sha,
          source_status: a.status,
          scope: `refs/heads/${pr.base.ref}`,
          status: 'never_promoted',
          base_scope: true,
          run_id: e.run_id,
          attempt: e.attempt,
          attempted_at: a.attempted_at,
          available_from: pr.merged_at,
        });
      for (const t of e.tasks.filter(
        (t) =>
          t.cacheable &&
          t.result === 'remote_hit' &&
          e.manifest.entries.some((m) => m.hash === t.hash) &&
          !e.store_attempts.some((a) => a.task === t.task && a.hash === t.hash),
      )) {
        mergedExpectations.push({
          task: t.task,
          hash: t.hash,
          os: e.os,
          pr: pr.number,
          merge_sha: pr.merge_commit_sha,
          source_status: 'replayed',
          status: 'never_promoted',
          scope: `refs/heads/${pr.base.ref}`,
          http_status: 200,
          base_scope: true,
          run_id: e.run_id,
          attempt: e.attempt,
          attempted_at: t.finished_at,
          available_from: pr.merged_at,
        });
      }
    }
    receipts.push(
      ...receiptRecords(
        gh,
        pr,
        attempts,
        end,
        `refs/heads/${pr.base.ref}`,
        predatesAdoption,
      ),
    );
  }
  const stores = evidence.flatMap((e) =>
    e.store_attempts.map((a) => ({
      ...a,
      pr: e.pr,
      run_id: e.run_id,
      attempt: e.attempt,
      base_scope: false,
    })),
  );
  stores.push(...mergedExpectations);
  for (const receipt of receipts)
    for (const a of receipt.attempts) {
      const source = evidence.filter(
        (e) =>
          e.run_id === receipt.source_run_id &&
          e.attempt === receipt.source_attempt &&
          e.pr === receipt.pr,
      );
      // A re-run after the merge removed the attempt this receipt names, whose receipt
      // is the promotion's own record and still says what it wrote.
      check(
        removedAttempts.has(
          `${receipt.source_run_id}/${receipt.source_attempt}`,
        ) ||
          source.some(
            (e) =>
              e.os === a.os &&
              e.tasks.some(
                (t) => t.task === a.task && t.hash === a.hash && t.cacheable,
              ) &&
              (e.store_attempts.some((s) => taskKey(s) === taskKey(a)) ||
                e.manifest.entries.some((m) => m.hash === a.hash)),
          ),
        'promotion receipt task/OS/hash has no source expectation',
      );
      check(
        Date.parse(a.attempted_at) <= Date.parse(receipt.promoted_at),
        'promotion attempt follows receipt time',
      );
      if (!['promoted', 'already_present'].includes(a.status))
        promotionFailures++;
      stores.push({
        ...a,
        pr: receipt.pr,
        run_id: receipt.source_run_id,
        attempt: receipt.source_attempt,
        base_scope: true,
        scope: receipt.scope,
      });
    }
  // The carried run that tested this task and OS at another hash on a moved base.
  function baseMoved(t, e) {
    let found = null;
    for (const [number, runs] of latestRuns)
      for (const run of runs) {
        const other = run.tasks.find(
          (o) => o.task === t.task && o.hash !== t.hash,
        );
        if (!other || run.os !== e.os) continue;
        if (typeof run.base_sha !== 'string') {
          unknownBase++;
          continue;
        }
        const landed = gh.read(
          `repos/${gh.repo}/commits/${prs.get(number).merge_commit_sha}`,
        ).parents?.[0]?.sha;
        check(
          /^[a-f0-9]{40}$/.test(landed),
          `merge commit of #${number} has no first parent`,
        );
        if (run.base_sha !== landed)
          found ??= { pr: number, at: other.hash, tested: run.base_sha, landed };
      }
    return found;
  }
  let hits = 0,
    eligible = 0;
  const groups = {
    release: { hits: 0, total: 0 },
    retry: { hits: 0, total: 0 },
  };
  for (const e of evidence)
    for (const t of e.tasks) {
      const id = [e.run_id, e.attempt, e.job, t.task, e.os, t.hash].join(':');
      if (seen.has(id)) continue;
      seen.add(id);
      if (!e.observe || !t.cacheable) continue;
      if (t.result === 'local_hit') {
        localHits++;
        continue;
      }
      const key = taskKey({ ...t, os: e.os });
      const prior = stores
        .filter(
          (a) =>
            taskKey(a) === key &&
            Date.parse(a.attempted_at) < Date.parse(t.started_at) &&
            (!a.available_from ||
              Date.parse(a.available_from) < Date.parse(t.started_at)) &&
            (a.base_scope || a.pr === e.pr) &&
            !(
              a.run_id === e.run_id &&
              a.attempt === e.attempt &&
              !a.base_scope
            ),
        )
        .sort(
          (a, b) => Date.parse(b.attempted_at) - Date.parse(a.attempted_at),
        );
      const applicable = prior.filter((a) => {
        if (a.pr === e.pr) return true;
        if (a.scope !== `refs/heads/${prs.get(e.pr).base.ref}`) return false;
        const source = prs.get(a.pr);
        const compare = gh.read(
          `repos/${gh.repo}/compare/${source.merge_commit_sha}...${e.selection.base_sha}`,
        );
        return ['ahead', 'identical'].includes(compare.status);
      });
      if (!applicable.length) {
        // The release pull request's hash is one no carried run proved. That is a miss
        // when a carried run proved this task at another hash against a base its merge
        // did not land on, since the release is then re-testing what that run would
        // have proved. A changed hash with its base unmoved is new work, as is a run
        // whose base is unknown, which the detail counts.
        if (e.release_pr && t.result !== 'remote_hit') {
          const moved = baseMoved(t, e);
          if (moved) {
            eligible++;
            groups.release.total++;
            detail.push(
              `miss ${t.task}/${e.os}/${t.hash}: base_moved, #${moved.pr} tested ${moved.at} on base ${moved.tested}, merged on ${moved.landed}`,
            );
          }
        }
        continue;
      }
      eligible++;
      const group = groups[e.release_pr ? 'release' : 'retry'];
      group.total++;
      if (t.result === 'remote_hit' && t.remote_status === 200) {
        hits++;
        group.hits++;
        continue;
      }
      // An inferred missing promotion cannot erase a recorded store/promotion
      // attempt in the scope this execution can read. Keep all raw statuses.
      const a =
          applicable.find((attempt) => attempt.status !== 'never_promoted') ||
          applicable[0],
        raw = applicable.map((p) => `${p.status}/${p.http_status}`).join(',');
      const cause =
        a.http_status === 429 || t.remote_status === 429
          ? 'throttled'
          : a.status === 'failed' ||
              a.source_status === 'failed' ||
              a.http_status === null ||
              a.http_status >= 500 ||
              t.remote_status === null ||
              t.remote_status >= 500
            ? 'failed'
            : a.status === 'never_promoted'
              ? 'never_promoted'
              : ['stored', 'promoted', 'already_present'].includes(a.status) &&
                  t.remote_status === 404
                ? 'evicted'
                : 'never_promoted';
      detail.push(
        `miss ${t.task}/${e.os}/${t.hash}: ${cause}, age ${Math.max(0, (Date.parse(t.started_at) - Date.parse(a.attempted_at)) / 1000)}s, attempts [${raw}], GET ${t.remote_status}`,
      );
    }
  check(eligible > 0, 'zero eligible executions');
  const value = (100 * hits) / eligible;
  for (const endpoint of gh.selectionComparisons)
    detail.push(`selection evidence: GET ${endpoint}`);
  detail.push(`aggregate: ${hits}/${eligible} remote hits (${value}%)`);
  for (const [name, g] of Object.entries(groups))
    detail.push(
      `${name === 'release' ? 'release PR' : 'same-PR retry/push'}: ${g.hits}/${g.total} (${g.total ? (100 * g.hits) / g.total : 'n/a'}%)`,
    );
  detail.push(
    `skipped local hits: ${localHits}; unavailable artifacts: ${unavailable.count}; carried runs with an unknown base, excluded: ${unknownBase}; promotion failures: ${promotionFailures}; stored bytes: ${storedBytes}; requests: ${gh.requests}; artifact downloads: ${gh.downloads}`,
  );
  check(env.ONEBUDGETSPEC_RESULT, 'ONEBUDGETSPEC_RESULT is unset');
  save(env.ONEBUDGETSPEC_RESULT, { value, detail: detail.join('\n') });
  console.error(detail.join('\n'));
}

// GitHub's line is a binary 10 GiB, and the figure's gigabytes are decimal.
const GIB = 1073741824;
const GIGABYTE = 1000000000;
// GitHub's stated allowance, for a token the storage-limit read refuses: `actions: read`
// reads the listing but not the limit, and that is every pull request's `budgets` job.
const STATED_ALLOWANCE_GIB = 10;
const RESULT_KEY = /^nx-cache-/;
// The key's trailing hex segments are rust-cache's environment and lockfile hashes.
const familyOf = (key) => key.replace(/(-[0-9a-f]{6,})+$/, '');

function headroomClient(repo) {
  const github = new Github(repo);
  const token = env.GH_TOKEN || env.GITHUB_TOKEN;
  check(token, 'no token: set GH_TOKEN or GITHUB_TOKEN');
  const seen = { requests: 0, lowest: null };
  async function read(path) {
    seen.requests++;
    const answer = await fetch(`${github.api.replace(/\/$/, '')}/repos/${repo}/${path}`, {
      headers: {
        accept: 'application/vnd.github+json',
        authorization: `Bearer ${token}`,
        'x-github-api-version': '2022-11-28',
      },
      signal: AbortSignal.timeout(60000),
    });
    const remaining = Number(answer.headers.get('x-ratelimit-remaining') ?? NaN);
    if (Number.isFinite(remaining))
      seen.lowest = seen.lowest === null ? remaining : Math.min(seen.lowest, remaining);
    return answer;
  }
  async function get(path) {
    const answer = await read(path);
    check(answer.ok, `GET ${path} answered HTTP ${answer.status}`);
    return answer.json();
  }
  return { read, get, seen };
}

// The allowance and where it came from. Only a 403 falls back: a rate-limited 403 also
// refuses the listing read next, and that still writes no figure.
async function storageAllowance({ read }) {
  const path = 'actions/cache/storage-limit';
  const answer = await read(path);
  if (answer.status === 403) {
    await answer.arrayBuffer();
    return {
      gib: STATED_ALLOWANCE_GIB,
      source: `GitHub's stated ${STATED_ALLOWANCE_GIB} GiB allowance; ${path} answered HTTP 403`,
    };
  }
  check(answer.ok, `GET ${path} answered HTTP ${answer.status}`);
  const limit = await answer.json();
  check(
    Number.isFinite(limit.max_cache_size_gb) && limit.max_cache_size_gb > 0,
    'storage limit carries no max_cache_size_gb',
  );
  return {
    gib: limit.max_cache_size_gb,
    source: `max_cache_size_gb ${limit.max_cache_size_gb}`,
  };
}

const LISTING_PAGE = 100;
const LISTING_READS = 900;
// The characters to probe a prefix with: printable ASCII less `%`, which also matches nothing
// and so never narrows. `_` matches any one character, reaching a `%` or non-ASCII key.
const KEY_CHARACTERS = Array.from({ length: 95 }, (_, at) =>
  String.fromCharCode(32 + at),
).filter((character) => character !== '%');

async function readListing(get, { key, sort = 'created_at', direction = 'desc', page = 1 }) {
  const query = new URLSearchParams({
    sort,
    direction,
    per_page: String(LISTING_PAGE),
    page: String(page),
  });
  if (key) query.set('key', key);
  const doc = await get(`actions/caches?${query}`);
  check(
    Array.isArray(doc.actions_caches) && Number.isSafeInteger(doc.total_count),
    `listing page ${page} is malformed`,
  );
  for (const entry of doc.actions_caches)
    check(
      Number.isSafeInteger(entry.id) &&
        typeof entry.key === 'string' &&
        Number.isSafeInteger(entry.size_in_bytes) &&
        Number.isFinite(Date.parse(entry.created_at)),
      `listing page ${page} holds a malformed entry`,
    );
  return doc;
}

// The whole listing with no page offset, for when pages reordered ties. GitHub offers no sort
// on a unique field, so no paged order is total, but `key` narrows by prefix: narrowed until
// an answer holds its whole total, that one response cannot repeat or omit an entry. `_` and
// `%` in a key are wildcards to GitHub, so a prefix can over-match; entries count by id.
async function listByKeyPrefix(get) {
  const entries = new Map();
  const nodes = new Map();
  let reads = 0;
  const read = async (options) => {
    reads++;
    check(reads <= LISTING_READS, `listing is incomplete: over ${LISTING_READS} reads`);
    const doc = await readListing(get, options);
    for (const entry of doc.actions_caches) entries.set(entry.id, entry);
    return doc;
  };
  const nextCharacters = (doc, prefix) => {
    const at = [...prefix].length;
    return doc.actions_caches
      .map((entry) => [...entry.key][at])
      .filter((character) => character !== undefined)
      .map((character) => prefix + character);
  };
  const descend = async (start) => {
    const pending = [start];
    while (pending.length) {
      const prefix = pending.pop();
      if (nodes.has(prefix)) continue;
      const doc = await read({ key: prefix });
      const total = doc.total_count;
      nodes.set(prefix, total);
      if (total > LISTING_PAGE) pending.push(...nextCharacters(doc, prefix));
      else
        check(
          doc.actions_caches.length === total,
          `listing is incomplete: ${doc.actions_caches.length} of ${total} entries read under key ${prefix || '(all)'}`,
        );
    }
  };
  // Whether a split prefix still holds entries no narrower prefix or seen entry accounts for.
  const unaccounted = (prefix) => {
    if (prefix.includes('%')) return false;
    const length = [...prefix].length;
    let held = [...entries.values()].filter((entry) => entry.key === prefix).length;
    for (const [other, total] of nodes)
      if (other.startsWith(prefix) && [...other].length === length + 1) held += total;
    return nodes.get(prefix) > LISTING_PAGE && held < nodes.get(prefix);
  };
  await descend('');
  const complete = () => entries.size >= nodes.get('');
  // A later key an answer never showed: name each character. The Map's iteration takes in
  // the prefixes this adds.
  for (const prefix of nodes.keys()) {
    if (complete()) break;
    if (!unaccounted(prefix)) continue;
    for (const character of KEY_CHARACTERS) await descend(prefix + character);
  }
  // What is left is one key held by many refs, which no prefix divides: it can only be seen.
  for (const prefix of [...nodes.keys()]) {
    for (const sort of ['created_at', 'last_accessed_at', 'size_in_bytes'])
      for (const direction of ['desc', 'asc'])
        for (let page = 1; page <= Math.ceil(nodes.get(prefix) / LISTING_PAGE); page++) {
          if (complete() || !unaccounted(prefix)) break;
          await read({ key: prefix, sort, direction, page });
        }
  }
  check(
    complete(),
    `listing is incomplete: ${entries.size} of ${nodes.get('')} entries read`,
  );
  return [...entries.values()];
}

// Every entry of the listing, or a thrown reason: a page not read is entries of any kind
// not counted, and a figure over the rest would be wrong. Pages run by created_at, which
// never changes and carries microseconds, so they usually complete in the fewest reads.
// Sizes tie, and GitHub reorders ties between pages; last_accessed_at moves as runs read.
async function listAllCaches(get) {
  const entries = new Map();
  for (let page = 1; page <= 1000; page++) {
    const doc = await readListing(get, { page });
    for (const entry of doc.actions_caches) entries.set(entry.id, entry);
    if (doc.actions_caches.length < LISTING_PAGE)
      return entries.size >= doc.total_count ? [...entries.values()] : listByKeyPrefix(get);
  }
  throw new Error('listing exceeds 1000 pages');
}

async function cacheHeadroom(args) {
  const families = [];
  let repo = env.GITHUB_REPOSITORY;
  for (let at = 0; at < args.length; at += 2) {
    const value = args[at + 1];
    if (!['--family', '--repo'].includes(args[at]))
      usage(`invalid argument ${args[at]}`);
    if (!value || value.startsWith('--')) usage(`${args[at]} needs a value`);
    if (args[at] === '--family') families.push(value);
    else repo = value;
  }
  if (!/^[\w.-]+\/[\w.-]+$/.test(repo ?? ''))
    usage('--repo must be OWNER/NAME, or set GITHUB_REPOSITORY');
  const client = headroomClient(repo);
  const { seen } = client;
  const limit = await storageAllowance(client);
  const entries = await listAllCaches(client.get);
  const allowance = limit.gib * GIB;
  // An entry a longer prefix also names belongs to that one.
  const named = new Map(families.map((prefix) => [prefix, []]));
  const byLength = [...named.keys()].sort((a, b) => b.length - a.length);
  const others = [];
  for (const entry of entries) {
    const prefix = byLength.find((name) => entry.key.startsWith(name));
    (prefix === undefined ? others : named.get(prefix)).push(entry);
  }
  const counted = [...others];
  const familyLines = [];
  const overfull = [];
  for (const [prefix, members] of named) {
    const [newest, ...older] = [...members].sort(
      (a, b) => Date.parse(b.created_at) - Date.parse(a.created_at) || b.id - a.id,
    );
    if (newest) counted.push(newest);
    const olderBytes = older.reduce((sum, entry) => sum + entry.size_in_bytes, 0);
    familyLines.push(
      `family ${prefix}: newest ${newest?.key ?? 'none'}; ${older.length} older ${older.length === 1 ? 'copy' : 'copies'}, ${olderBytes} bytes, not counted`,
    );
    if (members.length > 2) overfull.push([prefix, members.length]);
  }
  const results = entries.filter((entry) => RESULT_KEY.test(entry.key));
  const resultBytes = results.reduce((sum, entry) => sum + entry.size_in_bytes, 0);
  const nonResults = new Map();
  for (const entry of counted.filter((entry) => !RESULT_KEY.test(entry.key))) {
    const family = familyOf(entry.key);
    nonResults.set(family, (nonResults.get(family) ?? 0) + entry.size_in_bytes);
  }
  const used = counted.reduce((sum, entry) => sum + entry.size_in_bytes, 0);
  const oldest = results.reduce(
    (at, entry) => Math.min(at, Date.parse(entry.created_at)),
    Infinity,
  );
  const measured = Math.round(((allowance - used) / GIGABYTE) * 1000) / 1000;
  const detail = [
    `allowance: ${allowance} bytes (${limit.source}), ${allowance / GIGABYTE} GB; counted ${used} bytes`,
    ...[...nonResults]
      .sort((a, b) => b[1] - a[1])
      .map(([family, bytes]) => `non-result ${family}: ${bytes} bytes`),
    ...familyLines,
    ...overfull.map(
      ([prefix, count]) =>
        `family ${prefix} holds ${count} entries, more than the two a working cleanup leaves: figure 0, measured ${measured}`,
    ),
    `results: ${results.length} entries, ${resultBytes} bytes; oldest result age: ${results.length ? `${Math.max(0, Math.round((Date.now() - oldest) / 1000))}s` : 'n/a'}`,
    `requests: ${seen.requests}; lowest X-RateLimit-Remaining: ${seen.lowest ?? 'not reported'}`,
  ].join('\n');
  check(env.ONEBUDGETSPEC_RESULT, 'ONEBUDGETSPEC_RESULT is unset');
  save(env.ONEBUDGETSPEC_RESULT, {
    value: overfull.length ? 0 : measured,
    detail,
  });
  console.error(detail);
}

async function main() {
  const [command, ...args] = process.argv.slice(2);
  try {
    switch (command) {
      case 'trust':
        options(args, []);
        trust();
        break;
      case 'ready':
        await ready(options(args, ['timeout-seconds']));
        break;
      case 'report':
        await report(options(args, ['job', 'out'], ['job', 'out']));
        break;
      case 'cache-use':
        {
          const opts = options(args, ['release', 'repo']);
          if (opts.repo && !/^[\w.-]+\/[\w.-]+$/.test(opts.repo))
            usage('--repo must be OWNER/NAME');
          await cacheUse(opts);
        }
        break;
      case 'cache-headroom':
        await cacheHeadroom(args);
        break;
      case 'promote': {
        const opts = options(
          args,
          ['sha', 'pr', 'workflow', 'repo'],
          ['workflow'],
        );
        if (opts.repo && !/^[\w.-]+\/[\w.-]+$/.test(opts.repo))
          usage('--repo must be OWNER/NAME');
        if (
          Boolean(opts.sha) === Boolean(opts.pr) ||
          (opts.sha && !/^[a-f0-9]{40}$/.test(opts.sha)) ||
          (opts.pr && !/^[1-9][0-9]*$/.test(opts.pr))
        )
          usage('promote needs exactly one of --sha SHA or --pr N');
        await promote(opts);
        break;
      }
      case '_proxy':
        await proxy();
        break;
      default:
        usage('expected trust, ready, report, promote, cache-use or cache-headroom');
    }
  } catch (error) {
    console.error(error.message);
    process.exitCode = error.usage ? 2 : 1;
  }
}

if (process.argv[1] && resolve(process.argv[1]) === self) void main();
