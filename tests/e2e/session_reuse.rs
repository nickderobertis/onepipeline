//! A node pinned to a branch a stopped run left work on.
//!
//! A retry arrives as the same pin the run before it carried, and `onevcs` keeps
//! that run's session: the clone, the worktree, and the branch the work is
//! committed on. Taking it up is the sibling's decision and `onevcs` 0.4.2 is
//! where it makes it — below that pin every retry of a stranded node was refused
//! before it dispatched, with the work sitting on a branch no plan could reach.
//! What this file holds is that the engine *linked* against a sibling that takes
//! the session up, which is a fact about `Cargo.lock` rather than about any code
//! here: the first journey fails on the lockfile alone.
//!
//! The second journey is the boundary. Reuse is narrow on purpose — one open
//! record, on this identity, this branch, this base, this execution checkout,
//! with its run root still on disk and nobody holding it — and everything else
//! falls through to cutting a session the ordinary way. Two of those shapes are
//! what most of the stranded branches were actually in — a session its owner
//! closed, and a branch somebody landed by hand — and cutting a session the
//! ordinary way used to refuse both. `onevcs` 0.8.0 continues an existing branch
//! from its own tip instead, so the fresh session lands on the work rather than
//! beside it; that too is a fact about `Cargo.lock`, and it is asserted here as
//! the sibling decides it because widening the rule was the sibling's to do and
//! not this crate's.
//!
//! Offline and hermetic, like every journey beside it: a bare repository on disk
//! is the origin and the identity publishes `local-direct`, so no host is asked
//! for anything.
//!
// llmlint: ignore-file[e2e_not_mocked] `onevcs` is not substituted: it is the linked
// library under the compiled binary, and its own released executable builds the state
// each journey starts from. `oneagentgraph` is the double, because what is under test is
// which session a dispatch runs in rather than what the dispatch writes.

use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{json, Value};

use crate::harness::{git, lifecycle, onevcs_binary, plan_of, World, GIT_EMAIL, GIT_WHO};
use onevcs::Session;

// Reached only from the stopped-run journey below, which is `#[cfg(unix)]` for
// the reason stated there.
#[cfg(unix)]
use crate::harness::abandonable_hook_script;
#[cfg(unix)]
use std::path::PathBuf;

#[cfg(unix)]
const STRANDED: &str = "feature/stranded";

/// Why a run settled the way it did, as the sibling itself said it.
///
/// `result.json` records the status and the outcome and not a word of the reason,
/// and the reason here *is* the sibling's refusal — so this is what a failing
/// assertion below prints instead of a bare mismatch.
fn why(world: &World, run: &str) -> String {
    let settled: Vec<String> = world
        .events_of(run, "node-settled")
        .iter()
        .map(|event| {
            format!(
                "{} {} {}: {}",
                event["labels"]["node"],
                event["payload"]["status"],
                event["payload"]["outcome"],
                event["payload"]["detail"]
            )
        })
        .collect();
    format!("what the nodes settled on:\n  {}", settled.join("\n  "))
}

/// The **sibling's own** record of the session a run opened, or `Null` if it never
/// opened one.
///
/// Told apart from this crate's `session-opened` — which it writes beside the
/// sibling's for the merged stream — by the fields only the repository side knows:
/// the clone it cut, and whether the session was taken up rather than cut.
///
/// The *last* of them, because a session that is taken up keeps the stream the run
/// before it was writing: this run relays that stream from its start, so the
/// opening the stopped run made is in this run's journal too, ahead of the one
/// this run made.
fn opened(world: &World, run: &str) -> Value {
    world
        .journal(run)
        .into_iter()
        .rfind(|event| {
            event["source"] == "vcs"
                && event["kind"] == "session-opened"
                && event["payload"]["clone"].is_string()
        })
        .unwrap_or(Value::Null)
}

/// The released `onevcs` executable, run against this world's state root.
///
/// The same build the binary under test links — `harness::onevcs_binary` builds
/// the version `Cargo.lock` pins — so the state these journeys start from is the
/// state the sibling itself writes, rather than a shape this suite believes it
/// writes.
fn onevcs(world: &World, args: &[&str]) -> String {
    let (code, out, err) = ran(world, args);
    assert_eq!(code, Some(0), "onevcs {args:?} failed: {err}{out}");
    out
}

