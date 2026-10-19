//! The busy review round `review-round-github-quota` measures, run whole through the
//! compiled binary and the real linked `onevcs` over real git and a real origin on disk,
//! and recorded as the telemetry that budget's script reads.
//!
//! GitHub stands in at `onevcs`'s `ONEVCS_GH` seam: `fake-gh` keeps each change
//! request's own record — opened, retargeted, merged by a person onto the origin — and,
//! under `gh.standings-own`, hands every read of feedback, every reply and every write to
//! `onevcs-testing`'s remote double, whose call ledger charges each call. The plan store
//! is a local Markdown store served by the real `local-md` plugin through the scripted
//! source, which records every call the engine makes of it. The agents are the scripted
//! `oneagentgraph` double.

// llmlint: ignore-file[e2e_not_mocked] the crate under test is the real compiled binary,
// `onevcs` — which reads, restacks, publishes, retargets and replies — is the real library
// over real git, and the store is the real `local-md` plugin. GitHub is substituted at
// `onevcs`'s own `ONEVCS_GH` override, the remote double charging each call as the spikes
// measured GitHub charge it, and `oneagentgraph` at its subprocess boundary, because the
// task states this budget's workload over onevcs-testing's remote double, fake agents and
// a local Markdown store: no check may spend the shared account's quota.

// llmlint: ignore-file[expensive_tests_stay_behind_their_own_edge] measured rather than
// assumed: the busy round runs in about 25–30 seconds on an idle Linux host, its other
// journeys in under two, and it was in this target before it counted its git starts or
// read its attach streams' renders. What it exercises is the whole of an attached command —
// `driver`, `engine`, `views`, `vcs` and `rendercost` over the real linked `onevcs` — which
// any change under `src/` can move, so a project edged narrower than the crate could not
// honestly run it: it would drop out of `nx affected` for the very changes it exists to
// catch, among them the cost on a slow host this module now bounds.

use crate::draft_lifecycle::{team_repository, GREEN};
use crate::harness::{lifecycle, plan_of, World};
use crate::review_reply::{conversation, drafter_tasks, key, named, the_reply_to};
use crate::stacked_drafts::{
    change_from, changed_against, held_as_draft, service, settled, PREFIX,
};
use onevcs::{ChangeId, CommentId, CommentKind};
use onevcs_testing::{FileHost, HostComment, HostState};
use serde_json::{json, Value};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Where the round's telemetry is written for the budget's script to read.
pub(crate) const RECORD: &str = "target/budget-records/review-round-quota.json";

/// The drafts of the round and what each is stacked on: ten, as deep as six, `n9` and
/// `n10` each on a stack base over two parents.
const STACK: [(&str, &[&str]); 10] = [
    ("n1", &[]),
    ("n2", &["n1"]),
    ("n3", &["n2"]),
    ("n4", &["n3"]),
    ("n5", &["n4"]),
    ("n6", &["n5"]),
    ("n7", &["n1"]),
    ("n8", &["n7"]),
    ("n9", &["n3", "n7"]),
    ("n10", &["n4", "n8"]),
];

/// How many earlier comments and replies each draft carries into the round.
const EARLIER: u64 = 30;

/// The commands a phase of the round ran, and what each source was asked during it.
#[derive(Default)]
struct Marks {
    ledger: usize,
    invocations: usize,
    projections: usize,
}

fn marks(world: &World, host: &FileHost, run: &str) -> Marks {
    Marks {
        ledger: host.ledger().expect("the double's ledger").len(),
        invocations: world.invocations().len(),
        projections: projection_lines(world, run).len(),
    }
}

