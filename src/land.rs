//! Landing a branch **out of band**: `onepipeline publish-branch` and
//! `onepipeline repo-recover`.
//!
//! The drafter is this crate's — the lifecycle closeout's [`crate::lifecycle::draft`]
//! — and the verbs an operator lands a branch with outside a run are `onevcs`'s, so
//! this is the one place the two meet: the linked verb, **called** rather than
//! spawned, with that drafter in front of it. What the verbs promise, and why they
//! are this crate's rather than the consumer's, is entry 88 of
//! `docs/contract-divergences.md`.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use clap::{CommandFactory, Parser};

use crate::controls::NodeControls;
use crate::error::{Error, Result};
use crate::event::{Envelope, Labels};
use crate::executor::{CancellationToken, DispatchRequest, LocalExecutor, WorkspaceSpec};
use crate::ledger::RunPaths;
use crate::lifecycle::{Drafted, Undrafted, PR_AUTHOR_PERSONA};

/// The flag naming the graph a body is drafted by — the spelling `start` gives
/// the same graph.
pub(crate) const PR_AUTHOR_GRAPH_FLAG: &str = "--pr-author-graph";

/// The flag that lands with no drafting turn spent, for a bulk landing that would
/// otherwise pay one agent turn per branch.
pub(crate) const NO_DRAFT_FLAG: &str = "--no-draft";

/// Which `onevcs` verb a landing forwards to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verb {
    /// `onevcs publish-branch`, which `onepipeline publish-branch` fronts.
    PublishBranch,
    /// `onevcs recover`, which `onepipeline repo-recover` fronts.
    Recover,
}

impl Verb {
    /// The verb's name on `onevcs`'s own command line.
    fn onevcs(self) -> &'static str {
        match self {
            Self::PublishBranch => "publish-branch",
            Self::Recover => "recover",
        }
    }
}

/// What a landing's command line said, once this crate's two flags are taken out
/// of it.
#[derive(Debug, Default, PartialEq, Eq)]
struct Split {
    /// Everything else, in the order it was typed: `onevcs`'s to judge.
    forwarded: Vec<OsString>,
    /// What [`PR_AUTHOR_GRAPH_FLAG`] named, the last time it was given.
    graph: Option<OsString>,
    /// Whether the caller declined a draft with [`NO_DRAFT_FLAG`].
    drafting: Drafting,
}

/// Whether a landing may spend a drafting turn at all.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Drafting {
    /// Where one is wanted: the caller brought no body and the identity opens a
    /// change request.
    #[default]
    WhereWanted,
    /// Never: the caller said [`NO_DRAFT_FLAG`].
    Declined,
}

/// Land one branch through the linked `onevcs` verb, drafting its body first
/// where one is wanted, and answer with that verb's exit status.
pub(crate) fn land(verb: Verb, args: Vec<OsString>) -> Result<i32> {
    let split = split(verb, args)?;
    let argv = [OsString::from("onevcs"), OsString::from(verb.onevcs())]
        .into_iter()
        .chain(split.forwarded);
    // `onevcs`'s own parser, so what may be passed is its list and a refused
    // argument is refused in its words — exactly as its own binary would.
    let mut cli = match onevcs::cli::Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        Err(refused) => {
            // llmlint: ignore[cli_output_contract] this is `onevcs`'s own usage
            // refusal or help, printed to the stream clap chooses, at the code it
            // chooses — which is what the passthrough owes a caller.
            let _ = refused.print();
            return Ok(refused.exit_code());
        }
    };
    if let Some(body) = drafted_for(verb, &cli, &split.graph, split.drafting)? {
        match &mut cli.command {
            onevcs::cli::Command::PublishBranch(args) => args.body = Some(body),
            onevcs::cli::Command::Recover(args) => args.body = Some(body),
            _ => {}
        }
    }
    Ok(i32::from(onevcs::run(&cli)))
}

