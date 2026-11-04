// Copied from dero-skills' create-repo skill; edit only the source.
// source: github.com/nickderobertis/dero-skills/skills/bootstrap/create-repo/assets/ci/nx-version-free-inputs.mjs
// version: 1
// sha256: 72a6b39be03813a81f5f86067a236b53cd41a450b5121f8586db23843573d5ff
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { basename } from 'node:path';

const args = process.argv.slice(2);
const ecosystemAt = args.indexOf('--ecosystem');
const ecosystem = args[ecosystemAt + 1];
const keep = args.includes('--keep-version');
if (
  ecosystemAt < 0 ||
  !['cargo', 'npm', 'python', 'files'].includes(ecosystem) ||
  args.length !== (keep ? 3 : 2) ||
  args.some(
    (arg, i) =>
      i !== ecosystemAt + 1 && !['--ecosystem', '--keep-version'].includes(arg),
  )
) {
  console.error(
    'usage: node scripts/nx-version-free-inputs.mjs --ecosystem cargo|npm|python|files [--keep-version]',
  );
  process.exit(2);
}
// llmlint: ignore[scripts_runtime_independent] The contract hashes tracked consumer files, so git ls-files supplies the checkout boundary using git provided on the GitHub-hosted runner, without authoring dependencies.
const tracked = execFileSync('git', ['ls-files', '-z'], { encoding: 'utf8' })
  .split('\0')
  .filter(Boolean);
const selectedNames = {
  cargo: ['Cargo.toml', 'Cargo.lock'],
  npm: ['package.json', 'bun.lock', 'package-lock.json', 'pnpm-lock.yaml'],
  python: ['pyproject.toml', 'uv.lock'],
}[ecosystem];
let selected = selectedNames
  ? tracked.filter((path) => selectedNames.includes(basename(path)))
  : [];
let selectionError;
if (ecosystem === 'files') {
  try {
    selected = readFileSync('.github/nx-version-files', 'utf8')
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter((line) => line && !line.startsWith('#'));
    for (const path of selected)
      if (!tracked.includes(path)) throw new Error(`${path} is not tracked`);
  } catch (error) {
    selectionError = error;
    selected = tracked.filter((path) => path === '.github/nx-version-files');
  }
}
selected = [...new Set(selected)].sort();
let readError;
const raw = new Map(
  selected.map((path) => {
    try {
      return [path, readFileSync(path)];
    } catch (error) {
      readError = error;
      return [path, Buffer.from(`unreadable:${error.code}`)];
    }
  }),
);
function digest(files) {
  const hash = createHash('sha256');
  for (const [path, body] of files)
    hash
      .update(path)
      .update('\0')
      .update(String(Buffer.byteLength(body)))
      .update('\0')
      .update(body)
      .update('\0');
  return hash.digest('hex');
}
function assert(ok, message) {
  if (!ok) throw new Error(message);
}
// Deliberately a subset: unknown TOML forms use the raw digest. This parser
// validates value boundaries; it never reconstructs or reformats their bytes.
function recognizableValue(text) {
  let at = 0;
  const whitespace = () => {
    while (at < text.length) {
      if (/\s/.test(text[at])) at++;
      else if (text[at] === '#') {
        while (at < text.length && text[at] !== '\n') at++;
      } else break;
    }
  };
  const string = () => {
    const quote = text[at++];
    assert(
      text.slice(at - 1, at + 2) !== quote.repeat(3),
      'unsupported multi-line TOML string',
    );
    const start = at - 1;
    while (at < text.length) {
      const c = text[at++];
      assert(c !== '\n' && c !== '\r', 'unsupported multi-line TOML string');
      if (c === '\\' && quote === '"') at++;
      else if (c === quote) {
        if (quote === '"') JSON.parse(text.slice(start, at));
        return;
      }
    }
    throw new Error('unterminated TOML value');
  };
  const key = () => {
    whitespace();
    if (text[at] === '"' || text[at] === "'") string();
    else {
      const match = text.slice(at).match(/^[A-Za-z0-9_-]+/);
      assert(match, 'unsupported inline table key');
      at += match[0].length;
    }
  };
  const value = () => {
    whitespace();
    const c = text[at];
    if (c === '"' || c === "'") {
      string();
      return;
    }
    if (c === '[' || c === '{') {
      const close = c === '[' ? ']' : '}';
      at++;
      whitespace();
      if (text[at] === close) {
        at++;
        return;
      }
      while (true) {
        if (c === '{') {
          key();
          whitespace();
          assert(text[at++] === '=', 'invalid inline table assignment');
        }
        value();
        whitespace();
        if (text[at] === close) {
          at++;
          return;
        }
        assert(text[at++] === ',', 'unsupported TOML collection syntax');
        whitespace();
        if (text[at] === close) {
          assert(c === '[', 'unsupported trailing inline-table comma');
          at++;
          return;
        }
      }
    }
    const token = text.slice(at).match(/^[^\s,\]}#]+/)?.[0];
    assert(
      token &&
        (/^(true|false|[+-]?(inf|nan))$/.test(token) ||
          /^[+-]?(0|[1-9](?:_?\d)*)(?:\.\d(?:_?\d)*)?(?:[eE][+-]?\d(?:_?\d)*)?$/.test(
            token,
          ) ||
          /^0(x[\da-fA-F](?:_?[\da-fA-F])*|o[0-7](?:_?[0-7])*|b[01](?:_?[01])*)$/.test(
            token,
          ) ||
          /^\d{4}-\d\d-\d\d(?:[Tt]\d\d:\d\d:\d\d(?:\.\d+)?(?:[Zz]|[+-]\d\d:\d\d)?)?$/.test(
            token,
          ) ||
          /^\d\d:\d\d:\d\d(?:\.\d+)?$/.test(token)),
      'unsupported or malformed TOML value',
    );
    at += token.length;
  };
  value();
  whitespace();
  assert(at === text.length, 'unsupported trailing TOML syntax');
}

