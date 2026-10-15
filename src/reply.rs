//! Answering review comments on the host: the reply drafter's task, the result it
//! answers and the rules that result is held to, and the one POST each reply is.
//!
//! No reply state is kept anywhere but the change request. A reply carries
//! `onevcs`'s hidden marker with a key every host derives alike —
//! `sha256(<lineage root>:<comment id>:<revision>)` — so whether a comment is
//! answered is read off the host, and recovery is that read: a reply the host
//! accepted is seen by the next read and never posted again, and one it never
//! accepted leaves the comment owed. See `docs/contract-divergences.md` entry 127.

use crate::{
    error::{Error, Result},
    event::Envelope,
    executor::{CancellationToken, DispatchRequest, Executor, WorkspaceSpec},
    ledger::RunPaths,
    payload::{ReplyDisposition, ReplyNotDraftedEnding},
};
use onevcs::{CommentKind, ReviewComment};
use serde_json::Value;
use std::collections::BTreeSet;

/// The reserved persona a reply drafting runs as.
pub(crate) const PERSONA: &str = "review-reply";

/// The longest reply body the engine posts, in characters.
pub(crate) const BODY_LIMIT: usize = 500;

/// The label a declined comment's reply carries in its marker.
pub(crate) const DECLINED: &str = "declined";

/// What `## Worker transcript` says where this host's journal holds no transcript.
pub(crate) const NO_TRANSCRIPT: &str =
    "No worker transcript is available on this host; draft from the sections above.";

/// The fixed opening of every reply drafting's task.
pub(crate) const PREAMBLE: &str = "Draft one terse reply to each review comment below, as \
the author of the change request answering its reviewer in their own thread. For a comment \
the commits under `## Commits` answer, say in a line or two what changed and link at least \
one of those commits by its URL, with disposition `answered`. For a comment the change leaves \
as it is, say in a line why, link no commit, with disposition `declined`. Each body is at most \
500 characters. Answer `replies`: one entry per comment, naming its id.";

/// One commit a rework added: its sha, its subject and where a person reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Commit {
    // llmlint: ignore-block[invalid_states_unrepresentable] git's own answer about a branch on the origin, read off `git log` of the run's commit cache and printed into a drafting task and a view; nothing parses them back into a decision.
    pub(crate) sha: String,
    pub(crate) subject: String,
    pub(crate) url: String,
    // llmlint: ignore-end[invalid_states_unrepresentable]
    /// When it was committed, in seconds since the epoch.
    pub(crate) at: i64,
}

/// Everything one reply drafting is handed: the change request, the comments, the
/// commits a rework added (none for a decline), the reason, and where the worker's
/// transcript is, where this host's journal holds one.
#[derive(Debug, Clone)]
pub(crate) struct Drafting {
    // llmlint: ignore-block[invalid_states_unrepresentable] the change request URL is the one the run's record holds for the node, and the reason is the prose the op carried; both are printed into a task and never executed.
    pub(crate) change_url: String,
    pub(crate) reason: String,
    // llmlint: ignore-end[invalid_states_unrepresentable]
    pub(crate) comments: Vec<ReviewComment>,
    pub(crate) answering: Answering,
}

/// What a drafting answers the comments with.
#[derive(Debug, Clone)]
pub(crate) enum Answering {
    /// A rework's replies: the commits it added, and where its worker's transcript is on
    /// this host, where the journal holds one.
    Rework {
        commits: Vec<Commit>,
        transcript: Option<std::path::PathBuf>,
    },
    /// A decline: every reply is `declined`, carries no commit URL, and is drafted with
    /// no commits and no transcript.
    Decline,
}

/// Where a comment sits on its change request, as the task names it.
pub(crate) fn location(comment: &ReviewComment) -> String {
    match &comment.kind {
        CommentKind::ReviewThread { path, line, .. } => {
            line.map_or_else(|| path.clone(), |line| format!("{path}:{line}"))
        }
        CommentKind::Review { .. } => "review summary".to_owned(),
        CommentKind::Conversation => "conversation comment".to_owned(),
        _ => "review comment".to_owned(),
    }
}

impl Drafting {
    /// The drafter's task: the preamble, then `## Change request`, `## Comments`,
    /// `## Commits`, `## Reason` and `## Worker transcript`.
    pub(crate) fn task(&self) -> String {
        let mut task = format!(
            "{PREAMBLE}\n\n## Change request\n\n{}\n\n## Comments\n",
            self.change_url
        );
        for comment in &self.comments {
            task.push_str(&format!(
                "\n### {}\n\n- URL: {}\n- Author: {}\n- Location: {}\n\n{}\n",
                comment.id.0,
                comment.url,
                comment.author,
                location(comment),
                comment.body
            ));
        }
        task.push_str("\n## Commits\n\n");
        for commit in self.commits() {
            task.push_str(&format!(
                "- {} {} {}\n",
                commit.sha, commit.subject, commit.url
            ));
        }
        task.push_str(&format!(
            "\n## Reason\n\n{}\n\n## Worker transcript\n\n",
            self.reason
        ));
        match &self.answering {
            Answering::Rework {
                transcript: Some(path),
                ..
            } => task.push_str(&format!("{}\n", path.display())),
            _ => task.push_str(&format!("{NO_TRANSCRIPT}\n")),
        }
        task
    }

    /// The commits a rework added; none for a decline.
    pub(crate) fn commits(&self) -> &[Commit] {
        match &self.answering {
            Answering::Rework { commits, .. } => commits,
            Answering::Decline => &[],
        }
    }

    /// Whether this drafting is a decline's.
    pub(crate) fn is_decline(&self) -> bool {
        matches!(self.answering, Answering::Decline)
    }

    /// The comments' ids, in the order the task names them.
    pub(crate) fn comment_ids(&self) -> Vec<String> {
        self.comments
            .iter()
            .map(|comment| comment.id.0.clone())
            .collect()
    }
}

