#!/usr/bin/env node
// Compose the machine-readable record of what a release actually did, which
// `release.yml` attaches to the GitHub Release. What the three outcomes mean and
// how a consumer asks for the file are in README.md's "Release outcome".
//
// **Every target names a verification job, and there is no way to say it has
// none**: an artifact a release publishes and no release installs is a defect
// rather than a state to record.
//
// Usage:
//   node scripts/release-outcome.mjs --version <X.Y.Z> [--run-url <url>] \
//     [--out <path>] \
//     --target <registry:name> --published <result> --verified <result> \
//     [--target ... --published ... --verified ...]
//
// `<result>` is a GitHub Actions job result: success, failure, cancelled or
// skipped.
//
// Exits 0; 2 on a caller error, refused before anything is written; 1 when the
// output file could not be written. With `--out` the document goes to that file
// and stdout carries one summary line; without it, stdout carries the document.

import { writeFileSync } from "node:fs";

/// The shape this document is; consumers read it before anything else. Bump it
/// with the golden files under `npm/test/golden/` in the same change.
const SCHEMA_VERSION = 1;

/// Every state a target, or the release, can be in. This list is the vocabulary:
/// README.md tells a consumer these three names and nothing else, and
/// `npm/test/release-outcome.test.mjs` reads it back out of here to hold the two
/// together.
const STATES = ["nothing-shipped", "shipped-unverified", "shipped-verified"];
const [NOTHING_SHIPPED, SHIPPED_UNVERIFIED, SHIPPED_VERIFIED] = STATES;

// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] this is the one
// place in this repository that names GitHub Actions' job-result vocabulary — the
// workflow passes `needs.<job>.result` through without listing its values — so
// there is no second copy here to reconcile against, and GitHub publishes the set
// in prose rather than in a schema anything could fetch and diff. What the rule
// protects is held by failing closed instead: a value outside this list is refused
// by name below rather than read as "not success", so the day GitHub adds a fifth
// result the `outcome` job goes red saying which value it did not know, instead of
// silently downgrading a release that shipped.
const RESULTS = ["success", "failure", "cancelled", "skipped"];
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

/// The caller asked for something this cannot do, and nothing has been written.
/// Every refusal names what to do next: this runs inside a release job, where
/// the only diagnosis anyone gets is what it printed.
function die(msg, action) {
  process.stderr.write(`release-outcome: ${msg}\nACTION: ${action}\n`);
  process.exit(2);
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
    // An empty value is a caller error like any other, and refusing it here is
    // what keeps `--out ""` from being reported later as an unwritable path —
    // an operational failure it is not.
    if (value === undefined || value === "" || value.startsWith("--")) {
      die(`${flag} needs a value`, `give ${flag} a value`);
    }
    return value;
  };
  // Each of these names one thing about the whole record, so a second one is a
  // caller that meant two records — never a value to quietly overwrite.
  const once = (i, flag, key) => {
    if (out[key] !== null) die(`${flag} was given twice`, `pass ${flag} once`);
    return need(i, flag);
  };
  for (let i = 0; i < argv.length; i += 1) {
    const flag = argv[i];
    switch (flag) {
      case "--version":
        out.version = once(i, flag, "version");
        i += 1;
        break;
      case "--run-url":
        out.runUrl = once(i, flag, "runUrl");
        i += 1;
        break;
      case "--out":
        out.out = once(i, flag, "out");
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

/// **`nothing-shipped` is only said when it can be proved.** A publish job that
/// *failed* is not a job that published nothing: `scripts/publish-npm.sh` can
/// take a package the registry accepts and then fail awaiting propagation, and
/// a cancelled job can stop between two of five packages. Either leaves
/// artifacts public that nothing verified, which is the middle state. Only a
/// `skipped` publish — a job that never ran — proves the registry was never
/// written to.
function stateOf(target) {
  if (target.published === "success") {
    return target.verified === "success" ? SHIPPED_VERIFIED : SHIPPED_UNVERIFIED;
  }
  return target.published === "skipped" ? NOTHING_SHIPPED : SHIPPED_UNVERIFIED;
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
  // Parsed rather than pattern-matched: this is the one field that becomes a
  // link somebody clicks out of the record, and `https://%` matches a prefix
  // test while being no URL at all.
  if (args.runUrl !== null) {
    let parsed = null;
    try {
      parsed = new URL(args.runUrl);
    } catch {
      parsed = null;
    }
    if (parsed?.protocol !== "https:" || parsed.host === "") {
      die(
        `'${args.runUrl}' is not an https URL`,
        "pass the release run's own URL, or omit --run-url",
      );
    }
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
  // neither ships nor holds anything back; every other target does both. The
  // release says `nothing-shipped` only when every target does — which, by the
  // rule above, means every publish job was skipped.
  const taking_part = targets.filter((target) => target.published !== "skipped");
  const outcome = targets.every((target) => target.outcome === NOTHING_SHIPPED)
    ? NOTHING_SHIPPED
    : taking_part.every((target) => target.outcome === SHIPPED_VERIFIED)
      ? SHIPPED_VERIFIED
      : SHIPPED_UNVERIFIED;

  // `out` is where to put the record rather than part of it, so it is returned
  // beside the record: the file's shape does not depend on how it was asked for.
  return {
    out: args.out,
    record: {
      schema_version: SCHEMA_VERSION,
      version: args.version,
      outcome,
      run_url: args.runUrl,
      targets,
    },
  };
}

const { out, record } = compose(process.argv.slice(2));
const rendered = `${JSON.stringify(record, null, 2)}\n`;

// llmlint: ignore-block[tool_output_is_signal] without `--out` the document on
// stdout is this invocation's *product*, not a report about it — the mode exists so
// a caller can pipe the record somewhere this script does not know about, and the
// journeys in npm/test/release-outcome.test.mjs read it that way. Silence here would
// mean composing the record and discarding it. The `--out` branch is the one that
// reports, and it reports in one line.
//
// `--out` is the delivery, so stdout is a summary rather than a second copy of
// what was just written; without it stdout *is* the delivery.
if (out === null) {
  process.stdout.write(rendered);
  // llmlint: ignore-end[tool_output_is_signal]
} else {
  try {
    writeFileSync(out, rendered);
  } catch (error) {
    process.stderr.write(`release-outcome: cannot write ${out}: ${error.message}\n`);
    process.stderr.write("ACTION: pass --out a path in a directory that exists and is writable\n");
    process.exit(1);
  }
  process.stdout.write(`release-outcome: ${record.version} ${record.outcome} -> ${out}\n`);
}
