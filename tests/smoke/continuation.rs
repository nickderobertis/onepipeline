//! The live half of `docs/contract-divergences.md` entry 116: a project continued from a
//! real GitHub Projects board by a host holding none of the run's state, and the best-effort
//! claim on who drives it, against the real GitHub on both sides.
//!
//! The board is onetaskgraph's own live-lane board — `nickderobertis`'s Projects #1, whose
//! issues live in `nickderobertis/onetaskgraph-live-scratch` — and the repository the plan's
//! nodes publish to is the scratch repository the lifecycle smoke uses. Nothing else is
//! written: the store each host reads is configured here, with this board as its one source,
//! so no `plans` or `followups` source of the operator's is reachable, and everything the
//! journey created is closed or deleted when it ends, however it ends.
//!
//! What it does not hold anybody to is a verdict GitHub alone decides: the scratch repository
//! runs no checks, and each node is read for whatever settlement it reached rather than for a
//! particular one. Two hosts launching at once are recorded, and held only to what the
//! claim and the adoption of an open change request guarantee.

// llmlint: ignore-file[e2e_not_mocked] nothing on the layer under test is substituted: the
// compiled binary reads and writes the real GitHub Projects board through the linked
// `onetaskgraph`, and the real `onevcs` publishes to a real GitHub repository through real
// `git` and `gh`. The only stand-in is the paid model turn, at oneharness's own documented
// per-harness override, as `main.rs` states for this whole tier.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use onetaskgraph_core::{
    Body, Config, CopyItems, CopyRequest, CopyScope, Engine, Environment, GlobalId, Paging,
    ProjectSelector, Secrets, TaskCreate, TaskRequest,
};
use onetaskgraph_plugin_api::{MetadataKey, SourceName, StatusCategory};
use serde_json::{json, Value};

use crate::harness::{agent, plan_of, World};
use crate::repo::{ensure_repo, gh, gh_try, missing};
use crate::{authenticated_user, git, git_credentials, run_stamp, scratch_repo};

/// The board's owner, number and issue repository: onetaskgraph's live lane, never another.
const BOARD_OWNER: &str = "nickderobertis";
const BOARD_NUMBER: u32 = 1;
const BOARD_REPOSITORY: &str = "nickderobertis/onetaskgraph-live-scratch";

/// The source the board is configured under, for this journey's hosts alone.
const BOARD: &str = "board";

/// The variable the board's token is read from, as onetaskgraph's lane reads it.
const TOKEN: &str = "GH_PROJECTS_TOKEN";

/// The alias every host registers its clone of the scratch repository under, and so the
/// `repo` the plan's lifecycle nodes name: a cold host has a clone of its own.
const ALIAS: &str = "continue-smoke";

/// The idle claim check every driver here runs under, shortened through entry 116's test
/// seam so a driver with nothing to dispatch notices a takeover inside the journey.
const IDLE_CHECK_SECONDS: &str = "5";

/// How long a step may take against the real services before the journey gives up on it.
const PATIENCE: Duration = Duration::from_secs(120);

/// The board's token, or a refusal naming it: this tier never skips.
fn token() -> String {
    std::env::var(TOKEN)
        .ok()
        .filter(|token| !token.trim().is_empty())
        .unwrap_or_else(|| {
            panic!(
                "{}",
                missing(&format!(
                    "{TOKEN} is not set, and the continuation journey reads and writes \
                     onetaskgraph's live board with it"
                ))
            )
        })
}

/// The board's source configuration: its owner, number and issue repository, and the
/// board's own `Status` options — `Todo`, `In Progress`, `Done`, `Cancelled` — mapped onto
/// the categories a run writes, one per kind, since no two categories of one kind may name
/// one option: a queued task and a project waiting to start share `Todo`, and a task the run
/// settles under a word of its own — `failed` — and a cancelled project share `Cancelled`.
/// A task written `cancelled` — one whose publication a takeover withheld — has no option
/// here, and its projection is refused rather than mislabelled.
fn board_source() -> Value {
    json!({
        "plugin": "github-projects",
        "config": {
            "owner": BOARD_OWNER,
            "project_number": BOARD_NUMBER,
            "repository": BOARD_REPOSITORY,
            "token_env": TOKEN,
            "status_mapping": {
                "todo": {"project": "Todo"},
                "queued": {"task": "Todo"},
                "in-progress": "In Progress",
                "done": "Done",
                "cancelled": {"project": "Cancelled"},
                "unknown": {"task": "Cancelled"},
            },
        },
    })
}

/// The board and the plans store one world authored the plan in, linked in this process
/// for the journey's own setup, reads and cleanup — never for anything a host does.
struct Board {
    engine: Engine,
    runtime: tokio::runtime::Runtime,
}

