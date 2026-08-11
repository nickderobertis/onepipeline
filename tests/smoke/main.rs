//! The real-everything smoke: this crate, the real `onevcs`, real `git`, and the
//! real GitHub API, over one whole lifecycle.
//!
//! Every other journey in this repository is offline. That is right for them and
//! it is exactly what let R0 ship: the argv this crate built had only ever been
//! checked against a scripted double, so `onepipeline start` exited 0, printed a
//! launch record naming a pid, and dispatched nothing. The publication path had
//! the same gap and the same defect — this crate read `onevcs publish`'s stdout
//! as JSON, which that command has never printed. **This is the tier that proves
//! the composition against the world**, and there is exactly one journey in it.
//!
//! It is not run by `just test` or `just gate`: it lives in its own test binary,
//! which those exclude, so an ordinary gate on a laptop pays nothing for it and
//! needs no credential. `just smoke-real` is how a person runs it and the same
//! entry point CI uses on `pull_request`.
//!
//! What it does **not** do is skip. A smoke that passes without having talked to
//! GitHub is the defect this tier exists to remove, so a missing `gh`, a missing
//! credential, or a repository it is not allowed to touch all fail loudly and
//! name what is missing. Nothing here falls back to a provider or a double on
//! the repository or host side — the one thing standing in is the paid model
//! turn, at `oneagentgraph`'s own `ONEAGENTGRAPH_ONEHARNESS_BIN` override, which
//! `dispatch.rs` substitutes for the same reason.

// llmlint: ignore-file[e2e_not_mocked] nothing on the layer under test is substituted:
// the real `onevcs` binary drives real `git` against a real GitHub remote and the real
// `gh`-backed host opens and merges a real pull request. The only stand-in is one layer
// below the sibling — the provider turn `oneagentgraph` spawns, swapped at that library's
// own documented override — because there is no offline stand-in for a paid model turn
// and this journey is about the repository and host sides.

#[path = "../e2e/harness.rs"]
mod harness;

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

use harness::{plan_of, World};

/// The scratch repository this journey publishes to when nothing names another.
pub const DEFAULT_SMOKE_REPO: &str = "nickderobertis/onepipeline-smoke";

/// The variable that names a different scratch repository.
pub const SMOKE_REPO_ENV: &str = "ONEPIPELINE_SMOKE_REPO";

/// What a repository's name has to end with for this journey to touch it.
///
/// The whole guard, and deliberately a crude one: this journey pushes branches,
/// opens pull requests, and merges them, and the cost of pointing it at a
/// repository somebody works in is not recoverable. A name nobody would give a
/// repository they cared about is a rule a misconfiguration cannot slip past,
/// where "is it the one I meant?" is a judgement a program cannot make.
pub const SCRATCH_SUFFIX: &str = "-smoke";

/// The one substitution, named where a reader of a failure will look for it.
const WHAT_STANDS_IN: &str = "only the paid model turn stands in; git, onevcs, and GitHub are real";

fn main_repo() -> String {
    std::env::var(SMOKE_REPO_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_SMOKE_REPO.to_owned())
}

/// The scratch repository, or a refusal naming why this is not one.
///
/// Asserted **before the first mutating call**: creating the repository is
/// already a mutation, so the check cannot live any later than this.
fn scratch_repo() -> String {
    let slug = main_repo();
    let parts: Vec<&str> = slug.split('/').collect();
    let [owner, name] = parts[..] else {
        panic!(
            "{SMOKE_REPO_ENV}={slug:?} does not name one repository as owner/name, and this \
             journey pushes branches and merges pull requests — it will not guess"
        )
    };
    assert!(
        !owner.is_empty() && !name.is_empty(),
        "{SMOKE_REPO_ENV}={slug:?} names an empty owner or repository"
    );
    assert!(
        name.ends_with(SCRATCH_SUFFIX),
        "{SMOKE_REPO_ENV}={slug:?} is not a scratch repository: this journey pushes branches, \
         opens pull requests, and merges them, so it runs only against a repository whose name \
         ends in {SCRATCH_SUFFIX:?}. Point it at one, or leave it unset for \
         {DEFAULT_SMOKE_REPO:?}."
    );
    slug
}

