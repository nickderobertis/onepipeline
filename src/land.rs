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
use crate::event::{Envelope, Labels, PipelineKind};
use crate::executor::{CancellationToken, DispatchRequest, LocalExecutor, WorkspaceSpec};
use crate::flow::{Flow, Landing as Registered, LandingOperation, LandingSubject};
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
    /// `onevcs integrate`, which `onepipeline integrate` fronts.
    Integrate,
}

impl Verb {
    /// The verb's name on `onevcs`'s own command line.
    fn onevcs(self) -> &'static str {
        match self {
            Self::PublishBranch => "publish-branch",
            Self::Recover => "recover",
            Self::Integrate => "integrate",
        }
    }

    /// The engine verb a supervised landing records it as.
    const fn operation(self) -> LandingOperation {
        match self {
            Self::PublishBranch => LandingOperation::PublishBranch,
            Self::Recover => LandingOperation::RepoRecover,
            Self::Integrate => LandingOperation::Integrate,
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

/// Land through the linked `onevcs` verb — drafting a branch's body first where
/// one is wanted — and answer with that verb's exit status.
///
/// Where `ONEPIPELINE_LAUNCHER_SESSION` names a manager session, the landing is
/// that session's supervised work: every subject is resolved and its ownership
/// registered with a flow before anything is drafted or forwarded, and a
/// registration that fails starts no work. With no session it runs unowned, as
/// it always did, and says so.
pub(crate) fn land(verb: Verb, args: Vec<OsString>) -> Result<i32> {
    let split = match verb {
        // Every argument of `integrate` is the sibling's: nothing to take out.
        Verb::Integrate => Split {
            forwarded: args,
            ..Split::default()
        },
        Verb::PublishBranch | Verb::Recover => split(verb, args)?,
    };
    let argv: Vec<OsString> = [OsString::from("onevcs"), OsString::from(verb.onevcs())]
        .into_iter()
        .chain(split.forwarded.iter().cloned())
        .collect();
    // `onevcs`'s own parser, so what may be passed is its list and a refused
    // argument is refused in its words — exactly as its own binary would.
    let mut cli = match onevcs::cli::Cli::try_parse_from(&argv) {
        Ok(cli) => cli,
        Err(refused) => {
            // llmlint: ignore[cli_output_contract] this is `onevcs`'s own usage
            // refusal or help, printed to the stream clap chooses, at the code it
            // chooses — which is what the passthrough owes a caller.
            let _ = refused.print();
            return Ok(refused.exit_code());
        }
    };
    let root = crate::ledger::runs_root();
    let Some(session) = launcher_session() else {
        eprintln!(
            "onepipeline: {}: no manager session owns this landing — {} names none — so it runs \
             unowned, and nothing will hold a session for it",
            verb.operation().as_str(),
            crate::sys::LAUNCHER_SESSION_ENV
        );
        return forward(verb, &mut cli, &split, &root, None);
    };
    // Every subject, resolved before anything is registered, drafted or
    // forwarded. One the sibling cannot resolve is its own refusal, in its words
    // and at its code, exactly as the verb would have refused it.
    let subjects = match subjects_of(&cli.command) {
        Ok(subjects) => subjects,
        Err(refused) => {
            eprintln!("onevcs: {refused}");
            return Ok(i32::from(onevcs::FailureKind::of(&refused).exit_code()));
        }
    };
    let program: Vec<String> = std::iter::once("onepipeline".to_owned())
        .chain(std::iter::once(verb.operation().as_str().to_owned()))
        .chain(
            split
                .forwarded
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned()),
        )
        .collect();
    let mut supervision =
        Supervision::register(&root, &session, verb.operation(), subjects, &program)?;
    let forwarded = forward(verb, &mut cli, &split, &root, Some(&mut supervision));
    supervision.settle(forwarded)
}

/// The manager session `ONEPIPELINE_LAUNCHER_SESSION` names, where it names one.
fn launcher_session() -> Option<String> {
    std::env::var(crate::sys::LAUNCHER_SESSION_ENV)
        .ok()
        .filter(|session| !session.trim().is_empty())
}

/// Every branch a parsed landing can advance, in the identity it belongs to:
/// the one branch of a branch-keyed verb, or every member of an integration
/// train, in the identity of the registered repository the current directory is
/// in — the identity `onevcs integrate` itself answers for.
fn subjects_of(command: &onevcs::cli::Command) -> onevcs::Result<Vec<LandingSubject>> {
    let providers = onevcs::Providers::real();
    let (repo, branches) = match command {
        onevcs::cli::Command::PublishBranch(args) => (args.repo.clone(), vec![args.branch.clone()]),
        onevcs::cli::Command::Recover(args) => (args.repo.clone(), vec![args.branch.clone()]),
        onevcs::cli::Command::Integrate(args) => (
            std::env::current_dir().map_err(|error| onevcs::Error::Invalid {
                reason: format!("cannot read the current directory: {error}"),
            })?,
            args.branches.clone(),
        ),
        // llmlint: ignore[changed_behavior_has_e2e] unreachable: the command line was parsed
        // with one of the three verbs' names as its subcommand, so the sibling's parser
        // answers that variant or refuses.
        _ => return Ok(Vec::new()),
    };
    let identity = onevcs::resolve_repository(&providers, &repo.to_string_lossy())?.identity;
    Ok(branches
        .into_iter()
        .map(|branch| LandingSubject {
            identity: identity.clone(),
            branch,
        })
        .collect())
}

/// Draft, entitle and forward a parsed landing, answering the verb's status.
fn forward(
    verb: Verb,
    cli: &mut onevcs::cli::Cli,
    split: &Split,
    root: &Path,
    mut supervision: Option<&mut Supervision>,
) -> Result<i32> {
    if verb == Verb::Integrate {
        if let onevcs::cli::Command::Integrate(args) = &cli.command {
            let request = onevcs::IntegrateRequest {
                branches: args.branches.clone(),
                push: match args.push {
                    true => onevcs::BasePush::Push,
                    false => onevcs::BasePush::Keep,
                },
                term_scope: args.term_scope.scope(),
            };
            return Ok(match supervision {
                Some(supervision) => {
                    let identity = supervision.identity();
                    report_integration(supervision.operate(
                        onevcs::DirectOperation::Integrate {
                            repository: identity,
                        },
                        |handle| onevcs::integrate_in_operation(&request, handle),
                    )?)
                }
                None => report_integration(onevcs::integrate(&request)),
            });
        }
    }
    let mut drafting = |branch: &str| -> Result<()> {
        match supervision.as_deref_mut() {
            Some(supervision) => supervision.transition(
                PipelineKind::LandingDrafting,
                format!("drafting the change request's body for {branch}"),
            ),
            None => Ok(()),
        }
    };
    if let Some(body) = drafted_for(verb, cli, &split.graph, split.drafting, &mut drafting)? {
        match &mut cli.command {
            onevcs::cli::Command::PublishBranch(args) => args.body = Some(body),
            onevcs::cli::Command::Recover(args) => args.body = Some(body),
            _ => {}
        }
    }
    let source = entitle(&mut cli.command, root);
    if let Some(request) = Landing::of(&cli.command) {
        eprintln!("{}", source.line());
        return Ok(match supervision {
            Some(supervision) => {
                let selector = request.selector(supervision.identity());
                report(supervision.operate(selector, |handle| request.land_in(handle))?)
            }
            None => report(request.land()),
        });
    }
    // A command line the verb would refuse as it stands: forwarded to it, so it is
    // refused in its own words, with no operation prepared for work that never
    // starts.
    if let Some(supervision) = supervision {
        supervision.transition(
            PipelineKind::LandingStarted,
            format!("forwarding `onevcs {}`", verb.onevcs()),
        )?;
    }
    Ok(i32::from(onevcs::run(cli)))
}

/// Render what an integration train answered, exactly as `onevcs integrate`
/// does, and the code it exits with.
fn report_integration(integrated: onevcs::Result<onevcs::Integration>) -> i32 {
    match integrated {
        Ok(outcome) => {
            let said = |yes: bool| if yes { "yes" } else { "no" };
            let mut rendered = format!("Integration train for {}:\n", outcome.base);
            for branch in &outcome.branches {
                rendered.push_str(&format!(
                    "  {}: {}\n",
                    branch.branch,
                    branch.status.describe()
                ));
            }
            rendered.push_str(&format!(
                "Base advanced: {}\nPushed: {}\n",
                said(outcome.ending.advanced()),
                said(outcome.ending.pushed())
            ));
            // A reader that closed the pipe early is told on standard error; the
            // train's own status is still what the landing settles with.
            use std::io::Write;
            let mut out = std::io::stdout().lock();
            if let Err(error) = out
                .write_all(rendered.as_bytes())
                .and_then(|()| out.flush())
            {
                eprintln!("onepipeline: integrate could not write its report: {error}");
            }
            0
        }
        Err(error) => {
            eprintln!("onevcs: {error}");
            i32::from(onevcs::FailureKind::of(&error).exit_code())
        }
    }
}

/// Where the subject a landing publishes under came from.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SubjectSource {
    /// The run node that owns the branch, by its title.
    Owner {
        /// `<run>#<node>`.
        node: String,
        title: onevcs::Subject,
    },
    /// The caller's own `--title`.
    Title(String),
    /// `onevcs`'s default: no node, or more than one, owns the branch.
    Unowned,
    /// `onevcs`'s default: a run's journal could not be read, so no owner found
    /// elsewhere is known to be the only one.
    Unread(String),
    /// `onevcs`'s default: the owning node, `<run>#<node>`, states no title.
    Untitled(String),
    /// `onevcs`'s default: the owning node's title is not one `onevcs` will
    /// commit under, with what `onevcs` said of it.
    Refused {
        /// `<run>#<node>`.
        node: String,
        /// `onevcs`'s own reason.
        why: String,
    },
}

