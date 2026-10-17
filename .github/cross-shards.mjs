// Prove that the jobs the Windows leg of `ci.yml` is split across run every
// `test-quick` test once between them: none dropped, none run twice.
//
// Read off the same three places the leg is: the `cross` job's `shard` axis and
// the partition it hands each shard, the tiers `just` runs through
// `_tier-test-quick`, and nextest's own listing of each tier, whole and once per
// shard, over this host's build. The shards are nextest's `count:K/N`, which deals
// each binary's tests out in listing order, so the property is the same on any
// platform's listing; what this host can show is that the leg passes the
// partition it means to and that nextest splits that listing the way the split
// assumes.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { parse } from "yaml";

function fail(message) {
  console.error(`cross-shards: ${message}`);
  process.exit(1);
}

const ci = parse(readFileSync(".github/workflows/ci.yml", "utf8"));
const cross = ci.jobs?.cross;
if (!cross) fail("ci.yml has no `cross` job");
const shards = cross.strategy?.matrix?.shard;
if (!Array.isArray(shards) || !shards.length)
  fail("the `cross` job has no `shard` axis to split its leg across");
const count = shards.length;
if (!shards.every((shard, at) => shard === at + 1))
  fail(`the \`shard\` axis is ${JSON.stringify(shards)}, not 1 to ${count} in order`);
const handed = cross.env?.ONEPIPELINE_TEST_PARTITION;
const meant = `count:\${{ matrix.shard }}/${count}`;
if (handed !== meant)
  fail(
    `the \`cross\` job hands its shards ${JSON.stringify(handed)}, not ${JSON.stringify(meant)}`,
  );

// Every tier a `test-quick` target runs, named where the justfile runs it: each
// `_<project>-test-quick` recipe but the archive runs exactly one, and one this
// cannot read is refused rather than left out of the proof.
const justfile = readFileSync("justfile", "utf8");
const tiers = [];
for (const [, recipe, body] of justfile.matchAll(/^(_[a-z0-9-]+-test-quick):(.*)$/gm)) {
  if (recipe === "_crate-test-quick-archive") continue;
  const runs = [...body.matchAll(/\(_tier-test-quick ([a-z0-9-]+)\)/g)].map((m) => m[1]);
  if (runs.length !== 1)
    fail(`${recipe} runs ${runs.length} tiers through \`_tier-test-quick\`, not one`);
  tiers.push(runs[0]);
}
if (!tiers.length) fail("the justfile runs no tier through `_tier-test-quick`");

function listed(filter, partition) {
  const args = ["nextest", "list", "--locked", "-E", filter, "--message-format", "oneline"];
  if (partition) args.push("--partition", partition);
  const out = execFileSync("cargo", args, {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "inherit"],
  });
  return out.split("\n").filter(Boolean);
}

let total = 0;
for (const tier of tiers) {
  const filter = execFileSync("just", ["--evaluate", tier], { encoding: "utf8" }).trim();
  const all = listed(filter);
  const seen = new Map();
  for (let shard = 1; shard <= count; shard++) {
    for (const test of listed(filter, `count:${shard}/${count}`)) {
      if (seen.has(test))
        fail(`${tier}: ${test} runs in shard ${seen.get(test)} and in shard ${shard}`);
      seen.set(test, shard);
    }
  }
  const dropped = all.filter((test) => !seen.has(test));
  if (dropped.length) fail(`${tier}: no shard runs ${dropped.length} tests, first ${dropped[0]}`);
  const strays = [...seen.keys()].filter((test) => !all.includes(test));
  if (strays.length) fail(`${tier}: a shard runs ${strays[0]}, which the whole tier does not list`);
  const per = Array.from(
    { length: count },
    (_, at) => [...seen.values()].filter((s) => s === at + 1).length,
  );
  console.log(
    `cross-shards: ${tier}: ${all.length} tests, ${per.join(" + ")} across ${count} shards`,
  );
  total += all.length;
}
console.log(
  `cross-shards: ok, ${total} tests across ${tiers.length} tiers, each in exactly one of ${count} shards`,
);
