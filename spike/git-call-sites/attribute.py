"""SPIKE (spike-git-call-sites, never landed): attribute the engine's git processes.

usage: python3 -I attribute.py TRACES_DIR CI_GIT_TIER_DIR E2E_LIST MODULE...

TRACES_DIR holds, per test `<module>.<test>`, the three files `run.sh` had the
harness write: `.git.txt` (GIT_TRACE2, brief: what CI run 3 counted), `.event.jsonl`
(GIT_TRACE2_EVENT: one record per git process, its session id, its argv, and the
`ONEVCS_SPIKE_SPAWN` id it inherited), and `.spawns.jsonl` (the vendored onevcs's
per-spawn caller log, plus each `onepipeline` process's start and each reconcile
pass's start). CI_GIT_TIER_DIR is run 3's Linux `git-tier/`. E2E_LIST names every test
of this tree's e2e binary, one `<module>.<test>` a line: run 3's traces of tests outside
it (the smoke binary's, which share two module names) are not e2e and are left out.

Every figure printed is a count of processes in those files, or arithmetic on them.
"""

import collections
import glob
import json
import os
import re
import sys

traces, ci_dir, e2e_list, modules = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4:]
E2E = set(open(e2e_list).read().split())

FULL = re.compile(r"^[0-9a-f]{40}$")
ID_REV = re.compile(
    r"^\^?[0-9a-f]{40}(?:[~^]\d*)*(?:\^\{[a-z]*\})?"
    r"(?:\.\.\.?\^?[0-9a-f]{40}(?:[~^]\d*)*(?:\^\{[a-z]*\})?)?$"
)
OBJECT_TYPES = {"commit", "tree", "blob", "tag"}
# Commands that change a repository (or its config): never a "read" for class (a).
WRITES = {
    "fetch", "push", "clone", "commit", "commit-tree", "merge", "checkout", "switch",
    "reset", "add", "rm", "mv", "rebase", "cherry-pick", "revert", "update-ref",
    "update-index", "read-tree", "write-tree", "gc", "prune", "repack", "init",
    "pack-refs", "stash", "apply", "am", "tag", "notes", "maintenance", "worktree",
    "branch", "remote", "config", "symbolic-ref", "clean", "restore", "hash-object",
    "pull", "merge-tree", "replace", "index-pack", "unpack-objects", "receive-pack",
    "upload-pack", "lfs",
}
# Reads one `for-each-ref` (refs, ahead-behind, --merged) or one
# `cat-file --batch` (objects by name) could answer for a whole set at one instant.
BATCH_REFS = {"rev-parse", "show-ref", "for-each-ref", "symbolic-ref"}
BATCH_OBJECTS = {"cat-file"}
# rev-parse options that ask where the repository's files are rather than about a ref.
LAYOUT = {"--git-common-dir", "--git-dir", "--git-path", "--local-env-vars", "--show-toplevel",
          "--is-inside-work-tree", "--absolute-git-dir", "--show-prefix", "--is-bare-repository",
          "--show-cdup"}


def command_of(argv):
    """The git command a process ran: its subcommand, or the dashed program."""
    if not argv:
        return "?"
    first = os.path.basename(argv[0])
    if first not in ("git", "git.exe"):
        return first
    i = 1
    while i < len(argv):
        a = argv[i]
        if a in ("-c", "-C", "--git-dir", "--work-tree", "--namespace"):
            i += 2
            continue
        if a.startswith("-"):
            i += 1
            continue
        return a
    return argv[1] if len(argv) > 1 else "git"


def operands(argv):
    """Non-option arguments after the command, up to `--`."""
    out = []
    for a in argv[1:]:
        if a == "--":
            break
        if not a.startswith("-"):
            out.append(a)
    return out


def shape(argv):
    """An argv with commit ids, paths and branch names replaced by placeholders."""
    out = []
    for a in argv:
        a = re.sub(r"[0-9a-f]{40}", "<id>", a)
        a = re.sub(r"(/tmp|/home)/\S+", "<path>", a)
        if not a.startswith("-") and not a.startswith("<") and ("/" in a and not a.startswith("refs/")):
            a = "<ref>"
        out.append(a)
    return out


