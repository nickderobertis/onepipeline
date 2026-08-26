//! A `gh` executable, scripted from a directory.
//!
//! **Not a double for a sibling.** `onepipeline` calls `onevcs` as a library, so
//! nothing in this repository stands in for it any more; what stands in here is
//! *GitHub*, at `onevcs`'s own documented `ONEVCS_GH` override — the seam that
//! library declares precisely so a journey can decide what the host does without
//! a network or a credential. Everything on the repository side stays real: a
//! journey that reaches this has already cloned, committed, gated, and pushed
//! with real git against a real origin on disk.
//!
//! It answers exactly the `gh` invocations `onevcs::GitHub` makes and **refuses
//! everything else**, argument by argument: every flag that library passes is
//! required here, and an invocation carrying one it does not pass is refused. A
//! double is worth what it refuses — one that shrugged off an unknown flag would
//! let this stack reach the real `gh` with an argument it has never taken and
//! keep the suite green. `tests/smoke/` runs the same publication against the
//! real `gh`, which is what holds the shapes below honest.
//!
//! Two endings, because they are the two a host decides: a change request the
//! host **merged**, when `gh.merged` is scripted, and one it is holding
//! otherwise. Which of those a publication settles on is the repository's
//! policy, not this program's.
//!
//! And one non-ending: `gh.outage` makes this host **unreachable**, which is not
//! a decision about a change request but the absence of one. It is how a journey
//! reaches a publication whose push landed and whose merge path then could not be
//! read at all. Empty, it lasts: every invocation is refused, which is the host
//! that never comes back. Holding a **count**, it is the outage that ends — that
//! many invocations are refused and every one after them is answered, which is
//! what a caller that reads the merge path again has to meet to be worth
//! anything.

use onepipeline_testfakes as fake;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The login this host reports as the caller.
///
/// `onevcs` records it on the change request it opens, so a journey asserting
/// who opened one has a name to assert against.
const WHO: &str = "onepipeline-e2e";

/// The commit a change request's checks are reported against, where this host
/// has no branch to read one off. See [`head_of`].
const HEAD_SHA: &str = "0123456789abcdef0123456789abcdef01234567";

/// The commit a merge lands at, when this host merges one.
const MERGE_SHA: &str = "fedcba9876543210fedcba9876543210fedcba98";

/// What `gh` says when it never reached GitHub at all — its own two lines, in its
/// own order, on stderr with nothing on stdout and exit 1.
///
/// Scripted by the presence of `gh.outage`, and it answers **every** invocation:
/// an outage is not a per-command state, and a double that kept answering `pr
/// list` while refusing `pr checks` would be a host no operator has ever met.
/// What it is *for* is the one publication ending that has no other cause — the
/// publishing push reaches the origin, which is git and not this program, and
/// then the merge path behind it cannot be read.
const UNREACHABLE: &str = "error connecting to api.github.com\ncheck your internet \
                           connection or https://githubstatus.com";

/// What this host knows about one change request.
///
/// Two states and no third: a file holding anything else is a scenario nobody
/// wrote, and reading it leniently would be this program inventing a host
/// behaviour a journey did not ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    /// Opened, and the host has not landed it.
    Open,
    /// The host landed it, at [`MERGE_SHA`].
    Merged,
}

