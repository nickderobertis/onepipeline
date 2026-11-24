"""SPIKE (spike-git-call-sites, never landed): attribute the engine's git processes.

usage: python3 -I attribute.py TRACES_DIR CI_GIT_TIER_DIR E2E_LIST MODULE...

TRACES_DIR holds, per test `<module>.<test>`, the four files `run.sh` had the
harness write: `.git.txt` (GIT_TRACE2, brief: what CI run 3 counted), `.event.jsonl`
(GIT_TRACE2_EVENT of the engine's git: one record per process, its session id, argv,
times, repository and the `ONEVCS_SPIKE_SPAWN` id it inherited), `.harness.event.jsonl`
(the same for git the test itself started, read only for the writes it makes), and
`.spawns.jsonl` (the vendored onevcs's per-spawn caller log, the engine's own git call
sites, onevcs's in-process writes, and each `onepipeline` process's and reconcile
pass's start). CI_GIT_TIER_DIR is run 3's Linux `git-tier/`. E2E_LIST names every test
of this tree's e2e binary, one `<module>.<test>` a line: run 3's traces of tests outside
it (the smoke binary's, which share two module names) are not e2e and are left out.

Every figure printed is a count of processes in those files, or arithmetic on them.

A repeated read or a batch is **proven** removable only where its answer cannot have
moved: an answer keyed by full commit ids or the repository's fixed layout, or a read
of refs, objects or configuration with no write to the same repository — by any traced
process, or by onevcs in process — between the earlier answer and the repeat. A read
whose answer depends on the work tree's files is never proven: no trace sees a file
written without git. Everything else is printed as unproven, apart from the owner
totals.
"""

import collections
import datetime
import glob
import json
import os
import re
import sys

traces, ci_dir, e2e_list, modules = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4:]
E2E = set(open(e2e_list).read().split())

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
        return (entry, "(an onevcs CLI process the engine started)", "(an onevcs CLI process)")
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


# Commands whose answer depends on the work tree's files, which a process can change
# without starting git.
WORKTREE_READS = {"status", "ls-files", "diff-files", "check-ignore", "check-attr"}
# rev-parse options whose answer is fixed for a repository's life at one directory and
# environment (`--git-path` is not: `core.hooksPath` moves the hooks path).
FIXED_LAYOUT = LAYOUT - {"--git-path"}
DASHED_WRITES = {"receive-pack", "git-receive-pack", "unpack-objects", "index-pack"}
DASHED_READS = {"upload-pack", "git-upload-pack", "pack-objects", "rev-list", "for-each-ref"}


def subcommand_args(argv):
    """argv from the subcommand on, git's own options dropped."""
    first = os.path.basename(argv[0]) if argv else ""
    if first not in ("git", "git.exe"):
        return [first] + list(argv[1:])
    i = 1
    while i < len(argv):
        if argv[i] in ("-c", "-C", "--git-dir", "--work-tree", "--namespace"):
            i += 2
        elif argv[i].startswith("-"):
            i += 1
        else:
            return list(argv[i:])
    return []


def writes(argv):
    args = subcommand_args(argv)
    if not args:
        return False
    if args[0] in DASHED_WRITES:
        return True
    if args[0] in DASHED_READS:
        return False
    return not is_read(args)


def worktree_dependent(args):
    cmd = args[0] if args else ""
    if cmd in WORKTREE_READS:
        return True
    if cmd == "diff":
        revs = [o for o in operands(args)]
        return "--cached" not in args and "--staged" not in args and len(revs) < 2
    return False


def fixed_layout(args):
    return bool(args) and args[0] == "rev-parse" and any(a in FIXED_LAYOUT for a in args) \
        and not operands(args)


def immutable(args, status):
    """An id-keyed read whose answer no write can move: it succeeded (or, for
    `--is-ancestor`, answered either way), so every object it named was there."""
    if not id_keyed(args) or status is None:
        return False
    if args[0] == "merge-base" and "--is-ancestor" in args:
        return status in (0, 1)
    return status == 0


def epoch(iso):
    return datetime.datetime.fromisoformat(iso.replace("Z", "+00:00")).timestamp()


