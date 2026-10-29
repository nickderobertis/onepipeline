import { createHash } from "node:crypto";

export const baselineHash = (baseline) =>
  createHash("sha256").update(JSON.stringify(baseline)).digest("hex");

export const REASONS = {
  "live-control":
    "The control operation is polled by a running driver, or process ownership is ended under it; retain the live control-to-driver ordering and every assertion.",
  "live-observer":
    "An observer is armed in a separate process before the event/claim that must wake it; retain the live observer-to-producer ordering.",
  "live-ownership":
    "The named timing, ownership or handover relation has a live-process witness in the body; static records cannot replace that relation.",
  "support-boundary":
    "The real support subprocess/fixture boundary is the subject; keep these assertions in test-support rather than a production command suite.",
  "dispatch-contract":
    "Retain the complete parser/routing or version/help/usage-error journey against the shipped dispatcher; its assertions prove the public parser or routing boundary.",
  "public-api":
    "No CLI command is invoked: retain the existing public-library/contract assertions against the compatibility facade. A profile-area fallback is not a reason to invent command phases.",
  "command-phases":
    "Move each command phase's exit/output/state assertions into that command's suite. Build the next phase's precondition, including failure states, with real record/journal/channel writers. Preserve every postcondition. No assertion about live ordering moves to a fixture.",
  "isolated-command":
    "Keep the original command assertions together behind that command's production entry point.",
};

export function expandCatalog(catalog) {
  if (
    catalog?.schema !== 4 ||
    Object.keys(catalog).some(
      (key) => !["schema", "baselineSha256", "dispositions"].includes(key),
    ) ||
    !/^[a-f0-9]{64}$/.test(catalog.baselineSha256) ||
    !catalog.dispositions ||
    typeof catalog.dispositions !== "object" ||
    Array.isArray(catalog.dispositions)
  )
    throw new Error("invalid decomposition catalog");
  const tests = {};
  for (const [id, entry] of Object.entries(catalog.dispositions)) {
    if (
      !/^[a-z][a-z0-9_]*::[A-Za-z_]\w*$/.test(id) ||
      !entry ||
      typeof entry.reason !== "string" ||
      !Object.hasOwn(REASONS, entry.reason) ||
      !["support", "library", "dispatch", "integration", "decompose", "isolated"].includes(
        entry.kind,
      ) ||
      (entry.rationale !== undefined &&
        (typeof entry.rationale !== "string" || !entry.rationale.trim())) ||
      !Array.isArray(entry.originalCommands) ||
      !entry.originalCommands.every(
        (name) => typeof name === "string" && /^cmd-[a-z][a-z0-9-]*$/.test(name),
      ) ||
      Object.keys(entry).some(
        (key) => !["kind", "originalCommands", "reason", "rationale"].includes(key),
      )
    )
      throw new Error(`invalid disposition ${id}`);
    if (
      (["support", "library"].includes(entry.kind) && entry.originalCommands.length !== 0) ||
      (entry.kind === "isolated" && entry.originalCommands.length !== 1) ||
      (["integration", "decompose"].includes(entry.kind) && entry.originalCommands.length < 2)
    )
      throw new Error(`disposition has incompatible commands: ${id}`);
    const owners =
      entry.kind === "support"
        ? ["test-support"]
        : entry.kind === "library"
          ? ["public-api"]
          : entry.kind === "dispatch"
            ? ["full-dispatch"]
            : entry.kind === "integration"
              ? ["cross-command"]
              : entry.originalCommands;
    if (!owners.length || new Set(owners).size !== owners.length)
      throw new Error(`disposition has no unique owners: ${id}`);
    for (const owner of owners) {
      const piece = owners.length === 1 ? id : `${id}__phase_${owner.replaceAll("-", "_")}`;
      tests[piece] = {
        original: id,
        suite: owner,
        commands:
          entry.kind === "integration"
            ? entry.originalCommands
            : ["test-support", "public-api"].includes(owner)
              ? []
              : owner === "full-dispatch"
                ? entry.originalCommands
                : [owner],
        share: 1 / owners.length,
        reason: [REASONS[entry.reason], entry.rationale].filter(Boolean).join(" "),
      };
    }
  }
  return { ...catalog, tests };
}