impl Change {
    /// How the state is written down, and the only two spellings read back.
    fn as_str(self) -> &'static str {
        match self {
            Change::Open => "open",
            Change::Merged => "merged",
        }
    }

    fn parse(recorded: &str) -> Option<Self> {
        match recorded.trim() {
            "open" => Some(Change::Open),
            "merged" => Some(Change::Merged),
            _ => None,
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = fake::script_dir();
    fake::record(&dir, "gh", &args);

    // Recorded first, so a journey scripting the outage can still assert this
    // host was asked — an unreachable host is one that was called, not one that
    // never was.
    if unreachable(&dir) {
        eprintln!("{UNREACHABLE}");
        return ExitCode::from(1);
    }

    match (
        args.first().map(String::as_str),
        args.get(1).map(String::as_str),
    ) {
        (Some("api"), Some("user")) => user(&args),
        (Some("pr"), Some("create")) => create(&args, &dir),
        (Some("pr"), Some("list")) => list(&args, &dir),
        (Some("pr"), Some("view")) => view(&args, &dir),
        (Some("pr"), Some("checks")) => checks(&args, &dir),
        (Some("pr"), Some("merge")) => merge(&args, &dir),
        (Some("run"), Some("view")) => log(&args),
        (Some(one), Some(two)) => fake::refuse(&format!("unknown gh command '{one} {two}'")),
        (Some(one), None) => fake::refuse(&format!("unknown gh command '{one}'")),
        (None, _) => fake::refuse("gh takes a command"),
    }
}

/// Whether this invocation meets the outage a journey scripted, counting it in.
///
/// An empty script is the outage that does not end: every invocation is refused.
/// A script holding a count is the outage that does — the first `n` invocations
/// are refused and every one after them is answered — and the count of what has
/// been refused so far lives on disk, because nothing carries state between two
/// invocations of a program that exits.
///
/// A script holding anything else is a scenario nobody wrote, and reading it
/// leniently would be this program inventing a host behaviour a journey did not
/// ask for: an unparsable count read as "always" would make an outage that was
/// meant to end never end, and the journey written to prove a recovery would
/// prove the opposite.
fn unreachable(dir: &Path) -> bool {
    let Some(script) = fake::node_script(dir, "gh", "outage") else {
        return false;
    };
    if script.trim().is_empty() {
        return true;
    }
    let Ok(refuse) = script.trim().parse::<usize>() else {
        fake::fail(&format!(
            "gh.outage holds {script:?}, which is neither empty nor a count of invocations              to refuse"
        ));
    };
    let refused = dir.join("gh").join("refused");
    let so_far = read_if_present(&refused)
        .unwrap_or_default()
        .lines()
        .count();
    if so_far >= refuse {
        return false;
    }
    fake::append(&refused, "refused");
    true
}

/// What a flag's value has to be for this host to trust it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// Exactly this. A selector — `--jq`, `--json`, `--state` — decides what the
    /// real `gh` puts in its answer, and the answers below are written to one
    /// query each. Accepting a different selector while replying with the same
    /// object is this host answering a question nobody asked: `onevcs` narrowing
    /// its `--json` list would keep passing here and meet a smaller object from
    /// the real `gh`.
    Exact(&'static str),
    /// It names something — a repository, a branch, a title. Empty names
    /// nothing, so it is refused.
    Named,
    /// Free text a person wrote. `onevcs` passes `--body ""` for a change request
    /// that carries no body, and refusing that would refuse a shape the real `gh`
    /// takes every day.
    Prose,
}

/// Check one invocation against the exact shape `onevcs` passes.
///
/// `positional` is what must appear before the flags, `flags` every option that
/// must be present with a value of the shape named beside it, and `bare` every
/// option that must be present without one. Anything left over is an argument
/// this host has never taken, and is refused rather than ignored.
fn shaped(
    args: &[String],
    what: &str,
    positional: usize,
    flags: &[(&str, Shape)],
    bare: &[&str],
) -> Result<(), ExitCode> {
    let mut seen = positional;
    for (flag, shape) in flags {
        let Some(at) = args.iter().position(|arg| arg == flag) else {
            return Err(fake::refuse(&format!("gh {what} requires {flag}")));
        };
        // The *value*, not merely its presence: everything read here is
        // interpolated into the JSON this host prints and into the URL `onevcs`
        // takes a change request's number out of. A missing value leaves the
        // next option in its place, and `gh` would read that as an option rather
        // than as the value — so a host that accepted it would answer a question
        // nobody asked.
        let Some(value) = args.get(at + 1) else {
            return Err(fake::refuse(&format!("gh {what} needs a value for {flag}")));
        };
        if value.starts_with('-') {
            return Err(fake::refuse(&format!(
                "gh {what} was given {value:?} where {flag} needs a value, which gh reads as \
                 another option"
            )));
        }
        match shape {
            Shape::Exact(asked) if value != asked => {
                return Err(fake::refuse(&format!(
                    "gh {what} was given {flag} {value:?}; this host answers {asked:?} and \
                     nothing else"
                )));
            }
            Shape::Named if value.is_empty() => {
                return Err(fake::refuse(&format!(
                    "gh {what} was given an empty {flag}, which names nothing"
                )));
            }
            _ => {}
        }
        seen += 2;
    }
    for flag in bare {
        if !args.iter().any(|arg| arg == flag) {
            return Err(fake::refuse(&format!("gh {what} requires {flag}")));
        }
        seen += 1;
    }
    if args.len() != seen {
        return Err(fake::refuse(&format!(
            "gh {what} was given arguments it does not take: {args:?}"
        )));
    }
    Ok(())
}

/// `gh api user --jq .login`
fn user(args: &[String]) -> ExitCode {
    if let Err(refusal) = shaped(
        args,
        "api user",
        2,
        &[("--jq", Shape::Exact(".login"))],
        &[],
    ) {
        return refusal;
    }
    println!("{WHO}");
    ExitCode::SUCCESS
}

/// `gh run view --repo R --log --job N`
fn log(args: &[String]) -> ExitCode {
    if let Err(refusal) = shaped(
        args,
        "run view",
        2,
        &[("--repo", Shape::Named), ("--job", Shape::Named)],
        &["--log"],
    ) {
        return refusal;
    }
    // Named for the job it is the log of, so a journey reading back the artifact
    // a check's log was stored as can tell one check's from another's.
    println!(
        "the host's job log for job {}",
        fake::flag(args, "--job").unwrap_or_default()
    );
    ExitCode::SUCCESS
}

/// One change request this host opened, as `pr create` recorded it.
///
/// The same type both writes the record and reads it back, so the file's two
/// halves cannot drift; anything it does not accept — a missing field, an
/// unexpected one, a `number` of `0` — is a record this host did not write, and
/// answering it as a change request is how `pr list` comes to leave one out and
/// this host to open a second beside it.
///
/// `title` and `body` are the two nothing here consults: they are recorded for a
/// journey to read back, which is the only place a drafted body is a fact rather
/// than an argument that was passed.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Opened {
    repo: String,
    head: String,
    base: String,
    title: String,
    body: String,
    number: NonZeroU64,
}