/// Take this crate's two flags out of a landing's command line, leaving every
/// other argument where it was.
///
/// Before a `--` only, because everything after one is a positional to `onevcs`.
/// An option `onevcs`'s parser says takes a value has that value carried with it
/// unread, so a title that happens to read `--no-draft` stays a title: which
/// options take one is asked of the sibling's own parser rather than listed here.
fn split(verb: Verb, args: Vec<OsString>) -> Result<Split> {
    let valued = valued_options(verb);
    let mut split = Split::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let text = arg.to_str().unwrap_or_default();
        if text == "--" {
            split.forwarded.push(arg);
            split.forwarded.extend(args);
            break;
        } else if text == NO_DRAFT_FLAG {
            split.drafting = Drafting::Declined;
        } else if text == PR_AUTHOR_GRAPH_FLAG {
            split.graph = Some(args.next().ok_or_else(|| {
                Error::Invalid(format!(
                    "{PR_AUTHOR_GRAPH_FLAG} was given no value: name the graph the body is \
                     drafted by, or pass {NO_DRAFT_FLAG}"
                ))
            })?);
        } else if let Some(graph) = text.strip_prefix(&format!("{PR_AUTHOR_GRAPH_FLAG}=")) {
            split.graph = Some(graph.into());
        } else if valued.iter().any(|option| option == text) {
            split.forwarded.push(arg);
            split.forwarded.extend(args.next());
        } else {
            split.forwarded.push(arg);
        }
    }
    Ok(split)
}

/// Every spelling of an option the `onevcs` verb takes a separate value for, read
/// off that verb's own parser.
fn valued_options(verb: Verb) -> Vec<String> {
    let cli = onevcs::cli::Cli::command();
    let Some(command) = cli.find_subcommand(verb.onevcs()) else {
        return Vec::new();
    };
    command
        .get_arguments()
        .filter(|argument| !argument.is_positional() && argument.get_action().takes_values())
        .flat_map(|argument| {
            argument
                .get_long()
                .map(|long| format!("--{long}"))
                .into_iter()
                .chain(argument.get_short().map(|short| format!("-{short}")))
        })
        .collect()
}

