// How the npm journeys start a command so that it is found on every platform.
//
// On Windows `npm`, and every command npm installs, is a `.cmd` batch file. A
// process started directly is looked up with only `.com` and `.exe` appended,
// so a bare `npm` is never found (ENOENT), and Node refuses to start a `.cmd`
// it is handed by name without a shell (EINVAL). A user's console finds it by
// cmd.exe's rule and runs it under cmd.exe; `spawnable` does the same, so the
// journeys start what a user's command line starts.

import { existsSync, statSync } from "node:fs";
import { join, win32 } from "node:path";

/// cmd.exe's own default when the environment names no PATHEXT.
const DEFAULT_PATHEXT = ".COM;.EXE;.BAT;.CMD";

/// What cmd.exe reads as syntax rather than as text.
const META = /([()\][%!^"`<>&|;, *?])/g;

/// A shim npm writes for an installed package's bin re-parses its arguments
/// through cmd.exe once more, so what reaches it is escaped twice.
const BIN_SHIM = /node_modules[\\/]\.bin[\\/][^\\/]+\.cmd$/i;

/// `[file, args, options]` to spread into any `child_process` start function, so
/// that `command` resolves as a console resolves it on `platform`. Outside Windows,
/// and on it for anything but a batch file, that is `command` unchanged.
export function spawnable(command, args = [], options = {}, platform = process.platform) {
  if (platform !== "win32") return [command, args, options];
  const env = options.env ?? process.env;
  const found = resolveWindows(command, env);
  if (!found || !/\.(cmd|bat)$/i.test(found)) return [command, args, options];
  const doubleEscape = BIN_SHIM.test(found);
  const line = [
    win32.normalize(found).replace(META, "^$1"),
    ...args.map((arg) => quote(arg, doubleEscape)),
  ].join(" ");
  return [
    lookup(env, "ComSpec") ?? "cmd.exe",
    ["/d", "/s", "/c", `"${line}"`],
    { ...options, windowsVerbatimArguments: true },
  ];
}

/// `NODE_OPTIONS` preloading `file`. Node reads a backslash inside its quotes as
/// an escape, so a Windows path is quoted as JSON is, not as it is written.
export const preloading = (file) => `--require ${JSON.stringify(file)}`;

/// The file cmd.exe runs for `command`: a name with a directory in it is taken
/// from that directory, a bare one from the first PATH entry that holds it, and
/// either with each PATHEXT extension in turn appended — the name as given only
/// when it already carries one. Undefined when nothing matches.
export function resolveWindows(command, env = process.env) {
  const extensions = (lookup(env, "PATHEXT") ?? DEFAULT_PATHEXT).split(";").filter(Boolean);
  const candidates = [
    ...(win32.extname(command) ? [command] : []),
    ...extensions.map((ext) => command + ext.toLowerCase()),
  ];
  const directories = /[\\/]/.test(command)
    ? [""]
    : (lookup(env, "PATH") ?? "")
        .split(";")
        .map((dir) => dir.replace(/^"(.*)"$/, "$1"))
        .filter(Boolean);
  for (const dir of directories) {
    for (const name of candidates) {
      // The lookup is Windows'; the filesystem probed is this host's.
      const file = dir ? join(dir, name) : name;
      if (isFile(file)) return file;
    }
  }
  return undefined;
}

/// Windows environment names are case-insensitive: a spread `process.env` there
/// carries `Path`, not `PATH`.
function lookup(env, name) {
  const key = Object.keys(env).find((k) => k.toUpperCase() === name.toUpperCase());
  return key === undefined ? undefined : env[key];
}

function isFile(file) {
  return existsSync(file) && statSync(file).isFile();
}

/// One argument as cmd.exe passes it on intact to the program the batch file
/// runs: quoted by the C runtime's rule, every quote and the backslashes before
/// it escaped, then every cmd.exe metacharacter caret-escaped.
function quote(arg, doubleEscape) {
  let quoted = `"${String(arg)
    .replace(/(\\*)"/g, '$1$1\\"')
    .replace(/(\\*)$/, "$1$1")}"`.replace(META, "^$1");
  if (doubleEscape) quoted = quoted.replace(META, "^$1");
  return quoted;
}