fn state_of(dir: &Path, id: &str) -> PathBuf {
    dir.join("gh").join(fake::segment(id))
}

/// A file this host wrote, or nothing if it has not written it yet.
///
/// **Not present** and **not readable** are two different facts and only the
/// first is an answer. A file that is not there is a change request this host
/// has not opened, which every caller below has something true to say about; an
/// `EACCES` or a short read is a broken fixture, and answering it as absence
/// makes this host invent state — a change request reported missing, or a
/// numbering that restarts at 1 and addresses somebody else's. So only
/// `NotFound` becomes `None`, and every other error is fatal where it happened.
fn read_if_present(path: &Path) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => fake::fail(&format!("{} could not be read: {error}", path.display())),
    }
}

/// What this host knows about a change request, or nothing if it opened none.
///
/// A state file that exists and does not read as one of the two states is
/// **fatal**, not absent: this is the record `pr create` and `pr merge` wrote,
/// and reporting it as "no pull request found" would send the journey around it
/// looking for a change request that was opened. Nothing but this program writes
/// the file, so reaching the refusal means the program is wrong.
fn recorded(dir: &Path, id: &str) -> Option<Change> {
    let path = state_of(dir, id);
    let recorded = read_if_present(&path)?;
    Some(Change::parse(&recorded).unwrap_or_else(|| {
        fake::fail(&format!(
            "{} holds {recorded:?}, which is not a state this host records",
            path.display()
        ))
    }))
}

/// Write down what this host now knows about a change request.
fn record(dir: &Path, id: &str, state: Change) {
    let path = state_of(dir, id);
    if let Some(parent) = path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            fake::fail(&format!("cannot create {}: {error}", parent.display()));
        }
    }
    if let Err(error) = std::fs::write(&path, state.as_str()) {
        fake::fail(&format!("cannot write {}: {error}", path.display()));
    }
}

/// The next number, derived from the record rather than kept anywhere: nothing
/// carries a counter between two invocations of a program that exits.
fn next_number(dir: &Path) -> NonZeroU64 {
    NonZeroU64::MIN.saturating_add(opened_changes(dir).len() as u64)
}