/// The body this landing is drafted, where one is drafted at all.
///
/// Nothing is drafted — and no turn spent — for a caller who said `--no-draft`,
/// one who brought a body of their own, or an identity whose publication is
/// `local-direct`, which opens no change request for a body to describe. The
/// workflow is the identity's **rules**, read through the resolution the loader
/// asks — never the registration's own field — or the `--policy` a
/// `publish-branch` was narrowed to. An identity whose rules this build could not
/// read is drafted for, because a landing that misses a body somebody wanted is
/// the worse of the two ways to be wrong.
///
/// Where drafting would happen and no graph is named, the landing is refused
/// before anything runs: the caller asked for a body and said nothing that could
/// write one.
fn drafted_for(
    verb: Verb,
    cli: &onevcs::cli::Cli,
    graph: &Option<OsString>,
    drafting: Drafting,
) -> Result<Option<String>> {
    let (branch, repo, brought, narrowed) = match (&cli.command, verb) {
        (onevcs::cli::Command::PublishBranch(args), Verb::PublishBranch) => (
            &args.branch,
            &args.repo,
            args.body.is_some() || args.body_file.is_some(),
            args.policy,
        ),
        (onevcs::cli::Command::Recover(args), Verb::Recover) => (
            &args.branch,
            &args.repo,
            args.body.is_some() || args.body_file.is_some(),
            None,
        ),
        // llmlint: ignore[changed_behavior_has_e2e] unreachable: the command line was
        // parsed with this verb's name as its subcommand, so `onevcs`'s parser answers
        // that variant or refuses. Kept as a refusal rather than a panic so a sibling
        // release that renamed a verb is a message, not a crash.
        _ => {
            return Err(Error::Invalid(format!(
                "`onevcs {}` parsed as a different command",
                verb.onevcs()
            )))
        }
    };
    if drafting == Drafting::Declined || brought {
        return Ok(None);
    }
    let named = repo.to_string_lossy();
    let destination = crate::destination::resolve(&named);
    let publication = narrowed.or_else(|| {
        destination
            .as_ref()
            .ok()
            .and_then(|destination| destination.publication.clone().ok())
    });
    if publication.is_some_and(|policy| !crate::destination::opens_a_change_request(policy)) {
        return Ok(None);
    }
    let Some(graph) = graph else {
        return Err(Error::Invalid(format!(
            "`onepipeline {}` would draft the change request's body for {branch}, and no graph \
             to draft it with was named: pass {PR_AUTHOR_GRAPH_FLAG} <PATH>, bring a body with \
             --body or --body-file, or pass {NO_DRAFT_FLAG}",
            match verb {
                Verb::PublishBranch => "publish-branch",
                Verb::Recover => "repo-recover",
            }
        )));
    };
    let graph = graph.to_str().ok_or_else(|| {
        Error::Invalid(format!(
            "{PR_AUTHOR_GRAPH_FLAG} names a path that is not UTF-8: {}",
            graph.to_string_lossy()
        ))
    })?;
    let here = std::env::current_dir().map_err(|error| {
        Error::Invalid(format!(
            "the working directory {PR_AUTHOR_GRAPH_FLAG} resolves against cannot be read: \
             {error}"
        ))
    })?;
    let graph = crate::driver::resolve_graph(graph, &here)?;
    // The directory the drafter reads the branch from: the value itself where it is
    // one, and otherwise the publication checkout the registry maps it to.
    let checkout = if repo.is_dir() {
        Ok(repo.clone())
    } else {
        destination.map(|destination| destination.resolved.publication_checkout)
    };
    let drafted = match checkout {
        Ok(checkout) => draft(&graph, &checkout, branch),
        Err(why) => Drafted::Undrafted(Undrafted::Dispatch(format!(
            "the drafting dispatch could not start: {why}"
        ))),
    };
    Ok(match drafted {
        Drafted::Body(body) => Some(body),
        Drafted::Undrafted(ending) => {
            eprintln!(
                "onepipeline: {} ({}), so {branch} lands with no body",
                ending.why(),
                ending.ending()
            );
            None
        }
    })
}

/// Draft one branch's body through `graph`, in a temporary detached worktree of
/// the branch cut from `checkout` and removed however this returns.
fn draft(graph: &str, checkout: &Path, branch: &str) -> Drafted {
    let could_not_start = |why: String| {
        Drafted::Undrafted(Undrafted::Dispatch(format!(
            "the drafting dispatch could not start: {why}"
        )))
    };
    let scratch = match Scratch::new(checkout) {
        Ok(scratch) => scratch,
        Err(why) => return could_not_start(why),
    };
    let tree = match scratch.cut(branch) {
        Ok(tree) => tree,
        Err(why) => return could_not_start(why),
    };
    let paths = RunPaths::under(&scratch.dir, "draft");
    if let Err(error) = std::fs::create_dir_all(&paths.dir) {
        return could_not_start(format!(
            "{} could not be created: {error}",
            paths.dir.display()
        ));
    }
    let mut deaths = Vec::new();
    let (drafted, handle) = crate::lifecycle::draft(
        &LocalExecutor,
        DispatchRequest {
            graph: oneagentgraph::config::ConfigRef(graph.to_owned()),
            task: crate::lifecycle::out_of_band_drafting_task(branch, &tree.base, &tree.commits),
            // The persona alone: there is no run and no node, so the dispatch is
            // registered nowhere and composes nothing of a run's onto the graph —
            // the same drafting dispatch the closeout makes, outside a run.
            labels: Labels {
                persona: Some(PR_AUTHOR_PERSONA.to_owned()),
                ..Labels::default()
            },
            controls: NodeControls::default(),
            workspace: WorkspaceSpec::Path(tree.worktree),
            cancel: CancellationToken::new(),
            attempt: std::num::NonZeroU32::MIN,
        },
        &paths,
        &mut |envelope| deaths.extend(death(&envelope)),
    );
    drop(handle);
    if matches!(drafted, Drafted::Undrafted(Undrafted::Dispatch(_))) {
        for died in &deaths {
            eprintln!("onepipeline: {died}");
        }
    }
    drafted
}