impl Board {
    fn over(plans: &Path) -> Self {
        let config = Config::from_document(json!({
            "sources": {
                "plans": {"plugin": "local-md", "config": {"root": plans}},
                BOARD: board_source(),
            },
        }))
        .unwrap_or_else(|why| panic!("the journey's store configuration is refused: {why}"));
        let secrets = Secrets::load(Environment::from_os_pairs(std::env::vars_os()))
            .unwrap_or_else(|why| panic!("the store's secrets cannot be read: {why}"));
        Self {
            engine: Engine::build(&config, &secrets),
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime"),
        }
    }

    /// One logical command: the future, then the end of the command, so the next read is
    /// answered from the board as it is then rather than from this one's snapshot.
    fn call<T>(&self, future: impl std::future::Future<Output = T>) -> T {
        let answered = self.runtime.block_on(future);
        if let Err(why) = self.runtime.block_on(self.engine.end_command()) {
            eprintln!("smoke: the board's command did not end cleanly: {why}");
        }
        answered
    }

    /// Copy the plans store's project `local` onto the board with its tasks, and answer the
    /// board project's qualified id and every item the copy created.
    fn copy(&self, local: &str) -> (String, Vec<GlobalId>) {
        let id: GlobalId = local.parse().expect("a qualified project");
        let request = CopyRequest {
            items: CopyItems::new(vec![id.clone()]).expect("one item"),
            scope: CopyScope::Projects { tasks: true },
            destination: SourceName::new(BOARD).expect("a source name"),
            match_by: None,
            recreate: false,
            create: true,
            dry_run: false,
        };
        let report = self
            .call(self.engine.copy(&request))
            .unwrap_or_else(|why| panic!("the board refused the copy: {why}"));
        let report = serde_json::to_value(&report).expect("a report renders");
        let mut created = Vec::new();
        let mut project = None;
        for item in report["items"]
            .as_array()
            .expect("the copy names its items")
        {
            let landed = item["destination"]
                .as_str()
                .unwrap_or_else(|| panic!("the copy created no counterpart: {item}"));
            let landed: GlobalId = landed.parse().expect("a qualified destination");
            if item["source"] == json!(local) {
                project = Some(landed.to_string());
            }
            created.push(landed);
        }
        (
            project.unwrap_or_else(|| panic!("the copy created no project: {report}")),
            created,
        )
    }

    /// Every task of the board project `project`, as the store renders it.
    fn tasks(&self, project: &str) -> Vec<Value> {
        let id: GlobalId = project.parse().expect("a qualified project");
        let request = TaskRequest {
            sources: Vec::new(),
            filters: onetaskgraph_core::Filters::default(),
            priorities: Vec::new(),
            metadata: Vec::new(),
            origin: None,
            project: ProjectSelector::Qualified(id),
            commented_since: None,
            include_members: false,
            paging: Paging {
                limit: std::num::NonZeroU32::new(100).expect("not zero"),
                token: None,
            },
        };
        let page = self
            .call(self.engine.tasks(&request))
            .unwrap_or_else(|why| panic!("the board's tasks cannot be read: {why}"));
        page.items
            .iter()
            .map(|task| serde_json::to_value(task).expect("a task renders"))
            .collect()
    }

    /// The board item for node `node` of `project`, by its `onepipeline.id`.
    fn task(&self, project: &str, node: &str) -> Value {
        self.tasks(project)
            .into_iter()
            .find(|task| task["item"]["metadata"]["onepipeline.id"] == node)
            .unwrap_or_else(|| panic!("the board holds no item for {node}"))
    }

    /// The project's `onepipeline.driver`, `null` where it holds none.
    ///
    /// Asked again where the board answered in part — no item, or the item beside a source
    /// failure — rather than read as holding no claim: a partial answer says nothing about
    /// the claim, and reading it as `null` reported a driver gone that was still driving.
    fn claim(&self, project: &str) -> Value {
        let id: GlobalId = project.parse().expect("a qualified project");
        let mut said = String::new();
        for _ in 0..5 {
            let answer = self
                .call(self.engine.project(&id))
                .unwrap_or_else(|why| panic!("the board's project cannot be read: {why}"));
            if answer.items.len() == 1 && answer.errors.is_empty() {
                let project = serde_json::to_value(&answer.items).expect("a project renders");
                return project[0]["item"]["metadata"]
                    .get("onepipeline.driver")
                    .cloned()
                    .unwrap_or(Value::Null);
            }
            said = format!("{} item(s), errors {:?}", answer.items.len(), answer.errors);
            std::thread::sleep(Duration::from_secs(1));
        }
        panic!("the board answered for its project only in part: {said}");
    }

