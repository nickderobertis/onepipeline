//! What a lifecycle node's **destination repository** says about it, asked
//! before anything is dispatched.
//!
//! Every other rule the loader applies is identity-blind: it reads the plan
//! document and nothing else. Two refusals cannot be made that way, because what
//! decides them belongs to the repository the node publishes into — and both were
//! being discovered at the *end* of a node, with the whole dispatch and its judge
//! already paid for:
//!
//! * a `title` the destination repository's own `commit-msg` hook turns down, so
//!   the publication is refused after the work is finished and passed; and
//! * a non-empty [`consumes`](crate::plan::Node::consumes) on an identity that
//!   publishes without opening a change request, where the draft that holds such
//!   a node to its release has no change request to be a state of — which
//!   `onevcs` refuses outright at the last step of the node.
//!
//! **The rules are the repository's, read where it states them.** The subject
//! policy is that repository's own hook file, run the way git runs it, rather
//! than a copy of which types it releases from kept here — a copy is what goes
//! stale the first time a repository changes its mind, and it is the same reason
//! the 120-character limit beside it is taken from `onevcs::provenance` rather
//! than retyped. The publication policy is `onevcs`'s, read off its own
//! resolution verbs.
//!
//! **Those verbs are spawned**, and everywhere else in this crate `onevcs` is
//! called. At the pinned release the resolution they perform is on the command
//! line and not on the library surface — `store::resolve`, `policy::resolve` and
//! `git::message_policy` are private modules — so neither the publication
//! checkout nor the resolved policy can be had any other way. Spawning a
//! sibling's own published verb is not standing in for it; nothing is
//! substituted, and `tests/e2e/harness.rs` already builds and leads the `PATH`
//! with the real `onevcs` binary out of this same pin. The finding asking
//! `onevcs resolve` to carry the policy in the JSON it already prints is with
//! that sibling; when it lands, [`publication`]'s prose read retires and only
//! the one verb is spawned.
//!
//! **It stays inside this crate's own project rather than becoming one.** What
//! is spawned is not a command this tool exposes and not a client of a live
//! service: it is two offline reads of a sibling this crate is already built
//! around, made *by the loader*, so a change to the loader is exactly the change
//! that has to re-run these journeys. Edged into a project of its own it would
//! have to declare a dependency back on the one it was split from to stay in
//! `nx affected` at all, and this crate is one Rust build unit besides.
//!
//! **A question this host could not answer is never an accept.** A repository
//! that does not resolve, a verb that is not installed, an answer this build
//! cannot read: each of those is a node that was *not checked*, and the loader
//! says so on stderr and loads the plan. Refusing there would refuse every plan
//! whose repository this host has not registered, which is a plan that launches
//! correctly today; passing silently would let a plan load looking like it had
//! cleared a bar nobody applied.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use onevcs::MergePolicy;

use crate::plan::{Node, Plan};
use crate::refusal::Refusal;

/// The environment variable naming the `onevcs` executable.
///
/// In this crate's namespace rather than `onevcs`'s own, for the reason
/// [`crate::taskgraph::BINARY_ENV`] records about the other sibling: a product
/// that reads its configuration from its own prefix would take `ONEVCS_BIN` for a
/// setting called `bin`.
pub const BINARY_ENV: &str = "ONEPIPELINE_ONEVCS_BIN";

pub const DEFAULT_BINARY: &str = "onevcs";

/// The hook git puts a commit message to, and the one `onevcs` puts a publication
/// subject to.
const COMMIT_MSG_HOOK: &str = "commit-msg";

const PUBLICATION_LINE: &str = "publication:";

/// Hold every lifecycle node of `plan` to what its destination repository says.
///
/// The first refusal wins, as it does everywhere else the loader refuses: a plan
/// is corrected one thing at a time, and a list of them would be this crate
/// deciding which of a repository's rules the author meant to break.
///
/// Each repository is resolved once however many nodes name it — a plan's nodes
/// are commonly all in one — so a launch pays two subprocesses per repository
/// rather than two per node.
pub(crate) fn check(plan: &Plan) -> std::result::Result<(), Refusal> {
    let mut resolved: BTreeMap<String, Result<Destination, String>> = BTreeMap::new();
    for node in &plan.tasks {
        let Some(repo) = node.repo.as_deref() else {
            continue;
        };
        // Nothing to ask about is not a node that went unchecked: a node stating
        // no title and consuming nothing has neither of the two properties these
        // rules are about, so a repository that does not resolve leaves it exactly
        // as checked as it ever was.
        if node.title.is_none() && node.consumes.is_empty() {
            continue;
        }
        let destination = resolved
            .entry(repo.to_owned())
            .or_insert_with(|| resolve(repo));
        let destination = match destination {
            Ok(destination) => destination,
            Err(why) => {
                report_unchecked(&node.id, repo, why);
                continue;
            }
        };
        // Each rule in turn, and no further once one has refused: the title rule
        // runs the repository's own hook as a subprocess, and a node already being
        // refused for its `consumes` has no use for a verdict on a title it will
        // not publish under. A rule that could not be *asked* is not an answer, so
        // that one is reported and the next is still put.
        type Rule = fn(&Node, &Destination) -> std::result::Result<Option<Refusal>, String>;
        for ask in [consumes_refusal as Rule, title_refusal as Rule] {
            match ask(node, destination) {
                Ok(Some(refusal)) => return Err(refusal),
                Ok(None) => {}
                Err(why) => report_unchecked(&node.id, repo, &why),
            }
        }
    }
    Ok(())
}