def is_read(args):
    cmd = args[0] if args else ""
    if cmd == "config":
        return "--get" in args or "--list" in args or "--get-regexp" in args
    if cmd == "remote":
        return len(args) == 1 or args[1] in ("get-url", "show", "-v")
    if cmd == "symbolic-ref":
        return len(operands(args)) == 1
    if cmd == "branch":
        return any(a in args for a in ("--list", "-l", "--format", "-r", "-a", "--contains", "--merged"))
    if cmd == "worktree":
        return len(args) > 1 and args[1] == "list"
    if cmd == "tag":
        return any(a.startswith("--contains") or a in ("-l", "--list") for a in args)
    return cmd not in WRITES and cmd not in ("fetch", "push", "clone")


def id_keyed(args):
    """An immutable read keyed by full commit ids alone (class (b))."""
    cmd = args[0] if args else ""
    if cmd not in {"merge-base", "log", "diff", "merge-tree", "rev-list", "cat-file",
                   "ls-tree", "diff-tree", "show", "rev-parse", "name-rev"}:
        return False
    ops = [o for o in operands(args) if not (cmd == "cat-file" and o in OBJECT_TYPES)]
    return bool(ops) and all(ID_REV.match(o) for o in ops)


def batchable(args):
    """Which batch could answer this read with others like it, if any (class (c))."""
    cmd = args[0] if args else ""
    if cmd in BATCH_OBJECTS:
        return "objects"
    if cmd == "rev-parse" and (any(a in LAYOUT for a in args) or not operands(args)):
        return None  # where the repository's files are, not a ref or an object
    if cmd in BATCH_REFS and is_read(args):
        return "refs"
    if cmd == "rev-list" and "--count" in args:
        return "refs"  # %(ahead-behind:BASE)
    if cmd == "merge-base" and "--is-ancestor" in args:
        return "refs"  # --merged / --contains over the set
    if cmd == "log" and any(a in args for a in ("-1", "-n1", "--max-count=1")):
        return "objects"
    return None


def norm(frame):
    """A frame's function, without closure markers, generic arguments or hash."""
    f = re.sub(r"::\{closure#\d+\}", "", frame)
    f = re.sub(r"::\{shim:[^}]*\}", "", f)
    # Drop `::<...>` generic argument lists (balanced).
    out, depth, i = [], 0, 0
    while i < len(f):
        if f.startswith("::<", i) and depth == 0:
            depth, i = 1, i + 3
            while i < len(f) and depth:
                depth += {"<": 1, ">": -1}.get(f[i], 0)
                i += 1
            continue
        out.append(f[i])
        i += 1
    return re.sub(r"::h[0-9a-f]{16}$", "", "".join(out))


def crate_of(frame):
    bare = frame.lstrip("<")
    head = bare.split("::", 1)[0]
    return head if re.match(r"^[a-z_][a-z0-9_]*$", head) else "?"


OURS = {"onevcs", "onepipeline", "oneagentgraph", "onevcs_testing", "onetaskgraph_core"}


def site(frames):
    """(onevcs entry, direct caller, engine site) for a spawn's frames, innermost first.

    The entry is the outermost onevcs frame; the caller the nearest frame outward of
    it in another known crate; the engine site the nearest `onepipeline` frame outward
    that is not in `onepipeline::vcs` (the engine's onevcs wrappers) or
    `onepipeline::spans` (the span recorder they run inside)."""
    idx = [i for i, f in enumerate(frames) if crate_of(f) == "onevcs"]
    if not idx:
        return ("(no onevcs frame)", "?", "?")
    entry_i = max(idx)
    entry = norm(frames[entry_i])
    if entry == "onevcs::main":
        return (entry, "(an `onevcs` CLI process the engine started)", "(an `onevcs` CLI process)")
    caller = engine = None
    for f in frames[entry_i + 1:]:
        c = crate_of(f)
        if c in OURS and caller is None:
            caller = norm(f)
        if c == "onepipeline" and not norm(f).lstrip("<").startswith(("onepipeline::vcs::", "onepipeline::spans::")):
            engine = norm(f)
            break
    # No caller on the stack: the git ran on a thread onevcs itself started.
    return (entry, caller or "(an onevcs worker thread)", engine or "(an onevcs worker thread)")


