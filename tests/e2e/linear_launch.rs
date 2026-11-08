//! A plan homed in Linear, launched under the rendered-only check: its tasks are rendered from
//! `plan-task` by the released `onetaskgraph`, copied into Linear by the store the binary links,
//! and launched from there with `--require-rendered true`.
//!
//! Linear is `tests/e2e/linear_loopback.rs`, which stores every description and project and
//! document content as spike `spike-linear-stored-content` measured real Linear storing Markdown
//! — `- ` bullets come back `* `, a code span wrapped across a line is joined, bold spanning a
//! line break is split, a lazy ordered line is escaped, and more no store predicts — so what the
//! launch reads is not the bytes the template rendered. The check holds because the copy records
//! the digest of what each write's own answer said Linear stored; and it is no weaker for that,
//! because an edit made in Linear after the copy is still refused.
//!
//! The same plan store copies a Linear-homed plan there again after every review, finish and
//! approval, and an unchanged plan copied again costs Linear no write.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its subprocess
// boundary and nothing inside the crate under test, which is driven as a real compiled binary.
// Linear is the loopback endpoint beside this file — real HTTP on a real socket, reached by the
// real Linear plugin — because an offline check may not reach the hosted API. `harness.rs`
// carries the same suppression and the full rationale.

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::harness::{
    global, lifecycle, onetaskgraph_binary, plan_of, project_id, World, STORE_SOURCE,
};
use crate::linear_loopback::{content_of, metadata_of, Fault, Linear, Operation};
use crate::linear_writeback::{
    configuration, from_the_launch_directory, KEY_ENV, SETTLEMENT, SOURCE, STATES, STATUSES,
};
use onepipeline::templates::{BUILT_IN, RULE_BODY_CHANGED};

/// The plan's name, which is its project's title in both sources and the name its run takes.
const NAME: &str = "linear-launch";

/// The two nodes, each with the criteria it is rendered with. The second one's criteria hold
/// Markdown Linear rewrites beyond the bullet, each a rewrite no store predicts: a code span
/// wrapped across a line, bold spanning a line break, and a line that opens like an ordered item
/// running on from its criterion.
const NODES: &[(&str, &[&str])] = &[
    ("design", &["The design is written.", "It names its risks."]),
    (
        "build",
        &[
            "It builds.",
            "The `onepipeline start` and `onepipeline\nstatus` verbs run.",
            "**Both platforms build\nclean** on CI.",
            "It wraps onto\n3. a lazy line.",
        ],
    ),
];

/// What Linear stored of the `build` node's criteria, as the loopback stores them: the code span
/// joined, the bold split into one run a line, and the lazy line's `.` escaped.
const BUILD_STORED: &[&str] = &[
    "* The `onepipeline start` and `onepipeline status` verbs run.",
    "* **Both platforms build**\n  **clean** on CI.",
    "* It wraps onto\n  3\\. a lazy line.",
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
    homed_by(name, |world, _, launch, plan| {
        copied(
            world,
            launch,
            plan,
            onetaskgraph_core::CopyScope::Projects { tasks: true },
        )
    })
}