/// Say that one node was **not** held to its destination repository's rules.
///
/// On stderr, where every other diagnosis this binary makes goes, and in a
/// sentence that says the check did not run rather than that it passed: the two
/// readings are what this whole module exists to keep apart, and a plan that
/// loaded in silence would carry the second one for free.
fn report_unchecked(node: &str, repo: &str, why: &str) {
    eprintln!(
        "onepipeline: node '{node}': this build could not ask {repo} what it says about this \
         node, so the plan loaded without that check having run — {why}"
    );
}

/// The three fields of `onevcs resolve`'s answer this loader reads, as the
/// **shape** they arrive in rather than as keys probed off an untyped document.
///
/// A subprocess's stdout is external input, so it is deserialized at the boundary
/// and every field is typed: an absent key, a key of the wrong type, and a
/// `workflow` outside the sibling's own enum are each refused by serde, in serde's
/// own words, before anything downstream can read them. The verb prints more keys
/// than these — an alias, a repo type, a gate — and they are ignored rather than
/// refused, because a *newer* `onevcs` printing a fourth is one this build must go
/// on reading.
#[derive(serde::Deserialize)]
struct Resolved {
    // llmlint: ignore-block[invalid_states_unrepresentable] a repository identity is a
    // `String` on every surface `onevcs` publishes — `registry::Identity::origin`,
    // `SessionHolder::identity`, `RepositoryReleases::identity` — and this crate already
    // carries one as a `String` beside them, in `release::CrossRepoReference::repository`.
    // A newtype here would be *this* crate minting a second vocabulary for a value the
    // sibling owns and hands back, which is the identity chain `src/AGENTS.md` says stays
    // in that crate. What is checkable here is that the sibling stated one at all, and
    // `resolve` refuses a blank.
    identity: String, // llmlint: ignore-end[invalid_states_unrepresentable]
    workflow: onevcs::registry::Workflow,
    /// Where this repository's own hooks are, which is the whole of what it is
    /// read for here.
    publication_checkout: PathBuf,
}

/// What one repository answered about itself.
struct Destination {
    resolved: Resolved,
    /// The policy its rules file resolves it to, or why this build has no answer.
    ///
    /// Held apart from the resolution around it rather than sinking the whole of
    /// it, because the two refusals need different halves: the subject policy
    /// needs only the checkout, and a rules file this build could not read is no
    /// reason to stop asking a repository's own hook about a title.
    publication: std::result::Result<MergePolicy, String>,
}

/// The executable this process asks.
///
/// Read as an `OsString`, because a path is one: a value this platform allows and
/// Unicode does not would be *lost* by a `String` read, and the loader would then
/// ask an executable the operator never named while believing they named none.
fn binary() -> std::ffi::OsString {
    std::env::var_os(BINARY_ENV)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_BINARY.into())
}

