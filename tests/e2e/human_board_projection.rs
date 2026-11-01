//! A plan-owned human item filed onto the human-task boards through a real launch: a plan held
//! in its own `plans` source raises the item of its `kind: human` node, and the engine projects
//! that item through the linked store into the configured human-task source — never into the
//! plans source its plan lives in — carrying the repositories its plan's tasks name, so the
//! store's human-task routes classify and place it, and a link back to its plan.
//!
//! Four plans, one per placement: a task of a public repository lands on the public board; one
//! of a private repository on the private board; one spanning both is filed privately with no
//! public tracker issue; and one of an organization's repository follows that organization's
//! route into its tracker, Linear.
//!
//! GitHub is `tests/e2e/github_world.py` and Linear is `tests/e2e/linear_loopback.rs`, each
//! reached by the real plugin the engine links; everything else is the compiled binary, the
//! real linked store, and the real `onevcs` answering what each repository's visibility is.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its subprocess
// boundary and nothing inside the crate under test, which is driven as a real compiled binary.
// GitHub and Linear are the loopbacks beside this file — real HTTP on real sockets, reached by
// the real `github-projects` and `linear` plugins the engine links — because an offline check may
// not reach the hosted APIs. `harness.rs` carries the same suppression and the full rationale.

use serde_json::{json, Map, Value};

use crate::harness::World;
use crate::linear_loopback::Linear;
use crate::linear_writeback::{from_the_launch_directory, KEY_ENV, STATES, STATUSES};
use crate::private_github_home::{described, GitHub};

/// The environment variable every board source reads its token from.
const TOKEN_ENV: &str = "ONEPIPELINE_LOOPBACK_BOARD_TOKEN";
/// The host the store's projection authority names, and the one this journey runs as.
const HOST: &str = "projection-host";
/// The board every plan is held on, declared private, and the repository its plans are filed in.
const PLANS_BOARD: u64 = 1;
const PLANS_REPOSITORY: &str = "owner/plans";
/// The public human-task board, the human-task source, and its repository.
const PUBLIC_BOARD: u64 = 2;
const PUBLIC_REPOSITORY: &str = "owner/app";
/// The private human-task board, and its repository.
const PRIVATE_BOARD: u64 = 3;
const PRIVATE_REPOSITORY: &str = "owner/secret";
/// The organization's repository, whose human tasks its route sends to Linear.
const ORG_REPOSITORY: &str = "acme/widget";

/// The four placements: each plan's name, the repositories its human task names, and its
/// human node's id.
const CASES: &[(&str, &[&str])] = &[
    ("public-plan", &[PUBLIC_REPOSITORY]),
    ("private-plan", &[PRIVATE_REPOSITORY]),
    ("mixed-plan", &[PUBLIC_REPOSITORY, PRIVATE_REPOSITORY]),
    ("org-plan", &[ORG_REPOSITORY]),
];

/// The human step of plan `name`, whose title names its plan.
fn title_of(name: &str) -> String {
    format!("Approve the {name}")
}

/// The seeded issues: each plan's project and its one human task, and each human board's own
/// project, under which a planless or a plan-owned human task is filed.
fn seeded_issues() -> Vec<Value> {
    let mut issues = vec![
        json!({"id": "public-home", "repository": PUBLIC_REPOSITORY, "title": "Human work",
               "board": PUBLIC_BOARD, "status": "Todo", "parent": null,
               "body": described("## Plan\n- Work for people.", &json!({}))}),
        json!({"id": "private-home", "repository": PRIVATE_REPOSITORY,
               "title": "Private human work", "board": PRIVATE_BOARD, "status": "Todo",
               "parent": null,
               "body": described("## Plan\n- Private work for people.", &json!({}))}),
    ];
    for (name, repositories) in CASES {
        issues.push(json!({
            "id": name, "repository": PLANS_REPOSITORY, "title": name, "board": PLANS_BOARD,
            "status": "Todo", "parent": null,
            "body": described("## Plan\n- Ask a person.", &json!({
                "onetaskgraph.classification": "private",
                "onepipeline.schema_version": onepipeline::plan::PLAN_SCHEMA_VERSION,
                "onepipeline.concurrency": 1,
                "onepipeline.goal": {"text": "Have a person approve it"},
            })),
        }));
        // The human step names no repository, as a human node may not; the plan's work does,
        // one parked task a repository, so nothing dispatches while the step waits.
        issues.push(json!({
            "id": format!("{name}-approve"), "repository": PLANS_REPOSITORY,
            "title": title_of(name), "board": PLANS_BOARD, "status": "Todo", "parent": name,
            "body": described(
                "Approve it, which only a person can do.",
                &json!({"onepipeline.id": "approve", "onepipeline.kind": "human",
                        "onetaskgraph.classification": "private",
                        "onetaskgraph.repositories": []})),
        }));
        for (at, repository) in repositories.iter().enumerate() {
            issues.push(json!({
                "id": format!("{name}-work-{at}"), "repository": repository,
                "title": format!("chore: work {at}"), "board": PLANS_BOARD, "status": "Todo",
                "parent": name,
                "body": described(
                    &format!("## What\nDo work {at}.\n\n## Why\nSo the plan ships.\n\n\
                              ## Acceptance criteria\n- work {at} is done."),
                    &json!({"onepipeline.id": format!("work-{at}"),
                            "onepipeline.persona": "engineer", "onepipeline.parked": true,
                            "onetaskgraph.classification": "private"})),
            }));
        }
    }
    issues
}