def src_file(fn):
    """The engine source file a `onepipeline::…` function lives in."""
    bare = fn.lstrip("<").split(" as ")[0]
    parts = bare.split("::")
    if parts[0] != "onepipeline" or len(parts) < 3:
        return fn if fn.startswith("(") else "(" + crate_of(fn) + ")"
    return "src/" + parts[1] + ".rs"


def load_events(path):
    procs = collections.OrderedDict()
    for line in open(path, errors="replace"):
        try:
            e = json.loads(line)
        except ValueError:
            continue
        sid = e.get("sid")
        p = procs.setdefault(sid, {"sid": sid})
        ev = e.get("event")
        if ev == "start":
            p["argv"] = e.get("argv", [])
            p["time"] = e.get("time")
        elif ev == "cmd_ancestry":
            p["ancestry"] = e.get("ancestry", [])
        elif ev == "cmd_name":
            p["name"] = e.get("name")
        elif ev == "def_param" and e.get("param") == "ONEVCS_SPIKE_SPAWN":
            p["spawn"] = e.get("value")
        elif ev == "exit":
            p["code"] = e.get("code")
    return [p for p in procs.values() if "argv" in p]


def count_starts(path):
    if not os.path.exists(path):
        return 0
    return sum(1 for line in open(path, errors="replace") if line.startswith("start "))


def table(title, rows, header):
    print(f"\n### {title}\n")
    print("| " + " | ".join(header) + " |")
    print("| " + " | ".join(["---"] + ["---:"] * (len(header) - 1)) + " |")
    for r in rows:
        print("| " + " | ".join(str(x) for x in r) + " |")


def pct(n, d):
    return f"{100.0 * n / d:.1f}%" if d else "-"


# ---------------------------------------------------------------- per test
tests = sorted(
    {os.path.basename(p)[: -len(".git.txt")] for p in glob.glob(f"{traces}/*.git.txt")}
)
tests = [t for t in tests if t.split(".", 1)[0] in modules]

records = []  # one per git process, attributed and classified
brief_total = collections.Counter()
event_total = collections.Counter()
outside = collections.Counter()
unmatched = 0  # run_fed spawns no trace2 event names
calls = []  # one per run_fed spawn
for t in tests:
    module = t.split(".", 1)[0]
    brief_total[module] += count_starts(f"{traces}/{t}.git.txt")
    procs = load_events(f"{traces}/{t}.event.jsonl")
    event_total[module] += len(procs)
    spawns, passes, verbs = {}, collections.defaultdict(list), {}
    sp_path = f"{traces}/{t}.spawns.jsonl"
    for line in open(sp_path) if os.path.exists(sp_path) else []:
        try:
            x = json.loads(line)
        except ValueError:
            continue
        if x["kind"] == "spawn":
            x["test"] = t
            spawns[x["id"]] = x
        elif x["kind"] == "pass":
            passes[x["pid"]].append((x["t"], x["pass"]))
        elif x["kind"] == "process":
            verbs[x["pid"]] = (x.get("argv") or ["?"])[0]
    for v in passes.values():
        v.sort()

    # The window a spawn belongs to: its engine process's reconcile pass, or the
    # whole process where that process runs no loop.
    for s in spawns.values():
        pid = s["pid"]
        win = None
        for t0, n in passes.get(pid, []):
            if t0 <= s["t0"]:
                win = n
        s["window"] = (t, pid, "pass", win) if passes.get(pid) and win is not None else (
            (t, pid, "pre-loop", None) if passes.get(pid) else (t, pid, "process", None)
        )
        s["verb"] = verbs.get(pid, "?")
        s["entry"], s["caller"], s["engine"] = site(s["frames"])
        s["cmd"] = command_of(["git"] + s["argv"])
        s["children"] = []
        calls.append(s)

    by_spawn = collections.defaultdict(list)
    for p in procs:
        if p.get("spawn") in spawns:
            by_spawn[p["spawn"]].append(p)
        else:
            anc = p.get("ancestry") or ["?"]
            outside[(anc[0], command_of(p["argv"]), " ".join(shape(p["argv"][1:5])))] += 1
            records.append({"test": t, "module": module, "cmd": command_of(p["argv"]),
                            "spawn": None, "parent": anc[0], "direct": True})
    unmatched += sum(1 for i in spawns if i not in by_spawn)
    for sid_list in by_spawn.values():
        # The process run_fed started is the one with the shortest session id; the
        # rest are what it started in turn.
        sid_list.sort(key=lambda p: p["sid"].count("/"))
        s = spawns[sid_list[0]["spawn"]]
        for k, p in enumerate(sid_list):
            if k:
                s["children"].append(command_of(p["argv"]))
            records.append({"test": t, "module": module, "cmd": command_of(p["argv"]),
                            "spawn": s, "parent": (p.get("ancestry") or ["?"])[0],
                            "direct": k == 0})

