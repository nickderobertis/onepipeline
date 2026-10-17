// The Windows release archive is a `.zip`, so scripts/build-staged-release.py's
// `--verify` reads it through its zip branch. This host cannot build a Windows
// binary, so the staged record here is assembled the way a Windows build leaves
// it — a win_amd64 wheel, the archive, its checksum, and the npm platform
// package `scripts/npm-build.mjs` and `npm pack` make — around a stand-in
// payload, then verified and corrupted through the real verifier.

import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { after, before, describe, it } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const TARGET = "x86_64-pc-windows-msvc";
const VERSION = readFileSync(join(REPO_ROOT, "Cargo.toml"), "utf8").match(
  /^\[package\][\s\S]*?^version = "([^"]+)"/m,
)[1];

/// Writes the wheel, the archive (with `mutation` applied) and its checksum.
const ASSEMBLE = String.raw`
import hashlib, json, sys, zipfile
from pathlib import Path
out, version, payload, mutation = Path(sys.argv[1]), sys.argv[2], Path(sys.argv[3]).read_bytes(), sys.argv[4]
stem = f'onepipeline_cli-{version}'
wheel = out / f'{stem}-py3-none-win_amd64.whl'
with zipfile.ZipFile(wheel, 'w') as w:
    w.writestr(f'{stem}.data/scripts/onepipeline.exe', payload)
    w.writestr(f'{stem}.dist-info/METADATA', f'Metadata-Version: 2.4\nName: onepipeline-cli\nVersion: {version}\n')
    w.writestr(f'{stem}.dist-info/WHEEL', 'Wheel-Version: 1.0\nTag: py3-none-win_amd64\n')
    w.writestr(f'{stem}.dist-info/sboms/onepipeline.cyclonedx.json', json.dumps({'bomFormat': 'CycloneDX'}))
    w.writestr(f'{stem}.dist-info/RECORD', '')
archive = out / f'onepipeline-v{version}-x86_64-pc-windows-msvc.zip'
with zipfile.ZipFile(archive, 'w') as a:
    for name in ['README.md', 'LICENSE', 'CHANGELOG.md']:
        if mutation == 'missing-license' and name == 'LICENSE':
            continue
        if mutation == 'symlink' and name == 'LICENSE':
            link = zipfile.ZipInfo(name)
            link.external_attr = (0o120777 << 16)
            a.writestr(link, 'README.md')
            continue
        a.writestr(name, Path(name).read_text())
    if mutation == 'directory':
        a.writestr(zipfile.ZipInfo('onepipeline.exe/'), '')
    else:
        a.writestr('onepipeline.exe', payload + (b'corruption' if mutation == 'payload' else b''))
(out / f'onepipeline-v{version}-x86_64-pc-windows-msvc.sha256').write_text(
    f'{hashlib.sha256(archive.read_bytes()).hexdigest()}  {archive.name}\n')
print(json.dumps({'wheel': str(wheel), 'archive': str(archive),
                  'binary_sha256': hashlib.sha256(payload).hexdigest()}))
`;

describe("a Windows staged record's zip archive", () => {
  let work;
  let payload;
  let npmTgz;

  before(() => {
    work = mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "staged-zip-"));
    payload = join(work, "onepipeline.exe");
    writeFileSync(payload, Buffer.from("MZ stand-in for a Windows executable\n"));
    const pkg = execFileSync(
      "node",
      [
        "scripts/npm-build.mjs",
        "platform",
        "--target",
        TARGET,
        "--binary",
        payload,
        "--out",
        join(work, "npm"),
      ],
      { cwd: REPO_ROOT, encoding: "utf8" },
    ).trim();
    const packed = JSON.parse(
      execFileSync("npm", ["pack", "--json", "--pack-destination", work, pkg], {
        cwd: REPO_ROOT,
        encoding: "utf8",
      }),
    );
    npmTgz = join(work, packed[0].filename);
  });
  after(() => {
    if (work) rmSync(work, { recursive: true, force: true });
  });

  function verify(mutation) {
    const out = mkdtempSync(join(work, `${mutation}-`));
    const built = JSON.parse(
      execFileSync("python3", ["-c", ASSEMBLE, out, VERSION, payload, mutation], {
        cwd: REPO_ROOT,
        encoding: "utf8",
      }),
    );
    const record = join(out, "record.json");
    writeFileSync(
      record,
      JSON.stringify({
        schema_version: 1,
        tag: `v${VERSION}`,
        target: TARGET,
        version: VERSION,
        binary_sha256: built.binary_sha256,
        wheel: built.wheel,
        archive: built.archive,
        npm: npmTgz,
        executables: ["onepipeline.exe"],
      }),
    );
    return spawnSync("python3", ["scripts/build-staged-release.py", "--verify", record], {
      cwd: REPO_ROOT,
      encoding: "utf8",
    });
  }

  it("verifies when the archive carries the wheel's executable and the documents", () => {
    const result = verify("none");
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, new RegExp(`verified binary reuse and metadata for ${TARGET}`));
  });

  for (const [mutation, reason] of [
    ["payload", /archive executable differs from wheel payload/],
    ["missing-license", /archive layout mismatch/],
    ["symlink", /archive member is not a regular file: LICENSE/],
    ["directory", /archive (layout mismatch|member is not a regular file)/],
  ]) {
    it(`refuses an archive whose ${mutation} is wrong`, () => {
      const result = verify(mutation);
      assert.equal(result.status, 1, result.stdout);
      assert.match(result.stderr, reason);
      assert.match(result.stderr, /^next: /m, "the refusal names what to do next");
    });
  }
});