/// One `github-projects` source over board `number` of `github`.
fn board_source(
    github: &GitHub,
    name: &str,
    visibility: &str,
    number: u64,
    repository: &str,
    extra: &str,
) -> String {
    format!(
        "  {name}:\n    plugin: github-projects\n    visibility: {visibility}\n{extra}    config:\n      \
         owner: owner\n      project_number: {number}\n      repository: {repository}\n      \
         token_env: {TOKEN_ENV}\n      endpoint: {}\n      status_mapping:\n        todo: Todo\n        \
         queued: Queued\n        in-progress: In Progress\n        unknown: Needs Attention\n        \
         done: Done\n        cancelled: Canceled\n      priority_mapping:\n        urgent: Urgent\n        \
         high: High\n        medium: Medium\n        low: Low\n",
        github.board(number)
    )
}

/// The store the launch directory configures: the plans board, the public human-task source
/// whose human-task routes send private work to the private board and the organization's work to
/// Linear, and each destination's host-local projection authority.
fn configuration(github: &GitHub, linear: &Linear, intents: &std::path::Path) -> String {
    let authority = |project: &str, directory: &str| {
        format!(
            "    task_projection:\n      host: {HOST}\n      directory: {}\n      project: {project}\n",
            intents.join(directory).display()
        )
    };
    let routes = format!(
        "    routes:\n      - classification: private\n        to: human-private\n        \
         purpose: human-task\n      - repositories: [github.com/acme/*]\n        to: org\n        \
         purpose: human-task\n{}",
        authority("public-home", "public")
    );
    format!(
        "sources:\n{}{}{}  org:\n    plugin: linear\n{}    config:\n      api_key_env: {KEY_ENV}\n      \
         team: {}\n      endpoint: {}\n      status_mapping:\n        backlog:     {{ task: Proposed,        project: Proposal }}\n        \
         draft:       {{ task: Backlog,         project: Idea }}\n        todo:        {{ task: Todo,            project: Planned }}\n        \
         queued:      {{ task: Queued,          project: Accepted }}\n        in-progress: In Progress\n        \
         unknown:     {{ task: Needs Attention, project: Blocked }}\n        done:        {{ task: Done,            project: Completed }}\n        \
         cancelled:   Canceled\n",
        board_source(github, "plans", "private", PLANS_BOARD, PLANS_REPOSITORY, ""),
        board_source(
            github,
            "human-public",
            "public",
            PUBLIC_BOARD,
            PUBLIC_REPOSITORY,
            &routes
        ),
        board_source(
            github,
            "human-private",
            "private",
            PRIVATE_BOARD,
            PRIVATE_REPOSITORY,
            &authority("private-home", "private"),
        ),
        authority(&linear.project_id(), "org"),
        crate::linear_loopback::TEAM,
        linear.endpoint(),
    )
}

/// The issues GitHub holds that no seed put there: what the projections filed, by node id.
fn filed(state: &Value) -> Map<String, Value> {
    let seeded: Vec<String> = seeded_issues()
        .iter()
        .map(|issue| issue["id"].as_str().unwrap().to_owned())
        .collect();
    state["issues"]
        .as_object()
        .expect("the model's issues")
        .iter()
        .filter(|(id, _)| !seeded.contains(id))
        .map(|(id, issue)| (id.clone(), issue.clone()))
        .collect()
}