// Parse the line-oriented TOML forms carrying package identities. Complex values
// stay byte-for-byte; unsupported identity syntax falls back to the raw digest.
function tables(text) {
  const out = [];
  let section = { name: '', lines: [] };
  out.push(section);
  let depth = 0,
    quote = '',
    multiline = false;
  let pending = [];
  for (const line of text.split('\n')) {
    if (!depth && !quote) {
      const header = line.match(/^\s*(\[\[?)([^\]]+)(\]\]?)\s*(?:#.*)?$/);
      if (header) {
        assert(header[1].length === header[3].length, 'unbalanced TOML table');
        section = { name: header[2].trim(), lines: [line] };
        out.push(section);
        continue;
      }
      if (line.trim() && !line.trim().startsWith('#')) {
        assert(
          /^\s*(?:[\w.-]+|"[^"]+"|'[^']+')\s*=/.test(line),
          `cannot parse TOML line ${line}`,
        );
        assert(
          !/^\s*(package|project)\s*=\s*\{/.test(line),
          'unsupported inline package table',
        );
      }
    }
    if (!depth && !quote) {
      const assignment = line.match(
        /^\s*(?:[\w.-]+|"[^"]+"|'[^']+')\s*=\s*(.*)$/,
      );
      if (assignment) pending = [assignment[1]];
    } else pending.push(line);
    section.lines.push(line);
    for (let i = 0; i < line.length; i++) {
      const c = line[i];
      if (quote) {
        if (quote === '"' && c === '\\') {
          i++;
          continue;
        }
        if (c === quote) {
          if (multiline) {
            if (line.slice(i, i + 3) === quote.repeat(3)) {
              quote = '';
              multiline = false;
              i += 2;
            }
          } else quote = '';
        }
      } else if (c === '#') break;
      else if (c === '"' || c === "'") {
        quote = c;
        if (line.slice(i, i + 3) === c.repeat(3)) {
          multiline = true;
          i += 2;
        }
      } else if (c === '[' || c === '{') depth++;
      else if (c === ']' || c === '}') {
        depth--;
        assert(depth >= 0, 'unbalanced TOML value');
      }
    }
    assert(!quote || multiline, 'unterminated TOML string');
    if (!depth && !quote && pending.length) {
      recognizableValue(pending.join('\n'));
      pending = [];
    }
  }
  assert(!quote && depth === 0, 'incomplete TOML value');
  return out;
}
const scalar = (table, key) =>
  table?.lines
    .join('\n')
    .match(
      new RegExp(`^\\s*${key}\\s*=\\s*["']([^"']+)["']\\s*(?:#.*)?$`, 'm'),
    )?.[1];
const own = new Map();
const manifestNames = new Map();
const ownedManifests = new Set();
function identities() {
  if (ecosystem === 'files') {
    for (const name of ['Cargo.toml', 'package.json', 'pyproject.toml']) {
      if (!tracked.includes(name)) continue;
      const text = readFileSync(name, 'utf8');
      let version;
      if (name === 'package.json') version = JSON.parse(text).version;
      else {
        const ts = tables(text);
        version =
          scalar(
            ts.find((t) => t.name === 'workspace.package'),
            'version',
          ) ??
          scalar(
            ts.find((t) => ['package', 'project'].includes(t.name)),
            'version',
          );
      }
      if (typeof version === 'string' && version) {
        own.set('__workspace__', version);
        return;
      }
    }
    throw new Error('no root workspace version for literal files');
  }
  for (const path of tracked) {
    if (
      path
        .split('/')
        .some((part) =>
          [
            'tests',
            'fixtures',
            '__fixtures__',
            'vendor',
            'third_party',
          ].includes(part),
        )
    )
      continue;
    if (ecosystem === 'npm' && basename(path) === 'package.json') {
      const doc = JSON.parse(readFileSync(path, 'utf8'));
      if (doc.name && typeof doc.version === 'string') {
        own.set(doc.name, doc.version);
        ownedManifests.add(path);
        manifestNames.set(
          path.slice(0, -'package.json'.length).replace(/\/$/, ''),
          doc.name,
        );
      }
    } else if (
      ((ecosystem === 'cargo' || ecosystem === 'files') &&
        basename(path) === 'Cargo.toml') ||
      (ecosystem === 'python' && basename(path) === 'pyproject.toml')
    ) {
      const ts = tables(readFileSync(path, 'utf8'));
      const p = ts.find(
        (t) => t.name === (ecosystem === 'python' ? 'project' : 'package'),
      );
      const version =
        scalar(p, 'version') ??
        scalar(
          ts.find((t) => t.name === 'workspace.package'),
          'version',
        ) ??
        (/version\.workspace\s*=\s*true/.test(p?.lines.join('\n') ?? '')
          ? own.get('__workspace__')
          : undefined);
      if (scalar(p, 'name') && version) {
        own.set(scalar(p, 'name'), version);
        ownedManifests.add(path);
      }
      if (
        path === 'Cargo.toml' &&
        scalar(
          ts.find((t) => t.name === 'workspace.package'),
          'version',
        )
      )
        own.set(
          '__workspace__',
          scalar(
            ts.find((t) => t.name === 'workspace.package'),
            'version',
          ),
        );
    }
  }
}
function jsonWithoutComments(text) {
  // Bun's JSONC permits comments and trailing commas. Strip them only outside
  // strings so URLs and string contents remain significant.
  let out = '',
    quoted = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quoted) {
      out += c;
      if (c === '\\') out += text[++i];
      else if (c === '"') quoted = false;
    } else if (c === '"') {
      quoted = true;
      out += c;
    } else if (text.slice(i, i + 2) === '//') {
      while (i < text.length && text[i] !== '\n') i++;
      out += '\n';
    } else if (text.slice(i, i + 2) === '/*') {
      const end = text.indexOf('*/', i + 2);
      assert(end >= 0, 'unclosed JSONC comment');
      i = end + 1;
    } else if (c === ',' && /^\s*[}\]]/.test(text.slice(i + 1))) continue;
    else out += c;
  }
  return JSON.parse(out);
}
function normalizeJson(doc, path) {
  if (basename(path) === 'package.json') {
    if (ownedManifests.has(path) && typeof doc.version === 'string')
      doc.version = '';
  } else if (basename(path) === 'package-lock.json') {
    if (doc.name && own.has(doc.name)) doc.version = '';
    for (const [p, entry] of Object.entries(doc.packages ?? {})) {
      const name =
        entry.name ?? manifestNames.get(p) ?? p.split('node_modules/').at(-1);
      if (
        own.get(name) === entry.version &&
        (!p.includes('node_modules/') || entry.link)
      )
        entry.version = '';
    }
  } else if (basename(path) === 'bun.lock') {
    for (const entry of Object.values(doc.workspaces ?? {}))
      if (own.get(entry.name) === entry.version) entry.version = '';
  }
  // Internal dependency pins follow the member release, while third-party pins
  // always remain. Retain the operator so changing dependency semantics moves it.
  function pins(value, dependencyMap = false) {
    if (!value || typeof value !== 'object') return;
    for (const [key, child] of Object.entries(value)) {
      if (dependencyMap && typeof child === 'string' && own.has(key)) {
        const version = own.get(key);
        if (
          child === version ||
          ['^', '~', '='].some((op) => child === op + version)
        )
          value[key] = child.slice(0, -version.length) + '<version>';
      } else
        pins(
          child,
          [
            'dependencies',
            'devDependencies',
            'peerDependencies',
            'optionalDependencies',
          ].includes(key),
        );
    }
  }
  pins(doc);
  return JSON.stringify(doc);
}
try {
  if (selectionError) throw selectionError;
  if (readError) throw readError;
  if (keep) {
    console.log(digest(raw));
  } else {
    identities();
    const normalized = new Map();
    for (const [path, bytes] of raw) {
      const text = bytes.toString('utf8').replaceAll('\r\n', '\n');
      let result = text;
      if (ecosystem === 'files') {
        const versions = [...new Set(own.values())];
        assert(
          versions.length === 1,
          'cannot identify one current workspace version for literal files',
        );
        result = text.replaceAll(versions[0], '<version>');
      } else if (
        ['package.json', 'package-lock.json', 'bun.lock'].includes(
          basename(path),
        )
      )
        result = normalizeJson(jsonWithoutComments(text), path);
      else if (basename(path) === 'pnpm-lock.yaml') {
        // pnpm lockfiles do not contain a workspace package's own version;
        // retain their dependency graph verbatim.
        assert(/^lockfileVersion:/m.test(text), 'cannot parse pnpm lockfile');
      } else {
        const ts = tables(text);
        for (const table of ts) {
          const name = scalar(table, 'name'),
            version = scalar(table, 'version');
          const manifest =
            (ownedManifests.has(path) &&
              ['package', 'project'].includes(table.name)) ||
            (path === 'Cargo.toml' && table.name === 'workspace.package');
          const lock =
            table.name === 'package' &&
            ['Cargo.lock', 'uv.lock'].includes(basename(path));
          const workspaceLock =
            lock &&
            own.get(name) === version &&
            (basename(path) === 'Cargo.lock'
              ? !/^\s*source\s*=/m.test(table.lines.join('\n'))
              : /^\s*source\s*=\s*\{\s*(editable|virtual|workspace)\s*=/m.test(
                  table.lines.join('\n'),
                ));
          if ((manifest && !lock) || workspaceLock) {
            if (table.lines.some((line) => /^\s*version\s*=/.test(line)))
              assert(
                version !== undefined,
                'unsupported package version syntax',
              );
            table.lines = table.lines.map((line) =>
              line.replace(
                /^(\s*version\s*=\s*)["'][^"']*["'](\s*(?:#.*)?)$/,
                '$1""$2',
              ),
            );
          }
          table.lines = table.lines.map((line) => {
            for (const [member, v] of own) {
              const escaped = member.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
              if (
                new RegExp(`^\\s*(?:"${escaped}"|${escaped})\\s*=`).test(
                  line,
                ) &&
                (table.name.includes('dependencies') ||
                  /^\s*\{/.test(line.split('=')[1] ?? ''))
              )
                line = line.replaceAll('"' + v + '"', '"<version>"');
            }
            return line;
          });
        }
        result = ts
          .filter((t) => t.lines.length)
          .map((t) => t.lines.join('\n'))
          .join('\n');
      }
      normalized.set(path, result);
    }
    console.log(digest(normalized));
  }
} catch (error) {
  console.error(
    `nx-version-free-inputs: ${error.message}; hashing raw tracked inputs`,
  );
  console.log(digest(raw));
}