/// One `member-died` a drafting graph published, as the line that says what
/// `oneagentgraph` classified it as — or `None` for any other envelope.
///
/// Read by field rather than into the sibling's `MemberDied`, for the reason the
/// engine's own reading of a death gives: that type refuses unknown fields, and a
/// newer producer adding one would cost the operator the diagnosis. A detail the
/// producer marked truncated is said to be, because the fix could be in the part
/// that was cut.
fn death(envelope: &Envelope) -> Option<String> {
    if envelope.source != crate::event::Source::Agentgraph
        || envelope.kind.0 != oneagentgraph::event::EventKind::MemberDied.as_str()
    {
        return None;
    }
    let member = envelope.labels.member.as_deref().unwrap_or("a member");
    let stated: Vec<String> = ["rule", "cause", "detail"]
        .into_iter()
        .filter_map(|field| {
            let value = envelope.payload.get(field)?.as_str()?;
            (!value.is_empty()).then(|| format!("{field}={}", crate::views::one_line(value)))
        })
        .collect();
    let truncated = envelope
        .payload
        .get("truncated")
        .and_then(serde_json::Value::as_bool)
        == Some(true);
    Some(match (stated.is_empty(), truncated) {
        (true, _) => {
            format!("{member} died, and oneagentgraph named no rule, cause or detail for it")
        }
        (false, false) => format!("{member} died: {}", stated.join(" ")),
        (false, true) => format!(
            "{member} died: {} (oneagentgraph truncated this detail)",
            stated.join(" ")
        ),
    })
}

/// A drafting turn's own directory, and the worktree it cuts from the checkout.
///
/// Dropped on every way out of [`draft`], a panic included, and dropping it
/// removes both: the worktree **by path** with `git worktree remove`, which is
/// what takes its entry out of the checkout's worktree list — never `prune`, which
/// would sweep every other stale entry that checkout happens to carry, and those
/// are somebody else's — and then the directory. Either failing is said on
/// standard error rather than swallowed, because the promise is that the checkout
/// is left as it was found.
///
/// A drop is not every way out: a person interrupting the landing from a
/// terminal ends the process with no unwind at all. So for as long as it lives
/// it holds [`crate::sys::on_interrupt`], and a signal that guard holds removes the
/// same two things before the process ends of that signal. What both paths remove
/// is one [`Leftover`] behind one lock, so whichever comes second finds nothing
/// left to do, and what is recorded there is only ever what was made.
struct Scratch {
    dir: PathBuf,
    checkout: PathBuf,
    left: Arc<Mutex<Leftover>>,
    /// Declared last so it is released after [`Drop::drop`] has emptied `left`:
    /// a signal in between finds nothing to remove and still ends the process.
    _interrupts: crate::sys::OnInterrupt,
}

/// What a drafting turn has made and not yet removed.
struct Leftover {
    checkout: PathBuf,
    /// The turn's directory, once it has been created.
    dir: Option<PathBuf>,
    /// The worktree, once one has been cut — and only then, so what is removed is
    /// only ever what was added.
    worktree: Option<PathBuf>,
}