/// `gh pr create --repo R --head H --base B --title T --body B`
///
/// Prints the change request's URL, which is the only thing `onevcs` reads back:
/// it takes the host's number out of that URL's last segment, so a URL that does
/// not end in one would be refused there rather than here.
fn create(args: &[String], dir: &Path) -> ExitCode {
    if let Err(refusal) = shaped(
        args,
        "pr create",
        2,
        &[
            ("--repo", Shape::Named),
            ("--head", Shape::Named),
            ("--base", Shape::Named),
            ("--title", Shape::Named),
            ("--body", Shape::Prose),
        ],
        &[],
    ) {
        return refusal;
    }
    let flag = |name: &str| fake::flag(args, name).unwrap_or_default();
    let opened = Opened {
        repo: flag("--repo"),
        head: flag("--head"),
        base: flag("--base"),
        title: flag("--title"),
        body: flag("--body"),
        number: next_number(dir),
    };
    fake::append(
        &dir.join("gh").join("opened.jsonl"),
        &serde_json::to_string(&opened)
            .unwrap_or_else(|error| fake::fail(&format!("the record does not serialise: {error}"))),
    );
    record(dir, &opened.number.to_string(), Change::Open);
    println!("{}", url_of(&opened));
    ExitCode::SUCCESS
}

/// Where the origin behind one host slug actually is, as the world running this
/// journey wrote it down.
///
/// The identity says `github.com/owner/service` while the git remote under it is
/// a bare repository on this disk, and only that world knows both — so it writes
/// the pairing down and this reads it back, rather than either side deriving the
/// other from a directory layout neither owns.
fn origin_of(dir: &Path, repo: &str) -> Option<PathBuf> {
    let recorded = read_if_present(&dir.join("gh").join("origin").join(fake::segment(repo)))?;
    let origin = PathBuf::from(recorded.trim());
    // Checked rather than trusted for where it came from: what came back is a
    // path off a file, and it is handed to `git --git-dir`. An absolute
    // directory that is there is what the world writes; anything else names no
    // repository this can read, and the placeholder is the answer for that.
    (origin.is_absolute() && origin.is_dir()).then_some(origin)
}

/// A commit id this host reports: forty hexadecimal characters and nothing else.
///
/// A type rather than a `String` because one of these is read off `git`, and a
/// value that reached `headRefOid` has to have been checked on the way in rather
/// than trusted for where it came from — `onevcs` decides which checks are about
/// which commit by comparing it.
struct Sha(String);

impl Sha {
    fn parse(reported: &str) -> Option<Self> {
        let reported = reported.trim();
        (reported.len() == 40 && reported.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| Self(reported.to_ascii_lowercase()))
    }

