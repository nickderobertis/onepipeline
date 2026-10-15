//! The live half of `docs/contract-divergences.md` entry 119: a project continued from its
//! plan store by a host holding none of the run's state, and the best-effort claim on who
//! drives it, against the real GitHub on the repository side.
//!
//! The store is one local Markdown root every host of the journey shares, as a team shares
//! one board: which backend a project lives on is onetaskgraph's to test, not this
//! repository's, so this journey needs no credential beyond the one it publishes with. The
//! repository the plan's nodes publish to is the scratch repository the lifecycle smoke
//! uses, and every pull request the journey opened there is closed, with its branch, when
//! it ends, however it ends.
//!
//! What it does not hold anybody to is a verdict GitHub alone decides: the scratch repository
//! runs no checks, and each node is read for whatever settlement it reached rather than for a
//! particular one. Two hosts launching at once are recorded, and held only to what the
//! claim and the adoption of an open change request guarantee.

// llmlint: ignore-file[e2e_not_mocked] nothing on the layer under test is substituted: the
// compiled binary reads and writes its plan store through the linked `onetaskgraph`, and the
// real `onevcs` publishes to a real GitHub repository through real `git` and `gh`. The only
// stand-in is the paid model turn, at oneharness's own documented per-harness override, as
// `main.rs` states for this whole tier.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::harness::{agent, plan_of, World};
use crate::repo::{ensure_repo, gh, gh_try, missing, Cause};
use crate::{authenticated_user, git, git_credentials, run_stamp, scratch_repo};

/// The alias every host registers its clone of the scratch repository under, and so the
/// `repo` the plan's lifecycle nodes name: a cold host has a clone of its own.
const ALIAS: &str = "continue-smoke";

/// The idle claim check every driver here runs under, shortened through entry 119's test
/// seam so a driver with nothing to dispatch notices a takeover inside the journey.
const IDLE_CHECK_SECONDS: &str = "5";

/// How long a step may take against the real services before the journey gives up on it.
const PATIENCE: Duration = Duration::from_secs(120);

/// Every task of `project` as the shared store holds it.
fn tasks(world: &World, project: &str) -> Vec<Value> {
    world.store_tasks(project)
}

/// The stored task for node `node` of `project`, by its `onepipeline.id`.
fn task(world: &World, project: &str, node: &str) -> Value {
    tasks(world, project)
        .into_iter()
        .find(|task| task["item"]["metadata"]["onepipeline.id"] == node)
        .unwrap_or_else(|| panic!("the store holds no task for {node}"))
}