/// One reply the drafter answered, held to the rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Drafted {
    // llmlint: ignore-block[invalid_states_unrepresentable] a comment id the drafting named and a body `validated` held to its length and links; `reply_to_comment` holds both to the host's shape again before anything is posted.
    pub(crate) comment: String,
    pub(crate) disposition: ReplyDisposition,
    pub(crate) body: String,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

/// Every commit URL `body` carries: a link whose path names `commit/<sha>` or
/// `commits/<sha>`, by its sha — whichever commits the drafting was handed.
pub(crate) fn linked_commits(body: &str) -> Vec<String> {
    let mut found = Vec::new();
    for marker in ["/commit/", "/commits/"] {
        let mut rest = body;
        while let Some(at) = rest.find(marker) {
            rest = &rest[at + marker.len()..];
            let sha: String = rest.chars().take_while(char::is_ascii_hexdigit).collect();
            if sha.len() >= 7 && !found.contains(&sha) {
                found.push(sha);
            }
        }
    }
    found
}

/// Whether `body` links `url` itself: an occurrence that ends where the URL does — at the
/// end of the body, at whitespace, or at punctuation closing a sentence or a bracket —
/// rather than one a longer URL extends with more of a path, a query or a fragment.
pub(crate) fn links(body: &str, url: &str) -> bool {
    let mut from = 0;
    while let Some(at) = body[from..].find(url) {
        let end = from + at + url.len();
        let mut after = body[end..].chars();
        let ends = match after.next() {
            None => true,
            Some(next) if next.is_whitespace() => true,
            Some(')' | ']' | '>' | '"' | '\'' | ',' | ';' | '!' | '*' | '`') => true,
            // A sentence's full stop or colon, which no URL path here continues with.
            Some('.' | ':') => after.next().is_none_or(char::is_whitespace),
            Some(_) => false,
        };
        if ends {
            return true;
        }
        from = end;
    }
    false
}

/// The drafter's structured result, held to the rules: every comment the drafting
/// named exactly once and nothing else, every body non-empty and at most
/// [`BODY_LIMIT`] characters, an `answered` body carrying at least one supplied
/// commit URL, and a `declined` one carrying none — and, for a decline, every
/// reply `declined` and no commit URL at all.
pub(crate) fn validated(
    drafting: &Drafting,
    answer: &Value,
) -> std::result::Result<Vec<Drafted>, String> {
    let Some(replies) = answer.get("replies").and_then(Value::as_array) else {
        return Err("the result carries no `replies` list".to_owned());
    };
    let named: Vec<String> = drafting.comment_ids();
    let supplied: Vec<&str> = drafting
        .commits()
        .iter()
        .map(|commit| commit.url.as_str())
        .collect();
    let mut seen = BTreeSet::new();
    let mut drafted = Vec::new();
    for reply in replies {
        let comment = reply
            .get("comment")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("a reply names no `comment`: {reply}"))?;
        if !named.iter().any(|id| id == comment) {
            return Err(format!(
                "a reply answers comment {comment}, which the drafting did not name"
            ));
        }
        if !seen.insert(comment.to_owned()) {
            return Err(format!("comment {comment} is answered twice"));
        }
        let disposition = match reply.get("disposition").and_then(Value::as_str) {
            Some("answered") => ReplyDisposition::Answered,
            Some("declined") => ReplyDisposition::Declined,
            other => {
                return Err(format!(
                    "the reply to {comment} has disposition {other:?}, neither `answered` nor `declined`"
                ))
            }
        };
        let body = reply
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if body.trim().is_empty() {
            return Err(format!("the reply to {comment} is empty"));
        }
        let length = body.chars().count();
        if length > BODY_LIMIT {
            return Err(format!(
                "the reply to {comment} is {length} characters, over the {BODY_LIMIT} a reply may be"
            ));
        }
        let links_supplied = supplied.iter().any(|url| links(body, url));
        match disposition {
            ReplyDisposition::Answered if drafting.is_decline() => {
                return Err(format!(
                    "comment {comment} is declined, and its reply was drafted as `answered`"
                ))
            }
            ReplyDisposition::Answered if !links_supplied => {
                return Err(format!(
                    "the `answered` reply to {comment} links none of the rework's commits"
                ))
            }
            ReplyDisposition::Declined if links_supplied => {
                return Err(format!("the `declined` reply to {comment} links a commit"))
            }
            ReplyDisposition::Declined
                if drafting.is_decline() && !linked_commits(body).is_empty() =>
            {
                return Err(format!(
                    "the declining reply to {comment} carries a commit URL"
                ))
            }
            _ => {}
        }
        drafted.push(Drafted {
            comment: comment.to_owned(),
            disposition,
            body: body.to_owned(),
        });
    }
    if let Some(missing) = named.iter().find(|id| !seen.contains(id.as_str())) {
        return Err(format!("comment {missing} has no reply"));
    }
    Ok(drafted)
}

/// Why a reply drafting answered nothing to post.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NotDrafted {
    pub(crate) ending: ReplyNotDraftedEnding,
    // llmlint: ignore[invalid_states_unrepresentable] the drafter's or the engine's own words, journalled and surfaced as they are.
    pub(crate) detail: String,
}

impl NotDrafted {
    fn failed(detail: impl Into<String>) -> Self {
        Self {
            ending: ReplyNotDraftedEnding::Failed,
            detail: detail.into(),
        }
    }

    /// No reply drafter is configured for the run.
    pub(crate) fn unconfigured() -> Self {
        Self {
            ending: ReplyNotDraftedEnding::Unconfigured,
            detail: "no reply drafter is configured: start the run with --review-reply-graph, \
                     or name `review_reply_graph` in its launch config"
                .to_owned(),
        }
    }
}