/// Run one resolution verb and answer with its stdout, or why there is none.
///
/// The stdout of a subprocess is bytes, and this build reads it as UTF-8 or not at
/// all: a lossy decode would put replacement characters into the JSON a shape is
/// read from and into the line a policy is read off, and either could parse as
/// something nobody said. The **stderr** it quotes back is lossy on purpose — that
/// one is a sentence for a person, and refusing to relay a diagnostic because a
/// byte in it was not UTF-8 loses the only thing that failure had to say.
fn ask(verb: &[&str], repo: &str) -> Result<String, String> {
    let binary = binary();
    let named = || format!("`{} {} {repo}`", binary.to_string_lossy(), verb.join(" "));
    let output = Command::new(&binary)
        .args(verb)
        .arg(repo)
        .output()
        .map_err(|error| {
            format!(
                "{} could not be run: {error} (set {BINARY_ENV} to an executable one)",
                named()
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "{} refused: {}",
            named(),
            one_line(&String::from_utf8_lossy(&output.stderr))
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("{} answered bytes that are not UTF-8: {error}", named()))
}

/// One repository, as `onevcs`'s own resolution verbs answer for it.
fn resolve(repo: &str) -> Result<Destination, String> {
    let resolved: Resolved =
        serde_json::from_str(ask(&["resolve"], repo)?.trim()).map_err(|error| {
            format!("`onevcs resolve {repo}` did not answer the shape this build reads: {error}")
        })?;
    // The two things the shape cannot say. An identity is what every refusal below
    // names the repository by, and a blank one names nothing.
    if resolved.identity.trim().is_empty() {
        return Err(format!("`onevcs resolve {repo}` states a blank identity"));
    }
    // And a checkout is a place on this host, which a relative path does not name:
    // it would resolve against whatever directory this process happens to have been
    // started in, so the hook that answered would be some other repository's.
    //
    // llmlint: ignore-block[boundary_inputs_validated] that the checkout is *the* one for
    // this identity is not checkable here without re-deriving the registry lookup that
    // answered it, which is the identity chain `src/AGENTS.md` keeps in `onevcs`. It is
    // that crate's own answer for the repository this loader asked it about, obtained by
    // running the verb the rest of this stack already trusts, and the hook reached
    // through it is the same file `onevcs` runs from the same path at the publication —
    // so a second opinion here could only disagree with the thing it is predicting.
    // Absolute, and a directory git answers for, are what can be checked, and both are.
    if !resolved.publication_checkout.is_absolute() {
        return Err(format!(
            "`onevcs resolve {repo}` states a publication checkout that is not an absolute \
             path, {}",
            resolved.publication_checkout.display()
        ));
    }
    Ok(Destination {
        resolved,
        publication: ask(&["rules", "check"], repo)
            .and_then(|reported| publication(repo, &reported)),
    })
} // llmlint: ignore-end[boundary_inputs_validated]

/// The publication policy `onevcs rules check` reported, off the line it states
/// it on.
///
/// A prose read, deliberately and not happily: the policy is what decides whether
/// a change request is opened at all, and no `onevcs` surface answers it as data
/// at the pinned release. It is read defensively — the line must be there and the
/// word on it must be one the sibling's own type accepts — so a reworded report
/// becomes a node this build could not check rather than a node it waved through.
fn publication(repo: &str, reported: &str) -> Result<MergePolicy, String> {
    let line = reported
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix(PUBLICATION_LINE))
        .map(str::trim)
        .ok_or_else(|| {
            format!("`onevcs rules check {repo}` states no `{PUBLICATION_LINE}` line")
        })?;
    // The whole line rather than its first word. The sibling prints the policy and
    // then, in brackets, which rule decided it — so anything else on that line is a
    // report this build is reading by luck, and reading it by luck is how a
    // reworded one goes on being half-understood instead of saying it was not read.
    let (stated, whence) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    let whence = whence.trim();
    if !(whence.is_empty() || (whence.starts_with('(') && whence.ends_with(')'))) {
        return Err(format!(
            "`onevcs rules check {repo}` states a `{PUBLICATION_LINE}` line this build does \
             not read: {line:?}"
        ));
    }
    // Through the sibling's own type, so what may spell a policy is its list
    // rather than a second one here.
    serde_json::from_value(serde_json::Value::String(stated.to_owned())).map_err(|_| {
        format!("`onevcs rules check {repo}` states a publication policy this build does not know, '{stated}'")
    })
}

/// The refusal a node earns for consuming releases on an identity that can never
/// hold them.
///
/// One condition, and it is about the repository rather than about the node: it
/// has to publish without opening a change request. A `consumes` says this node's
/// work is pinned to a release, and every mechanism that holds such a pin back —
/// the draft `onevcs` opens, and the change request that draft is a *state of* —
/// requires a change request to exist. Where none is opened there is nothing to
/// hold, whatever the node's adoption says about *when* it starts, so the pair is
/// refused outright rather than at a publication that has already been paid for.
fn consumes_refusal(
    node: &Node,
    destination: &Destination,
) -> std::result::Result<Option<Refusal>, String> {
    // A node consuming nothing has nothing to hold, whatever its repository
    // publishes with — so it is answered without the resolved policy, and is never
    // reported as a node this build could not check for want of one.
    if node.consumes.is_empty() {
        return Ok(None);
    }
    // The policy this node publishes under: the one it states, where it states
    // one, and its repository's otherwise. Its own decides *whichever way it
    // points* — a node that names `local-direct` on a repository that opens change
    // requests has said it opens none, and reading its repository's answer over
    // its own would let exactly that node through. Where what it states is a
    // widening its repository forbids, `onevcs` refuses it at its own boundary,
    // which is a second refusal about a different thing rather than this one.
    let publication = match node.merge_policy {
        Some(stated) => stated,
        None => destination.publication.clone()?,
    };
    if opens_a_change_request(publication) {
        return Ok(None);
    }
    let consumed = node
        .consumes
        .iter()
        .map(|(dependency, target)| format!("{dependency}={target}"))
        .collect::<Vec<_>>()
        .join(", ");
    Ok(Some(
        Refusal::node(
            &node.id,
            format!(
                "it consumes the release targets {consumed}, and its repository {identity} \
                 (workflow: {workflow}) publishes with {publication}, which opens no change \
                 request at all — so there is nothing for the draft that holds this node to a \
                 release to be a state of, and `onevcs` refuses the publication outright at \
                 the last step of the node. Publish it under a change-* policy, on this node \
                 or in that repository's own rules, or drop `consumes`",
                identity = destination.resolved.identity,
                workflow = spell(destination.resolved.workflow),
                publication = spell(publication),
            ),
        )
        .field("consumes"),
    ))
}

/// Whether a publication under this policy opens a change request to draft.
///
/// One policy does not, and it is the one `onevcs` names when it refuses a draft:
/// `local-direct` squashes the branch onto the base with git alone.
fn opens_a_change_request(policy: MergePolicy) -> bool {
    policy != MergePolicy::LocalDirect
}

/// How the sibling's own types spell one of their values.
///
/// Through serde rather than a `match`, so neither list is restated here.
fn spell<T: serde::Serialize + std::fmt::Debug>(value: T) -> String {
    serde_json::to_value(&value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{value:?}"))
}

/// The refusal a node earns for a title its destination repository turns down.
///
/// `Err` is the hook not having been *asked* — a checkout git cannot answer for,
/// a hook that would not run — which is a node this build could not check rather
/// than one whose repository was satisfied.
fn title_refusal(node: &Node, destination: &Destination) -> Result<Option<Refusal>, String> {
    let Some(title) = node
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    else {
        return Ok(None);
    };
    let Some(rejection) = ask_the_hook(&destination.resolved.publication_checkout, title)? else {
        return Ok(None);
    };
    Ok(Some(
        Refusal::node(
            &node.id,
            format!(
                "the repository {identity} turns this node's title down at its own \
                 {COMMIT_MSG_HOOK} hook, so the publication this node ends with would be \
                 refused with the whole dispatch already paid for. The title is {title:?}, and \
                 the hook ({termination}) said:\n{said}",
                identity = destination.resolved.identity,
                termination = rejection.termination,
                said = rejection.said,
            ),
        )
        .field("title"),
    ))
}

/// How a process ended, which for a hook is two answers and not one.
///
/// A status and a signal are mutually exclusive and a `String` could hold both or
/// neither, which is how a report comes to name an exit status a hook never
/// reached. `Display` is the one place either is spelled.
#[derive(Debug, PartialEq, Eq)]
enum Termination {
    /// It ran to the end and answered with this status.
    Exit(i32),
    /// A signal stopped it before it reached one, so there is no status to name.
    Signal,
}

impl std::fmt::Display for Termination {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exit(status) => write!(f, "exit {status}"),
            Self::Signal => f.write_str("killed by a signal"),
        }
    }
}

