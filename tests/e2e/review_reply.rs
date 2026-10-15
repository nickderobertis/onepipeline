//! Replies to review comments, drafted and posted by the compiled binary through
//! the real linked `onevcs` over real git, with GitHub at `onevcs`'s `ONEVCS_GH`
//! seam: `fake-gh` answers each reply POST through `onevcs-testing`'s remote
//! double, whose ledger and state are what these journeys read back. The reply
//! drafter is the scripted `oneagentgraph` double answering a `review-reply`
//! dispatch; nothing of the engine under test is stood in for.

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
                && args.iter().any(|arg| *arg == "--task")
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