    fn placeholder() -> Self {
        Self::parse(HEAD_SHA).unwrap_or_else(|| fake::fail("HEAD_SHA is not an object id"))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

/// A branch name this host will build a ref out of.
///
/// `opened.head` is read back off a file rather than off this process's own
/// command line, and it is interpolated into `refs/heads/…` — so a value
/// carrying `..` or a separator of its own names a ref somewhere else in the
/// namespace. Checked on the way in, by git's own branch rules as far as one
/// component needs them.
struct Branch(String);

impl Branch {
    fn parse(named: &str) -> Option<Self> {
        let usable = !named.is_empty()
            && !named.starts_with('/')
            && !named.ends_with('/')
            && !named.starts_with('.')
            && !named.ends_with(".lock")
            && !named.contains("..")
            && !named.contains("//")
            && !named.contains("@{")
            && !named.contains("/.")
            && named
                .chars()
                .all(|c| !c.is_ascii_control() && !" ~^:?*[\\".contains(c));
        usable.then(|| Self(named.to_owned()))
    }

    fn as_ref_name(&self) -> String {
        format!("refs/heads/{}", self.0)
    }
}

/// The commit this host reports as a change request's head.
///
/// Read off the branch, because `onevcs` sets aside every check attached to some
/// *other* commit. Answered from a constant, every check this host reports names
/// a commit no publication pushed, so a red one reads as a check that has not
/// arrived rather than as a check that failed.
///
/// [`HEAD_SHA`] where no usable origin was written down for the slug, where the
/// recorded head is not a branch name a ref can be built from, where the origin
/// carries no such branch, or where git answers something that is not an object
/// id: a change request over a branch this host cannot see has no better answer.
fn head_of(dir: &Path, opened: &Opened) -> Sha {
    // Scripted `gh.head` is a host reporting some commit other than the branch's
    // tip — the state a real host is in for the seconds after a push it has not
    // processed, and the one a journey cannot reach by pushing, because every
    // commit it makes is the tip by the time it asks.
    if let Some(scripted) = fake::node_script(dir, "gh", "head") {
        return Sha::parse(&scripted).unwrap_or_else(|| {
            fake::fail(&format!(
                "scripted `gh.head` {scripted:?} is not an object id"
            ))
        });
    }
    let (Some(origin), Some(branch)) = (origin_of(dir, &opened.repo), Branch::parse(&opened.head))
    else {
        return Sha::placeholder();
    };
    std::process::Command::new("git")
        .arg("--git-dir")
        .arg(&origin)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(branch.as_ref_name())
        .output()
        .ok()
        .filter(|answer| answer.status.success())
        .and_then(|answer| String::from_utf8(answer.stdout).ok())
        .as_deref()
        .and_then(Sha::parse)
        .unwrap_or_else(Sha::placeholder)
}

/// The URL a change request this host opened is reached at.
///
/// Written once because `pr create` prints it and `pr list` reports it, and
/// `onevcs` takes the change request's number out of its last segment either
/// way: two spellings of it could disagree about the very number this host is
/// asked about next.
fn url_of(opened: &Opened) -> String {
    format!("https://github.com/{}/pull/{}", opened.repo, opened.number)
}

/// `gh pr list --repo R --head H --base B --state open --json …`
///
/// Every change request this host still has open from `head` into `base`, which
/// is the question `onevcs` asks before it opens one. It used to answer empty
/// always, on the reading that a journey publishes a branch once — and a node
/// whose publication fails and is dispatched again publishes it twice. Against
/// the real host the second one finds the first's change request and adopts it,
/// and `gh pr create` on a branch that already has one **fails**; a host that
/// answered empty here would have this suite proving a second change request
/// nobody can open.
fn list(args: &[String], dir: &Path) -> ExitCode {
    if let Err(refusal) = shaped(
        args,
        "pr list",
        2,
        &[
            ("--repo", Shape::Named),
            ("--head", Shape::Named),
            ("--base", Shape::Named),
            ("--state", Shape::Exact("open")),
            ("--json", Shape::Exact("number,url,state,headRefOid")),
        ],
        &[],
    ) {
        return refusal;
    }
    let flag = |name: &str| fake::flag(args, name).unwrap_or_default();
    let open: Vec<serde_json::Value> = opened_changes(dir)
        .into_iter()
        // One state file holds every repository a journey registered, so without
        // this a branch name is answered out of another repository's changes.
        .filter(|change| change.repo == flag("--repo"))
        .filter(|change| change.head == flag("--head"))
        .filter(|change| change.base == flag("--base"))
        .filter(|change| recorded(dir, &change.number.to_string()) == Some(Change::Open))
        .map(|change| {
            let head = head_of(dir, &change);
            serde_json::json!({
                "number": change.number,
                "url": url_of(&change),
                "state": "OPEN",
                "headRefOid": head.as_str(),
            })
        })
        .collect();
    println!("{}", serde_json::json!(open));
    ExitCode::SUCCESS
}

/// Every change request this host has opened, in the order it opened them.
///
/// A line that is not one of [`Opened`] is **fatal**, for the reason [`recorded`]
/// gives: this file is what `pr create` wrote, and a record dropped here is a
/// change request this host would go on to open a second one beside. Syntax and
/// shape are one refusal, because a well-formed object missing `head` is no more
/// answerable than a line that is not JSON at all.
fn opened_changes(dir: &Path) -> Vec<Opened> {
    let path = dir.join("gh").join("opened.jsonl");
    read_if_present(&path)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|error| {
                fake::fail(&format!(
                    "{} holds a line this host did not write: {error}: {line}",
                    path.display()
                ))
            })
        })
        .collect()
}