# ---------------------------------------------------------------- classes
# Each run_fed spawn gets one class, tried in the order (e), (d), (a), (b), (c), (f); the
# processes it started in turn share it.
TRANSFER = {"pack-objects", "unpack-objects", "index-pack"}


def remote_of(s):
    """What a fetch, push or ls-remote talks to: `origin` (the world's stand-in for the
    host), a path (another repository onevcs holds on this host), or another name."""
    ops = operands(s["argv"])  # after the command itself
    target = ops[0] if ops else "(default)"
    if target.startswith("/") or target.startswith("file:") or re.match(r"^[A-Za-z]:[\\/]", target):
        return "a path on this host"
    return target if target in ("origin", "(default)") else "another named remote"


seen_window = {}     # (window, cwd, argv, answer) -> first spawn
seen_process = {}    # (test, pid, cwd, argv, answer) -> first spawn, any read
seen_test = {}       # (test, cwd, argv, answer) for id-keyed reads, across processes
for s in sorted(calls, key=lambda s: (s["test"], s["pid"], s["t0"])):
    args = s["argv"]
    key_args = tuple(args)
    s["local"] = any(c in ("upload-pack", "receive-pack", "git-upload-pack", "git-receive-pack")
                     for c in s["children"])
    s["transferred"] = any(c in TRANSFER for c in s["children"])
    s["owner"] = None
    s["removable"] = False
    answered = s["status"] is not None
    kw = (s["window"], s["cwd"], key_args, s["answer"])
    kp = (s["test"], s["pid"], s["cwd"], key_args, s["answer"])
    if s["cmd"] == "ls-remote" and "--symref" in args and "default_branch" in " ".join(s["frames"]):
        s["cls"] = "e"
        s["removable"], s["owner"] = True, "onevcs"
    elif s["cmd"] in ("fetch", "push", "ls-remote"):
        s["cls"] = "d"
        s["remote"] = remote_of(s)
        # An ls-remote, or a fetch that moved no object and updated no ref (git wrote
        # nothing to stderr), against a repository on this filesystem: all it learned
        # is the remote's refs, which onevcs already reads in process for such a
        # remote (native_refs::advertised_tips).
        s["noop"] = s["cmd"] == "fetch" and not s["transferred"] and s.get("quiet") is True
        if s["local"] and (s["cmd"] == "ls-remote" or s["noop"]):
            s["removable"], s["owner"] = True, "onevcs"
    elif is_read(args) and answered and kw in seen_window:
        s["cls"] = "a"
        first = seen_window[kw]
        s["gap"] = s["t0"] - first["t1"]
        s["same_scope"] = bool(s.get("scope")) and first.get("scope") == s.get("scope") \
            and first["thread"] == s["thread"]
        # Asked again after a wait of a second or more, it is a poll: the same answer
        # was not knowable before asking, so it is needed.
        s["removable"] = s["gap"] < 1.0
        # An id-keyed answer, or where the repository's files are, never moves, so
        # onevcs can keep it for the process; anything else is only safe to reuse while
        # the caller says nothing has moved — for one pass, which onevcs cannot see.
        layout = s["cmd"] == "rev-parse" and any(a in LAYOUT for a in args)
        s["owner"] = None if not s["removable"] else \
            "onevcs" if (id_keyed(args) or layout or s["same_scope"]) else \
            "call sites + new onevcs pass scope"
    elif id_keyed(args):
        s["cls"] = "b"
        if answered and kp in seen_process:
            s["b_repeat"] = "same engine process, earlier pass"
            s["removable"], s["owner"] = True, "onevcs"
        elif answered and (s["test"], s["cwd"], key_args, s["answer"]) in seen_test:
            s["b_repeat"] = "another process of the same test"
            s["removable"], s["owner"] = True, "onevcs"
        else:
            s["b_repeat"] = "first asked in the test"
    else:
        s["cls"] = None
        s["f_repeat"] = is_read(args) and answered and kp in seen_process
        # The (b) commands asked by branch name: id-keyed once the name is resolved.
        s["pinnable"] = args[0] in {"merge-base", "log", "diff", "rev-list", "cat-file"} \
            and bool(operands(args)) and not id_keyed(args)
        # Where the repository's files are does not move while it exists: a repeat of
        # that read in the same process, in any pass, is a repeat onevcs can keep.
        if s["f_repeat"] and s["cmd"] == "rev-parse" and any(a in LAYOUT for a in args):
            s["removable"], s["owner"] = True, "onevcs"
            s["layout_repeat"] = True
    if is_read(args) and answered:
        seen_window.setdefault(kw, s)
        seen_process.setdefault(kp, s)
    if id_keyed(args) and answered:
        seen_test.setdefault((s["test"], s["cwd"], key_args, s["answer"]), s)