#[derive(Debug)]
struct Rejected {
    termination: Termination,
    said: String,
}

/// Put one title to a repository's own `commit-msg` hook, the way git puts a
/// message to it.
///
/// `Ok(None)` covers both a hook that accepted and a repository that states no
/// policy at all: git runs a `commit-msg` only where there is an executable one,
/// so a repository without it is one this refusal has nothing to read, exactly as
/// `onevcs` reads it at the publication.
///
/// Where the hook lives is git's own answer — `core.hooksPath` wherever the
/// repository configures one — rather than a path composed here, so the file run
/// is the file the repository's own `git commit` runs.
fn ask_the_hook(checkout: &Path, title: &str) -> Result<Option<Rejected>, String> {
    let hooks = git_path(checkout, "hooks")?;
    let hook = hooks.join(COMMIT_MSG_HOOK);
    if !runnable(&hook)? {
        return Ok(None);
    }
    let message = write_message_file(title)?;
    let ran = Command::new(&hook)
        .arg(&message)
        .current_dir(checkout)
        .output()
        .map_err(|error| {
            format!(
                "the {COMMIT_MSG_HOOK} hook at {} could not be run: {error}",
                hook.display()
            )
        });
    // Best effort, and deliberately: a temporary file that outlives its use is
    // untidy, and turning that into a refusal would throw away a verdict the
    // repository has already given.
    let _ = std::fs::remove_file(&message);
    let ran = ran?;
    if ran.status.success() {
        return Ok(None);
    }
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&ran.stdout),
        String::from_utf8_lossy(&ran.stderr)
    );
    let said = said.trim();
    Ok(Some(Rejected {
        termination: ran
            .status
            .code()
            .map_or(Termination::Signal, Termination::Exit),
        said: if said.is_empty() {
            "<no output>".to_owned()
        } else {
            said.to_owned()
        },
    }))
}