def load_events(path):
    procs = collections.OrderedDict()
    if not os.path.exists(path):
        return []
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
            p["start"] = epoch(e["time"])
        elif ev == "cmd_ancestry":
            p["ancestry"] = e.get("ancestry", [])
        elif ev == "def_param" and e.get("param") == "ONEVCS_SPIKE_SPAWN":
            p["spawn"] = e.get("value")
        elif ev == "def_repo":
            p["worktree"] = e.get("worktree")
        elif ev in ("exit", "atexit"):
            p["end"] = epoch(e["time"])
            if ev == "exit":
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


def engine_site(frames):
    """(call site, engine site) of a git the engine started itself."""
    ours = [norm(f) for f in frames if crate_of(f) == "onepipeline"
            and not norm(f).startswith("onepipeline::spike_git::")]
    call = ours[0] if ours else "(no onepipeline frame)"
    outer = [f for f in ours if not f.lstrip("<").startswith(("onepipeline::vcs::", "onepipeline::spans::"))]
    return call, (outer[0] if outer else call)


# ---------------------------------------------------------------- per test
tests = sorted(
    {os.path.basename(p)[: -len(".git.txt")] for p in glob.glob(f"{traces}/*.git.txt")}
)
tests = [t for t in tests if t.split(".", 1)[0] in modules]

records = []        # one per engine git process, attributed and classified
brief_total = collections.Counter()
unmatched = 0       # spawns no trace2 event names: attempts that started no process
calls = []          # one per run_fed spawn
direct_calls = []   # one per git the engine started itself
writes_by_test = {}  # test -> [(start, end, repository or None, source)]
unresolved_writes = collections.Counter()
for t in tests:
    module = t.split(".", 1)[0]
    brief_total[module] += count_starts(f"{traces}/{t}.git.txt")
    procs = load_events(f"{traces}/{t}.event.jsonl")
    harness = load_events(f"{traces}/{t}.harness.event.jsonl")
    spawns, direct_spawns, passes, verbs, inproc = {}, {}, collections.defaultdict(list), {}, []
    sp_path = f"{traces}/{t}.spawns.jsonl"
    for line in open(sp_path) if os.path.exists(sp_path) else []:
        try:
            x = json.loads(line)
        except ValueError:
            continue
        x["test"] = t
        if x["kind"] == "spawn":
            spawns[x["id"]] = x
        elif x["kind"] == "engine-spawn":
            direct_spawns[x["id"]] = x
        elif x["kind"] == "write":
            inproc.append(x)
        elif x["kind"] == "pass":
            passes[x["pid"]].append((x["t"], x["pass"]))
        elif x["kind"] == "process":
            verbs[x["pid"]] = (x.get("argv") or ["?"])[0]
    for v in passes.values():
        v.sort()

    # Which repository a directory belongs to, as onevcs resolved it at each spawn.
    repo_of = {}
    for x in spawns.values():
        if x.get("cwd") and x.get("repository"):
            repo_of[x["cwd"].rstrip("/")] = x["repository"]
            repo_of[x["repository"].rstrip("/")] = x["repository"]
            if x["repository"].endswith("/.git"):
                repo_of[x["repository"][: -len("/.git")]] = x["repository"]

    def resolve(p):
        """The repository a traced process ran in, or None where no spawn named it."""
        paths = []
        args = p["argv"]
        for i, a in enumerate(args[:-1]):
            if a in ("--git-dir", "-C"):
                paths.append(args[i + 1])
        if p.get("worktree"):
            paths.append(p["worktree"])
        if os.path.basename(args[0]) in ("git-receive-pack", "git-upload-pack") and len(args) > 1:
            paths.append(args[-1])
        if args[1:2] and args[1] in ("receive-pack", "upload-pack", "index-pack"):
            paths.extend(a for a in args[2:] if a.startswith("/"))
        for path in paths:
            path = path.strip("'").rstrip("/")
            walk = path
            while walk and walk != "/":
                if walk in repo_of:
                    return repo_of[walk]
                walk = os.path.dirname(walk)
        # A repository no spawn read, named by path (the bare remote a push lands in):
        # its own path is its key, as `common_dir` would have given it.
        for path in paths:
            if os.path.basename(args[0]).endswith(("receive-pack", "upload-pack")) or \
                    args[1:2] in (["receive-pack"], ["upload-pack"]):
                return os.path.normpath(path.strip("'"))
        return None

    boundary = []
    for source, plist in (("engine", procs), ("harness", harness)):
        # A process git started in turn (a receive-pack's unpack-objects and gc) with
        # nothing naming its repository runs in its parent's.
        by_sid = {p["sid"]: resolve(p) for p in plist}
        for p in plist:
            sid = p["sid"]
            while by_sid.get(sid) is None and "/" in sid:
                sid = sid.rsplit("/", 1)[0]
            p["repo"] = by_sid.get(sid)
        for p in plist:
            if writes(p["argv"]):
                repo = p["repo"]
                boundary.append((p["start"], p.get("end", float("inf")), repo, source))
                if repo is None:
                    unresolved_writes[(source, p.get("worktree") is not None,
                                       " ".join(shape(subcommand_args(p["argv"]))[:3]))] += 1
    for w in inproc:
        boundary.append((w["t0"], w["t1"], w.get("repository"), "onevcs in process"))
    writes_by_test[t] = boundary

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
    for d in direct_spawns.values():
        d["call"], d["engine"] = engine_site(d["frames"])
        d["verb"] = verbs.get(d["pid"], "?")

    by_spawn = collections.defaultdict(list)
    for p in procs:
        if p.get("spawn") in spawns or p.get("spawn") in direct_spawns:
            by_spawn[p["spawn"]].append(p)
        else:
            anc = p.get("ancestry") or ["?"]
            records.append({"test": t, "module": module, "cmd": command_of(p["argv"]),
                            "spawn": None, "direct_site": None, "parent": anc[0],
                            "direct": True, "argv": p["argv"]})
    unmatched += sum(1 for i in list(spawns) + list(direct_spawns) if i not in by_spawn)
    for sid, plist in by_spawn.items():
        # The process the spawn started is the one with the shortest session id; the
        # rest are what it started in turn.
        plist.sort(key=lambda p: p["sid"].count("/"))
        s = spawns.get(sid)
        d = direct_spawns.get(sid)
        if d is not None:
            d["argv"] = plist[0]["argv"]
            d["cmd"] = command_of(plist[0]["argv"])
            direct_calls.append(d)
        for k, p in enumerate(plist):
            if k and s is not None:
                s["children"].append(command_of(p["argv"]))
            records.append({"test": t, "module": module, "cmd": command_of(p["argv"]),
                            "spawn": s, "direct_site": d,
                            "parent": (p.get("ancestry") or ["?"])[0],
                            "direct": k == 0, "argv": p["argv"]})