# (c): what is left that a batch could answer, grouped per window and repository.
groups = collections.defaultdict(list)
for s in calls:
    if s["cls"] is None:
        kind = batchable(s["argv"])
        if kind:
            groups[(s["window"], s["cwd"], kind)].append(s)
for g in groups.values():
    if len({tuple(x["argv"]) for x in g}) < 2:
        continue
    scopes = {(x["thread"], x.get("scope")) for x in g}
    one_call = len(scopes) == 1 and next(iter(scopes))[1]
    for x in g:
        x["cls"] = "c"
    for x in g[1:]:
        x["removable"] = True
        x["owner"] = "onevcs" if one_call else "call sites + new onevcs batch read"
for s in calls:
    if s["cls"] is None:
        s["cls"] = "f"

for r in records:
    s = r["spawn"]
    r["cls"] = s["cls"] if s else "f"
    r["owner"] = s["owner"] if s and s["removable"] else None

# ---------------------------------------------------------------- output
N = len(records)
direct = sum(1 for r in records if r["spawn"] and r["direct"])
print(f"# Engine git attribution over {len(tests)} tests in {len(modules)} modules (Linux, this host)")
print(f"\ngit processes (GIT_TRACE2_EVENT): {N}; brief GIT_TRACE2 `start` lines: {sum(brief_total.values())}")
print(f"started by onevcs `run_fed`: {direct}; started in turn by those: "
      f"{sum(1 for r in records if r['spawn'] and not r['direct'])}; started outside run_fed: "
      f"{sum(1 for r in records if not r['spawn'])}; run_fed spawns with no trace2 event: {unmatched}")

# Cross-check against CI run 3.
ci = collections.Counter()
ci_tests = collections.Counter()
for p in glob.glob(f"{ci_dir}/*.spans.git.txt"):
    t = os.path.basename(p)[: -len(".spans.git.txt")]
    m = t.split(".", 1)[0]
    if m in modules and t in E2E:
        ci[m] += count_starts(p)
        ci_tests[m] += 1
ci_all = sum(count_starts(p) for p in glob.glob(f"{ci_dir}/*.*.spans.git.txt")
             if os.path.basename(p)[: -len(".spans.git.txt")] in E2E)
