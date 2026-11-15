// Measurement-only engine-startup spike; the inventory owns trace validation and scope.
import { pathToFileURL } from "node:url";
import { foldJob, readJob } from "./launch-inventory.mjs";

export function main(args) {
  if (args[0] !== "fold" || args.length < 2)
    throw new Error(
      "use: node scripts/engine-startup.mjs fold LABEL=UNPACKED_JOB [...]; unzip each job's test-profile artifact and timestamped job.log together",
    );
  const folds = [];
  const rows = args.slice(1).map((arg) => {
    const at = arg.indexOf("=");
    if (at < 1 || at === arg.length - 1) throw new Error(`expected LABEL=UNPACKED_JOB, got ${arg}`);
    const f = foldJob(readJob(arg.slice(0, at), arg.slice(at + 1)));
    folds.push({ label: f.label, os: f.os, wallS: f.wallS, names: f.names });
    const lines = [
      `${f.label} (${f.os})`,
      `runner: ${f.host?.cpu_model ?? "not recorded"}; cores: ${f.host?.cpus ?? "not recorded"}`,
      `e2e minutes: ${f.wallS == null ? "not measured" : (f.wallS / 60).toFixed(6)}`,
    ];
    for (const verb of ["start", "adopt"]) {
      const r = f.verbs.find((v) => v.key === `onepipeline ${verb}`);
      const n = r?.lives.length ?? 0;
      const mean = n ? r.lives.reduce((a, b) => a + b, 0) / n / 1e6 : null;
      lines.push(
        `${verb}: ${mean == null ? "not measured" : `${mean.toFixed(6)} ms mean; ${n} completed / ${r.count} observed`}`,
      );
    }
    lines.push(
      "clock: completed launch-trace lifetime; excludes process creation; unfinished processes excluded",
    );
    lines.push(
      `test profiles: ${f.testCount}; names digest: ${f.testsDigest}; scope: ${JSON.stringify(f.scope)}`,
    );
    return lines.join("\n");
  });
  const groups = new Map();
  for (const f of folds) {
    const side = f.label.split("/")[0];
    const key = `${side}/${f.os}`;
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(f);
  }
  const mean = (values) => values.reduce((a, b) => a + b, 0) / values.length;
  for (const [key, members] of groups) {
    const minutes = members.filter((f) => f.wallS != null).map((f) => f.wallS / 60);
    if (!minutes.length) continue;
    rows.push(
      `${key}: e2e mean ${mean(minutes).toFixed(6)} min; range ${Math.min(...minutes).toFixed(6)}..${Math.max(...minutes).toFixed(6)}; n=${minutes.length}`,
    );
    const baseline = groups.get(`before/${members[0].os}`);
    if (!baseline || key.startsWith("before/")) continue;
    const baseMinutes = baseline.filter((f) => f.wallS != null).map((f) => f.wallS / 60);
    if (!baseMinutes.length) continue;
    const delta = mean(baseMinutes) - mean(minutes);
    const spread = (v) => Math.max(...v) - Math.min(...v);
    const decided = Math.abs(delta) > Math.max(spread(minutes), spread(baseMinutes));
    rows.push(
      `${key}: before-minus-after ${delta.toFixed(6)} min; ${decided ? "outside observed spreads" : "undecided (inside a side's spread)"}; typical PR estimate (27/30) ${((delta * 27) / 30).toFixed(6)} min${decided ? "" : " (undecided, not a saving)"}`,
    );
    const baselineNames = new Set(baseline.flatMap((f) => f.names));
    for (const f of members) {
      const present = new Set(f.names);
      const missing = [...baselineNames].filter((name) => !present.has(name));
      rows.push(
        `${f.label}: baseline test profiles missing ${JSON.stringify(missing)}; profile presence does not establish pass status (read nextest outcomes)`,
      );
    }
  }
  return `${rows.join("\n\n")}\n`;
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  try {
    process.stdout.write(main(process.argv.slice(2)));
  } catch (error) {
    console.error(`engine-startup: ${error.message}${error.next ? `; ${error.next}` : ""}`);
    process.exitCode = 2;
  }
}
