// Prove that the jobs the Windows leg of `ci.yml` is split across run every
// `test-quick` test once between them: none dropped, none run twice.
//
// Read off the same places the leg is: the `cross` job's `shard` axis and the
// partition it hands each shard, the tiers `just` runs through `_tier-test-quick`,
// and nextest's own listing of each tier, whole and once per shard, over this
// host's build. That listing is not Windows': some tests are built on one platform
// and not the other. So the deal nextest makes — `count:K/N` hands each binary's
// listed tests out in turn, in listing order — is written down below and has to
// reproduce nextest's every shard here exactly, and is then dealt over
// `.github/windows-test-inventory.tsv`, every test a green Windows leg ran, which
// it has to place each in exactly one shard.
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

// The shard nextest's `count:K/N` puts each of a tier's tests in: a binary's
// tests, filtered and in listing order — by binary, then by name — dealt one to
// each shard in turn, starting again for every binary.
function dealt(tests) {
  const order = (a, b) => (a < b ? -1 : a > b ? 1 : 0);
  const sorted = [...tests].sort((a, b) => order(a.binary, b.binary) || order(a.test, b.test));
  const dealtSoFar = new Map();
  return sorted.map(({ binary, test }) => {
    const at = dealtSoFar.get(binary) ?? 0;
    dealtSoFar.set(binary, at + 1);
    return { id: `${binary} ${test}`, shard: (at % count) + 1 };
  });
}

function asTest(line) {
  const space = line.indexOf(" ");
  if (space < 1) fail(`nextest listed ${JSON.stringify(line)}, which names no binary and test`);
  return { binary: line.slice(0, space), test: line.slice(space + 1) };
}

const hostListed = new Map();
let total = 0;
for (const tier of tiers) {
  const filter = execFileSync("just", ["--evaluate", tier], { encoding: "utf8" }).trim();
  const all = listed(filter);
  hostListed.set(tier, new Set(all));
  const seen = new Map();
  for (let shard = 1; shard <= count; shard++) {
    for (const test of listed(filter, `count:${shard}/${count}`)) {
      if (seen.has(test))
        fail(`${tier}: ${test} runs in shard ${seen.get(test)} and in shard ${shard}`);
      seen.set(test, shard);
    }
  }
  for (const { id, shard } of dealt(all.map(asTest))) {
    if (seen.get(id) !== shard)
      fail(
        `${tier}: nextest puts ${id} in shard ${seen.get(id)}, where the deal this check makes over the Windows inventory puts it in ${shard}`,
      );
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

// The same deal over every test the Windows leg ran, by the tier that ran it.
const inventoryPath = ".github/windows-test-inventory.tsv";
const inventory = new Map(tiers.map((tier) => [tier, []]));
const listedOnce = new Set();
let source = "";
for (const [at, line] of readFileSync(inventoryPath, "utf8").split("\n").entries()) {
  if (!line) continue;
  if (line.startsWith("#")) {
    if (at === 0) source = line.replace(/^# /, "");
    continue;
  }
  const fields = line.split("\t");
  if (fields.length !== 3 || fields.some((field) => !field))
    fail(`${inventoryPath}:${at + 1} is not a tier, a binary and a test`);
  const [tier, binary, test] = fields;
  if (!inventory.has(tier))
    fail(`${inventoryPath}:${at + 1} names ${tier}, which no \`test-quick\` recipe runs`);
  if (listedOnce.has(`${binary} ${test}`))
    fail(`${inventoryPath}:${at + 1} lists ${binary} ${test} a second time`);
  listedOnce.add(`${binary} ${test}`);
  inventory.get(tier).push({ binary, test });
}
let windows = 0;
let elsewhere = 0;
const perShard = Array(count).fill(0);
for (const [tier, tests] of inventory) {
  if (!tests.length) fail(`${inventoryPath} holds no test of ${tier}, so it is not a whole leg`);
  const placed = new Map();
  for (const { id, shard } of dealt(tests)) {
    if (placed.has(id)) fail(`${tier}: ${id} is dealt to shards ${placed.get(id)} and ${shard}`);
    placed.set(id, shard);
    perShard[shard - 1] += 1;
    if (!hostListed.get(tier).has(id)) elsewhere += 1;
  }
  if (placed.size !== tests.length)
    fail(`${tier}: ${tests.length - placed.size} tests dealt nowhere`);
  windows += placed.size;
}
console.log(`cross-shards: ${source}`);
console.log(
  `cross-shards: windows ok, ${windows} tests, ${perShard.join(" + ")} across ${count} shards, each in exactly one; ${elsewhere} of them this host does not list`,
);