def crossing_write(test, repository, since, until):
    """Why an answer from `since` cannot be proven still to hold at `until`: the first
    write to the same repository, or to one no spawn resolved, in between."""
    if repository is None:
        return "its own repository was not resolved"
    for start, end, repo, source in writes_by_test[test]:
        if start < until and end > since and (repo is None or repo == repository):
            return f"a write by the {source} to " + ("the same repository" if repo else "an unresolved repository")
    return None


# ---------------------------------------------------------------- classes
# Each run_fed spawn gets one class, tried in the order (e), (d), (a), (b), (c), (f); the
# processes it started in turn share it. `proven` is set on what can be removed with its
# answer unchanged; `unproven` names why a repeat or batch member is not.
TRANSFER = {"pack-objects", "unpack-objects", "index-pack"}
PASS_SCOPE = "call sites + new onevcs pass scope"
BATCH = "call sites + new onevcs batch read"


def remote_of(s):
    """What a fetch, push or ls-remote talks to: `origin` (the world's stand-in for the
    host), a path (another repository onevcs holds on this host), or another name."""
    ops = operands(s["argv"])  # after the command itself
    target = ops[0] if ops else "(default)"
    if target.startswith("/") or target.startswith("file:") or re.match(r"^[A-Za-z]:[\\/]", target):
        return "a path on this host"
    return target if target in ("origin", "(default)") else "another named remote"


def key_of(s):
    return (s["cwd"], tuple(s["argv"]), tuple(map(tuple, s.get("env") or [])), s["answer"])


