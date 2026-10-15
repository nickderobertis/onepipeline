//! Replies to review comments, drafted and posted by the compiled binary through
//! the real linked `onevcs` over real git, with GitHub at `onevcs`'s `ONEVCS_GH`
//! seam: `fake-gh` answers each reply POST through `onevcs-testing`'s remote
//! double, whose ledger and state are what these journeys read back. The reply
//! drafter is the scripted `oneagentgraph` double answering a `review-reply`
//! dispatch; nothing of the engine under test is stood in for.

// llmlint: ignore-file[e2e_not_mocked] the task states these journeys over onevcs-testing's
// remote double and fake agents, because no check may spend the shared account: GitHub is
// substituted at `onevcs`'s own `ONEVCS_GH` override and the drafter at `oneagentgraph`'s
// subprocess boundary, while the compiled binary, the real linked `onevcs` and real git run
// every reply, decline and recovery. The same journey against real GitHub pull-request
// comments and the real `oneagentgraph` is the live tier's `tests/smoke/review.rs`.

use crate::harness::World;
use crate::review::{add, host, launched_with, reply, review, surfaces};
use crate::stacked_drafts::{settled, team_world};
use onevcs::{ChangeId, CommentId, CommentKind};
use onevcs_testing::{FileHost, HostComment};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// The task every reply drafting opens with.
const PREAMBLE: &str = "Draft one terse reply to each review comment";

/// The sentence `## Worker transcript` holds where this host holds no transcript.
const NO_TRANSCRIPT: &str =
    "No worker transcript is available on this host; draft from the sections above.";

/// A GitHub comment node id as GitHub hands one out now — `<prefix>_` and the
/// URL-safe, unpadded base64 of the MessagePack array `[0, repository, object]` —
/// which `onevcs` decodes to address a reply.
pub(crate) fn node_id(prefix: &str, database: u64) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut bytes = vec![0x93, 0x00, 0x01, 0xce];
    bytes.extend_from_slice(
        &u32::try_from(database)
            .expect("a test id fits")
            .to_be_bytes(),
    );
    let mut out = format!("{prefix}_");
    for chunk in bytes.chunks(3) {
        let word = chunk.iter().enumerate().fold(0u32, |acc, (at, byte)| {
            acc | (u32::from(*byte) << (16 - 8 * at))
        });
        for at in 0..=chunk.len() {
            out.push(char::from(
                ALPHABET[((word >> (18 - 6 * at)) & 0x3f) as usize],
            ));
        }
    }
    out
}

/// A reviewer's conversation comment, addressable by `onevcs`.
pub(crate) fn conversation(database: u64, number: u32) -> HostComment {
    HostComment {
        id: CommentId(node_id("IC", database)),
        kind: CommentKind::Conversation,
        author: "reviewer".to_owned(),
        body: format!("Please change {database}"),
        url: format!("https://github.com/owner/service/pull/{number}#issuecomment-{database}"),
        created_at: crate::review::AT.to_owned(),
        updated_at: crate::review::AT.to_owned(),
        in_reply_to: None,
        revision: 0,
    }
}

/// A reviewer's comment opening a review thread on `path`.
pub(crate) fn threaded(database: u64, number: u32, path: &str) -> HostComment {
    HostComment {
        id: CommentId(node_id("PRRC", database)),
        kind: CommentKind::ReviewThread {
            thread: format!("PRRT_{database}"),
            path: path.to_owned(),
            line: Some(1),
            outdated: false,
            resolved: false,
        },
        author: "reviewer".to_owned(),
        body: format!("This line, {database}"),
        url: format!("https://github.com/owner/service/pull/{number}#discussion_r{database}"),
        created_at: crate::review::AT.to_owned(),
        updated_at: crate::review::AT.to_owned(),
        in_reply_to: None,
        revision: 0,
    }
}

/// The reply key every host derives for a comment's revision.
pub(crate) fn key(root: &str, comment: &str, revision: &str) -> String {
    Sha256::digest(format!("{root}:{comment}:{revision}"))
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The `{"id", "url"}` a rework or a decline names a comment by.
pub(crate) fn named(comment: &HostComment) -> Value {
    json!({"id": comment.id.0, "url": comment.url})
}

/// Every reply-drafter dispatch, by its task.
pub(crate) fn drafter_tasks(world: &World) -> Vec<String> {
    world
        .invocations()
        .into_iter()
        .filter(|call| call["tool"] == "oneagentgraph")
        .filter_map(|call| {
            let args = call["args"].as_array()?.clone();
            let at = args.iter().position(|arg| arg == "--task")?;
            args.get(at + 1)?.as_str().map(str::to_owned)
        })
        .filter(|task| task.starts_with(PREAMBLE))
        .collect()
}

/// Every worker dispatch of `node`, by its node label.
pub(crate) fn worker_dispatches(world: &World, node: &str) -> usize {
    world
        .invocations()
        .into_iter()
        .filter(|call| call["tool"] == "oneagentgraph")
        .filter(|call| {
            let args: Vec<&str> = call["args"]
                .as_array()
                .map(|args| args.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            args.windows(2)
                .any(|pair| pair[0] == "--label" && pair[1] == format!("onepipeline.node={node}"))
                && !args.windows(2).any(|pair| {
                    pair[0] == "--label"
                        && (pair[1] == "onepipeline.persona=pr-author"
                            || pair[1] == "onepipeline.persona=review-reply")
                })
                && args.contains(&"--task")
        })
        .count()
}

/// The section of a drafting task headed `## <name>`, without its heading.
pub(crate) fn section<'a>(task: &'a str, name: &str) -> &'a str {
    task.split(&format!("\n## {name}\n"))
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .unwrap_or_else(|| panic!("the task has no `## {name}`:\n{task}"))
}

/// Every `gh api` POST `fake-gh` was asked to make.
pub(crate) fn posts(world: &World) -> Vec<Vec<String>> {
    world
        .invocations()
        .into_iter()
        .filter(|call| call["tool"] == "gh")
        .filter_map(|call| {
            let args: Vec<String> = call["args"]
                .as_array()?
                .iter()
                .filter_map(|arg| arg.as_str().map(str::to_owned))
                .collect();
            args.iter().any(|arg| arg == "POST").then_some(args)
        })
        .collect()
}

/// The marked replies the host holds on change request `number`, newest last.
pub(crate) fn replies(host: &FileHost, number: u32) -> Vec<HostComment> {
    host.state()
        .expect("the host answers")
        .review_comments
        .get(&ChangeId(number.to_string()))
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|comment| comment.body.contains("<!-- onevcs:reply "))
        .collect()
}

/// The reply on `number` answering `comment`, where there is exactly one.
pub(crate) fn the_reply_to(host: &FileHost, number: u32, comment: &HostComment) -> HostComment {
    let marker = format!("in-reply-to={}", comment.id.0);
    let found: Vec<HostComment> = replies(host, number)
        .into_iter()
        .filter(|reply| reply.body.contains(&marker))
        .collect();
    assert_eq!(found.len(), 1, "{} has replies {found:#?}", comment.id.0);
    found.into_iter().next().expect("one reply")
}

