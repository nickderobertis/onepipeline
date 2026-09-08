// The three states a release can end in, driven through the real composer.
//
// `scripts/release-outcome.mjs` turns a release run's job results into the
// `release-outcome.json` asset `release.yml` attaches to the GitHub Release, so
// its shape is a contract a consumer outside this repository reads:
// `schema_version` is bumped with the goldens beside this file, in the same
// change.

import { execFile } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { after, describe, it } from "node:test";
import assert from "node:assert/strict";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const GOLDEN = join(REPO_ROOT, "npm", "test", "golden");
const execFileAsync = promisify(execFile);

const work = mkdtempSync(join(tmpdir(), "onepipeline-outcome-"));
after(() => rmSync(work, { recursive: true, force: true }));

/// Run the composer, returning what it wrote and what it exited with.
async function compose(args) {
  try {
    const { stdout } = await execFileAsync("node", ["scripts/release-outcome.mjs", ...args], {
      cwd: REPO_ROOT,
      encoding: "utf8",
    });
    return { code: 0, stdout, stderr: "" };
  } catch (error) {
    return { code: error.code ?? 1, stdout: error.stdout ?? "", stderr: error.stderr ?? "" };
  }
}

/// The three targets `release.yml`'s `outcome` job passes, with the results a
/// caller wants for this case.
function targetsFor({
  cratePublished,
  crateVerified,
  pypiPublished,
  pypiVerified,
  npmPublished,
  npmVerified,
}) {
  return [
    "--target",
    "crate:onepipeline",
    "--published",
    cratePublished,
    "--verified",
    crateVerified,
    "--target",
    "pypi:onepipeline-cli",
    "--published",
    pypiPublished,
    "--verified",
    pypiVerified,
    "--target",
    "npm:onepipeline-cli",
    "--published",
    npmPublished,
    "--verified",
    npmVerified,
  ];
}

const ALL_GREEN = {
  cratePublished: "success",
  crateVerified: "success",
  pypiPublished: "success",
  pypiVerified: "success",
  npmPublished: "success",
  npmVerified: "success",
};