/// The change request an invocation addresses, or the refusal that it addresses
/// none.
///
/// `gh` addresses one by `--repo` **and** a number together, and so does this:
/// the number alone is a key into a state directory holding every repository a
/// journey registered, and a host that answered it would answer one
/// repository's question out of another's change requests. The number is one
/// and never zero besides — that is what `onevcs` takes out of the URL this host
/// printed and hands back here, and anything else names nothing it opened.
///
/// Every verb that takes an `ID` reads it through here, so none of them can be
/// the lenient one.
fn addressed(args: &[String], dir: &Path) -> Result<Opened, ExitCode> {
    let id = fake::required(args, 2, "ID")?;
    let Ok(number) = id.parse::<NonZeroU64>() else {
        eprintln!("{id} is not a pull request number");
        return Err(ExitCode::from(1));
    };
    let repo = fake::flag(args, "--repo").unwrap_or_default();
    opened_changes(dir)
        .into_iter()
        .find(|opened| opened.number == number && opened.repo == repo)
        .ok_or_else(|| {
            eprintln!("no pull request found for {id} in {repo}");
            ExitCode::from(1)
        })
}

/// What this host recorded about a change request it opened.
///
/// **Fatal** where there is nothing, rather than an answer: `pr create` writes
/// the record and the state together, so a change request [`addressed`] resolved
/// and this cannot is a fixture that lost one of the two.
fn state_of_opened(dir: &Path, opened: &Opened) -> Change {
    let id = opened.number.to_string();
    recorded(dir, &id).unwrap_or_else(|| {
        fake::fail(&format!(
            "this host opened change request {id} and has no state for it"
        ))
    })
}

/// `gh pr view ID --repo R --json …`
fn view(args: &[String], dir: &Path) -> ExitCode {
    let fields = match args
        .windows(2)
        .find(|pair| pair[0] == "--json")
        .map(|pair| pair[1].as_str())
    {
        Some("headRefOid") => "headRefOid",
        Some("state,mergeCommit") => "state,mergeCommit",
        Some("statusCheckRollup") => "statusCheckRollup",
        // The rollup *and* the commit it was reported against, in one read.
        // `onevcs` 0.15.0 asks for the pair because a rollup read on its own
        // cannot say whether the checks in it ran on the head the publication
        // just pushed — which is how a stale verdict from an earlier head
        // decided a merge path and consumed a node's last retry.
        Some("headRefOid,statusCheckRollup") => "headRefOid,statusCheckRollup",
        _ => "number,state,mergeStateStatus,headRefOid,mergeCommit,statusCheckRollup",
    };
    if let Err(refusal) = shaped(
        args,
        "pr view",
        3,
        &[("--repo", Shape::Named), ("--json", Shape::Exact(fields))],
        &[],
    ) {
        return refusal;
    }
    let opened = match addressed(args, dir) {
        Ok(opened) => opened,
        Err(refusal) => return refusal,
    };
    let id = opened.number.to_string();
    let merged = state_of_opened(dir, &opened) == Change::Merged;
    let reported = scripted_checks(dir, &id);
    let head = head_of(dir, &opened);
    println!(
        "{}",
        serde_json::json!({
            "number": opened.number,
            "state": if merged { "MERGED" } else { "OPEN" },
            "mergeStateStatus": "CLEAN",
            "headRefOid": head.as_str(),
            "mergeCommit": merged.then(|| serde_json::json!({"oid": MERGE_SHA})),
            // What this host reports about the change request's checks, which is
            // the scripted part: unscripted it reports none, which is the
            // repository whose only bar is the `command:` gate its rules name.
            "statusCheckRollup": reported
                .iter()
                .map(Check::rollup_entry)
                .collect::<Vec<_>>(),
        })
    );
    ExitCode::SUCCESS
}

/// Where a check is, and — once it has finished — how it ended.
///
/// One value rather than a status beside an optional conclusion, because those
/// two have exactly one legal pairing each way: a `completed` check has a
/// conclusion and an unfinished one cannot have. Split, a scripted line could say
/// `in_progress failure`, which is a host state GitHub never reports and a
/// journey would then be asserting against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Queued,
    Running,
    Settled(Conclusion),
}