impl Leftover {
    /// Remove whatever is left, saying on standard error what could not be.
    fn remove(&mut self) {
        if let Some(worktree) = self.worktree.take() {
            let removed = git_in(
                &self.checkout,
                [
                    OsStr::new("worktree"),
                    OsStr::new("remove"),
                    OsStr::new("--force"),
                    worktree.as_os_str(),
                ],
            );
            if let Err(why) = removed {
                eprintln!(
                    "onepipeline: the drafting worktree at {} could not be removed from {}: \
                     {why}; remove it with `git -C {} worktree remove --force {}`",
                    worktree.display(),
                    self.checkout.display(),
                    self.checkout.display(),
                    worktree.display()
                );
            }
        }
        if let Some(dir) = self.dir.take() {
            if let Err(error) = std::fs::remove_dir_all(&dir) {
                eprintln!(
                    "onepipeline: the drafting turn's directory {} could not be removed: \
                     {error}; remove it by hand",
                    dir.display()
                );
            }
        }
    }
}

/// The leftover behind its lock, whoever panicked while holding it: what it
/// records is still exactly what was made.
fn lock_leftover(leftover: &Mutex<Leftover>) -> std::sync::MutexGuard<'_, Leftover> {
    leftover
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What the branch says about itself, read out of the checkout before the
/// worktree is cut.
struct Tree {
    /// The base, as a ref the checkout resolves.
    base: String,
    /// The full commit messages of `<base>..<branch>`, oldest first.
    commits: String,
    /// The detached worktree of the branch the drafter works in.
    worktree: PathBuf,
}