    /// File one more node under `project`: the task a planner adds on the board.
    fn add(&self, project: &str, node: &Value) -> GlobalId {
        let id: GlobalId = project.parse().expect("a qualified project");
        let mut metadata = BTreeMap::new();
        for (key, value) in node.as_object().expect("a node") {
            if key == "task" || key == "title" || key == "deps" {
                continue;
            }
            metadata.insert(
                MetadataKey::new(format!("onepipeline.{key}")).expect("a key"),
                value.clone(),
            );
        }
        let request = TaskCreate {
            source: SourceName::new(BOARD).expect("a source name"),
            project: id.native.clone(),
            title: node["title"].as_str().unwrap_or("linger").to_owned(),
            body: Body::plain(node["task"].as_str().expect("a task")),
            status: Some(StatusCategory::Queued),
            labels: Vec::new(),
            repositories: Vec::new(),
            depends_on: Vec::new(),
            delivers: Vec::new(),
            metadata,
            assets: Vec::new(),
            classification: Default::default(),
        };
        let created = self
            .call(self.engine.create_task(&request))
            .unwrap_or_else(|why| panic!("the board refused the new task: {why}"));
        created.task.id
    }
}

/// What the journey created, closed or deleted however it ends: every issue on the board
/// and every pull request — with its branch — on the scratch repository whose title carries
/// this run's stamp.
///
/// Found by listing rather than by what the journey recorded as it went, so a journey that
/// failed before it read back what it made still leaves nothing behind; and declared before
/// every world, so it drops after them — once each world has stopped the drivers it started,
/// which would otherwise go on writing to the board after it was cleaned.
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
        let token = std::env::var(TOKEN).unwrap_or_default();
        let issues = Command::new("gh")
            .args([
                "issue",
                "list",
                "--repo",
                BOARD_REPOSITORY,
                "--state",
                "open",
                "--limit",
                "100",
                "--json",
                "number,title",
            ])
            .env("GH_TOKEN", &token)
            .stdin(std::process::Stdio::null())
            .output()
            .ok()
            .and_then(|listed| serde_json::from_slice::<Vec<Value>>(&listed.stdout).ok())
            .unwrap_or_default();
        for issue in issues {
            if !issue["title"]
                .as_str()
                .is_some_and(|title| title.contains(&self.stamp))
            {
                continue;
            }
            let number = issue["number"].to_string();
            let closed = Command::new("gh")
                .args([
                    "issue",
                    "close",
                    &number,
                    "--repo",
                    BOARD_REPOSITORY,
                    "--reason",
                    "not planned",
                ])
                .env("GH_TOKEN", &token)
                .stdin(std::process::Stdio::null())
                .output();
            if !closed.is_ok_and(|closed| closed.status.success()) {
                eprintln!("smoke: could not close board issue {number}");
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
/// registered under [`ALIAS`], the publication rules a team's drafts are kept under, and the
/// board configured as its store.
fn host(world: World, slug: &str, store: &Value) -> World {
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
    std::fs::write(
        world.project.join("onetaskgraph.yaml"),
        serde_json::to_string_pretty(store).expect("the store renders"),
    )
    .expect("the store's configuration is written");
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
        // The plan read lists a project's tasks from the default sources, and the harness
        // names its own `plans` there.
        .env("ONETASKGRAPH_DEFAULT_SOURCES", BOARD)
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

/// `node`, titled: an issue on the board cannot be untitled.
fn titled(mut node: Value, title: &str) -> Value {
    node["title"] = json!(title);
    node
}

/// A lifecycle node of the plan, publishing to the scratch repository under [`ALIAS`].
fn change(id: &str, deps: &[&str], stamp: &str) -> Value {
    json!({
        "id": id,
        "repo": ALIAS,
        "persona": "engineer",
        "title": format!("feat: continue {id} ({stamp})"),
        "task": format!("## What\nContinue {id} for {stamp}.\n\n## Why\nProve a project carries \
                         on from its board.\n\n## Acceptance criteria\n- {id} is drafted."),
        "deps": deps,
    })
}

/// Author `nodes` as a project of `world`'s plans store which a copy writes onto the board's
/// own options: the project as `todo` and its tasks as `queued`.
fn author(world: &World, name: &str, nodes: Vec<Value>) -> String {
    let local = world.plan(name, &plan_of(name, nodes));
    let file = world
        .store()
        .join("projects")
        .join(format!("{name}-board.md"));
    let text = std::fs::read_to_string(&file).expect("the authored project");
    std::fs::write(&file, text.replacen("---\n", "---\nstatus: todo\n", 1))
        .expect("the project is written as todo");
    let tasks = world.store().join("tasks").join(format!("{name}-board"));
    for entry in std::fs::read_dir(&tasks).expect("the authored tasks") {
        let path = entry.expect("a task").path();
        let text = std::fs::read_to_string(&path).expect("a task document");
        let queued = text.replacen("---\n", "---\nstatus: queued\n", 1);
        std::fs::write(&path, queued).expect("the task is queued");
    }
    local
}

/// A project on the live board, drafted by one host, is refused to a cold host while that
/// host's claim stands, taken over by it while the first driver is still live — which stops
/// at its next claim check — and carried on from the board alone: no node the first host
/// settled `done` is dispatched again, a failed one is continued on its branch, and a node
/// depending on the stacked one is drafted on that one's branch. Then two hosts launch the
/// drafted project at the same moment, and every draft is still the one pull request open
/// from its branch.
#[test]
fn a_board_project_is_carried_on_by_a_cold_host_that_takes_it_over() {
    let slug = scratch_repo();
    authenticated_user();
    token();
    ensure_repo(&slug);
    let stamp = run_stamp();
    let store = json!({"sources": {BOARD: board_source()}});
    let _residue = Created {
        slug: slug.clone(),
        stamp: stamp.clone(),
    };

    // Its host's name on every command it runs, a `stop` its drop sends included: a driver
    // is reached only from the host it records.
    let first = host(
        World::new("smoke-continue").with_env("HOSTNAME", "smoke-host-a"),
        &slug,
        &store,
    );
    first.write_graphs();
    assert!(
        !World::substitutes_the_host(&first.real_cmd(&["start"])),
        "{}",
        missing("this run still carries a stand-in for `gh`, so it would never reach GitHub")
    );
    first.script("harness.work", &format!("continued from the board {stamp}"));
    first.script("harness.work-per-task", "");
    let name = format!("continue-{}", stamp.replace(['_', '.'], "-"));
    let local = author(
        &first,
        &name,
        vec![
            change("a", &[], &stamp),
            change("b", &["a"], &stamp),
            titled(agent("hold", &["b"]), &format!("hold the stack ({stamp})")),
            change("c", &["b", "hold"], &stamp),
        ],
    );
    let board = Board::over(&first.store());
    let (project, _) = board.copy(&local);
    println!("smoke: the plan is on the board as {project}");

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
            saw_dispatch = board.tasks(&project).into_iter().find_map(|task| {
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
        "smoke: a dispatch key the board carried while the first host worked: {saw_dispatch:?}"
    );
    until_or(
        "the board to hold both settlements",
        || {
            ["a", "b"].iter().all(|node| {
                board.task(&project, node)["item"]["metadata"]
                    .get("onepipeline.settlement")
                    .is_some_and(Value::is_object)
            })
        },
        || recorded(&first, &run),
    );
    let stored: BTreeMap<String, Value> = ["a", "b"]
        .into_iter()
        .map(|node| {
            let task = board.task(&project, node);
            (
                node.to_owned(),
                task["item"]["metadata"]["onepipeline.settlement"].clone(),
            )
        })
        .collect();
    println!("smoke: the board's settlements: {stored:#?}");
    assert_eq!(
        stored["b"]["stack"]["parents"][0]["branch"], stored["a"]["branch"],
        "the board's settlement of b names no stack anchor on a: {stored:#?}"
    );
    let claim = board.claim(&project);
    assert_eq!(claim["run"], json!(run), "{claim}");
    assert_eq!(claim["host"], "smoke-host-a", "{claim}");

    // A cold host is refused while that claim stands, then takes the project over.
    let second = host(first.on_host("smoke-host-b"), &slug, &store);
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
    until("the second host's claim to reach the board", || {
        board.claim(&project)["host"] == "smoke-host-b"
    });
    let claimed = board.claim(&project);
    println!("smoke: the board's claim after the takeover: {claimed}");
    let since = Instant::now();
    until("the first driver to notice the takeover", || {
        !first.events_of(&run, "driver-superseded").is_empty()
    });
    println!(
        "smoke: the first driver noticed the takeover {:?} after the claim reached the board",
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
    assert_eq!(taken_over[0]["payload"]["previous"], claim);

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
        board.claim(&project),
        Value::Null,
        "the second driver left its claim"
    );

    // Two hosts launch the drafted project at the same moment, with one more node to keep
    // each driving long enough to read the claim.
    board.add(
        &project,
        &json!({"id": "linger", "persona": "engineer", "title": format!("linger ({stamp})"),
                "task": "## What\nLinger.\n\n## Why\nKeep a driver driving.\n\n## Acceptance criteria\n- It lingered."}),
    );
    for gate in ["turn.go", "turn.settle"] {
        let _ = std::fs::remove_file(first.fakes.join(gate));
    }
    first.script("turn.hold", "");
    let racing: Vec<World> = ["smoke-host-c", "smoke-host-d"]
        .into_iter()
        .map(|name| host(first.on_host(name), &slug, &store))
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
        let now = board.claim(&project);
        if now != reading {
            println!(
                "smoke: {:?} into the race the board's claim reads {now}",
                watching.elapsed()
            );
            reading = now;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let mut left = 0;
    let claim_then = board.claim(&project);
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
    println!("smoke: the board's claim once both had checked it: {claim_then}");
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
