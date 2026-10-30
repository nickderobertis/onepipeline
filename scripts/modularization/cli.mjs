// The exit-status contract every modularization CLI keeps: 0 on success; 1 when an input is refused,
// a check finds drift, or a path cannot be read or written, with the cause and the next step on
// stderr; 2 when the arguments do not match the usage the CLI prints.

/** Arguments that do not match a CLI's usage; the CLI exits 2 for it. */
export class UsageError extends Error {}

/** The exit status the contract above gives a CLI's failure. */
export const exitStatus = (error) => (error instanceof UsageError ? 2 : 1);
