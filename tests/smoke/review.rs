//! The live half of `docs/contract-divergences.md` entry 129: review comments on a real draft,
//! found by one host, answered on a cold one continuing from the plan store.
//!
//! The store is onetaskgraph's live-lane board — nickderobertis Projects #1, its issues in
//! `nickderobertis/onetaskgraph-live-scratch` — declared as the source [`LIVE_SOURCE`] under the
//! world's own configuration home, with the board's credential, `GH_PROJECTS_TOKEN`, and
//! nothing of the operator's `plans` or `followups` sources: the world reads only the sources
//! it declares. The plan is authored in a scratch `local-md` folder and copied onto the board,
//! and both hosts launch it from the board. The drafts are on the scratch repository the
//! lifecycle smoke uses. Host A drafts a stack of two, two review comments are posted on the
//! bottom draft, and `onepipeline review` on A surfaces both; A's runs root and `ONEVCS_HOME`
//! are then deleted, and host B — holding nothing but the board — continues the project,
//! reworks the bottom draft for the first comment and declines the second. The board is read
//! back at each step. Every pull request, branch and board issue the journey made is removed
//! when it ends, however it ends.
//!
//! What it does not hold anybody to is a verdict GitHub alone decides: the scratch repository
//! runs no checks, and each republication is read for whatever settlement it reached.

// llmlint: ignore-file[e2e_not_mocked] nothing on the layer under test is substituted: the
// compiled binary reads and writes the live board through the linked `onetaskgraph`, and the
// real `onevcs` reads review comments from, publishes to and replies on a real GitHub pull
// request through real `git` and `gh`. The only stand-in is the paid model turn, at
// oneharness's own documented per-harness override, as `main.rs` states for this whole tier.

use std::collections::BTreeSet;
use std::process::Command;

use serde_json::{json, Value};

use crate::continuation::{
    change, host, onepipeline, open_pulls, ran, recorded, said, Created, Tracked,
};
use crate::harness::{plan_of, project_id, World};
use crate::repo::{ensure_repo, gh, gh_try, missing};
use crate::{authenticated_user, run_stamp, scratch_repo};

/// The name the live-lane board is declared under in each host's store configuration.
const LIVE_SOURCE: &str = "live";

/// The board's owner and number: onetaskgraph's live lane, which a journey may write to.
const BOARD_OWNER: &str = "nickderobertis";
const BOARD_NUMBER: u32 = 1;

/// The repository the board files its issues in.
const BOARD_REPOSITORY: &str = "nickderobertis/onetaskgraph-live-scratch";

/// The variable the board's credential arrives under.
const BOARD_TOKEN_ENV: &str = "GH_PROJECTS_TOKEN";

/// The Status option each status category the engine writes lands on. `queued` is every node's
/// claim before dispatch and `unknown` a settlement with no category of its own, so a board
/// missing either refuses the run's projections; [`board_token`] refuses the journey first.
const STATUS_OPTIONS: [(&str, &str); 6] = [
    ("todo", "Todo"),
    ("queued", "Queued"),
    ("in-progress", "In Progress"),
    ("unknown", "Needs attention"),
    ("done", "Done"),
    ("cancelled", "Cancelled"),
];

/// The board's credential, refused when it is missing or when the board lacks an option the
/// engine writes to: either would fail the journey half way through, past what it created.
fn board_token() -> String {
    let token = std::env::var(BOARD_TOKEN_ENV)
        .ok()
        .filter(|token| !token.trim().is_empty())
        .unwrap_or_else(|| {
            panic!(
                "{}",
                missing(&format!(
                    "{BOARD_TOKEN_ENV} is not set, and this journey's plan store is the \
                     live-lane board, Projects #{BOARD_NUMBER} of {BOARD_OWNER}"
                ))
            )
        });
    let read = board_gh(
        &token,
        &[
            "api",
            "graphql",
            "-f",
            &format!(
                "query={{user(login:\"{BOARD_OWNER}\"){{projectV2(number:{BOARD_NUMBER}){{\
                 field(name:\"Status\"){{... on ProjectV2SingleSelectField{{options{{name}}}}}}}}}}}}"
            ),
            "--jq",
            ".data.user.projectV2.field.options[].name",
        ],
    )
    .unwrap_or_else(|why| panic!("{}", missing(&format!("the board did not answer: {why}"))));
    let options: BTreeSet<String> = read
        .lines()
        .map(|name| name.trim().to_lowercase())
        .collect();
    let absent: Vec<&str> = STATUS_OPTIONS
        .iter()
        .map(|(_, option)| *option)
        .filter(|option| !options.contains(&option.to_lowercase()))
        .collect();
    assert!(
        absent.is_empty(),
        "the live-lane board carries no Status option {absent:?}, which the engine writes; add \
         them with `onetaskgraph sources status-options` or the board's own settings: {read}"
    );
    token
}