/// The project's `onepipeline.driver`, `null` where it holds none.
///
/// Asked again where the store answered in part — no item, or the item beside a source
/// failure — rather than read as holding no claim: a partial answer says nothing about the
/// claim, and the journey reads it while two drivers are writing the same document.
fn claim(world: &World, project: &str) -> Value {
    let mut said = String::new();
    for _ in 0..5 {
        match world.store_answers_project(project) {
            Ok(answer) if answer.items.len() == 1 && answer.errors.is_empty() => {
                let project = serde_json::to_value(&answer.items).expect("a project renders");
                return project[0]["item"]["metadata"]
                    .get("onepipeline.driver")
                    .cloned()
                    .unwrap_or(Value::Null);
            }
            Ok(answer) => {
                said = format!("{} item(s), errors {:?}", answer.items.len(), answer.errors);
            }
            Err(why) => said = why,
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    panic!("the store answered for its project only in part: {said}");
}

/// What one invocation of the journey opened on the scratch repository: the pull requests,
/// by number, and the branches every host recorded publishing under the run's prefix.
#[derive(Default)]
pub(crate) struct Tracked {
    pub(crate) pulls: BTreeSet<u64>,
    pub(crate) branches: BTreeSet<String>,
}

impl Tracked {
    /// Add what `world` recorded for `run`: every `branch`, `url` and `change_url` its
    /// journal's `onevcs` records and settlements carry, wherever in the payload they sit.
    pub(crate) fn read(&mut self, world: &World, run: &str, slug: &str, prefix: &str) {
        let pulls = format!("https://github.com/{slug}/pull/");
        for event in world.journal(run) {
            if event["source"] != "vcs" && event["kind"] != "node-settled" {
                continue;
            }
            let mut pending = vec![&event["payload"]];
            while let Some(value) = pending.pop() {
                match value {
                    Value::Object(fields) => {
                        for (key, value) in fields {
                            match (key.as_str(), value.as_str()) {
                                ("branch", Some(branch)) if branch.starts_with(prefix) => {
                                    self.branches.insert(branch.to_owned());
                                }
                                ("url" | "change_url", Some(url)) => {
                                    if let Some(number) = url
                                        .strip_prefix(&pulls)
                                        .and_then(|rest| rest.parse::<u64>().ok())
                                    {
                                        self.pulls.insert(number);
                                    }
                                }
                                _ => pending.push(value),
                            }
                        }
                    }
                    Value::Array(values) => pending.extend(values),
                    _ => {}
                }
            }
        }
    }
}

/// What the journey created on the scratch repository, and its cleanup.
///
/// [`Created::clean`] is the journey's own last step: it closes every pull request the hosts
/// recorded opening and every open one whose title carries this run's stamp, deletes every
/// branch under the run's prefix, and then reads each one back from GitHub to show it closed
/// or gone — answering every failure it met rather than counting it as done. Its `Drop` runs
/// the same sweep, best-effort, for a journey that failed before that step: declared before
/// every world, it drops after them, once each has stopped the drivers it started, which
/// would otherwise go on publishing after it was cleaned. A failure there is printed, since
/// a panic while unwinding would abort the report of the one that got it there.
pub(crate) struct Created {
    pub(crate) slug: String,
    pub(crate) stamp: String,
    /// The prefixes every branch of the run is cut under — here the run's id and a `/`, once
    /// the first launch has minted it; on a board, each task's issue number and a `/`.
    pub(crate) prefixes: Vec<String>,
    pub(crate) cleaned: bool,
}

/// What a cleanup met: each step it took, and every failure.
#[derive(Default)]
pub(crate) struct Cleanup {
    pub(crate) observed: Vec<String>,
    pub(crate) failures: Vec<String>,
}

impl Cleanup {
    fn failed(&mut self, what: impl std::fmt::Display) {
        self.failures.push(what.to_string());
    }
}

impl Created {
    /// Close and delete everything `tracked` names and everything the stamp and the prefixes
    /// find, then read each back.
    pub(crate) fn clean(&mut self, tracked: &Tracked) -> Cleanup {
        self.cleaned = true;
        let mut cleanup = Cleanup::default();
        let mut pulls = tracked.pulls.clone();
        let stamped = self.stamped_open(&mut cleanup);
        let untracked: Vec<u64> = stamped.difference(&tracked.pulls).copied().collect();
        if !tracked.pulls.is_empty() && !untracked.is_empty() {
            cleanup.failed(format!(
                "open pull requests carrying the stamp that no host recorded opening: {untracked:?}"
            ));
        }
        pulls.extend(stamped);
        let mut branches = tracked.branches.clone();
        // Highest first, so a stacked pull request is closed before the one under it.
        for number in pulls.iter().rev() {
            let Some((state, head)) = self.pull(*number, &mut cleanup) else {
                continue;
            };
            branches.insert(head);
            if state == "OPEN" {
                match gh_try(&["pr", "close", &number.to_string(), "--repo", &self.slug]) {
                    Ok(_) => cleanup.observed.push(format!("closed #{number}")),
                    Err(refusal) => cleanup.failed(format!("closing #{number}: {refusal}")),
                }
            } else {
                cleanup
                    .observed
                    .push(format!("#{number} was already {state}"));
            }
        }
        branches.extend(self.remote_branches(&mut cleanup));
        for branch in &branches {
            if !self
                .prefixes
                .iter()
                .any(|prefix| branch.starts_with(prefix.as_str()))
            {
                cleanup.failed(format!(
                    "{branch} is outside the run's prefixes {:?}, so it is not deleted",
                    self.prefixes
                ));
                continue;
            }
            match self.branch_exists(branch, &mut cleanup) {
                Some(true) => match gh_try(&[
                    "api",
                    "-X",
                    "DELETE",
                    &format!("repos/{}/git/refs/heads/{branch}", self.slug),
                ]) {
                    Ok(_) => cleanup.observed.push(format!("deleted {branch}")),
                    Err(refusal) => cleanup.failed(format!("deleting {branch}: {refusal}")),
                },
                Some(false) => cleanup.observed.push(format!("{branch} was already gone")),
                None => {}
            }
        }
        // Read back: every pull request closed, every branch gone, nothing left to find.
        let mut closed = Vec::new();
        for number in &pulls {
            match self.pull(*number, &mut cleanup) {
                Some((state, _)) if state == "CLOSED" => closed.push(*number),
                Some((state, _)) => {
                    cleanup.failed(format!("#{number} reads {state} after the cleanup"));
                }
                None => {}
            }
        }
        let mut gone = Vec::new();
        for branch in &branches {
            match self.branch_exists(branch, &mut cleanup) {
                Some(false) => gone.push(branch.as_str()),
                Some(true) => {
                    cleanup.failed(format!("{branch} is still on the remote after the cleanup"));
                }
                None => {}
            }
        }
        let left = self.stamped_open(&mut cleanup);
        if !left.is_empty() {
            cleanup.failed(format!("still open after the cleanup: {left:?}"));
        }
        let remaining = self.remote_branches(&mut cleanup);
        if !remaining.is_empty() {
            cleanup.failed(format!(
                "branches under the prefix after the cleanup: {remaining:?}"
            ));
        }
        cleanup.observed.push(format!(
            "read back from GitHub: closed {closed:?} of {pulls:?}; gone {gone:?} of {branches:?}"
        ));
        cleanup
    }

    /// The open pull requests on the scratch repository whose title carries the stamp.
    fn stamped_open(&self, cleanup: &mut Cleanup) -> BTreeSet<u64> {
        let listed = match gh_try(&[
            "pr",
            "list",
            "--repo",
            &self.slug,
            "--state",
            "open",
            "--limit",
            "200",
            "--json",
            "number,title",
        ]) {
            Ok(listed) => listed,
            Err(refusal) => {
                cleanup.failed(format!("listing open pull requests: {refusal}"));
                return BTreeSet::new();
            }
        };
        let listed: Vec<Value> = match serde_json::from_str(&listed) {
            Ok(listed) => listed,
            Err(why) => {
                cleanup.failed(format!("reading the open pull requests: {why}: {listed}"));
                return BTreeSet::new();
            }
        };
        listed
            .iter()
            .filter(|pull| {
                pull["title"]
                    .as_str()
                    .is_some_and(|title| title.contains(&self.stamp))
            })
            .filter_map(|pull| pull["number"].as_u64())
            .collect()
    }

    /// Pull request `number`'s state and head branch, or the failure that kept them.
    fn pull(&self, number: u64, cleanup: &mut Cleanup) -> Option<(String, String)> {
        let read = gh_try(&[
            "pr",
            "view",
            &number.to_string(),
            "--repo",
            &self.slug,
            "--json",
            "state,headRefName",
        ]);
        let read = match read {
            Ok(read) => read,
            Err(refusal) => {
                cleanup.failed(format!("reading #{number}: {refusal}"));
                return None;
            }
        };
        let pull: Value = match serde_json::from_str(&read) {
            Ok(pull) => pull,
            Err(why) => {
                cleanup.failed(format!("reading #{number}: {why}: {read}"));
                return None;
            }
        };
        match (pull["state"].as_str(), pull["headRefName"].as_str()) {
            (Some(state), Some(head)) => Some((state.to_owned(), head.to_owned())),
            _ => {
                cleanup.failed(format!("#{number} names no state or head: {pull}"));
                None
            }
        }
    }

    /// Whether `branch` is on the remote: `None` where GitHub's answer said neither.
    fn branch_exists(&self, branch: &str, cleanup: &mut Cleanup) -> Option<bool> {
        match gh_try(&[
            "api",
            &format!("repos/{}/git/ref/heads/{branch}", self.slug),
        ]) {
            Ok(_) => Some(true),
            Err(refusal) if refusal.cause == Cause::NotFound => Some(false),
            Err(refusal) => {
                cleanup.failed(format!("reading {branch}: {refusal}"));
                None
            }
        }
    }

    /// Every branch on the remote under one of the run's prefixes.
    fn remote_branches(&self, cleanup: &mut Cleanup) -> BTreeSet<String> {
        let mut branches = BTreeSet::new();
        for prefix in &self.prefixes {
            let listed = match gh_try(&[
                "api",
                &format!("repos/{}/git/matching-refs/heads/{prefix}", self.slug),
            ]) {
                Ok(listed) => listed,
                Err(refusal) => {
                    cleanup.failed(format!("listing branches under {prefix}: {refusal}"));
                    continue;
                }
            };
            let refs: Vec<Value> = match serde_json::from_str(&listed) {
                Ok(refs) => refs,
                Err(why) => {
                    cleanup.failed(format!("reading branches under {prefix}: {why}: {listed}"));
                    continue;
                }
            };
            branches.extend(
                refs.iter()
                    .filter_map(|reference| reference["ref"].as_str())
                    .filter_map(|reference| reference.strip_prefix("refs/heads/"))
                    .filter(|branch| branch.starts_with(prefix.as_str()))
                    .map(str::to_owned),
            );
        }
        branches
    }
}

impl Drop for Created {
    fn drop(&mut self) {
        if self.cleaned {
            return;
        }
        let cleanup = self.clean(&Tracked::default());
        for step in &cleanup.observed {
            eprintln!("smoke: cleanup after a failed journey: {step}");
        }
        for failure in &cleanup.failures {
            eprintln!("smoke: cleanup after a failed journey FAILED: {failure}");
        }
    }
}

/// The open pull requests whose head is `branch` on `slug`; a listing GitHub refused, or one
/// that does not read, fails the journey rather than reading as none.
pub(crate) fn open_pulls(slug: &str, branch: &str) -> Vec<Value> {
    let listed = gh_try(&[
        "pr",
        "list",
        "--repo",
        slug,
        "--head",
        branch,
        "--state",
        "open",
        "--json",
        "number,url,isDraft,baseRefName",
    ])
    .unwrap_or_else(|refusal| panic!("listing the pull requests from {branch}: {refusal}"));
    serde_json::from_str(&listed)
        .unwrap_or_else(|why| panic!("reading the pull requests from {branch}: {why}: {listed}"))
}

/// One host of the journey: a world of its own, with a clone of the scratch repository
/// registered under [`ALIAS`], and the publication rules a team's drafts are kept under. Its
/// store is the world's own, which every host made from one world shares.
pub(crate) fn host(world: World, slug: &str) -> World {
    git_credentials(&world);
    let at = world
        .home
        .as_ref()
        .and_then(|home| home.parent())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| world.root.clone());
    let checkout = at.join(ALIAS);
    if !checkout.exists() {
        git(
            &world,
            &at,
            &["clone", &format!("https://github.com/{slug}.git"), ALIAS],
        );
    }
    let home = world.onevcs_home();
    std::fs::create_dir_all(&home).expect("a state root");
    // A team's repository: published as a draft, which is kept for its user's review.
    std::fs::write(
        home.join("rules.yml"),
        "version: 3\nrules: []\ndefault:\n  publication: change-open\n  approvals: required\n",
    )
    .expect("the rules file is written");
    world.register(&checkout, None);
    assert_eq!(
        world.identity(&checkout).origin,
        format!("github.com/{slug}"),
        "the registered checkout does not resolve to the scratch repository"
    );
    world
}

/// The credential `gh` holds for this user: what every `gh` a driver runs authenticates
/// with, since a world points `XDG_CONFIG_HOME` — where `gh` keeps its own login — at itself.
pub(crate) fn gh_token() -> String {
    std::env::var("GH_TOKEN")
        .ok()
        .filter(|token| !token.trim().is_empty())
        .unwrap_or_else(|| gh(&["auth", "token"]).trim().to_owned())
}

/// Run `onepipeline` on `world` against the real services, from its project directory.
pub(crate) fn onepipeline(world: &World, args: &[&str]) -> Command {
    let mut command = world.real_cmd(args);
    command
        .current_dir(&world.project)
        .env("GH_TOKEN", gh_token())
        .env("GIT_CONFIG_GLOBAL", git_credentials(world))
        .env("ONEPIPELINE_IDLE_CLAIM_CHECK_SECONDS", IDLE_CHECK_SECONDS);
    command
}

pub(crate) fn ran(command: &mut Command) -> Output {
    command.output().expect("the binary runs")
}

pub(crate) fn said(output: &Output) -> String {
    format!(
        "exit {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Wait for `ready`, reading the real services, or fail naming `what`.
pub(crate) fn until(what: &str, ready: impl FnMut() -> bool) {
    until_or(what, ready, String::new);
}

/// [`until`], failing with what `why` says the hosts recorded.
pub(crate) fn until_or(what: &str, mut ready: impl FnMut() -> bool, why: impl Fn() -> String) {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if ready() {
            return;
        }
        std::thread::sleep(Duration::from_secs(3));
    }
    panic!("timed out waiting for {what}\n{}", why());
}

/// What a run's driver said and what its projections recorded, for a failure to name.
pub(crate) fn recorded(world: &World, run: &str) -> String {
    let read = |file: &str| std::fs::read_to_string(world.run_file(run, file)).unwrap_or_default();
    let log = read("driver.log");
    let tail: Vec<&str> = log.lines().rev().take(20).collect();
    let projections = read("writeback-projections.jsonl");
    let last: Vec<&str> = projections.lines().rev().take(5).collect();
    format!(
        "driver.log, newest first:\n{}\nprojections, newest first:\n{}",
        tail.join("\n"),
        last.join("\n")
    )
}

/// The node ids `run` dispatched in `world`.
fn dispatched(world: &World, run: &str) -> Vec<String> {
    world
        .events_of(run, "node-dispatched")
        .iter()
        .filter_map(|event| event["labels"]["node"].as_str().map(str::to_owned))
        .collect()
}

/// A lifecycle node of the plan, publishing to the scratch repository under [`ALIAS`].
pub(crate) fn change(id: &str, deps: &[&str], stamp: &str) -> Value {
    json!({
        "id": id,
        "repo": ALIAS,
        "persona": "engineer",
        "title": format!("feat: continue {id} ({stamp})"),
        "task": format!("## What\nContinue {id} for {stamp}.\n\n## Why\nProve a project carries \
                         on from its store.\n\n## Acceptance criteria\n- {id} is drafted."),
        "deps": deps,
    })
}

/// A project on a shared plan store, drafted by one host, is refused to a cold host while that
/// host's claim stands, taken over by it while the first driver is still live — which stops
/// at its next claim check — and carried on from the store alone: no node the first host
/// settled `done` is dispatched again, a failed one is continued on its branch, and a node
/// depending on the stacked one is drafted on that one's branch. Then two hosts launch the
/// drafted project at the same moment, and every draft is still the one pull request open
/// from its branch.
#[test]
fn a_stored_project_is_carried_on_by_a_cold_host_that_takes_it_over() {
    let slug = scratch_repo();
    authenticated_user();
    ensure_repo(&slug);
    let stamp = run_stamp();
    let mut residue = Created {
        slug: slug.clone(),
        stamp: stamp.clone(),
        prefixes: Vec::new(),
        cleaned: false,
    };

    // Its host's name on every command it runs, a `stop` its drop sends included: a driver
    // is reached only from the host it records.
    let first = host(
        World::new("smoke-continue").with_env("HOSTNAME", "smoke-host-a"),
        &slug,
    );
    first.write_graphs();
    assert!(
        !World::substitutes_the_host(&first.real_cmd(&["start"])),
        "{}",
        missing("this run still carries a stand-in for `gh`, so it would never reach GitHub")
    );
    first.script("harness.work", &format!("continued from the store {stamp}"));
    first.script("harness.work-per-task", "");
    let name = format!("continue-{}", stamp.replace(['_', '.'], "-"));
    let project = first.plan(
        &name,
        &plan_of(
            &name,
            vec![
                change("a", &[], &stamp),
                change("b", &["a"], &stamp),
                agent("hold", &["b"]),
                change("c", &["b", "hold"], &stamp),
            ],
        ),
    );
    println!("smoke: the plan is on the shared store as {project}");

    // The first host drafts the stack and holds the agent node above it in flight.
    let launched = ran(&mut onepipeline(&first, &["start", &project, "--detach"]));
    assert!(
        launched.status.success(),
        "the first launch: {}",
        said(&launched)
    );
    let run = std::fs::read_dir(&first.runs)
        .expect("the runs root")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .next()
        .expect("the launch minted a run");
    residue.prefixes = vec![format!("{run}/")];
    let mut saw_dispatch = None;
    let early = |what: &str| {
        if first.run_file(&run, "result.json").is_file() {
            let settled: Vec<String> = first
                .events_of(&run, "node-settled")
                .iter()
                .map(|event| format!("{} {}", event["labels"]["node"], event["payload"]))
                .collect();
            panic!("the first run ended before {what}: {settled:#?}");
        }
    };
    until("the stacked node's turn", || {
        early("its stacked node's turn");
        if saw_dispatch.is_none() {
            saw_dispatch = tasks(&first, &project).into_iter().find_map(|task| {
                task["item"]["metadata"]
                    .get("onepipeline.dispatch")
                    .cloned()
            });
        }
        first
            .invocations()
            .iter()
            .filter(|call| {
                call["tool"]
                    .as_str()
                    .is_some_and(|tool| tool.ends_with("-work"))
            })
            .count()
            >= 2
    });
    first.script("turn.hold", "");
    until(
        "the first host to settle the stack and start the node above it",
        || {
            early("it started the node above the stack");
            dispatched(&first, &run).contains(&"hold".to_owned())
        },
    );
    println!(
        "smoke: a dispatch key the store carried while the first host worked: {saw_dispatch:?}"
    );
    until_or(
        "the store to hold both settlements",
        || {
            ["a", "b"].iter().all(|node| {
                task(&first, &project, node)["item"]["metadata"]
                    .get("onepipeline.settlement")
                    .is_some_and(Value::is_object)
            })
        },
        || recorded(&first, &run),
    );
    let stored: BTreeMap<String, Value> = ["a", "b"]
        .into_iter()
        .map(|node| {
            let task = task(&first, &project, node);
            (
                node.to_owned(),
                task["item"]["metadata"]["onepipeline.settlement"].clone(),
            )
        })
        .collect();
    println!("smoke: the store's settlements: {stored:#?}");
    assert_eq!(
        stored["b"]["stack"]["parents"][0]["branch"], stored["a"]["branch"],
        "the store's settlement of b names no stack anchor on a: {stored:#?}"
    );
    let held = claim(&first, &project);
    assert_eq!(held["run"], json!(run), "{held}");
    assert_eq!(held["host"], "smoke-host-a", "{held}");

    // A cold host is refused while that claim stands, then takes the project over.
    let second = host(first.on_host("smoke-host-b"), &slug);
    let refused = ran(&mut onepipeline(&second, &["start", &project, "--attach"]));
    assert_eq!(refused.status.code(), Some(2), "{}", said(&refused));
    let refusal = String::from_utf8_lossy(&refused.stderr);
    assert!(
        refusal.contains("project driven elsewhere refused:")
            && refusal.contains(&format!("run '{run}'"))
            && refusal.contains("--take-over"),
        "{refusal}"
    );
    let taken = ran(&mut onepipeline(
        &second,
        &["start", &project, "--detach", "--take-over"],
    ));
    assert!(taken.status.success(), "the takeover: {}", said(&taken));
    until("the second host's claim to reach the store", || {
        claim(&first, &project)["host"] == "smoke-host-b"
    });
    let claimed = claim(&first, &project);
    println!("smoke: the store's claim after the takeover: {claimed}");
    let since = Instant::now();
    until("the first driver to notice the takeover", || {
        !first.events_of(&run, "driver-superseded").is_empty()
    });
    println!(
        "smoke: the first driver noticed the takeover {:?} after the claim reached the store",
        since.elapsed()
    );
    let journal = first.journal(&run);
    let checked = journal
        .iter()
        .position(|event| event["kind"] == "driver-superseded")
        .expect("superseded");
    assert_eq!(journal[checked]["payload"]["by"], claimed);
    let after: Vec<&Value> = journal[checked..]
        .iter()
        .filter(|event| {
            event["kind"] == "node-dispatched"
                || (event["source"] == "vcs" && event["kind"] == "change-opened")
        })
        .collect();
    assert!(
        after.is_empty(),
        "the superseded driver started work after its check: {after:?}"
    );
    let taken_over = second.events_of(&run, "driver-taken-over");
    assert_eq!(taken_over[0]["payload"]["previous"], held);

    // Let the node above the stack go: the first driver ends, the second carries on.
    first.release("turn.go");
    first.release("turn.settle");
    until("the first driver to end", || {
        first.run_file(&run, "result.json").is_file()
    });
    until("the second driver to settle", || {
        second.run_file(&run, "result.json").is_file()
    });
    let second_dispatched = dispatched(&second, &run);
    println!("smoke: the second host dispatched {second_dispatched:?}");
    for (node, settlement) in &stored {
        match settlement["status"].as_str() {
            Some("done") => {
                assert!(
                    !second_dispatched.contains(node),
                    "{node}, settled done, was dispatched again"
                );
                let continued = second
                    .events_of(&run, "node-settled")
                    .into_iter()
                    .find(|event| event["labels"]["node"] == *node)
                    .expect("continued");
                assert_eq!(continued["payload"]["continued"]["source"], "store");
                assert_eq!(continued["payload"]["branch"], settlement["branch"]);
                assert_eq!(continued["payload"]["change_url"], settlement["change_url"]);
            }
            other => {
                println!("smoke: {node} had settled {other:?} on the first host");
                if let Some(branch) = settlement["branch"].as_str() {
                    let opened: Vec<String> = second
                        .journal(&run)
                        .iter()
                        .filter(|event| {
                            event["source"] == "vcs" && event["kind"] == "session-opened"
                        })
                        .filter(|event| event["labels"]["node"] == *node)
                        .filter_map(|event| event["payload"]["branch"].as_str().map(str::to_owned))
                        .collect();
                    assert!(opened.iter().all(|opened| opened == branch), "{opened:?}");
                }
            }
        }
        if let Some(branch) = settlement["branch"].as_str() {
            let open = open_pulls(&slug, branch);
            assert_eq!(open.len(), 1, "{branch}: {open:?}");
            assert_eq!(open[0]["isDraft"], true, "{branch}: {open:?}");
        }
    }
    let c = second
        .events_of(&run, "node-settled")
        .into_iter()
        .rfind(|event| event["labels"]["node"] == "c")
        .expect("c settled on the second host");
    println!("smoke: c settled {}", c["payload"]);
    assert_eq!(
        c["payload"]["base"], stored["b"]["branch"],
        "c was not placed on b's recorded branch"
    );
    assert_eq!(
        claim(&first, &project),
        Value::Null,
        "the second driver left its claim"
    );

    // Two hosts launch the drafted project at the same moment, with one more node to keep
    // each driving long enough to read the claim.
    first.add_task(
        &name,
        "004-linger",
        &json!({"id": "linger", "persona": "engineer",
                "task": "## What\nLinger.\n\n## Why\nKeep a driver driving.\n\n## Acceptance criteria\n- It lingered."}),
        &[],
    );
    for gate in ["turn.go", "turn.settle"] {
        let _ = std::fs::remove_file(first.fakes.join(gate));
    }
    first.script("turn.hold", "");
    let racing: Vec<World> = ["smoke-host-c", "smoke-host-d"]
        .into_iter()
        .map(|name| host(first.on_host(name), &slug))
        .collect();
    let children: Vec<std::process::Child> = racing
        .iter()
        .map(|host| {
            onepipeline(host, &["start", &project, "--detach"])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("a launch starts")
        })
        .collect();
    let outputs: Vec<Output> = children
        .into_iter()
        .map(|child| child.wait_with_output().expect("a launch answers"))
        .collect();
    // Watched rather than slept through: what the claim says, each time it changes.
    let watching = Instant::now();
    let mut reading = Value::Null;
    while watching.elapsed() < Duration::from_secs(30) {
        let now = claim(&first, &project);
        if now != reading {
            println!(
                "smoke: {:?} into the race the store's claim reads {now}",
                watching.elapsed()
            );
            reading = now;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let mut left = 0;
    let claim_then = claim(&first, &project);
    for (host, output) in racing.iter().zip(&outputs) {
        let name = host
            .environment
            .iter()
            .rfind(|(key, _)| key == "HOSTNAME")
            .map(|(_, name)| name.clone())
            .unwrap_or_default();
        let ending = match output.status.code() {
            Some(2) => {
                assert!(
                    String::from_utf8_lossy(&output.stderr)
                        .contains("project driven elsewhere refused:"),
                    "{}",
                    said(output)
                );
                "refused with ProjectDriven".to_owned()
            }
            Some(0) if !host.events_of(&run, "driver-superseded").is_empty() => {
                "superseded".to_owned()
            }
            Some(0) => {
                left += 1;
                format!(
                    "driving to the end (settled: {})",
                    host.run_file(&run, "result.json").is_file()
                )
            }
            _ => panic!("a racing launch failed: {}", said(output)),
        };
        println!(
            "smoke: the racing launch on {name} was {ending}; it dispatched {:?}",
            dispatched(host, &run)
        );
    }
    println!("smoke: the store's claim once both had checked it: {claim_then}");
    for host in &racing {
        println!(
            "smoke: what a racing host recorded:\n{}",
            recorded(host, &run)
        );
    }
    assert!(
        left <= 1,
        "{left} racing drivers were left driving past their claim checks"
    );
    first.release("turn.go");
    first.release("turn.settle");
    for (host, output) in racing.iter().zip(&outputs) {
        if output.status.success() {
            until("a racing driver to end", || {
                host.run_file(&run, "result.json").is_file()
            });
        }
    }
    for (node, settlement) in &stored {
        if let Some(branch) = settlement["branch"].as_str() {
            let open = open_pulls(&slug, branch);
            assert_eq!(open.len(), 1, "{node} on {branch}: {open:?}");
            assert_eq!(open[0]["isDraft"], true, "{node} on {branch}: {open:?}");
        }
    }

    // The journey's own cleanup, read back through GitHub: every pull request any host
    // recorded opening is closed and every branch under the run's prefix is gone.
    let mut tracked = Tracked::default();
    let prefix = format!("{run}/");
    for world in [&first, &second].into_iter().chain(&racing) {
        tracked.read(world, &run, &slug, &prefix);
    }
    let mut published = stored.clone();
    published.insert("c".to_owned(), c["payload"].clone());
    for (node, settlement) in &published {
        if let Some(branch) = settlement["branch"].as_str() {
            assert!(
                tracked.branches.contains(branch),
                "{node}'s branch {branch} is not among what the hosts recorded: {:?}",
                tracked.branches
            );
        }
    }
    println!(
        "smoke: the hosts recorded opening pull requests {:?} from branches {:?}",
        tracked.pulls, tracked.branches
    );
    let cleanup = residue.clean(&tracked);
    for step in &cleanup.observed {
        println!("smoke: cleanup: {step}");
    }
    assert!(
        cleanup.failures.is_empty(),
        "the cleanup did not finish: {:#?}",
        cleanup.failures
    );
    drop(racing);
    drop(second);
}
