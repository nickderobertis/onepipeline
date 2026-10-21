//! A plan homed in Linear, launched under the rendered-only check: its tasks are rendered from
//! `plan-task` by the released `onetaskgraph`, copied into Linear by the store the binary links,
//! and launched from there with `--require-rendered true`.
//!
//! Linear is `tests/e2e/linear_loopback.rs`, which stores every description and project content
//! as onetaskgraph-linear's ruling says Linear stores Markdown — `- ` bullets come back `* `,
//! domains come back linked, brackets and tildes come back escaped — so what the launch reads is
//! not the bytes the template rendered. The check holds because the copy records the digest of
//! what Linear stores; and it is no weaker for that, because an edit made in Linear after the
//! copy is still refused.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its subprocess
// boundary and nothing inside the crate under test, which is driven as a real compiled binary.
// Linear is the loopback endpoint beside this file — real HTTP on a real socket, reached by the
// real Linear plugin — because an offline check may not reach the hosted API. `harness.rs`
// carries the same suppression and the full rationale.

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::harness::{global, onetaskgraph_binary, plan_of, project_id, World, STORE_SOURCE};
use crate::linear_loopback::{metadata_of, stored_form, Linear, PROJECT};
use crate::linear_writeback::{
    configuration, from_the_launch_directory, KEY_ENV, SETTLEMENT, SOURCE, STATES, STATUSES,
};
use onepipeline::templates::{BUILT_IN, RULE_BODY_CHANGED};

/// The plan's name, which is its project's title in both sources and the name its run takes.
const NAME: &str = "linear-launch";

/// The two nodes, each with the criteria it is rendered with. The second one's criteria hold
/// prose Linear rewrites beyond the bullet: a domain it links, a bracket and a tilde it escapes.
const NODES: &[(&str, &[&str])] = &[
    ("design", &["The design is written.", "It names its risks."]),
    (
        "build",
        &["It builds.", "The docs at example.com list [x] ~2 steps."],
    ),
];

/// A world with a plan of `plan-task`-rendered tasks in its own `local-md` source, copied into
/// an empty loopback Linear by the linked store's own copy, and a launch directory configuring
/// that Linear as the store's one source.
struct Homed {
    world: World,
    linear: Linear,
    launch: PathBuf,
    /// Each node's Linear issue, by node id.
    issues: Vec<(String, String)>,
}