/// [`homed`], with the plan's project and tasks copied into Linear by `copy`, which is handed
/// the world, the loopback, the launch directory and the plan's id in the world's own source,
/// and answers where the plan landed.
fn homed_by(name: &str, copy: impl FnOnce(&World, &Linear, &Path, &str) -> String) -> Homed {
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
    let home = copy(
        &world,
        &linear,
        &launch,
        &format!("{STORE_SOURCE}:{}", project_id(NAME)),
    );
    assert_eq!(home, format!("{SOURCE}:{}", linear.project_id()));
    // Each task filed under the plan's project, which the copy created in a Linear that held
    // none.
    // The create that filed each issue Linear holds is its last under that issue's id: one a
    // refused earlier attempt sent under the same id filed nothing.
    let log = linear.log();
    for (held, _, _) in linear.issues() {
        let filed = log
            .iter()
            .rev()
            .find(|request| {
                request.operation == Operation::IssueCreate
                    && request.variables["input"]["id"] == held
            })
            .unwrap_or_else(|| panic!("no create filed {held}: {log:#?}"));
        assert_eq!(
            filed.variables["input"]["projectId"],
            json!(linear.project_id()),
            "{held} was not filed under the plan's project: {filed:?}"
        );
    }
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
        content_of(&held) == DESIGN_STORED,
        "the design document is not held as Linear stores it:\n{held}"
    );
    assert!(
        held.contains(&format!("{DESIGN_STORED}\n<!-- onetaskgraph.metadata `")),
        "the slot after the design's closing table is not set off by one line break:\n{held}"
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

/// The plan's design document: every construct spike `spike-linear-stored-content` measured
/// Linear rewriting that no store predicts — a list after a heading and after a paragraph line, a
/// lazy ordered line, a code span and bold wrapped across a line, an indented code block — ending
/// in a table whose delimiter row Linear rewrites.
const DESIGN: &str = "## Design\n- Build on the store.\n\nA paragraph line.\n- a decision \
     that wraps onto\n3. a lazy line\n\nThe `code span that\nwraps` and **bold that\nspans** \
     stay.\n\n    an indented code line\n\n| part | owner |\n| --- | --- |\n| core | build |\n";

/// [`DESIGN`] as the loopback Linear stores it, and so as the copy reads it back.
const DESIGN_STORED: &str = "## Design\n\n* Build on the store.\n\nA paragraph line.\n\n* a \
     decision that wraps onto\n  3\\. a lazy line\n\nThe `code span that wraps` and **bold \
     that**\n**spans** stay.\n\n```\nan indented code line\n```\n\n| part | owner |\n| -- | -- \
     |\n| core | build |\n";

/// A design document's budget table, written with reference-style links to the tasks of its plan,
/// lands at a Linear source declared private with each cell an inline link to that task's copy and
/// its budget heading. Linear keeps a reference-style link only as escaped text and rewrites what a
/// copy writes, and neither stops the copy. Two tasks also name each other by a path alone in a
/// table cell, so one of them is finished by a follow-up write as a bare address Linear autolinks,
/// which the copy takes for what it is rather than refusing. An unchanged plan copied again writes
/// nothing.
// llmlint: ignore[expensive_tests_stay_behind_their_own_edge] this journey composes the linked store's copy with the Linear plugin over a real socket and reads what the endpoint stored, which only the e2e binary's loopback offers, as for the Linear journeys above it.
#[test]
fn a_budget_table_of_reference_links_lands_on_each_tasks_heading_at_a_private_linear_source() {
    let world = World::new("linear-private-budget-table").with_env(KEY_ENV, "a-loopback-key");
    let linear = Linear::serve_empty(STATES, STATUSES);
    linear.store_links();
    let launch = world.root.join("launch-directory");
    std::fs::create_dir_all(&launch).expect("a launch directory");
    std::fs::write(
        launch.join("onetaskgraph.yaml"),
        configuration(&linear).replacen(
            "    plugin: linear",
            "    visibility: private\n    plugin: linear",
            1,
        ),
    )
    .expect("the launch directory's store configuration");

    let budgeted = |id: &str, budget: &str, other: &str, other_budget: &str| {
        json!({
            "id": id,
            "task": format!(
                "## What\nShip {id}, within [the other budget][other].\n\n\
                 ## Budget {budget}\nOne minute.\n\n\
                 ## Acceptance criteria\n- {id} is published.\n\n\
                 | Next |\n| --- |\n| {other}.md#budget-{other_budget} |\n\n\
                 [other]: {other}.md#budget-{other_budget}\n"
            ),
        })
    };
    let name = "linear-budgeted";
    world.plan(
        name,
        &plan_of(
            name,
            vec![
                budgeted("core", "core-time", "001-later", "later-time"),
                budgeted("later", "later-time", "000-core", "core-time"),
            ],
        ),
    );
    let project = project_id(name);
    for task in ["000-core", "001-later"] {
        let file = world
            .store()
            .join("tasks")
            .join(&project)
            .join(format!("{task}.md"));
        assert!(file.is_file(), "the table below names no task at {file:?}");
    }
    world.write_item(
        &world.store().join("documents").join("design.md"),
        &[("title", json!("Design")), ("project", json!(project))],
        &format!(
            "## Budgets\n\n| Budget | Limit |\n| --- | --- |\n| [core time][1] | 1m |\n\
             | [later time][2] | 2m |\n\n\
             [1]: ../tasks/{project}/000-core.md#budget-core-time\n\
             [2]: ../tasks/{project}/001-later.md#budget-later-time\n"
        ),
    );

    let copy = || {
        let home = copied(
            &world,
            &launch,
            &format!("{STORE_SOURCE}:{project}"),
            onetaskgraph_core::CopyScope::Projects { tasks: true },
        );
        let document = copied(
            &world,
            &launch,
            &format!("{STORE_SOURCE}:design"),
            onetaskgraph_core::CopyScope::Documents,
        );
        (home, document)
    };
    let landed = copy();

    let issue_of = |node: &str| {
        linear
            .issues()
            .into_iter()
            .find(|(_, _, description)| metadata_of(description)["onepipeline.id"] == node)
            .unwrap_or_else(|| panic!("Linear holds no copy of {node}: {:#?}", linear.log()))
    };
    let link = |node: &str, text: &str, budget: &str, slug: &str| {
        let (id, _, description) = issue_of(node);
        assert!(
            description
                .lines()
                .any(|line| line == format!("## Budget {budget}")),
            "{node}'s copy holds no budget heading: {description}"
        );
        let heading = onetaskgraph_linear::headings::decoded(&linear.issue_editor_state(&id))
            .expect("the issue's editor state decodes")
            .into_iter()
            .find(|heading| heading.text == format!("Budget {budget}"))
            .expect("the editor state names the budget heading");
        let id8: String = heading.id.chars().filter(|c| *c != '-').take(8).collect();
        let address = linear.issue_address(&id);
        format!("[{text}](<{address}#heading-{slug}-{id8}>)")
    };
    for (node, other, budget) in [
        ("core", "later", "later-time"),
        ("later", "core", "core-time"),
    ] {
        let wanted = link(
            other,
            "the other budget",
            budget,
            &format!("budget-{budget}"),
        );
        let (other_id, _, _) = issue_of(other);
        let address = linear.issue_address(&other_id);
        let fragment = wanted
            .rsplit_once('#')
            .and_then(|(_, fragment)| fragment.strip_suffix(">)"))
            .expect("a link names its heading");
        let identifier = address.rsplit('/').next().expect("an issue's identifier");
        let autolinked = format!("| [{identifier}]({address}#{fragment}) |");
        let (_, _, description) = issue_of(node);
        let held = content_of(&description);
        assert!(
            held.contains(&format!("within {wanted}.")) && held.contains(&autolinked),
            "{node}'s links to {other} did not land on its budget heading, wanted {wanted} and \
             {autolinked}: {description}"
        );
    }
    // What makes the store's own follow-up worth holding: it sent one of those addresses bare, and
    // what Linear stored of it is not what it sent, which the copy did not refuse.
    let finished_bare = linear.log().iter().any(|request| {
        request.operation == Operation::IssueUpdate
            && request.variables["input"]["description"]
                .as_str()
                .is_some_and(|sent| sent.contains("| https://linear.app/generic/issue/"))
    });
    assert!(
        finished_bare,
        "no follow-up finished a bare address: {:#?}",
        linear.log()
    );
    let wanted = format!(
        "## Budgets\n\n| Budget | Limit |\n| -- | -- |\n| {} | 1m |\n| {} | 2m |\n",
        link("core", "core time", "core-time", "budget-core-time"),
        link("later", "later time", "later-time", "budget-later-time"),
    );
    let held = linear
        .documents()
        .into_iter()
        .find(|(id, _)| landed.1 == format!("{SOURCE}:{id}"))
        .map(|(_, content)| content)
        .unwrap_or_else(|| panic!("Linear holds no {}", landed.1));
    assert_eq!(content_of(&held), wanted, "as Linear stores it:\n{held}");
    let first = linear.log().len();
    assert_eq!(copy(), landed);
    let written: Vec<_> = linear.log()[first..]
        .iter()
        .filter(|request| request.operation.writes())
        .cloned()
        .collect();
    assert!(
        written.is_empty(),
        "an unchanged plan copied again wrote to Linear: {written:#?}"
    );
}

/// A plan copied to a Linear source declared private, whose filtered and searched reads lag every
/// write as Linear's were observed to on `TES2`, and at once copied again: the repeat finds each
/// record the first copy wrote and creates none, and the design document copied right after its
/// plan resolves its link to each of the plan's tasks. A store that finds its counterparts only
/// through those reads creates the plan a second time, and leaves the document's links unresolved.
// llmlint: ignore[expensive_tests_stay_behind_their_own_edge] this journey composes the linked store's copy with the Linear plugin over a real socket and reads what the endpoint stored and was sent, which only the e2e binary's loopback offers, as for the Linear journeys above it.
#[test]
fn a_plan_copied_again_while_linears_listings_lag_creates_nothing_and_its_document_links_resolve() {
    let world = World::new("linear-lagging-listings").with_env(KEY_ENV, "a-loopback-key");
    let linear = Linear::serve_empty(STATES, STATUSES);
    linear.lag_listings();
    let launch = world.root.join("launch-directory");
    std::fs::create_dir_all(&launch).expect("a launch directory");
    std::fs::write(
        launch.join("onetaskgraph.yaml"),
        configuration(&linear).replacen(
            "    plugin: linear",
            "    visibility: private\n    plugin: linear",
            1,
        ),
    )
    .expect("the launch directory's store configuration");

    let name = "linear-lagging";
    world.plan(
        name,
        &plan_of(
            name,
            vec![
                json!({"id": "core", "task": "## What\nShip core.\n"}),
                json!({"id": "later", "task": "## What\nShip later.\n"}),
            ],
        ),
    );
    let project = project_id(name);
    world.write_item(
        &world.store().join("documents").join("design.md"),
        &[("title", json!("Design")), ("project", json!(project))],
        &format!(
            "## Tasks\n\nFirst [core](../tasks/{project}/000-core.md), then \
             [later](../tasks/{project}/001-later.md).\n"
        ),
    );

    let copy = || {
        let home = copied(
            &world,
            &launch,
            &format!("{STORE_SOURCE}:{project}"),
            onetaskgraph_core::CopyScope::Projects { tasks: true },
        );
        let document = copied(
            &world,
            &launch,
            &format!("{STORE_SOURCE}:design"),
            onetaskgraph_core::CopyScope::Documents,
        );
        (home, document)
    };
    let landed = copy();
    let issues = linear.issues();
    assert_eq!(issues.len(), 2, "one issue a task: {issues:#?}");
    let address = |node: &str| {
        let (id, _, _) = issues
            .iter()
            .find(|(_, _, description)| metadata_of(description)["onepipeline.id"] == node)
            .unwrap_or_else(|| panic!("Linear holds no copy of {node}: {issues:#?}"));
        linear.issue_address(id)
    };
    let held = linear
        .documents()
        .into_iter()
        .find(|(id, _)| landed.1 == format!("{SOURCE}:{id}"))
        .map(|(_, content)| content)
        .unwrap_or_else(|| panic!("Linear holds no {}", landed.1));
    assert_eq!(
        content_of(&held),
        format!(
            "## Tasks\n\nFirst [core]({}), then [later]({}).",
            address("core"),
            address("later")
        ),
        "the document's links resolve to each task's copy, as Linear stores it:\n{held}"
    );

    let first = linear.log().len();
    assert_eq!(copy(), landed, "the repeat lands on what the first copy wrote");
    let created: Vec<_> = linear.log()[first..]
        .iter()
        .filter(|request| {
            matches!(
                request.operation,
                Operation::ProjectCreate | Operation::IssueCreate | Operation::DocumentCreate
            )
        })
        .cloned()
        .collect();
    assert!(
        created.is_empty(),
        "the repeat created a record the first copy wrote: {created:#?}"
    );
    assert_eq!(linear.issues().len(), 2, "{:#?}", linear.issues());
    assert_eq!(linear.documents().len(), 1, "{:#?}", linear.documents());
}

/// Copy `item` into Linear with the linked store's copy over `scope`, and answer where it landed.
fn copied(world: &World, launch: &Path, item: &str, scope: onetaskgraph_core::CopyScope) -> String {
    let report = copy_report(world, launch, item, scope)
        .unwrap_or_else(|error| panic!("the copy files it in Linear: {error}"));
    landed(&report)
}

/// Where the one item a copy reports landed.
fn landed(report: &onetaskgraph_core::CopyReport) -> String {
    report
        .items
        .first()
        .and_then(|item| item.action.destination())
        .unwrap_or_else(|| panic!("the copy reports where it landed: {report:?}"))
        .to_string()
}

/// Copy `item` into Linear with the linked store's copy over `scope`, and answer its report, or
/// the store's refusal as it renders it.
fn copy_report(
    world: &World,
    launch: &Path,
    item: &str,
    scope: onetaskgraph_core::CopyScope,
) -> Result<onetaskgraph_core::CopyReport, String> {
    let item = global(item);
    linked_store(world, launch, |engine| async move {
        engine
            .copy(&onetaskgraph_core::CopyRequest {
                items: onetaskgraph_core::CopyItems::new(vec![item]).expect("one item"),
                scope,
                destination: onetaskgraph_plugin_api::SourceName::new(SOURCE)
                    .expect("a source name"),
                status_policy: onetaskgraph_core::StatusPolicy::Source,
                target_task: None,
                match_by: None,
                recreate: false,
                create: false,
                dry_run: false,
                comments: false,
                no_link: false,
                expect: None,
                statuses: Vec::new(),
                conditions: onetaskgraph_core::CopyConditions::default(),
            })
            .await
            .map_err(|error| error.to_string())
    })
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
        let home = format!("{SOURCE}:{}", self.linear.project_id());
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

/// Launch `homed` from its Linear home under the rendered-only check, with nothing refused, and
/// see the run read every node of it and run each to its end.
fn launched(homed: &Homed) {
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
}

/// A plan of `plan-task` renderings, copied into a Linear that rewrites Markdown on write,
/// launches from there under `--require-rendered true`: every node is dispatched, and every
/// settlement is written back to its issue. An edit made to one issue in Linear afterwards is
/// refused by the next launch, naming that task.
#[test]
fn a_rendered_plan_copied_into_linear_launches_from_there_under_the_rendered_only_check() {
    let homed = homed("linear-launch");
    assert_eq!(
        (
            homed.linear.creates(Operation::ProjectCreate),
            homed.linear.creates(Operation::IssueCreate)
        ),
        (1, NODES.len()),
        "the copy into an empty Linear sent other than one project create and one issue \
         create a node: {:#?}",
        homed.linear.log()
    );

    // What Linear holds is not what was rendered: every criterion's bullet came back `* `, the
    // second node's wrapped constructs came back joined, split and escaped, and each rendering's
    // provenance records the digest of what Linear stored rather than of what was rendered.
    for (node, criteria) in NODES {
        let held = homed.linear.description_of(homed.issue_of(node));
        assert!(!held.contains("\n- "), "{node} kept a `- ` bullet:\n{held}");
        assert!(
            held.contains(&format!("* {}", criteria[0])),
            "{node}'s first criterion is not held as a `* ` bullet:\n{held}"
        );
        let provenance = &metadata_of(&held)["onetaskgraph.template"];
        assert_eq!(
            provenance["body_digest"],
            json!(onetaskgraph_plugin_api::body_digest(content_of(&held))),
            "{node}'s rendering does not record the digest of what Linear stored:\n{held}"
        );
    }
    let build = homed.linear.description_of(homed.issue_of("build"));
    for stored in BUILD_STORED {
        assert!(
            build.contains(stored),
            "build does not hold {stored:?} as Linear stores it:\n{build}"
        );
    }
    let before_launch = homed.linear.log().len();

    launched(&homed);

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
    // A write-back's own answer is what Linear stores, so it reads its item only to merge into,
    // before writing it: once the launch has written an issue, every read of it is followed
    // straight away by a write of it, and the last request it is sent is a write — no read of it
    // follows a write-back's metadata write.
    let written_back = &homed.linear.log()[before_launch..];
    for (node, _) in NODES {
        let issue = homed.issue_of(node);
        let sent: Vec<Operation> = written_back
            .iter()
            .filter(|request| request.variables["id"] == issue)
            .map(|request| request.operation)
            .collect();
        let first_write = sent
            .iter()
            .position(|operation| *operation == Operation::IssueUpdate)
            .unwrap_or_else(|| panic!("{node}'s settlement never wrote {issue}: {sent:?}"));
        let reads_then_write = sent[first_write..].windows(2).all(|pair| {
            !matches!(pair[0], Operation::Issue | Operation::IssueResolved)
                || pair[1] == Operation::IssueUpdate
        });
        assert!(
            reads_then_write && sent.last() == Some(&Operation::IssueUpdate),
            "{node}'s write-back read {issue} after writing its metadata: {sent:?}"
        );
    }

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

/// Linear may apply a create whose answer never arrives. A plan copied into an empty Linear whose
/// project create Linear applied and whose answer was lost, and then copied again, leaves Linear
/// holding that one project with each task filed under it once — never a second project, and
/// never a task twice — and the launch from there reads every node.
#[test]
fn a_plan_whose_project_create_lost_its_answer_is_copied_again_into_one_project() {
    let homed = homed_by("linear-launch-retried", |world, linear, launch, plan| {
        let scope = || onetaskgraph_core::CopyScope::Projects { tasks: true };
        // The connection closes with no answer after the loopback applied the create.
        linear.fail_next(Operation::ProjectCreate, Fault::Drop);
        // The copy that lost the answer reads the id it sent the create under, finds the
        // project Linear applied there, and finishes it with the tasks under it.
        let first = copied(world, launch, plan, scope());
        let log = linear.log();
        let create = log
            .iter()
            .position(|request| request.operation == Operation::ProjectCreate)
            .unwrap_or_else(|| panic!("the copy sent no project create: {log:#?}"));
        let sent = log[create].variables["input"]["id"].clone();
        assert!(
            log[create + 1..].iter().any(|request| {
                request.operation == Operation::Project && request.variables["id"] == sent
            }),
            "the copy never read the project its unanswered create was sent as: {log:#?}"
        );
        assert_eq!(linear.issues().len(), NODES.len(), "{log:#?}");
        // Copied again, it lands where the first did and files nothing more.
        let again = copied(world, launch, plan, scope());
        assert_eq!(first, again, "the two copies landed in different places");
        again
    });
    assert_eq!(
        (
            homed.linear.creates(Operation::ProjectCreate),
            homed.linear.creates(Operation::IssueCreate)
        ),
        (1, NODES.len()),
        "the project or a task was created again after the answer was lost: {:#?}",
        homed.linear.log()
    );
    launched(&homed);
}

/// A copy Linear refuses partway is undone, and its undo takes back the project it created — but
/// Linear lets an id name one record ever, so the id that project held, which the copy's create
/// key derives and every later copy of the plan sends again, is one Linear refuses a create
/// under (`conflict on insert of Project`). Copied again into a Linear that holds no project,
/// the plan lands in one new project under another id its key derives, with each task filed
/// under it once, and the launch from there reads every node.
#[test]
fn a_plan_copied_again_after_its_undo_took_its_project_back_lands_in_one_new_project() {
    let mut taken_back = String::new();
    let homed = homed_by("linear-launch-undone", |world, linear, launch, plan| {
        let scope = || onetaskgraph_core::CopyScope::Projects { tasks: true };
        // Every id the first task's create could be tried under is refused, so the copy fails
        // after creating its project, and its undo deletes that project.
        for _ in 0..onetaskgraph_linear::CREATE_ID_GENERATIONS {
            linear.fail_next(Operation::IssueCreate, Fault::Refuse);
        }
        let refused = copy_report(world, launch, plan, scope());
        assert!(
            refused.is_err(),
            "a copy whose task creates were refused landed: {refused:?}"
        );
        linear.clear_faults();
        let log = linear.log();
        taken_back = log
            .iter()
            .find(|request| request.operation == Operation::ProjectDelete)
            .and_then(|request| request.variables["id"].as_str())
            .unwrap_or_else(|| panic!("the undo took no project back: {log:#?}"))
            .to_owned();
        assert!(
            linear.issues().is_empty() && linear.creates(Operation::ProjectCreate) == 1,
            "the refused copy left Linear holding something: {log:#?}"
        );
        copied(world, launch, plan, scope())
    });
    let log = homed.linear.log();
    let creates: Vec<&serde_json::Value> = log
        .iter()
        .filter(|request| request.operation == Operation::ProjectCreate)
        .map(|request| &request.variables["input"]["id"])
        .collect();
    // The second copy sends its create first under the deleted project's id, which Linear
    // refuses, and lands under the next id its key derives.
    assert_eq!(
        creates,
        [
            &json!(taken_back),
            &json!(taken_back),
            &json!(homed.linear.project_id())
        ],
        "{log:#?}"
    );
    assert_ne!(homed.linear.project_id(), taken_back);
    launched(&homed);
}

/// A plan homed in Linear is copied there again after every review, finish and approval, and
/// each write spends Linear's quota, notifies the people following the item and churns its
/// dependency links. A plan with a design document, three tasks and a dependency between two of
/// them, copied into a Linear that rewrites its Markdown on write by the store the binary links,
/// lands whole; copied again unchanged, the store reads what Linear holds and sends no write.
#[test]
fn an_unchanged_plan_copied_into_linear_a_second_time_sends_no_write() {
    const PLAN: &str = "linear-recopy";
    let world = World::new(PLAN).with_env(KEY_ENV, "a-loopback-key");
    let linear = Linear::serve_empty(STATES, STATUSES);
    let launch = world.root.join("launch-directory");
    std::fs::create_dir_all(&launch).expect("a launch directory");
    std::fs::write(launch.join("onetaskgraph.yaml"), configuration(&linear))
        .expect("the launch directory's store configuration");
    // Each task names a hosted origin, which rides the store's own `repositories` field: a
    // folder keeps it in front matter and Linear in the item's metadata slot, the difference a
    // store before 0.12.12 took for an edit and rewrote, re-filing the dependency with it.
    let plan = world.plan(
        PLAN,
        &plan_of(
            PLAN,
            [
                ("design", &[][..]),
                ("build", &["design"][..]),
                ("docs", &[][..]),
            ]
            .into_iter()
            .map(|(id, deps)| {
                let mut node = lifecycle(id, deps);
                node["repo"] = json!("github.com/owner/service");
                node
            })
            .collect(),
        ),
    );
    world.write_item(
        &world.store().join("documents").join("design.md"),
        &[
            ("title", json!("Design")),
            ("project", json!(project_id(PLAN))),
        ],
        DESIGN,
    );
    let copy = || {
        let home = copied(
            &world,
            &launch,
            &plan,
            onetaskgraph_core::CopyScope::Projects { tasks: true },
        );
        let document = copied(
            &world,
            &launch,
            &format!("{STORE_SOURCE}:design"),
            onetaskgraph_core::CopyScope::Documents,
        );
        (home, document)
    };

    // The first copy files the plan: its project, a Linear issue a task, the dependency between
    // `build` and `design`, and the design document, each held as Linear rewrites it.
    let (home, document) = copy();
    assert_eq!(home, format!("{SOURCE}:{}", linear.project_id()));
    let node_of = |issue: &str| -> String {
        let (_, _, description) = linear
            .issues()
            .into_iter()
            .find(|(id, _, _)| id == issue)
            .unwrap_or_else(|| panic!("Linear holds no {issue}"));
        metadata_of(&description)["onepipeline.id"]
            .as_str()
            .unwrap_or_else(|| panic!("a copied issue carries its node id: {description}"))
            .to_owned()
    };
    let mut nodes: Vec<String> = linear
        .issues()
        .iter()
        .map(|(id, _, _)| node_of(id))
        .collect();
    nodes.sort();
    assert_eq!(nodes, ["build", "design", "docs"], "{:#?}", linear.log());
    let relations = || -> Vec<(String, String, String)> {
        linear
            .relations()
            .into_iter()
            .map(|(issue, related, kind)| (node_of(&issue), node_of(&related), kind))
            .collect()
    };
    let related = relations();
    assert_eq!(
        related,
        [("build".to_owned(), "design".to_owned(), "blocks".to_owned())],
        "{:#?}",
        linear.log()
    );
    let held = linear.documents();
    assert!(
        held.iter()
            .any(|(id, content)| document == format!("{SOURCE}:{id}")
                && content_of(content) == DESIGN_STORED),
        "the design document is not held as Linear stores it: {held:?}"
    );
    for (issue, _, description) in linear.issues() {
        assert!(
            !description.contains("\n- "),
            "{issue} kept a `- ` bullet Linear rewrites:\n{description}"
        );
    }
    let issues = linear.issues();
    let project = linear.project_content();

    let first = linear.log().len();
    assert_eq!(copy(), (home, document));
    let again = &linear.log()[first..];
    let written: Vec<&crate::linear_loopback::Request> = again
        .iter()
        .filter(|request| request.operation.writes())
        .collect();
    assert!(
        written.is_empty(),
        "the second copy of an unchanged plan sent {} Linear write(s): {written:#?}",
        written.len()
    );
    assert!(!again.is_empty(), "the second copy read nothing of Linear");
    assert_eq!(linear.issues(), issues);
    assert_eq!(relations(), related);
    assert_eq!(linear.documents(), held);
    assert_eq!(linear.project_content(), project);
}