last_window = {}     # (window, key) -> latest spawn with that read and answer
first_process = {}   # (test, pid, key) -> first spawn
first_test = {}      # (test, key) -> first spawn, id-keyed
for s in sorted(calls, key=lambda s: (s["test"], s["pid"], s["t0"])):
    args = s["argv"]
    s["local"] = any(c in ("upload-pack", "receive-pack", "git-upload-pack", "git-receive-pack")
                     for c in s["children"])
    s["transferred"] = any(c in TRANSFER for c in s["children"])
    s["owner"] = None
    s["unproven"] = None
    answered = s["status"] is not None
    k = key_of(s)
    kw, kp = (s["window"], k), (s["test"], s["pid"], k)
    if s["cmd"] == "ls-remote" and "--symref" in args and "default_branch" in " ".join(s["frames"]):
        s["cls"] = "e"
        s["owner"] = "onevcs"
    elif s["cmd"] in ("fetch", "push", "ls-remote"):
        s["cls"] = "d"
        s["remote"] = remote_of(s)
        # An ls-remote, or a fetch that moved no object and updated no ref (git wrote
        # nothing to stderr), against a repository on this filesystem: what it learned
        # is the remote's refs at that instant, which an in-process read of that
        # remote's refs gives at the same instant (native_refs::advertised_tips).
        s["noop"] = s["cmd"] == "fetch" and not s["transferred"] and s.get("quiet") is True
        if s["local"] and (s["cmd"] == "ls-remote" or s["noop"]):
            s["owner"] = "onevcs"
    elif is_read(args) and answered and kw in last_window:
        s["cls"] = "a"
        prior = last_window[kw]
        s["a_pair"] = ("the same onevcs function, from the same caller"
                       if (prior["entry"], prior["caller"]) == (s["entry"], s["caller"])
                       else "a different onevcs function or caller")
        s["same_scope"] = bool(s.get("scope")) and prior.get("scope") == s.get("scope") \
            and prior["thread"] == s["thread"]
        if immutable(args, s["status"]) or fixed_layout(args):
            s["owner"] = "onevcs"
        elif worktree_dependent(args):
            s["unproven"] = "it reads the work tree's files, which no trace sees written"
        else:
            s["unproven"] = crossing_write(s["test"], s.get("repository"), prior["t0"], s["t1"])
            if not s["unproven"]:
                s["owner"] = "onevcs" if s["same_scope"] else PASS_SCOPE
    elif id_keyed(args):
        s["cls"] = "b"
        if not immutable(args, s["status"]):
            s["b_repeat"] = "answered a failure, which a fetch can change"
        elif kp in first_process:
            s["b_repeat"] = "repeat in the same engine process"
            s["owner"] = "onevcs"
        elif (s["test"], k) in first_test:
            s["b_repeat"] = "repeat only in another process of the test"
            s["unproven"] = "cross-process reuse needs a store shared between processes"
        else:
            s["b_repeat"] = "first asked in the test"
    else:
        s["cls"] = None
        prior = first_process.get(kp)
        s["f_repeat"] = is_read(args) and answered and prior is not None
        s["pinnable"] = args[0] in {"merge-base", "log", "diff", "rev-list", "cat-file"} \
            and bool(operands(args)) and not id_keyed(args)
        if s["f_repeat"] and fixed_layout(args):
            s["owner"] = "onevcs"
            s["layout_repeat"] = True
    if is_read(args) and answered:
        last_window[kw] = s
        first_process.setdefault(kp, s)
    if id_keyed(args) and answered:
        first_test.setdefault((s["test"], k), s)

# (c): what is left that a batch could answer, grouped per window and repository, and cut
# wherever a write to that repository falls between two of its reads: one batch answers
# at one instant, so it may only stand in for reads no write separates.
groups = collections.defaultdict(list)
for s in calls:
    if s["cls"] is None:
        kind = batchable(s["argv"])
        if kind:
            groups[(s["window"], s["cwd"], kind)].append(s)
