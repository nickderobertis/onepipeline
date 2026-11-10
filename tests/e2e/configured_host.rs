//! The three generic primitives, adopted by configuration alone.
//!
//! An orchestrator repository with none of this repository's files configures them in a
//! directory of its own: its store configuration names three Linear team sources — none a
//! default, and no repository route — beside a `local-md` `drafts` source and a `local-md`
//! `authoring` source its plans live in; its template root registers its own names; its
//! status vocabulary is its own; and its launch config names a launch check and a plan-edit
//! hook of its own. A plan of its own is launched from `authoring`, and its command node files
//! a follow-up into the Linear team its argv names — the destination named by configuration,
//! never by a default — while the check refuses nothing and the hook judges a live edit.
//!
//! Each Linear team is `tests/e2e/linear_loopback.rs`, served once per team: real HTTP on a
//! real socket, reached by the real Linear plugin of the released `onetaskgraph` the command
//! runs.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its subprocess
// boundary and nothing inside the crate under test, which is driven as a real compiled binary;
// Linear is the loopback endpoint, because an offline check may not reach the hosted API.
// `harness.rs` and `linear_loopback.rs` carry the same suppression and the full rationale.

use std::path::Path;

use serde_json::{json, Value};

use crate::harness::{agent, human, onetaskgraph_binary, plan_of, posix_script, shell_path, World};
use crate::linear_loopback::{Linear, Operation, TEAM};
use crate::linear_writeback::{from_the_launch_directory, KEY_ENV};

/// The team's workflow states, in this host's own words, by Linear's `WorkflowState.type`.
const STATES: &[(&str, &str)] = &[
    ("Inbox", "backlog"),
    ("Shaped", "backlog"),
    ("Ready", "unstarted"),
    ("Claimed", "unstarted"),
    ("Doing", "started"),
    ("Stuck", "started"),
    ("Shipped", "completed"),
    ("Dropped", "canceled"),
];

/// The workspace's project statuses, in this host's own words.
const STATUSES: &[(&str, &str)] = &[
    ("Proposed", "backlog"),
    ("Sketch", "backlog"),
    ("Committed", "planned"),
    ("Taken", "planned"),
    ("Active", "started"),
    ("Held", "started"),
    ("Delivered", "completed"),
    ("Abandoned", "canceled"),
];

/// This host's status vocabulary, as each Linear source's `status_mapping` states it.
const MAPPING: &str = "      status_mapping:
        backlog:     { task: Inbox,   project: Proposed }
        draft:       { task: Shaped,  project: Sketch }
        todo:        { task: Ready,   project: Committed }
        queued:      { task: Claimed, project: Taken }
        in-progress: { task: Doing,   project: Active }
        unknown:     { task: Stuck,   project: Held }
        done:        { task: Shipped, project: Delivered }
        cancelled:   { task: Dropped, project: Abandoned }
";

/// The host's three Linear teams, by the source name its configuration gives each.
const TEAMS: [&str; 3] = ["product", "platform", "research"];

/// The team the plan's command files its follow-up into, by name.
const DESTINATION: &str = "platform";

/// The host's store configuration: three Linear team sources, `drafts` and `authoring` —
/// and no `default_sources`, so every command names the source it means.
fn store_configuration(teams: &[(&str, &Linear)], host: &Path) -> String {
    let mut config = String::from("sources:\n");
    for (name, linear) in teams {
        config.push_str(&format!(
            "  {name}:\n    plugin: linear\n    config:\n      api_key_env: {KEY_ENV}\n      \
             team: {TEAM}\n      endpoint: {}\n{MAPPING}",
            linear.endpoint()
        ));
    }
    for local in ["drafts", "authoring"] {
        let root = host.join(local);
        std::fs::create_dir_all(&root).expect("a local-md root");
        config.push_str(&format!(
            "  {local}:\n    plugin: local-md\n    config:\n      root: {}\n",
            root.display()
        ));
    }
    config
}

