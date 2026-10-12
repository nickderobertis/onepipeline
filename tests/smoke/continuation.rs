//! The live half of `docs/contract-divergences.md` entry 116: a project continued from its
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

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::harness::{agent, plan_of, World};
use crate::repo::{ensure_repo, gh, gh_try, missing};
use crate::{authenticated_user, git, git_credentials, run_stamp, scratch_repo};

/// The alias every host registers its clone of the scratch repository under, and so the
/// `repo` the plan's lifecycle nodes name: a cold host has a clone of its own.
const ALIAS: &str = "continue-smoke";

/// The idle claim check every driver here runs under, shortened through entry 116's test
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

/// What the journey created on GitHub, closed however it ends: every pull request — with its
/// branch — on the scratch repository whose title carries this run's stamp.
///
/// Found by listing rather than by what the journey recorded as it went, so a journey that
/// failed before it read back what it made still leaves nothing behind; and declared before
/// every world, so it drops after them — once each world has stopped the drivers it started,
/// which would otherwise go on publishing to the repository after it was cleaned.
struct Created {
    slug: String,
    stamp: String,
}

impl Drop for Created {
    fn drop(&mut self) {
        let listed = gh_try(&[
            "pr",
            "list",
            "--repo",
            &self.slug,
            "--state",
            "open",
            "--limit",
            "100",
            "--json",
            "number,title",
        ])
        .ok()
        .and_then(|listed| serde_json::from_str::<Vec<Value>>(&listed).ok())
        .unwrap_or_default();
        for pull in listed {
            if !pull["title"]
                .as_str()
                .is_some_and(|title| title.contains(&self.stamp))
            {
                continue;
            }
            let number = pull["number"].to_string();
            if let Err(refusal) = gh_try(&[
                "pr",
                "close",
                &number,
                "--repo",
                &self.slug,
                "--delete-branch",
            ]) {
                eprintln!("smoke: could not close pull request {number}: {refusal}");
            }
        }
    }
}

/// The open pull requests whose head is `branch` on `slug`.
fn open_pulls(slug: &str, branch: &str) -> Vec<Value> {
    gh_try(&[
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
    .ok()
    .and_then(|listed| serde_json::from_str::<Vec<Value>>(&listed).ok())
    .unwrap_or_default()
}

/// One host of the journey: a world of its own, with a clone of the scratch repository
/// registered under [`ALIAS`], and the publication rules a team's drafts are kept under. Its
/// store is the world's own, which every host made from one world shares.
fn host(world: World, slug: &str) -> World {
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
fn gh_token() -> String {
    std::env::var("GH_TOKEN")
        .ok()
        .filter(|token| !token.trim().is_empty())
        .unwrap_or_else(|| gh(&["auth", "token"]).trim().to_owned())
}

/// Run `onepipeline` on `world` against the real services, from its project directory.
fn onepipeline(world: &World, args: &[&str]) -> Command {
    let mut command = world.real_cmd(args);
    command
        .current_dir(&world.project)
        .env("GH_TOKEN", gh_token())
        .env("GIT_CONFIG_GLOBAL", git_credentials(world))
        .env("ONEPIPELINE_IDLE_CLAIM_CHECK_SECONDS", IDLE_CHECK_SECONDS);
    command
}

fn ran(command: &mut Command) -> Output {
    command.output().expect("the binary runs")
}

fn said(output: &Output) -> String {
    format!(
        "exit {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Wait for `ready`, reading the real services, or fail naming `what`.
fn until(what: &str, ready: impl FnMut() -> bool) {
    until_or(what, ready, String::new);
}

/// [`until`], failing with what `why` says the hosts recorded.
fn until_or(what: &str, mut ready: impl FnMut() -> bool, why: impl Fn() -> String) {
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
fn recorded(world: &World, run: &str) -> String {
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
fn change(id: &str, deps: &[&str], stamp: &str) -> Value {
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
    let _residue = Created {
        slug: slug.clone(),
        stamp: stamp.clone(),
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
    drop(racing);
    drop(second);
}