/// Render the plan's tasks, copy the plan into Linear, and answer where it landed.
fn homed(name: &str) -> Homed {
    let world = World::new(name).with_env(KEY_ENV, "a-loopback-key");
    let linear = Linear::serve_empty(STATES, STATUSES);
    let launch = world.root.join("launch-directory");
    std::fs::create_dir_all(&launch).expect("a launch directory");
    std::fs::write(launch.join("onetaskgraph.yaml"), configuration(&linear))
        .expect("the launch directory's store configuration");

    // The plan, in the world's own source: a project holding the plan's metadata, and each
    // task created from `plan-task` by the released `onetaskgraph`, which records the
    // provenance the rendered-only check reads.
    let plan = world.plan(NAME, &plan_of(NAME, vec![]));
    let native = plan.split_once(':').expect("a qualified id").1.to_owned();
    let loader = world
        .run_from(&world.project, &["template", "resolve", BUILT_IN, "--json"])
        .exited(0)
        .stdout
        .clone();
    for (node, criteria) in NODES {
        render(&world, &native, node, criteria, &loader);
    }
    for task in world.store_tasks(&plan) {
        let content = task["item"]["content"].as_str().unwrap_or_default();
        assert!(
            content.contains("\n- ")
                && task["item"]["metadata"]["onetaskgraph.template"]["template"]
                    == format!("onepipeline:{BUILT_IN}"),
            "a task was not rendered from {BUILT_IN} with `- ` criteria: {task}"
        );
    }

    // The store's own copy files the plan in Linear: the project, then each task under it,
    // then the plan's design document after them, as an operator's tooling copies one.
    let home = copied(
        &world,
        &launch,
        &format!("{STORE_SOURCE}:{}", project_id(NAME)),
        onetaskgraph_core::CopyScope::Projects { tasks: true },
    );
    assert_eq!(home, format!("{SOURCE}:{PROJECT}"));
    world.write_item(
        &world.store().join("documents").join("design.md"),
        &[
            ("title", json!("Design")),
            ("project", json!(project_id(NAME))),
        ],
        DESIGN,
    );
    let document = copied(
        &world,
        &launch,
        &format!("{STORE_SOURCE}:design"),
        onetaskgraph_core::CopyScope::Documents,
    );
    let held = linear
        .documents()
        .into_iter()
        .find(|(id, _)| document == format!("{SOURCE}:{id}"))
        .map(|(_, content)| content)
        .unwrap_or_else(|| panic!("Linear holds no {document}"));
    assert!(
        held.starts_with(&stored_form(DESIGN)) && !held.contains("\n- "),
        "the design document is not held as Linear stores it:\n{held}"
    );

    let issues = linear
        .issues()
        .into_iter()
        .map(|(id, _, description)| {
            let node = metadata_of(&description)["onepipeline.id"]
                .as_str()
                .unwrap_or_else(|| panic!("a copied issue carries its node id: {description}"))
                .to_owned();
            (node, id)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        issues.len(),
        NODES.len(),
        "the copy filed one issue a node: {issues:?}"
    );
    Homed {
        world,
        linear,
        launch,
        issues,
    }
}

/// The plan's design document: a list, and a table whose delimiter row Linear rewrites.
const DESIGN: &str =
    "## Design\n- Build on example.com.\n\n| part | owner |\n| --- | --- |\n| core | build |\n";

/// Copy `item` into Linear with the linked store's copy over `scope`, and answer where it landed.
fn copied(world: &World, launch: &Path, item: &str, scope: onetaskgraph_core::CopyScope) -> String {
    let item = global(item);
    let report = linked_store(world, launch, |engine| async move {
        engine
            .copy(&onetaskgraph_core::CopyRequest {
                items: onetaskgraph_core::CopyItems::new(vec![item]).expect("one item"),
                scope,
                destination: onetaskgraph_plugin_api::SourceName::new(SOURCE)
                    .expect("a source name"),
                match_by: None,
                recreate: false,
                create: false,
                dry_run: false,
                comments: false,
                no_link: false,
                expect: None,
                statuses: Vec::new(),
            })
            .await
            .unwrap_or_else(|error| panic!("the copy files it in Linear: {error}"))
    });
    report
        .items
        .first()
        .and_then(|item| item.action.destination())
        .unwrap_or_else(|| panic!("the copy reports where it landed: {report:?}"))
        .to_string()
}

/// One task of `native`, created by the released `onetaskgraph` from `plan-task` with `criteria`.
fn render(world: &World, native: &str, node: &str, criteria: &[&str], loader: &str) {
    let answers = world.root.join(format!("{node}.answers.yaml"));
    std::fs::write(
        &answers,
        onepipeline_testfakes::yaml::to_string(&json!({"acceptance_criteria": criteria}))
            .expect("answers serialise"),
    )
    .expect("the answers file");
    let title = format!("feat: {node} it");
    let id = format!("onepipeline.id=\"{node}\"");
    let args = [
        "task",
        "create",
        STORE_SOURCE,
        "--project",
        native,
        "--title",
        &title,
        "--template-loader",
        "-",
        "--answers",
        answers.to_str().expect("a path"),
        "--metadata",
        &id,
        "--metadata",
        "onepipeline.persona=\"engineer\"",
        "--no-interactive",
        "--json",
    ];
    let command = world.cmd_on(&onetaskgraph_binary(), &args);
    world.run_with_stdin_on(command, loader).exited(0);
}

/// One call against the store the binary links, configured as the world's own source and the
/// launch directory's Linear together — the two a copy from one into the other names.
fn linked_store<T, Fut>(
    world: &World,
    launch: &Path,
    call: impl FnOnce(onetaskgraph_core::Engine) -> Fut,
) -> T
where
    Fut: std::future::Future<Output = T>,
{
    let environment = vec![
        (
            "XDG_CONFIG_HOME".to_owned(),
            world.root.join("xdg").to_string_lossy().into_owned(),
        ),
        ("HOME".to_owned(), world.root.to_string_lossy().into_owned()),
        (
            format!(
                "ONETASKGRAPH_SOURCES__{}__PLUGIN",
                STORE_SOURCE.to_uppercase()
            ),
            "local-md".to_owned(),
        ),
        (
            crate::harness::store_root_env(),
            world.store().to_string_lossy().into_owned(),
        ),
        (KEY_ENV.to_owned(), "a-loopback-key".to_owned()),
    ];
    let loaded = onetaskgraph_core::config::load(
        launch,
        &onetaskgraph_core::Environment::from_pairs(environment),
        &onetaskgraph_core::config::Layer::default(),
    )
    .unwrap_or_else(|error| panic!("the copy's store configuration loads: {error}"));
    let engine = onetaskgraph_core::Engine::build(&loaded.config, &loaded.secrets);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime for one store call")
        .block_on(call(engine))
}

impl Homed {
    /// `start` of the Linear home under the rendered-only check, from the launch directory.
    fn start(&self) -> crate::harness::Run {
        let home = format!("{SOURCE}:{PROJECT}");
        let command = from_the_launch_directory(
            &self.world,
            &self.launch,
            &["start", &home, "--detach", "--require-rendered", "true"],
        );
        self.world.run_on(command, "start --require-rendered true")
    }

    fn issue_of(&self, node: &str) -> &str {
        self.issues
            .iter()
            .find(|(id, _)| id == node)
            .map(|(_, issue)| issue.as_str())
            .unwrap_or_else(|| panic!("no issue for {node}: {:?}", self.issues))
    }
}

/// A plan of `plan-task` renderings, copied into a Linear that rewrites Markdown on write,
/// launches from there under `--require-rendered true`: every node is dispatched, and every
/// settlement is written back to its issue. An edit made to one issue in Linear afterwards is
/// refused by the next launch, naming that task.
#[test]
fn a_rendered_plan_copied_into_linear_launches_from_there_under_the_rendered_only_check() {
    let homed = homed("linear-launch");

    // What Linear holds is not what was rendered: every criterion's bullet came back `* `, and
    // the second node's prose came back linked and escaped.
    for (node, criteria) in NODES {
        let held = homed.linear.description_of(homed.issue_of(node));
        for criterion in *criteria {
            let stored = stored_form(&format!("- {criterion}"));
            assert!(
                stored.starts_with("* ") && held.contains(&stored),
                "{node}'s criterion {criterion:?} is not held as Linear stores it:\n{held}"
            );
        }
        assert!(!held.contains("\n- "), "{node} kept a `- ` bullet:\n{held}");
    }
    assert!(homed
        .linear
        .description_of(homed.issue_of("build"))
        .contains("[example.com](<http://example.com>) list \\[x\\] \\~2 steps."));

    // Loaded under the rendered-only check, with nothing refused, and run to its end.
    let started = homed.start();
    started.exited(0);
    for refusal in [
        RULE_BODY_CHANGED,
        "not rendered",
        "acceptance criteria",
        "Acceptance criteria",
    ] {
        assert!(
            !started.stdout.contains(refusal) && !started.stderr.contains(refusal),
            "the launch reported {refusal:?}:\n{}\n{}",
            started.stdout,
            started.stderr
        );
    }
    homed.world.until("the run to settle", |world| {
        world.run_file(NAME, "result.json").is_file()
    });
    let result = homed.world.run_json(NAME, "result.json");
    let dispatched: Vec<String> = homed
        .world
        .events_of(NAME, "node-dispatched")
        .into_iter()
        .filter_map(|event| event["labels"]["node"].as_str().map(ToOwned::to_owned))
        .collect();
    for (node, _) in NODES {
        assert!(
            dispatched.iter().any(|id| id == node),
            "{node} was never dispatched: {dispatched:?}"
        );
        let status = result["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .find(|held| held["id"] == *node)
            .map(|held| held["status"].clone());
        assert_eq!(status, Some(json!("done")), "{result}");
    }

    // Each settlement reaches its issue, as Linear stores it: the issue moved to the state
    // the mapping gives `done`, and its metadata carries the settlement record.
    homed.world.until("both settlements to reach Linear", |_| {
        NODES.iter().all(|(node, _)| {
            let issue = homed.issue_of(node);
            homed.linear.state_of(issue) == "Done"
                && metadata_of(&homed.linear.description_of(issue))
                    .get(SETTLEMENT)
                    .is_some_and(|settlement| settlement["status"] == "done")
        })
    });

    // Edited in Linear after the copy: the next launch is refused, naming that task, because
    // its body is no longer the one its provenance records.
    let edited = homed.issue_of("design");
    homed.linear.edit_description(edited, |held| {
        held.replacen(
            "It names its risks.",
            "It names its risks, as edited in Linear.",
            1,
        )
    });
    let refused = homed.start();
    assert_ne!(
        refused.code, 0,
        "an edited task launched:\n{}",
        refused.stdout
    );
    let said = format!("{}{}", refused.stdout, refused.stderr);
    assert!(
        said.contains(RULE_BODY_CHANGED) && said.contains(&format!("{SOURCE}:{edited}")),
        "the refusal does not name {SOURCE}:{edited} as differing from its rendering:\n{said}"
    );
}