local_tests = collections.Counter(t.split(".", 1)[0] for t in tests)
rows = []
for m in sorted(modules, key=lambda m: -ci[m]):
    d = brief_total[m] - ci[m]
    rows.append((m, ci_tests[m], ci[m], pct(ci[m], ci_all), local_tests[m], brief_total[m],
                 f"{d:+d}", pct(d, ci[m])))
rows.append(("**total**", sum(ci_tests.values()), sum(ci.values()), pct(sum(ci.values()), ci_all),
             len(tests), sum(brief_total.values()),
             f"{sum(brief_total.values()) - sum(ci.values()):+d}",
             pct(sum(brief_total.values()) - sum(ci.values()), sum(ci.values()))))
table(f"Per module: CI run 3 Linux against this host (run 3's e2e traces total {ci_all})", rows,
      ["module", "run 3 traced tests", "run 3 git", "share of run 3", "here tests", "here git", "here − run 3", "diff"])

# Per test, the largest differences.
per_test_here = collections.Counter()
for t in tests:
    per_test_here[t] = count_starts(f"{traces}/{t}.git.txt")
per_test_ci = {}
for p in glob.glob(f"{ci_dir}/*.spans.git.txt"):
    t = os.path.basename(p)[: -len(".spans.git.txt")]
    if t.split(".", 1)[0] in modules and t in E2E:
        per_test_ci[t] = count_starts(p)
diffs = sorted(set(per_test_here) | set(per_test_ci),
               key=lambda t: -abs(per_test_here.get(t, 0) - per_test_ci.get(t, 0)))
table("Tests differing most from run 3", [
    (t, per_test_ci.get(t, "absent"), per_test_here.get(t, "absent"),
     f"{per_test_here.get(t, 0) - per_test_ci.get(t, 0):+d}") for t in diffs[:15]],
    ["test", "run 3", "here", "diff"])

# Which differences the code between run 3 and this tree explains: #1837 put the
# clock-bound journeys on a clock the test controls (`.on_journey_clock()`), so their
# polling passes no longer follow the wall clock.
E2E_SRC = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "tests", "e2e")
on_clock = set()
for m in modules:
    path = os.path.join(E2E_SRC, f"{m}.rs")
    if not os.path.exists(path):
        continue
    text = open(path).read()
    for mt in re.finditer(r"\nfn ([a-z0-9_]+)\(\)", text):
        end = text.find("\n}\n", mt.end())
        if "on_journey_clock()" in text[mt.end(): end]:
            on_clock.add(f"{m}.{mt.group(1)}")
rows = []
for label, keep in (("on the journey clock (#1837)", lambda t: t in on_clock),
                    ("every other test", lambda t: t not in on_clock)):
    ts = [t for t in set(per_test_here) | set(per_test_ci) if keep(t)]
    a, b = sum(per_test_ci.get(t, 0) for t in ts), sum(per_test_here.get(t, 0) for t in ts)
    absd = sum(abs(per_test_here.get(t, 0) - per_test_ci.get(t, 0)) for t in ts)
    rows.append((label, len(ts), a, b, f"{b - a:+d}", pct(b - a, a), absd))
table("Run 3 against this host, split by the clock change between them", rows,
      ["tests", "count", "run 3", "here", "diff", "diff %", "sum of per-test |diff|"])

# Who started them.
by_parent = collections.Counter()
for r in records:
    if r["spawn"]:
        by_parent["onevcs run_fed" if r["direct"] else "child of a run_fed git (" + r["parent"] + ")"] += 1
    else:
        by_parent["outside run_fed (parent " + r["parent"] + ")"] += 1
table("Every git process, by what started it", [(k, v, pct(v, N)) for k, v in by_parent.most_common()],
      ["started by", "processes", "share"])
table("Started outside run_fed, by parent and argv", [
    (k[0], k[2], v, pct(v, N)) for k, v in outside.most_common(25)], ["parent", "argv", "processes", "share"])


def by(key, limit, title, label):
    agg = collections.defaultdict(collections.Counter)
    for r in records:
        if r["spawn"]:
            agg[key(r["spawn"])][r["cmd"]] += 1
    rows = []
    for k, c in sorted(agg.items(), key=lambda kv: -sum(kv[1].values()))[:limit]:
        n = sum(c.values())
        top = ", ".join(f"{cmd} {v}" for cmd, v in c.most_common(6))
        rows.append((f"`{k}`", n, pct(n, N), top))
    table(title, rows, [label, "processes", "share", "by git command (leading six)"])