describe("the release outcome record", () => {
  const cases = [
    {
      state: "nothing-shipped",
      // A release whose publishing never ran: the repository variables are off,
      // or the gate went red before anything reached a registry.
      results: {
        cratePublished: "skipped",
        crateVerified: "skipped",
        pypiPublished: "skipped",
        pypiVerified: "skipped",
        npmPublished: "skipped",
        npmVerified: "skipped",
      },
    },
    {
      state: "shipped-unverified",
      // What every release from 0.16.4 onward actually was: all three artifacts
      // public, and the npm install of them red.
      results: {
        cratePublished: "success",
        crateVerified: "success",
        pypiPublished: "success",
        pypiVerified: "success",
        npmPublished: "success",
        npmVerified: "failure",
      },
    },
    {
      state: "shipped-verified",
      results: ALL_GREEN,
    },
  ];

  for (const { state, results } of cases) {
    it(`reports ${state}, byte for byte as the golden`, async () => {
      const out = join(work, `${state}.json`);
      const composed = await compose([
        "--out",
        out,
        "--version",
        "1.2.3",
        "--run-url",
        "https://github.com/nickderobertis/onepipeline/actions/runs/1",
        ...targetsFor(results),
      ]);
      assert.equal(composed.code, 0, composed.stderr);
      const written = readFileSync(out, "utf8");
      // `--out` is the delivery, so stdout says where it went rather than
      // repeating it — a release log carries one summary line, not the document.
      assert.equal(composed.stdout, `release-outcome: 1.2.3 ${state} -> ${out}\n`);
      assert.equal(JSON.parse(written).version, "1.2.3");
      assert.equal(
        written,
        readFileSync(join(GOLDEN, `release-outcome-${state}.json`), "utf8"),
        `the record for ${state} changed; update the golden and bump schema_version in the same change`,
      );
      assert.equal(JSON.parse(written).outcome, state);
    });
  }

  it("is only verified when every target taking part is", async () => {
    // One red verification is the whole release's answer, whatever the other
    // targets did — the record exists so that cannot be read past.
    const one_red = await compose([
      "--version",
      "1.2.3",
      ...targetsFor({ ...ALL_GREEN, pypiVerified: "failure" }),
    ]);
    const record = JSON.parse(one_red.stdout);
    assert.equal(record.outcome, "shipped-unverified");
    assert.equal(
      record.targets.find((t) => t.id === "pypi:onepipeline-cli").outcome,
      "shipped-unverified",
    );
    assert.equal(
      record.targets.find((t) => t.id === "npm:onepipeline-cli").outcome,
      "shipped-verified",
      "a green target is still recorded green when a sibling is red",
    );
    assert.equal(record.run_url, null, "an omitted --run-url is recorded as absent, not as ''");
  });

  it("a target switched off does not take part, and one that failed to ship does", async () => {
    // A registry the operator has not switched on is not this release's
    // business: PYPI_PUBLISH off must not report a release that shipped and
    // verified two artifacts as unverified.
    const off = await compose([
      "--version",
      "1.2.3",
      ...targetsFor({ ...ALL_GREEN, pypiPublished: "skipped", pypiVerified: "skipped" }),
    ]);
    assert.equal(JSON.parse(off.stdout).outcome, "shipped-verified");

    // A publish that was meant to happen and failed is a different thing, and
    // it holds the release out of the top state rather than disappearing.
    const broke = await compose([
      "--version",
      "1.2.3",
      ...targetsFor({ ...ALL_GREEN, pypiPublished: "failure", pypiVerified: "skipped" }),
    ]);
    const failed = JSON.parse(broke.stdout);
    assert.equal(failed.outcome, "shipped-unverified");
    assert.equal(
      failed.targets.find((t) => t.id === "pypi:onepipeline-cli").outcome,
      "nothing-shipped",
    );

    // And when nothing shipped at all, no amount of green elsewhere invents one.
    const nothing = await compose([
      "--version",
      "1.2.3",
      ...targetsFor({
        cratePublished: "failure",
        crateVerified: "skipped",
        pypiPublished: "skipped",
        pypiVerified: "skipped",
        npmPublished: "cancelled",
        npmVerified: "skipped",
      }),
    ]);
    assert.equal(JSON.parse(nothing.stdout).outcome, "nothing-shipped");
  });

  it("refuses input it cannot read rather than downgrading a release", async () => {
    const base = targetsFor(ALL_GREEN);
    const refusals = [
      [["--version", "v1.2.3", ...base], /is not a version/],
      [["--version", "1.2.3", "--version", "1.2.4", ...base], /--version was given twice/],
      [
        ["--version", "1.2.3", "--run-url", "http://example.invalid/x", ...base],
        /is not an https URL/,
      ],
      [["--version", "1.2.3", "--out", ...base], /--out needs a value/],
      [["--version", "1.2.3", "--nope", "x", ...base], /unexpected argument: --nope/],
      [
        ["--version", "1.2.3", "--published", "success", ...base],
        /--published came before any --target/,
      ],
      [
        [
          "--version",
          "1.2.3",
          ...base,
          "--target",
          "npm:onepipeline-cli",
          "--published",
          "success",
          "--verified",
          "success",
        ],
        /was given twice/,
      ],
      [
        [
          "--version",
          "1.2.3",
          "--target",
          "npm:onepipeline-cli",
          "--published",
          "suceess",
          "--verified",
          "success",
        ],
        /--published is 'suceess'/,
      ],
      // There is no way to say a target has no verification job, and that is the
      // point: an artifact nothing installs is the defect, not a state to record.
      [
        [
          "--version",
          "1.2.3",
          "--target",
          "npm:onepipeline-cli",
          "--published",
          "success",
          "--verified",
          "none",
        ],
        /--verified is 'none'/,
      ],
      [
        [
          "--version",
          "1.2.3",
          "--target",
          "not a target",
          "--published",
          "success",
          "--verified",
          "success",
        ],
        /is not a <registry>:<name> identifier/,
      ],
      [
        ["--version", "1.2.3", "--target", "npm:onepipeline-cli", "--published", "success"],
        /has no --verified/,
      ],
      [["--version", "1.2.3"], /no --target was given/],
    ];
    for (const [args, says] of refusals) {
      const refused = await compose(args);
      // 2 rather than 1: a caller error is a different thing from a release
      // job's output directory being unwritable, and the exit code says which.
      assert.equal(refused.code, 2, `${args.join(" ")} was accepted or misreported`);
      assert.match(refused.stderr, says);
      assert.match(refused.stderr, /^ACTION: /m, "every refusal names what to do next");
      assert.equal(refused.stdout, "", "a refusal writes nothing to stdout");
    }
  });

  it("reports a record it could not write as its own failure, not as a caller error", async () => {
    const nowhere = join(work, "no-such-directory", "release-outcome.json");
    const failed = await compose([
      "--version",
      "1.2.3",
      "--out",
      nowhere,
      ...targetsFor(ALL_GREEN),
    ]);
    assert.equal(failed.code, 1, "an unwritable --out is not a caller error");
    assert.match(failed.stderr, /cannot write .*no-such-directory/);
    assert.match(failed.stderr, /^ACTION: /m);
    assert.equal(failed.stdout, "", "nothing claims a record that was never written");
  });

  it("names the targets release-targets.toml declares, and the ones release.yml passes", () => {
    const declared = [
      ...readFileSync(join(REPO_ROOT, "release-targets.toml"), "utf8").matchAll(
        /^\[\[target\]\]\nid = "([^"]+)"/gm,
      ),
    ].map((m) => m[1]);
    assert.ok(declared.length > 0, "release-targets.toml declares no [[target]]");

    const workflow = readFileSync(join(REPO_ROOT, ".github", "workflows", "release.yml"), "utf8");
    const start = workflow.indexOf("\n  outcome:\n");
    assert.notEqual(start, -1, "no `outcome` job in release.yml");
    const passed = [...workflow.slice(start).matchAll(/--target (\S+) /g)].map((m) => m[1]);
    assert.deepEqual(
      [...passed].sort(),
      [...declared].sort(),
      "the outcome record answers for a different set of artifacts than release-targets.toml declares",
    );

    // And every golden answers for exactly those, so a target added to the
    // declaration cannot be left out of the record it is supposed to appear in.
    for (const { state } of cases) {
      const golden = JSON.parse(
        readFileSync(join(GOLDEN, `release-outcome-${state}.json`), "utf8"),
      );
      assert.deepEqual(
        golden.targets.map((t) => t.id).sort(),
        [...declared].sort(),
        `the ${state} golden does not answer for every declared target`,
      );
    }
  });
});