/// Run `gh`, or say what is missing rather than pretending it is not needed.
fn gh(args: &[&str]) -> String {
    match gh_try(args) {
        Ok(out) => out,
        Err(refusal) => panic!("{refusal}"),
    }
}

fn gh_try(args: &[&str]) -> Result<String, String> {
    let output = Command::new("gh")
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| missing(&format!("`gh` could not be run: {error}")))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    Err(format!(
        "gh {} exited {}: {}",
        args.join(" "),
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

/// What a run with no credential is told, which has to be enough to fix it.
fn missing(what: &str) -> String {
    format!(
        "the real-everything smoke needs the GitHub CLI and a credential for it, and {what}. \
         Install `gh` (https://cli.github.com), then either run `gh auth login` or set GH_TOKEN \
         to a token that can push to, open a pull request on, and merge in the scratch \
         repository. In CI it is the SMOKE_GH_TOKEN secret, passed as GH_TOKEN: a fine-grained PAT on the scratch repository alone. \
         This journey never skips and never falls back to a fake: a smoke that can pass without \
         talking to GitHub proves nothing."
    )
}

/// Who GitHub says is calling, or a refusal naming the missing credential.
fn authenticated_user() -> String {
    match gh_try(&["api", "user", "--jq", ".login"]) {
        Ok(login) if !login.trim().is_empty() => login.trim().to_owned(),
        Ok(_) => panic!("{}", missing("GitHub named no authenticated user")),
        Err(refusal) => panic!("{}", missing(&format!("it refused: {refusal}"))),
    }
}

/// The scratch repository, created private if it is not there yet.
///
/// Created rather than required, so a first run on a fresh account works — and
/// **never deleted**, by this journey or any other: what it removes is the
/// branches and pull requests it made.
fn ensure_repo(slug: &str) {
    if gh_try(&["repo", "view", slug, "--json", "name"]).is_ok() {
        return;
    }
    gh(&[
        "repo",
        "create",
        slug,
        "--private",
        "--add-readme",
        "--description",
        "Scratch repository for onepipeline's real-everything smoke. Nothing here is kept.",
    ]);
    // `--add-readme` commits asynchronously often enough to matter: a clone that
    // wins the race gets a repository with no branch at all, and every later
    // step then fails naming something other than the reason.
    for _ in 0..60 {
        if gh_try(&["api", &format!("repos/{slug}/commits/HEAD")]).is_ok() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    panic!("the scratch repository {slug} was created but never got its first commit");
}

/// Teach this world's git to authenticate the way `gh` does.
///
/// Written into the world's own config rather than by running `gh auth
/// setup-git`, which would edit the global config of whoever ran the smoke.
/// `GIT_CONFIG_GLOBAL` already points every process this run starts — including
/// the ones `onevcs` spawns — at that file, so the push, the fetch, and the
/// fast-forward all authenticate and nothing outside the world is configured.
fn git_credentials(world: &World) -> PathBuf {
    let path = world.gitconfig();
    let config = std::fs::read_to_string(&path).unwrap_or_default();
    if !config.contains("helper") {
        std::fs::write(
            &path,
            format!(
                "{config}[credential \"https://github.com\"]\n\thelper = !gh auth git-credential\n"
            ),
        )
        .expect("the world's git config is written");
    }
    path
}

/// Run git, refusing to continue on anything it rejects.
fn git(world: &World, repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_CONFIG_GLOBAL", git_credentials(world))
        .env("GIT_AUTHOR_NAME", harness::GIT_WHO)
        .env("GIT_AUTHOR_EMAIL", harness::GIT_EMAIL)
        .env("GIT_COMMITTER_NAME", harness::GIT_WHO)
        .env("GIT_COMMITTER_EMAIL", harness::GIT_EMAIL)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} in {} failed: {}",
        repo.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The branches this run made, removed however it ends.
///
/// A `Drop` rather than a line at the end of the test, because the run that most
/// needs cleaning up is the one that failed half way. The repository itself is
/// never touched.
struct Scratch {
    slug: String,
    branch: String,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Best effort and quiet about success: a merged pull request's head
        // branch is often already gone, and a cleanup that failed must not be
        // reported as the journey failing.
        let reference = format!("repos/{}/git/refs/heads/{}", self.slug, self.branch);
        if let Err(refusal) = gh_try(&["api", "-X", "DELETE", &reference]) {
            eprintln!(
                "smoke: could not remove the branch {}: {refusal}",
                self.branch
            );
        }
    }
}

/// The rules the scratch repository publishes under.
///
/// `change-direct`: push the branch, open a pull request on the real GitHub, and
/// merge it. That is the whole of what this tier exists to prove, and the one
/// policy that reaches every step of it. The gate is a real command — the
/// repository holds nothing to verify, and a gate that is not run is a
/// publication nothing checked.
fn rules(home: &Path) {
    std::fs::write(
        home.join("rules.yml"),
        "version: 2\nrules: []\ndefault:\n  publication: change-direct\n  approvals: none\n  \
         gate:\n    command: [\"git\", \"--version\"]\n",
    )
    .expect("the rules file is written");
}

/// The file the substituted worker turn wrote, named by the turn itself.
///
/// Read out of what the double recorded rather than restated here: a journey
/// that spelled the name itself would keep asserting against a file after the
/// turn had stopped writing one, and "the base carries no such file" would read
/// as a publication that lost work.
fn work_file(world: &World) -> String {
    let wrote = world
        .invocations()
        .iter()
        .find(|invocation| invocation["tool"] == "oneharness-work")
        .and_then(|invocation| invocation["args"][0].as_str().map(str::to_owned))
        .unwrap_or_else(|| {
            panic!(
                "the substituted worker turn wrote nothing into the session's worktree, so \
                    there was never anything for this run to publish"
            )
        });
    Path::new(&wrote)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .expect("the turn recorded the file it wrote")
}

/// What the run recorded, by kind, from the sibling's own stream.
fn vcs_kinds(world: &World, run: &str) -> Vec<String> {
    world
        .journal(run)
        .iter()
        .filter(|event| event["source"] == "vcs")
        .filter_map(|event| event["kind"].as_str().map(str::to_string))
        .collect()
}

/// Why a run settled the way it did, as the sibling itself said it.
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
    // The one substituted turn is where a run that published nothing usually
    // went wrong — a worker that never wrote leaves a clean tree, and `onevcs
    // publish` then succeeds having done nothing.
    let turns: Vec<String> = world
        .invocations()
        .iter()
        .filter(|invocation| {
            invocation["tool"]
                .as_str()
                .is_some_and(|tool| tool.starts_with("oneharness"))
        })
        .map(|invocation| format!("{}: {}", invocation["tool"], invocation["args"]))
        .collect();
    format!(
        "what the nodes settled on ({WHAT_STANDS_IN}):\n  {}\n  the sibling recorded: {:?}\n  \
         the substituted turn was asked for: {turns:?}",
        settled.join("\n  "),
        vcs_kinds(world, run)
    )
}

/// One lifecycle, end to end, against the real world.
///
/// Open a session, do work in it, verify it, open a pull request on GitHub,
/// merge it, and confirm the base advanced. Every one of those is the real
/// thing; none of them is asserted from anywhere a person could not look.
#[test]
fn a_lifecycle_node_opens_a_real_pull_request_merges_it_and_the_base_advances() {
    // Before anything is created, pushed, or merged.
    let slug = scratch_repo();
    let who = authenticated_user();
    ensure_repo(&slug);

    let world = World::new("smoke-real");
    world.write_graphs();
    // Before anything is pushed: the offline journeys answer `gh` out of a
    // scratch directory at `onevcs`'s own `ONEVCS_GH` override, and a command
    // that still carried it would open a change request nobody can look at and
    // report it merged. This tier talks to GitHub or it fails.
    assert!(
        !World::substitutes_the_host(&world.real_cmd(&["start"])),
        "{}",
        missing("this run still carries a stand-in for `gh`, so it would never reach GitHub")
    );
    let home = world.onevcs_home();
    std::fs::create_dir_all(&home).expect("a scratch state root");
    rules(&home);

    let checkout = world.root.join("checkout");
    git(
        &world,
        &world.root,
        &[
            "clone",
            &format!("https://github.com/{slug}.git"),
            "checkout",
        ],
    );

    // A branch per run: several pull requests may be in flight on one scratch
    // repository at once, and two runs sharing a branch name would each publish
    // the other's work.
    let branch = format!(
        "onepipeline-smoke/{}-{}",
        std::process::id(),
        world
            .root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    );
    let title = format!("feat: smoke {branch}");
    let _scratch = Scratch {
        slug: slug.clone(),
        branch: branch.clone(),
    };

    // Registered by calling the sibling, not by spawning it: the credential
    // helper is already in this world's git config, which the registration reads
    // through `GIT_CONFIG_GLOBAL`.
    git_credentials(&world);
    world.register(&checkout, None);

    // The identity the publication will be addressed to, read back from the
    // sibling rather than assumed: this is the last point before a push, and it
    // is what makes "the scratch repository and nothing else" a checked fact. As
    // the sibling's own `Identity`, so a refusal arrives as an error naming the
    // repository rather than as output this journey has to parse.
    assert_eq!(
        world.identity(&checkout).origin,
        format!("github.com/{slug}"),
        "the registered checkout does not resolve to the scratch repository, so this run would \
         publish somewhere nobody chose"
    );

    // The worker turn writes into the worktree `oneagentgraph` resolved for it,
    // and writes something **no earlier run wrote**. The scratch repository
    // keeps what every run merged into it, so a fixed body is a diff exactly
    // once: the second run finds its own content already on the base, `onevcs
    // publish` correctly reports nothing to publish, and the journey fails on
    // its own history rather than on anything this crate did.
    let wrote = format!("the worker wrote this for {branch}");
    world.script("harness.work", &wrote);
    let plan = world.plan(
        "smoke",
        &plan_of(
            "smoke",
            vec![json!({
                "id": "service",
                "repo": checkout.to_string_lossy(),
                "branch": branch,
                "persona": "engineer",
                "title": title,
                "task": "## What\nShip the service.\n\n## Why\nProve the composition against \
                         the real world.\n\n## Acceptance criteria\n- It is merged.",
            })],
        ),
    );

    let started = world
        .real_cmd(&["start", &plan.to_string_lossy(), "--attach"])
        .env("GIT_CONFIG_GLOBAL", git_credentials(&world))
        .output()
        .expect("the binary runs");
    // A launch that failed because the *run* failed exits non-zero having said
    // nothing about why on any descriptor a test can read: `monitor`'s rendering
    // carries the kinds and not the settlement's detail, and the detail is the
    // sibling's own refusal. So it is read out of the journal here, before the
    // world is torn down — this assertion used to fail with a bare exit code, one
    // debugging session away from the reason.
    assert!(
        started.status.success(),
        "`onepipeline start` exited {}\n{}\nstdout: {}\nstderr: {}",
        started.status.code().unwrap_or(-1),
        why(&world, "smoke"),
        String::from_utf8_lossy(&started.stdout),
        String::from_utf8_lossy(&started.stderr)
    );
    world.until("the run to settle", |world| {
        !world.events_of("smoke", "round-finished").is_empty()
    });

    // What this crate made of it.
    let result = world.run_json("smoke", "round-01/result.json");
    assert_eq!(
        result["nodes"][0]["status"],
        "done",
        "{result}\n{}",
        why(&world, "smoke")
    );
    assert_eq!(
        result["nodes"][0]["outcome"],
        "merged",
        "{result}\n{}",
        why(&world, "smoke")
    );

    // What GitHub made of it, asked of GitHub. The change request the run
    // recorded is the one this looks up, so a run that merged somebody else's
    // pull request could not read as a pass.
    let opened = world.events_of("smoke", "change-opened");
    let opened = opened.first().unwrap_or_else(|| {
        panic!(
            "no change request reached the merged store\n{}",
            why(&world, "smoke")
        )
    });
    let url = opened["payload"]["url"]
        .as_str()
        .expect("the change request names where a human reads it")
        .to_owned();
    assert!(
        url.contains(&slug),
        "the change request was opened somewhere other than the scratch repository: {url}"
    );
    assert_eq!(
        opened["payload"]["author"],
        json!(who),
        "the change request was opened by somebody other than the credential this run used: \
         {opened}"
    );

    let view: Value = serde_json::from_str(&gh(&[
        "pr",
        "view",
        &url,
        "--json",
        "number,state,title,mergeCommit,headRefName,baseRefName",
    ]))
    .expect("gh answers with JSON");
    assert_eq!(
        view["state"],
        json!("MERGED"),
        "GitHub does not call it merged: {view}"
    );
    assert_eq!(view["headRefName"], json!(branch), "{view}");
    // The title the plan gave the node is the one the change request carries.
    assert_eq!(view["title"], json!(title), "{view}");

    // The branch the change landed on, as GitHub names it. Required rather than
    // defaulted to `main`: this is what the two reads below are addressed to, and
    // a silent fallback would point them at a branch this run never published to
    // — so a merge onto some other base would read as a pass.
    let base_ref = view["baseRefName"].as_str().unwrap_or_else(|| {
        panic!("GitHub named no base branch for the change request this run opened: {view}")
    });

    // And the base advanced by exactly this change, read off the repository.
    let head: Value =
        serde_json::from_str(&gh(&["api", &format!("repos/{slug}/commits/{base_ref}")]))
            .expect("gh answers with JSON");
    assert_eq!(
        head["sha"], view["mergeCommit"]["oid"],
        "the merge landed but the base does not point at it: {head}"
    );
    // GitHub squashes a single-commit change under that commit's own subject
    // rather than the request's title, and appends the request's number — which
    // is the part that says *which* change the base's tip is.
    let subject = head["commit"]["message"].as_str().unwrap_or_default();
    let numbered = format!("(#{})", view["number"]);
    assert!(
        subject.contains(&numbered),
        "the base's tip is not the change request this run published: it says {subject:?}, and \
         this run opened {numbered}"
    );
    let file = work_file(&world);
    let landed = gh(&[
        "api",
        "-H",
        "Accept: application/vnd.github.raw",
        &format!("repos/{slug}/contents/{file}?ref={base_ref}"),
    ]);
    assert_eq!(
        landed.trim(),
        wrote,
        "the base carries a {file} that is not the one this run's worker wrote"
    );

    // The sibling's own record of the publication reached the merged store, once
    // each and under the node it belongs to.
    let kinds = vcs_kinds(&world, "smoke");
    for kind in ["gate-verdict", "push", "change-merged", "session-closed"] {
        let seen = kinds.iter().filter(|seen| *seen == kind).count();
        assert_eq!(
            seen, 1,
            "the publication's {kind} reached the merged store {seen} time(s): {kinds:?}"
        );
    }
    let verdict = &world.events_of("smoke", "gate-verdict")[0];
    assert_eq!(verdict["labels"]["node"], "service", "{verdict}");
}