impl Scratch {
    fn new(checkout: &Path) -> std::result::Result<Self, String> {
        static MINTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let leftover = Arc::new(Mutex::new(Leftover {
            checkout: checkout.to_path_buf(),
            dir: None,
            worktree: None,
        }));
        // Held before anything is made, so there is no moment in which something
        // exists that an interrupt would leave behind.
        let interrupts = crate::sys::on_interrupt({
            let leftover = Arc::clone(&leftover);
            move || {
                eprintln!(
                    "onepipeline: interrupted while drafting; removing the drafting worktree"
                );
                lock_leftover(&leftover).remove();
            }
        })?;
        let root = std::env::temp_dir();
        for _ in 0..64 {
            let dir = root.join(format!(
                "onepipeline-draft-{}-{}",
                crate::sys::pid(),
                MINTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            let mut made = lock_leftover(&leftover);
            match std::fs::create_dir(&dir) {
                Ok(()) => {
                    made.dir = Some(dir.clone());
                    drop(made);
                    return Ok(Self {
                        dir,
                        checkout: checkout.to_path_buf(),
                        left: leftover,
                        _interrupts: interrupts,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(format!("{} could not be created: {error}", dir.display()))
                }
            }
        }
        Err(format!(
            "no directory for the drafting turn could be created under {}",
            root.display()
        ))
    }

    /// Read what the branch says about itself and cut a detached worktree of it.
    ///
    /// The branch is the checkout's own where it has one, and its origin's where
    /// it does not — a preserved branch `recover` lands may be only there. The base
    /// is the origin's default branch, which is the base `onevcs` lands both verbs
    /// onto.
    fn cut(&self, branch: &str) -> std::result::Result<Tree, String> {
        let head = [
            format!("refs/heads/{branch}"),
            format!("refs/remotes/origin/{branch}"),
        ]
        .into_iter()
        .find(|reference| {
            self.git(&["show-ref", "--verify", "--quiet", reference])
                .is_ok()
        })
        .ok_or_else(|| {
            format!(
                "the checkout {} has no branch '{branch}', locally or on its origin",
                self.checkout.display()
            )
        })?;
        let base = self.base().ok_or_else(|| {
            format!(
                "the base {branch} lands on is unknown: {} names no default branch for its \
                 origin",
                self.checkout.display()
            )
        })?;
        let commits = self.git(&[
            "log",
            "--reverse",
            "--format=commit %h%n%n%B",
            &format!("{base}..{head}"),
            "--",
        ])?;
        let worktree = self.dir.join("branch");
        // Under the lock, so an interrupt arriving while git is adding the
        // worktree waits for it and then removes it, rather than finding nothing
        // recorded and leaving the entry git is about to write.
        let mut made = lock_leftover(&self.left);
        git_in(
            &self.checkout,
            [
                OsStr::new("worktree"),
                OsStr::new("add"),
                OsStr::new("--detach"),
                worktree.as_os_str(),
                OsStr::new(&head),
            ],
        )?;
        made.worktree = Some(worktree.clone());
        drop(made);
        Ok(Tree {
            base,
            commits,
            worktree,
        })
    }

    /// The origin's default branch, as a ref the checkout resolves — asked the way
    /// `onevcs` asks it for the landing itself: the checkout's own record of the
    /// origin's `HEAD`, then what the origin advertises, then the one branch the
    /// checkout tracks there when it tracks only one.
    fn base(&self) -> Option<String> {
        if let Ok(tracked) = self.git(&[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ]) {
            return Some(tracked);
        }
        let advertised = self
            .git(&["ls-remote", "--symref", "origin", "HEAD"])
            .ok()
            .and_then(|said| {
                said.lines().find_map(|line| {
                    let (named, _) = line.strip_prefix("ref: refs/heads/")?.split_once('\t')?;
                    Some(format!("origin/{named}"))
                })
            })
            .filter(|named| {
                self.git(&[
                    "show-ref",
                    "--verify",
                    "--quiet",
                    &format!("refs/remotes/{named}"),
                ])
                .is_ok()
            });
        if advertised.is_some() {
            return advertised;
        }
        let tracked = self
            .git(&[
                "for-each-ref",
                "--format=%(refname:short)",
                "refs/remotes/origin",
            ])
            .ok()?;
        let mut candidates = tracked.lines().filter(|named| *named != "origin/HEAD");
        match (candidates.next(), candidates.next()) {
            (Some(only), None) => Some(only.to_owned()),
            _ => None,
        }
    }

    /// Run git in the checkout, answering its stdout or why it did not answer.
    fn git(&self, args: &[&str]) -> std::result::Result<String, String> {
        git_in(&self.checkout, args.iter().map(OsStr::new))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        lock_leftover(&self.left).remove();
    }
}

/// Run one git command in `dir`, answering its trimmed stdout or its refusal.
///
/// Its stdout is read as UTF-8 or not at all: it is refs, a base and the commit
/// messages a drafter is handed, and a lossy decode would hand the drafter text
/// nobody wrote. Its stderr is a sentence for a person, relayed lossily so a byte
/// that is not UTF-8 does not cost the operator the only thing the failure said —
/// the split `destination::ask` makes for the same reason.
fn git_in<'a>(
    dir: &Path,
    args: impl IntoIterator<Item = &'a OsStr>,
) -> std::result::Result<String, String> {
    let args: Vec<&OsStr> = args.into_iter().collect();
    let named = || {
        let words: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        format!("`git {}` in {}", words.join(" "), dir.display())
    };
    let output = Command::new("git")
        .args(&args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("{} could not be run: {error}", named()))?;
    if !output.status.success() {
        return Err(format!(
            "{} exited {}: {}",
            named(),
            output.status.code().unwrap_or(-1),
            // llmlint: ignore[boundary_inputs_validated] a diagnostic relayed to a
            // person, never parsed or acted on: see this function's own note.
            crate::views::one_line(&String::from_utf8_lossy(&output.stderr))
        ));
    }
    String::from_utf8(output.stdout)
        .map(|said| said.trim().to_owned())
        .map_err(|error| format!("{} answered bytes that are not UTF-8: {error}", named()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    /// This crate's two flags come out wherever they sit — the consumer appends
    /// `--pr-author-graph` after everything it forwards — and every other argument
    /// keeps its place, a valued option's value included however it reads.
    #[test]
    fn only_this_crates_two_flags_are_taken_out_and_the_rest_keep_their_order() {
        let taken = split(
            Verb::PublishBranch,
            words(&[
                "feature",
                "--title",
                "--no-draft",
                "--repo=service",
                "--no-draft",
                "--policy",
                "change-open",
                "--pr-author-graph",
                "graphs/pr-author.yaml",
            ]),
        )
        .expect("the line splits");
        assert_eq!(
            taken,
            Split {
                forwarded: words(&[
                    "feature",
                    "--title",
                    "--no-draft",
                    "--repo=service",
                    "--policy",
                    "change-open"
                ]),
                graph: Some("graphs/pr-author.yaml".into()),
                drafting: Drafting::Declined,
            }
        );
        // After `--` nothing is this crate's.
        let after = split(
            Verb::Recover,
            words(&["--pr-author-graph=g.yaml", "--", "--no-draft"]),
        )
        .expect("the line splits");
        assert_eq!(after.forwarded, words(&["--", "--no-draft"]));
        assert_eq!(after.graph, Some("g.yaml".into()));
        assert_eq!(after.drafting, Drafting::WhereWanted);
        assert!(split(Verb::Recover, words(&["b", "--pr-author-graph"])).is_err());
    }

    /// The set of options that carry a value is `onevcs`'s, read off its parser.
    #[test]
    fn the_valued_options_are_read_off_the_siblings_own_parser() {
        let valued = valued_options(Verb::PublishBranch);
        for option in ["--repo", "--title", "--policy", "--body", "--body-file"] {
            assert!(
                valued.iter().any(|known| known == option),
                "{option}: {valued:?}"
            );
        }
        let valued = valued_options(Verb::Recover);
        assert!(
            !valued.iter().any(|known| known == "--policy"),
            "{valued:?}"
        );
    }

    /// A death reads as the producer classified it, and a truncated detail says so.
    ///
    /// Every payload here is the sibling's own `MemberDied`, serialised, so the field
    /// names [`death`] reads by are held to the type `oneagentgraph` publishes deaths
    /// through: a field renamed there fails this rather than reading as a death that
    /// named nothing.
    #[test]
    fn a_member_death_is_said_in_oneagentgraphs_own_classification() {
        let envelope = |died: oneagentgraph::event::MemberDied| -> Envelope {
            serde_json::from_value(serde_json::json!({
                "v": 1, "ts": "2026-01-01T00:00:00Z", "stream": "s", "seq": 1,
                "source": "agentgraph",
                "kind": oneagentgraph::event::EventKind::MemberDied.as_str(),
                "labels": {"member": "author"},
                "payload": serde_json::to_value(died).expect("a death serialises"),
            }))
            .expect("an envelope")
        };
        let died = |rule: oneagentgraph::member::Rule,
                    cause: oneagentgraph::event::Cause,
                    detail: &str,
                    truncated: bool| oneagentgraph::event::MemberDied {
            rule: rule.as_str().to_owned(),
            cause,
            detail: detail.to_owned(),
            truncated,
            exit_code: None,
            disposition: None,
            stderr_tail: None,
            candidates: Vec::new(),
        };
        assert_eq!(
            death(&envelope(died(
                oneagentgraph::member::Rule::Unstartable,
                oneagentgraph::event::Cause::Spawn,
                "no such file",
                false
            ))),
            Some("author died: rule=unstartable cause=spawn detail=no such file".to_owned())
        );
        assert_eq!(
            death(&envelope(died(
                oneagentgraph::member::Rule::ProviderFailure,
                oneagentgraph::event::Cause::Unclassified,
                "cut",
                true
            ))),
            Some(
                "author died: rule=provider-failure cause=unclassified detail=cut \
                 (oneagentgraph truncated this detail)"
                    .to_owned()
            )
        );
        let mut bare = envelope(died(
            oneagentgraph::member::Rule::Unstartable,
            oneagentgraph::event::Cause::Spawn,
            "",
            false,
        ));
        bare.payload.clear();
        assert_eq!(
            death(&bare),
            Some("author died, and oneagentgraph named no rule, cause or detail for it".to_owned())
        );
    }
}