/// Run one reply drafting under the reserved persona, in a scratch directory of the
/// run's own, and hold its answer to the rules.
///
/// Composed as the body drafter is — the graph the launch named and nothing of the
/// node's — and labelled with the node whose draft carries the comments. Every
/// envelope it relays is ingested, its report retained under the run, and then
/// handed to `relay`.
pub(crate) fn draft(
    paths: &RunPaths,
    graph: &str,
    node: &str,
    drafting: &Drafting,
    relay: &mut dyn FnMut(Envelope),
) -> std::result::Result<Vec<Drafted>, NotDrafted> {
    let workspace = paths
        .dir
        .join("review-replies")
        .join(format!("{node}-{}", crate::sys::now_millis()));
    std::fs::create_dir_all(&workspace).map_err(|error| {
        NotDrafted::failed(format!(
            "the drafting's directory {} could not be made: {error}",
            workspace.display()
        ))
    })?;
    let executor = crate::executor::LocalExecutor;
    let request = DispatchRequest {
        graph: oneagentgraph::config::ConfigRef(graph.to_owned()),
        task: drafting.task(),
        labels: crate::engine::dispatch_labels(&paths.run, node, None, Some(PERSONA)),
        controls: crate::controls::NodeControls::default(),
        workspace: WorkspaceSpec::Path(workspace),
        cancel: CancellationToken::new(),
        attempt: std::num::NonZeroU32::MIN,
    };
    let mut handle = executor.dispatch(request).map_err(|error| {
        NotDrafted::failed(format!("the reply drafting could not start: {error}"))
    })?;
    let mut retained = Vec::new();
    for envelope in handle.events() {
        let Ok(envelope) = envelope else { continue };
        crate::report::retain(paths, &envelope);
        if envelope.source == crate::event::Source::Agentgraph
            && envelope.kind.0 == crate::report::MEMBER_SETTLED
        {
            retained.push(paths.report_for(&envelope.stream, envelope.seq));
        }
        relay(envelope);
    }
    match handle.wait() {
        Ok(outcome) if outcome.succeeded => {}
        Ok(outcome) => {
            return Err(NotDrafted::failed(format!(
                "the reply drafting settled without succeeding: {}",
                outcome
                    .detail
                    .lines()
                    .find(|line| !line.trim().is_empty())
                    .unwrap_or("it said nothing")
                    .trim()
            )))
        }
        Err(error) => {
            return Err(NotDrafted::failed(format!(
                "the reply drafting could not be waited on: {error}"
            )))
        }
    }
    let kept: Vec<Value> = retained
        .iter()
        .filter_map(|kept| crate::report::read(kept))
        .collect();
    let answer = crate::report::structured(&kept).map_err(NotDrafted::failed)?;
    validated(drafting, &answer).map_err(|detail| NotDrafted {
        ending: ReplyNotDraftedEnding::Invalid,
        detail,
    })
}

/// Post one reply with `onevcs`'s `reply_to_comment`: one POST, with no read of the
/// thread first, because the read that decided it was owed has just shown no reply
/// carrying `key`.
pub(crate) fn post(
    change_url: &str,
    comment: &str,
    body: &str,
    key: &str,
    disposition: ReplyDisposition,
    term_scope: onevcs::TermScope,
) -> Result<onevcs::PostedReply> {
    crate::vcs::reply_to_comment(&onevcs::ReplyRequest {
        change: onevcs::ChangeRef::Url(change_url.to_owned()),
        comment: onevcs::CommentId(comment.to_owned()),
        body: body.to_owned(),
        key: key.to_owned(),
        label: (disposition == ReplyDisposition::Declined).then(|| DECLINED.to_owned()),
        verified_absent: true,
        term_scope,
    })
}

/// The URL a person reads `sha` at, beside the change request it belongs to.
fn commit_url(change_url: &str, sha: &str) -> String {
    match change_url.split_once("/pull/") {
        Some((repository, _)) => format!("{repository}/commit/{sha}"),
        None => format!("{}/commit/{sha}", change_url.trim_end_matches('/')),
    }
}

/// The commits on `branch` from `from` (exclusive) to `to`, oldest first, each with
/// its subject and URL — read off the branch on the repository's origin, fetched
/// into a commit cache of the run's own so no checkout anybody works in is moved.
/// `from` absent is a rework recorded before its superseded head was: the store can
/// show only the replacement's head, so that one commit is all that is read.
pub(crate) fn commits(
    paths: &RunPaths,
    repo: &str,
    branch: &str,
    change_url: &str,
    from: Option<&str>,
    to: &str,
) -> Result<Vec<Commit>> {
    let origin = origin_url(repo)?;
    let cache = paths.dir.join("review-replies").join("commits.git");
    if !cache.join("HEAD").is_file() {
        std::fs::create_dir_all(&cache).map_err(|source| Error::Ledger {
            path: cache.clone(),
            source,
        })?;
        git(&cache, &["init", "--quiet", "--bare"])?;
    }
    git(
        &cache,
        &[
            "fetch",
            "--quiet",
            "--no-tags",
            &origin,
            &format!("+refs/heads/{branch}:refs/heads/reply-source"),
        ],
    )?;
    let range = match from {
        Some(from) => vec![format!("{from}..{to}")],
        None => vec!["-1".to_owned(), to.to_owned()],
    };
    let mut args = vec!["log", "--reverse", "--format=%H%x1f%ct%x1f%s"];
    args.extend(range.iter().map(String::as_str));
    let listed = git(&cache, &args)?;
    Ok(listed
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\u{1f}');
            let sha = fields.next()?.to_owned();
            let at = fields.next()?.parse().ok()?;
            let subject = fields.next().unwrap_or_default().to_owned();
            Some(Commit {
                url: commit_url(change_url, &sha),
                sha,
                subject,
                at,
            })
        })
        .collect())
}

