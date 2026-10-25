#!/usr/bin/env node
// The one part of the judged tier these journeys replace: the paid model call.
//
// llmlint reaches its judge through the `oneharness` binary `LLMLINT_ONEHARNESS_BIN`
// names. A `run` is the judge call, so this answers it from `STANDIN_VERDICT` and
// records it in `STANDIN_LOG`; everything else (`--version`, and the `config` that
// `llmlint fingerprint` resolves the harness configuration through) is handed to the
// real oneharness at `STANDIN_REAL_ONEHARNESS`, so the digest stays real.
//
// STANDIN_VERDICT: `pass` holds every rule asked; `fail` breaks the first one on
// `STANDIN_FILE`; `none` answers no rule; `some` answers only the first.
import { spawnSync } from "node:child_process";
import { appendFileSync, readFileSync } from "node:fs";

const args = process.argv.slice(2);

if (args[0] !== "run") {
  const real = spawnSync(process.env.STANDIN_REAL_ONEHARNESS, args, { stdio: "inherit" });
  process.exit(real.status ?? 1);
}

const flagValue = (flag) => {
  const at = args.indexOf(flag);
  return at === -1 ? undefined : args[at + 1];
};
// What a journey or llmlint hands this is checked before it is answered, so a
// typo in a journey or a change in llmlint's schema fails loudly rather than
// passing as a judge that held every rule.
const refuse = (why) => {
  process.stderr.write(`oneharness stand-in: ${why}\n`);
  process.exit(2);
};
const verdict = process.env.STANDIN_VERDICT ?? "pass";
if (!["pass", "fail", "none", "some"].includes(verdict)) {
  refuse(`STANDIN_VERDICT is '${verdict}', not one of pass, fail, none or some`);
}
const schema = JSON.parse(readFileSync(flagValue("--schema"), "utf8"));
const properties = schema?.properties;
if (typeof properties !== "object" || properties === null || Array.isArray(properties)) {
  refuse("the judge schema names no rules under `properties`");
}
const rules = Object.keys(properties);
appendFileSync(process.env.STANDIN_LOG, `${JSON.stringify({ rules })}\n`);

const structured = {};
const answered = verdict === "none" ? [] : verdict === "some" ? rules.slice(0, 1) : rules;
for (const [index, rule] of answered.entries()) {
  structured[rule] =
    verdict === "fail" && index === 0
      ? {
          name: rule,
          rationale: "the stand-in judge breaks this rule",
          holds: false,
          violations: [{ file: process.env.STANDIN_FILE, line: 1, message: "stand-in violation" }],
        }
      : { name: rule, rationale: "the stand-in judge holds this rule", holds: true };
}
const harness = flagValue("--harness") ?? "claude-code";
process.stdout.write(
  `${JSON.stringify({
    schema_version: "0.1",
    oneharness_version: "stand-in",
    results: [
      {
        harness,
        status: "ok",
        exit_code: 0,
        structured,
        schema_valid: true,
        schema_attempts: 1,
        schema_error: null,
        error: null,
      },
    ],
  })}\n`,
);