/// A path git owns for a checkout, resolved **by git** rather than composed here.
///
/// `--git-path hooks` is the one name git resolves against `core.hooksPath`, so
/// asking it is the only way to get the answer a repository configured for
/// itself.
fn git_path(checkout: &Path, name: &str) -> Result<PathBuf, String> {
    let output = Command::new("git")
        .args(["rev-parse", "--git-path", name])
        .current_dir(checkout)
        .output()
        .map_err(|error| {
            format!(
                "git could not be run in the publication checkout {}: {error}",
                checkout.display()
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "git does not answer for the publication checkout {}: {}",
            checkout.display(),
            one_line(&String::from_utf8_lossy(&output.stderr))
        ));
    }
    let path = os_path(&output.stdout)?;
    Ok(if path.is_absolute() {
        path
    } else {
        checkout.join(path)
    })
}

/// A path git printed, as the bytes it printed.
///
/// A filesystem path is not text on this platform, so git's answer is kept in the
/// platform's own encoding: decoding it as UTF-8 — lossily above all — would turn
/// a hooks directory whose name is not valid Unicode into one that does not exist,
/// and the repository's own policy would go unread with nothing said about why.
#[cfg(unix)]
fn os_path(printed: &[u8]) -> Result<PathBuf, String> {
    use std::os::unix::ffi::OsStrExt;
    // Only the record terminator git ends the line with. A space and a tab are
    // legal in a path, so trimming whitespace would quietly rename a directory
    // that has one at either end into one that does not exist.
    let named = printed
        .strip_suffix(b"\n")
        .unwrap_or(printed)
        .strip_suffix(b"\r")
        .unwrap_or_else(|| printed.strip_suffix(b"\n").unwrap_or(printed));
    if named.is_empty() {
        return Err("git named no path at all".to_owned());
    }
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(named)))
}

/// A path git printed, on a platform whose paths this build can only read as text.
///
/// Windows paths are UTF-16 and a `Command`'s pipe hands back bytes, so there is
/// no lossless way across; an answer that is not UTF-8 is refused rather than
/// mangled into a path that names something else.
// llmlint: ignore-block[changed_behavior_has_e2e] no journey can reach this arm, on this
// platform or the one it is written for: git for Windows prints its paths as UTF-8, so
// producing the input is not something a test can arrange, and the arm exists precisely
// because a caller must not be handed a path that names something else if it ever did.
// The byte-preserving Unix half beside it *is* driven, by
// `a_hooks_directory_whose_name_is_not_unicode_is_found_and_its_hook_answers`, which
// fails against the lossy decode this replaced.
#[cfg(not(unix))]
fn os_path(printed: &[u8]) -> Result<PathBuf, String> {
    std::str::from_utf8(printed)
        .map(|path| PathBuf::from(path.trim()))
        .map_err(|error| format!("git printed a path that is not UTF-8: {error}"))
} // llmlint: ignore-end[changed_behavior_has_e2e]

/// Whether git would run this file as a hook.
///
/// The executable bit, which is git's own test on this platform: a `commit-msg`
/// that is there and not executable is one git skips, and skipping it here is
/// what keeps this answer and the publication's the same.
#[cfg(unix)]
fn runnable(path: &Path) -> Result<bool, String> {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(meta) => Ok(meta.is_file() && meta.permissions().mode() & 0o111 != 0),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        // A hook the filesystem will not answer for is not a hook that is absent:
        // reading the second as the first is how a repository that does state a
        // policy has none applied.
        Err(error) => Err(format!(
            "the {COMMIT_MSG_HOOK} hook at {} cannot be read: {error}",
            path.display()
        )),
    }
}

/// Whether git would run this file as a hook.
///
/// Windows carries no executable bit, so presence is the test — which is what Git
/// for Windows does too.
// llmlint: ignore-block[changed_behavior_has_e2e] the presence arm is driven on that
// platform by every journey here that installs a hook; what has no journey is the metadata
// error beside it, and it is the same case as `os_path`'s Windows arm above — a filesystem
// that answers neither "here" nor "not found" is not a state a test can arrange, and the
// arm exists so that one is never read as the hook being absent, which is how a repository
// that does state a policy would have none applied.
#[cfg(not(unix))]
fn runnable(path: &Path) -> Result<bool, String> {
    match std::fs::metadata(path) {
        Ok(meta) => Ok(meta.is_file()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!(
            "the {COMMIT_MSG_HOOK} hook at {} cannot be read: {error}",
            path.display()
        )),
    }
} // llmlint: ignore-end[changed_behavior_has_e2e]