impl SubjectSource {
    /// The line said on stderr before the landing publishes, so the permanent
    /// headline's source is read before it lands rather than after.
    ///
    /// The default names no subject: which of the branch's commits supplies it is
    /// `onevcs`'s ranking, which this crate does not repeat, and a guess that
    /// differed from its pick would misstate the very headline this line exists
    /// to show.
    fn line(&self) -> String {
        match self {
            Self::Owner { node, title } => {
                format!("publishing under '{}' (from {node})", &**title)
            }
            Self::Title(title) => format!("publishing under '{title}' (--title)"),
            Self::Unowned => {
                "publishing under onevcs's default subject (no unique owning node)".to_owned()
            }
            Self::Unread(why) => format!(
                "publishing under onevcs's default subject (no owning node is known to be \
                 unique: {why})"
            ),
            Self::Untitled(node) => {
                format!("publishing under onevcs's default subject ({node} states no title)")
            }
            Self::Refused { node, why } => format!(
                "publishing under onevcs's default subject ({node}'s title is not a subject \
                 onevcs will commit under: {why})"
            ),
        }
    }
}

/// Settle the subject a branch-keyed landing publishes under, writing the owning
/// node's title into the command where the caller gave none, and answer where it
/// came from. A command that lands no branch is left as it is.
fn entitle(command: &mut onevcs::cli::Command, runs: &Path) -> SubjectSource {
    let (branch, title) = match command {
        onevcs::cli::Command::PublishBranch(args) => (&args.branch, &mut args.title),
        onevcs::cli::Command::Recover(args) => (&args.branch, &mut args.title),
        _ => return SubjectSource::Unowned,
    };
    if let Some(title) = title {
        return SubjectSource::Title(title.clone());
    }
    let owner = match owner_of(runs, branch) {
        Ownership::One(owner) => owner,
        Ownership::NotOne => return SubjectSource::Unowned,
        Ownership::Unread(why) => return SubjectSource::Unread(why),
    };
    let node = format!("{}#{}", owner.run, owner.node);
    let Some(owned) = owner.title else {
        return SubjectSource::Untitled(node);
    };
    match onevcs::Subject::try_from(owned) {
        Ok(subject) => {
            *title = Some(String::from(subject.clone()));
            SubjectSource::Owner {
                node,
                title: subject,
            }
        }
        Err(why) => SubjectSource::Refused { node, why },
    }
}

/// The one run node whose work a branch is.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Owner {
    run: String,
    node: String,
    /// Its title as its task record states it, where it states one.
    title: Option<String>,
}

/// What the runs root says owns a branch.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ownership {
    /// Exactly one node, or one retry lineage, in one run.
    One(Owner),
    /// None, or more than one.
    NotOne,
    /// A run's journal could not be read, and why: it may name the branch too.
    Unread(String),
}