fn projection_lines(world: &World, run: &str) -> Vec<Value> {
    std::fs::read_to_string(world.run_file(run, "writeback-projections.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// One `gh` invocation `fake-gh` answered from its own records — the calls the remote
/// double does not answer — charged as the host charges it: a `gh pr` command is one
/// GraphQL query, and a `gh api` route one REST request.
fn unledgered(args: &[String]) -> Option<Value> {
    let first = args.first().map(String::as_str)?;
    let second = args.get(1).map(String::as_str).unwrap_or_default();
    // Answered by the remote double, whose ledger already charges them: every GraphQL read,
    // every reply POST and every PATCH it is handed.
    if first == "api"
        && (second == "graphql" || args.iter().any(|arg| arg == "POST" || arg == "PATCH"))
    {
        return None;
    }
    let endpoint = if first == "api" {
        let route: Vec<&str> = second.split('/').collect();
        format!("REST {}", route.get(3).copied().unwrap_or("repository"))
    } else {
        format!("gh {first} {second}")
    };
    let (graphql, rest) = if first == "api" { (0, 1) } else { (1, 0) };
    Some(json!({"endpoint": endpoint, "graphql_points": graphql, "rest_requests": rest}))
}

/// What one phase of the round spent, out of what each source recorded during it.
fn phase(
    world: &World,
    host: &FileHost,
    run: &str,
    name: &str,
    from: &Marks,
    wall_ms: u128,
) -> Value {
    let ledger: Vec<Value> = host.ledger().expect("the double's ledger")[from.ledger..]
        .iter()
        .map(|call| {
            json!({"method": call.method, "changes": call.changes.len(),
                   "graphql_points": call.charge.graphql_points,
                   "rest_requests": call.charge.rest_requests})
        })
        .collect();
    let invocations = world.invocations();
    let since = &invocations[from.invocations..];
    let gh: Vec<Value> = since
        .iter()
        .filter(|call| call["tool"] == "gh")
        .filter_map(|call| {
            let args: Vec<String> = serde_json::from_value(call["args"].clone()).ok()?;
            unledgered(&args)
        })
        .collect();
    let store_calls: Vec<&str> = since
        .iter()
        .filter(|call| call["tool"] == crate::harness::SCRIPTED_KEY)
        .filter_map(|call| call["args"][0].as_str())
        .collect();
    // Every call the store received is charged below or is its session's handshake or close,
    // which reach no board: a call of a kind this list does not know fails the round rather
    // than going uncharged.
    let unknown: Vec<&&str> = store_calls
        .iter()
        .filter(|method| {
            ![
                "initialize",
                "end_command",
                "get_project",
                "query_tasks",
                "get_task",
                "task_dependencies",
                "update_task",
                "set_project_metadata",
            ]
            .contains(method)
        })
        .collect();
    assert!(
        unknown.is_empty(),
        "{name}: uncharged store calls {unknown:?}"
    );
    let reads: Vec<Value> = store_calls
        .iter()
        .copied()
        .filter(|method| {
            [
                "get_project",
                "query_tasks",
                "get_task",
                "task_dependencies",
            ]
            .contains(method)
        })
        .map(|method| json!(method))
        .collect();
    let mut writes: Vec<Value> = projection_lines(world, run)[from.projections..]
        .iter()
        .flat_map(|record| {
            let calls = record["calls"].clone();
            let mut written = Vec::new();
            for (kind, count) in calls.as_object().into_iter().flatten() {
                let count = count.as_u64().unwrap_or_default();
                let tasks = match kind.as_str() {
                    // A copy writes each item it carries; every other write names one.
                    "project-copy" => record["items"].as_array().map_or(0, Vec::len) as u64,
                    "task-update" | "project-metadata-set" => count,
                    _ => 0,
                };
                for _ in 0..tasks {
                    written.push(json!(kind));
                }
            }
            written
        })
        .collect();
    // The driver writes its claim's and its publications' keys on the project itself, outside
    // the write-back, so no projection record names those: each such write the store received
    // is charged as the task written it is, under the driver's own kind.
    for (method, projected, unprojected) in [
        ("update_task", "task-update", "driver-task-update"),
        (
            "set_project_metadata",
            "project-metadata-set",
            "driver-metadata-set",
        ),
    ] {
        let received = store_calls.iter().filter(|call| **call == method).count();
        let named = writes.iter().filter(|kind| **kind == projected).count();
        assert!(
            named <= received,
            "{name}: {named} {projected} projected over {received} {method} calls"
        );
        writes.extend(std::iter::repeat_n(json!(unprojected), received - named));
    }
    let dispatches = since
        .iter()
        .filter(|call| call["tool"] == "oneagentgraph")
        .filter_map(|call| {
            let args: Vec<&str> = call["args"]
                .as_array()?
                .iter()
                .filter_map(Value::as_str)
                .collect();
            args.contains(&"--task").then(|| {
                args.iter()
                    .find_map(|arg| arg.strip_prefix("onepipeline.persona="))
                    .map_or("worker", |persona| match persona {
                        "pr-author" | "review-reply" => persona,
                        _ => "worker",
                    })
                    .to_owned()
            })
        })
        .collect::<Vec<_>>();
    json!({"phase": name, "wall_ms": wall_ms as u64, "ledger": ledger, "gh": gh,
           "store_reads": reads, "store_writes": writes, "dispatches": dispatches})
}

/// Milliseconds since the epoch, on the clock the journal stamps with.
fn now_millis() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after the epoch")
            .as_millis(),
    )
    .expect("a millisecond count fits")
}

/// A journal stamp — `YYYY-MM-DDThh:mm:ss.sssZ` — as milliseconds since the epoch.
fn stamp_millis(stamp: &str) -> Option<u64> {
    let (date, time) = stamp.strip_suffix('Z')?.split_once('T')?;
    let mut date = date.split('-').map(|part| part.parse::<i64>().ok());
    let (year, month, day) = (date.next()??, date.next()??, date.next()??);
    let (clock, fraction) = time.split_once('.').unwrap_or((time, "0"));
    let mut clock = clock.split(':').map(|part| part.parse::<i64>().ok());
    let (hour, minute, second) = (clock.next()??, clock.next()??, clock.next()??);
    let millis: i64 = format!("{fraction:0<3}")[..3].parse().ok()?;
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year - era * 400;
    let of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let days = era * 146_097 + of_era * 365 + of_era / 4 - of_era / 100 + of_year - 719_468;
    u64::try_from(((days * 86_400 + hour * 3_600 + minute * 60 + second) * 1000) + millis).ok()
}

/// A pass's wall time split by its steps, off the journal's own stamps: the
/// readjustment until discovery's first read is journalled, discovery until the first
/// reply is drafted, and the replies after it.
fn pass_split(world: &World, run: &str, from: u64, to: u64) -> Value {
    let stamped: Vec<(u64, Value)> = world
        .journal(run)
        .into_iter()
        .filter_map(|event| Some((stamp_millis(event["ts"].as_str()?)?, event)))
        .filter(|(at, _)| (from..=to).contains(at))
        .collect();
    let first = |wanted: &dyn Fn(&Value) -> bool| {
        stamped
            .iter()
            .find(|(_, event)| wanted(event))
            .map_or(to, |(at, _)| *at)
    };
    let read = first(&|event| event["kind"] == "review-comments-read");
    let drafting = first(&|event| {
        event["labels"]["persona"] == "review-reply"
            || event["kind"] == "review-reply-posted"
            || event["kind"] == "review-reply-not-drafted"
    })
    .max(read);
    json!({"readjust_ms": read.saturating_sub(from), "discovery_ms": drafting - read,
           "replies_ms": to.saturating_sub(drafting)})
}

/// Run `step`, and record what the round spent while it ran — and, for a pass, its
/// wall time by step.
fn timed(
    world: &World,
    host: &FileHost,
    run: &str,
    name: &str,
    phases: &mut Vec<Value>,
    step: impl FnOnce(),
) {
    let from = marks(world, host, run);
    let began = std::time::Instant::now();
    let since = now_millis();
    step();
    let mut spent = phase(world, host, run, name, &from, began.elapsed().as_millis());
    if name.starts_with("pass-") {
        spent["pass"] = pass_split(world, run, since, now_millis());
    }
    phases.push(spent);
}

/// The change request number of a node's draft, off its settlement.
fn number_of(world: &World, run: &str, node: &str) -> u32 {
    settled(world, run, node)["change_url"]
        .as_str()
        .and_then(|url| url.rsplit('/').next())
        .and_then(|number| number.parse().ok())
        .unwrap_or_else(|| panic!("{node} has no change request number"))
}

/// The remote double, seeded with every draft by its own number and, on each, its
/// earlier comments — each answered by a marked reply carrying its revision's key, so the
/// round starts with nothing pending.
fn seeded(world: &World, run: &str) -> FileHost {
    let mut state = HostState::default();
    for (node, _) in STACK {
        let settlement = settled(world, run, node);
        let number = number_of(world, run, node);
        state.changes.push(
            serde_json::from_value(json!({
                "id": number.to_string(), "url": settlement["change_url"],
                "head_sha": settlement["head"], "base": settlement["base"],
            }))
            .expect("the real publication supplied this change request"),
        );
        state.heads.insert(
            ChangeId(number.to_string()),
            settlement["branch"].as_str().unwrap().to_owned(),
        );
    }
    let host = FileHost::seeded(world.fakes.join("review-host.json"), state).expect("host seeded");
    for (at, (node, _)) in STACK.iter().enumerate() {
        let number = number_of(world, run, node);
        for nth in 0..EARLIER / 2 {
            let earlier = conversation(10_000 + at as u64 * 100 + nth, number);
            host.add_comment(&ChangeId(number.to_string()), earlier.clone())
                .expect("an earlier comment");
            let reply = HostComment {
                id: CommentId(crate::review_reply::node_id(
                    "IC",
                    20_000 + at as u64 * 100 + nth,
                )),
                kind: CommentKind::Conversation,
                author: "onepipeline-e2e".to_owned(),
                body: format!(
                    "Re: {}\n\nDone.\n<!-- onevcs:reply in-reply-to={} key={} -->",
                    earlier.url,
                    earlier.id.0,
                    key(node, &earlier.id.0, &earlier.updated_at)
                ),
                url: format!(
                    "https://github.com/owner/service/pull/{number}#issuecomment-{}",
                    20_000 + at as u64 * 100 + nth
                ),
                created_at: crate::review::AT.to_owned(),
                updated_at: crate::review::AT.to_owned(),
                in_reply_to: None,
                revision: 0,
            };
            host.add_comment(&ChangeId(number.to_string()), reply)
                .expect("an earlier reply");
        }
    }
    host
}

fn passed(output: &crate::harness::Run, what: &str) {
    assert_eq!(
        output.code, 0,
        "{what}:\n{}\n{}",
        output.stdout, output.stderr
    );
}

/// Where `bash` resolves on `path`, given the Windows directory `system_root` names.
///
/// Not `Command::new("bash")`, which on Windows finds the WSL launcher first — the reason
/// `tests/linked_engines.rs` gives. Windows spells that directory in either case,
/// `C:\Windows` beside `C:\WINDOWS\system32`, so it is compared without case. Taken as
/// parameters so that the case only Windows produces is driven on every platform.
fn bash_on(path: &OsStr, system_root: Option<&OsStr>) -> PathBuf {
    let windows_dir = system_root.map(|root| root.to_string_lossy().to_lowercase());
    std::env::split_paths(path)
        .filter(|dir| {
            windows_dir.as_ref().is_none_or(|root| {
                !dir.to_string_lossy()
                    .to_lowercase()
                    .starts_with(root.as_str())
            })
        })
        .flat_map(|dir| ["bash", "bash.exe"].map(|name| dir.join(name)))
        .find(|candidate| candidate.is_file())
        .expect("a bash on PATH outside the Windows system directory runs the reporter")
}

/// The reporter, run from `cwd` by the `bash` [`bash_on`] finds on `path`, with `path` as
/// its own `PATH`.
///
/// The script is named with `/`: it finds its module through `$(dirname "$0")`, and `/` is
/// the separator that both `dirname` and `node` read on every platform.
fn reporter(cwd: &Path, path: &OsStr, system_root: Option<&OsStr>) -> std::process::Command {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/review-round-quota-budget.sh");
    let mut command = std::process::Command::new(bash_on(path, system_root));
    command
        .arg(script.to_string_lossy().replace('\\', "/"))
        .current_dir(cwd)
        .env("PATH", path);
    command
}

/// What `scripts/review-round-quota-budget.sh` reports over `record`, run from `cwd`.
pub(crate) fn budget_report(cwd: &Path, result: &Path) -> std::process::Output {
    reporter(
        cwd,
        &std::env::var_os("PATH").unwrap_or_default(),
        std::env::var_os("SystemRoot").as_deref(),
    )
    .env("ONEBUDGETSPEC_RESULT", result)
    .output()
    .expect("the committed reporter runs")
}

#[test]
fn the_busy_review_round_ends_answered_and_true_and_records_what_it_spent() {
    let world = World::new("review-round")
        .with_env("ONEVCS_BRANCH_PREFIX", PREFIX)
        .through_scripted_source();
    team_repository(&world);
    world.script("gh.checks", GREEN);
    #[cfg(unix)]
    let world = world.counting_git();
    let graph = world.review_reply_graph();
    let run = "round";
    let nodes: Vec<Value> = STACK
        .iter()
        .map(|(node, deps)| {
            world.script(&format!("{node}.work"), &format!("{node}'s own change\n"));
            lifecycle(node, deps)
        })
        .collect();
    // At the plan's own concurrency: the round reads each draft's number off its
    // settlement, so nothing needs the host to number them in plan order.
    let plan = plan_of(run, nodes);
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
    passed(&launched, "the stack settles");
    let host = seeded(&world, run);
    world.script("gh.standings-own", "");

    // Six new comments across three drafts.
    let (n1, n3, n7) = (
        number_of(&world, run, "n1"),
        number_of(&world, run, "n3"),
        number_of(&world, run, "n7"),
    );
    let on_b: Vec<HostComment> = (0..3).map(|nth| conversation(600 + nth, n3)).collect();
    let on_h: Vec<HostComment> = (0..2).map(|nth| conversation(610 + nth, n7)).collect();
    let left = conversation(620, n1);
    for comment in &on_b {
        host.add_comment(&ChangeId(n3.to_string()), comment.clone())
            .unwrap();
    }
    for comment in &on_h {
        host.add_comment(&ChangeId(n7.to_string()), comment.clone())
            .unwrap();
    }
    host.add_comment(&ChangeId(n1.to_string()), left.clone())
        .unwrap();

    let mut phases = Vec::new();
    let round_from = projection_lines(&world, run).len();
    let mut reworked = Vec::new();
    // Each rework's attach stream, as it printed and recorded its renders.
    let mut streamed = Vec::new();
    timed(&world, &host, run, "pass-1", &mut phases, || {
        passed(&world.run(&["review", run]), "the first pass");
    });
    timed(&world, &host, run, "decline", &mut phases, || {
        passed(
            &world.run_with_stdin(
                &["reply", run],
                &json!({"version": 3, "commands": [{"op": "decline", "id": "n1",
                    "comments": [named(&left)], "reason": "Out of scope for this stack."}]})
                .to_string(),
            ),
            "the decline",
        );
    });
    for (name, node, comments, file) in [
        ("rework-b", "n3", &on_b, "n3"),
        ("rework-h", "n7", &on_h, "n7"),
    ] {
        world.script(
            &format!("{node}-rework-1.work"),
            &format!("{node}, as the review asked\n"),
        );
        world.script(&format!("{node}-rework-1.work-file"), file);
        let from = projection_lines(&world, run).len();
        timed(&world, &host, run, name, &mut phases, || {
            passed(
                &world.run_with_stdin(
                    &["reply", run],
                    &json!({"version": 3, "commands": [{"op": "rework", "id": node,
                        "reason": "the review asked for it", "instruction": "Change what the comments name",
                        "comments": comments.iter().map(named).collect::<Vec<_>>()}]})
                    .to_string(),
                ),
                name,
            );
            let record = world.root.join(format!("{name}.reads"));
            let adopted = world.run_recording_renders(&record, &["adopt", run]);
            passed(&adopted, name);
            streamed.push((record, adopted.stderr.clone()));
        });
        reworked.push((node, from, projection_lines(&world, run).len()));
    }
    // The bottom draft merged by a person mid-round, its branch kept — no call of the run's.
    let person = std::process::Command::new(crate::harness::double("fake-gh"))
        .args([
            "__person",
            "merge",
            "owner/service",
            &n1.to_string(),
            "squash",
            "kept",
        ])
        .env(onepipeline_testfakes::SCRIPT_DIR_ENV, &world.fakes)
        .output()
        .expect("the host double runs");
    assert!(
        person.status.success(),
        "{}",
        String::from_utf8_lossy(&person.stderr)
    );
    timed(&world, &host, run, "readjust", &mut phases, || {
        passed(&world.run(&["readjust", run]), "the readjustment");
    });
    timed(&world, &host, run, "pass-2", &mut phases, || {
        passed(&world.run(&["review", run]), "the second pass");
    });
    #[cfg(unix)]
    the_round_started_few_enough_gits(&world);
    // After the bound, so a round that costs too much is told so before anything about
    // how its streams spent it.
    for (record, stderr) in &streamed {
        moved_drafts_were_read_again(&world, record, stderr, run);
    }

    // Every comment answered or declined exactly once.
    for comment in on_b.iter().chain(&on_h) {
        let number = comment
            .url
            .split("/pull/")
            .nth(1)
            .and_then(|rest| rest.split('#').next())
            .unwrap();
        let reply = the_reply_to(&host, number.parse().unwrap(), comment);
        assert!(reply.body.contains("/commit/"), "{}", reply.body);
        assert!(!reply.body.contains("label=declined"));
    }
    let declined = the_reply_to(&host, n1, &left);
    assert!(declined.body.contains("label=declined"));
    assert_eq!(
        drafter_tasks(&world).len(),
        3,
        "a decline and a drafting per rework"
    );

    // Every remaining draft a draft, on the root or its nearest unlanded ancestors, showing
    // only its own change.
    let repo = service(&world);
    let heads = [
        ("n2", "n2"),
        ("n3-rework-1", "n3"),
        ("n4", "n4"),
        ("n5", "n5"),
        ("n6", "n6"),
        ("n7-rework-1", "n7"),
        ("n8", "n8"),
        ("n9", "n9"),
        ("n10", "n10"),
    ];
    for (node, own) in heads {
        let settlement = settled(&world, run, node);
        let branch = settlement["branch"].as_str().unwrap().to_owned();
        let change = change_from(&world, &branch);
        assert!(
            held_as_draft(&world, &change["number"]),
            "{node} is no longer a draft"
        );
        let base = change["base"].as_str().unwrap().to_owned();
        let landed_parent = settled(&world, run, "n1")["branch"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_ne!(base, landed_parent, "{node} is still on the landed draft");
        if ["n2", "n7-rework-1"].contains(&node) {
            assert_eq!(base, "main", "{node} is not on the root");
        }
        assert_eq!(
            changed_against(&world, &repo, &base, &branch),
            [format!("{own}.md")],
            "{node} shows more than its own change"
        );
    }

    // A board task is written only where the write changes what its item shows: no update
    // the round made is one the store answered as changing nothing.
    let projections = projection_lines(&world, run);
    let unchanged: Vec<&Value> = projections[round_from..]
        .iter()
        .filter(|record| record["actions"]["unchanged"].as_u64().unwrap_or_default() > 0)
        .collect();
    assert!(
        unchanged.is_empty(),
        "writes that changed nothing: {unchanged:?}"
    );
    // Each reworked node's item moves as it did before writes that changed nothing were
    // dropped: the Queued claim reopening it with the rework's task and record, its start,
    // its dispatch once, and its settlement — and ends there.
    let tasks = world.store_tasks(&project);
    for (node, from, to) in reworked {
        let wrote: Vec<(&Value, &Value)> = projections[from..to]
            .iter()
            .filter(|record| {
                record["items"].as_array().unwrap().contains(&json!(node))
                    && record["calls"]["task-update"].as_u64().is_some()
            })
            .map(|record| (&record["updated_fields"], &record["actions"]["reopened"]))
            .collect();
        assert_eq!(
            wrote,
            [
                (
                    &json!({"content": 1, "status": 1, "metadata": 1}),
                    &json!(1)
                ),
                (&json!({"status": 1}), &json!(0)),
                (&json!({"metadata": 1}), &json!(0)),
                (&json!({"status": 1, "metadata": 1}), &json!(0)),
            ],
            "{node}'s writes"
        );
        let item = &tasks
            .iter()
            .find(|task| task["item"]["metadata"]["onepipeline.id"] == node)
            .unwrap_or_else(|| panic!("{node} has an item: {tasks:?}"))["item"];
        assert_eq!(item["status"]["name"], "done", "{item}");
        let metadata = item["metadata"].as_object().unwrap();
        for key in [
            "onepipeline.rework",
            "onepipeline.settlement",
            "onepipeline.change_url",
        ] {
            assert!(
                metadata.contains_key(key),
                "{node} carries no {key}: {item}"
            );
        }
        assert!(
            !metadata.contains_key("onepipeline.dispatch"),
            "{node} still carries its dispatch: {item}"
        );
    }

    // The telemetry the budget's script reads, and what it reports over it.
    let record = json!({
        "schema_version": 1,
        "workload": {"drafts": STACK.len(), "earlier_comments_per_draft": EARLIER,
                     "new_comments": 6, "passes": 2, "reworks": 2, "declines": 1,
                     "landed": 1, "replies": 6},
        "phases": phases,
    });
    let directory: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/budget-records");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        Path::new(env!("CARGO_MANIFEST_DIR")).join(RECORD),
        serde_json::to_string_pretty(&record).unwrap(),
    )
    .unwrap();
    let mut expected = 0u64;
    for phase in record["phases"].as_array().unwrap() {
        for call in phase["ledger"]
            .as_array()
            .unwrap()
            .iter()
            .chain(phase["gh"].as_array().unwrap())
        {
            expected +=
                call["graphql_points"].as_u64().unwrap() + call["rest_requests"].as_u64().unwrap();
        }
        expected += 2 * phase["store_writes"].as_array().unwrap().len() as u64;
        expected += phase["store_reads"].as_array().unwrap().len() as u64;
    }
    // One command asks the host a repository's visibility once, however many envelopes,
    // edits and store writes its boundary judges: the decline's one ask is the engine's, and
    // `onevcs`'s own guard on the reply it posts reads the answer that ask recorded.
    let decline = &record["phases"][1];
    assert_eq!(decline["phase"], "decline");
    let probes = decline["gh"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|call| call["endpoint"] == "REST repository")
        .count();
    assert_eq!(
        probes, 1,
        "the decline asked the repository's visibility {probes} times"
    );
    // Every restack and readjustment merges its moved base itself, and says so at its
    // open, so no open asks whether the base requires an up-to-date branch.
    let rule_reads: Vec<&Value> = record["phases"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|phase| phase["gh"].as_array().unwrap().iter())
        .filter(|call| call["endpoint"] == "REST rules")
        .collect();
    assert!(
        rule_reads.is_empty(),
        "base branch-rule reads: {rule_reads:?}"
    );
    let replies: usize = record["phases"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|phase| phase["ledger"].as_array().unwrap().iter())
        .filter(|call| call["method"] == "reply_to_comment")
        .count();
    assert_eq!(replies, 6, "each reply one POST on the double's ledger");
    let result_file = world.root.join("budget-result.json");
    let reported = budget_report(Path::new(env!("CARGO_MANIFEST_DIR")), &result_file);
    assert!(
        reported.status.success(),
        "{}",
        String::from_utf8_lossy(&reported.stderr)
    );
    let result: Value = serde_json::from_slice(&std::fs::read(&result_file).unwrap()).unwrap();
    assert_eq!(result["value"], json!(expected), "{result}");
    let detail = result["detail"].as_str().unwrap();
    for part in [
        "gh token",
        "board token",
        "pass-1",
        "pass-2",
        "replies",
        "store writes",
        "store reads",
        "reply_to_comment",
        "dispatches by role",
        "typical round",
    ] {
        assert!(detail.contains(part), "the detail lacks {part:?}: {detail}");
    }
}

/// The most `git` processes the round's commands may start, from the launch through
/// the second pass.
///
/// The figure that sets how long this journey takes where starting a process is slow:
/// on the Windows runner a `git` start costs some twenty times what it does on Linux,
/// and this round ran 420–540 seconds there against its 540-second bound. Counted on
/// Linux it started 18,300–19,500 of them, nearly half the attach stream reading every
/// unlanded draft's landing afresh on each of its renders, about forty `git` a read. Now
/// it starts about 10,000, and 12,000–13,500 with every start slowed by sixty
/// milliseconds of CPU — a slower host renders and asks on a timer more often over a
/// longer run — so the bound sits above the second and below anything the round cost
/// before.
const GIT_STARTS: usize = 15_000;

/// Hold the round to [`GIT_STARTS`].
///
/// Floored as well as bounded: a count that saw nothing is a `counted-git` the round's
/// commands never reached, and an empty log is under any bound.
#[cfg(unix)]
fn the_round_started_few_enough_gits(world: &World) {
    let started = world.git_starts().len();
    assert!(started > 0, "the round's git starts were never counted");
    assert!(
        started <= GIT_STARTS,
        "the round started {started} git processes, over its {GIT_STARTS}"
    );
}

/// `counted-git` run as a command would run `git`: with both of its settings, from a
/// directory, and with whatever arguments the caller has.
#[cfg(unix)]
fn counted_git(world: &World, real: Option<&Path>, log: Option<&Path>) -> std::process::Command {
    let mut command = std::process::Command::new(crate::harness::double("counted-git"));
    command
        .current_dir(&world.root)
        .env_remove(onepipeline_testfakes::COUNTED_GIT_REAL_ENV)
        .env_remove(onepipeline_testfakes::COUNTED_GIT_LOG_ENV);
    if let Some(real) = real {
        command.env(onepipeline_testfakes::COUNTED_GIT_REAL_ENV, real);
    }
    if let Some(log) = log {
        command.env(onepipeline_testfakes::COUNTED_GIT_LOG_ENV, log);
    }
    command
}

#[cfg(unix)]
#[test]
fn the_counted_git_counts_each_start_and_is_the_real_git_for_it() {
    let world = World::new("counted-git");
    let real =
        World::resolved_on(&std::process::Command::new("git"), "git").expect("the host has a git");
    let log = world.root.join("starts.jsonl");

    // Counted, and then the real program, which answers and fails as itself.
    let init = counted_git(&world, Some(&real), Some(&log))
        .args(["init", "--quiet", "here"])
        .output()
        .expect("the counted git runs");
    assert!(init.status.success(), "{init:?}");
    assert!(
        world.root.join("here/.git").is_dir(),
        "the real git made no repository"
    );
    let refused = counted_git(&world, Some(&real), Some(&log))
        .args(["-C", "nowhere", "status"])
        .output()
        .expect("the counted git runs");
    assert_eq!(refused.status.code(), Some(128), "{refused:?}");
    let starts: Vec<Value> = std::fs::read_to_string(&log)
        .expect("the starts were counted")
        .lines()
        .map(|line| serde_json::from_str(line).expect("each start is one JSON line"))
        .collect();
    assert_eq!(
        starts,
        [
            json!({"args": ["init", "--quiet", "here"]}),
            json!({"args": ["-C", "nowhere", "status"]}),
        ]
    );

    // Refused without a git to run or a file to count into, which counts nothing; and
    // refused when the git it names will not run, which counts the start it tried.
    for (real, log, says) in [
        (
            None,
            Some(log.as_path()),
            onepipeline_testfakes::COUNTED_GIT_REAL_ENV,
        ),
        (
            Some(real.as_path()),
            None,
            onepipeline_testfakes::COUNTED_GIT_LOG_ENV,
        ),
        (
            Some(Path::new("/nonexistent/git")),
            Some(log.as_path()),
            "cannot run the real git",
        ),
    ] {
        let out = counted_git(&world, real, log)
            .arg("--version")
            .output()
            .expect("the counted git runs");
        assert!(!out.status.success(), "{out:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(says), "the refusal names {says}: {stderr}");
    }
    assert_eq!(
        std::fs::read_to_string(&log).unwrap().lines().count(),
        3,
        "a refused start was counted, or the one whose git would not run was not"
    );

    // And a start it cannot count is not run: a gap in the count would be a cost the
    // bound never saw.
    let uncounted = counted_git(&world, Some(&real), Some(&world.root))
        .args(["init", "--quiet", "uncounted"])
        .output()
        .expect("the counted git runs");
    assert!(!uncounted.status.success(), "{uncounted:?}");
    assert!(
        String::from_utf8_lossy(&uncounted.stderr).contains("cannot"),
        "the refusal says it could not count: {uncounted:?}"
    );
    assert!(
        !world.root.join("uncounted").exists(),
        "a start nothing counted ran the real git"
    );
}

// llmlint: ignore-block[tests_mirror_real_usage] whether a render read a landing again or
// carried the read it held prints the same line on the terminal whenever both answers
// agree, so it is counted by the record the binary writes of its own renders — the seam
// `tests/e2e/landing.rs` measures every render's reads by, for the reason recorded there.
// The command is the `adopt` a user types, over the real round; the record is read after it
// exits.
/// Every draft a rework moved had its landing read again in the render that printed the
/// move, whatever read of it the attach stream that render was part of already held: its
/// `node-restacked`, or a push, a merge of its base or a change request opened from its
/// session — the one move of an already-settled node a run makes on its own, recorded
/// once by the engine and once by `onevcs` as the session does it.
///
/// What [`crate::landing::stream_renders`] splits the stream into. A session's line names
/// its stream rather than its node, so each is read back to its node through the run's
/// own merged store. Where the move lands inside the window the stream carries a read for
/// — every one of them, on a host as quick as this suite's Linux runners — it is the move
/// alone that asks again, and a session's records arrive renders ahead of the engine's,
/// so some render is moved by those alone.
fn moved_drafts_were_read_again(world: &World, record: &Path, stderr: &str, run: &str) {
    let renders = crate::landing::stream_renders(world, record, stderr, run);
    let store = std::fs::read_to_string(world.run_file(run, "events.jsonl"))
        .expect("the run's merged store");
    let node_of: std::collections::BTreeMap<String, String> = store
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| event["source"] == "vcs")
        .filter_map(|event| {
            Some((
                format!("vcs:{}", event["stream"].as_str()?),
                event["labels"]["node"].as_str()?.to_owned(),
            ))
        })
        .collect();
    let moves: Vec<String> = [
        onevcs::EventKind::Push,
        onevcs::EventKind::BaseMerge,
        onevcs::EventKind::ChangeOpened,
    ]
    .into_iter()
    .map(|kind| {
        serde_json::to_value(kind)
            .ok()
            .and_then(|kind| kind.as_str().map(str::to_owned))
            .expect("an onevcs kind is its wire word")
    })
    .collect();
    let last = renders.len() - 1;
    let (mut restacked, mut by_its_session) = (0, 0);
    for (at, render) in renders.iter().enumerate().take(last) {
        for (node, _) in STACK {
            let by_engine = render.printed_of(node, "node-restacked");
            let by_session = render.printed.iter().any(|line| {
                let mut words = line.split_whitespace().skip(1);
                let (Some(id), Some(kind)) = (words.next(), words.next()) else {
                    return false;
                };
                node_of.get(id).map(String::as_str) == Some(node) && moves.iter().any(|m| m == kind)
            });
            // Only of a draft the render reports on: a reworked one is counted under its
            // replacement, and its own old records are not a move of anything reported.
            if !(by_engine || by_session) || !render.reported(node) {
                continue;
            }
            restacked += usize::from(by_engine);
            by_its_session += usize::from(by_session && !render.printed_of(node, ""));
            assert!(
                render.read_at().contains_key(node),
                "render {at} printed {node} moving and did not read its landing again:\n{stderr}"
            );
        }
    }
    assert!(
        restacked > 0,
        "the rework's stream printed no restack before its last render:\n{stderr}"
    );
    assert!(
        by_its_session > 0,
        "no render printed a draft's session moving it without the engine's own record \
         beside it, so nothing here shows a session's record ending a carried read:\n{stderr}"
    );
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// A whole round's record, as the journey writes one, with `replies` reply posts on its
/// last pass's ledger and every other figure the smallest a phase can carry.
fn whole_round(replies: usize) -> Value {
    let phase = |name: &str| {
        json!({"phase": name, "wall_ms": 1, "ledger": [], "gh": [], "store_reads": [],
               "store_writes": [], "dispatches": []})
    };
    let mut phases: Vec<Value> = [
        "pass-1", "decline", "rework-b", "rework-h", "readjust", "pass-2",
    ]
    .into_iter()
    .map(phase)
    .collect();
    phases[0]["ledger"] = json!([{"method": "review_standings", "changes": 10,
                                  "graphql_points": 6, "rest_requests": 0}]);
    phases[0]["gh"] = json!([{"endpoint": "gh pr view", "graphql_points": 1, "rest_requests": 0}]);
    phases[0]["store_reads"] = json!(["get_project"]);
    phases[0]["store_writes"] = json!(["task-update"]);
    phases[0]["pass"] = json!({"readjust_ms": 1, "discovery_ms": 0, "replies_ms": 0});
    phases[5]["ledger"] = json!((0..replies)
        .map(|_| json!({"method": "reply_to_comment", "changes": 1,
                        "graphql_points": 0, "rest_requests": 1}))
        .collect::<Vec<_>>());
    phases[5]["dispatches"] = json!(["review-reply"]);
    json!({
        "schema_version": 1,
        "workload": {"drafts": 10, "earlier_comments_per_draft": 30, "new_comments": 6,
                     "passes": 2, "reworks": 2, "declines": 1, "landed": 1, "replies": 6},
        "phases": phases,
    })
}

/// The reporter where the first `bash` the child's search meets is not a shell but a
/// stand-in for the WSL launcher in the Windows directory — writing its refusal to stdout and
/// exiting 1, as it does on a Windows runner — spelled in the two cases Windows spells that
/// directory. It still refuses a missing record by naming it, and reports a whole round.
///
/// Unix only: on Windows the real launcher is the condition, and
/// `the_round_quota_reporter_refuses_missing_and_malformed_records` meets it there.
#[cfg(unix)]
#[test]
fn the_round_quota_reporter_runs_its_shell_and_not_the_windows_wsl_launcher() {
    let world = World::new("review-round-reporter-wsl");
    let system32 = world.root.join("WINDOWS").join("system32");
    std::fs::create_dir_all(&system32).unwrap();
    onepipeline_testfakes::executable(
        &system32.join("bash"),
        "#!/bin/sh\necho 'Windows Subsystem for Linux has no installed distributions.'\nexit 1\n",
    );
    let path = std::env::join_paths(std::iter::once(system32).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();
    let system_root = world.root.join("Windows").into_os_string();
    let result = world.root.join("result.json");
    let record = world.root.join(RECORD);
    std::fs::create_dir_all(record.parent().unwrap()).unwrap();

    let missing = reporter(&world.root, &path, Some(&system_root))
        .env("ONEBUDGETSPEC_RESULT", &result)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&missing.stdout);
    let stderr = String::from_utf8_lossy(&missing.stderr);
    assert!(!missing.status.success(), "a missing record was accepted");
    assert!(
        !stdout.contains("Windows Subsystem for Linux"),
        "the launcher answered for the reporter: {stdout}"
    );
    assert!(
        stderr.contains(&format!("{RECORD}: missing")) && stderr.contains("review_round::"),
        "stderr: {stderr}\nstdout: {stdout}"
    );
    assert!(!result.exists(), "a failed measurement reported a value");

    std::fs::write(&record, whole_round(6).to_string()).unwrap();
    let whole = reporter(&world.root, &path, Some(&system_root))
        .env("ONEBUDGETSPEC_RESULT", &result)
        .output()
        .unwrap();
    assert!(
        whole.status.success(),
        "stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&whole.stderr),
        String::from_utf8_lossy(&whole.stdout)
    );
    let reported: Value = serde_json::from_slice(&std::fs::read(&result).unwrap()).unwrap();
    assert_eq!(reported["value"], json!(6 + 1 + 6 + 2 + 1));
}

#[test]
fn the_round_quota_reporter_refuses_missing_and_malformed_records() {
    let world = World::new("review-round-reporter");
    let result = world.root.join("result.json");
    let record = world.root.join(RECORD);
    std::fs::create_dir_all(record.parent().unwrap()).unwrap();
    let mut short_of_a_draft = whole_round(6);
    short_of_a_draft["workload"]["drafts"] = json!(9);
    let mut more_than_the_round = whole_round(6);
    more_than_the_round["workload"]["extra"] = json!(1);
    let mut a_phase_missing = whole_round(6);
    a_phase_missing["phases"].as_array_mut().unwrap().remove(1);
    let mut out_of_order = whole_round(6);
    out_of_order["phases"].as_array_mut().unwrap().swap(0, 5);
    let mut negative = whole_round(6);
    negative["phases"][0]["ledger"][0]["graphql_points"] = json!(-1);
    let refused = [
        None,
        Some("{".to_owned()),
        Some("{}".to_owned()),
        Some(r#"{"schema_version":2,"workload":{},"phases":[]}"#.to_owned()),
        Some(r#"{"schema_version":1,"workload":{},"phases":[]}"#.to_owned()),
        Some(
            r#"{"schema_version":1,"workload":{"drafts":10},"phases":[{"phase":"pass-1"}]}"#
                .to_owned(),
        ),
        Some(short_of_a_draft.to_string()),
        Some(more_than_the_round.to_string()),
        Some(a_phase_missing.to_string()),
        Some(out_of_order.to_string()),
        Some(whole_round(5).to_string()),
        Some(negative.to_string()),
    ];
    for body in refused {
        if let Some(body) = &body {
            std::fs::write(&record, body).unwrap();
        }
        let output = budget_report(&world.root, &result);
        assert!(!output.status.success(), "{body:?} was accepted");
        assert!(!result.exists(), "a failed measurement reported a value");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(RECORD) && stderr.contains("review_round::"),
            "stderr: {stderr}\nstdout: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
    std::fs::write(&record, whole_round(6).to_string()).unwrap();
    // A result that cannot be written is a failure, and names what to fix.
    let unwritable = budget_report(&world.root, &world.root);
    assert!(!unwritable.status.success());
    assert!(
        String::from_utf8_lossy(&unwritable.stderr)
            .contains("ensure ONEBUDGETSPEC_RESULT names a writable file"),
        "{}",
        String::from_utf8_lossy(&unwritable.stderr)
    );
    let output = budget_report(&world.root, &result);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reported: Value = serde_json::from_slice(&std::fs::read(&result).unwrap()).unwrap();
    assert_eq!(reported["value"], json!(6 + 1 + 6 + 2 + 1));
    let missing_env = reporter(
        &world.root,
        &std::env::var_os("PATH").unwrap_or_default(),
        std::env::var_os("SystemRoot").as_deref(),
    )
    .env_remove("ONEBUDGETSPEC_RESULT")
    .output()
    .unwrap();
    assert!(!missing_env.status.success());
    assert!(String::from_utf8_lossy(&missing_env.stderr).contains("ONEBUDGETSPEC_RESULT"));
}