/// `gh` authenticated as the board's credential rather than the host's.
fn board_gh(token: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new("gh")
        .args(args)
        .env("GH_TOKEN", token)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|why| format!("gh does not run: {why}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!(
            "gh {args:?} exited {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

/// Declare the board as [`LIVE_SOURCE`] in `world`'s configuration home, which every host made
/// from it reads, and every store read of it here.
fn declare_board(world: &World) {
    let home = world.root.join("xdg").join("onetaskgraph");
    std::fs::create_dir_all(&home).expect("the world's configuration home");
    let mapping: String = STATUS_OPTIONS
        .iter()
        .map(|(category, option)| format!("        {category}: {option}\n"))
        .collect();
    std::fs::write(
        home.join("config.yaml"),
        format!(
            "sources:\n  {LIVE_SOURCE}:\n    plugin: github-projects\n    config:\n      \
             owner: {BOARD_OWNER}\n      project_number: {BOARD_NUMBER}\n      \
             repository: {BOARD_REPOSITORY}\n      token_env: {BOARD_TOKEN_ENV}\n      \
             status_mapping:\n        backlog: null\n{mapping}"
        ),
    )
    .expect("the board's source is declared");
}

/// The setting that carries the board's credential to a store read made here.
fn credential(token: &str) -> [(&str, &str); 1] {
    [(BOARD_TOKEN_ENV, token)]
}

/// Author `plan` in a scratch `local-md` folder and copy it onto the board, answering the
/// board project's qualified id. The copy writes nothing back to the folder.
fn on_board(world: &World, token: &str, name: &str, plan: &Value) -> String {
    let seed = world.store_apart("seed");
    world.plan_in(&seed, name, plan);
    let root = seed.to_string_lossy().into_owned();
    let mut settings = vec![
        ("ONETASKGRAPH_SOURCES__SEED__PLUGIN", "local-md"),
        ("ONETASKGRAPH_SOURCES__SEED__CONFIG__ROOT", root.as_str()),
    ];
    settings.extend(credential(token));
    let authored = crate::harness::global(&format!("seed:{}", project_id(name)));
    let report = world.store_call_with(&settings, |engine| async move {
        engine
            .copy(&onetaskgraph_core::CopyRequest {
                items: onetaskgraph_core::CopyItems::new(vec![authored]).expect("one item"),
                scope: onetaskgraph_core::CopyScope::Projects { tasks: true },
                destination: onetaskgraph_plugin_api::SourceName::new(LIVE_SOURCE)
                    .expect("a source name"),
                match_by: None,
                recreate: false,
                create: true,
                dry_run: false,
                comments: false,
                no_link: true,
                expect: None,
            })
            .await
            .unwrap_or_else(|error| panic!("the plan is copied onto the board: {error}"))
    });
    let project = report
        .items
        .first()
        .and_then(|item| item.action.destination())
        .unwrap_or_else(|| panic!("the copy names where the project landed: {report:?}"));
    assert_eq!(project.source.as_str(), LIVE_SOURCE, "{report:?}");
    project.to_string()
}

/// The board's issues the journey made, deleted however it ends: every issue of the board's
/// repository whose title carries the run's stamp — the project's and each task's, a rework's
/// replacement included, since a replacement keeps its node's title — and every one a board
/// read of the project named. Deleting an issue removes its board item with it.
struct BoardResidue {
    token: String,
    stamp: String,
    seen: BTreeSet<u64>,
    cleaned: bool,
}

impl BoardResidue {
    /// Record every issue of the board's repository `tasks` name.
    fn saw(&mut self, tasks: &[Value]) {
        let issues = format!("https://github.com/{BOARD_REPOSITORY}/issues/");
        let mut pending: Vec<&Value> = tasks.iter().collect();
        while let Some(value) = pending.pop() {
            match value {
                Value::String(text) => {
                    if let Some(number) = text
                        .strip_prefix(&issues)
                        .and_then(|rest| rest.split(['/', '#']).next())
                        .and_then(|number| number.parse().ok())
                    {
                        self.seen.insert(number);
                    }
                }
                Value::Array(values) => pending.extend(values),
                Value::Object(fields) => pending.extend(fields.values()),
                _ => {}
            }
        }
    }

    /// The issues of the board's repository the journey made, by number, with their node ids.
    fn issues(&self) -> Result<Vec<(u64, String, String)>, String> {
        let listed = board_gh(
            &self.token,
            &[
                "api",
                "--paginate",
                "--slurp",
                &format!("repos/{BOARD_REPOSITORY}/issues?state=all&per_page=100"),
            ],
        )?;
        let pages: Vec<Vec<Value>> =
            serde_json::from_str(&listed).map_err(|why| format!("{why}: {listed}"))?;
        Ok(pages
            .into_iter()
            .flatten()
            .filter_map(|issue| {
                let number = issue["number"].as_u64()?;
                let title = issue["title"].as_str()?.to_owned();
                let node = issue["node_id"].as_str()?.to_owned();
                (title.contains(&self.stamp) || self.seen.contains(&number))
                    .then_some((number, title, node))
            })
            .collect())
    }

    /// Delete every issue [`BoardResidue::issues`] finds, then read the repository back.
    fn clean(&mut self) -> (Vec<String>, Vec<String>) {
        self.cleaned = true;
        let (mut observed, mut failures) = (Vec::new(), Vec::new());
        let found = match self.issues() {
            Ok(found) => found,
            Err(why) => return (observed, vec![format!("listing the board's issues: {why}")]),
        };
        for (number, title, node) in &found {
            match board_gh(
                &self.token,
                &[
                    "api",
                    "graphql",
                    "-f",
                    "query=mutation($id:ID!){deleteIssue(input:{issueId:$id}){clientMutationId}}",
                    "-f",
                    &format!("id={node}"),
                ],
            ) {
                Ok(_) => observed.push(format!("deleted board issue #{number} {title:?}")),
                Err(why) => failures.push(format!("deleting board issue #{number}: {why}")),
            }
        }
        match self.issues() {
            Ok(left) if left.is_empty() => observed.push(format!(
                "read back: none of the {} board issue(s) is left",
                found.len()
            )),
            Ok(left) => failures.push(format!("board issues left after the cleanup: {left:?}")),
            Err(why) => failures.push(format!("reading the board's issues back: {why}")),
        }
        (observed, failures)
    }
}

impl Drop for BoardResidue {
    fn drop(&mut self) {
        if self.cleaned {
            return;
        }
        let (observed, failures) = self.clean();
        for step in &observed {
            eprintln!("smoke: board cleanup after a failed journey: {step}");
        }
        for failure in &failures {
            eprintln!("smoke: board cleanup after a failed journey FAILED: {failure}");
        }
    }
}

/// Every reserved key the board's tasks of `project` carry, by node id, read from the board
/// itself; every issue the read names is recorded for the cleanup.
fn store_keys(
    world: &World,
    board: &mut BoardResidue,
    project: &str,
) -> Vec<(String, BTreeSet<String>)> {
    let tasks = world.store_tasks_with(&credential(&board.token), project);
    board.saw(&tasks);
    tasks
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
fn holds_no_review_state(world: &World, board: &mut BoardResidue, project: &str, when: &str) {
    let keys = store_keys(world, board, project);
    println!("smoke: the board's keys {when}: {keys:?}");
    assert!(
        !keys.is_empty(),
        "{when}: the board holds no task of {project}"
    );
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
    let token = board_token();
    ensure_repo(&slug);
    let stamp = run_stamp();
    let mut board = BoardResidue {
        token: token.clone(),
        stamp: stamp.clone(),
        seen: BTreeSet::new(),
        cleaned: false,
    };
    let mut residue = Created {
        slug: slug.clone(),
        stamp: stamp.clone(),
        prefixes: Vec::new(),
        cleaned: false,
    };

    let first = host(
        World::new("smoke-review").with_env("HOSTNAME", "smoke-review-a"),
        &slug,
    );
    first.write_graphs();
    declare_board(&first);
    let graph = first.review_reply_graph();
    assert!(
        !World::substitutes_the_host(&first.real_cmd(&["start"])),
        "{}",
        missing("this run still carries a stand-in for `gh`, so it would never reach GitHub")
    );
    first.script("harness.work", &format!("reviewed on the store {stamp}"));
    first.script("harness.work-per-task", "");
    let name = format!("review-{}", stamp.replace(['_', '.'], "-"));
    let project = on_board(
        &first,
        &token,
        &name,
        &plan_of(
            &name,
            vec![change("a", &[], &stamp), change("b", &["a"], &stamp)],
        ),
    );
    println!("smoke: the plan is on the live-lane board as {project}");
    holds_no_review_state(
        &first,
        &mut board,
        &project,
        "once the plan is on the board",
    );

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
    residue.prefixes = vec![format!("{run}/")];
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
    for node in [&a, &b] {
        if let Some((cut, _)) = node["branch"]
            .as_str()
            .and_then(|branch| branch.split_once('/'))
        {
            let prefix = format!("{cut}/");
            if !residue.prefixes.contains(&prefix) {
                residue.prefixes.push(prefix);
            }
        }
    }
    let pull = number(&a["change_url"]);
    holds_no_review_state(
        &first,
        &mut board,
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
        &mut board,
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
        &mut board,
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
    holds_no_review_state(&second, &mut board, &project, "after the rework");
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
    holds_no_review_state(&second, &mut board, &project, "after the rework's reply");

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
    holds_no_review_state(&second, &mut board, &project, "after the decline");
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
    assert!(
        cleanup.failures.is_empty(),
        "the cleanup did not finish: {:#?}",
        cleanup.failures
    );
    drop(second);
    let (observed, failures) = board.clean();
    for step in &observed {
        println!("smoke: board cleanup: {step}");
    }
    assert!(
        failures.is_empty(),
        "the board's cleanup did not finish: {failures:#?}"
    );
    let _ = gh_try(&["api", "rate_limit", "--jq", ".resources.graphql.remaining"]).map(|left| {
        println!(
            "smoke: graphql points left on the gh token: {}",
            left.trim()
        )
    });
}
