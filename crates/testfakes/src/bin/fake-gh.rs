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

use onepipeline_testfakes as fake;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The login this host reports as the caller.
///
/// `onevcs` records it on the change request it opens, so a journey asserting
/// who opened one has a name to assert against.
const WHO: &str = "onepipeline-e2e";

/// The commit a change request's checks are reported against.
///
/// A real object id shape — forty hex characters — because it travels into
/// `onevcs::Sha` and out onto the stream a journey reads.
const HEAD_SHA: &str = "0123456789abcdef0123456789abcdef01234567";

/// The commit a merge lands at, when this host merges one.
const MERGE_SHA: &str = "fedcba9876543210fedcba9876543210fedcba98";

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

/// The branch one change request was opened from, as this host recorded it.
///
/// Read back out of what `pr create` wrote rather than taken off the argv: `gh
/// pr checks` is addressed by the change request's number and says nothing about
/// a branch, so a host that named one from an argument it was handed would be
/// inventing the very fact it is being asked about.
fn head_of(dir: &Path, id: &str) -> Option<String> {
    opened_changes(dir)
        .into_iter()
        .find(|opened| opened["number"].as_u64().map(|n| n.to_string()).as_deref() == Some(id))
        .and_then(|opened| opened["head"].as_str().map(str::to_owned))
}

/// Where this host's state for one change request lives.
fn state_of(dir: &Path, id: &str) -> PathBuf {
    dir.join("gh").join(fake::segment(id))
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
    let recorded = std::fs::read_to_string(&path).ok()?;
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

/// The number this host gives the next change request it opens.
///
/// Counted from what it has already opened, so a journey that opens two reads
/// two different change requests rather than one addressed twice.
fn next_number(dir: &Path) -> u64 {
    let opened = dir.join("gh").join("opened.jsonl");
    std::fs::read_to_string(&opened)
        .map(|text| text.lines().filter(|line| !line.trim().is_empty()).count() as u64)
        .unwrap_or(0)
        + 1
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
    let number = next_number(dir);
    fake::append(
        &dir.join("gh").join("opened.jsonl"),
        &serde_json::json!({
            "repo": flag("--repo"),
            "head": flag("--head"),
            "base": flag("--base"),
            "title": flag("--title"),
            // What a reviewer actually reads. Recorded rather than only
            // shape-checked: the body is the whole product of a drafting
            // dispatch, and a journey that could not read it back would be
            // asserting that *something* was passed.
            "body": flag("--body"),
            "number": number,
        })
        .to_string(),
    );
    record(dir, &number.to_string(), Change::Open);
    println!("https://github.com/{}/pull/{number}", flag("--repo"));
    ExitCode::SUCCESS
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
        .filter(|change| change["head"].as_str() == Some(flag("--head").as_str()))
        .filter(|change| change["base"].as_str() == Some(flag("--base").as_str()))
        .filter(|change| {
            change["number"]
                .as_u64()
                .is_some_and(|number| recorded(dir, &number.to_string()) == Some(Change::Open))
        })
        .map(|change| {
            let number = change["number"].as_u64().unwrap_or_default();
            serde_json::json!({
                "number": number,
                "url": format!("https://github.com/{}/pull/{number}", flag("--repo")),
                "state": "OPEN",
                "headRefOid": HEAD_SHA,
            })
        })
        .collect();
    println!("{}", serde_json::json!(open));
    ExitCode::SUCCESS
}

/// Every change request this host has opened, in the order it opened them.
///
/// A line that is not JSON is **fatal**, for the reason [`recorded`] gives: this
/// file is what `pr create` wrote, and a record dropped here is a change request
/// this host would go on to open a second one beside.
fn opened_changes(dir: &Path) -> Vec<serde_json::Value> {
    let path = dir.join("gh").join("opened.jsonl");
    std::fs::read_to_string(&path)
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
    let id = match fake::required(args, 2, "ID") {
        Ok(id) => id,
        Err(refusal) => return refusal,
    };
    // A number, because that is what `onevcs` takes out of the URL this host
    // printed and hands back here. Anything else names no change request it
    // opened, and answering one would be inventing a host that numbers its
    // changes some other way.
    let Ok(number) = id.parse::<u64>() else {
        eprintln!("{id} is not a pull request number");
        return ExitCode::from(1);
    };
    let Some(state) = recorded(dir, &id) else {
        eprintln!("no pull request found for {id}");
        return ExitCode::from(1);
    };
    let merged = state == Change::Merged;
    let reported = scripted_checks(dir, &id);
    println!(
        "{}",
        serde_json::json!({
            "number": number,
            "state": if merged { "MERGED" } else { "OPEN" },
            "mergeStateStatus": "CLEAN",
            "headRefOid": HEAD_SHA,
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
    /// The host has the check and it has not started.
    Queued,
    /// It is running, so how it ends is the one thing the host cannot yet know.
    Running,
    /// It finished, this way.
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

/// Whether the host says a check stands between the change and its merge.
///
/// Named rather than a `bool`, because it is the field that decides whether a red
/// check ends a publication, and `Check { .., true }` at a call site says nothing
/// about which of the two it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Blocks {
    /// Branch protection lists it, so a merge waits on it.
    Required,
    /// It runs and reports, and nothing waits on it.
    Advisory,
}

impl Conclusion {
    /// Every conclusion this host reports, for the refusal that lists them.
    const EVERY: &'static [Self] = &[
        Self::Success,
        Self::Skipped,
        Self::Neutral,
        Self::Failure,
        Self::Cancelled,
        Self::TimedOut,
    ];

    /// How GitHub spells it, which is what goes on the wire.
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
    /// The `status` GitHub reports for this state.
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

/// One check this host reports on a change request.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Check {
    /// Branch protection lists checks by name, and a red one's refusal names it,
    /// so this is the whole of how a journey addresses one.
    name: String,
    /// Where it is, and how it ended if it has.
    state: State,
    /// Whether a merge waits on it.
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
/// Scripted `gh.checks`, one check per line, and `gh.checks.<NUMBER>` for a
/// journey whose change requests are answered differently — which is what a node
/// that publishes, fails its checks, and publishes again is: two change requests,
/// on two trees, and a host that reported the same thing about both would make a
/// recovery indistinguishable from a loop.
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
    let id = match fake::required(args, 2, "ID") {
        Ok(id) => id,
        Err(refusal) => return refusal,
    };
    if recorded(dir, &id).is_none() {
        eprintln!("no pull request found for {id}");
        return ExitCode::from(1);
    }
    let reported = scripted_checks(dir, &id);
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
            eprintln!(
                "no required checks reported on the {} branch",
                head_of(dir, &id).unwrap_or_else(|| "unknown".to_owned())
            );
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
    let repo = fake::flag(args, "--repo").unwrap_or_default();
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
    let id = match fake::required(args, 2, "ID") {
        Ok(id) => id,
        Err(refusal) => return refusal,
    };
    if recorded(dir, &id).is_none() {
        eprintln!("no pull request found for {id}");
        return ExitCode::from(1);
    }
    if dir.join("gh.merged").exists() {
        record(dir, &id, Change::Merged);
    }
    ExitCode::SUCCESS
}