/// The node ids of the issues board `number` holds an item for.
fn on_board(state: &Value, number: u64) -> Vec<String> {
    state["boards"][number.to_string()]["items"]
        .as_array()
        .expect("the board's items")
        .iter()
        .filter_map(|item| state["items"][item.as_str()?]["content"].as_str())
        .map(str::to_owned)
        .collect()
}

/// The one open item plan `name`'s run raised, read from the host's human-item store.
fn item_of(world: &World, launch: &std::path::Path, name: &str) -> Value {
    let list = from_the_launch_directory(world, launch, &["human-items", "list", "--json"]);
    let listed = world.run_on(list, "human-items list");
    listed.exited(0).err_lacks("its projection into");
    let all: Vec<Value> = serde_json::from_str(&listed.stdout).expect("a JSON listing");
    let mine: Vec<&Value> = all
        .iter()
        .filter(|view| view["item"]["dedup_key"] == format!("run:{name}#approve"))
        .collect();
    assert_eq!(mine.len(), 1, "run {name} raised one item: {all:#?}");
    mine[0]["item"].clone()
}

/// Launch every plan from the launch directory, each raising its human node's item, and project
/// each onto the human-task source its plan's repositories route it to.
#[test]
fn a_plan_owned_human_item_is_filed_on_the_board_its_plan_repositories_route_it_to() {
    let onevcs = crate::harness::onevcs_binary()
        .to_string_lossy()
        .into_owned();
    let world = World::new("human-board-projection")
        .with_env("HOSTNAME", HOST)
        .with_env("ONETASKGRAPH_HOST", HOST)
        .with_env(TOKEN_ENV, "loopback-token")
        .with_env(KEY_ENV, "a-loopback-key")
        .with_env(
            "ONETASKGRAPH_WRITE_POLICY__CHECK_COMMAND",
            &format!("{onevcs},boundary,check,--destination,public,--input,-"),
        )
        .with_env(
            "ONETASKGRAPH_WRITE_POLICY__VISIBILITY_COMMAND",
            &format!("{onevcs},boundary,inspect,--input,-,--json"),
        );
    // What each repository's visibility is, as this host's `onevcs` answers it.
    std::fs::create_dir_all(world.onevcs_home()).expect("the onevcs home");
    std::fs::write(
        world.onevcs_home().join("rules.yml"),
        "version: 4\nrules:\n  - match: {owner: owner, name: app}\n    visibility: public\n  \
         - match: {owner: owner, name: secret}\n    visibility: private\n  \
         - match: {owner: acme}\n    visibility: public\n  \
         - match: {owner: owner}\n    visibility: private\n\
         default:\n  publication: local-direct\n  approvals: none\n",
    )
    .expect("the visibility rules");
    // Every repository a plan's work names is registered on this host, as a launch resolves it.
    world.extra_repository("app");
    world.extra_repository("secret");
    let widget = world.extra_repository("widget").checkout;
    let org_checkout = world.root.join("acme-widget");
    let cloned = std::process::Command::new("git")
        .arg("clone")
        .arg("--quiet")
        .arg(&widget)
        .arg(&org_checkout)
        .status()
        .expect("git runs");
    assert!(cloned.success(), "the organization's checkout is cloned");
    world.register(&org_checkout, Some("https://github.com/acme/widget.git"));
    let host_config = world.root.join("xdg/onepipeline");
    std::fs::create_dir_all(&host_config).expect("the host configuration's directory");
    std::fs::write(
        host_config.join("config.yaml"),
        "human_items:\n  enabled: true\n  task_source: human-public\n",
    )
    .expect("the host's human-item configuration");

    let github = GitHub::serve(
        &world,
        &json!({
            "owner": "owner",
            "repositories": {PLANS_REPOSITORY: "private", PUBLIC_REPOSITORY: "public",
                             PRIVATE_REPOSITORY: "private", ORG_REPOSITORY: "public"},
            "boards": {PLANS_BOARD.to_string(): {"public": false},
                       PUBLIC_BOARD.to_string(): {"public": true, "priority": true},
                       PRIVATE_BOARD.to_string(): {"public": false, "priority": true}},
            "issues": seeded_issues(),
        }),
    );
    let linear = Linear::serve(
        STATES,
        STATUSES,
        "Organization work",
        "Work for acme.",
        "Planned",
    );
    let launch = world.root.join("launch-directory");
    let intents = world.root.join("intents");
    for dir in [&launch, &intents] {
        std::fs::create_dir_all(dir).expect("a directory");
    }
    std::fs::write(
        launch.join("onetaskgraph.yaml"),
        configuration(&github, &linear, &intents),
    )
    .expect("the store configuration");

    for (name, _) in CASES {
        let start = from_the_launch_directory(
            &world,
            &launch,
            &["start", &format!("plans:{name}"), "--attach"],
        );
        let started = world.run_on(start, &format!("start {name}"));
        // A refusal is said by the driver and leaves the item unbound: either fails here.
        started.exited(0).err_lacks("its projection into");
        let item = item_of(&world, &launch, name);
        assert!(
            item["tracker"]["url"]
                .as_str()
                .is_some_and(|url| url.starts_with("http")),
            "the item of {name} is bound to no tracker: {item:#}\nstderr: {}\nunsupported: {}",
            started.stderr,
            github.state()["unsupported"]
        );
    }

    let state = github.state();
    let filed = filed(&state);
    let titled = |name: &str| -> Vec<(&String, &Value)> {
        filed
            .iter()
            .filter(|(_, issue)| issue["title"] == title_of(name))
            .collect()
    };
    // Every filed human task links its own plan, by the address its plan's project answers at.
    let links_its_plan = |name: &str, body: &str| {
        let plan_number = state["issues"][name]["number"]
            .as_u64()
            .unwrap_or_else(|| panic!("plan {name} has a number: {:#}", state["issues"][name]));
        let link =
            format!("[plans:{name}](https://github.com/{PLANS_REPOSITORY}/issues/{plan_number})");
        assert!(
            body.contains(&link),
            "{name}'s task does not link {link}:\n{body}"
        );
    };

    // Public: one issue, in the public repository, on the public board alone.
    let public = titled("public-plan");
    assert_eq!(public.len(), 1, "{filed:#?}");
    let (id, issue) = public[0];
    assert_eq!(issue["repository"], PUBLIC_REPOSITORY, "{issue:#}");
    assert!(
        on_board(&state, PUBLIC_BOARD).contains(id),
        "{:#}",
        state["boards"]
    );
    assert!(
        !on_board(&state, PRIVATE_BOARD).contains(id),
        "{:#}",
        state["boards"]
    );
    assert_eq!(issue["parent"], "public-home", "{issue:#}");
    links_its_plan("public-plan", issue["body"].as_str().unwrap());

    // Private, and private and public together: each filed privately, in the private repository
    // on the private board, with no public tracker issue anywhere.
    for name in ["private-plan", "mixed-plan"] {
        let private = titled(name);
        assert_eq!(private.len(), 1, "{name}: {filed:#?}");
        let (id, issue) = private[0];
        assert_eq!(issue["repository"], PRIVATE_REPOSITORY, "{name}: {issue:#}");
        assert!(
            on_board(&state, PRIVATE_BOARD).contains(id),
            "{name}: {:#}",
            state["boards"]
        );
        assert!(
            !on_board(&state, PUBLIC_BOARD).contains(id),
            "{name}: {:#}",
            state["boards"]
        );
        links_its_plan(name, issue["body"].as_str().unwrap());
    }

    // The organization's: filed in its tracker alone, linking its plan.
    let org = titled("org-plan");
    assert!(
        org.is_empty(),
        "the organization's task reached GitHub: {org:#?}"
    );
    let in_linear: Vec<(String, String, String)> = linear
        .issues()
        .into_iter()
        .filter(|(_, title, _)| *title == title_of("org-plan"))
        .collect();
    assert_eq!(in_linear.len(), 1, "{:#?}", linear.issues());
    links_its_plan("org-plan", &in_linear[0].2);

    // Nothing public carries any private plan's work: the public repository and board hold the
    // public plan's task and the board's own project alone.
    let public_repository: Vec<&String> = filed
        .iter()
        .filter(|(_, issue)| issue["repository"] == PUBLIC_REPOSITORY)
        .map(|(id, _)| id)
        .collect();
    assert_eq!(public_repository, vec![id_of(&public)], "{filed:#?}");
    let mut held = on_board(&state, PUBLIC_BOARD);
    held.sort();
    let mut expected = vec!["public-home".to_owned(), id_of(&public).clone()];
    expected.sort();
    assert_eq!(held, expected, "{:#}", state["boards"]);
    // And nothing was filed into the plans source.
    assert!(
        filed
            .values()
            .all(|issue| issue["repository"] != PLANS_REPOSITORY),
        "a human task was filed among the plans: {filed:#?}"
    );
}

fn id_of<'a>(one: &[(&'a String, &Value)]) -> &'a String {
    one[0].0
}