by(lambda s: s["entry"], 40, "Per onevcs public function (the outermost onevcs frame)", "onevcs function")
by(lambda s: src_file(s["caller"]), 30, "Per onepipeline file of the direct caller", "file")
by(lambda s: s["caller"], 50, "Per direct caller of onevcs", "caller")
by(lambda s: s["engine"], 40, "Per engine site (first onepipeline frame outside src/vcs.rs)", "engine site")
by(lambda s: s["verb"], 20, "Per onepipeline verb (the process that started them)", "verb")
by(lambda s: s["window"][2], 5, "Per window kind", "window")

# Commands overall.
cmds = collections.Counter(r["cmd"] for r in records)
table("Every git process, by git command", [(c, v, pct(v, N)) for c, v in cmds.most_common(25)],
      ["command", "processes", "share"])

# Classes.
NAMES = {
    "a": "(a) same read, same answer, repeated in one pass of one engine process",
    "b": "(b) immutable read keyed by full commit ids",
    "c": "(c) per-node/per-branch reads of one repository in one pass one batch could answer",
    "d": "(d) local-transport fetch, push, ls-remote, with what each starts",
    "e": "(e) default_branch's fallback to ls-remote --symref",
    "f": "(f) the rest",
}
cls = collections.Counter(r["cls"] for r in records)
removable = collections.Counter(r["cls"] for r in records if r["owner"])
owner = collections.Counter(r["owner"] for r in records if r["owner"])
rows = []
for k in "abcdef":
    sub = collections.Counter(r["cmd"] for r in records if r["cls"] == k)
    rows.append((NAMES[k], cls[k], pct(cls[k], N), removable[k], cls[k] - removable[k],
                 ", ".join(f"{c} {v}" for c, v in sub.most_common(6))))
rows.append(("**all**", N, "100%", sum(removable.values()), N - sum(removable.values()), ""))
table("Classes", rows, ["class", "processes", "share", "removable", "needed", "by git command (leading six)"])

owner_cls = collections.Counter((r["owner"], r["cls"]) for r in records if r["owner"])
table("Removable processes per owner", [(k[0], k[1], v, pct(v, N)) for k, v in sorted(owner_cls.items())] +
      [(o, "all", v, pct(v, N)) for o, v in owner.most_common()] +
      [("**total removable**", "", sum(owner.values()), pct(sum(owner.values()), N))],
      ["owner", "class", "processes", "share of all"])


def outcome(sp):
    if sp["cmd"] == "ls-remote":
        return "a ref listing"
    if sp["transferred"]:
        return "objects moved"
    return "nothing moved or updated" if sp.get("noop") else "refs updated, no objects"


def split(c, label, key, limit=20):
    agg = collections.Counter()
    for r in records:
        if r["cls"] == c and r["spawn"]:
            agg[key(r)] += 1
    table(label, [(k, v) for k, v in agg.most_common(limit)], ["", "processes"])


def by_class(key, limit, title, label):
    agg = collections.defaultdict(collections.Counter)
    for r in records:
        if r["spawn"]:
            k = key(r["spawn"])
            agg[k][r["cls"]] += 1
            agg[k]["all"] += 1
            if r["owner"]:
                agg[k]["removable"] += 1
                agg[k]["by " + r["owner"]] += 1
    rows = []
    for k, c in sorted(agg.items(), key=lambda kv: -kv[1]["all"])[:limit]:
        rows.append((f"`{k}`", c["all"], *(c[x] for x in "abcdef"), c["removable"],
                     c["by onevcs"], c["by call sites + new onevcs pass scope"]
                     + c["by call sites + new onevcs batch read"]))
    table(title, rows, [label, "all", "(a)", "(b)", "(c)", "(d)", "(e)", "(f)", "removable",
                        "of it onevcs alone", "of it call sites"])