/// The file the hook is handed, which is the single argument git hands it.
///
/// Named for this process and for this call, because several loads can be running
/// on one host at once and a shared name is one of them reading another's title.
fn write_message_file(title: &str) -> Result<PathBuf, String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static ASKED: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "onepipeline-{COMMIT_MSG_HOOK}-{}-{}",
        crate::sys::pid(),
        ASKED.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, format!("{title}\n")).map_err(|error| {
        format!(
            "the message put to the hook could not be written to {}: {error}",
            path.display()
        )
    })?;
    Ok(path)
}

fn one_line(said: &str) -> String {
    let said = said.trim();
    match said.lines().next() {
        Some(first) if !first.is_empty() => first.to_owned(),
        _ => "it said nothing".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The policy is read off the line the sibling states it on, and every other
    /// answer is a node this build could not check.
    ///
    /// The refusals are the point. This read is prose, deliberately — no `onevcs`
    /// surface answers the resolved policy as data at the pinned release — so what
    /// keeps it honest is that anything it does not recognise becomes "could not
    /// check" rather than an accept: a reworded report must never be able to turn a
    /// real refusal into a silent pass.
    #[test]
    fn the_publication_policy_is_read_off_the_line_and_nothing_else_is_taken_for_one() {
        let reported = "repo: github.com/owner/service\n\
             identity: github.com/owner/service\n\
             checkout: /tmp/service\n\
             rules: /tmp/home/rules.yml\n\
             matched: rule 1 {host: github.com, owner: owner, name: service}\n\
             publication: local-direct (from rule 1)\n\
             approvals: none (from rule 1)\n";
        assert_eq!(
            publication("service", reported).expect("the policy is read"),
            MergePolicy::LocalDirect
        );
        assert_eq!(
            publication("service", "publication: change-auto (from the default)\n")
                .expect("the policy is read"),
            MergePolicy::ChangeAuto
        );

        // A report with no such line at all — which is every rewording of it.
        let missing = publication("service", "approvals: none\n").expect_err("no policy is stated");
        assert!(
            missing.contains("states no `publication:` line"),
            "{missing}"
        );

        // A word on that line the sibling's own type does not accept. `approvals`
        // sits on the line below and is a legal answer to a different question, so
        // it is the value most likely to arrive here by a wiring mistake.
        let unknown = publication("service", "publication: none (from rule 1)\n")
            .expect_err("an unknown policy is not a policy");
        assert!(unknown.contains("does not know, 'none'"), "{unknown}");

        // The whole line, not its first word: the sibling states the policy and
        // then, in brackets, which rule decided it. Anything else on it is a
        // report this build would be reading by luck.
        let reworded = publication("service", "publication: local-direct, from rule 1\n")
            .expect_err("a line this build does not read is not read anyway");
        assert!(
            reworded.contains("line this build does not read"),
            "{reworded}"
        );
        assert_eq!(
            publication("service", "publication: change-open\n").expect("a bare policy is read"),
            MergePolicy::ChangeOpen,
            "the bracketed provenance is optional, as a `--policy` override prints it"
        );
    }

    /// A refusal that carries somebody else's output inline carries one line of
    /// it, and says so where there was none.
    #[test]
    fn a_borrowed_sentence_is_one_line_and_never_an_empty_one() {
        assert_eq!(
            one_line("  onevcs: no such identity\nand more\n"),
            "onevcs: no such identity"
        );
        assert_eq!(one_line("   \n  \n"), "it said nothing");
        assert_eq!(one_line(""), "it said nothing");
    }

    /// Both siblings' vocabularies are spelled by their own types.
    #[test]
    fn a_policy_and_an_adoption_are_spelled_by_the_types_that_own_them() {
        assert_eq!(spell(MergePolicy::LocalDirect), "local-direct");
        assert_eq!(spell(MergePolicy::ChangeAuto), "change-auto");
        assert_eq!(spell(onevcs::Adoption::Published), "published");
        assert_eq!(spell(onevcs::Adoption::Fast), "fast");
    }

    /// A destination this journey states, so the two refusal rules can be asked
    /// about a repository without one being resolved.
    fn destination(publication: MergePolicy) -> Destination {
        Destination {
            resolved: resolved(),
            publication: Ok(publication),
        }
    }

    /// What the resolution verb stated, as this journey states it.
    fn resolved() -> Resolved {
        Resolved {
            identity: "github.com/owner/service".to_owned(),
            workflow: onevcs::registry::Workflow::Remote,
            publication_checkout: PathBuf::from("/tmp/service"),
        }
    }

    fn consuming(adoption: Option<onevcs::Adoption>) -> Node {
        let mut node = Node {
            id: "consumer".to_owned(),
            repo: Some("service".to_owned()),
            deps: vec!["engine".to_owned()],
            adoption,
            ..Node::default()
        };
        node.consumes
            .insert("engine".to_owned(), "crate".parse().expect("a target name"));
        node
    }

    /// The rule the `consumes` refusal actually applies, in each of its arms.
    ///
    /// It turns on the **repository** and on nothing else: where no change request
    /// is opened there is nothing for a draft to be a state of, so every `consumes`
    /// on such an identity is refused whatever its adoption says about when the
    /// node starts. What loads is a node with nothing to consume, and one whose
    /// publication opens a change request — its repository's, or its own.
    #[test]
    fn a_consumes_is_refused_wherever_its_publication_opens_no_change_request() {
        let refusal = consumes_refusal(&consuming(None), &destination(MergePolicy::LocalDirect))
            .expect("the policy is known, so the rule is answerable")
            .expect("a fast node consuming on a local-direct repository is refused");
        assert_eq!(refusal.node.as_deref(), Some("consumer"));
        assert_eq!(refusal.field.as_deref(), Some("consumes"));
        for named in [
            "node 'consumer'",
            "engine=crate",
            "github.com/owner/service",
            "workflow: remote",
            "local-direct",
        ] {
            assert!(
                refusal.message.contains(named),
                "the refusal does not name {named}: {}",
                refusal.message
            );
        }

        let loads = |why: &str, node: &Node, destination: &Destination| {
            assert!(
                consumes_refusal(node, destination)
                    .expect("the rule is answerable")
                    .is_none(),
                "{why}"
            );
        };
        // Adoption decides when a node starts, not whether its publication opens a
        // change request — so it does not enter this rule at all.
        for adoption in [onevcs::Adoption::Fast, onevcs::Adoption::Published] {
            assert!(
                consumes_refusal(
                    &consuming(Some(adoption)),
                    &destination(MergePolicy::LocalDirect)
                )
                .expect("the rule is answerable")
                .is_some(),
                "a `{}` node consuming on a local-direct repository was not refused",
                spell(adoption)
            );
        }
        loads(
            "a repository that opens a change request has something to draft",
            &consuming(None),
            &destination(MergePolicy::ChangeAuto),
        );
        let mut bare = consuming(None);
        bare.consumes.clear();
        loads(
            "a node consuming nothing holds no pin",
            &bare,
            &destination(MergePolicy::LocalDirect),
        );

        // A node states the policy it publishes under, and it decides whichever
        // way it points.
        let mut narrowed = consuming(None);
        narrowed.merge_policy = Some(MergePolicy::ChangeOpen);
        loads(
            "a node that named a change-* policy opens a change request to draft",
            &narrowed,
            &destination(MergePolicy::LocalDirect),
        );
        let mut stated = consuming(None);
        stated.merge_policy = Some(MergePolicy::LocalDirect);
        assert!(
            consumes_refusal(&stated, &destination(MergePolicy::ChangeAuto))
                .expect("the rule is answerable")
                .is_some(),
            "a node that named `local-direct` itself was let through on its repository's answer"
        );
    }

    /// A policy this build could not read is a node it could not check — and only
    /// for the rule that needed the policy.
    ///
    /// The two rules are asked separately on purpose. A `rules check` that could
    /// not be read says nothing about a repository's subject policy, so a title is
    /// still put to that repository's own hook; and a node with nothing to consume,
    /// or one that has narrowed to a change-* policy, is answered without the
    /// policy at all rather than reported as unchecked for want of it.
    #[test]
    fn a_policy_this_build_could_not_read_is_reported_only_where_the_rule_needed_it() {
        let unreadable = Destination {
            resolved: resolved(),
            publication: Err(
                "`onevcs rules check service` states no `publication:` line".to_owned()
            ),
        };
        let why = consumes_refusal(&consuming(None), &unreadable)
            .expect_err("a rule that needs the policy cannot be answered without it");
        assert!(why.contains("states no `publication:` line"), "{why}");

        let mut bare = consuming(None);
        bare.consumes.clear();
        assert!(consumes_refusal(&bare, &unreadable)
            .expect("a node consuming nothing needs no policy")
            .is_none());
        let mut narrowed = consuming(None);
        narrowed.merge_policy = Some(MergePolicy::ChangeAuto);
        assert!(consumes_refusal(&narrowed, &unreadable)
            .expect("a node that named its own change-* policy needs no resolved one")
            .is_none());
    }

    /// A hook that is not there is a repository stating no policy; a path that is
    /// not a repository at all is a question this build could not ask.
    #[test]
    fn an_absent_hook_states_no_policy_and_a_directory_git_cannot_answer_for_is_not_a_verdict() {
        let scratch = std::env::temp_dir().join(format!(
            "onepipeline-destination-{}-{}",
            crate::sys::pid(),
            "nohook"
        ));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).expect("a scratch directory");

        assert!(
            !runnable(&scratch.join(COMMIT_MSG_HOOK))
                .expect("an absent hook is readable as absent"),
            "a hook that is not there was read as one git would run"
        );

        // Not a git repository, so git answers for nothing here — which is a
        // node that could not be checked rather than a title that passed.
        let why = ask_the_hook(&scratch, "feat: x")
            .expect_err("a directory that is no repository answers no verdict");
        assert!(
            why.contains(&scratch.display().to_string()),
            "the refusal does not name the checkout it could not read: {why}"
        );

        let _ = std::fs::remove_dir_all(&scratch);
    }

    /// A repository's own hook, asked about three titles in a real checkout.
    ///
    /// The one arm worth stating on its own is the **silent** refusal: a hook that
    /// exits non-zero and writes nothing has still turned the title down, and a
    /// refusal that carried an empty quotation would read as one nobody made.
    ///
    /// Unix-only for the reason [`ask_the_hook`]'s callers are: a `#!` script is
    /// started directly here, exactly as `onevcs` starts it at the publication.
    #[cfg(unix)]
    #[test]
    fn a_repositorys_own_hook_answers_for_each_title_it_is_put() {
        use std::os::unix::fs::PermissionsExt;

        let checkout = std::env::temp_dir().join(format!(
            "onepipeline-destination-hook-{}",
            crate::sys::pid()
        ));
        let _ = std::fs::remove_dir_all(&checkout);
        std::fs::create_dir_all(&checkout).expect("a checkout to ask");
        let git = |args: &[&str]| {
            let ran = Command::new("git")
                .args(args)
                .current_dir(&checkout)
                .output()
                .expect("git runs");
            assert!(ran.status.success(), "git {args:?}: {ran:?}");
        };
        git(&["init", "--initial-branch=main"]);

        // Where this repository keeps its hooks is its own business, and
        // `core.hooksPath` is how it says so — the setting the answer has to be
        // read through rather than around.
        let hooks = checkout.join("policy-hooks");
        std::fs::create_dir_all(&hooks).expect("a hooks directory");
        git(&["config", "core.hooksPath", "policy-hooks"]);

        // Nothing installed yet: the repository states no subject policy.
        assert!(
            ask_the_hook(&checkout, "refactor(x): y")
                .expect("a repository with no hook answers")
                .is_none(),
            "a repository with no commit-msg hook acquired one by being asked"
        );

        let install = |body: &str| {
            let path = hooks.join(COMMIT_MSG_HOOK);
            std::fs::write(&path, body).expect("the hook is written");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("the hook is executable");
        };

        install(
            "#!/bin/sh\ngrep -q '^feat' \"$1\" && exit 0\necho 'only feat: here' >&2\nexit 3\n",
        );
        assert!(
            ask_the_hook(&checkout, "feat: ship it")
                .expect("the hook runs")
                .is_none(),
            "a title this repository releases from was turned down"
        );
        let rejected = ask_the_hook(&checkout, "refactor(x): y")
            .expect("the hook runs")
            .expect("a title this repository does not release from is turned down");
        assert_eq!(rejected.termination, Termination::Exit(3));
        assert_eq!(rejected.said, "only feat: here");

        // A hook that refuses and says nothing has still refused.
        install("#!/bin/sh\nexit 1\n");
        let silent = ask_the_hook(&checkout, "feat: ship it")
            .expect("the hook runs")
            .expect("a silent refusal is still a refusal");
        assert_eq!(silent.termination, Termination::Exit(1));
        assert_eq!(
            silent.said, "<no output>",
            "a refusal nobody can read must say that it said nothing"
        );

        // A hook git would not run — present, and not executable — is a
        // repository stating no policy, because that is what git makes of it.
        let path = hooks.join(COMMIT_MSG_HOOK);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .expect("the hook is made unrunnable");
        assert!(
            ask_the_hook(&checkout, "refactor(x): y")
                .expect("a hook git would skip answers")
                .is_none(),
            "a hook git would skip was run anyway"
        );

        let _ = std::fs::remove_dir_all(&checkout);
    }

    /// The message put to the hook is written per call, so two loads on one host
    /// never read each other's title.
    #[test]
    fn every_message_put_to_a_hook_is_its_own_file() {
        let first = write_message_file("feat: one").expect("a message file");
        let second = write_message_file("feat: two").expect("a second message file");
        assert_ne!(first, second, "two calls shared one message file");
        assert_eq!(
            std::fs::read_to_string(&first).expect("the first is written"),
            "feat: one\n"
        );
        assert_eq!(
            std::fs::read_to_string(&second).expect("the second is written"),
            "feat: two\n"
        );
        for path in [first, second] {
            let _ = std::fs::remove_file(path);
        }
    }
}
