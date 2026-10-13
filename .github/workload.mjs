export const eventClasses = [
  ["pr-opened", "pull_request", "opened"],
  ["pr-synchronize", "pull_request", "synchronize"],
  ["pr-reopened", "pull_request", "reopened"],
  ["base-push", "push", null],
  ["release-pr-merge", "push", null],
  ["release-tag", "push", null],
  ["release-published", "release", "published"],
  ["downstream", "workflow_run", "completed"],
  ["repository-dispatch", "repository_dispatch", null],
  ["schedule", "schedule", null],
  ["manual-default", "workflow_dispatch", null],
  ["manual-opt-in", "workflow_dispatch", null],
];
export const diffClasses = ["source", "version", "dependency"];
export const ownership = [false, true];
export const publishFlags = ["false", "true"];
export function publicationId(fork, { pypi, npm, crate }) {
  return `${fork ? "fork" : "same"}/pypi-${pypi}/npm-${npm}/crate-${crate}`;
}
export function scenarioId(id, kind, fork, opt) {
  return `${id}/${kind}/${publicationId(fork, opt)}`;
}
export function inventory() {
  const scenarios = [];
  const lifecycles = [];
  for (const fork of ownership)
    for (const pypi of publishFlags)
      for (const npm of publishFlags)
        for (const crate of publishFlags) {
          const opt = { pypi, npm, crate };
          const id = publicationId(fork, opt);
          lifecycles.push(id, `${id}/without-release-pr`);
          for (const [event] of eventClasses)
            for (const kind of diffClasses) scenarios.push(scenarioId(event, kind, fork, opt));
        }
  return { event_classes: eventClasses.map((c) => c[0]), scenarios, lifecycles };
}