/// The commits on the origin from `from` (exclusive) to `to`.
pub(crate) fn commits_between(world: &World, from: &str, to: &str) -> Vec<String> {
    let listed = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(world.root.join("origin.git"))
        .args(["rev-list", "--reverse", &format!("{from}..{to}")])
        .output()
        .expect("git runs");
    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    String::from_utf8_lossy(&listed.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

/// The write-back's projection records, as the run holds them.
pub(crate) fn projections(world: &World, run: &str) -> String {
    std::fs::read_to_string(world.run_file(run, "writeback-projections.jsonl")).unwrap_or_default()
}

/// A run of two independent lifecycle nodes, `a` and `b`, launched with the reply
/// drafter, their drafts open on the host double.
pub(crate) fn launched_with_replies(world: &World, run: &str) -> (String, FileHost) {
    let graph = world.review_reply_graph();
    let project = launched_with(world, run, &["--review-reply-graph", &graph]);
    let host = host(world, run, &["a", "b"]);
    (project, host)
}

/// Rework `a` answering `comments` from the planner's channel, and drive the rework to
/// its settlement.
pub(crate) fn rework(world: &World, run: &str, comments: &[&HostComment]) {
    world.script("a-rework-1.work", "reworked as the review asked\n");
    let output = reply(
        world,
        run,
        json!([{"op": "rework", "id": "a", "reason": "the review asked for it",
                "instruction": "Change what the comments name",
                "comments": comments.iter().map(|comment| named(comment)).collect::<Vec<_>>()}]),
    );
    assert_eq!(output.code, 0, "{}\n{}", output.stdout, output.stderr);
    let adopted = world.run(&["adopt", run]);
    assert_eq!(adopted.code, 0, "{}\n{}", adopted.stdout, adopted.stderr);
}

#[test]
fn a_rework_s_replies_are_drafted_once_and_posted_one_post_each_in_their_threads() {
    let world = team_world("reply-rework");
    let run = "answers";
    let (project, host) = launched_with_replies(&world, run);
    let first = conversation(501, 1);
    let second = threaded(601, 1, "a.md");
    add(&host, 1, first.clone());
    add(&host, 1, second.clone());
    review(&world, run);
    assert_eq!(surfaces(&world, run).len(), 2);
    rework(&world, run, &[&first, &second]);
    assert!(
        drafter_tasks(&world).is_empty(),
        "a reply was drafted before any pass"
    );
    let before = projections(&world, run);
    let tasks_before = world.store_tasks(&project);
    let ledger_from = host.ledger().expect("a ledger").len();

    review(&world, run);

    let tasks = drafter_tasks(&world);
    assert_eq!(tasks.len(), 1, "{tasks:#?}");
    let task = &tasks[0];
    let from = settled(&world, run, "a")["head"]
        .as_str()
        .unwrap()
        .to_owned();
    let to = settled(&world, run, "a-rework-1")["head"]
        .as_str()
        .unwrap()
        .to_owned();
    let added = commits_between(&world, &from, &to);
    assert!(!added.is_empty(), "the rework added no commit");
    let commits = section(task, "Commits");
    for sha in &added {
        assert!(
            commits.contains(sha.as_str()),
            "{sha} is not in:\n{commits}"
        );
    }
    assert!(section(task, "Change request").contains("https://github.com/owner/service/pull/1"));
    let listed = section(task, "Comments");
    for comment in [&first, &second] {
        assert!(
            listed.contains(&comment.id.0) && listed.contains(&comment.url),
            "{listed}"
        );
    }
    assert!(listed.contains("Location: a.md:1"), "{listed}");
    assert!(section(task, "Reason")
        .contains("the review asked for it\n\nChange what the comments name"));
    assert!(
        !section(task, "Worker transcript").contains(NO_TRANSCRIPT),
        "{task}"
    );

    let posted = world.events_of(run, "review-reply-posted");
    assert_eq!(posted.len(), 2, "{posted:#?}");
    for comment in [&first, &second] {
        let reply = the_reply_to(&host, 1, comment);
        let marker = key("a", &comment.id.0, &comment.updated_at);
        assert!(
            reply.body.contains(&format!("key={marker}")),
            "{}",
            reply.body
        );
        assert!(
            added
                .iter()
                .any(|sha| reply.body.contains(&format!("/commit/{sha}"))),
            "{} links none of {added:?}",
            reply.body
        );
        let record = posted
            .iter()
            .find(|event| event["payload"]["comment"] == comment.id.0)
            .expect("its reply is journalled");
        assert_eq!(record["payload"]["node"], "a-rework-1");
        assert_eq!(record["payload"]["disposition"], "answered");
        assert_eq!(record["payload"]["url"], reply.url);
        assert!(added.contains(&record["payload"]["commits"][0].as_str().unwrap().to_owned()));
    }
    // A thread comment's reply is in its thread; the conversation comment's links it.
    assert!(the_reply_to(&host, 1, &first)
        .body
        .starts_with(&format!("Re: {}", first.url)));
    assert_eq!(
        the_reply_to(&host, 1, &second).in_reply_to,
        Some(second.id.clone())
    );

    // Each reply is one POST, with no read of its thread between the pass's read and it.
    let ledger = host.ledger().expect("a ledger");
    let methods: Vec<&str> = ledger[ledger_from..]
        .iter()
        .map(|call| call.method.as_str())
        .collect();
    assert_eq!(
        methods,
        ["review_standings", "reply_to_comment", "reply_to_comment"],
        "{methods:?}"
    );
    assert_eq!(posts(&world).len(), 2, "{:#?}", posts(&world));
    // Nothing of a reply is written to the store, and no thread is resolved.
    assert_eq!(projections(&world, run), before);
    assert_eq!(world.store_tasks(&project), tasks_before);
    assert!(host.state().unwrap().review_comments[&ChangeId("1".into())]
        .iter()
        .all(|comment| !matches!(
            comment.kind,
            CommentKind::ReviewThread { resolved: true, .. }
        )));

    // Answered is read off the host: the next pass drafts and posts nothing.
    review(&world, run);
    assert_eq!(drafter_tasks(&world).len(), 1);
    assert_eq!(posts(&world).len(), 2);
    let results = world.run(&["results", run, "--json"]);
    let results: Value = serde_json::from_str(&results.stdout).unwrap();
    let node = results["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "a-rework-1")
        .unwrap()
        .clone();
    for comment in node["comments"].as_array().unwrap() {
        assert_eq!(comment["state"], "answered", "{comment}");
        assert_eq!(comment["rework"], "a-rework-1", "{comment}");
    }
}

/// The newest answer the run's driver wrote to its command queue.
pub(crate) fn last_outcome(world: &World, run: &str) -> Value {
    std::fs::read_to_string(world.run_file(run, "channel/command-outcomes.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .next_back()
        .unwrap_or_else(|| panic!("run {run} has no command outcome"))
}

/// A `decline` of `comments` on `node`'s draft, sent on the planner's channel.
pub(crate) fn decline(
    world: &World,
    run: &str,
    node: &str,
    comments: &[&HostComment],
    reason: &str,
) -> crate::harness::Run {
    reply(
        world,
        run,
        json!([{"op": "decline", "id": node,
                "comments": comments.iter().map(|comment| named(comment)).collect::<Vec<_>>(),
                "reason": reason}]),
    )
}

/// The head `branch` stands at on the origin.
pub(crate) fn origin_head(world: &World, branch: &str) -> String {
    let read = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(world.root.join("origin.git"))
        .args(["rev-parse", &format!("refs/heads/{branch}")])
        .output()
        .expect("git runs");
    String::from_utf8_lossy(&read.stdout).trim().to_owned()
}

/// What `## Reason` holds, exactly: the text between its heading and the next.
pub(crate) fn reason_of(task: &str) -> &str {
    task.split("\n## Reason\n\n")
        .nth(1)
        .and_then(|rest| rest.split("\n\n## Worker transcript\n").next())
        .unwrap_or_else(|| panic!("the task has no `## Reason`:\n{task}"))
}

/// The body a reply carries above `onevcs`'s marker, without a conversation reply's
/// `Re:` line.
pub(crate) fn said(reply: &HostComment) -> String {
    let body = reply
        .body
        .split("<!-- onevcs:reply ")
        .next()
        .unwrap_or_default();
    let body = match body.strip_prefix("Re: ") {
        Some(rest) => rest.split_once("\n\n").map_or("", |(_, said)| said),
        None => body,
    };
    body.trim_end().to_owned()
}

#[test]
fn a_decline_posts_one_declined_reply_and_is_the_whole_of_its_record_on_every_host() {
    let world = team_world("decline");
    let run = "declined";
    let (project, host) = launched_with_replies(&world, run);
    let left = threaded(801, 1, "a.md");
    add(&host, 1, left.clone());
    review(&world, run);
    assert_eq!(surfaces(&world, run).len(), 1);
    let branch = settled(&world, run, "a")["branch"]
        .as_str()
        .unwrap()
        .to_owned();
    let head = origin_head(&world, &branch);
    let workers = worker_dispatches(&world, "a");
    let before = projections(&world, run);
    let tasks_before = world.store_tasks(&project);
    let reason = "Out of scope for this change:  the cache is tracked in #42.\n";

    let output = decline(&world, run, "a", &[&left], reason);
    assert_eq!(output.code, 0, "{}\n{}", output.stdout, output.stderr);

    let tasks = drafter_tasks(&world);
    assert_eq!(tasks.len(), 1, "{tasks:#?}");
    assert_eq!(section(&tasks[0], "Commits").trim(), "", "{}", tasks[0]);
    assert_eq!(
        reason_of(&tasks[0]),
        reason,
        "the reason was not handed over byte-for-byte"
    );
    assert!(section(&tasks[0], "Worker transcript").contains(NO_TRANSCRIPT));
    let posted = the_reply_to(&host, 1, &left);
    assert_eq!(
        posted.in_reply_to,
        Some(left.id.clone()),
        "not in the comment's thread"
    );
    let text = said(&posted);
    assert!(!text.is_empty() && text.chars().count() <= 500, "{text}");
    assert!(!text.contains("/commit/"), "{text}");
    assert!(
        posted.body.contains(&format!(
            "key={} label=declined",
            key("a", &left.id.0, &left.updated_at)
        )),
        "{}",
        posted.body
    );
    let applied = world
        .events_of(run, "edit-committed")
        .into_iter()
        .filter(|event| event["payload"]["operation_kinds"] == json!(["decline-applied"]))
        .collect::<Vec<_>>();
    assert_eq!(applied.len(), 1, "{applied:#?}");
    assert_eq!(
        applied[0]["payload"]["operations"][0]["replies"],
        json!([posted.url])
    );
    assert_eq!(applied[0]["payload"]["operations"][0]["reason"], reason);
    // Posted before the edit committed: the host's stamp is no later than the record's.
    assert!(posted.created_at.as_str() <= applied[0]["ts"].as_str().unwrap());
    let recorded = world.events_of(run, "review-reply-posted");
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0]["payload"]["disposition"], "declined");
    assert_eq!(recorded[0]["payload"]["commits"], json!([]));
    // No worker, no branch moved, nothing written to the store, no thread resolved.
    assert_eq!(worker_dispatches(&world, "a"), workers);
    assert_eq!(origin_head(&world, &branch), head);
    assert_eq!(projections(&world, run), before);
    assert_eq!(world.store_tasks(&project), tasks_before);
    assert!(host.state().unwrap().review_comments[&ChangeId("1".into())]
        .iter()
        .all(|comment| !matches!(
            comment.kind,
            CommentKind::ReviewThread { resolved: true, .. }
        )));

    // The identical decline again: refused by kind, naming the reply, with nothing posted.
    let again = decline(&world, run, "a", &[&left], reason);
    assert_eq!(again.code, 2, "{}\n{}", again.stdout, again.stderr);
    assert!(
        again
            .stderr
            .contains(&format!("already answered by {}", posted.url)),
        "{}",
        again.stderr
    );
    let rejected = world.events_of(run, "edit-rejected");
    let refusal = &rejected.last().expect("the refusal is journalled")["payload"]["refusal"];
    assert_eq!(refusal["kind"], "already-answered", "{refusal}");
    assert_eq!(refusal["url"], posted.url);
    assert_eq!(posts(&world).len(), 1);
    assert_eq!(drafter_tasks(&world).len(), 1);

    let status = world.run(&["status", run]);
    assert!(
        status.stdout.contains(&format!("declined: {}", posted.url)),
        "{}",
        status.stdout
    );

    // And a cold host, every local record deleted, reads it declined off the host alone.
    let cold = world.on_host("cold-decline-host");
    std::fs::remove_dir_all(&world.runs).unwrap();
    std::fs::remove_dir_all(world.onevcs_home()).unwrap();
    let continued = cold.run_from(&cold.project, &["start", &project, "--attach"]);
    assert_eq!(
        continued.code, 0,
        "{}\n{}",
        continued.stdout, continued.stderr
    );
    let status = cold.run(&["status", run]);
    assert!(
        status.stdout.contains(&format!("declined: {}", posted.url)),
        "{}",
        status.stdout
    );
}

/// How many times `gh` was asked a repository's own record — its visibility — since
/// the `from`th invocation.
fn visibility_probes(world: &World, from: usize) -> usize {
    world.invocations()[from..]
        .iter()
        .filter(|call| call["tool"] == "gh")
        .filter(|call| {
            let args: Vec<&str> = call["args"]
                .as_array()
                .map(|args| args.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            args.len() == 2 && args[0] == "api" && args[1].split('/').count() == 3
        })
        .count()
}

/// A command asks the host a repository's visibility once however often its boundary
/// resolves — and, where the host would not say, asks it again at each resolution rather
/// than keeping a refusal as an answer.
#[test]
fn a_command_asks_a_repositorys_visibility_once_and_again_only_where_the_host_did_not_say() {
    let world = team_world("visibility-probed");
    let run = "probed";
    let (_, host) = launched_with_replies(&world, run);
    let first = threaded(811, 1, "a.md");
    let second = threaded(812, 1, "a.md");
    add(&host, 1, first.clone());
    add(&host, 1, second.clone());
    review(&world, run);

    // The host refuses the read: nothing is kept, so it is asked again.
    world.script("gh.visibility", "unsayable");
    let from = world.invocations().len();
    let output = decline(&world, run, "a", &[&first], "Out of scope here.");
    assert_eq!(output.code, 0, "{}\n{}", output.stdout, output.stderr);
    let refused = visibility_probes(&world, from);
    assert!(
        refused > 2,
        "a refused visibility was kept: asked {refused} times"
    );

    // The host answers: the decline's command asks once, and `onevcs`'s guard on the
    // reply it posts once.
    world.script("gh.visibility", "private");
    let from = world.invocations().len();
    let output = decline(&world, run, "a", &[&second], "Out of scope here.");
    assert_eq!(output.code, 0, "{}\n{}", output.stdout, output.stderr);
    assert_eq!(visibility_probes(&world, from), 2);
    the_reply_to(&host, 1, &first);
    the_reply_to(&host, 1, &second);
}

/// A run of `a` and `b` settled as drafts while `hold`, after them, is held
/// open — so a driver is live and every command goes through its queue, and is
/// answered in the outcome document. At `concurrency` above one the two drafts may
/// open in either order, so the host double is seeded by each one's own number,
/// answered beside it.
fn driven(
    world: &World,
    run: &str,
    extra: &[&str],
    concurrency: u64,
) -> (std::process::Child, FileHost, u32) {
    world.script("a.work", "first draft\n");
    world.script("b.work", "second draft\n");
    world.script("hold.wait", "");
    let mut plan = crate::harness::plan_of(
        run,
        vec![
            crate::harness::lifecycle("a", &[]),
            crate::harness::lifecycle("b", &[]),
            // On `b` alone, so `a` has no dependent in flight and may be reworked;
            // at a concurrency of one, in plan order, it starts after both drafts.
            crate::harness::agent("hold", &["b"]),
        ],
    );
    plan["concurrency"] = json!(concurrency);
    let project = world.plan(run, &plan);
    let mut args = vec!["start", project.as_str(), "--attach"];
    args.extend_from_slice(extra);
    let mut driving = world.cmd(&args);
    driving
        .current_dir(&world.project)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let driver = driving.spawn().expect("the driver starts");
    world.until("both drafts to settle", |world| {
        ["a", "b"].iter().all(|node| {
            world
                .events_of(run, "node-settled")
                .iter()
                .any(|event| event["labels"]["node"] == *node)
        })
    });
    let number = |node: &str| -> u32 {
        settled(world, run, node)["change_url"]
            .as_str()
            .and_then(|url| url.rsplit('/').next())
            .and_then(|number| number.parse().ok())
            .expect("a draft's change request number")
    };
    let mut state = onevcs_testing::HostState::default();
    for node in ["a", "b"] {
        let settlement = settled(world, run, node);
        state.changes.push(
            serde_json::from_value(json!({
                "id": number(node).to_string(), "url": settlement["change_url"],
                "head_sha": settlement["head"], "base": settlement["base"],
            }))
            .expect("the real publication supplied this change request"),
        );
        state.heads.insert(
            ChangeId(number(node).to_string()),
            settlement["branch"].as_str().unwrap().to_owned(),
        );
    }
    let host = FileHost::seeded(world.fakes.join("review-host.json"), state).expect("host seeded");
    (driver, host, number("a"))
}

/// Let the held node finish, and wait for the driver to end the run.
fn released(world: &World, mut driver: std::process::Child) {
    world.script("hold.go", "");
    let ended = driver.wait().expect("the driver ends");
    assert!(ended.success(), "{ended:?}");
}

/// The refusal a decline's outcome carries, its reason, and the run's posts after it.
fn refused(world: &World, run: &str, output: &crate::harness::Run) -> (Value, String) {
    assert_eq!(output.code, 2, "{}\n{}", output.stdout, output.stderr);
    let outcome = last_outcome(world, run);
    assert_eq!(outcome["applied"], false, "{outcome}");
    let result = &outcome["results"][0];
    assert_eq!(result["op"], "decline");
    assert_eq!(result["outcome"], "refused");
    let reason = result["reason"].as_str().expect("a reason").to_owned();
    assert!(
        output.stderr.contains(&reason),
        "{}\n{reason}",
        output.stderr
    );
    (result["refusal"].clone(), reason)
}

#[test]
fn every_decline_refusal_reaches_the_outcome_by_kind_and_posts_nothing_it_should_not() {
    let world = team_world("decline-refusals");
    let run = "refusals";
    let graph = world.review_reply_graph();
    let (driver, host, _) = driven(&world, run, &["--review-reply-graph", &graph], 1);
    let first = conversation(701, 1);
    let second = conversation(702, 1);
    add(&host, 1, first.clone());
    add(&host, 1, second.clone());
    review(&world, run);
    assert_eq!(surfaces(&world, run).len(), 2);
    let unread = conversation(703, 1);
    add(&host, 1, unread.clone());
    let pending = |world: &World| {
        let status = world.run(&["status", run]);
        for comment in [&first, &second] {
            assert!(
                status
                    .stdout
                    .contains(&format!("review {}: pending", comment.id.0)),
                "{}",
                status.stdout
            );
        }
    };

    let (refusal, reason) = refused(
        &world,
        run,
        &decline(&world, run, "a", &[&unread], "not ours"),
    );
    assert_eq!(
        refusal,
        json!({"kind": "comment-not-read", "node": "a", "comment": unread.id.0}),
        "{reason}"
    );
    let (refusal, _) = refused(&world, run, &decline(&world, run, "a", &[&first], "   "));
    assert_eq!(refusal, json!({"kind": "blank-reason", "node": "a"}));
    assert!(
        drafter_tasks(&world).is_empty(),
        "a refused decline was drafted"
    );

    // A drafting that fails, and each result the rules refuse: nothing is posted.
    world.script("review-reply.fail", "1");
    let (refusal, _) = refused(
        &world,
        run,
        &decline(&world, run, "a", &[&first], "out of scope"),
    );
    assert_eq!(refusal["kind"], "decline-not-posted", "{refusal}");
    assert_eq!(refusal["node"], "a");
    assert_eq!(refusal["comments"], json!([first.id.0]));
    world.unscript("review-reply.fail");
    let one = |disposition: &str, body: String| {
        json!({"replies": [{"comment": first.id.0, "disposition": disposition, "body": body}]})
            .to_string()
    };
    for (result, says) in [
        (json!({"replies": []}).to_string(), "has no reply"),
        (one("declined", "   ".to_owned()), "is empty"),
        (one("declined", "x".repeat(501)), "over the 500"),
        (
            one(
                "declined",
                "see https://github.com/owner/service/commit/0123456789abcdef".to_owned(),
            ),
            "carries a commit URL",
        ),
        (one("answered", "fixed".to_owned()), "drafted as `answered`"),
    ] {
        world.script("review-reply.result", &result);
        let (refusal, reason) = refused(
            &world,
            run,
            &decline(&world, run, "a", &[&first], "out of scope"),
        );
        assert_eq!(refusal["kind"], "decline-not-posted", "{refusal}");
        assert!(
            refusal["detail"].as_str().unwrap().contains(says),
            "{refusal}"
        );
        assert!(reason.contains(says), "{reason}");
        pending(&world);
    }
    world.unscript("review-reply.result");
    assert!(posts(&world).is_empty(), "{:#?}", posts(&world));
    assert_eq!(surfaces(&world, run).len(), 2);

    // Two comments, the second post refused: the first reply stays, and the same
    // decline again posts only the second.
    world.script("gh.reply.fail-after", "1");
    let (refusal, _) = refused(
        &world,
        run,
        &decline(&world, run, "a", &[&first, &second], "both out of scope"),
    );
    assert_eq!(refusal["kind"], "decline-not-posted", "{refusal}");
    assert_eq!(refusal["comments"], json!([first.id.0, second.id.0]));
    assert_eq!(
        posts(&world).len(),
        2,
        "the first post, and the refused second"
    );
    let kept = the_reply_to(&host, 1, &first);
    assert!(replies(&host, 1)
        .iter()
        .all(|reply| !reply.body.contains(&format!("in-reply-to={}", second.id.0))));
    world.unscript("gh.reply.fail-after");
    let resent = decline(&world, run, "a", &[&first, &second], "both out of scope");
    assert_eq!(resent.code, 0, "{}\n{}", resent.stdout, resent.stderr);
    assert_eq!(posts(&world).len(), 3, "only the second was posted again");
    let completed = the_reply_to(&host, 1, &second);
    assert_eq!(the_reply_to(&host, 1, &first).url, kept.url);
    let applied = world
        .events_of(run, "edit-committed")
        .into_iter()
        .filter(|event| event["payload"]["operation_kinds"] == json!(["decline-applied"]))
        .collect::<Vec<_>>();
    assert_eq!(applied.len(), 1);
    assert_eq!(
        applied[0]["payload"]["operations"][0]["replies"],
        json!([kept.url, completed.url])
    );
    assert_eq!(
        last_outcome(&world, run)["results"][0]["refusal"],
        Value::Null
    );

    // Sent once more, every comment is declined already.
    let (refusal, _) = refused(
        &world,
        run,
        &decline(&world, run, "a", &[&first, &second], "both out of scope"),
    );
    assert_eq!(
        refusal,
        json!({"kind": "already-answered", "node": "a", "comment": first.id.0, "url": kept.url})
    );
    assert_eq!(posts(&world).len(), 3);
    released(&world, driver);
}

#[test]
fn a_decline_with_no_reply_drafter_is_refused_and_posts_nothing() {
    let world = team_world("decline-undrafted");
    let run = "undrafted";
    launched_with(&world, run, &[]);
    let host = host(&world, run, &["a", "b"]);
    let left = conversation(901, 1);
    add(&host, 1, left.clone());
    review(&world, run);
    let output = decline(&world, run, "a", &[&left], "out of scope");
    assert_eq!(output.code, 2, "{}\n{}", output.stdout, output.stderr);
    assert!(
        output.stderr.contains("no reply drafter is configured"),
        "{}",
        output.stderr
    );
    let rejected = world.events_of(run, "edit-rejected");
    let refusal = &rejected.last().unwrap()["payload"]["refusal"];
    assert_eq!(refusal["kind"], "decline-not-posted", "{refusal}");
    assert!(posts(&world).is_empty());
    assert!(drafter_tasks(&world).is_empty());
    let status = world.run(&["status", run]);
    assert!(
        status
            .stdout
            .contains(&format!("review {}: pending", left.id.0)),
        "{}",
        status.stdout
    );
}

/// Every `review-reply-pending` surface the run raised.
pub(crate) fn pending_replies(world: &World, run: &str) -> Vec<Value> {
    std::fs::read_to_string(world.run_file(run, "channel/surfaces.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|surface| surface["kind"] == "review-reply-pending")
        .collect()
}

#[test]
fn a_rework_s_drafting_that_fails_or_answers_against_the_rules_posts_nothing_until_it_drafts() {
    let world = team_world("reply-refused");
    let run = "refused";
    let (_, host) = launched_with_replies(&world, run);
    let first = conversation(511, 1);
    let second = conversation(512, 1);
    add(&host, 1, first.clone());
    add(&host, 1, second.clone());
    review(&world, run);
    rework(&world, run, &[&first, &second]);
    let from = settled(&world, run, "a")["head"]
        .as_str()
        .unwrap()
        .to_owned();
    let to = settled(&world, run, "a-rework-1")["head"]
        .as_str()
        .unwrap()
        .to_owned();
    let link = format!(
        "https://github.com/owner/service/commit/{}",
        commits_between(&world, &from, &to)[0]
    );
    let reply_to = |comment: &HostComment, disposition: &str, body: &str| json!({"comment": comment.id.0, "disposition": disposition, "body": body});
    let answered =
        |comment: &HostComment| reply_to(comment, "answered", &format!("Done in {link}"));
    let cases = [
        (
            "a named comment missing",
            json!({"replies": [answered(&first)]}),
            "has no reply",
        ),
        (
            "a comment answered twice",
            json!({"replies": [answered(&first), answered(&first), answered(&second)]}),
            "answered twice",
        ),
        (
            "an empty body",
            json!({"replies": [reply_to(&first, "answered", " "), answered(&second)]}),
            "is empty",
        ),
        (
            "a body over 500 characters",
            json!({"replies": [
                reply_to(&first, "answered", &format!("{link} {}", "x".repeat(500))),
                answered(&second)]}),
            "over the 500",
        ),
        (
            "an answered body linking none of the commits",
            json!({"replies": [reply_to(&first, "answered", "Done."), answered(&second)]}),
            "links none",
        ),
        (
            "a declined body linking a commit",
            json!({"replies": [reply_to(&first, "declined", &format!("Not this: {link}")), answered(&second)]}),
            "links a commit",
        ),
        (
            "an answered body whose only link extends a commit's URL",
            json!({"replies": [reply_to(&first, "answered", &format!("Done in {link}/files")),
                answered(&second)]}),
            "links none",
        ),
        (
            "an answered body carrying a commit's URL inside another URL",
            json!({"replies": [
                reply_to(&first, "answered", &format!("Done in https://example.invalid/{link}")),
                answered(&second)]}),
            "links none",
        ),
        (
            "a reply to a comment the drafting did not name",
            json!({"replies": [answered(&first), answered(&second),
                {"comment": "IC_never_named", "disposition": "answered",
                 "body": format!("Done in {link}")}]}),
            "did not name",
        ),
    ];
    let mut journalled = 0;
    for (case, result, says) in cases {
        world.script("review-reply.result", &result.to_string());
        review(&world, run);
        journalled += 1;
        let records = world.events_of(run, "review-reply-not-drafted");
        assert_eq!(records.len(), journalled, "{case}: {records:#?}");
        let record = &records.last().unwrap()["payload"];
        assert_eq!(record["node"], "a-rework-1", "{case}");
        assert_eq!(record["ending"], "invalid", "{case}");
        assert_eq!(
            record["comments"],
            json!([first.id.0, second.id.0]),
            "{case}"
        );
        assert!(
            record["detail"].as_str().unwrap().contains(says),
            "{case}: {record}"
        );
        assert!(posts(&world).is_empty(), "{case}: something was posted");
        assert!(
            !pending_replies(&world, run).is_empty(),
            "{case}: nothing was surfaced"
        );
    }
    world.unscript("review-reply.result");
    world.script("review-reply.fail", "1");
    review(&world, run);
    let records = world.events_of(run, "review-reply-not-drafted");
    assert_eq!(records.last().unwrap()["payload"]["ending"], "failed");
    assert!(posts(&world).is_empty());
    let surfaced = pending_replies(&world, run);
    assert_eq!(
        surfaced.len(),
        2,
        "one per ending, not one per pass: {surfaced:#?}"
    );
    assert!(surfaced.iter().all(|surface| surface["blocking"] == false));
    assert!(surfaced[0]["message"]
        .as_str()
        .unwrap()
        .contains("onepipeline review"));

    // Drafting succeeds, and the next pass posts both.
    world.unscript("review-reply.fail");
    review(&world, run);
    assert_eq!(posts(&world).len(), 2);
    for comment in [&first, &second] {
        assert!(the_reply_to(&host, 1, comment).body.contains(&link));
    }
    assert_eq!(world.events_of(run, "review-reply-posted").len(), 2);
}

#[test]
fn with_no_reply_drafter_a_rework_s_replies_stay_owed_and_surfaced() {
    let world = team_world("reply-unconfigured");
    let run = "unconfigured";
    launched_with(&world, run, &[]);
    let host = host(&world, run, &["a", "b"]);
    let comment = conversation(521, 1);
    add(&host, 1, comment.clone());
    review(&world, run);
    rework(&world, run, &[&comment]);
    review(&world, run);
    review(&world, run);
    let records = world.events_of(run, "review-reply-not-drafted");
    assert_eq!(records.len(), 2, "{records:#?}");
    assert!(records
        .iter()
        .all(|record| record["payload"]["ending"] == "unconfigured"));
    assert_eq!(pending_replies(&world, run).len(), 1);
    assert!(posts(&world).is_empty());
    assert!(drafter_tasks(&world).is_empty());
    let status = world.run(&["status", run]);
    assert!(
        status.stdout.contains(&format!(
            "review {}: being reworked by a-rework-1",
            comment.id.0
        )),
        "{}",
        status.stdout
    );
}

#[test]
fn an_edit_after_its_reply_is_pending_again_and_a_rework_naming_it_answers_the_new_revision() {
    let world = team_world("reply-edited");
    let run = "edited";
    let (_, host) = launched_with_replies(&world, run);
    let comment = conversation(531, 1);
    add(&host, 1, comment.clone());
    review(&world, run);
    rework(&world, run, &[&comment]);
    review(&world, run);
    let first = the_reply_to(&host, 1, &comment);
    assert!(first
        .body
        .contains(&key("a", &comment.id.0, &comment.updated_at)));

    // Second granularity is what a commit's own clock keeps: the edit lands a whole
    // second after the rework's commits, and the next rework's a second after it.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    host.edit_comment(
        &ChangeId("1".into()),
        &comment.id,
        "Please change 531 another way",
    )
    .expect("the reviewer edits the comment");
    let revised = host.state().unwrap().review_comments[&ChangeId("1".into())]
        .iter()
        .find(|held| held.id == comment.id)
        .unwrap()
        .updated_at
        .clone();
    review(&world, run);
    let surfaced = surfaces(&world, run);
    assert_eq!(surfaced.len(), 2, "{surfaced:#?}");
    assert!(surfaced[1]["correlation"]
        .as_str()
        .unwrap()
        .ends_with(&revised));
    assert_eq!(
        posts(&world).len(),
        1,
        "the earlier rework answered an edit it never saw"
    );
    let status = world.run(&["status", run]);
    assert!(
        status
            .stdout
            .contains(&format!("review {}: pending", comment.id.0)),
        "{}",
        status.stdout
    );

    std::thread::sleep(std::time::Duration::from_millis(1100));
    world.script("a-rework-2.work", "reworked again, as the edit asked\n");
    let output = reply(
        &world,
        run,
        json!([{"op": "rework", "id": "a-rework-1", "reason": "the comment was edited",
                "instruction": "Change it another way", "comments": [named(&comment)]}]),
    );
    assert_eq!(output.code, 0, "{}\n{}", output.stdout, output.stderr);
    assert_eq!(world.run(&["adopt", run]).code, 0);
    review(&world, run);
    let answered: Vec<HostComment> = replies(&host, 1)
        .into_iter()
        .filter(|reply| {
            reply
                .body
                .contains(&format!("in-reply-to={}", comment.id.0))
        })
        .collect();
    assert_eq!(answered.len(), 2, "{answered:#?}");
    assert_eq!(answered[0], first, "the first reply was changed");
    assert!(
        answered[1]
            .body
            .contains(&key("a", &comment.id.0, &revised)),
        "{}",
        answered[1].body
    );
    let from = settled(&world, run, "a-rework-1")["head"]
        .as_str()
        .unwrap()
        .to_owned();
    let to = settled(&world, run, "a-rework-2")["head"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(commits_between(&world, &from, &to)
        .iter()
        .any(|sha| answered[1].body.contains(sha.as_str())));
}

/// Every branch the origin holds.
pub(crate) fn origin_branches(world: &World) -> Vec<String> {
    let listed = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(world.root.join("origin.git"))
        .args(["for-each-ref", "--format=%(refname:short)", "refs/heads"])
        .output()
        .expect("git runs");
    String::from_utf8_lossy(&listed.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Delete every record this host keeps — its runs root and `ONEVCS_HOME` — and
/// continue the project from its store on a fresh host, launched with the reply
/// drafter.
pub(crate) fn continued_cold(world: &World, project: &str, host: &str) -> World {
    let cold = world.on_host(host);
    std::fs::remove_dir_all(&world.runs).unwrap();
    std::fs::remove_dir_all(world.onevcs_home()).unwrap();
    let graph = world.review_reply_graph();
    let continued = cold.run_from(
        &cold.project,
        &["start", project, "--attach", "--review-reply-graph", &graph],
    );
    assert_eq!(
        continued.code, 0,
        "{}\n{}",
        continued.stdout, continued.stderr
    );
    cold
}

#[test]
fn a_cold_host_reworks_a_stack_on_its_existing_branch_and_change_request_and_replies() {
    let world = team_world("reply-cold-stack");
    let run = "cold-stack";
    let graph = world.review_reply_graph();
    world.script("a.work", "first draft\n");
    world.script("b.work", "second draft, stacked\n");
    let mut plan = crate::harness::plan_of(
        run,
        vec![
            crate::harness::lifecycle("a", &[]),
            crate::harness::lifecycle("b", &["a"]),
        ],
    );
    plan["concurrency"] = json!(1);
    let project = world.plan(run, &plan);
    let launched = world.run_from(
        &world.project,
        &[
            "start",
            &project,
            "--attach",
            "--review-reply-graph",
            &graph,
        ],
    );
    assert_eq!(launched.code, 0, "{}\n{}", launched.stdout, launched.stderr);
    let host = host(&world, run, &["a", "b"]);
    let comment = conversation(541, 1);
    add(&host, 1, comment.clone());
    let opened = world.changes_opened().len();
    assert_eq!(opened, 2);
    let branches = origin_branches(&world);
    let original_head = settled(&world, run, "a")["head"]
        .as_str()
        .unwrap()
        .to_owned();

    let cold = continued_cold(&world, &project, "cold-stack-host");
    review(&cold, run);
    let found = surfaces(&cold, run);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0]["correlation"]
        .as_str()
        .unwrap()
        .contains(&comment.id.0));
    let b_workers = worker_dispatches(&cold, "b");
    rework(&cold, run, &[&comment]);
    let restacked = cold.events_of(run, "node-restacked");
    let b = restacked
        .iter()
        .find(|event| event["payload"]["node"] == "b")
        .unwrap_or_else(|| panic!("b was not restacked: {restacked:#?}"));
    assert_eq!(b["payload"]["agent_dispatched"], false);
    assert_eq!(
        worker_dispatches(&cold, "b"),
        b_workers,
        "the restack dispatched b's worker"
    );
    review(&cold, run);

    let reply = the_reply_to(&host, 1, &comment);
    assert!(reply
        .body
        .contains(&key("a", &comment.id.0, &comment.updated_at)));
    let reworked = settled(&cold, run, "a-rework-1")["head"]
        .as_str()
        .unwrap()
        .to_owned();
    let added = commits_between(&world, &original_head, &reworked);
    assert!(!added.is_empty());
    assert!(
        added.iter().any(|sha| reply.body.contains(sha.as_str())),
        "{}",
        reply.body
    );
    assert_eq!(drafter_tasks(&cold).len(), 1);
    // The original run's two change requests and branches, and nothing beyond them.
    assert_eq!(cold.changes_opened().len(), opened);
    assert_eq!(origin_branches(&world), branches);
    assert_eq!(
        settled(&cold, run, "a-rework-1")["change_url"],
        settled(&cold, run, "a")["change_url"]
    );
}

/// The crash seam a debug binary stops at: `stage` reached, the process killed.
fn crashed_at(world: &World, args: &[&str], stdin: Option<&str>, stage: &str) {
    let seam = world.root.join(format!("reply-crash-{stage}"));
    std::fs::create_dir_all(&seam).unwrap();
    std::fs::write(seam.join(stage), "").unwrap();
    let mut process = world
        .cmd(args)
        .env("ONEPIPELINE_REVIEW_FAILPOINT", &seam)
        .stdin(if stdin.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(crate::review::Held)
        .unwrap();
    if let Some(text) = stdin {
        use std::io::Write;
        process
            .0
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }
    world.until("the crash boundary", |_| {
        seam.join(format!("{stage}.reached")).is_file()
    });
    process.0.kill().unwrap();
    process.0.wait().unwrap();
}

#[test]
fn a_host_lost_after_the_rework_settled_leaves_its_replies_to_the_next_host() {
    let world = team_world("reply-lost-before-drafting");
    let run = "lost";
    let (project, host) = launched_with_replies(&world, run);
    let first = conversation(551, 1);
    let second = conversation(552, 1);
    add(&host, 1, first.clone());
    add(&host, 1, second.clone());
    review(&world, run);
    // Two commits — the worker's own, and the one its publication makes of what it
    // left — so only the superseded head the store carries tells them from the
    // original worker's.
    world.script(
        "a-rework-1.commit-saying",
        "fix: change what the review named",
    );
    world.script("a-rework-1.work-anew", "and what it named next");
    rework(&world, run, &[&first, &second]);
    assert!(drafter_tasks(&world).is_empty());
    let from = settled(&world, run, "a")["head"]
        .as_str()
        .unwrap()
        .to_owned();
    let to = settled(&world, run, "a-rework-1")["head"]
        .as_str()
        .unwrap()
        .to_owned();
    let added = commits_between(&world, &from, &to);
    assert_eq!(added.len(), 2, "{added:?}");

    let cold = continued_cold(&world, &project, "cold-before-drafting");
    review(&cold, run);
    review(&cold, run);
    let tasks = drafter_tasks(&cold);
    assert_eq!(tasks.len(), 1, "{tasks:#?}");
    assert!(section(&tasks[0], "Worker transcript").contains(NO_TRANSCRIPT));
    assert!(reason_of(&tasks[0]).starts_with("the review asked for it"));
    let commits = section(&tasks[0], "Commits");
    for sha in &added {
        assert!(
            commits.contains(sha.as_str()),
            "{sha} is not in:\n{commits}"
        );
    }
    assert!(
        !commits.contains(&from),
        "the superseded head is not the rework's:\n{commits}"
    );
    for comment in [&first, &second] {
        the_reply_to(&host, 1, comment);
    }
    assert_eq!(worker_dispatches(&cold, "a-rework-1"), 1);
    assert_eq!(worker_dispatches(&cold, "a"), 1);
    assert!(
        surfaces(&cold, run).is_empty(),
        "{:#?}",
        surfaces(&cold, run)
    );
}

#[test]
fn a_host_lost_after_the_host_took_a_reply_leaves_nothing_to_post_again() {
    let world = team_world("reply-lost-after-post");
    let run = "lost";
    let (project, host) = launched_with_replies(&world, run);
    let comment = conversation(561, 1);
    add(&host, 1, comment.clone());
    review(&world, run);
    rework(&world, run, &[&comment]);
    crashed_at(&world, &["review", run], None, "after-reply-post");
    let posted = the_reply_to(&host, 1, &comment);
    assert!(world.events_of(run, "review-reply-posted").is_empty());

    let cold = continued_cold(&world, &project, "cold-after-post");
    review(&cold, run);
    assert_eq!(drafter_tasks(&cold).len(), 1, "the cold host drafted again");
    assert_eq!(posts(&cold).len(), 1, "the cold host posted again");
    assert_eq!(the_reply_to(&host, 1, &comment), posted);
    let status = cold.run(&["status", run]);
    assert!(
        status
            .stdout
            .contains(&format!("review {}: answered by a-rework-1", comment.id.0)),
        "{}",
        status.stdout
    );
    assert_eq!(worker_dispatches(&cold, "a-rework-1"), 1);
    assert!(
        surfaces(&cold, run).is_empty(),
        "{:#?}",
        surfaces(&cold, run)
    );
}

#[test]
fn a_host_lost_after_a_decline_posted_leaves_the_decline_answered_on_the_next_host() {
    let world = team_world("reply-lost-decline");
    let run = "lost";
    let (project, host) = launched_with_replies(&world, run);
    let comment = conversation(571, 1);
    add(&host, 1, comment.clone());
    review(&world, run);
    let envelope = json!({"version": 3, "commands": [{"op": "decline", "id": "a",
        "comments": [named(&comment)], "reason": "out of scope"}]})
    .to_string();
    crashed_at(
        &world,
        &["reply", run],
        Some(&envelope),
        "after-decline-post",
    );
    let posted = the_reply_to(&host, 1, &comment);
    assert!(posted.body.contains("label=declined"));
    assert!(world
        .events_of(run, "edit-committed")
        .iter()
        .all(|event| event["payload"]["operation_kinds"] != json!(["decline-applied"])));

    let cold = continued_cold(&world, &project, "cold-after-decline");
    review(&cold, run);
    assert_eq!(drafter_tasks(&cold).len(), 1, "the cold host drafted again");
    assert_eq!(posts(&cold).len(), 1, "the cold host posted again");
    let status = cold.run(&["status", run]);
    assert!(
        status.stdout.contains(&format!("declined: {}", posted.url)),
        "{}",
        status.stdout
    );
    let again = decline(&cold, run, "a", &[&comment], "out of scope");
    assert_eq!(again.code, 2, "{}\n{}", again.stdout, again.stderr);
    assert!(
        again
            .stderr
            .contains(&format!("already answered by {}", posted.url)),
        "{}",
        again.stderr
    );
    assert_eq!(the_reply_to(&host, 1, &comment), posted);
    assert!(
        surfaces(&cold, run).is_empty(),
        "{:#?}",
        surfaces(&cold, run)
    );
}

#[test]
fn a_driven_review_still_drafting_at_the_reply_timeout_returns_requested_and_posts_after() {
    let world = team_world("reply-driven");
    let run = "driven";
    let graph = world.review_reply_graph();
    let (driver, host, number) = driven(&world, run, &["--review-reply-graph", &graph], 2);
    let first = conversation(581, number);
    let second = conversation(582, number);
    add(&host, number, first.clone());
    add(&host, number, second.clone());
    review(&world, run);
    world.script("a-rework-1.work", "reworked as the review asked\n");
    let output = reply(
        &world,
        run,
        json!([{"op": "rework", "id": "a", "reason": "the review asked for it",
                "instruction": "Change what the comments name",
                "comments": [named(&first), named(&second)]}]),
    );
    assert_eq!(output.code, 0, "{}\n{}", output.stdout, output.stderr);
    world.until("the rework to settle", |world| {
        world
            .events_of(run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "a-rework-1")
    });
    world.script("review-reply.wait", "");
    let reads = host
        .ledger()
        .unwrap()
        .iter()
        .filter(|call| call.method == "review_standings")
        .count();
    let receipt = world
        .cmd(&["review", run, "--json"])
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("review runs");
    assert!(receipt.status.success(), "{receipt:?}");
    let receipt: Value = serde_json::from_slice(&receipt.stdout).unwrap();
    assert_eq!(receipt["state"], "requested", "{receipt}");
    assert!(posts(&world).is_empty());
    world.script("review-reply.go", "");
    world.until("the driver to post both replies", |world| {
        world.events_of(run, "review-reply-posted").len() == 2
    });
    for comment in [&first, &second] {
        the_reply_to(&host, number, comment);
    }
    assert_eq!(drafter_tasks(&world).len(), 1);
    assert_eq!(
        host.ledger()
            .unwrap()
            .iter()
            .filter(|call| call.method == "review_standings")
            .count(),
        reads + 1,
        "a second pass read the host"
    );
    released(&world, driver);
}

#[test]
fn both_views_on_both_hosts_name_the_rework_and_commits_that_answered_and_the_decline() {
    let world = team_world("reply-views");
    let run = "views";
    let (project, host) = launched_with_replies(&world, run);
    let first = conversation(591, 1);
    let second = threaded(592, 1, "a.md");
    let third = conversation(593, 1);
    for comment in [&first, &second, &third] {
        add(&host, 1, comment.clone());
    }
    review(&world, run);
    rework(&world, run, &[&first, &second]);
    review(&world, run);
    let left = decline(&world, run, "a-rework-1", &[&third], "out of scope here");
    assert_eq!(left.code, 0, "{}\n{}", left.stdout, left.stderr);
    let linked = |comment: &HostComment| -> Vec<String> {
        let body = the_reply_to(&host, 1, comment).body;
        body.split("/commit/")
            .skip(1)
            .map(|rest| rest.chars().take(7).collect())
            .collect()
    };
    let declined = the_reply_to(&host, 1, &third).url;

    let check = |world: &World, node: &str| {
        for verb in ["status", "results"] {
            let output = world.run(&[verb, run]);
            assert_eq!(output.code, 0, "{verb}: {}", output.stderr);
            for comment in [&first, &second] {
                let shas = linked(comment);
                assert!(!shas.is_empty());
                let line = format!(
                    "{node} review {}: answered by a-rework-1 ({}): ",
                    comment.id.0,
                    shas.join(", ")
                );
                assert!(
                    output.stdout.contains(&line),
                    "{verb} lacks {line:?}:\n{}",
                    output.stdout
                );
            }
            assert!(
                output.stdout.contains(&format!(
                    "{node} review {}: declined: {declined}",
                    third.id.0
                )),
                "{verb}:\n{}",
                output.stdout
            );
            assert!(
                !output.stdout.contains(": pending"),
                "{verb}:\n{}",
                output.stdout
            );
        }
        let json = world.run(&["results", run, "--json"]);
        assert_eq!(json.code, 0, "{}", json.stderr);
        let json: Value = serde_json::from_str(&json.stdout).unwrap();
        assert_eq!(json["schema_version"], 2);
        let comments = json["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"] == node)
            .unwrap()["comments"]
            .clone();
        for comment in [&first, &second] {
            let entry = comments
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["id"] == comment.id.0)
                .unwrap();
            assert_eq!(entry["state"], "answered", "{entry}");
            assert_eq!(entry["rework"], "a-rework-1", "{entry}");
            assert_eq!(entry["commits"], json!(linked(comment)), "{entry}");
        }
        let entry = comments
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"] == third.id.0)
            .unwrap();
        assert_eq!(entry["state"], "declined");
        assert_eq!(entry["reply_url"], declined);
    };
    check(&world, "a-rework-1");
    let cold = continued_cold(&world, &project, "cold-views");
    check(&cold, "a");
}

/// The refusal an undriven `decline` was journalled with, as `edit-rejected` carries it.
fn rejected(world: &World, run: &str, output: &crate::harness::Run) -> Value {
    assert_eq!(output.code, 2, "{}\n{}", output.stdout, output.stderr);
    world
        .events_of(run, "edit-rejected")
        .last()
        .expect("the decline was rejected")["payload"]["refusal"]
        .clone()
}

/// The graph a reply-drafter dispatch ran, resolved.
fn drafter_graphs(world: &World) -> Vec<std::path::PathBuf> {
    world
        .invocations()
        .iter()
        .filter(|call| {
            call["tool"] == "oneagentgraph"
                && call["args"].as_array().is_some_and(|args| {
                    args.iter()
                        .any(|arg| arg == "onepipeline.persona=review-reply")
                })
        })
        .filter_map(|call| call["args"][1].as_str())
        .map(|graph| std::fs::canonicalize(graph).expect("the dispatched graph resolves"))
        .collect()
}

/// The reply drafter is named by the launch config's `review_reply_graph` on the
/// drafting graph's terms: a reference relative to the launch directory, recorded
/// resolved; `--review-reply-graph` over the config; and a blank rung — the key or the
/// flag — naming none, so a decline is refused `decline-not-posted` and posts nothing.
#[test]
fn the_launch_config_names_the_reply_drafter_the_flag_overrides_it_and_a_blank_names_none() {
    let version = onepipeline::filter::LAUNCH_CONFIG_SCHEMA_VERSION;
    let declined = |name: &str, key: &str, extra: &[&str]| {
        let world = team_world(name);
        let declared = std::path::PathBuf::from(world.review_reply_graph());
        let overriding = world.graphs().join("review-reply-override.yaml");
        std::fs::copy(&declared, &overriding).expect("the overriding graph is written");
        let config = world.root.join("launch.yaml");
        std::fs::write(&config, format!("schema_version: {version}\n{key}"))
            .expect("the launch config is written");
        let config = config.to_string_lossy().into_owned();
        let mut args = vec!["--launch-config", config.as_str()];
        args.extend_from_slice(extra);
        // Launched from the project directory, which `../graphs` is written against.
        launched_with(&world, "graphs", &args);
        let host = host(&world, "graphs", &["a", "b"]);
        let left = conversation(1101, 1);
        add(&host, 1, left.clone());
        review(&world, "graphs");
        let output = decline(&world, "graphs", "a", &[&left], "out of scope here");
        let recorded = world.run_json("graphs", "launch.json")["review_reply_graph"].clone();
        (world, host, left, output, recorded, declared, overriding)
    };
    let resolved = |recorded: &Value| {
        std::fs::canonicalize(recorded.as_str().expect("a recorded graph"))
            .expect("the recorded graph resolves")
    };

    let configured = "review_reply_graph: ../graphs/review-reply.yaml\n";
    let (world, host, left, output, recorded, declared, _) =
        declined("reply-graph-config", configured, &[]);
    assert_eq!(output.code, 0, "{}\n{}", output.stdout, output.stderr);
    assert!(
        recorded.as_str().is_some_and(|path| path.starts_with('/')),
        "{recorded}"
    );
    let declared = std::fs::canonicalize(declared).unwrap();
    assert_eq!(resolved(&recorded), declared);
    assert_eq!(drafter_graphs(&world), [declared]);
    assert!(the_reply_to(&host, 1, &left)
        .body
        .contains("label=declined"));

    let (world, host, left, output, recorded, _, overriding) = declined(
        "reply-graph-flag",
        configured,
        &[
            "--review-reply-graph",
            "../graphs/review-reply-override.yaml",
        ],
    );
    assert_eq!(output.code, 0, "{}\n{}", output.stdout, output.stderr);
    let overriding = std::fs::canonicalize(overriding).unwrap();
    assert_eq!(resolved(&recorded), overriding);
    assert_eq!(drafter_graphs(&world), [overriding]);
    the_reply_to(&host, 1, &left);

    for (name, key, extra) in [
        (
            "reply-graph-blank-flag",
            configured,
            &["--review-reply-graph", ""][..],
        ),
        (
            "reply-graph-blank-key",
            "review_reply_graph: \"\"\n",
            &[][..],
        ),
    ] {
        let (world, host, _, output, recorded, _, _) = declined(name, key, extra);
        assert!(recorded.is_null(), "{name}: {recorded}");
        let refusal = rejected(&world, "graphs", &output);
        assert_eq!(refusal["kind"], "decline-not-posted", "{name}: {refusal}");
        assert!(drafter_graphs(&world).is_empty(), "{name}");
        assert!(posts(&world).is_empty(), "{name}");
        assert!(replies(&host, 1).is_empty(), "{name}");
    }
}

/// A drafter answer its graph's schema refuses is never read: a decline whose every
/// answer the schema refused, or whose drafter answered nothing structured, is refused
/// `decline-not-posted` saying which, with nothing posted; and one whose first harness's
/// answer the schema refused posts the answer after it.
#[test]
fn a_drafter_answer_its_schema_refuses_is_never_read_and_the_one_after_it_is() {
    let world = team_world("reply-schema");
    let run = "schema";
    let (_, host) = launched_with_replies(&world, run);
    let left = conversation(1111, 1);
    add(&host, 1, left.clone());
    review(&world, run);
    let refused_by_schema = json!({"replies": [{"comment": left.id.0,
        "disposition": "declined", "body": "FIRST-NEVER-READ", "mood": "tired"}]})
    .to_string();

    world.script("review-reply.result", &refused_by_schema);
    let refusal = rejected(
        &world,
        run,
        &decline(&world, run, "a", &[&left], "out of scope"),
    );
    assert_eq!(refusal["kind"], "decline-not-posted", "{refusal}");
    assert!(
        refusal["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("refused by its schema")),
        "{refusal}"
    );
    world.unscript("review-reply.result");

    world.script("review-reply.unanswered", "");
    let refusal = rejected(
        &world,
        run,
        &decline(&world, run, "a", &[&left], "out of scope"),
    );
    assert_eq!(refusal["kind"], "decline-not-posted", "{refusal}");
    assert!(
        refusal["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("answered nothing its schema accepted")),
        "{refusal}"
    );
    world.unscript("review-reply.unanswered");
    assert!(posts(&world).is_empty());
    assert!(replies(&host, 1).is_empty());

    world.script("review-reply.refused-first", &refused_by_schema);
    let output = decline(&world, run, "a", &[&left], "out of scope");
    assert_eq!(output.code, 0, "{}\n{}", output.stdout, output.stderr);
    let reply = the_reply_to(&host, 1, &left);
    assert!(!reply.body.contains("FIRST-NEVER-READ"), "{}", reply.body);
    assert!(
        reply.body.contains("Left as it is: out of scope"),
        "{}",
        reply.body
    );
    assert_eq!(drafter_tasks(&world).len(), 3);
}

/// A rework's reply the host refuses is left owed: the pass fails naming it, the reply
/// the host took before it stays and is never posted again, and the next pass drafts and
/// posts only the one refused.
#[test]
fn a_rework_s_reply_the_host_refuses_is_posted_by_the_next_pass_and_the_one_it_took_is_not_again() {
    let world = team_world("reply-post-refused");
    let run = "refusedpost";
    let (_, host) = launched_with_replies(&world, run);
    let first = conversation(1121, 1);
    let second = conversation(1122, 1);
    add(&host, 1, first.clone());
    add(&host, 1, second.clone());
    review(&world, run);
    world.script("gh.reply.fail-after", "1");
    rework(&world, run, &[&first, &second]);
    let pass = world.run(&["review", run]);
    assert_ne!(pass.code, 0, "{}\n{}", pass.stdout, pass.stderr);
    assert!(
        pass.stderr
            .contains(&format!("the reply to {} was not posted", second.id.0)),
        "{}",
        pass.stderr
    );
    the_reply_to(&host, 1, &first);
    let marker = format!("in-reply-to={}", second.id.0);
    assert!(replies(&host, 1)
        .iter()
        .all(|reply| !reply.body.contains(&marker)));
    assert_eq!(world.events_of(run, "review-reply-posted").len(), 1);

    world.unscript("gh.reply.fail-after");
    review(&world, run);
    the_reply_to(&host, 1, &first);
    the_reply_to(&host, 1, &second);
    let tasks = drafter_tasks(&world);
    assert_eq!(tasks.len(), 2, "{tasks:#?}");
    let comments = section(&tasks[1], "Comments");
    assert!(
        comments.contains(&format!("### {}", second.id.0)),
        "{comments}"
    );
    assert!(
        !comments.contains(&format!("### {}", first.id.0)),
        "{comments}"
    );
    assert_eq!(world.events_of(run, "review-reply-posted").len(), 2);
}

/// Drop `onepipeline.rework`'s `replaces_head` from every document of `world`'s store
/// carrying it — the record as a build before the key wrote it.
fn as_an_older_build_wrote_it(world: &World) {
    let mut pending = vec![world.store()];
    let mut rewritten = 0;
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("the store reads") {
            let path = entry.expect("a store entry").path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("a store document reads");
            if !text.contains("replaces_head") {
                continue;
            }
            let rest = text.strip_prefix("---\n").expect("front matter");
            let (front, body) = rest.split_once("\n---\n").expect("front matter ends");
            let mut front: Value = serde_norway::from_str(front).expect("front matter parses");
            let record = front["metadata"]["onepipeline.rework"]
                .as_object_mut()
                .expect("a rework record");
            assert!(record.remove("replaces_head").is_some());
            let front = serde_norway::to_string(&front).expect("front matter renders");
            std::fs::write(&path, format!("---\n{front}---\n{body}"))
                .expect("the document is written");
            rewritten += 1;
        }
    }
    assert_eq!(rewritten, 1, "one rework record carries the key");
}

/// A cold host continuing from a stored rework record that names no superseded head —
/// one a build before the key wrote — drafts the reply owed with the replacement's
/// settled head as the only commit, and posts it linking that commit.
#[test]
fn a_stored_rework_record_naming_no_superseded_head_drafts_from_the_replacement_s_head_alone() {
    let world = team_world("reply-older-record");
    let run = "older";
    let project = launched_with(&world, run, &[]);
    let host = host(&world, run, &["a", "b"]);
    let left = conversation(1131, 1);
    add(&host, 1, left.clone());
    review(&world, run);
    rework(&world, run, &[&left]);
    let branch = settled(&world, run, "a-rework-1")["branch"]
        .as_str()
        .expect("a branch")
        .to_owned();
    let head = origin_head(&world, &branch);
    world.until_store("the rework's record to reach the store", |world| {
        world
            .store_tasks(&project)
            .iter()
            .any(|task| task["item"]["metadata"]["onepipeline.rework"]["replaces_head"].is_string())
    });
    as_an_older_build_wrote_it(&world);
    assert!(world.store_tasks(&project).iter().all(|task| {
        task["item"]["metadata"]["onepipeline.rework"]
            .get("replaces_head")
            .is_none()
    }));
    assert!(drafter_tasks(&world).is_empty());

    let cold = continued_cold(&world, &project, "cold-older");
    review(&cold, run);
    let tasks = drafter_tasks(&cold);
    assert_eq!(tasks.len(), 1, "{tasks:#?}");
    let listed: Vec<&str> = section(&tasks[0], "Commits")
        .lines()
        .filter(|line| line.starts_with("- "))
        .collect();
    assert_eq!(listed.len(), 1, "{listed:#?}");
    assert!(listed[0].starts_with(&format!("- {head} ")), "{listed:#?}");
    assert!(the_reply_to(&host, 1, &left)
        .body
        .contains(&format!("/commit/{head}")));
}

/// A decline whose own read of the change request fails is refused
/// `decline-not-posted` saying so, drafts and posts nothing, and posts once it reads.
#[test]
fn a_decline_whose_read_of_the_change_request_fails_posts_nothing_and_posts_once_it_reads() {
    let world = team_world("decline-read-fails");
    let run = "unread";
    let (_, host) = launched_with_replies(&world, run);
    let left = conversation(1141, 1);
    add(&host, 1, left.clone());
    review(&world, run);
    world.script("gh.outage", "");
    let refusal = rejected(
        &world,
        run,
        &decline(&world, run, "a", &[&left], "out of scope"),
    );
    assert_eq!(refusal["kind"], "decline-not-posted", "{refusal}");
    assert!(
        refusal["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("could not be read")),
        "{refusal}"
    );
    assert!(drafter_tasks(&world).is_empty());
    assert!(posts(&world).is_empty());
    world.unscript("gh.outage");
    let output = decline(&world, run, "a", &[&left], "out of scope");
    assert_eq!(output.code, 0, "{}\n{}", output.stdout, output.stderr);
    assert!(the_reply_to(&host, 1, &left)
        .body
        .contains("label=declined"));
}

/// A decline of a comment a rework's reply already answers is refused
/// `already-answered` naming that reply, with nothing drafted or posted.
#[test]
fn a_decline_of_a_comment_a_rework_answered_is_refused_already_answered_and_posts_nothing() {
    let world = team_world("decline-after-rework");
    let run = "answered";
    let (_, host) = launched_with_replies(&world, run);
    let left = conversation(1151, 1);
    add(&host, 1, left.clone());
    review(&world, run);
    rework(&world, run, &[&left]);
    review(&world, run);
    let answered = the_reply_to(&host, 1, &left);
    assert!(answered.body.contains("/commit/"), "{}", answered.body);
    let (drafted, posted) = (drafter_tasks(&world).len(), posts(&world).len());
    let refusal = rejected(
        &world,
        run,
        &decline(&world, run, "a-rework-1", &[&left], "never mind"),
    );
    assert_eq!(
        refusal,
        json!({"kind": "already-answered", "node": "a-rework-1", "comment": left.id.0,
               "url": answered.url})
    );
    assert_eq!(drafter_tasks(&world).len(), drafted);
    assert_eq!(posts(&world).len(), posted);
    assert_eq!(the_reply_to(&host, 1, &left).url, answered.url);
}

/// A decline whose drafting cannot start — no drafter binary to run — or cannot be given
/// a directory is refused `decline-not-posted` saying which, with nothing dispatched or
/// posted, and posts once the drafting can run.
#[test]
fn a_decline_whose_drafting_cannot_start_or_be_given_a_directory_posts_once_it_can() {
    let world = team_world("decline-drafting-unstartable");
    let run = "unstartable";
    let (_, host) = launched_with_replies(&world, run);
    let left = conversation(1161, 1);
    add(&host, 1, left.clone());
    review(&world, run);
    let envelope = json!({"version": 3, "commands": [{"op": "decline", "id": "a",
        "comments": [named(&left)], "reason": "out of scope"}]})
    .to_string();

    let mut command = world.cmd(&["reply", run]);
    command.env(
        "ONEPIPELINE_ONEAGENTGRAPH_BIN",
        world.root.join("no-such-oneagentgraph"),
    );
    let refusal = rejected(&world, run, &world.run_with_stdin_on(command, &envelope));
    assert_eq!(refusal["kind"], "decline-not-posted", "{refusal}");
    assert!(
        refusal["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("could not start")),
        "{refusal}"
    );

    // The run's `review-replies` a file, so no drafting's directory can be made in it.
    let blocked = world.run_file(run, "review-replies");
    let _ = std::fs::remove_dir_all(&blocked);
    std::fs::write(&blocked, "").expect("the blocking file is written");
    let refusal = rejected(
        &world,
        run,
        &world.run_with_stdin(&["reply", run], &envelope),
    );
    assert_eq!(refusal["kind"], "decline-not-posted", "{refusal}");
    assert!(
        refusal["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("could not be made")),
        "{refusal}"
    );
    assert!(drafter_tasks(&world).is_empty());
    assert!(posts(&world).is_empty());

    std::fs::remove_file(&blocked).expect("the blocking file is removed");
    let output = world.run_with_stdin(&["reply", run], &envelope);
    assert_eq!(output.code, 0, "{}\n{}", output.stdout, output.stderr);
    assert!(the_reply_to(&host, 1, &left)
        .body
        .contains("label=declined"));
    assert_eq!(drafter_tasks(&world).len(), 1);
    assert_eq!(posts(&world).len(), 1);
}

/// A rework whose commits cannot be read posts nothing: the pass journals
/// `review-reply-not-drafted` as a failed drafting and surfaces the replies pending,
/// with no drafter dispatched, and the next pass that can read them posts the reply.
#[test]
fn a_rework_whose_commits_cannot_be_read_surfaces_its_replies_and_posts_once_they_can() {
    let world = team_world("reply-commits-unread");
    let run = "unreadcommits";
    let (_, host) = launched_with_replies(&world, run);
    let left = conversation(1171, 1);
    add(&host, 1, left.clone());
    review(&world, run);
    rework(&world, run, &[&left]);
    // The run's `review-replies` a file, so the cache the commits are read through
    // cannot be made.
    let blocked = world.run_file(run, "review-replies");
    let _ = std::fs::remove_dir_all(&blocked);
    std::fs::write(&blocked, "").expect("the blocking file is written");
    review(&world, run);
    let records = world.events_of(run, "review-reply-not-drafted");
    let record = &records.last().expect("the drafting is journalled")["payload"];
    assert_eq!(record["ending"], "failed", "{record}");
    assert!(
        record["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("commits could not be read")),
        "{record}"
    );
    assert!(!pending_replies(&world, run).is_empty());
    assert!(drafter_tasks(&world).is_empty());
    assert!(replies(&host, 1).is_empty());

    std::fs::remove_file(&blocked).expect("the blocking file is removed");
    review(&world, run);
    assert!(the_reply_to(&host, 1, &left).body.contains("/commit/"));
    assert_eq!(drafter_tasks(&world).len(), 1);
}

/// A `decline` the plan's own terms refuse is refused naming why, with nothing drafted
/// or posted: a node the run has none of, a node with no repository, a lifecycle node
/// with no open change request, no comment named, a comment named by a blank id, an id
/// carrying a control character or a URL no web address is, and one comment named twice.
#[test]
fn a_decline_the_plan_refuses_names_why_and_posts_nothing() {
    let world = team_world("decline-plan-refusals");
    let run = "planrefusals";
    world.script("a.work", "first draft\n");
    world.script("d.fail", "1");
    let graph = world.review_reply_graph();
    let plan = crate::harness::plan_of(
        run,
        vec![
            crate::harness::lifecycle("a", &[]),
            crate::harness::agent("c", &[]),
            crate::harness::lifecycle("d", &[]),
        ],
    );
    let project = world.plan(run, &plan);
    world.run_from(
        &world.project,
        &[
            "start",
            &project,
            "--attach",
            "--review-reply-graph",
            &graph,
        ],
    );
    let host = host(&world, run, &["a"]);
    let left = conversation(1181, 1);
    add(&host, 1, left.clone());
    review(&world, run);
    let comment = |id: &str, url: &str| json!({"id": id, "url": url});
    let cases = [
        (
            "a node the run has none of",
            "zz",
            json!([named(&left)]),
            "no node 'zz'",
        ),
        (
            "a node with no repository",
            "c",
            json!([named(&left)]),
            "is not a lifecycle node",
        ),
        (
            "a lifecycle node with no open change request",
            "d",
            json!([named(&left)]),
            "has no open change request",
        ),
        ("no comment", "a", json!([]), "name at least one comment"),
        (
            "a blank comment id",
            "a",
            json!([comment(" ", &left.url)]),
            "is not a comment's id and URL",
        ),
        (
            "a comment id carrying a control character",
            "a",
            json!([comment("IC_\u{7}", &left.url)]),
            "is not a comment's id and URL",
        ),
        (
            "a URL no web address is",
            "a",
            json!([comment(&left.id.0, "ftp://example.invalid/1")]),
            "is not a comment's id and URL",
        ),
        (
            "one comment named twice",
            "a",
            json!([named(&left), named(&left)]),
            "twice",
        ),
    ];
    for (case, node, comments, says) in cases {
        let output = reply(
            &world,
            run,
            json!([{"op": "decline", "id": node, "comments": comments,
                    "reason": "out of scope"}]),
        );
        assert_eq!(
            output.code, 2,
            "{case}: {}\n{}",
            output.stdout, output.stderr
        );
        assert!(output.stderr.contains(says), "{case}: {}", output.stderr);
    }
    assert!(drafter_tasks(&world).is_empty());
    assert!(posts(&world).is_empty());
    assert!(replies(&host, 1).is_empty());
}

/// A rework's drafter may decline one comment it names while it answers another: the
/// declined reply carries no commit and is labelled `declined`, it is journalled with that
/// disposition and no commits, the views read it declined, and no later pass posts again.
#[test]
fn a_rework_s_drafter_may_decline_one_comment_while_it_answers_another() {
    let world = team_world("reply-rework-declines");
    let run = "partly";
    let (_, host) = launched_with_replies(&world, run);
    let first = conversation(1191, 1);
    let second = conversation(1192, 1);
    add(&host, 1, first.clone());
    add(&host, 1, second.clone());
    review(&world, run);
    rework(&world, run, &[&first, &second]);
    let from = settled(&world, run, "a")["head"]
        .as_str()
        .unwrap()
        .to_owned();
    let to = settled(&world, run, "a-rework-1")["head"]
        .as_str()
        .unwrap()
        .to_owned();
    let sha = commits_between(&world, &from, &to)[0].clone();
    let link = format!("https://github.com/owner/service/commit/{sha}");
    world.script(
        "review-reply.result",
        &json!({"replies": [
            {"comment": first.id.0, "disposition": "answered", "body": format!("Done in {link}")},
            {"comment": second.id.0, "disposition": "declined",
             "body": "Left as it is: that module is a change of its own."},
        ]})
        .to_string(),
    );
    review(&world, run);
    let answered = the_reply_to(&host, 1, &first);
    assert!(answered.body.contains(&link), "{}", answered.body);
    assert!(
        !answered.body.contains("label=declined"),
        "{}",
        answered.body
    );
    let declined = the_reply_to(&host, 1, &second);
    assert!(!declined.body.contains("/commit/"), "{}", declined.body);
    assert!(
        declined.body.contains("label=declined"),
        "{}",
        declined.body
    );
    let posted = world.events_of(run, "review-reply-posted");
    assert_eq!(posted.len(), 2, "{posted:#?}");
    let record = |comment: &HostComment| {
        posted
            .iter()
            .find(|event| event["payload"]["comment"] == comment.id.0)
            .expect("journalled")["payload"]
            .clone()
    };
    assert_eq!(record(&first)["disposition"], "answered");
    assert_eq!(record(&first)["commits"], json!([sha]));
    assert_eq!(record(&second)["disposition"], "declined");
    assert_eq!(record(&second)["commits"], json!([]));
    let status = world.run(&["status", run]);
    assert_eq!(status.code, 0, "{}", status.stderr);
    assert!(
        status.stdout.contains(&format!(
            "review {}: declined: {}",
            second.id.0, declined.url
        )),
        "{}",
        status.stdout
    );
    world.unscript("review-reply.result");
    let before = posts(&world).len();
    review(&world, run);
    assert_eq!(posts(&world).len(), before);
    assert_eq!(drafter_tasks(&world).len(), 1);
}

/// A driver lost after it journalled a decline's typed refusal and before it answered
/// the envelope leaves the next driver to answer it from the journal: the outcome carries
/// the refusal by kind beside its text, and the refusal is not journalled a second time.
#[test]
fn a_typed_refusal_a_lost_driver_never_answered_is_answered_from_the_journal_by_kind() {
    let world = team_world("decline-refusal-recovered");
    let run = "recovered";
    world.script("a.work", "first draft\n");
    world.script("b.work", "second draft\n");
    world.script("hold.wait", "");
    let graph = world.review_reply_graph();
    let mut plan = crate::harness::plan_of(
        run,
        vec![
            crate::harness::lifecycle("a", &[]),
            crate::harness::lifecycle("b", &[]),
            crate::harness::agent("hold", &["b"]),
        ],
    );
    plan["concurrency"] = json!(1);
    let project = world.plan(run, &plan);
    let seam = world.root.join("reply-crash-after-edit-rejected");
    std::fs::create_dir_all(&seam).unwrap();
    std::fs::write(seam.join("after-edit-rejected"), "").unwrap();
    let mut lost = world
        .cmd(&[
            "start",
            &project,
            "--attach",
            "--review-reply-graph",
            &graph,
        ])
        .current_dir(&world.project)
        .env("ONEPIPELINE_REVIEW_FAILPOINT", &seam)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(crate::review::Held)
        .unwrap();
    world.until("both drafts to settle", |world| {
        ["a", "b"].iter().all(|node| {
            world
                .events_of(run, "node-settled")
                .iter()
                .any(|event| event["labels"]["node"] == *node)
        })
    });
    let left = conversation(1211, 1);
    let mut sending = world.cmd(&["reply", run]);
    sending.env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "1");
    world.run_with_stdin_on(
        sending,
        &json!({"version": 3, "commands": [{"op": "decline", "id": "a",
            "comments": [named(&left)], "reason": "   "}]})
        .to_string(),
    );
    world.until("the refusal to be journalled and the driver held", |_| {
        seam.join("after-edit-rejected.reached").is_file()
    });
    lost.0.kill().unwrap();
    lost.0.wait().unwrap();
    assert_eq!(world.events_of(run, "edit-rejected").len(), 1);
    let answered = || {
        std::fs::read_to_string(world.run_file(run, "channel/command-outcomes.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .any(|outcome| outcome["results"][0]["op"] == "decline")
    };
    assert!(!answered(), "the lost driver answered the envelope");

    let mut next = world
        .cmd(&["adopt", run])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    world.until("the next driver to answer the envelope", |_| answered());
    let outcome = last_outcome(&world, run);
    assert_eq!(outcome["applied"], false, "{outcome}");
    let result = &outcome["results"][0];
    assert_eq!(result["outcome"], "refused", "{outcome}");
    assert_eq!(
        result["refusal"],
        json!({"kind": "blank-reason", "node": "a"}),
        "{outcome}"
    );
    assert_eq!(
        result["reason"], "refused: decline: node 'a': the reason cannot be blank",
        "{outcome}"
    );
    assert_eq!(world.events_of(run, "edit-rejected").len(), 1);
    world.script("hold.go", "");
    let ended = next.wait().expect("the next driver ends");
    assert!(ended.success(), "{ended:?}");
}