#[test]
fn a_host_configures_command_steps_launch_checks_and_the_plan_edit_hook_by_configuration_alone() {
    let world = World::new("configured-host").with_env(KEY_ENV, "a-loopback-key");
    let linears: Vec<Linear> = TEAMS
        .iter()
        .map(|_| Linear::serve_empty(STATES, STATUSES))
        .collect();
    let host = world.root.join("host");
    std::fs::create_dir_all(host.join("templates")).expect("the host's template root");
    let teams: Vec<(&str, &Linear)> = TEAMS.iter().copied().zip(linears.iter()).collect();
    std::fs::write(
        host.join("onetaskgraph.yaml"),
        store_configuration(&teams, &host),
    )
    .expect("the host's store configuration");
    std::fs::write(
        host.join("templates").join("templates.yaml"),
        "onepipeline_templates: 1\ntemplates:\n  decision-record:\n    role: document\n    \
         description: The host's own decision record.\n",
    )
    .expect("the host's template registration");
    std::fs::write(
        host.join("templates").join("decision-record.md.j2"),
        "# {{ title }}\n\n## Decision\n{{ decision }}\n",
    )
    .expect("the host's own template");

    // A launch check of its own, which counts its runs, and a plan-edit hook of its own,
    // which keeps what it was shown and passes it.
    let checked = shell_path(&host.join("checked"));
    let check = posix_script(
        &host,
        "check-plan",
        &format!("cat > /dev/null\necho run >> '{checked}'\nprintf '{{\"refusals\": []}}'"),
    );
    let offered = shell_path(&host.join("offered.json"));
    let judge = posix_script(&host, "judge-edit", &format!("cat > '{offered}'\nexit 0"));
    std::fs::write(
        host.join("launch.json"),
        json!({
            "schema_version": 14,
            "template_root": "./templates",
            "checks": [[check.display().to_string()]],
            "plan_edit_hook": {"command": [judge.display().to_string()], "timeout_seconds": 60},
        })
        .to_string(),
    )
    .expect("the host's launch config");

    std::fs::write(
        host.join("follow-up.md"),
        "## What\nFollow up on what the configured lifecycle found.\n",
    )
    .expect("the follow-up's body");

    // Its own plan, in its own `authoring` source, with personas of its own: an analyst's
    // draft, a reviewer's pass, a command filing the follow-up into the team it names, and a
    // person's sign-off.
    world.script("draft.wait", "hold");
    let mut draft = agent("draft", &[]);
    draft["persona"] = json!("analyst");
    let mut review = agent("review", &["draft"]);
    review["persona"] = json!("staff-reviewer");
    let follow_up = json!({
        "id": "file-follow-up",
        "kind": "command",
        "command": [
            onetaskgraph_binary().display().to_string(),
            "project", "create", DESTINATION,
            "--id", "follow-up-configured", "--title", "Follow up on the configured lifecycle",
            "--body-file", host.join("follow-up.md").display().to_string(),
        ],
        "deps": ["review"],
    });
    let plan = plan_of(
        "configured",
        vec![
            draft,
            review,
            follow_up,
            human("sign-off", &["file-follow-up"]),
        ],
    );
    let written = world.plan_in(&host.join("authoring"), "configured", &plan);
    let project = format!(
        "authoring:{}",
        written.split_once(':').expect("a qualified id").1
    );

    let run = |args: &[&str]| {
        world.run_on(
            from_the_launch_directory(&world, &host, args),
            &args.join(" "),
        )
    };
    run(&[
        "start",
        &project,
        "--detach",
        "--launch-config",
        "launch.json",
    ])
    .exited(0);
    let name = "configured";
    assert_eq!(
        std::fs::read_to_string(host.join("checked"))
            .expect("the launch check ran")
            .lines()
            .count(),
        1
    );

    // A live edit is judged by the host's own hook, shown the task as it will be dispatched.
    world.until("the draft to be dispatched", |world| {
        world
            .events_of(name, "node-dispatched")
            .iter()
            .any(|event| event["labels"]["node"] == "draft")
    });
    let ruling = "Name the decision record this follow-up is filed against.";
    let replied = world.run_with_stdin_on(
        from_the_launch_directory(&world, &host, &["reply", name]),
        &json!({"version": 2, "commands": [{"op": "amend", "id": "review", "text": ruling}]})
            .to_string(),
    );
    replied.exited(0);
    let shown: Value = serde_json::from_slice(
        &std::fs::read(host.join("offered.json")).expect("the hook was offered the edit"),
    )
    .expect("the hook's document is JSON");
    assert_eq!(shown["plan"], json!(project));
    assert_eq!(shown["node"]["persona"], "staff-reviewer");
    assert!(
        shown["task"]["body"]
            .as_str()
            .expect("a body")
            .contains(ruling),
        "{shown}"
    );

    // The command files the follow-up into the team it names, and into no other.
    world.release("draft.go");
    world.until("the run to pause on the sign-off", |world| {
        world
            .events_of(name, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "sign-off")
    });
    let settled: Vec<Value> = world.events_of(name, "node-settled");
    assert!(
        settled
            .iter()
            .any(|event| event["labels"]["node"] == "file-follow-up"
                && event["payload"]["status"] == "done"),
        "{}",
        world.dump()
    );
    for ((team, _), linear) in teams.iter().zip(&linears) {
        assert_eq!(
            linear.creates(Operation::ProjectCreate),
            usize::from(*team == DESTINATION),
            "team {team} was written to"
        );
    }
    run(&["stop", name]);
}
