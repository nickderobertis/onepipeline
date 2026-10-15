//! The live half of `docs/contract-divergences.md` entry 127: review comments on a real draft,
//! found by one host, answered on a cold one continuing from a real board.
//!
//! The plan store is onetaskgraph's live-lane board — the GitHub Project `GH_PROJECTS_OWNER`
//! and `GH_PROJECTS_NUMBER` name, its issues in `GH_PROJECTS_REPOSITORY` — declared as a source
//! of this journey's own, `board`, so nothing reaches the operator's `plans` or `followups`
//! sources. The drafts are on the scratch repository the lifecycle smoke uses. Host A drafts a
//! stack of two, two review comments are posted on the bottom draft, and `onepipeline review`
//! on A surfaces both; A's runs root and `ONEVCS_HOME` are then deleted, and host B — holding
//! nothing but the board — continues the project, reworks the bottom draft for the first
//! comment and declines the second. Every pull request, branch and board issue the journey
//! made is removed when it ends, however it ends.
//!
//! What it does not hold anybody to is a verdict GitHub alone decides: the scratch repository
//! runs no checks, and each republication is read for whatever settlement it reached.

// llmlint: ignore-file[e2e_not_mocked] nothing on the layer under test is substituted: the
// compiled binary reads and writes the real board through the linked `onetaskgraph`, and the
// real `onevcs` reads review comments from, publishes to and replies on a real GitHub pull
// request through real `git` and `gh`. The only stand-in is the paid model turn, at
// oneharness's own documented per-harness override, as `main.rs` states for this whole tier.

use std::collections::BTreeSet;
use std::process::Command;

use serde_json::{json, Value};

use crate::continuation::{
    change, host, onepipeline, open_pulls, ran, recorded, said, Created, Tracked,
};
use crate::harness::{global, plan_of, World};
use crate::repo::{ensure_repo, gh, gh_try, missing};
use crate::{authenticated_user, run_stamp, scratch_repo};

/// The source this journey declares the board as, on every host.
const BOARD: &str = "board";

/// The variables that name the live-lane board and the credential that writes it.
const BOARD_ENV: [&str; 4] = [
    "GH_PROJECTS_TOKEN",
    "GH_PROJECTS_OWNER",
    "GH_PROJECTS_NUMBER",
    "GH_PROJECTS_REPOSITORY",
];

/// Each board variable's value, or a refusal naming the one missing: this journey never
/// falls back to a local store.
fn board_settings() -> Vec<(&'static str, String)> {
    BOARD_ENV
        .iter()
        .map(|name| {
            let value = std::env::var(name)
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    panic!(
                        "{}",
                        missing(&format!(
                            "{name} to name onetaskgraph's live-lane board (GH_PROJECTS_TOKEN, \
                             GH_PROJECTS_OWNER, GH_PROJECTS_NUMBER and GH_PROJECTS_REPOSITORY \
                             together), which the review journey stores its project on"
                        ))
                    )
                });
            (*name, value)
        })
        .collect()
}