/// Which run node under the runs root owns `branch`.
///
/// Read off what each run's journal already records: every session a node's
/// dispatch opened on the branch — every one, not only the current dispatch's —
/// and every node the plan pins to it, which is how a retry carries the branch
/// from one attempt to its replacement. A node dispatched more than once onto
/// the branch is one owner, and so is a retry lineage, owned by its most recent
/// node because a retry can carry a corrected title. Two lineages, or two runs,
/// naming it own nothing.
fn owner_of(runs: &Path, branch: &str) -> Ownership {
    // The branch as its journal spells it inside a JSON string, so a run that
    // never names it is passed over without folding.
    let spelled = serde_json::to_string(branch).unwrap_or_default();
    let spelled = spelled.trim_matches('"');
    let index = crate::ledger::all_runs(runs);
    // A run root the index refuses is passed over only where it holds no journal
    // naming the branch: one that does may be another owner this build cannot read.
    for skipped in &index.skipped {
        let journal = skipped.path.join("events.jsonl");
        match journal_naming(&journal, spelled) {
            Ok(false) => {}
            Ok(true) => {
                return Ownership::Unread(format!(
                    "{} records the branch and is not a run this build reads: {}",
                    skipped.path.display(),
                    skipped.reason
                ))
            }
            Err(why) => return Ownership::Unread(why),
        }
    }
    let mut owners = Vec::new();
    for paths in index.runs {
        let journal = paths.journal();
        match journal_naming(&journal, spelled) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(why) => return Ownership::Unread(why),
        }
        // A line this build cannot read may be the session, pin or retry that
        // makes the owner not the only one, so a partial read answers nothing.
        if crate::journal::has_unreadable_lines(&journal) {
            return Ownership::Unread(format!(
                "{} holds a line this build cannot read",
                journal.display()
            ));
        }
        let events = crate::journal::read(&journal);
        let state = crate::projection::fold(&events);
        let opened = events
            .iter()
            .filter(|event| {
                event.source == crate::event::Source::Vcs
                    && crate::vcs::is_session_opened(&event.kind)
            })
            .filter_map(|event| {
                let node = event.labels.node.as_deref()?;
                let session = crate::vcs::DispatchSession::read_from(event)?;
                (session.branch().as_str() == branch).then_some(node)
            });
        let pinned = state
            .graph
            .iter()
            .filter(|node| node.branch.as_deref() == Some(branch))
            .map(|node| node.id.as_str());
        let mut heads: Vec<&str> = opened
            .chain(pinned)
            .map(|node| lineage_head(&state.superseded, node))
            .collect();
        heads.sort_unstable();
        heads.dedup();
        owners.extend(heads.into_iter().map(|node| Owner {
            run: paths.run.clone(),
            node: node.to_owned(),
            title: state.graph.get(node).and_then(|node| node.title.clone()),
        }));
    }
    match <[Owner; 1]>::try_from(owners) {
        Ok([owner]) => Ownership::One(owner),
        Err(_) => Ownership::NotOne,
    }
}

/// Whether a journal names `spelled` — `false` where it does not exist yet, as a
/// run's does not between its launch record and its first event — or why it
/// cannot be read.
fn journal_naming(journal: &Path, spelled: &str) -> std::result::Result<bool, String> {
    match std::fs::read_to_string(journal) {
        Ok(text) => Ok(text.contains(spelled)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("{} cannot be read: {error}", journal.display())),
    }
}

/// The latest replacement a node's retries reached, or the node itself where it
/// was never retried. A journal a person edited can hold a cycle; it is walked
/// once whatever it holds.
fn lineage_head<'a>(
    superseded: &'a std::collections::BTreeMap<String, String>,
    node: &'a str,
) -> &'a str {
    let mut at = node;
    let mut seen = vec![node];
    while let Some(replacement) = superseded.get(at) {
        // llmlint: ignore[changed_behavior_has_e2e] a cycle is reachable only from a journal
        // a person edited — `retry` refuses an id the graph already holds, so no verb writes
        // one — and the unit test below holds it; the lineage a verb does write is driven end
        // to end by `a_retried_nodes_branch_lands_under_its_latest_replacements_title`.
        if seen.contains(&replacement.as_str()) {
            break;
        }
        at = replacement;
        seen.push(at);
    }
    at
}

/// One branch-keyed landing, in the library form of the verb it fronts.
///
/// Called rather than handed to `onevcs::run`, so what the landing **answered** is a
/// value this crate renders — the draft lifecycle's green-and-kept-for-review ending
/// is reported under this crate's own word for it. Only a command line the verb
/// would take as it stands is built here; every other one — a title the sibling
/// will not commit under, a body named twice, a body file that cannot be read — is
/// left to `onevcs::run`, which refuses it in its own words.
enum Landing {
    /// A completed branch no session holds, which `onevcs` refuses where it
    /// carries an unattested incomplete-step marker.
    PublishBranch(onevcs::BranchPublishRequest),
    /// A branch a step left behind, attested as it lands.
    Recover(onevcs::RecoverRequest),
}

impl Landing {
    fn of(command: &onevcs::cli::Command) -> Option<Self> {
        let title = |title: &Option<String>| -> Option<Option<onevcs::Subject>> {
            match title {
                None => Some(None),
                Some(title) => onevcs::Subject::try_from(title.clone()).ok().map(Some),
            }
        };
        let body = |body: &Option<String>, file: &Option<PathBuf>| -> Option<Option<String>> {
            match (body, file) {
                (Some(_), Some(_)) => None,
                (Some(body), None) => Some(Some(body.clone())),
                (None, Some(path)) => std::fs::read_to_string(path).ok().map(Some),
                (None, None) => Some(None),
            }
        };
        match command {
            onevcs::cli::Command::PublishBranch(args) => {
                Some(Self::PublishBranch(onevcs::BranchPublishRequest {
                    repo: args.repo.clone(),
                    branch: args.branch.clone(),
                    title: title(&args.title)?,
                    body: body(&args.body, &args.body_file)?,
                    policy: args.policy,
                    term_scope: args.term_scope.scope(),
                }))
            }
            onevcs::cli::Command::Recover(args) => Some(Self::Recover(onevcs::RecoverRequest {
                repo: args.repo.clone(),
                branch: args.branch.clone(),
                title: title(&args.title)?,
                body: body(&args.body, &args.body_file)?,
                term_scope: args.term_scope.scope(),
            })),
            _ => None,
        }
    }