by_class(lambda s: s["entry"], 15, "Per onevcs public function, by class", "onevcs function")
by_class(lambda s: s["engine"], 15, "Per engine site, by class", "engine site")


split("a", "(a) by where the earlier answer was", lambda r: (
    ("one onevcs call (same read scope)" if r["spawn"].get("same_scope") else "an earlier onevcs call")
    + (", id-keyed" if id_keyed(r["spawn"]["argv"]) else "") + "; window: " + r["spawn"]["window"][2]))
split("a", "(a) by engine site", lambda r: f'`{r["spawn"]["engine"]}`', limit=12)


def gap_band(g):
    for edge in (0.1, 1, 5, 30):
        if g < edge:
            return f"under {edge} s"
    return "30 s or more"


split("a", "(a) by time since the same read was first answered in the pass",
      lambda r: gap_band(r["spawn"]["gap"]))
split("a", "(a) under `publish_rereading_the_merge_path`, by time since the first answer",
      lambda r: gap_band(r["spawn"]["gap"]) if r["spawn"]["engine"].endswith("publish_rereading_the_merge_path")
      else "(another engine site)")
split("a", "(a) by git command and onevcs function", lambda r: f'{r["cmd"]} — `{r["spawn"]["entry"]}`')
split("b", "(b) by whether the same id-keyed read was already answered", lambda r: r["spawn"]["b_repeat"])
split("b", "(b) by git command and argv shape", lambda r: " ".join(shape(r["spawn"]["argv"])[:4]))
split("c", "(c) by owner of the removable part (blank: the one read a batch keeps)",
      lambda r: (r["owner"] or "kept") + " — " + r["cmd"])
split("c", "(c) by argv shape", lambda r: " ".join(shape(r["spawn"]["argv"])[:4]))
split("d", "(d) by command, remote, transfer and process", lambda r: (
    f'{r["spawn"]["cmd"]} to {r["spawn"]["remote"]}, ' + outcome(r["spawn"])
    + (", local transport" if r["spawn"]["local"] else ", no local transport child")
    + ("" if r["direct"] else " — started in turn: " + r["cmd"])), limit=40)
split("d", "(d) removable or needed", lambda r: "removable" if r["owner"] else "needed")

split("d", "(d) by command, remote and outcome, with every process each started",
      lambda r: f'{"removable" if r["owner"] else "needed"} — {r["spawn"]["cmd"]} to '
                f'{r["spawn"]["remote"]}: {outcome(r["spawn"])}')
split("e", "(e) by caller and process", lambda r: f'`{r["spawn"]["caller"]}` — {"direct" if r["direct"] else r["cmd"]}')
split("f", "(f) by git command and argv shape", lambda r: " ".join(shape(r["spawn"]["argv"])[:4]))
split("f", "(f) reads already answered the same in an earlier pass of the same process",
      lambda r: ("yes, a layout read (removable)" if r["spawn"].get("layout_repeat") else "yes")
      if r["spawn"].get("f_repeat") else "no")
split("f", "(f) repeats across passes, by argv shape",
      lambda r: " ".join(shape(r["spawn"]["argv"])[:4]) if r["spawn"].get("f_repeat") else "(not a repeat)")

split("f", "(f) the (b) commands asked by a branch name rather than a commit id",
      lambda r: f'{r["cmd"]} by name' if r["spawn"].get("pinnable") else "(other)")
split("a", "(a) removable, or a re-read after a wait (needed)",
      lambda r: "removable" if r["owner"] else "re-read after a wait of 1 s or more")

f_out = collections.Counter()
for r in records:
    if not r["spawn"]:
        f_out[r["parent"]] += 1
table("(f) started outside run_fed, by parent", list(f_out.most_common()), ["parent", "processes"])

# Machine-readable totals for the report.
json.dump({
    "tests": len(tests), "processes": N, "brief": sum(brief_total.values()),
    "classes": dict(cls), "removable": dict(removable), "owner": dict(owner),
    "ci": dict(ci), "here": dict(brief_total), "ci_all": ci_all,
}, open(os.path.join(os.path.dirname(traces.rstrip("/")), "summary.json"), "w"), indent=1)