/// The same command, for a verb this journey needs to be *refused*.
///
/// Answers what it said, which is where the refusal is: a command that succeeded
/// is the failure, and it fails here rather than several assertions later.
fn onevcs_refusal(world: &World, args: &[&str]) -> String {
    let (code, out, err) = ran(world, args);
    assert_ne!(code, Some(0), "onevcs {args:?} was not refused: {out}{err}");
    err
}

fn ran(world: &World, args: &[&str]) -> (Option<i32>, String, String) {
    let output = Command::new(onevcs_binary())
        .args(args)
        .env("ONEVCS_HOME", world.onevcs_home())
        .env("GIT_CONFIG_GLOBAL", world.gitconfig())
        .env("GIT_AUTHOR_NAME", GIT_WHO)
        .env("GIT_AUTHOR_EMAIL", GIT_EMAIL)
        .env("GIT_COMMITTER_NAME", GIT_WHO)
        .env("GIT_COMMITTER_EMAIL", GIT_EMAIL)
        .stdin(Stdio::null())
        .output()
        .expect("the onevcs binary runs");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// A session opened by a process that has since exited.
///
/// Which is what makes its holder *stale* rather than live: a launch meeting a
/// live one is refused by the identity interlock — a different rule, tested in
/// `concurrency.rs` — and every journey here is about what happens after that.
fn abandoned_session(world: &World, branch: &str) -> Session {
    let printed = onevcs(world, &["session", "open", "service", "--branch", branch]);
    serde_json::from_str(printed.trim())
        .unwrap_or_else(|e| panic!("`onevcs session open` did not print a session: {e}: {printed}"))
}

/// The run root a session was cut under.
///
/// Derived rather than reported: a [`Session`] names the worktree, and the run
/// root is what `onevcs` cuts that worktree inside.
fn run_root(session: &Session) -> &Path {
    session
        .worktree
        .parent()
        .expect("a session worktree sits inside its run root")
}

/// Hold a run root's occupancy lease exclusively, as a reclamation probing that
/// directory holds it.
///
/// This is the one state a journey cannot ask the sibling's own command line for:
/// a lease is deliberately one the OS releases with the process, so no verb holds
/// one past its own exit and no sequence of them leaves a run root occupied. The
/// file is therefore locked here, and its *name* is computed the way `onevcs`
/// computes it — `run:<run root>`, SHA-256'd, under the state root's `locks/` —
/// because the module that names it is private to the sibling.
///
/// A restated name is only worth having while something proves it is still the
/// right one, so this does not trust it twice over. The file has to already exist,
/// which catches a rename; and the sibling is then asked, through its own
/// `session adopt`, to take the session up — the same lease `resumable` consults,
/// asked by the public verb built on it. A refusal is the proof that what this
/// holds is what decides occupancy. Should `onevcs` move that decision to another
/// lock, the adoption succeeds and this says so, rather than leaving the journeys
/// below holding a file nothing consults and asserting nothing.
// llmlint: ignore-block[tests_mirror_real_usage] this function alone, for the reason
// above: no command leaves a run root occupied, so there is no interface to reach it
// through. Every journey below drives the compiled binary.
fn hold_lease(world: &World, session: &Session) -> std::fs::File {
    use sha2::{Digest, Sha256};
    let lease = format!("run:{}", run_root(session).display());
    let digest: String = Sha256::digest(lease.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let path = world
        .onevcs_home()
        .join("locks")
        .join(format!("{digest}.lock"));
    assert!(
        path.is_file(),
        "onevcs keeps no lease for {lease} at {}; the lock it names a run root's \
         occupancy after has moved, and this journey is no longer holding one",
        path.display()
    );
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap_or_else(|e| panic!("cannot open the lease at {}: {e}", path.display()));
    assert!(
        matches!(fs4::fs_std::FileExt::try_lock_exclusive(&file), Ok(true)),
        "the lease at {} is already held, so this journey never made the run root occupied",
        path.display()
    );
    let refused = onevcs_refusal(world, &["session", "adopt", &session.token.0]);
    assert!(
        refused.contains("is occupied by another process"),
        "the sibling took up a session whose lease this journey holds, so {} is not \
         the lock occupancy is decided by: {refused}",
        path.display()
    );
    file
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// Run one lifecycle node to settlement, pinned to `branch` or naming none.
///
/// Answers what the run settled and the sibling's own record of the session it
/// worked in, which is what says whether a session was taken up or cut.
fn dispatched(world: &World, case: &str, branch: Option<&str>) -> (Value, Value) {
    world.script(&format!("{case}.work"), &format!("{case} wrote this\n"));
    let mut node = lifecycle(case, &[]);
    if let Some(branch) = branch {
        node["branch"] = json!(branch);
    }
    let path = world.plan(case, &plan_of(case, vec![node]));
    world
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .settled();
    world.until("the run to settle", |world| {
        world.run_file(case, "result.json").is_file()
    });
    (world.run_json(case, "result.json"), opened(world, case))
}

/// How long the publication under a stopped run is given to leave.
///
/// Neither platform's stop can be refused — `SIGKILL` on one, `taskkill /F` on
/// the other — so this is not a wait for the work to wind down; it is only the
/// slack a loaded machine needs between the stop and the last process leaving.
/// Generous on purpose — past it the journey fails rather than releasing the
/// rendezvous into the race it exists to close.
///
/// Unix-only with the rest of the stop, for the reason the journey below states.
#[cfg(unix)]
const PUBLICATION_STOPS_WITHIN: std::time::Duration = std::time::Duration::from_secs(30);

/// Start a run's owner in a **process group of its own**, so the sweep below has
/// a handle on everything it starts that does not also name this test runner.
///
/// [`process_group`]: std::os::unix::process::CommandExt::process_group
#[cfg(unix)]
fn in_a_group_of_its_own(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

/// What a process lister wrote, once it has said it succeeded.
///
/// The exit status is checked and not merely the bytes, because of what the two
/// readers below do with an empty answer: [`holders_of`] reporting nothing is how
/// the sweep in [`stop_the_publication`] concludes that the publication has
/// stopped, and [`process_table`] returning nothing is a tree with no branches to
/// signal. A lister that failed writes nothing to standard output either, so
/// taking its bytes unasked turns "this host could not answer" into "there is
/// nothing there" — and the journey then passes at exactly the point it exists to
/// prove something.
///
/// Rows that do not parse are still dropped by the callers, and deliberately: a
/// header line or two columns run together must not cost a read the rows it got
/// right. That is a statement about the *shape* of an answer this host gave;
/// this is about whether it gave one at all.
///
/// Unix-only with both of its callers, for the reason the journey below states.
#[cfg(unix)]
fn listed_by(mut lister: Command, what: &str) -> String {
    let listed = lister.output().expect("the host lists its process table");
    assert!(
        listed.status.success(),
        "{what} answered {:?} rather than listing this host's processes, so nothing below can \
         tell a publication that has stopped from one this journey cannot see: {}{}",
        listed.status.code(),
        String::from_utf8_lossy(&listed.stdout),
        String::from_utf8_lossy(&listed.stderr)
    );
    String::from_utf8_lossy(&listed.stdout).into_owned()
}

/// The host's own process table, as `(pid, parent, group)`.
///
/// Read from `ps` rather than assumed, and every row that does not parse is
/// dropped rather than failing the read: a platform that prefixes a header or
/// runs two columns together must not cost this journey the rows it got right.
///
/// `-ww` for the reason [`holders_of`] gives, and here too so both reads of the
/// table are the same read.
#[cfg(unix)]
fn process_table() -> Vec<(u32, u32, i32)> {
    let mut lister = Command::new("ps");
    lister.args(["-ww", "-eo", "pid=,ppid=,pgid="]);
    listed_by(lister, "ps -eo pid=,ppid=,pgid=")
        .lines()
        .filter_map(|row| {
            let mut column = row.split_whitespace();
            Some((
                column.next()?.parse().ok()?,
                column.next()?.parse().ok()?,
                column.next()?.parse().ok()?,
            ))
        })
        .collect()
}

/// Every process group the tree under `root` is spread across, `root`'s own
/// included.
///
/// Groups rather than pids, because of what is being reached for: `onevcs`
/// starts the publishing `git` with `process_group(0)` — deliberately, so one
/// handle covers a transport it restarts whenever the connection drops — and the
/// `pre-push` hook git runs is inside *that* group. A signal to the owner's
/// group alone reaches neither, which is the whole of the defect this fixes.
///
/// Read **before** anything is signalled: an orphan is reparented the moment its
/// parent goes, and the descent that finds it is broken along with it.
#[cfg(unix)]
fn groups_under(root: u32) -> Vec<i32> {
    let table = process_table();
    let mut tree = vec![root];
    // The table is unordered, so a child can be listed before its parent; sweep
    // until a pass adds nothing rather than trusting one walk of it.
    loop {
        let grown: Vec<u32> = table
            .iter()
            .filter(|(pid, parent, _)| tree.contains(parent) && !tree.contains(pid))
            .map(|(pid, _, _)| *pid)
            .collect();
        if grown.is_empty() {
            break;
        }
        tree.extend(grown);
    }
    let mut groups: Vec<i32> = table
        .iter()
        .filter(|(pid, _, _)| tree.contains(pid))
        .map(|(_, _, group)| *group)
        .collect();
    groups.sort_unstable();
    groups.dedup();
    groups
}

/// Whether a process group still holds anything this process may signal.
///
/// Signal `0` sends nothing and asks only that question, so `ESRCH` back from it
/// is the kernel saying the group is empty.
#[cfg(unix)]
fn group_holds_anything(group: i32) -> bool {
    // SAFETY: `killpg` takes a group id and a signal and reports failure in its
    // return value; nothing is borrowed, and signal 0 sends nothing at all.
    unsafe { libc::killpg(group, 0) == 0 }
}

/// Stop a run **and the whole publication running under it**, and do not return
/// while any of it is still alive.
///
/// A publication is a tree, not a process — the owner starts a worker, the worker
/// calls `onevcs`, `onevcs` runs `git push`, and git runs the `pre-push` hook
/// this journey is holding — so [`std::process::Child::kill`], which signals the
/// one pid it was given, leaves the push alive to land once the rendezvous is
/// released. Three things make this stop mean what the journey says instead.
///
/// The **moment**: it waits until the hook is actually holding a push.
/// `merge-queued` is published before the push it precedes, so stopping on that
/// alone can land anywhere in the publication, with the hook yet to come.
///
/// The **reach**: every group the tree is spread across, not the owner's alone,
/// because `onevcs` detaches git into a group of its own. See [`groups_under`].
///
/// The **check**: signal delivery is asynchronous, so returning before the last
/// process has left would put the release back in the race this closes.
///
/// Unix-only, and there is no arm beside it — the journey below says why, and
/// carries the same gate.
#[cfg(unix)]
fn stop_the_publication(world: &World, owner: &mut std::process::Child, rendezvous: &Path) {
    let held = rendezvous.to_string_lossy().into_owned();
    world.until("the pre-push hook to take hold of the publication", |_| {
        !holders_of(&held).is_empty()
    });
    let groups = groups_under(owner.id());
    for group in &groups {
        // SAFETY: as in `group_holds_anything`. Every group here belongs to a
        // process descended from this journey's own owner, which was put in a
        // group of its own precisely so that this set cannot reach the runner.
        unsafe { libc::killpg(*group, libc::SIGKILL) };
    }
    owner.wait().expect("the stopped run's owner exits");

    let deadline = std::time::Instant::now() + PUBLICATION_STOPS_WITHIN;
    while std::time::Instant::now() < deadline {
        if !groups.iter().copied().any(group_holds_anything) && holders_of(&held).is_empty() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!(
        "the stopped run's publication is still running {PUBLICATION_STOPS_WITHIN:?} after \
         every process group under it was signalled, so releasing the rendezvous below would \
         let the push it was stopped in the middle of finish after all; the groups were \
         {groups:?} and {} process(es) still hold {held}",
        holders_of(&held).len()
    );
}

/// The pids whose command line names `rendezvous` — this world's blocked
/// `pre-push` hook, and nothing else, because the path carries the world's own
/// scratch directory in it.
///
/// The path is the whole of the match, and the hook's *name* is deliberately not
/// part of it: `harness::install_hook` writes the hook as `exec` over the verb,
/// so what git leaves running is the verb's own argv — `sh .../hook.sh wait-for
/// .../push.go` — and the word `pre-push` appears nowhere in it. Matching on the
/// name as well found nothing, waited out the bound, and failed the journey on a
/// hook that was holding the push exactly as intended.
///
/// `-ww` because the path is at the *end* of that argv and the default width is
/// not the same promise on both platforms: Linux's `procps` writes an unbounded
/// line once stdout is not a terminal, while the BSD `ps` on macOS still cuts
/// the row at 79 columns — which is inside a world's scratch path, so the match
/// below would find nothing, the wait above it would run out its bound, and the
/// journey would fail on macOS for the second time in a row and for a new
/// reason. Asking for every column removes the difference.
#[cfg(unix)]
fn holders_of(rendezvous: &str) -> Vec<u32> {
    let mut lister = Command::new("ps");
    lister.args(["-ww", "-eo", "pid=,args="]);
    listed_by(lister, "ps -eo pid=,args=")
        .lines()
        .filter(|row| row.contains(rendezvous))
        .filter_map(|row| row.split_whitespace().next()?.parse().ok())
        .collect()
}

/// A run stopped mid-publication, and the retry that takes up the session it left
/// its work in.
///
/// **Unix-only:** this journey *stops* a publication while the hook holds it, the
/// shape [`harness::abandonable_hook_script`](crate::harness::abandonable_hook_script)
/// names as unreapable off Unix — and this journey abandons it deliberately, so a
/// release on drop would defeat what it asserts rather than protect it. Windows gives up a run stopped mid-publication, the work
/// it strands, and the retry that continues the session; `main` ran it there over
/// `onevcs`'s *gate*, a direct child of the stopped process rather than git's
/// grandchild.
#[cfg(unix)]
#[test]
fn a_retry_takes_up_the_session_a_stopped_run_left_its_work_in() {
    let world = World::new("session-reuse-adopt");
    // A `pre-push` hook the journey holds, which is how a run is stopped
    // *mid-publication*: `onevcs` commits the worktree onto the branch before it
    // pushes, so by the time the repository's merge path is running the work is on
    // the branch and the session is the only thing that still knows where.
    let go = world.fakes.join("push.go");
    let held = abandonable_hook_script(&world, &go);
    let repo = world.repository(
        "local-direct",
        &held.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    world.script("service.work", "the worker wrote this\n");

    // A run that stops in the middle of its publication, with its work committed
    // on the branch the plan pinned.
    let mut stopped = lifecycle("service", &[]);
    stopped["branch"] = json!(STRANDED);
    let path = world.plan("stopped", &plan_of("stopped", vec![stopped]));
    let mut launch = world.cmd(&["start", &path.to_string_lossy(), "--attach"]);
    launch.stdout(Stdio::piped()).stderr(Stdio::piped());
    in_a_group_of_its_own(&mut launch);
    let mut owner = launch.spawn().expect("the stopped run's owner starts");
    world.until("the publication to reach its merge path", |world| {
        world
            .journal("stopped")
            .iter()
            .any(|event| event["source"] == "vcs" && event["kind"] == "merge-queued")
    });
    let stranded = opened(&world, "stopped");
    let stranded = stranded["payload"]["token"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "the stopped run opened no session\n{}",
                why(&world, "stopped")
            )
        })
        .to_owned();
    let clone = PathBuf::from(
        opened(&world, "stopped")["payload"]["clone"]
            .as_str()
            .expect("the sibling named the clone it cut"),
    );
    stop_the_publication(&world, &mut owner, &go);

    // What it left, which is the whole reason a retry used to be refused: a branch
    // carrying a commit its base does not.
    let ahead = git(
        &world,
        &clone,
        &["log", "--format=%s", &format!("origin/main..{STRANDED}")],
    );
    assert_eq!(
        ahead.lines().collect::<Vec<_>>(),
        vec![format!("chore: preserve work on {STRANDED}")],
        "the stopped run left nothing on {STRANDED} for its retry to inherit"
    );

    world.release("push.go");
    world.script("continuation.work", "and then the worker wrote this\n");
    let mut retry = lifecycle("continuation", &[]);
    retry["branch"] = json!(STRANDED);
    let path = world.plan("retry", &plan_of("retry", vec![retry]));
    world
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .settled();
    world.until("the retry to settle", |world| {
        world.run_file("retry", "result.json").is_file()
    });

    let result = world.run_json("retry", "result.json");
    assert_eq!(
        result["nodes"][0]["status"],
        "done",
        "{result}\n{}",
        why(&world, "retry")
    );
    assert_eq!(
        result["nodes"][0]["outcome"],
        "merged",
        "{result}\n{}",
        why(&world, "retry")
    );

    // It worked in the stopped run's own session rather than cutting a second one
    // on the same name — the sibling says both, by the token and by saying so.
    let taken = opened(&world, "retry");
    assert_eq!(
        taken["payload"]["token"].as_str(),
        Some(stranded.as_str()),
        "the retry cut a second session on {STRANDED} instead of taking up the one \
         holding the work: {taken}"
    );
    assert_eq!(
        taken["payload"]["reused"],
        json!(true),
        "the sibling did not record the session as one it took up: {taken}"
    );

    // And what that buys, which is the only thing an operator cares about: the
    // work the stopped run stranded reached the base, beside the retry's own.
    assert_eq!(
        repo.base_file("service.md").as_deref().map(str::trim),
        Some("the worker wrote this"),
        "the stranded work never reached the base"
    );
    assert_eq!(
        repo.base_file("continuation.md").as_deref().map(str::trim),
        Some("and then the worker wrote this"),
        "the retry's own work never reached the base"
    );
    assert_eq!(
        repo.base_commits(&world),
        vec![
            "feat: ship continuation".to_string(),
            "chore: seed the repository".to_string(),
        ],
        "the base did not advance by exactly the retry's publication"
    );
}

/// Everything reuse does *not* cover, in the order the sibling decides it.
///
/// All six fall through to cutting a session the ordinary way, which is not a
/// refusal and never was. What separates them is what the fresh session is cut
/// *onto*: four name a branch nothing carries yet and get one cut from the base,
/// and the last two name one that already exists — a session its owner closed,
/// and a branch somebody landed by hand — and are continued from its own tip.
/// Those two are here because they are the shapes most of the stranded branches
/// were in, and until `onevcs` 0.8.0 both were refused outright with the work
/// sitting where no plan could reach it. Asserted as they are rather than
/// changed: what a pin whose branch already carries work may do is the sibling's
/// rule.
#[test]
fn every_other_shape_cuts_a_fresh_session_onto_its_branch_or_from_the_base() {
    let world = World::new("session-reuse-fallbacks");
    let repo = world.repository("local-direct", &[]);

    // A session nothing ever came back for, opened before the first run below so
    // that run's own opening reclaims it — a real reclamation by the sibling
    // rather than a directory this journey removed to look like one.
    let reclaimed = abandoned_session(&world, "feature/reclaimed");

    // 1. Occupied. Somebody has taken the run root against the world, so the
    //    session cannot be taken up and a fresh one is cut instead.
    let occupied = abandoned_session(&world, "feature/occupied");
    let lease = hold_lease(&world, &occupied);
    let (settled, session) = dispatched(&world, "occupied", Some("feature/occupied"));
    assert_eq!(
        settled["nodes"][0]["status"],
        "done",
        "{settled}\n{}",
        why(&world, "occupied")
    );
    assert_ne!(
        session["payload"]["token"].as_str(),
        Some(occupied.token.0.as_str()),
        "a run root somebody else is holding was taken up anyway: {session}"
    );
    drop(lease);

    // 2. Reclaimed. The record still names a run root; the directory is gone, and
    //    with it every place the branch could have been continued.
    assert!(
        !run_root(&reclaimed).is_dir(),
        "the sibling did not reclaim the abandoned run root at {}, so this case is \
         not the one it names",
        run_root(&reclaimed).display()
    );
    let (settled, session) = dispatched(&world, "reclaimed", Some("feature/reclaimed"));
    assert_eq!(
        settled["nodes"][0]["status"],
        "done",
        "{settled}\n{}",
        why(&world, "reclaimed")
    );
    assert_ne!(
        session["payload"]["token"].as_str(),
        Some(reclaimed.token.0.as_str()),
        "a session whose run root is gone was taken up: {session}"
    );

    // 3. Ambiguous. Two open sessions answer to one branch — which is the state a
    //    host that cut them before this rule existed is in — and nothing here can
    //    choose between them, so neither is taken up. Built the way it arises: the
    //    second is cut while the first is occupied.
    let first = abandoned_session(&world, "feature/ambiguous");
    let lease = hold_lease(&world, &first);
    let second = abandoned_session(&world, "feature/ambiguous");
    drop(lease);
    assert_ne!(
        first.token, second.token,
        "the second opening took up the first session, so there is no ambiguity to test"
    );
    let (settled, session) = dispatched(&world, "ambiguous", Some("feature/ambiguous"));
    assert_eq!(
        settled["nodes"][0]["status"],
        "done",
        "{settled}\n{}",
        why(&world, "ambiguous")
    );
    let taken = session["payload"]["token"].as_str();
    assert!(
        taken != Some(first.token.0.as_str()) && taken != Some(second.token.0.as_str()),
        "one of two sessions answering to the same branch was taken up: {session}"
    );

    // 4. Unpinned. A node naming no branch is asking for a name of its own, and a
    //    session it might have inherited is not one it asked for.
    let unpinned = abandoned_session(&world, "feature/unpinned");
    let (settled, session) = dispatched(&world, "unpinned", None);
    assert_eq!(
        settled["nodes"][0]["status"],
        "done",
        "{settled}\n{}",
        why(&world, "unpinned")
    );
    assert_ne!(
        session["payload"]["token"].as_str(),
        Some(unpinned.token.0.as_str()),
        "a node that pinned nothing inherited somebody else's branch: {session}"
    );
    assert!(
        session["payload"]["branch"]
            .as_str()
            .is_some_and(|branch| branch.starts_with("onevcs/")),
        "a node that pinned nothing did not get a name of its own: {session}"
    );

    // 5. A predecessor that was **closed**. Closing is the statement that a
    //    session is finished: the branch goes back to the execution checkout and
    //    the record is spent, so the retry meets the branch and not the session.
    //    A fresh session is cut — and `onevcs` 0.8.0 cuts it *onto that branch*,
    //    continuing the work rather than refusing a pin it cannot honour.
    let finished = abandoned_session(&world, "feature/closed");
    std::fs::write(
        finished.worktree.join("left-behind.md"),
        "the work somebody left\n",
    )
    .expect("the finished work is written");
    git(&world, &finished.worktree, &["add", "-A"]);
    git(
        &world,
        &finished.worktree,
        &["commit", "-m", "feat: what the closed session did"],
    );
    onevcs(&world, &["session", "close", &finished.token.0]);
    let (settled, session) = dispatched(&world, "closed", Some("feature/closed"));
    assert_eq!(
        settled["nodes"][0]["status"],
        "done",
        "{settled}\n{}",
        why(&world, "closed")
    );
    assert_ne!(
        session["payload"]["token"].as_str(),
        Some(finished.token.0.as_str()),
        "a session its owner closed was taken up rather than replaced: {session}"
    );
    // Continued rather than cut afresh, which is the whole difference: both the
    // closed session's commit and this run's own reached the base.
    assert_eq!(
        repo.base_file("left-behind.md").as_deref().map(str::trim),
        Some("the work somebody left"),
        "the closed session's work never reached the base, so the pin was not \
         continued:\n{}",
        why(&world, "closed")
    );
    assert_eq!(
        repo.base_file("closed.md").as_deref().map(str::trim),
        Some("closed wrote this"),
        "the run's own work never reached the base:\n{}",
        why(&world, "closed")
    );

    // 6. No session at all. A branch somebody finished by hand is work no record
    //    points at, which is the other half of what a reclaimed run root leaves —
    //    and it is continued for the same reason: the branch exists, so it is
    //    where the work goes.
    let handmade = world.root.join("by-hand");
    git(
        &world,
        &repo.checkout,
        &[
            "worktree",
            "add",
            "-b",
            "feature/handmade",
            &handmade.to_string_lossy(),
            "main",
        ],
    );
    std::fs::write(
        handmade.join("by-hand.md"),
        "somebody finished this by hand\n",
    )
    .expect("the hand-finished work is written");
    git(&world, &handmade, &["add", "-A"]);
    git(
        &world,
        &handmade,
        &["commit", "-m", "feat: finished by hand"],
    );
    git(
        &world,
        &repo.checkout,
        &["worktree", "remove", &handmade.to_string_lossy()],
    );
    let (settled, _) = dispatched(&world, "handmade", Some("feature/handmade"));
    assert_eq!(
        settled["nodes"][0]["status"],
        "done",
        "{settled}\n{}",
        why(&world, "handmade")
    );
    assert_eq!(
        repo.base_file("by-hand.md").as_deref().map(str::trim),
        Some("somebody finished this by hand"),
        "the hand-finished work never reached the base, so the pin was not \
         continued:\n{}",
        why(&world, "handmade")
    );
    assert_eq!(
        repo.base_file("handmade.md").as_deref().map(str::trim),
        Some("handmade wrote this"),
        "the run's own work never reached the base:\n{}",
        why(&world, "handmade")
    );
}