for g in groups.values():
    if len({tuple(x["argv"]) for x in g}) < 2:
        continue
    g.sort(key=lambda x: x["t0"])
    for x in g:
        x["cls"] = "c"
    segments, current = [], [g[0]]
    for prev, x in zip(g, g[1:]):
        cut = crossing_write(x["test"], x.get("repository"), prev["t0"], x["t1"])
        if cut:
            x["c_cut"] = cut
            segments.append(current)
            current = [x]
        else:
            current.append(x)
    segments.append(current)
    for seg in segments:
        if len({tuple(x["argv"]) for x in seg}) < 2:
            for x in seg[1:]:
                x["unproven"] = "its batch is cut by " + x.get("c_cut", "a write")
            continue
        one_call = len({(x["thread"], x.get("scope")) for x in seg}) == 1 and seg[0].get("scope")
        for x in seg[1:]:
            x["owner"] = "onevcs" if one_call else BATCH
    for x in g:
        if x is not g[0] and x["owner"] is None and x["unproven"] is None:
            x["unproven"] = "its batch is cut by " + x.get("c_cut", "a write")
for s in calls:
    if s["cls"] is None:
        s["cls"] = "f"

for r in records:
    s = r["spawn"]
    r["cls"] = s["cls"] if s else "f"
    r["owner"] = s["owner"] if s else None
    r["unproven"] = s["unproven"] if s else None

# ---------------------------------------------------------------- output
N = len(records)
direct = sum(1 for r in records if r["spawn"] and r["direct"])
engine_direct = sum(1 for r in records if r["direct_site"] and r["direct"])
print(f"# Engine git attribution over {len(tests)} tests in {len(modules)} modules (Linux, this host)")
print(f"\ngit processes (GIT_TRACE2_EVENT): {N}; brief GIT_TRACE2 `start` lines: {sum(brief_total.values())}")
print(f"\nstarted by onevcs `run_fed`: {direct}; by the engine's own git call sites: {engine_direct}; "
      f"started in turn by those: {sum(1 for r in records if (r['spawn'] or r['direct_site']) and not r['direct'])}; "
      f"started by neither: {sum(1 for r in records if not r['spawn'] and not r['direct_site'])}; "
      f"logged spawns with no trace2 event: {unmatched}")
writes_all = collections.Counter()
for b in writes_by_test.values():
    for _, _, repo, source in b:
        writes_all[(source, "resolved" if repo else "unresolved")] += 1
table("Write boundaries seen (git writes by any traced process, onevcs's in-process writes)",
      [(k[0], k[1], v) for k, v in sorted(writes_all.items())], ["by", "repository", "writes"])
table("Writes whose repository no spawn resolved (each invalidates every read it overlaps)",
      [(k[0], "yes" if k[1] else "no", k[2], v) for k, v in unresolved_writes.most_common()],
      ["by", "git named a work tree", "argv shape", "writes"])

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
        by_parent["onevcs run_fed" if r["direct"] else "in turn, under a run_fed git (parent " + r["parent"] + ")"] += 1
    elif r["direct_site"]:
        by_parent["the engine's own call site" if r["direct"] else "in turn, under an engine-started git (parent " + r["parent"] + ")"] += 1
    else:
        by_parent["neither (parent " + r["parent"] + ")"] += 1
table("Every git process, by what started it", [(k, v, pct(v, N)) for k, v in by_parent.most_common()],
      ["started by", "processes", "share"])
neither = collections.Counter((r["parent"], " ".join(shape(r["argv"][1:5])))
                              for r in records if not r["spawn"] and not r["direct_site"])
table("Started by neither, by parent and argv (every row)",
      [(k[0], k[1], v) for k, v in neither.most_common()], ["parent", "argv", "processes"])


def by_command(key, title, label, which=lambda r: r["spawn"]):
    """Every key, and every git command under it: no row is left out."""
    agg = collections.defaultdict(collections.Counter)
    for r in records:
        sp = which(r)
        if sp:
            agg[key(sp)][r["cmd"]] += 1
    rows = []
    for k, c in sorted(agg.items(), key=lambda kv: (-sum(kv[1].values()), kv[0])):
        n = sum(c.values())
        rows.append((f"`{k}`", "**all**", n, pct(n, N)))
        for cmd, v in sorted(c.items(), key=lambda cv: (-cv[1], cv[0])):
            rows.append(("", cmd, v, pct(v, N)))
    table(title, rows, [label, "git command", "processes", "share"])


by_command(lambda s: s["entry"], "Per onevcs public function (the outermost onevcs frame), by git command",
           "onevcs function")