/// The origin a registered checkout of `repo` fetches from.
fn origin_url(repo: &str) -> Result<String> {
    let identity = crate::vcs::repository_identity(repo)?;
    let listed = onevcs::repositories(&onevcs::Providers::real(), onevcs::GateAudit::Skipped)
        .map_err(|error| Error::Sibling {
            tool: "onevcs",
            message: error.to_string(),
        })?;
    let checkout = listed
        .iter()
        .filter(|repository| repository.identity == identity || repository.identity == repo)
        .flat_map(|repository| repository.checkouts.iter())
        .map(|checkout| checkout.path.clone())
        .next()
        .ok_or_else(|| {
            Error::Invalid(format!(
                "no checkout of {repo} is registered on this host to read its commits through"
            ))
        })?;
    let url = git(&checkout, &["remote", "get-url", "origin"])?;
    Ok(url.trim().to_owned())
}

/// Run git in `dir`, answering its stdout or refusing with what it said.
fn git(dir: &std::path::Path, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|error| Error::Invalid(format!("git could not run: {error}")))?;
    if !output.status.success() {
        return Err(Error::Invalid(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The test-only crash seam, shared with discovery's: see [`crate::review::failpoint`].
pub(crate) fn failpoint(stage: &str) -> Result<()> {
    crate::review::failpoint(stage)
}

/// What a drafting that could not post anything tells the journal: `ending` and
/// `detail` beside the node and the comments it named.
pub(crate) fn not_drafted(
    node: &str,
    comments: Vec<String>,
    why: &NotDrafted,
) -> crate::payload::ReviewReplyNotDrafted {
    crate::payload::ReviewReplyNotDrafted {
        node: node.to_owned(),
        comments,
        ending: why.ending,
        detail: why.detail.clone(),
    }
}

/// The superseded node's settled head, as the run's record holds it: where the
/// commits a rework of it added begin.
pub(crate) fn replaces_head(state: &crate::projection::RunState, replaces: &str) -> Option<String> {
    state.heads.get(replaces).cloned()
}

/// What a lineage head's rework asked, read off the run's own record on the host
/// that applied it and off the store's `onepipeline.rework` on any other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Provenance {
    // llmlint: ignore-block[invalid_states_unrepresentable] the `rework-requested` operation's own fields, or the store's provenance `continuation::rework_provenance` validated, as the run's record holds them.
    /// The replacement the rework was applied as.
    pub(crate) rework: String,
    /// Its reason and instruction, byte-for-byte.
    pub(crate) reason: String,
    pub(crate) instruction: String,
    /// The comments it answers, by id.
    pub(crate) comments: Vec<String>,
    /// The superseded node's settled head, where the record holds it.
    pub(crate) replaces_head: Option<String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

/// The rework `node` — a lineage head — was applied as, where it is one.
pub(crate) fn provenance(state: &crate::projection::RunState, node: &str) -> Option<Provenance> {
    if let Some((replaces, rework)) = state
        .reworks
        .iter()
        .find(|(_, rework)| rework.replacement == node)
    {
        return Some(Provenance {
            rework: rework.replacement.clone(),
            reason: rework.reason.clone(),
            instruction: rework.instruction.clone(),
            comments: rework
                .comments
                .iter()
                .map(|comment| comment.id.clone())
                .collect(),
            replaces_head: replaces_head(state, replaces),
        });
    }
    state
        .continued_reworks
        .get(node)
        .map(|continued| Provenance {
            rework: continued.replacement.clone(),
            reason: continued.reason.clone(),
            instruction: continued.instruction.clone(),
            comments: continued
                .comments
                .iter()
                .map(|comment| comment.id.clone())
                .collect(),
            replaces_head: continued.replaces_head.clone(),
        })
}

impl Provenance {
    /// `## Reason` of a rework's drafting: its reason and its instruction, each as the
    /// record holds it.
    pub(crate) fn reason_section(&self) -> String {
        format!("{}\n\n{}", self.reason, self.instruction)
    }
}

/// Seconds since the epoch of an RFC 3339 instant in UTC — `YYYY-MM-DDThh:mm:ss`,
/// an optional fraction of digits and `Z` — or `None` for anything else: a field of
/// the wrong width or not all digits, a component too many, or a date or a time of day
/// the calendar and the clock do not have.
pub(crate) fn epoch_seconds(at: &str) -> Option<i64> {
    let (date, time) = at.strip_suffix('Z')?.split_once('T')?;
    let (clock, fraction) = time.split_once('.').unwrap_or((time, "0"));
    if fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let fields = |text: &str, separator: char, widths: &[usize]| -> Option<Vec<i64>> {
        let parts: Vec<&str> = text.split(separator).collect();
        if parts.len() != widths.len() {
            return None;
        }
        parts
            .iter()
            .zip(widths)
            .map(|(part, width)| {
                (part.len() == *width && part.bytes().all(|byte| byte.is_ascii_digit()))
                    .then(|| part.parse::<i64>().ok())
                    .flatten()
            })
            .collect()
    };
    let date = fields(date, '-', &[4, 2, 2])?;
    let clock = fields(clock, ':', &[2, 2, 2])?;
    let (year, month, day) = (date[0], date[1], date[2]);
    let (hour, minute, second) = (clock[0], clock[1], clock[2]);
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if !(1..=days_in_month).contains(&day) || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    // Days from the civil date (Howard Hinnant's algorithm).
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year - era * 400;
    let of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let of_era_days = of_era * 365 + of_era / 4 - of_era / 100 + of_year;
    let days = era * 146_097 + of_era_days - 719_468;
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// One owed drafting: a rework replacement's draft, the comments of it the rework
/// names that no marked reply answers, and what the drafter is handed.
#[derive(Debug, Clone)]
pub(crate) struct Owed {
    // llmlint: ignore[invalid_states_unrepresentable] the lineage root `review::root` walked to, as the run's record holds it.
    pub(crate) root: String,
    pub(crate) drafting: Drafting,
}

/// What a reply's key and posting need of one comment of an owed drafting.
pub(crate) fn key_of(owed_root: &str, comment: &ReviewComment) -> String {
    crate::review::key(owed_root, comment)
}

/// The drafting `node`'s draft owes, out of the read the pass has just made of it:
/// none where the node is no rework replacement naming comments, has not settled as
/// a review draft, or every comment it names is answered — a reply carrying the
/// current revision's key — or edited since the rework that names it was made, which
/// is new feedback that rework did not see.
///
/// # Errors
///
/// The commits the rework added could not be read off its branch.
pub(crate) fn owed(
    paths: &RunPaths,
    state: &crate::projection::RunState,
    node: &str,
    change_url: &str,
    read: &onevcs::ReviewRead,
) -> std::result::Result<Option<Owed>, NotDrafted> {
    let Some(provenance) = provenance(state, node).filter(|rework| !rework.comments.is_empty())
    else {
        return Ok(None);
    };
    let settled_as_draft = state.statuses().get(node) == Some(&crate::graph::NodeStatus::Done)
        && state
            .outcomes
            .get(node)
            .is_some_and(|outcome| outcome.starts_with("change-review-draft"));
    if !settled_as_draft {
        return Ok(None);
    }
    let root = crate::review::root(state, node).to_owned();
    let mut candidates = Vec::new();
    for id in &provenance.comments {
        let Some(comment) = read
            .comments
            .iter()
            .find(|comment| !comment.ours && comment.id.0 == *id)
        else {
            continue;
        };
        match crate::review::derive(state, node, comment, &read.comments) {
            crate::review::State::BeingReworked { .. } => candidates.push((comment.clone(), false)),
            // An older reply answered an earlier revision: the edit is owed by this
            // rework only where the rework came after it.
            crate::review::State::Pending => candidates.push((comment.clone(), true)),
            _ => {}
        }
    }
    if candidates.is_empty() {
        return Ok(None);
    }
    let (Some(repo), Some(branch), Some(head)) = (
        state.graph.get(node).and_then(|node| node.repo.clone()),
        state.branches.get(node).cloned(),
        state.heads.get(node).cloned(),
    ) else {
        return Err(NotDrafted::failed(format!(
            "node '{node}' has no repository, branch or head on the run's record to read its \
             rework's commits from"
        )));
    };
    let commits = commits(
        paths,
        &repo,
        &branch,
        change_url,
        provenance.replaces_head.as_deref(),
        &head,
    )
    .map_err(|error| {
        NotDrafted::failed(format!("the rework's commits could not be read: {error}"))
    })?;
    let made = commits.iter().map(|commit| commit.at).max();
    let comments: Vec<ReviewComment> = candidates
        .into_iter()
        .filter(|(comment, edited)| {
            !edited
                || matches!(
                    (made, epoch_seconds(&comment.updated_at)),
                    (Some(made), Some(revised)) if made > revised
                )
        })
        .map(|(comment, _)| comment)
        .collect();
    if comments.is_empty() {
        return Ok(None);
    }
    Ok(Some(Owed {
        root,
        drafting: Drafting {
            change_url: change_url.to_owned(),
            reason: provenance.reason_section(),
            comments,
            answering: Answering::Rework {
                commits,
                transcript: transcript(paths, &provenance.rework),
            },
        },
    }))
}

/// Where this host retained the rework worker's transcript, where its journal holds
/// the worker's settlement and the copy is here.
fn transcript(paths: &RunPaths, rework: &str) -> Option<std::path::PathBuf> {
    crate::journal::read(&paths.journal())
        .into_iter()
        .rev()
        .filter(|envelope| {
            envelope.source == crate::event::Source::Agentgraph
                && envelope.kind.0 == crate::report::MEMBER_SETTLED
                && envelope.labels.node.as_deref() == Some(rework)
                && envelope.labels.persona.as_deref() != Some(crate::lifecycle::PR_AUTHOR_PERSONA)
                && envelope.labels.persona.as_deref() != Some(PERSONA)
        })
        .map(|envelope| paths.report_for(&envelope.stream, envelope.seq))
        .find(|path| path.is_file())
}

/// A `decline`, judged against the run's record and not yet posted: the one
/// operation it compiled to, and what posting its replies needs.
pub(crate) struct ValidatedDecline {
    pub(crate) compiled: [crate::edits::Operation; 1],
    paths: RunPaths,
    // llmlint: ignore-block[invalid_states_unrepresentable] the launch record's resolved graph reference, the run's own change request URL and the lineage root `review::root` walked to.
    graph: Option<String>,
    change_url: String,
    root: String,
    // llmlint: ignore-end[invalid_states_unrepresentable]
    term_scope: onevcs::TermScope,
}

/// Every comment id a read this run journalled found on a node of `node`'s lineage.
fn read_for(paths: &RunPaths, state: &crate::projection::RunState, node: &str) -> BTreeSet<String> {
    let root = crate::review::root(state, node);
    crate::journal::read(&paths.journal())
        .into_iter()
        .filter(|envelope| {
            envelope.source == crate::event::Source::Pipeline
                && envelope.kind.as_str() == "review-comments-read"
        })
        .filter(|envelope| {
            envelope
                .labels
                .node
                .as_deref()
                .is_some_and(|read| crate::review::root(state, read) == root)
        })
        .filter_map(|envelope| {
            serde_json::from_value::<crate::payload::ReviewCommentsRead>(Value::Object(
                envelope.payload,
            ))
            .ok()
        })
        .flat_map(|read| read.comments.into_iter().map(|comment| comment.id.0))
        .collect()
}

/// Stage a compiled `decline` for the delivery phase: each comment it names has to
/// be one a read of this run found on the node's draft.
///
/// # Errors
///
/// [`Refusal::CommentNotRead`](crate::channel::Refusal::CommentNotRead) for the
/// first comment no read found.
pub(crate) fn staged_decline(
    paths: &RunPaths,
    state: &crate::projection::RunState,
    graph: Option<&str>,
    compiled: Vec<crate::edits::Operation>,
) -> Result<crate::engine::Staged> {
    let one = match <[crate::edits::Operation; 1]>::try_from(compiled) {
        Ok(one @ [crate::edits::Operation::DeclineApplied { .. }]) => one,
        // llmlint: ignore[changed_behavior_has_e2e] `compile_decline` returns exactly one `decline-applied` or refuses, so no command reaches this arm; it is the type's.
        Ok(other) => return Ok(crate::engine::Staged::Compiled(other.into())),
        Err(other) => return Ok(crate::engine::Staged::Compiled(other)),
    };
    let [crate::edits::Operation::DeclineApplied {
        node,
        comments,
        reason,
        ..
    }] = &one
    else {
        return Ok(crate::engine::Staged::Compiled(one.into()));
    };
    if reason.trim().is_empty() {
        return Err(crate::channel::Refusal::BlankReason { node: node.clone() }.into());
    }
    let read = read_for(paths, state, node);
    if let Some(unread) = comments.iter().find(|comment| !read.contains(&comment.id)) {
        return Err(crate::channel::Refusal::CommentNotRead {
            node: node.clone(),
            comment: unread.id.clone(),
        }
        .into());
    }
    let change_url = state.known_change_url(node).ok_or_else(|| {
        Error::Refused(format!(
            "decline: node '{node}' has no change request on the run's record"
        ))
    })?;
    Ok(crate::engine::Staged::Decline(Box::new(ValidatedDecline {
        root: crate::review::root(state, node).to_owned(),
        compiled: one,
        paths: paths.clone(),
        graph: graph.map(str::to_owned),
        change_url,
        term_scope: crate::boundary::of_run(&crate::boundary::plan_now(state)).term_scope(),
    })))
}

/// Post a validated `decline`'s replies and answer the operation naming them: one
/// fresh read of the change request decides which comments a reply already
/// answers, one drafting drafts the rest, and each is one POST labelled `declined`.
///
/// # Errors
///
/// [`Refusal::AlreadyAnswered`](crate::channel::Refusal::AlreadyAnswered) where a
/// rework's reply answers a named comment, or every one is already declined;
/// [`Refusal::DeclineNotPosted`](crate::channel::Refusal::DeclineNotPosted) where no
/// drafter is configured, the read, the drafting or a post failed — any reply posted
/// before the failure staying on the host.
pub(crate) fn decline(validated: &ValidatedDecline) -> Result<Vec<crate::edits::Operation>> {
    let [crate::edits::Operation::DeclineApplied {
        node,
        comments,
        reason,
        ..
    }] = &validated.compiled
    else {
        return Ok(validated.compiled.to_vec());
    };
    let not_posted = |detail: String| -> Error {
        crate::channel::Refusal::DeclineNotPosted {
            node: node.clone(),
            comments: comments.iter().map(|comment| comment.id.clone()).collect(),
            detail,
        }
        .into()
    };
    let Some(graph) = &validated.graph else {
        return Err(not_posted(NotDrafted::unconfigured().detail));
    };
    let read = crate::vcs::review_comments(&validated.change_url)
        .map_err(|error| not_posted(format!("the change request could not be read: {error}")))?;
    let mut replies: Vec<Option<String>> = Vec::with_capacity(comments.len());
    let mut to_post = Vec::new();
    for named in comments {
        let Some(comment) = read
            .comments
            .iter()
            .find(|comment| !comment.ours && comment.id.0 == named.id)
        else {
            return Err(not_posted(format!(
                "comment {} is no longer on {}",
                named.id, validated.change_url
            )));
        };
        let key = key_of(&validated.root, comment);
        let answering = read.comments.iter().rev().find(|reply| {
            reply.ours
                && reply
                    .reply_marker
                    .as_ref()
                    .is_some_and(|marker| marker.in_reply_to == comment.id && marker.key == key)
        });
        match answering {
            Some(reply)
                if reply
                    .reply_marker
                    .as_ref()
                    .and_then(|marker| marker.label.as_deref())
                    == Some(DECLINED) =>
            {
                replies.push(Some(reply.url.clone()));
            }
            Some(reply) => {
                return Err(crate::channel::Refusal::AlreadyAnswered {
                    node: node.clone(),
                    comment: named.id.clone(),
                    url: reply.url.clone(),
                }
                .into())
            }
            None => {
                replies.push(None);
                to_post.push((comment.clone(), key));
            }
        }
    }
    if to_post.is_empty() {
        let (named, url) = comments
            .iter()
            .zip(&replies)
            .find_map(|(named, url)| url.as_ref().map(|url| (named, url)))
            .ok_or_else(|| not_posted("the decline names no comment".to_owned()))?;
        return Err(crate::channel::Refusal::AlreadyAnswered {
            node: node.clone(),
            comment: named.id.clone(),
            url: url.clone(),
        }
        .into());
    }
    let drafting = Drafting {
        change_url: validated.change_url.clone(),
        reason: reason.clone(),
        comments: to_post.iter().map(|(comment, _)| comment.clone()).collect(),
        answering: Answering::Decline,
    };
    let drafted = draft(&validated.paths, graph, node, &drafting, &mut |_| {})
        .map_err(|why| not_posted(why.detail))?;
    for reply in drafted {
        let Some((comment, key)) = to_post
            .iter()
            .find(|(comment, _)| comment.id.0 == reply.comment)
        else {
            continue;
        };
        let posted = post(
            &validated.change_url,
            &comment.id.0,
            &reply.body,
            key,
            ReplyDisposition::Declined,
            validated.term_scope.clone(),
        )
        .map_err(|error| {
            not_posted(format!(
                "the reply to {} was not posted: {error}",
                comment.id.0
            ))
        })?;
        if let Some(at) = comments.iter().position(|named| named.id == comment.id.0) {
            replies[at] = Some(posted.url);
        }
    }
    failpoint("after-decline-post")?;
    Ok(vec![crate::edits::Operation::DeclineApplied {
        node: node.clone(),
        comments: comments.clone(),
        reason: reason.clone(),
        replies: replies.into_iter().flatten().collect(),
    }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn comment(id: &str) -> ReviewComment {
        serde_json::from_value(json!({
            "id": id, "kind": {"type": "conversation"}, "author": "reviewer",
            "body": format!("fix {id}"), "url": format!("https://github.com/o/r/pull/1#issuecomment-{id}"),
            "created_at": "2026-10-10T00:00:00Z", "updated_at": "2026-10-10T00:00:00Z",
            "ours": false
        }))
        .expect("a comment")
    }

    fn drafting(decline: bool) -> Drafting {
        Drafting {
            change_url: "https://github.com/o/r/pull/1".to_owned(),
            reason: "  the reason, verbatim  ".to_owned(),
            comments: vec![comment("c1"), comment("c2")],
            answering: if decline {
                Answering::Decline
            } else {
                Answering::Rework {
                    commits: vec![Commit {
                        sha: "1234567890abcdef1234567890abcdef12345678".to_owned(),
                        subject: "fix: the thing".to_owned(),
                        url:
                            "https://github.com/o/r/commit/1234567890abcdef1234567890abcdef12345678"
                                .to_owned(),
                        at: 0,
                    }],
                    transcript: None,
                }
            },
        }
    }

    const LINK: &str = "https://github.com/o/r/commit/1234567890abcdef1234567890abcdef12345678";

    #[test]
    fn the_task_carries_each_section_in_order_and_the_reason_verbatim() {
        let task = drafting(false).task();
        let at = |section: &str| {
            task.find(&format!("\n{section}\n"))
                .unwrap_or_else(|| panic!("{section} missing:\n{task}"))
        };
        assert!(task.starts_with(PREAMBLE));
        assert!(at("## Change request") < at("## Comments"));
        assert!(at("## Comments") < at("## Commits"));
        assert!(at("## Commits") < at("## Reason"));
        assert!(at("## Reason") < at("## Worker transcript"));
        assert!(task.contains("\n\n  the reason, verbatim  \n\n"), "{task}");
        assert!(task.contains(NO_TRANSCRIPT));
        assert!(task.contains("- Location: conversation comment"));
        let decline = drafting(true).task();
        assert!(decline.contains("## Commits\n\n\n## Reason"), "{decline}");
    }

    #[test]
    fn a_result_is_held_to_every_rule() {
        let ok = json!({"replies": [
            {"comment": "c1", "disposition": "answered", "body": format!("Fixed in {LINK}")},
            {"comment": "c2", "disposition": "declined", "body": "Left as is: out of scope."},
        ]});
        assert_eq!(validated(&drafting(false), &ok).expect("valid").len(), 2);
        let refused = |answer: Value, decline: bool, said: &str| {
            let why = validated(&drafting(decline), &answer).expect_err("refused");
            assert!(why.contains(said), "{why}");
        };
        refused(json!({}), false, "no `replies`");
        refused(
            json!({"replies": [{"comment": "c1", "disposition": "answered", "body": LINK}]}),
            false,
            "c2 has no reply",
        );
        refused(
            json!({"replies": [
                {"comment": "c1", "disposition": "answered", "body": LINK},
                {"comment": "c1", "disposition": "answered", "body": LINK},
            ]}),
            false,
            "answered twice",
        );
        refused(
            json!({"replies": [{"comment": "c9", "disposition": "answered", "body": LINK}]}),
            false,
            "did not name",
        );
        refused(
            json!({"replies": [{"comment": "c1", "disposition": "maybe", "body": LINK}]}),
            false,
            "neither",
        );
        refused(
            json!({"replies": [{"comment": "c1", "disposition": "answered", "body": "  "}]}),
            false,
            "empty",
        );
        refused(
            json!({"replies": [{"comment": "c1", "disposition": "answered", "body": format!("{LINK} {}", "x".repeat(500))}]}),
            false,
            "over the 500",
        );
        refused(
            json!({"replies": [{"comment": "c1", "disposition": "answered", "body": "fixed"}]}),
            false,
            "links none",
        );
        refused(
            json!({"replies": [{"comment": "c1", "disposition": "declined", "body": LINK}]}),
            false,
            "links a commit",
        );
        refused(
            json!({"replies": [{"comment": "c1", "disposition": "answered", "body": "no"}]}),
            true,
            "drafted as `answered`",
        );
        refused(
            json!({"replies": [{"comment": "c1", "disposition": "declined", "body": "see https://github.com/o/r/commit/abcdef0"}]}),
            true,
            "carries a commit URL",
        );
        let exact = json!({"replies": [
            {"comment": "c1", "disposition": "declined", "body": "x".repeat(500)},
            {"comment": "c2", "disposition": "declined", "body": "out of scope"},
        ]});
        assert!(validated(&drafting(true), &exact).is_ok());
    }

    /// Entry 127's block, out of the divergence record this build ships.
    fn entry_127() -> Value {
        include_str!("../docs/contract-divergences.md")
            .split("\n## ")
            .find(|entry| entry.starts_with("127."))
            .and_then(|entry| entry.split("```json").nth(1))
            .and_then(|rest| rest.split("```").next())
            .map(|block| serde_json::from_str(block).expect("entry 127's block is JSON"))
            .expect("the record carries entry 127's block")
    }

    /// What entry 127 states of the drafter, the reply and the records behind them is
    /// what the private types read and write.
    #[test]
    fn the_drafter_the_reply_and_the_records_are_entry_127s() {
        let block = entry_127();
        assert_eq!(block["persona"], PERSONA);
        assert_eq!(block["body_limit"], json!(BODY_LIMIT));
        assert_eq!(block["declined_label"], DECLINED);
        assert_eq!(block["no_transcript"], NO_TRANSCRIPT);
        let task = drafting(false).task();
        let mut at = 0;
        for section in block["task_sections"].as_array().expect("the sections") {
            let section = section.as_str().expect("a heading");
            let found = task[at..]
                .find(&format!("\n{section}\n"))
                .unwrap_or_else(|| panic!("{section} is not after the one before it:\n{task}"));
            at += found + 1;
        }
        // The block's result, held to the rules over a drafting naming its comments and
        // supplying the commit its answer links.
        let named: Vec<ReviewComment> = block["result"]["replies"]
            .as_array()
            .expect("the replies")
            .iter()
            .map(|reply| {
                let mut held = comment("x");
                held.id = onevcs::CommentId(reply["comment"].as_str().unwrap().to_owned());
                held
            })
            .collect();
        let sha = "1f2e3d4c5b6a79881726354453627180918a7b6c";
        let supplied = Drafting {
            comments: named,
            answering: Answering::Rework {
                commits: vec![Commit {
                    sha: sha.to_owned(),
                    subject: "fix: retry".to_owned(),
                    url: format!("https://github.com/owner/service/commit/{sha}"),
                    at: 0,
                }],
                transcript: None,
            },
            ..drafting(false)
        };
        let drafted = validated(&supplied, &block["result"]).expect("the block's result is valid");
        let dispositions: Vec<Value> = drafted
            .iter()
            .map(|reply| serde_json::to_value(reply.disposition).unwrap())
            .collect();
        assert_eq!(Value::Array(dispositions), block["dispositions"]);
        let endings: Vec<Value> = [
            NotDrafted::unconfigured().ending,
            ReplyNotDraftedEnding::Failed,
            ReplyNotDraftedEnding::Invalid,
        ]
        .iter()
        .map(|ending| serde_json::to_value(ending).unwrap())
        .collect();
        assert_eq!(Value::Array(endings), block["endings"]);
        let stored = crate::continuation::rework_provenance(&block["rework_metadata"])
            .expect("the store's provenance, with its superseded head, reads");
        assert_eq!(
            stored.replaces_head.as_deref(),
            block["rework_metadata"]["replaces_head"].as_str()
        );
        let mut older = block["rework_metadata"].clone();
        older.as_object_mut().unwrap().remove("replaces_head");
        assert_eq!(
            crate::continuation::rework_provenance(&older)
                .expect("a record without it reads")
                .replaces_head,
            None
        );
        assert_eq!(
            block["checkpoint_schema_version"],
            json!(crate::checkpoint::CHECKPOINT_SCHEMA_VERSION)
        );
        assert_eq!(
            block["results_schema_version"],
            json!(crate::review::RESULTS_SCHEMA_VERSION)
        );
        let operation: crate::edits::Operation = serde_json::from_value({
            let mut applied = block["decline_applied"].clone();
            applied["kind"] = json!("decline-applied");
            applied
        })
        .expect("the block's decline-applied is an operation");
        assert_eq!(block["operation_kinds"], json!([operation.kind()]));
        assert!(operation.commits_a_change());
        for (kind, record) in block["records"].as_object().expect("the records") {
            let read = match kind.as_str() {
                "review-reply-posted" => {
                    serde_json::from_value::<crate::payload::ReviewReplyPosted>(record.clone())
                        .map(|_| ())
                }
                "review-reply-not-drafted" => {
                    serde_json::from_value::<crate::payload::ReviewReplyNotDrafted>(record.clone())
                        .map(|_| ())
                }
                other => panic!("entry 127 records {other}, which no payload here is"),
            };
            read.unwrap_or_else(|why| panic!("{kind}: {why}"));
        }
    }

    #[test]
    fn a_body_links_a_supplied_url_only_where_that_url_ends() {
        for body in [
            format!("Fixed in {LINK}"),
            format!("Fixed in {LINK}."),
            format!("Fixed ({LINK}), thanks"),
            format!("See {LINK}: the rename"),
        ] {
            assert!(links(&body, LINK), "{body}");
        }
        for body in [
            format!("Fixed in {LINK}0"),
            format!("Fixed in {LINK}/files"),
            format!("Fixed in {LINK}#diff-1"),
            format!("Fixed in {LINK}?w=1"),
            format!("Fixed in {LINK}.patch"),
        ] {
            assert!(!links(&body, LINK), "{body}");
        }
        let extended = json!({"replies": [
            {"comment": "c1", "disposition": "answered", "body": format!("Fixed in {LINK}/files")},
            {"comment": "c2", "disposition": "declined", "body": "Left as is."},
        ]});
        let why = validated(&drafting(false), &extended).expect_err("refused");
        assert!(why.contains("links none of the rework's commits"), "{why}");
    }

    #[test]
    fn an_instant_is_read_only_whole_and_as_the_calendar_has_it() {
        assert_eq!(epoch_seconds("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch_seconds("2026-10-10T18:34:13Z"), Some(1_791_657_253));
        assert_eq!(
            epoch_seconds("2026-10-10T18:34:13.250Z"),
            Some(1_791_657_253)
        );
        assert_eq!(epoch_seconds("2024-02-29T00:00:00Z"), Some(1_709_164_800));
        for refused in [
            "2026-10-10T18:34:13",
            "2026-10-10 18:34:13Z",
            "2026-02-30T00:00:00Z",
            "2025-02-29T00:00:00Z",
            "2026-13-01T00:00:00Z",
            "2026-10-10T24:00:00Z",
            "2026-10-10T18:60:00Z",
            "2026-10-10T18:34:60Z",
            "2026-10-10T18:34:13.Z",
            "2026-10-10T18:34:13.2xZ",
            "2026-10-10T18:34:13:01Z",
            "2026-10-10-01T18:34:13Z",
            "26-10-10T18:34:13Z",
            "2026-1-10T18:34:13Z",
            "+026-10-10T18:34:13Z",
            "99999999999999999999-10-10T18:34:13Z",
        ] {
            assert_eq!(epoch_seconds(refused), None, "{refused}");
        }
    }

    #[test]
    fn commit_links_are_read_by_sha_and_built_beside_the_change_request() {
        assert_eq!(
            linked_commits("see https://github.com/o/r/commit/abcdef1234 and /commits/0123456789."),
            vec!["abcdef1234".to_owned(), "0123456789".to_owned()]
        );
        assert!(linked_commits("https://github.com/o/r/commit/abc").is_empty());
        assert_eq!(
            commit_url("https://github.com/o/r/pull/7", "abc"),
            "https://github.com/o/r/commit/abc"
        );
    }
}