/// A world whose store holds the board as [`BOARD`], beside its own local source, and
/// reads the board where it names no source.
fn on_board(mut world: World, settings: &[(&'static str, String)]) -> World {
    let value = |name: &str| {
        settings
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.clone())
            .expect("every board setting was read")
    };
    let prefix = format!("ONETASKGRAPH_SOURCES__{}", BOARD.to_uppercase());
    for (key, setting) in [
        ("PLUGIN", "github-projects".to_owned()),
        ("CONFIG__OWNER", value("GH_PROJECTS_OWNER")),
        ("CONFIG__PROJECT_NUMBER", value("GH_PROJECTS_NUMBER")),
        ("CONFIG__REPOSITORY", value("GH_PROJECTS_REPOSITORY")),
        ("CONFIG__TOKEN_ENV", "GH_PROJECTS_TOKEN".to_owned()),
    ] {
        world = world.with_env(&format!("{prefix}__{key}"), &setting);
    }
    // A read naming no source asks the default ones, so the project read is the board's.
    world.with_env("ONETASKGRAPH_DEFAULT_SOURCES", BOARD)
}

/// `gh`, authenticated as the board's own credential, for the board's issues.
fn board_gh(settings: &[(&'static str, String)], args: &[&str]) -> Result<String, String> {
    let token = &settings
        .iter()
        .find(|(key, _)| *key == "GH_PROJECTS_TOKEN")
        .expect("the board's credential was read")
        .1;
    let output = Command::new("gh")
        .args(args)
        .env("GH_TOKEN", token)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| format!("gh {}: {error}", args.join(" ")))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!(
            "gh {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// One issue of the board's repository, as the REST listing gives it: a pull request is
/// listed there too, and told apart by the key it alone carries.
#[derive(serde::Deserialize)]
struct Listed {
    number: u64,
    title: String,
    #[serde(default)]
    pull_request: Option<Value>,
}

/// The board issues carrying `stamp` in their titles, open or closed, by number — read
/// through every page of the repository's issues, and refused whole where any page or
/// entry does not read.
fn stamped_issues(settings: &[(&'static str, String)], stamp: &str) -> Result<Vec<u64>, String> {
    let repository = &settings
        .iter()
        .find(|(key, _)| *key == "GH_PROJECTS_REPOSITORY")
        .expect("the board's repository was read")
        .1;
    let listed = board_gh(
        settings,
        &[
            "api",
            "--paginate",
            "--slurp",
            &format!("repos/{repository}/issues?state=all&per_page=100"),
        ],
    )?;
    let pages: Vec<Vec<Listed>> =
        serde_json::from_str(&listed).map_err(|why| format!("the issue listing: {why}"))?;
    Ok(pages
        .into_iter()
        .flatten()
        .filter(|issue| issue.pull_request.is_none() && issue.title.contains(stamp))
        .map(|issue| issue.number)
        .collect())
}

/// What the journey wrote to the board, and its removal: every issue carrying the stamp is
/// deleted — which takes it off the board too — and the repository is read again to show none
/// is left. `Drop` runs the same sweep, best-effort, for a journey that failed first.
struct BoardResidue {
    settings: Vec<(&'static str, String)>,
    stamp: String,
    cleaned: bool,
}

impl BoardResidue {
    fn clean(&mut self) -> Result<Vec<u64>, String> {
        self.cleaned = true;
        let repository = self
            .settings
            .iter()
            .find(|(key, _)| *key == "GH_PROJECTS_REPOSITORY")
            .expect("the board's repository was read")
            .1
            .clone();
        let stamped = stamped_issues(&self.settings, &self.stamp)?;
        let mut failures = Vec::new();
        for number in &stamped {
            if let Err(why) = board_gh(
                &self.settings,
                &[
                    "issue",
                    "delete",
                    &number.to_string(),
                    "--repo",
                    &repository,
                    "--yes",
                ],
            ) {
                failures.push(why);
            }
        }
        let left = stamped_issues(&self.settings, &self.stamp)?;
        if !left.is_empty() {
            failures.push(format!("board issues left after the cleanup: {left:?}"));
        }
        if failures.is_empty() {
            Ok(stamped)
        } else {
            Err(failures.join("\n"))
        }
    }
}

impl Drop for BoardResidue {
    fn drop(&mut self) {
        if self.cleaned {
            return;
        }
        match self.clean() {
            Ok(deleted) => eprintln!("smoke: board cleanup after a failed journey: {deleted:?}"),
            Err(why) => eprintln!("smoke: board cleanup after a failed journey FAILED: {why}"),
        }
    }
}

/// The board's credential, as the setting a store call made here reads it from.
fn board_token<'a>(settings: &'a [(&'static str, String)]) -> [(&'static str, &'a str); 1] {
    let token = settings
        .iter()
        .find(|(key, _)| *key == "GH_PROJECTS_TOKEN")
        .map(|(_, token)| token.as_str())
        .expect("the board's credential was read");
    [("GH_PROJECTS_TOKEN", token)]
}

/// Every document of `world`'s local source given the status `todo`: one the store writes
/// with none reads as `backlog`, a column the live-lane board does not have, and a copy
/// carrying it there is refused.
fn as_todo(world: &World) {
    let mut pending = vec![world.store()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("the local store reads") {
            let path = entry.expect("a store entry").path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if path.extension().is_none_or(|extension| extension != "md") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("a store document reads");
            let Some(front) = text.strip_prefix("---\n") else {
                continue;
            };
            let head = front.split("\n---\n").next().unwrap_or_default();
            if head.lines().any(|line| line.starts_with("status:")) {
                continue;
            }
            std::fs::write(&path, format!("---\nstatus: todo\n{front}"))
                .expect("a store document is written");
        }
    }
}

/// The project the plan written to `world`'s local source is, copied onto the board whole,
/// and the board's id for it.
fn copied_to_board(world: &World, settings: &[(&'static str, String)], local: &str) -> String {
    let local = global(local);
    let item = local.clone();
    let report = world.store_call_with(&board_token(settings), |engine| async move {
        let request = onetaskgraph_core::CopyRequest {
            items: onetaskgraph_core::CopyItems::new(vec![item]).expect("one item"),
            scope: onetaskgraph_core::CopyScope::Projects { tasks: true },
            destination: onetaskgraph_plugin_api::SourceName::new(BOARD).expect("a source name"),
            match_by: None,
            recreate: false,
            create: true,
            dry_run: false,
            comments: false,
            no_link: true,
            expect: None,
        };
        let report = engine
            .copy(&request)
            .await
            .unwrap_or_else(|error| panic!("the plan is copied onto the board: {error}"));
        let _ = engine.end_command().await;
        serde_json::to_value(&report).expect("a copy report renders")
    });
    println!("smoke: the copy onto the board reported {report}");
    report["items"]
        .as_array()
        .expect("the copy reports its items")
        .iter()
        .find(|item| item["source"] == json!(local.to_string()))
        .and_then(|item| item["destination"].as_str())
        .unwrap_or_else(|| panic!("the copy names no board project: {report}"))
        .to_owned()
}

/// Every reserved key the board's tasks of `project` carry, by node id.
fn board_keys(
    world: &World,
    settings: &[(&'static str, String)],
    project: &str,
) -> Vec<(String, BTreeSet<String>)> {
    world
        .store_tasks_with(&board_token(settings), project)
        .iter()
        .map(|task| {
            let metadata = &task["item"]["metadata"];
            let node = metadata["onepipeline.id"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let keys = metadata
                .as_object()
                .map(|keys| keys.keys().cloned().collect())
                .unwrap_or_default();
            (node, keys)
        })
        .collect()
}

/// The board holds no review state: no key naming a reply, a decline or a review, and
/// `onepipeline.rework` only on a rework replacement — the task that supersedes another.
fn holds_no_review_state(
    world: &World,
    settings: &[(&'static str, String)],
    project: &str,
    when: &str,
) {
    let keys = board_keys(world, settings, project);
    println!("smoke: the board's keys {when}: {keys:?}");
    for (node, keys) in &keys {
        for key in keys {
            assert!(
                !["reply", "replies", "decline", "review"]
                    .iter()
                    .any(|word| key.contains(word)),
                "{when}: {node} carries {key} on the board"
            );
        }
        assert_eq!(
            keys.contains("onepipeline.rework"),
            keys.contains("onepipeline.supersedes"),
            "{when}: {node}'s keys {keys:?}"
        );
    }
}

/// `results --json`'s comments, keyed by comment id, as `[node, state, rework, reply_url]`.
fn comment_views(world: &World, run: &str) -> Vec<(String, Value)> {
    let output = ran(&mut onepipeline(world, &["results", run, "--json"]));
    assert!(output.status.success(), "results: {}", said(&output));
    let results: Value = serde_json::from_slice(&output.stdout).expect("`results --json` is JSON");
    let mut views = Vec::new();
    for node in results["nodes"].as_array().into_iter().flatten() {
        for comment in node["comments"].as_array().into_iter().flatten() {
            views.push((
                comment["id"].as_str().unwrap_or_default().to_owned(),
                json!({"node": node["id"], "url": comment["url"], "state": comment["state"],
                       "rework": comment["rework"], "commits": comment["commits"],
                       "reply_url": comment["reply_url"]}),
            ));
        }
    }
    views.sort_by(|a, b| a.0.cmp(&b.0));
    views
}

/// One comment of a pull request's conversation, as the REST listing gives it.
#[derive(serde::Deserialize)]
struct Commented {
    body: String,
}

/// The marked replies on pull request `number`'s conversation answering `comment`, read
/// through every page; a page or a comment that does not read fails the journey.
fn replies_to(slug: &str, number: u64, comment: &str) -> Vec<String> {
    let listed = gh(&[
        "api",
        "--paginate",
        "--slurp",
        &format!("repos/{slug}/issues/{number}/comments?per_page=100"),
    ]);
    let pages: Vec<Vec<Commented>> = serde_json::from_str(&listed)
        .unwrap_or_else(|why| panic!("reading #{number}'s conversation: {why}: {listed}"));
    let bodies: Vec<String> = pages
        .into_iter()
        .flatten()
        .map(|comment| comment.body)
        .collect();
    let marker = format!("in-reply-to={comment} ");
    bodies
        .into_iter()
        .filter(|body| body.contains("<!-- onevcs:reply") && body.contains(&marker))
        .collect()
}

/// Whether `ancestor` is an ancestor of `of` on `slug`, as GitHub compares them.
fn is_ancestor(slug: &str, ancestor: &str, of: &str) -> bool {
    let compared = gh(&[
        "api",
        &format!("repos/{slug}/compare/{ancestor}...{of}"),
        "--jq",
        ".status",
    ]);
    matches!(compared.trim(), "ahead" | "identical")
}

/// The run id the only run in `world`'s runs root has.
fn the_run(world: &World) -> String {
    std::fs::read_dir(&world.runs)
        .expect("the runs root")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .next()
        .expect("the launch minted a run")
}

/// Send `commands` on `run`'s planner channel from `world`.
fn reply(world: &World, run: &str, commands: Value) -> std::process::Output {
    let mut command = onepipeline(world, &["reply", run]);
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().expect("`reply` starts");
    {
        use std::io::Write;
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(
                json!({"version": 3, "commands": commands})
                    .to_string()
                    .as_bytes(),
            )
            .expect("the commands are sent");
    }
    child.wait_with_output().expect("`reply` answers")
}

/// The pull request number at the end of `url`.
fn number(url: &Value) -> u64 {
    url.as_str()
        .and_then(|url| url.rsplit('/').next())
        .and_then(|number| number.parse().ok())
        .unwrap_or_else(|| panic!("{url} names no pull request"))
}

/// Two review comments on a real draft are surfaced by one host, and answered — one by a
/// rework that pushes new commits onto the draft's existing pull request and restacks the
/// draft above it, one by a decline posting exactly one reply — by a cold host continuing
/// the project from the live-lane board, which holds no review state at any step.
#[test]
fn review_comments_found_on_one_host_are_reworked_and_declined_on_a_cold_one_from_the_board() {
    let slug = scratch_repo();
    authenticated_user();
    ensure_repo(&slug);
    let settings = board_settings();
    let stamp = run_stamp();
    let mut residue = Created {
        slug: slug.clone(),
        stamp: stamp.clone(),
        prefixes: Vec::new(),
        cleaned: false,
    };
    let mut board = BoardResidue {
        settings: settings.clone(),
        stamp: stamp.clone(),
        cleaned: false,
    };

    let first = host(
        on_board(
            World::new("smoke-review").with_env("HOSTNAME", "smoke-review-a"),
            &settings,
        ),
        &slug,
    );
    first.write_graphs();
    let graph = first.review_reply_graph();
    assert!(
        !World::substitutes_the_host(&first.real_cmd(&["start"])),
        "{}",
        missing("this run still carries a stand-in for `gh`, so it would never reach GitHub")
    );
    first.script("harness.work", &format!("reviewed on the board {stamp}"));
    first.script("harness.work-per-task", "");
    let name = format!("review-{}", stamp.replace(['_', '.'], "-"));
    let local = first.plan(
        &name,
        &plan_of(
            &name,
            vec![change("a", &[], &stamp), change("b", &["a"], &stamp)],
        ),
    );
    as_todo(&first);
    let project = copied_to_board(&first, &settings, &local);
    println!("smoke: the plan is on the live-lane board as {project}");
    // A board task's branches are cut under its issue's number. The repository's
    // listing lags the copy, so it is read until it holds the project and both tasks.
    let issues = || {
        stamped_issues(&settings, &stamp)
            .unwrap_or_else(|why| panic!("reading the board's issues: {why}"))
    };
    crate::continuation::until("the listing to hold the copy's three issues", || {
        issues().len() == 3
    });
    residue.prefixes = issues().iter().map(|number| format!("{number}/")).collect();
    crate::continuation::until("the board to read back both tasks", || {
        first
            .store_tasks_with(&board_token(&settings), &project)
            .len()
            == 2
    });

    // Host A drafts the stack from the board.
    let launched = ran(&mut onepipeline(
        &first,
        &[
            "start",
            &project,
            "--attach",
            "--review-reply-graph",
            &graph,
        ],
    ));
    assert!(
        std::fs::read_dir(&first.runs).is_ok_and(|mut runs| runs.next().is_some()),
        "the first launch minted no run: {}",
        said(&launched)
    );
    let run = the_run(&first);
    assert!(
        launched.status.success(),
        "the first launch: {}\n{}",
        said(&launched),
        recorded(&first, &run)
    );
    let settled = |world: &World, node: &str| {
        world
            .events_of(&run, "node-settled")
            .into_iter()
            .rfind(|event| event["labels"]["node"] == node)
            .unwrap_or_else(|| panic!("{node} settled"))["payload"]
            .clone()
    };
    let (a, b) = (settled(&first, "a"), settled(&first, "b"));
    println!("smoke: a settled {a}\nsmoke: b settled {b}");
    let pull = number(&a["change_url"]);
    holds_no_review_state(
        &first,
        &settings,
        &project,
        "after the first host drafted the stack",
    );

    // Two review comments on the bottom draft: one in its conversation, one a review's
    // summary, whose reply is a linked conversation comment.
    gh(&[
        "api",
        &format!("repos/{slug}/issues/{pull}/comments"),
        "-f",
        &format!("body=Please name the change in the file ({stamp})."),
    ]);
    gh(&[
        "pr",
        "review",
        &pull.to_string(),
        "--repo",
        &slug,
        "--comment",
        "--body",
        &format!("Could this also cover the other module? ({stamp})"),
    ]);
    let reviewed = ran(&mut onepipeline(&first, &["review", &run]));
    assert!(
        reviewed.status.success(),
        "review on A: {}",
        said(&reviewed)
    );
    let on_first = comment_views(&first, &run);
    println!("smoke: the first host's view of the comments: {on_first:#?}");
    let pending: Vec<&(String, Value)> = on_first
        .iter()
        .filter(|(_, view)| view["state"] == "pending")
        .collect();
    assert_eq!(pending.len(), 2, "both comments surfaced: {on_first:#?}");
    let reworked = pending
        .iter()
        .find(|(_, view)| {
            view["url"]
                .as_str()
                .is_some_and(|url| url.contains("issuecomment"))
        })
        .copied()
        .cloned()
        .expect("the conversation comment surfaced");
    let declined = pending
        .iter()
        .find(|(id, _)| *id != reworked.0)
        .copied()
        .cloned()
        .expect("the review's comment surfaced");
    holds_no_review_state(
        &first,
        &settings,
        &project,
        "after the first host's review pass",
    );

    // Every record host A keeps is deleted; host B holds nothing but the board.
    std::fs::remove_dir_all(&first.runs).expect("the first host's runs root is deleted");
    std::fs::remove_dir_all(first.onevcs_home()).expect("the first host's state is deleted");
    let second = host(first.on_host("smoke-review-b"), &slug);
    let continued = ran(&mut onepipeline(
        &second,
        &[
            "start",
            &project,
            "--attach",
            "--review-reply-graph",
            &graph,
        ],
    ));
    assert!(
        continued.status.success(),
        "the cold host's launch: {}\n{}",
        said(&continued),
        recorded(&second, &run)
    );
    assert_eq!(the_run(&second), run, "the cold host continued another run");
    let reviewed = ran(&mut onepipeline(&second, &["review", &run]));
    assert!(
        reviewed.status.success(),
        "review on B: {}",
        said(&reviewed)
    );
    let on_second = comment_views(&second, &run);
    assert_eq!(
        on_second
            .iter()
            .map(|(id, view)| (id.clone(), view["url"].clone(), view["state"].clone()))
            .collect::<Vec<_>>(),
        on_first
            .iter()
            .map(|(id, view)| (id.clone(), view["url"].clone(), view["state"].clone()))
            .collect::<Vec<_>>(),
        "the cold host's view of the comments is not the first host's"
    );
    holds_no_review_state(
        &second,
        &settings,
        &project,
        "after the cold host's review pass",
    );

    // B reworks the bottom draft for the first comment, in a file of its own so the draft
    // above it restacks over it cleanly.
    second.script("harness.work-file", "review-answer.md");
    second.script(
        "harness.work",
        &format!("named as the review asked {stamp}"),
    );
    let node = reworked.1["node"].as_str().expect("a node").to_owned();
    let before = open_pulls(&slug, a["branch"].as_str().expect("a's branch"));
    assert_eq!(before.len(), 1, "{before:?}");
    let old_head = gh(&[
        "pr",
        "view",
        &pull.to_string(),
        "--repo",
        &slug,
        "--json",
        "headRefOid",
        "--jq",
        ".headRefOid",
    ])
    .trim()
    .to_owned();
    let sent = reply(
        &second,
        &run,
        json!([{"op": "rework", "id": node, "reason": "the reviewer asked for it",
                "instruction": "Name the change in the file",
                "comments": [{"id": reworked.0, "url": reworked.1["url"]}]}]),
    );
    assert!(sent.status.success(), "the rework: {}", said(&sent));
    let adopted = ran(&mut onepipeline(&second, &["adopt", &run]));
    assert!(
        adopted.status.success(),
        "driving the rework: {}\n{}",
        said(&adopted),
        recorded(&second, &run)
    );
    let replacement = second
        .events_of(&run, "node-settled")
        .into_iter()
        .rfind(|event| {
            event["labels"]["node"]
                .as_str()
                .is_some_and(|id| id.starts_with(&format!("{node}-rework-")))
        })
        .expect("the rework settled")["payload"]
        .clone();
    println!("smoke: the rework settled {replacement}");
    assert_eq!(
        replacement["branch"], a["branch"],
        "the rework moved branches"
    );
    assert_eq!(
        number(&replacement["change_url"]),
        pull,
        "the rework opened another pull request"
    );
    let after = open_pulls(&slug, a["branch"].as_str().expect("a's branch"));
    assert_eq!(
        after.len(),
        1,
        "a second pull request is open from a's branch: {after:?}"
    );
    let new_head = replacement["head"].as_str().expect("a head").to_owned();
    assert_ne!(new_head, old_head, "the rework pushed nothing");
    assert!(
        is_ancestor(&slug, &old_head, &new_head),
        "the previous head {old_head} is not an ancestor of {new_head}"
    );
    let b_head = gh(&[
        "pr",
        "view",
        &number(&b["change_url"]).to_string(),
        "--repo",
        &slug,
        "--json",
        "headRefOid",
        "--jq",
        ".headRefOid",
    ])
    .trim()
    .to_owned();
    assert!(
        is_ancestor(&slug, &new_head, &b_head),
        "b was not restacked onto the rework's head {new_head}: b is at {b_head}"
    );
    holds_no_review_state(&second, &settings, &project, "after the rework");
    // The rework settled with its verdict deferred to the next trigger, which drafts and
    // posts the reply it owes: one, linking a commit the rework added.
    let reviewed = ran(&mut onepipeline(&second, &["review", &run]));
    assert!(
        reviewed.status.success(),
        "the pass after the rework: {}",
        said(&reviewed)
    );
    let answers = replies_to(&slug, pull, &reworked.0);
    println!("smoke: the rework's replies on #{pull}: {answers:#?}");
    assert_eq!(answers.len(), 1, "{answers:#?}");
    assert!(
        answers[0].contains(&format!("/commit/{new_head}")),
        "the reply links no commit the rework added: {}",
        answers[0]
    );
    holds_no_review_state(&second, &settings, &project, "after the rework's reply");

    // B declines the second comment: exactly one reply in its thread, no commit linked.
    let holder = comment_views(&second, &run)
        .into_iter()
        .find(|(id, _)| *id == declined.0)
        .map(|(_, view)| view["node"].as_str().unwrap_or_default().to_owned())
        .expect("the second comment is still read");
    let refused = reply(
        &second,
        &run,
        json!([{"op": "decline", "id": holder,
                "comments": [{"id": declined.0, "url": declined.1["url"]}],
                "reason": "The other module is a change of its own."}]),
    );
    assert!(refused.status.success(), "the decline: {}", said(&refused));
    let declines = replies_to(&slug, pull, &declined.0);
    assert_eq!(declines.len(), 1, "{declines:#?}");
    assert!(!declines[0].contains("/commit/"), "{}", declines[0]);
    assert!(declines[0].contains("label=declined"), "{}", declines[0]);
    holds_no_review_state(&second, &settings, &project, "after the decline");
    let finally = comment_views(&second, &run);
    println!("smoke: the cold host's view after the round: {finally:#?}");
    let state = |id: &str| {
        finally
            .iter()
            .find(|(seen, _)| seen == id)
            .map(|(_, view)| view["state"].clone())
            .unwrap_or(Value::Null)
    };
    assert_eq!(state(&declined.0), "declined", "{finally:#?}");
    assert_eq!(state(&reworked.0), "answered", "{finally:#?}");

    // The journey's own cleanup, read back from GitHub.
    let mut tracked = Tracked::default();
    for prefix in &residue.prefixes {
        tracked.read(&second, &run, &slug, prefix);
    }
    let cleanup = residue.clean(&tracked);
    for step in &cleanup.observed {
        println!("smoke: cleanup: {step}");
    }
    let removed = board.clean();
    println!("smoke: board cleanup: {removed:?}");
    assert!(
        cleanup.failures.is_empty(),
        "the cleanup did not finish: {:#?}",
        cleanup.failures
    );
    assert!(
        removed.is_ok(),
        "the board cleanup did not finish: {removed:?}"
    );
    let _ = gh_try(&["api", "rate_limit", "--jq", ".resources.graphql.remaining"]).map(|left| {
        println!(
            "smoke: graphql points left on the gh token: {}",
            left.trim()
        )
    });
    drop(second);
}