/// How a settled check ended, as GitHub spells it.
///
/// Green and red both: a journey proving a red check has to be able to write a
/// green one beside it or it has proved only that this host says no.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Conclusion {
    Success,
    Skipped,
    Neutral,
    Failure,
    Cancelled,
    TimedOut,
}

/// Whether branch protection lists the check, so that a merge waits on it.
///
/// Named rather than a `bool`, because it is the field that decides whether a red
/// check ends a publication, and `Check { .., true }` at a call site says nothing
/// about which of the two it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Blocks {
    Required,
    Advisory,
}

impl Conclusion {
    const EVERY: &'static [Self] = &[
        Self::Success,
        Self::Skipped,
        Self::Neutral,
        Self::Failure,
        Self::Cancelled,
        Self::TimedOut,
    ];

    fn wire(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Skipped => "skipped",
            Self::Neutral => "neutral",
            Self::Failure => "failure",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
        }
    }

    fn parse(word: &str) -> Option<Self> {
        Self::EVERY.iter().copied().find(|it| it.wire() == word)
    }
}

impl State {
    fn status(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "in_progress",
            Self::Settled(_) => "completed",
        }
    }

    /// One scripted `STATUS CONCLUSION` pair, or why it is not one.
    ///
    /// `-` is the conclusion of a check that has not finished, and it is
    /// **required** there rather than optional: a line short of a field is a line
    /// whose author meant something this program would have to guess.
    fn parse(status: &str, conclusion: &str) -> Result<Self, String> {
        let concluded = |conclusion: &str| {
            Conclusion::parse(conclusion).ok_or_else(|| {
                let every: Vec<&str> = Conclusion::EVERY.iter().map(|it| it.wire()).collect();
                format!("{conclusion:?} is not a conclusion this host reports; they are {every:?}")
            })
        };
        match (status, conclusion) {
            ("queued", "-") => Ok(Self::Queued),
            ("in_progress", "-") => Ok(Self::Running),
            ("completed", "-") => Err(
                "a `completed` check concludes something; write its conclusion where the `-` is"
                    .to_owned(),
            ),
            ("completed", concluded_as) => concluded(concluded_as).map(Self::Settled),
            ("queued" | "in_progress", concluded_as) => Err(format!(
                "a {status} check has not finished and cannot conclude {concluded_as:?}; write `-`"
            )),
            (other, _) => Err(format!(
                "{other:?} is not a status this host reports; they are \"queued\", \
                 \"in_progress\", and \"completed\""
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Check {
    /// Branch protection lists checks by name, and a red one's refusal names it,
    /// so this is the whole of how a journey addresses one.
    name: String,
    state: State,
    blocks: Blocks,
}

impl Check {
    /// One scripted line, or the refusal that it is not one.
    ///
    /// `NAME STATUS CONCLUSION REQUIRED`, four fields always. Nothing is
    /// defaulted: a line this program filled in for its author would answer a
    /// journey that was never written.
    fn parse(line: &str) -> Result<Self, String> {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [name, status, conclusion, blocks] = fields[..] else {
            return Err(format!(
                "a scripted check is `NAME STATUS CONCLUSION REQUIRED`, and {line:?} is not"
            ));
        };
        let state =
            State::parse(status, conclusion).map_err(|why| format!("check {name:?}: {why}"))?;
        let blocks = match blocks {
            "required" => Blocks::Required,
            "advisory" => Blocks::Advisory,
            other => {
                return Err(format!(
                    "check {name:?} is {other:?}, which says nothing about whether it blocks the \
                     merge; write `required` or `advisory`"
                ))
            }
        };
        Ok(Check {
            name: name.to_owned(),
            state,
            blocks,
        })
    }

    /// The check as `gh pr view --json statusCheckRollup` reports it.
    ///
    /// `conclusion` is **absent** rather than null while a check is running,
    /// which is what the real rollup does and what `onevcs` reads as "the host
    /// cannot know yet".
    fn rollup_entry(&self) -> serde_json::Value {
        let mut entry = serde_json::json!({"name": self.name, "status": self.state.status()});
        if let State::Settled(conclusion) = self.state {
            entry["conclusion"] = serde_json::json!(conclusion.wire());
        }
        entry
    }
}

/// What this host reports about one change request's checks.
///
/// Scripted `gh.checks`, one check per line, with `gh.checks.<NUMBER>` beside it
/// for a journey that has more than one change request open at once and needs
/// this host to answer differently about each.
///
/// A line this program cannot read is **fatal**, not skipped: a journey scripting
/// a check this host quietly dropped would assert against a rollup nobody wrote.
fn scripted_checks(dir: &Path, id: &str) -> Vec<Check> {
    let text = fake::node_script(dir, "gh", &format!("checks.{}", fake::segment(id)))
        .or_else(|| fake::node_script(dir, "gh", "checks"));
    let Some(text) = text else {
        return Vec::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| Check::parse(line).unwrap_or_else(|why| fake::fail(&why)))
        .collect()
}

/// `gh pr checks ID --repo R [--required] --json …`
///
/// Two queries and no others, because `onevcs` makes two: which checks block the
/// merge, and where each check ran. A `--json` selector this host has not been
/// written an answer to is refused rather than answered with the wrong object.
fn checks(args: &[String], dir: &Path) -> ExitCode {
    let required_only = args.iter().any(|arg| arg == "--required");
    let (fields, bare): (&str, &[&str]) = if required_only {
        ("name", &["--required"])
    } else {
        ("name,link", &[])
    };
    if let Err(refusal) = shaped(
        args,
        "pr checks",
        3,
        &[("--repo", Shape::Named), ("--json", Shape::Exact(fields))],
        bare,
    ) {
        return refusal;
    }
    let opened = match addressed(args, dir) {
        Ok(opened) => opened,
        Err(refusal) => return refusal,
    };
    let reported = scripted_checks(dir, &opened.number.to_string());
    if required_only {
        let required: Vec<&Check> = reported
            .iter()
            .filter(|check| check.blocks == Blocks::Required)
            .collect();
        // A repository with no branch protection declares no required check, and
        // `gh` says so by *failing* — exit 1, nothing on stdout, and a line that
        // opens exactly this way. `onevcs` reads that whole shape as an answer,
        // so a host that reported an empty list instead would leave the one path
        // every unprotected repository takes untested here.
        if required.is_empty() {
            eprintln!("no required checks reported on the {} branch", opened.head);
            return ExitCode::from(1);
        }
        println!(
            "{}",
            serde_json::json!(required
                .iter()
                .map(|check| serde_json::json!({"name": check.name}))
                .collect::<Vec<_>>())
        );
        return ExitCode::SUCCESS;
    }
    let repo = &opened.repo;
    println!(
        "{}",
        serde_json::json!(reported
            .iter()
            .enumerate()
            .map(|(index, check)| serde_json::json!({
                "name": check.name,
                // The details URL, whose last segment is the job id `onevcs`
                // takes out of it to ask for a log. A link with no `/job/<id>`
                // is what the real `gh` reports for a check no workflow ran, and
                // answering one here for every check would make the id this host
                // is asked for unpredictable.
                "link": format!("https://github.com/{repo}/actions/runs/1/job/{}", index + 1),
            }))
            .collect::<Vec<_>>())
    );
    ExitCode::SUCCESS
}

/// `gh pr merge ID --repo R --squash [--auto]`
///
/// Whether the merge *lands* is the scripted part. Without `gh.merged` the host
/// has accepted the request and not landed it, which is what a queued merge is —
/// and `onevcs` reads that back from the next `pr view`, not from this command,
/// so scripting it here rather than in the exit code is what makes the two agree.
fn merge(args: &[String], dir: &Path) -> ExitCode {
    // `--auto` is the difference between `change-auto` and `change-direct`, so
    // the shape is checked with and without it rather than by ignoring it.
    let auto = args.iter().any(|arg| arg == "--auto");
    let bare: &[&str] = if auto {
        &["--squash", "--auto"]
    } else {
        &["--squash"]
    };
    if let Err(refusal) = shaped(args, "pr merge", 3, &[("--repo", Shape::Named)], bare) {
        return refusal;
    }
    let opened = match addressed(args, dir) {
        Ok(opened) => opened,
        Err(refusal) => return refusal,
    };
    if dir.join("gh.merged").exists() {
        record(dir, &opened.number.to_string(), Change::Merged);
    }
    ExitCode::SUCCESS
}