by_command(lambda s: s["caller"], "Per direct caller of onevcs, by git command", "caller")
by_command(lambda s: s["engine"], "Per engine site (first onepipeline frame outside src/vcs.rs), by git command",
           "engine site")
by_command(lambda d: d["call"], "Per engine call site that starts git itself, by git command", "call site",
           which=lambda r: r["direct_site"])
by_command(lambda d: d["engine"], "Per engine site above the engine's own git call sites, by git command",
           "engine site", which=lambda r: r["direct_site"])
by_command(lambda s: s["verb"], "Per onepipeline verb (onevcs-started git)", "verb")

cmds = collections.Counter(r["cmd"] for r in records)
table("Every git process, by git command (every row)", [(c, v, pct(v, N)) for c, v in cmds.most_common()],
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
proven = collections.Counter(r["cls"] for r in records if r["owner"])
unproven = collections.Counter(r["cls"] for r in records if r["unproven"])
owner = collections.Counter(r["owner"] for r in records if r["owner"])
rows = []
for k in "abcdef":
    rows.append((NAMES[k], cls[k], pct(cls[k], N), proven[k], unproven[k],
                 cls[k] - proven[k] - unproven[k]))
rows.append(("**all**", N, "100%", sum(proven.values()), sum(unproven.values()),
             N - sum(proven.values()) - sum(unproven.values())))
table("Classes", rows, ["class", "processes", "share", "removable, proven", "a repeat or batch, unproven",
                        "needed"])
table("Classes, by git command (every row)", [
    (NAMES[k][:3], c, v) for k in "abcdef"
    for c, v in collections.Counter(r["cmd"] for r in records if r["cls"] == k).most_common()],
    ["class", "git command", "processes"])

owner_cls = collections.Counter((r["owner"], r["cls"]) for r in records if r["owner"])
table("Proven removable processes per owner (the figures for the tasks' bars)",
      [(k[0], k[1], v, pct(v, N)) for k, v in sorted(owner_cls.items())] +
      [(o, "all", v, pct(v, N)) for o, v in owner.most_common()] +
      [("**total proven removable**", "", sum(owner.values()), pct(sum(owner.values()), N))],
      ["owner", "class", "processes", "share of all"])
why = collections.Counter((r["cls"], r["unproven"]) for r in records if r["unproven"])
table("Unproven repeats and batch members, by why (not in the owner totals)",
      [(f"({k[0]})", k[1], v) for k, v in sorted(why.items(), key=lambda kv: (kv[0][0], -kv[1]))],
      ["class", "why it is not proven", "processes"])


def outcome(sp):
    if sp["cmd"] == "ls-remote":
        return "a ref listing"
    if sp["transferred"]:
        return "objects moved"
    return "nothing moved or updated" if sp.get("noop") else "refs updated, no objects"


def split(c, label, key):
    agg = collections.Counter()
    for r in records:
        if r["cls"] == c and r["spawn"]:
            agg[key(r)] += 1
    table(label, [(k, v) for k, v in agg.most_common()], ["", "processes"])


def by_class(key, title, label):
    agg = collections.defaultdict(collections.Counter)
    for r in records:
        if r["spawn"]:
            k = key(r["spawn"])
            agg[k][r["cls"]] += 1
            agg[k]["all"] += 1
            if r["owner"]:
                agg[k]["by " + r["owner"]] += 1
            if r["unproven"]:
                agg[k]["unproven"] += 1
    rows = []
    for k, c in sorted(agg.items(), key=lambda kv: (-kv[1]["all"], kv[0])):
        rows.append((f"`{k}`", c["all"], *(c[x] for x in "abcdef"), c["by onevcs"],
                     c["by " + PASS_SCOPE] + c["by " + BATCH], c["unproven"]))
    table(title, rows, [label, "all", "(a)", "(b)", "(c)", "(d)", "(e)", "(f)",
                        "proven, onevcs alone", "proven, call sites", "unproven"])


by_class(lambda s: s["entry"], "Per onevcs public function, by class (every row)", "onevcs function")
by_class(lambda s: s["engine"], "Per engine site, by class (every row)", "engine site")

split("a", "(a) by where the earlier answer was", lambda r: (
    ("one onevcs call (same read scope)" if r["spawn"].get("same_scope") else "an earlier onevcs call")
    + "; window: " + r["spawn"]["window"][2]))
split("a", "(a) by outcome and owner", lambda r: r["owner"] or ("unproven: " + r["unproven"]))
split("a", "(a) proven for the call sites: the earlier answer's onevcs function and caller",
      lambda r: r["spawn"]["a_pair"] if r["owner"] == PASS_SCOPE else "(not proven for the call sites)")
split("a", "(a) by git command and onevcs function, with outcome", lambda r: (
    f'{r["cmd"]} — `{r["spawn"]["entry"]}` — ' + ("proven: " + r["owner"] if r["owner"] else "unproven")))
split("b", "(b) by whether the same id-keyed read was already answered", lambda r: r["spawn"]["b_repeat"])
split("b", "(b) repeats only in another process, by whether onevcs's on-disk proof cache was on at the spawn",
      lambda r: ("proof cache on" if r["spawn"].get("proofs") else "proof cache off")
      if r["spawn"]["b_repeat"] == "repeat only in another process of the test" else "(other)")
split("b", "(b) by argv shape", lambda r: " ".join(shape(r["spawn"]["argv"])[:4]))
split("c", "(c) by owner of the removable part, or why not", lambda r: (
    (r["owner"] or ("unproven: " + r["unproven"] if r["unproven"] else "kept: the read a batch makes"))
    + " — " + r["cmd"]))
split("c", "(c) by argv shape", lambda r: " ".join(shape(r["spawn"]["argv"])[:4]))
split("d", "(d) by command, remote and outcome, with every process each started",
      lambda r: f'{"removable" if r["owner"] else "needed"} — {r["spawn"]["cmd"]} to '
                f'{r["spawn"]["remote"]}: {outcome(r["spawn"])}')
split("e", "(e) by caller and process", lambda r: f'`{r["spawn"]["caller"]}` — {"direct" if r["direct"] else r["cmd"]}')
split("f", "(f) by argv shape", lambda r: " ".join(shape(r["spawn"]["argv"])[:4]))


def f_repeat_kind(r):
    sp = r["spawn"]
    if not sp.get("f_repeat"):
        return "not a repeat in the process"
    if sp.get("layout_repeat"):
        return "a repeat of a fixed layout read (proven, onevcs)"
    return "a repeat across passes of a read that can move (needed: ruling a0 point 5)"


split("f", "(f) reads already answered the same earlier in the same process", f_repeat_kind)
split("f", "(f) the (b) commands asked by a branch name rather than a commit id",
      lambda r: f'{r["cmd"]} by name' if r["spawn"].get("pinnable") else "(other)")

# The functions the plan's git-reduction tasks name, wherever they sit on the stack.
NAMED = ["onepipeline::vcs::working_session", "onepipeline::vcs::session_tip",
         "onepipeline::vcs::worktree_head", "onepipeline::vcs::evidence_in",
         "onepipeline::vcs::level_with_base",
         "onepipeline::vcs::LandingPass", "onepipeline::engine::landings_after_asking_again",
         "onepipeline::views::read_side_by_side", "onevcs::vcs::settle_published",
         "onevcs::git::fetch_tracking", "onevcs::git::remote_tip_read",
         "onevcs::git::default_branch"]
rows = []
for name in NAMED:
    n = rem = unp = 0
    for r in records:
        sp = r["spawn"] or r["direct_site"]
        if sp and any(name in norm(f) for f in sp["frames"]):
            n += 1
            rem += bool(r["owner"])
            unp += bool(r["unproven"])
    rows.append((f"`{name}`", n, pct(n, N), rem, unp))
table("Processes under the functions the plan's tasks name (anywhere on the stack)", rows,
      ["function", "processes", "share", "proven removable", "unproven"])

# Machine-readable totals for the report.
json.dump({
    "tests": len(tests), "processes": N, "brief": sum(brief_total.values()),
    "classes": dict(cls), "proven": dict(proven), "unproven": dict(unproven), "owner": dict(owner),
    "ci": dict(ci), "here": dict(brief_total), "ci_all": ci_all,
}, open(os.path.join(os.path.dirname(traces.rstrip("/")), "summary.json"), "w"), indent=1)
