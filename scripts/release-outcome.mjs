#!/usr/bin/env node
// Compose the machine-readable record of what a release actually did.
//
// A release has three outcomes, not two, and the third is the one that cost
// this repository months: **nothing shipped**, **shipped but unverified**, and
// **shipped and verified**. A boolean collapses the middle into whichever
// neighbour the reader guesses — and the middle is exactly what every release
// from 0.16.4 onward was, because the artifacts published and the npm
// verification then failed on the platforms whose packages the registry had not
// yet made resolvable.
//
// This turns each target's two job results into that three-state answer, and
// `.github/workflows/release.yml` attaches the result to the GitHub Release as
// `release-outcome.json`. A consumer outside this repository asks for it by
// URL, with no credential and without reading a job log; README.md's "Release
// outcome" section says how.
//
// The three states are per target *and* for the release as a whole, and the
// whole is the weakest of its parts: a release is only `shipped-verified` when
// every target it declares is.
//
// **Every target names a verification job, and there is no way to say it has
// none.** That is deliberate: an artifact a release publishes and no release
// installs is precisely the hole aarch64 Linux sat in on npm and crates.io sat
// in until `verify-crate`. A target with nothing to verify it would have to be
// recorded as permanently unverified, which would peg every release to the
// middle state and make the record as unreadable as the red square it replaces.
//
// A `skipped` publish is a target the operator switched off, and it is not part
// of this release's outcome; a `failure` or `cancelled` one is a target that was
// meant to ship and did not, and it holds the release out of the top state.
//
// Usage:
//   node scripts/release-outcome.mjs --version <X.Y.Z> [--run-url <url>] \
//     --target <registry:name> --published <result> --verified <result> \
//     [--target ... --published ... --verified ...]
//
// `<result>` is a GitHub Actions job result: success, failure, cancelled or
// skipped.

import { writeFileSync } from "node:fs";

/// The shape this document is; consumers read it before anything else. Bump it
/// with the golden files under `npm/test/golden/` in the same change.
const SCHEMA_VERSION = 1;

/// Every state a target, or the release, can be in. Ordered weakest first: the
/// release's own state is the weakest of its targets'.
const STATES = ["nothing-shipped", "shipped-unverified", "shipped-verified"];

/// What GitHub Actions reports for a job.
const RESULTS = ["success", "failure", "cancelled", "skipped"];

/// Every failure names what to do next: this runs inside a release job, where
/// the only diagnosis anyone gets is what it printed.
function die(msg, action) {
  process.stderr.write(`release-outcome: ${msg}\nACTION: ${action}\n`);
  process.exit(1);
}

const VERSION = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;
// The registry-qualified identifier `release-targets.toml` declares. Qualified
// because two of this repository's targets share the name `onepipeline-cli`.
const TARGET_ID = /^[A-Za-z0-9][A-Za-z0-9_-]*:[A-Za-z0-9][A-Za-z0-9._@/-]*$/;

/// Parse the argv into a version, an optional run URL, and the targets. Every
/// value is checked here rather than where it lands: this is called with
/// workflow expressions, and a job result that arrived misspelled would
/// otherwise be read as "not success" and quietly downgrade a release.
function parseArgs(argv) {
  const out = { version: null, runUrl: null, out: null, targets: [] };
  let current = null;
  const need = (i, flag) => {
    const value = argv[i + 1];
    if (value === undefined || value.startsWith("--"))
      die(`${flag} needs a value`, `give ${flag} a value`);
    return value;
  };
  for (let i = 0; i < argv.length; i += 1) {
    const flag = argv[i];
    switch (flag) {
      case "--version":
        out.version = need(i, flag);
        i += 1;
        break;
      case "--run-url":
        out.runUrl = need(i, flag);
        i += 1;
        break;
      case "--out":
        out.out = need(i, flag);
        i += 1;
        break;
      case "--target":
        current = { id: need(i, flag), published: null, verified: null };
        out.targets.push(current);
        i += 1;
        break;
      case "--published":
      case "--verified": {
        if (!current)
          die(
            `${flag} came before any --target`,
            "give each --target its own --published and --verified, in that order",
          );
        const key = flag === "--published" ? "published" : "verified";
        if (current[key] !== null)
          die(
            `${current.id} was given ${flag} twice`,
            "pass each of --published and --verified once per --target",
          );
        current[key] = need(i, flag);
        i += 1;
        break;
      }
      default:
        die(
          `unexpected argument: ${flag}`,
          "pass --version, --run-url, --out, --target, --published, --verified",
        );
    }
  }
  return out;
}

/// The three-state answer for one target.
function stateOf(target) {
  if (target.published !== "success") return "nothing-shipped";
  return target.verified === "success" ? "shipped-verified" : "shipped-unverified";
}

function compose(argv) {
  const args = parseArgs(argv);
  if (!args.version)
    die("--version is required", "pass the released version, e.g. --version 1.2.3");
  if (!VERSION.test(args.version)) {
    die(
      `'${args.version}' is not a version`,
      "pass the tag with its leading 'v' stripped, e.g. --version 1.2.3",
    );
  }
  if (args.runUrl !== null && !/^https:\/\/[^\s]+$/.test(args.runUrl)) {
    die(
      `'${args.runUrl}' is not an https URL`,
      "pass the release run's own URL, or omit --run-url",
    );
  }
  if (args.targets.length === 0) {
    die(
      "no --target was given",
      "pass one --target per artifact in release-targets.toml, each with --published and --verified",
    );
  }

  const seen = new Set();
  const targets = args.targets.map((target) => {
    if (!TARGET_ID.test(target.id)) {
      die(
        `'${target.id}' is not a <registry>:<name> identifier`,
        "name the target as release-targets.toml declares it, e.g. npm:onepipeline-cli",
      );
    }
    if (seen.has(target.id)) die(`${target.id} was given twice`, "pass each target once");
    seen.add(target.id);
    for (const key of ["published", "verified"]) {
      if (target[key] === null)
        die(`${target.id} has no --${key}`, `pass --${key} with the job result for that step`);
    }
    if (!RESULTS.includes(target.published)) {
      die(
        `${target.id}'s --published is '${target.published}'`,
        `pass one of: ${RESULTS.join(", ")}`,
      );
    }
    if (!RESULTS.includes(target.verified)) {
      die(
        `${target.id}'s --verified is '${target.verified}'`,
        `pass one of: ${RESULTS.join(", ")} — every target has a verification job, so there is always a result to pass`,
      );
    }
    return {
      id: target.id,
      outcome: stateOf(target),
      published: target.published,
      verified: target.verified,
    };
  });

  // A target the operator switched off did not take part in this release, so it
  // neither ships nor holds anything back; every other target does both.
  const taking_part = targets.filter((target) => target.published !== "skipped");
  const shipped = targets.filter((target) => target.published === "success");
  const outcome =
    shipped.length === 0
      ? STATES[0]
      : taking_part.every((target) => target.outcome === "shipped-verified")
        ? "shipped-verified"
        : "shipped-unverified";

  return {
    schema_version: SCHEMA_VERSION,
    version: args.version,
    outcome,
    run_url: args.runUrl,
    targets,
  };
}

const args = process.argv.slice(2);
const document = compose(args);
const rendered = `${JSON.stringify(document, null, 2)}\n`;
const where = args[args.indexOf("--out") + 1];
if (args.includes("--out")) {
  try {
    writeFileSync(where, rendered);
  } catch (error) {
    die(
      `cannot write ${where}: ${error.message}`,
      "check that --out names a path in a writable directory",
    );
  }
}
process.stdout.write(rendered);