    fn land(&self) -> onevcs::Result<onevcs::PublishOutcome> {
        let providers = onevcs::Providers::real();
        match self {
            Self::PublishBranch(request) => onevcs::publish_branch(&providers, request),
            Self::Recover(request) => onevcs::recover(&providers, request),
        }
    }

    /// The same landing, into the one invocation `operation` reserved.
    fn land_in(
        &self,
        operation: &onevcs::OperationHandle,
    ) -> onevcs::Result<onevcs::PublishOutcome> {
        let providers = onevcs::Providers::real();
        match self {
            Self::PublishBranch(request) => {
                onevcs::publish_branch_in_operation(&providers, request, operation)
            }
            Self::Recover(request) => onevcs::recover_in_operation(&providers, request, operation),
        }
    }

    /// The invocation selector a handle for this landing is prepared against, in
    /// the identity its subjects were resolved to.
    fn selector(&self, repository: String) -> onevcs::DirectOperation {
        match self {
            Self::PublishBranch(request) => onevcs::DirectOperation::PublishBranch {
                repository,
                branch: request.branch.clone(),
            },
            Self::Recover(request) => onevcs::DirectOperation::Recover {
                repository,
                branch: request.branch.clone(),
            },
        }
    }
}

/// Render what a branch-keyed landing answered, and the exit code it answers with.
///
/// Every ending `onevcs` renders is rendered in its words and at its code, exactly
/// as `onevcs::run` would. The one this crate names is a change request whose checks
/// came back green and that is kept as a draft for its user's review: it leads with
/// [`crate::vcs::REVIEW_DRAFTED`] and the change request's URL, and exits as every
/// successful landing does.
fn report(landed: onevcs::Result<onevcs::PublishOutcome>) -> i32 {
    match landed {
        Ok(onevcs::PublishOutcome::ChangeReviewDraft(url)) => {
            println!(
                "{}: {url}: the required checks are green and the change request is kept as a \
                 draft for its user's review; lift it on the host when it is ready for the team",
                crate::vcs::REVIEW_DRAFTED
            );
            0
        }
        // A publication that ran and did not land is a failure however it arrived,
        // said on stderr at its kind's code — never a success on stdout.
        Ok(onevcs::PublishOutcome::Failed { kind, reason, .. }) => {
            eprintln!("onevcs: {reason}");
            i32::from(kind.exit_code())
        }
        Ok(outcome) => {
            println!("{}", outcome.describe());
            0
        }
        Err(error) => {
            eprintln!("onevcs: {error}");
            i32::from(onevcs::FailureKind::of(&error).exit_code())
        }
    }
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
    starting: &mut dyn FnMut(&str) -> Result<()>,
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
            verb.operation().as_str()
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
    // Said before the turn is paid for, so a manager watching the landing sees
    // the drafting start rather than only its result.
    starting(branch)?;
    let drafted = match checkout {
        Ok(checkout) => draft(&graph, &checkout, branch),
        Err(why) => Drafted::Undrafted(Undrafted::Dispatch(format!(
            "the drafting dispatch could not start: {why}"
        ))),
    };
    Ok(match drafted {
        Drafted::Body(body, _) => Some(body),
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

/// What a drafting turn has made and not yet removed, in the checkout it was made
/// in.
struct Leftover {
    checkout: PathBuf,
    made: Made,
}

/// How far a drafting turn has got, each step inside the one before: a worktree
/// is only ever cut into the turn's own directory, so what is removed is only
/// ever what was added.
enum Made {
    Nothing,
    Dir(PathBuf),
    Worktree { dir: PathBuf, worktree: PathBuf },
}

impl Leftover {
    /// Remove whatever is left, saying on standard error what could not be.
    fn remove(&mut self) {
        let dir = match std::mem::replace(&mut self.made, Made::Nothing) {
            Made::Nothing => return,
            Made::Dir(dir) => dir,
            Made::Worktree { dir, worktree } => {
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
                dir
            }
        };
        if let Err(error) = std::fs::remove_dir_all(&dir) {
            eprintln!(
                "onepipeline: the drafting turn's directory {} could not be removed: \
                 {error}; remove it by hand",
                dir.display()
            );
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
            made: Made::Nothing,
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
            let mut left = lock_leftover(&leftover);
            match std::fs::create_dir(&dir) {
                Ok(()) => {
                    left.made = Made::Dir(dir.clone());
                    drop(left);
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
        let mut left = lock_leftover(&self.left);
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
        left.made = Made::Worktree {
            dir: self.dir.clone(),
            worktree: worktree.clone(),
        };
        drop(left);
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

/// A manager landing this process registered and is doing: the flow that holds
/// it, its own record, and the journals its progress goes to.
pub(crate) struct Supervision {
    /// The flow that holds it.
    flow: Flow,
    /// Whose flow it is: one registered for this landing alone, which ends when
    /// the landing does, or a longer one it is a step of.
    holding: Holding,
    /// The landing, as registered.
    landing: Registered,
    /// Its journal and every associated run's, behind one lock the relay thread
    /// shares.
    relay: Arc<Mutex<Relay>>,
    /// Why progress could not be relayed, where it could not: a blocker the
    /// settlement reports rather than a landing that reads as fully supervised.
    relay_failed: Option<String>,
}

impl Supervision {
    /// Register a landing of `session` over `subjects` before any side effect.
    ///
    /// Reuses the live flow this process was started in where the same session
    /// owns it, refuses one another session owns, and otherwise registers a flow
    /// for this landing alone. The request is written whole and renamed into
    /// place under the lock a branch acknowledgement takes, so neither can fall
    /// between the other's check and write.
    ///
    /// # Errors
    ///
    /// Anything that could not be registered or recorded: the landing then does
    /// no work at all.
    fn register(
        root: &Path,
        session: &str,
        operation: LandingOperation,
        subjects: Vec<LandingSubject>,
        program: &[String],
    ) -> Result<Self> {
        let refused = |why: String| {
            Error::Refused(format!(
                "the {} landing could not be registered with its manager session, so nothing \
                 was drafted or forwarded: {why}",
                operation.as_str()
            ))
        };
        let runs = associated_runs(root, &subjects).map_err(refused)?;
        let lock = crate::flow::registration_lock(root).map_err(|e| refused(e.to_string()))?;
        // The flow this process runs in, where `ONEPIPELINE_FLOW` names one of
        // this runs root: one that is there and cannot be read is refused rather
        // than passed over, because a landing registered beside it could be the
        // second owner of work that flow already supervises. A name with no flow
        // under this root — another root's, or one long gone — names none.
        let inherited = match crate::flow::inherited() {
            Some(id) if crate::flow::paths(root, id.as_str()).dir.exists() => {
                Some(Flow::open(root, id.as_str()).map_err(|e| refused(e.to_string()))?)
            }
            _ => None,
        };
        let (flow, holding) = match inherited.filter(Flow::is_live) {
            Some(parent) if parent.record.session == session => (parent, Holding::ParentFlow),
            Some(parent) => {
                return Err(refused(format!(
                    "this process runs in flow '{}', which belongs to session [{}], not to this \
                     session; a landing is supervised by the session that owns its flow",
                    parent.id(),
                    crate::sys::session_digest(&parent.record.session)
                )))
            }
            None => {
                let (first, rest) = program.split_first().map_or(
                    (OsString::from("onepipeline"), Vec::new()),
                    |(first, rest)| {
                        (
                            OsString::from(first),
                            rest.iter().map(OsString::from).collect(),
                        )
                    },
                );
                let flow =
                    crate::flow::register(root, operation.as_str(), session, &first, &rest, None)
                        .map_err(|e| refused(e.to_string()))?;
                (flow, Holding::OwnFlow)
            }
        };
        let landing_id = format!("landing-{}", crate::watchers::nonce());
        let pid = crate::sys::pid();
        let request = crate::flow::LandingRequest {
            schema_version: crate::flow::FLOW_SCHEMA_VERSION,
            landing_id: landing_id.clone(),
            flow_id: flow.id().to_owned(),
            session: session.to_owned(),
            operation,
            subjects,
            runs,
            pid: std::num::NonZeroU32::new(pid).unwrap_or(std::num::NonZeroU32::MIN),
            host: crate::sys::hostname(),
            started: crate::sys::process_start_token(pid)
                .map(|token| token.recorded().to_string())
                .unwrap_or_default(),
            began_at: crate::sys::now_rfc3339(),
        };
        let landings = flow.paths.dir.join(crate::flow::LANDINGS_DIR);
        let staged = landings.join(format!(".{landing_id}"));
        let dir = landings.join(&landing_id);
        let placed = std::fs::create_dir_all(&staged)
            .map_err(|e| format!("{}: {e}", staged.display()))
            .and_then(|()| {
                crate::ledger::write_json(&staged.join("request.json"), &request)
                    .map_err(|e| e.to_string())
            })
            .and_then(|()| {
                std::fs::rename(&staged, &dir).map_err(|e| format!("{}: {e}", dir.display()))
            });
        if let Err(why) = placed {
            let _ = std::fs::remove_dir_all(&staged);
            if holding == Holding::OwnFlow {
                crate::flow::record_ending(&flow, SUPERVISION_FAILED);
            }
            return Err(refused(why));
        }
        drop(lock);
        let landing = Registered { dir, request };
        let relay = match Relay::open(&flow, &landing, root) {
            Ok(relay) => relay,
            Err(error) => {
                if holding == Holding::OwnFlow {
                    crate::flow::record_ending(&flow, SUPERVISION_FAILED);
                }
                return Err(refused(error.to_string()));
            }
        };
        let mut supervision = Self {
            flow,
            holding,
            landing,
            relay: Arc::new(Mutex::new(relay)),
            relay_failed: None,
        };
        // llmlint: ignore-block[changed_behavior_has_e2e] the first append is to a file inside
        // the directory this process created and renamed into place an instant before, so no
        // journey can stage a write refused there without a failpoint this crate does not
        // ship; what a refusal does is the same `refused` every placement failure above takes,
        // which `a_registration_that_cannot_be_kept_starts_no_work` drives, and the ending it
        // records is the one `a_landing_whose_journal_or_ending_cannot_be_written_fails_closed`
        // reads back as failed.
        if let Err(error) = supervision.transition(
            PipelineKind::LandingRegistered,
            format!(
                "registered for session [{}]",
                crate::sys::session_digest(session)
            ),
        ) {
            if supervision.holding == Holding::OwnFlow {
                crate::flow::record_ending(&supervision.flow, SUPERVISION_FAILED);
            }
            let _ = supervision.record_ending(SUPERVISION_FAILED);
            return Err(refused(error.to_string()));
        }
        // llmlint: ignore-end[changed_behavior_has_e2e]
        if supervision.holding == Holding::OwnFlow {
            eprintln!(
                "flow {id}: watch it with: onepipeline watch --flow {id}",
                id = supervision.flow.id()
            );
        }
        eprintln!(
            "landing {} of flow {}: {} registered for this session",
            supervision.landing.id(),
            supervision.flow.id(),
            operation.as_str()
        );
        Ok(supervision)
    }

    /// The identity every subject was resolved to: one per landing.
    fn identity(&self) -> String {
        self.landing
            .request
            .subjects
            .first()
            .map(|subject| subject.identity.clone())
            .unwrap_or_default()
    }

    /// Record one transition of this landing in its journal and every
    /// associated run's.
    fn transition(&mut self, kind: PipelineKind, message: String) -> Result<()> {
        let payload = crate::flow::LandingTransition {
            landing_id: self.landing.id().to_owned(),
            flow_id: self.flow.id().to_owned(),
            operation: self.landing.request.operation,
            subjects: self.landing.request.subjects.clone(),
            message,
        };
        lock_relay(&self.relay).transition(kind, crate::journal::object(&payload)?)
    }

    /// Prepare the `onevcs` operation `selector` names, persist its id, and make
    /// `call` into it while its events are relayed as they are recorded.
    ///
    /// # Errors
    ///
    /// A handle `onevcs` would not prepare, or an id that could not be
    /// persisted: the landing then forwards nothing rather than doing work no
    /// reader could reopen.
    fn operate<T: Send>(
        &mut self,
        selector: onevcs::DirectOperation,
        call: impl FnOnce(&onevcs::OperationHandle) -> T,
    ) -> Result<T> {
        let handle = onevcs::OperationHandle::prepare(&selector).map_err(|error| {
            Error::Refused(format!(
                "onevcs would not prepare the landing's operation, so nothing was forwarded: \
                 {error}"
            ))
        })?;
        crate::ledger::write_json(
            &self.landing.dir.join("operation.json"),
            &crate::flow::LandingOperationRecord {
                schema_version: crate::flow::FLOW_SCHEMA_VERSION,
                operation_id: handle.id().to_owned(),
            },
        )?;
        lock_relay(&self.relay).operation_id = Some(handle.id().to_owned());
        self.transition(
            PipelineKind::LandingStarted,
            format!(
                "forwarding `onevcs {}` as operation {}",
                match self.landing.request.operation {
                    LandingOperation::PublishBranch => "publish-branch",
                    LandingOperation::RepoRecover => "recover",
                    LandingOperation::Integrate => "integrate",
                },
                handle.id()
            ),
        )?;
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let relayed = std::thread::scope(|scope| {
            let pump = {
                let handle = handle.clone();
                let relay = Arc::clone(&self.relay);
                let stop = Arc::clone(&stop);
                scope.spawn(move || pump(&handle, &relay, &stop))
            };
            let out = call(&handle);
            stop.store(true, std::sync::atomic::Ordering::SeqCst);
            let relayed = pump
                .join()
                .unwrap_or_else(|_| Err("the relay thread panicked".to_owned()));
            (out, relayed)
        });
        let (out, relayed) = relayed;
        if let Err(why) = relayed {
            eprintln!(
                "onepipeline: landing {}: its operation's progress could not be relayed: {why}",
                self.landing.id()
            );
            self.relay_failed = Some(why);
        }
        Ok(out)
    }

    /// Record how the landing ended and end its flow where it holds one,
    /// answering what the forwarded verb answered.
    fn settle(mut self, forwarded: Result<i32>) -> Result<i32> {
        let status = match &forwarded {
            Ok(status) => *status,
            Err(error) => error.exit_code(),
        };
        let ending_kept = self.record_ending(status);
        let refused = status != 0 || self.relay_failed.is_some() || !ending_kept;
        let message = match (&forwarded, &self.relay_failed) {
            (Err(error), _) => format!("refused: {}", crate::views::one_line(&error.to_string())),
            (Ok(_), None) if !ending_kept => format!(
                "the forwarded verb ended with status {status}, and that ending could not be \
                 recorded, so the landing reads as died once its process is gone"
            ),
            (Ok(_), Some(why)) => format!(
                "its operation's progress could not be relayed, so it is not fully supervised: \
                 {why}"
            ),
            (Ok(0), None) => "the forwarded verb finished with status 0".to_owned(),
            (Ok(status), None) => format!("the forwarded verb ended with status {status}"),
        };
        let payload = crate::flow::LandingTerminal {
            transition: crate::flow::LandingTransition {
                landing_id: self.landing.id().to_owned(),
                flow_id: self.flow.id().to_owned(),
                operation: self.landing.request.operation,
                subjects: self.landing.request.subjects.clone(),
                message,
            },
            status,
        };
        let kind = if refused {
            PipelineKind::LandingRefused
        } else {
            PipelineKind::LandingFinished
        };
        let recorded = crate::journal::object(&payload)
            .and_then(|payload| lock_relay(&self.relay).transition(kind, payload));
        if let Err(error) = &recorded {
            eprintln!(
                "onepipeline: landing {}: its ending could not be journalled: {error}",
                self.landing.id()
            );
        }
        if self.holding == Holding::OwnFlow {
            // Fail closed: a landing whose progress or ending could not be kept
            // leaves a flow owed closure even where the verb itself succeeded.
            let flow_status = if status == 0 && (refused || recorded.is_err()) {
                SUPERVISION_FAILED
            } else {
                status
            };
            crate::flow::record_ending(&self.flow, flow_status);
        }
        forwarded
    }

    /// Write `ending.json`, saying on standard error where it could not be, and
    /// answer whether it was kept: an unrecorded ending reads as a death once
    /// this process is gone, which is owed closure — never a success nobody
    /// recorded.
    fn record_ending(&mut self, status: i32) -> bool {
        let ending = crate::flow::LandingEnding {
            schema_version: crate::flow::FLOW_SCHEMA_VERSION,
            landing_id: self.landing.id().to_owned(),
            status,
            at: crate::sys::now_rfc3339(),
        };
        match crate::ledger::write_json(&self.landing.dir.join("ending.json"), &ending) {
            Ok(()) => true,
            Err(error) => {
                eprintln!(
                    "onepipeline: landing {}: its ending (status {status}) could not be recorded, \
                     so it will read as died: {error}",
                    self.landing.id()
                );
                false
            }
        }
    }
}

/// Whether a landing's process holds the flow it is supervised in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Holding {
    /// The flow was registered for this landing alone: its holder is this
    /// process, and its ending is the landing's.
    OwnFlow,
    /// The landing is one step of a longer flow of the same session, which goes
    /// on when the landing ends.
    ParentFlow,
}

/// The status a landing's flow ends with where its supervision failed — its
/// registration, its relay, or the record of its ending — though the verb did
/// not: non-zero, so the flow is owed closure rather than read as a clean
/// success.
const SUPERVISION_FAILED: i32 = 1;

/// The relay behind its lock, whoever panicked holding it.
fn lock_relay(relay: &Mutex<Relay>) -> std::sync::MutexGuard<'_, Relay> {
    relay
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Read the operation's events as `onevcs` records them, relaying each, until
/// `stop` is set and a read after it finds nothing more — so every complete
/// record the call wrote is relayed before the landing settles.
fn pump(
    handle: &onevcs::OperationHandle,
    relay: &Mutex<Relay>,
    stop: &std::sync::atomic::AtomicBool,
) -> std::result::Result<(), String> {
    let mut reader = onevcs::OperationEvents::open(handle).map_err(|e| e.to_string())?;
    loop {
        let stopping = stop.load(std::sync::atomic::Ordering::SeqCst);
        let batch = reader.read().map_err(|e| e.to_string())?;
        let drained = batch.is_empty();
        for envelope in batch {
            lock_relay(relay)
                .vcs(&envelope)
                .map_err(|e| e.to_string())?;
        }
        if stopping && drained {
            return Ok(());
        }
        if drained {
            std::thread::sleep(RELAY_POLL);
        }
    }
}

/// How often the relay asks the operation's stream for what was appended.
const RELAY_POLL: std::time::Duration = std::time::Duration::from_millis(20);

/// Where one landing's progress is written: its own journal and every
/// associated run's, each appended through the store's locked writer and never
/// twice for one record of the operation.
// llmlint: ignore-block[invalid_states_unrepresentable] the ids are the labels every relayed record carries, as JSON strings on the wire, and each was checked where it was read: the landing and flow ids against the directories their request sits in, the run ids by `LandingRequest`'s own run-id rule, and the operation id by `onevcs::OperationHandle::open`, which refuses one it cannot open. A newtype per id would be a conversion back to the same string at the one place each is used.
pub(crate) struct Relay {
    landing_id: String,
    flow_id: String,
    /// The operation the landing prepared, once it has.
    operation_id: Option<String>,
    journal: LandingJournal,
    runs: Vec<(String, crate::journal::Journal, Seen)>,
    seen: Seen,
}

// llmlint: ignore-end[invalid_states_unrepresentable]

/// The `(stream, seq)` of every operation record a journal already holds.
type Seen = std::collections::HashSet<(String, u64)>;

impl Relay {
    /// The journals of `landing` and of every run it names.
    ///
    /// # Errors
    ///
    /// An operation record that is there and cannot be read: relaying as though
    /// the landing were legacy coarse-only work would drop the evidence its
    /// operation recorded.
    fn open(flow: &Flow, landing: &Registered, root: &Path) -> Result<Self> {
        let operation_id = landing.operation_id().map_err(Error::Invalid)?;
        let runs = landing
            .request
            .runs
            .iter()
            .map(|run| {
                let paths = RunPaths::under(root, run);
                let seen = seen_in(&paths.journal(), operation_id.as_deref());
                (run.clone(), crate::journal::Journal::open(&paths), seen)
            })
            .collect();
        Ok(Self {
            landing_id: landing.id().to_owned(),
            flow_id: flow.id().to_owned(),
            seen: seen_in(&landing.journal(), operation_id.as_deref()),
            operation_id,
            journal: LandingJournal::open(landing.journal()),
            runs,
        })
    }

    /// Record one landing transition, stamped with its significance.
    fn transition(
        &mut self,
        kind: PipelineKind,
        payload: serde_json::Map<String, serde_json::Value>,
    ) -> Result<()> {
        let labels = landing_labels(
            &self.landing_id,
            &self.flow_id,
            self.operation_id.as_deref(),
            crate::flow::landing_significance(kind),
        );
        self.journal.emit(kind, labels.clone(), payload.clone())?;
        for (run, journal, _) in &mut self.runs {
            let mut labels = labels.clone();
            labels.run_id = Some(run.clone());
            journal.emit(kind, labels, payload.clone())?;
        }
        Ok(())
    }

    /// Relay one `onevcs` operation record, as its producer wrote it plus this
    /// landing's labels, into every journal that does not already hold it.
    fn vcs(&mut self, envelope: &onevcs::Envelope) -> Result<()> {
        let mut value = serde_json::to_value(envelope)
            .map_err(|e| Error::Invalid(format!("an operation event did not serialize: {e}")))?;
        let key = (envelope.stream.clone(), envelope.seq);
        // The producer's own `operation_id` and `significance` are kept as it
        // stamped them; this landing's two labels are added beside them.
        let ours = landing_labels(&self.landing_id, &self.flow_id, None, None);
        if let Some(labels) = value.get_mut("labels").and_then(|l| l.as_object_mut()) {
            labels.extend(ours.extra);
        }
        let crossed: Envelope = serde_json::from_value(value).map_err(|e| {
            Error::Invalid(format!(
                "an operation event does not cross into this crate's envelope: {e}"
            ))
        })?;
        if self.seen.insert(key.clone()) {
            self.journal.append(&crossed)?;
        }
        for (run, journal, seen) in &mut self.runs {
            if seen.insert(key.clone()) {
                let mut copy = crossed.clone();
                copy.labels.run_id.get_or_insert_with(|| run.clone());
                journal.relay(&copy)?;
            }
        }
        Ok(())
    }
}

/// The labels a landing's records carry beside their own: the landing, its
/// flow, the operation it prepared once it has, and — on the engine's own
/// transitions — the significance it stamps. The one place their names are
/// spelled.
fn landing_labels(
    landing: &str,
    flow: &str,
    operation: Option<&str>,
    significance: Option<onevcs::Significance>,
) -> Labels {
    let mut labels = Labels::default();
    labels
        .extra
        .insert("landing_id".to_owned(), landing.to_owned().into());
    labels
        .extra
        .insert("flow_id".to_owned(), flow.to_owned().into());
    if let Some(operation) = operation {
        labels
            .extra
            .insert("operation_id".to_owned(), operation.to_owned().into());
    }
    if let Some(significance) = significance {
        labels.extra.insert(
            "significance".to_owned(),
            serde_json::to_value(significance).unwrap_or_default(),
        );
    }
    labels
}

/// The `(stream, seq)` of every record of `operation` a journal already holds.
fn seen_in(journal: &Path, operation: Option<&str>) -> Seen {
    let Some(operation) = operation else {
        return Seen::new();
    };
    crate::ledger::read_lines(journal)
        .iter()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|record| {
            record.get("source").and_then(serde_json::Value::as_str) == Some("vcs")
                && record
                    .pointer("/labels/operation_id")
                    .and_then(serde_json::Value::as_str)
                    == Some(operation)
        })
        .filter_map(|record| {
            Some((
                record.get("stream")?.as_str()?.to_owned(),
                record.get("seq")?.as_u64()?,
            ))
        })
        .collect()
}

/// A landing's own journal: an append-only store of envelopes, appended under
/// the store's lock, this process's own records on a stream of its own.
struct LandingJournal {
    path: PathBuf,
    stream: String,
    next_seq: u64,
}

impl LandingJournal {
    fn open(path: PathBuf) -> Self {
        let stream = format!("{}-{}", crate::sys::hostname(), crate::sys::pid());
        let next_seq = crate::ledger::read_lines(&path)
            .iter()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|value| {
                value.get("stream").and_then(serde_json::Value::as_str) == Some(stream.as_str())
            })
            .filter_map(|value| value.get("seq").and_then(serde_json::Value::as_u64))
            .max()
            .map_or(0, |max| max + 1);
        Self {
            path,
            stream,
            next_seq,
        }
    }

    fn emit(
        &mut self,
        kind: PipelineKind,
        labels: Labels,
        payload: serde_json::Map<String, serde_json::Value>,
    ) -> Result<()> {
        let envelope = Envelope {
            v: crate::event::ENVELOPE_VERSION,
            ts: crate::sys::now_rfc3339(),
            stream: self.stream.clone(),
            seq: self.next_seq,
            source: crate::event::Source::Pipeline,
            kind: kind.into(),
            dimensions: Default::default(),
            labels,
            payload,
            artifacts: Vec::new(),
        };
        self.next_seq += 1;
        self.append(&envelope)
    }

    fn append(&mut self, envelope: &Envelope) -> Result<()> {
        let line =
            serde_json::to_string(envelope).map_err(|e| Error::Invalid(format!("event: {e}")))?;
        crate::ledger::append_line_healed(&self.path, &line).map(|_| ())
    }
}

/// Every run under `root` whose own record opened a session on one of
/// `subjects` — the runs a landing of them came from. Each stays its own
/// manager's: the association is recorded on the landing, never on the run.
///
/// A run whose journal cannot be read is said on standard error and left
/// unassociated, as the subject derivation beside this leaves it unowned: the
/// landing itself is still supervised by its flow, and refusing it would cost
/// the manager the landing over a record nobody can repair from here.
fn associated_runs(
    root: &Path,
    subjects: &[LandingSubject],
) -> std::result::Result<Vec<String>, String> {
    let mut runs = Vec::new();
    for paths in crate::ledger::all_runs(root).runs {
        let journal = paths.journal();
        let named = subjects.iter().try_fold(false, |named, subject| {
            let spelled = serde_json::to_string(&subject.branch).unwrap_or_default();
            journal_naming(&journal, spelled.trim_matches('"')).map(|this| named || this)
        });
        let named = match named {
            Ok(named) => named,
            Err(why) => {
                eprintln!(
                    "onepipeline: run '{}' is not associated with this landing, because whether \
                     it names a subject cannot be said: {why}",
                    paths.run
                );
                false
            }
        };
        if !named {
            continue;
        }
        let opened = crate::journal::read(&journal).into_iter().any(|event| {
            event.source == crate::event::Source::Vcs
                && crate::vcs::is_session_opened(&event.kind)
                && crate::vcs::DispatchSession::read_from(&event).is_some_and(|session| {
                    let identity = event
                        .payload
                        .get("identity")
                        .or_else(|| event.labels.extra.get("identity"))
                        .and_then(serde_json::Value::as_str);
                    subjects.iter().any(|subject| {
                        session.branch().as_str() == subject.branch
                            && identity.is_none_or(|identity| identity == subject.identity)
                    })
                })
        });
        if opened {
            runs.push(paths.run.clone());
        }
    }
    runs.sort();
    runs.dedup();
    Ok(runs)
}

/// Write the death of a landing whose process is proved gone with no ending:
/// every complete operation record its handle still holds is relayed first,
/// so the evidence already recorded is kept, then one `landing-died` — once,
/// whoever proves it, never a completion.
///
/// # Errors
///
/// A journal that could not be read or written.
pub(crate) fn settle_death(root: &Path, flow: &Flow, landing: &Registered) -> Result<()> {
    if !matches!(landing.standing(), Ok(crate::flow::Standing::Died)) {
        return Ok(());
    }
    let lock_path = landing.dir.join("settle.lock");
    let _lock = crate::sys::open_locked_append(&lock_path).map_err(|source| Error::Ledger {
        path: lock_path.clone(),
        source,
    })?;
    let ended = crate::journal::read(&landing.journal())
        .iter()
        .any(|event| crate::flow::is_terminal_landing_kind(&event.kind.0));
    if ended {
        return Ok(());
    }
    let mut relay = Relay::open(flow, landing, root)?;
    let mut evidence = "no operation was prepared".to_owned();
    if let Some(operation) = relay.operation_id.clone() {
        evidence = match onevcs::OperationHandle::open(&operation)
            .and_then(|handle| onevcs::OperationEvents::open(&handle))
            .and_then(|mut reader| reader.read())
        {
            Ok(batch) => {
                for envelope in &batch {
                    relay.vcs(envelope)?;
                }
                format!("operation {operation}'s recorded events were kept")
            }
            Err(error) => format!("operation {operation}'s events could not be read: {error}"),
        };
    }
    let payload = crate::flow::LandingTransition {
        landing_id: landing.id().to_owned(),
        flow_id: flow.id().to_owned(),
        operation: landing.request.operation,
        subjects: landing.request.subjects.clone(),
        message: format!(
            "its process (pid {}) is gone with no ending recorded; {evidence}",
            landing.request.pid
        ),
    };
    relay.transition(PipelineKind::LandingDied, crate::journal::object(&payload)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An owning node that states no title — a plan from before titles were
    /// required — lands under the default, and the line names the node rather
    /// than claiming no node owns the branch.
    #[test]
    fn an_untitled_owner_is_named_as_the_reason_for_the_default() {
        assert_eq!(
            SubjectSource::Untitled("owned#service".to_owned()).line(),
            "publishing under onevcs's default subject (owned#service states no title)"
        );
    }

    /// A retry chain a person edited into a cycle is walked once, and ends on a
    /// node rather than looping.
    #[test]
    fn a_cyclic_retry_chain_ends_on_a_node() {
        let superseded: std::collections::BTreeMap<String, String> = [("a", "b"), ("b", "a")]
            .into_iter()
            .map(|(from, to)| (from.to_owned(), to.to_owned()))
            .collect();
        assert_eq!(lineage_head(&superseded, "a"), "b");
        assert_eq!(lineage_head(&superseded, "c"), "c");
    }

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

    /// Entry 123 of `docs/contract-divergences.md` names the labels a landing's
    /// records carry, and they are exactly the ones this builds.
    #[test]
    fn the_landing_labels_are_the_ones_the_divergence_record_names() {
        let record = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("docs")
                .join("contract-divergences.md"),
        )
        .expect("the divergence record ships");
        let entry = record
            .split("\n## ")
            .find(|entry| entry.starts_with("123."))
            .expect("entry 123 is recorded");
        let block: serde_json::Value = serde_json::from_str(
            entry
                .split("```json")
                .nth(1)
                .and_then(|rest| rest.split("```").next())
                .expect("entry 123 carries its block"),
        )
        .expect("the block is JSON");
        let built: Vec<String> = landing_labels(
            "landing-1",
            "plan",
            Some("o-1"),
            Some(onevcs::Significance::Progress),
        )
        .extra
        .keys()
        .cloned()
        .collect();
        assert_eq!(block["labels"], serde_json::json!(built));
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
