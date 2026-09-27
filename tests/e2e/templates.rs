//! Task templates, registered and layered here and rendered by `onetaskgraph` (contract C4,
//! C6a, C7 and C8).
//!
//! Every journey drives the compiled binary. The seam journeys pipe this build's
//! `template resolve --json` into the **released** `onetaskgraph` — the one `just bootstrap`
//! installs at the release the engine links — which renders into a real `local-md` store,
//! and then hold what it wrote to `plan check`, `start` and `template check`. Every template
//! name a journey uses beside `plan-task` is registered by that journey's own host root.

// llmlint: ignore-file[e2e_not_mocked] nothing here is substituted: the store is the real
// `local-md` plugin, the renderer is the released `onetaskgraph` binary, and the one board a
// node is placed on is `scripted-source`, the real `local-md` plugin served over the
// store's own subprocess plugin protocol — standing in for a hosted board that keeps no
// answers, which the manager ruled in place of a GitHub Projects loopback this repository
// does not have (`docs/contract-divergences.md` entry 93).

// llmlint: ignore-file[expensive_tests_stay_behind_their_own_edge] measured rather than
// assumed: these journeys take about a minute on the wall under the suite's parallelism,
// most of it the released `onetaskgraph` and a few attached launches. What they exercise is
// `templates`, `taskgraph`, `plancheck`, `driver`, `filter` and `verbs` together, and the
// seam to the store the engine links, which any change under `src/` can move; a project
// edged narrower than the crate would drop them out of `nx affected` for the very changes
// they exist to catch — the ground `branch_template.rs` and `criteria_rule.rs` carry.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::harness::{double, human, onetaskgraph_binary, plan_of, Run, World, REFUSED};
use onepipeline::templates::{
    BASE, BUILT_IN, REQUIRE_RENDERED_ENVIRONMENT, ROOT_ENVIRONMENT, ROOT_KEY, RULE_BODY_CHANGED,
    RULE_CRITERIA_TYPE, RULE_FOREIGN, RULE_NOT_EXTENDED, RULE_NO_PROVENANCE, RULE_TEMPLATE_CHANGED,
};

/// `plan check`'s exit when the loader refused the plan.
const HAS_REFUSALS: i32 = 1;

/// What every C7 refusal names as its remedy, in part.
const REMEDY: &str = "--json | onetaskgraph task render";

/// The two names every host root here registers beside `plan-task`: one of each role.
const REGISTRATION: &str = "onepipeline_templates: 1\n\
templates:\n  \
follow-up:\n    \
role: task\n    \
description: Work a finished node leaves for later.\n  \
design-doc:\n    \
role: document\n    \
description: A design document a run writes before it builds.\n";

/// What a verb printed, read as the one JSON document it is: the `template` verbs and
/// `onetaskgraph` both print theirs pretty, across many lines.
trait Whole {
    fn whole(&self) -> Value;
}

impl Whole for Run {
    fn whole(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|error| {
            panic!(
                "`{}` printed no JSON document ({error}):\n{}\nstderr:\n{}",
                self.args, self.stdout, self.stderr
            )
        })
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("a directory")).expect("a directory");
    std::fs::write(path, text).expect("the file is written");
}

fn text(path: &Path) -> String {
    path.to_str().expect("a path as text").to_owned()
}

/// A host root registering [`REGISTRATION`]'s names, holding no template file yet.
fn host(world: &World) -> PathBuf {
    let root = world.root.join("host");
    write(&root.join("templates.yaml"), REGISTRATION);
    root
}

/// A role-`task` template: it extends the base, states `what` before the criteria, and
/// carries `marker` so a journey can tell which file rendered.
fn task_template(marker: &str) -> String {
    format!(
        "---\nonetaskgraph_template: 1\nvariables:\n  what:\n    description: What the task \
         builds.\n    type: text\n---\n{{% extends \"{BASE}\" %}}\n{{% block before_criteria \
         %}}\n## What\n\n{{{{ what }}}}\n\n<!-- {marker} -->\n\n{{% endblock %}}\n"
    )
}

/// A role-`document` template extending nothing.
fn document_template(marker: &str) -> String {
    format!("# Design\n\n{marker}\n")
}

/// The template a name of `role` is written with here.
fn template_for(name: &str, marker: &str) -> String {
    if name == "design-doc" {
        document_template(marker)
    } else {
        task_template(marker)
    }
}

/// `onepipeline template ...`, run from `dir` with `root` as the host root.
fn verb(world: &World, dir: &Path, root: &Path, args: &[&str]) -> Run {
    let mut all = vec!["template"];
    all.extend_from_slice(args);
    all.extend(["--template-root", root.to_str().expect("a path")]);
    world.run_from(dir, &all)
}

/// The released `onetaskgraph`, wired to this world's store — and `stdin`, where given.
fn otg(world: &World, args: &[&str], stdin: Option<&str>) -> Run {
    otg_with(world, args, stdin, &[])
}

fn otg_with(world: &World, args: &[&str], stdin: Option<&str>, env: &[(String, String)]) -> Run {
    let mut command = world.cmd_on(&onetaskgraph_binary(), args);
    command.envs(env.iter().map(|(key, value)| (key, value)));
    let run = match stdin {
        Some(stdin) => world.run_with_stdin_on(command, stdin),
        None => world.run_on(command, &args.join(" ")),
    };
    assert_eq!(
        run.code,
        0,
        "`onetaskgraph {}` failed\nstdout: {}\nstderr: {}",
        args.join(" "),
        run.stdout,
        run.stderr
    );
    run
}

/// A project of this world's store with no tasks yet: the id `start` takes, and its native
/// half, which `onetaskgraph task create --project` takes.
fn project(world: &World, name: &str) -> (String, String) {
    let id = world.plan(name, &plan_of(name, vec![]));
    let native = id.split_once(':').expect("a qualified id").1.to_owned();
    (id, native)
}

/// Answers for a task template written by [`task_template`].
fn answers(what: &str, criteria: &[&str]) -> String {
    serde_norway::to_string(&json!({"what": what, "acceptance_criteria": criteria}))
        .expect("answers serialise")
}

/// A node `node` of `native`, created by the released `onetaskgraph` from `loader` — this
/// build's `template resolve --json` — with `answers`: its qualified id and its file.
fn created(
    world: &World,
    native: &str,
    node: &str,
    loader: &str,
    answers: &str,
    extra: &[&str],
) -> (String, PathBuf) {
    let file = world.root.join(format!("{native}-{node}.answers.yaml"));
    write(&file, answers);
    let title = format!("feat: {node} of {native}");
    let id_metadata = format!("onepipeline.id=\"{node}\"");
    let mut args = vec![
        "task",
        "create",
        crate::harness::STORE_SOURCE,
        "--project",
        native,
        "--title",
        &title,
        "--template-loader",
        "-",
        "--answers",
        file.to_str().expect("a path"),
        "--metadata",
        &id_metadata,
        "--metadata",
        "onepipeline.persona=\"engineer\"",
        "--no-interactive",
        "--json",
    ];
    args.extend_from_slice(extra);
    let made = otg(world, &args, Some(loader)).whole();
    let item = &made["items"][0];
    (
        item["id"].as_str().expect("an id").to_owned(),
        PathBuf::from(
            item["item"]["location"]["path"]
                .as_str()
                .expect("a local-md item has a path"),
        ),
    )
}

/// Every file of this world's store and what it holds, to hold a verb to having written
/// nothing.
fn store_snapshot(world: &World) -> Vec<(PathBuf, String)> {
    let mut files = Vec::new();
    let mut pending = vec![world.store()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("a store directory") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let held = std::fs::read_to_string(&path).expect("a store file");
                files.push((path, held));
            }
        }
    }
    files.sort();
    files
}

#[test]
fn a_host_registers_names_beside_plan_task_and_each_bad_declaration_is_refused_by_name() {
    let world = World::new("templates-registration");
    let dir = &world.project;

    // No root at all, and a root holding no registration file: `plan-task` alone.
    let listed = world.run_from(dir, &["template", "list", "--json"]);
    listed.exited(0);
    let listed = listed.whole();
    assert_eq!(listed["registration"], Value::Null);
    assert_eq!(
        listed["templates"],
        json!([{
            "name": BUILT_IN, "role": "task",
            "description": "The task onepipeline dispatches, ending in its acceptance criteria.",
            "layer": "built-in", "path": null
        }])
    );
    let empty = world.root.join("empty-root");
    std::fs::create_dir_all(&empty).expect("a root");
    let listed = verb(&world, dir, &empty, &["list", "--json"]).whole();
    assert_eq!(listed["templates"].as_array().map(Vec::len), Some(1));

    // A registration: both of its names beside `plan-task`, with their roles and
    // descriptions, and no layer supplying either yet.
    let root = host(&world);
    let listed = verb(&world, dir, &root, &["list", "--json"]);
    listed.exited(0);
    let listed = listed.whole();
    assert_eq!(
        listed["registration"],
        json!(text(&root.join("templates.yaml")))
    );
    let names: Vec<(&str, &str, &str)> = listed["templates"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|entry| {
            (
                entry["name"].as_str().unwrap(),
                entry["role"].as_str().unwrap(),
                entry["description"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        names,
        [
            (
                BUILT_IN,
                "task",
                "The task onepipeline dispatches, ending in its acceptance criteria."
            ),
            (
                "design-doc",
                "document",
                "A design document a run writes before it builds."
            ),
            (
                "follow-up",
                "task",
                "Work a finished node leaves for later."
            ),
        ]
    );
    assert_eq!(listed["templates"][1]["layer"], Value::Null);
    verb(&world, dir, &root, &["list"])
        .exited(0)
        .out_has("follow-up [task] no layer supplies it: Work a finished node leaves for later.")
        .out_has("plan-task [task] built-in:");
    // The variable names the root where no flag does, and the flag beats it.
    let mut command = world.cmd(&["template", "list", "--json"]);
    command.current_dir(dir).env(ROOT_ENVIRONMENT, text(&root));
    let listed = world.run_on(command, "template list").whole();
    assert_eq!(
        listed["registration"],
        json!(text(&root.join("templates.yaml")))
    );
    let mut command = world.cmd(&[
        "template",
        "list",
        "--json",
        "--template-root",
        &text(&empty),
    ]);
    command.current_dir(dir).env(ROOT_ENVIRONMENT, text(&root));
    let listed = world.run_on(command, "template list").whole();
    assert_eq!(listed["registration"], Value::Null);
    assert_eq!(listed["templates"].as_array().map(Vec::len), Some(1));

    // Each bad declaration, refused naming the file and what is wrong with it — by the
    // verbs, by `plan check` and by `start` alike.
    let plan = world.plan(
        "registered",
        &plan_of("registered", vec![human("approve", &[])]),
    );
    for (registration, named) in [
        (
            "onepipeline_templates: 1\ntemplates:\n  x:\n    role: task\n    description: d\n    \
             colour: red\n",
            "unknown field `colour`",
        ),
        (
            "onepipeline_templates: 1\nextra: 1\ntemplates: {}\n",
            "unknown field `extra`",
        ),
        (
            "onepipeline_templates: 1\ntemplates:\n  x:\n    role: task\n",
            "missing field `description`",
        ),
        (
            "onepipeline_templates: 1\ntemplates:\n  x:\n    role: task\n    description: \"  \"\n",
            "template `x` has a blank `description`",
        ),
        (
            "onepipeline_templates: 1\ntemplates:\n  Bad_Name:\n    role: task\n    description: \
             d\n",
            "`Bad_Name` is not a template name",
        ),
        (
            "onepipeline_templates: 1\ntemplates:\n  plan-task:\n    role: task\n    \
             description: d\n",
            "`plan-task` is pre-registered",
        ),
        (
            "onepipeline_templates: 2\ntemplates: {}\n",
            "`onepipeline_templates: 2` is not a registration version",
        ),
    ] {
        let bad = world.root.join("bad-root");
        write(&bad.join("templates.yaml"), registration);
        let file = text(&bad.join("templates.yaml"));
        verb(&world, dir, &bad, &["list"])
            .exited(REFUSED)
            .err_has(&file)
            .err_has(named);
        world
            .run_from(
                dir,
                &["plan", "check", &plan, "--template-root", &text(&bad)],
            )
            .exited(HAS_REFUSALS)
            .out_has(named);
        world
            .run_from(
                dir,
                &["start", &plan, "--detach", "--template-root", &text(&bad)],
            )
            .exited(REFUSED)
            .err_has(&file)
            .err_has(named);
    }
    assert!(!world.runs.join("registered").exists());
}

#[test]
fn the_host_root_is_the_flag_then_the_variable_then_the_schema_12_key_and_adopt_keeps_it() {
    let world = World::new("templates-root");
    let dir = world.project.clone();
    let [flag, variable, keyed] = ["flag-root", "variable-root", "config-root"].map(|name| {
        let root = dir.join(name);
        std::fs::create_dir_all(&root).expect("a root");
        root
    });
    let config = world.root.join("launch.yaml");
    write(
        &config,
        "schema_version: 12\ntemplate_root: ./config-root\nrequire_rendered: true\n",
    );
    let config = text(&config);
    let plan_named = |name: &str| world.plan(name, &plan_of(name, vec![human("approve", &[])]));

    let launched = |run: &str, extra: &[&str], env: Option<&str>| {
        let plan = plan_named(run);
        let mut args = vec![
            "start",
            plan.as_str(),
            "--attach",
            "--launch-config",
            &config,
        ];
        args.extend_from_slice(extra);
        let mut command = world.cmd(&args);
        command.current_dir(&dir);
        if let Some(env) = env {
            command.env(ROOT_ENVIRONMENT, env);
        }
        world.run_on(command, run).settled();
        world.run_json(run, "launch.json")
    };
    let record = launched(
        "flagged",
        &["--template-root", "flag-root"],
        Some("variable-root"),
    );
    assert_eq!(record[ROOT_KEY], json!(text(&flag)));
    let record = launched("varied", &[], Some("variable-root"));
    assert_eq!(record[ROOT_KEY], json!(text(&variable)));
    let record = launched("configured", &[], None);
    assert_eq!(record[ROOT_KEY], json!(text(&keyed)));
    assert_eq!(record["require_rendered"], json!(true));
    // Named nowhere: the record says nothing about either.
    let plan = plan_named("unnamed");
    world
        .run_from(&dir, &["start", &plan, "--attach"])
        .settled();
    let record = world.run_json("unnamed", "launch.json");
    assert!(record.get(ROOT_KEY).is_none() && record.get("require_rendered").is_none());

    // An adopting process's own environment names another root and turns the check off,
    // and the run keeps what it was launched with.
    world.run(&["attest", "configured", "approve"]).exited(0);
    let mut command = world.cmd(&["adopt", "configured"]);
    command
        .env(ROOT_ENVIRONMENT, text(&variable))
        .env(REQUIRE_RENDERED_ENVIRONMENT, "false");
    world.run_on(command, "adopt configured").exited(0);
    let record = world.run_json("configured", "launch.json");
    assert_eq!(record[ROOT_KEY], json!(text(&keyed)));
    assert_eq!(record["require_rendered"], json!(true));
    assert!(
        world
            .run(&["adopt", "configured", "--template-root", "x"])
            .code
            != 0
    );

    // A root that cannot be read is refused by the rung that named it, before any run.
    let plan = plan_named("unreadable");
    let missing = dir.join("no-such-root");
    world
        .run_from(
            &dir,
            &[
                "start",
                &plan,
                "--detach",
                "--template-root",
                "no-such-root",
            ],
        )
        .exited(REFUSED)
        .err_has(&format!(
            "template root {} (from --template-root) cannot be read",
            missing.display()
        ));
    let mut command = world.cmd(&["plan", "check", &plan]);
    command
        .current_dir(&dir)
        .env(ROOT_ENVIRONMENT, "no-such-root");
    world
        .run_on(command, "plan check")
        .exited(REFUSED)
        .err_has(&format!("(from {ROOT_ENVIRONMENT}) cannot be read"));
    let broken = world.root.join("broken.yaml");
    write(
        &broken,
        "schema_version: 12\ntemplate_root: ./no-such-root\n",
    );
    world
        .run_from(
            &dir,
            &[
                "start",
                &plan,
                "--detach",
                "--launch-config",
                &text(&broken),
            ],
        )
        .exited(REFUSED)
        .err_has(&format!(
            "(from the launch config's `{ROOT_KEY}`) cannot be read"
        ));
    assert!(!world.runs.join("unreadable").exists());

    // Schema 12's keys, refused by name below it and refused blank at it; and every
    // earlier schema still loads — schema 11's key among them.
    for (document, named) in [
        (
            "schema_version: 11\ntemplate_root: ./config-root\n",
            "`template_root` is a schema 12 key",
        ),
        (
            "schema_version: 11\nrequire_rendered: true\n",
            "`require_rendered` is a schema 12 key",
        ),
        (
            "schema_version: 12\ntemplate_root: \"\"\n",
            "`template_root` is present and names nothing",
        ),
        (
            "schema_version: 12\ntemplate_root:\n",
            "`template_root` is present and names nothing",
        ),
        (
            "schema_version: 12\nrequire_rendered:\n",
            "`require_rendered` is present and names nothing",
        ),
    ] {
        write(&world.root.join("refused.yaml"), document);
        world
            .run_from(
                &dir,
                &[
                    "start",
                    &plan,
                    "--detach",
                    "--launch-config",
                    &text(&world.root.join("refused.yaml")),
                ],
            )
            .exited(REFUSED)
            .err_has(named);
    }
    for version in 1..=11 {
        let name = format!("schema{version}");
        let plan = plan_named(&name);
        let document = world.root.join(format!("{name}.yaml"));
        let branch = if version == 11 {
            "branch_template: \"{{ node.id }}\"\n"
        } else {
            ""
        };
        write(&document, &format!("schema_version: {version}\n{branch}"));
        world
            .run_from(
                &dir,
                &[
                    "start",
                    &plan,
                    "--attach",
                    "--launch-config",
                    &text(&document),
                ],
            )
            .settled();
    }
}

#[test]
fn every_name_resolves_at_the_first_layer_that_supplies_it_and_says_which() {
    let world = World::new("templates-layers");
    let root = host(&world);
    let repo = world.root.join("repo");
    std::fs::create_dir_all(&repo).expect("a checkout overriding nothing yet");
    let dir = world.project.clone();

    // `plan-task` with nothing over it is the built-in.
    let resolved = verb(&world, &dir, &root, &["resolve", BUILT_IN, "--json"]).whole();
    assert_eq!(resolved["layer"], "built-in");
    assert_eq!(resolved["path"], Value::Null);
    assert_eq!(resolved["reference"], "onepipeline:plan-task");
    assert_eq!(resolved["entry"], BASE);

    for name in [BUILT_IN, "follow-up", "design-doc"] {
        let file = format!("{name}.md.j2");
        let hosted = root.join(&file);
        let overridden = repo.join(".onepipeline").join("templates").join(&file);
        let explicit = world.root.join(format!("explicit-{file}"));

        // Registered and supplied by no layer: refused naming every path searched.
        if name != BUILT_IN {
            verb(
                &world,
                &dir,
                &root,
                &["resolve", name, "--repo", &text(&repo)],
            )
            .exited(REFUSED)
            .err_has(&format!("no template for {name}"))
            .err_has(&text(&overridden))
            .err_has(&text(&hosted));
        }

        write(&hosted, &template_for(name, "host"));
        let resolved = verb(
            &world,
            &dir,
            &root,
            &["resolve", name, "--repo", &text(&repo), "--json"],
        )
        .whole();
        assert_eq!(resolved["layer"], "host", "{name}");
        assert_eq!(resolved["path"], json!(text(&hosted)), "{name}");

        write(&overridden, &template_for(name, "repository"));
        let resolved = verb(
            &world,
            &dir,
            &root,
            &["resolve", name, "--repo", &text(&repo), "--json"],
        )
        .whole();
        assert_eq!(resolved["layer"], "repository", "{name}");
        assert_eq!(resolved["path"], json!(text(&overridden)), "{name}");
        assert_eq!(
            resolved["search_path"],
            json!([text(overridden.parent().unwrap())])
        );
        // With no `--repo`, the working directory is the checkout.
        let resolved = verb(&world, &repo, &root, &["resolve", name, "--json"]).whole();
        assert_eq!(resolved["layer"], "repository", "{name}");

        write(&explicit, &template_for(name, "explicit"));
        verb(
            &world,
            &dir,
            &root,
            &[
                "resolve",
                name,
                "--repo",
                &text(&repo),
                "--template",
                &text(&explicit),
            ],
        )
        .exited(0)
        .out_has(&format!("explicit layer, {}", explicit.display()));

        // What each layer resolves to is what the released renderer renders.
        for (extra, marker) in [
            (vec!["--repo".to_owned(), text(&repo)], "repository"),
            (vec!["--template".to_owned(), text(&explicit)], "explicit"),
            (vec![], "host"),
        ] {
            let mut args = vec!["resolve", name, "--json"];
            args.extend(extra.iter().map(String::as_str));
            let loader = verb(&world, &dir, &root, &args).stdout;
            let rendered = otg(
                &world,
                &[
                    "template",
                    "render",
                    "--template-loader",
                    "-",
                    "--var",
                    "what=layers",
                    "--var",
                    "acceptance_criteria=[\"it resolves\"]",
                    "--no-interactive",
                    "--json",
                ]
                .into_iter()
                .filter(|arg| name != "design-doc" || !arg.contains('=') && *arg != "--var")
                .collect::<Vec<_>>(),
                Some(&loader),
            )
            .whole();
            assert!(
                rendered["body"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(marker),
                "{name} at the {marker} layer rendered {rendered}"
            );
        }
    }

    // A parent a chain names in a subdirectory resolves over the same search path — the
    // resolved file's own directory — as every other name, and a template reaching the base
    // through it passes C6a and renders through the released renderer.
    let templates = repo.join(".onepipeline").join("templates");
    write(
        &templates.join("parts/parent.md.j2"),
        &format!(
            "{{% extends \"{BASE}\" %}}\n{{% block before_criteria %}}From the parent.\n\n\
             {{% endblock %}}\n"
        ),
    );
    write(
        &templates.join("follow-up.md.j2"),
        "{% extends \"parts/parent.md.j2\" %}\n",
    );
    verb(
        &world,
        &dir,
        &root,
        &["check", "follow-up", "--repo", &text(&repo)],
    )
    .exited(0)
    .out_has("ok (repository layer,");
    let loader = verb(
        &world,
        &dir,
        &root,
        &["resolve", "follow-up", "--repo", &text(&repo), "--json"],
    )
    .stdout;
    let rendered = otg(
        &world,
        &[
            "template",
            "render",
            "--template-loader",
            "-",
            "--var",
            "acceptance_criteria=[\"the parent is there\"]",
            "--no-interactive",
            "--json",
        ],
        Some(&loader),
    )
    .whole();
    assert_eq!(
        rendered["body"],
        "From the parent.\n\n## Acceptance criteria\n\n- the parent is there\n"
    );

    // A repository template extending the base and including a sibling in its own
    // directory: it passes C6a, and renders with the sibling in it.
    write(
        &templates.join("sibling.md.j2"),
        "The sibling says hello.\n",
    );
    write(
        &templates.join("follow-up.md.j2"),
        &format!(
            "{{% extends \"{BASE}\" %}}\n{{% block before_criteria %}}\n{{% include \
             \"sibling.md.j2\" %}}\n\n{{% endblock %}}\n"
        ),
    );
    verb(
        &world,
        &dir,
        &root,
        &["check", "follow-up", "--repo", &text(&repo)],
    )
    .exited(0)
    .out_has("template follow-up [task]: ok (repository layer,");
    let loader = verb(
        &world,
        &dir,
        &root,
        &["resolve", "follow-up", "--repo", &text(&repo), "--json"],
    )
    .stdout;
    let rendered = otg(
        &world,
        &[
            "template",
            "render",
            "--template-loader",
            "-",
            "--var",
            "acceptance_criteria=[\"the sibling is there\"]",
            "--no-interactive",
            "--json",
        ],
        Some(&loader),
    )
    .whole();
    assert_eq!(
        rendered["body"],
        "The sibling says hello.\n\n## Acceptance criteria\n\n- the sibling is there\n"
    );

    // The single `--repository` origin names the checkout `onevcs` resolves it to, and a
    // `--repo` that is not there is refused rather than searched as empty.
    let repository = world.repository("local-direct", &[]);
    let registered = repository
        .checkout
        .join(".onepipeline")
        .join("templates")
        .join("plan-task.md.j2");
    write(&registered, &task_template("the registered checkout's"));
    let resolved = verb(
        &world,
        &dir,
        &root,
        &[
            "resolve",
            BUILT_IN,
            "--repository",
            "github.com/owner/service",
            "--json",
        ],
    )
    .whole();
    assert_eq!(resolved["layer"], "repository");
    assert_eq!(resolved["path"], json!(text(&registered)));
    verb(
        &world,
        &dir,
        &root,
        &["resolve", BUILT_IN, "--repo", "no-such-checkout"],
    )
    .exited(REFUSED)
    .err_has("--repo")
    .err_has("no-such-checkout")
    .err_has("cannot be read as a checkout");
    // Several origins name no single checkout, so the working directory is the one.
    let resolved = verb(
        &world,
        &repo,
        &root,
        &[
            "resolve",
            BUILT_IN,
            "--repository",
            "github.com/owner/service",
            "--repository",
            "github.com/owner/other",
            "--json",
        ],
    )
    .whole();
    assert_eq!(resolved["layer"], "repository");
    assert_eq!(
        resolved["path"],
        json!(text(
            &repo
                .join(".onepipeline")
                .join("templates")
                .join("plan-task.md.j2")
        ))
    );
    // A single origin `onevcs` cannot resolve is refused, never read as the working
    // directory.
    verb(
        &world,
        &repo,
        &root,
        &[
            "resolve",
            BUILT_IN,
            "--repository",
            "github.com/owner/unregistered",
        ],
    )
    .exited(REFUSED)
    .err_has("--repository github.com/owner/unregistered: its checkout could not be resolved");
    // A layer's file that is there and is not a template file is refused where it is,
    // never passed over for the layer under it.
    let odd_root = world.root.join("odd-root");
    std::fs::create_dir_all(odd_root.join("plan-task.md.j2")).expect("a directory by that name");
    verb(&world, &dir, &odd_root, &["resolve", BUILT_IN])
        .exited(REFUSED)
        .err_has(&format!(
            "{} (host layer) is not a template file",
            odd_root.join("plan-task.md.j2").display()
        ));
    verb(&world, &dir, &odd_root, &["list"])
        .exited(REFUSED)
        .err_has("(host layer) is not a template file");
    // An explicit file that is not there is refused, never passed over for the next layer.
    let absent = world.root.join("absent.md.j2");
    verb(
        &world,
        &dir,
        &root,
        &["resolve", BUILT_IN, "--template", &text(&absent)],
    )
    .exited(REFUSED)
    .err_has(&format!(
        "--template {} is not a readable template file",
        absent.display()
    ));

    // An unregistered name, refused naming the registration read.
    verb(&world, &dir, &root, &["resolve", "nope"])
        .exited(REFUSED)
        .err_has("template nope is not registered")
        .err_has(&text(&root.join("templates.yaml")));
    world
        .run_from(&dir, &["template", "resolve", "follow-up"])
        .exited(REFUSED)
        .err_has("template follow-up is not registered (no template root was named");
}

#[test]
fn c6a_holds_a_task_template_to_the_base_and_its_criteria_and_a_rendering_to_listing_one() {
    let world = World::new("templates-c6a");
    let root = host(&world);
    let repo = world.root.join("repo");
    let dir = world.project.clone();

    // Not extending the base, at the host layer and as a repository override of
    // `plan-task`: refused naming the rule and the layer.
    let hosted = root.join("follow-up.md.j2");
    write(
        &hosted,
        "---\nonetaskgraph_template: 1\nvariables:\n  what:\n    description: What.\n    type: \
         text\n---\n## What\n\n{{ what }}\n",
    );
    verb(&world, &dir, &root, &["check", "follow-up"])
        .exited(REFUSED)
        .err_has(&format!(
            "template follow-up (host layer, {}): {RULE_NOT_EXTENDED}",
            hosted.display()
        ));
    let overridden = repo
        .join(".onepipeline")
        .join("templates")
        .join("plan-task.md.j2");
    write(&overridden, "## Acceptance criteria\n\n- fixed\n");
    verb(
        &world,
        &dir,
        &root,
        &["check", BUILT_IN, "--repo", &text(&repo)],
    )
    .exited(REFUSED)
    .err_has(&format!(
        "(repository layer, {}): {RULE_NOT_EXTENDED}",
        overridden.display()
    ));

    // Extending the base and redeclaring its criteria as one string, or as a list of
    // objects: each is not a list of strings.
    for declared in ["type: string", "type: list\n    items: object"] {
        write(
            &hosted,
            &format!(
                "---\nonetaskgraph_template: 1\nvariables:\n  acceptance_criteria:\n    \
                 description: Criteria.\n    {declared}\n---\n{{% extends \"{BASE}\" %}}\n"
            ),
        );
        verb(&world, &dir, &root, &["check", "follow-up"])
            .exited(REFUSED)
            .err_has(&format!(
                "(host layer, {}): {RULE_CRITERIA_TYPE}",
                hosted.display()
            ));
    }
    // An `extends` spelled in the front matter is a YAML value: the template extends nothing.
    write(
        &hosted,
        &format!(
            "---\nonetaskgraph_template: 1\ndescription: \"Written as {{% extends '{BASE}' %}}\"\n\
             ---\n## Acceptance criteria\n\n- fixed\n"
        ),
    );
    verb(&world, &dir, &root, &["check", "follow-up"])
        .exited(REFUSED)
        .err_has(RULE_NOT_EXTENDED);
    // An `extends` inside a comment is text: the template extends nothing.
    write(
        &hosted,
        &format!("{{# {{% extends \"{BASE}\" %}} #}}\n## Acceptance criteria\n\n- fixed\n"),
    );
    verb(&world, &dir, &root, &["check", "follow-up"])
        .exited(REFUSED)
        .err_has(RULE_NOT_EXTENDED);

    // A chain that does not load is refused naming the layer and the file, and a stored
    // item named by anything but a qualified id is refused before any store is asked.
    write(
        &hosted,
        "{% extends \"onepipeline/plan-task.md.j2\" %}{% block unclosed %}",
    );
    verb(&world, &dir, &root, &["check", "follow-up"])
        .exited(REFUSED)
        .err_has(&format!(
            "template follow-up (host layer, {}) does not load",
            hosted.display()
        ));
    verb(
        &world,
        &dir,
        &root,
        &["check", BUILT_IN, "--item", "not-qualified"],
    )
    .exited(REFUSED)
    .err_has("'not-qualified' is not a qualified onetaskgraph id");

    // A document extending nothing passes; so do task templates using their variables any
    // way minijinja allows — nothing is rendered to check them. Both below redeclare the
    // criteria as a `list` naming no `items`, which is a list of strings, the default.
    write(
        &root.join("design-doc.md.j2"),
        &document_template("{{ anything }}"),
    );
    verb(&world, &dir, &root, &["check", "design-doc"]).exited(0);
    for body in [
        "{% block before_criteria %}The third: {{ items[2] }}\n\n{% endblock %}",
        "{% block before_criteria %}{% for item in items | sort %}{% if item | length > 3 \
         %}- {{ item | upper }}\n{% endif %}{% endfor %}{{ items | join(', ') | default('none') \
         }}\n\n{% endblock %}{% block more_criteria %}{% if items %}- {{ items | first }} is \
         done.\n{% endif %}{% endblock %}",
    ] {
        write(
            &hosted,
            &format!(
                "---\nonetaskgraph_template: 1\nvariables:\n  items:\n    description: Things.\n    \
                 type: list\n  acceptance_criteria:\n    description: Criteria.\n    type: list\n    \
                 default: []\n---\n{{% extends \"{BASE}\" %}}\n{body}\n"
            ),
        );
        verb(&world, &dir, &root, &["check", "follow-up"])
            .exited(0)
            .out_has("ok (host layer,");
    }

    // A rendering: refused with C6b's rule when it lists no criterion, for role `task`;
    // any text at all for role `document`.
    let mut command = world.cmd(&[
        "template",
        "check",
        BUILT_IN,
        "--rendering",
        "-",
        "--template-root",
        &text(&root),
    ]);
    command.current_dir(&dir);
    let refused = world.run_with_stdin_on(command, "## What\n\nIt.\n\n## Acceptance criteria\n\n");
    refused
        .exited(REFUSED)
        .err_has("the rendering: no criteria listed");
    let mut command = world.cmd(&[
        "template",
        "check",
        BUILT_IN,
        "--rendering",
        "-",
        "--template-root",
        &text(&root),
    ]);
    command.current_dir(&dir);
    world
        .run_with_stdin_on(command, "## Acceptance criteria\n\n- it is listed\n")
        .exited(0)
        .out_has("rendering: ok");
    let mut command = world.cmd(&[
        "template",
        "check",
        "design-doc",
        "--rendering",
        "-",
        "--template-root",
        &text(&root),
    ]);
    command.current_dir(&dir);
    world
        .run_with_stdin_on(command, "no criteria at all, and that is fine")
        .exited(0);
    let rendering = world.root.join("rendering.md");
    write(
        &rendering,
        "## Acceptance criteria\n\n- it is listed in a file\n",
    );
    verb(
        &world,
        &dir,
        &root,
        &["check", BUILT_IN, "--rendering", &text(&rendering)],
    )
    .exited(0)
    .out_has("rendering: ok");
    let missing = world.root.join("no-such-rendering.md");
    verb(
        &world,
        &dir,
        &root,
        &["check", BUILT_IN, "--rendering", &text(&missing)],
    )
    .exited(REFUSED)
    .err_has(&format!("--rendering {} cannot be read", missing.display()));
}

/// The seam, end to end, for one role-`task` name: rendered by the released `onetaskgraph`
/// from this build's resolved template, accepted, refused once its host template changes,
/// cleared by a re-render, and refused again once an answer renders no criterion.
fn a_rendered_task_holds_until_its_template_changes(world: &World, name: &str) {
    let root = world.root.join("host");
    let dir = world.project.clone();
    let hosted = root.join(format!("{name}.md.j2"));
    write(&hosted, &task_template("first"));
    let (plan, native) = project(world, &format!("seam-{name}"));
    let loader = verb(world, &dir, &root, &["resolve", name, "--json"]).stdout;
    let (id, _) = created(
        world,
        &native,
        "build",
        &loader,
        &answers("Build it.", &["It builds.", "It ships."]),
        &[],
    );
    let before = store_snapshot(world);

    let check = |expect: i32| {
        world
            .run_from(
                &dir,
                &[
                    "plan",
                    "check",
                    &plan,
                    "--require-rendered",
                    "true",
                    "--template-root",
                    &text(&root),
                ],
            )
            .exited(expect)
            .stdout
            .clone()
    };
    check(0);
    verb(world, &dir, &root, &["check", name, "--item", &id])
        .exited(0)
        .out_has(&format!("item {id}: ok"));
    // A well-formed id naming no item is refused naming it, rather than read as empty.
    let missing = format!("{id}-missing");
    let refused = verb(world, &dir, &root, &["check", name, "--item", &missing]);
    assert_ne!(refused.code, 0, "an absent item passed");
    refused.err_has(&missing);
    // No `template` verb wrote anything.
    verb(world, &dir, &root, &["list"]).exited(0);
    verb(world, &dir, &root, &["resolve", name, "--json"]).exited(0);
    assert_eq!(
        store_snapshot(world),
        before,
        "a template verb wrote to the store"
    );

    // The host template is edited: both refuse, naming both digests and where.
    write(&hosted, &task_template("second"));
    let refused = check(HAS_REFUSALS);
    assert!(
        refused.contains(RULE_TEMPLATE_CHANGED) && refused.contains(&text(&hosted)),
        "{refused}"
    );
    assert!(
        refused.contains(REMEDY) && refused.contains("--template-loader -"),
        "{refused}"
    );
    verb(world, &dir, &root, &["check", name, "--item", &id])
        .exited(REFUSED)
        .err_has(RULE_TEMPLATE_CHANGED)
        .err_has("host layer");

    // Re-rendered from what resolves now: accepted again, and the new template rendered.
    let loader = verb(world, &dir, &root, &["resolve", name, "--json"]).stdout;
    otg(
        world,
        &[
            "task",
            "render",
            &id,
            "--template-loader",
            "-",
            "--no-interactive",
        ],
        Some(&loader),
    );
    check(0);
    verb(world, &dir, &root, &["check", name, "--item", &id]).exited(0);
    assert!(world
        .store_tasks(&plan)
        .iter()
        .any(|task| task["item"]["content"]
            .as_str()
            .unwrap_or_default()
            .contains("second")));

    // An answer rendering no criterion: refused on the dry run's own output, and — once
    // written — by the plan's own criteria rule, which holds whether or not C7 is on.
    let empty = world.root.join(format!("{name}-empty.yaml"));
    write(&empty, "acceptance_criteria: []\n");
    let dry = otg(
        world,
        &[
            "task",
            "render",
            &id,
            "--template-loader",
            "-",
            "--answers",
            &text(&empty),
            "--dry-run",
            "--no-interactive",
            "--json",
        ],
        Some(&loader),
    )
    .whole();
    let body = dry["body"].as_str().expect("a dry run shows its body");
    let mut command = world.cmd(&[
        "template",
        "check",
        name,
        "--rendering",
        "-",
        "--template-root",
        &text(&root),
    ]);
    command.current_dir(&dir);
    world
        .run_with_stdin_on(command, body)
        .exited(REFUSED)
        .err_has("no criteria listed");
    otg(
        world,
        &[
            "task",
            "render",
            &id,
            "--template-loader",
            "-",
            "--answers",
            &text(&empty),
            "--no-interactive",
        ],
        Some(&loader),
    );
    let refused = check(HAS_REFUSALS);
    assert!(refused.contains("no criteria listed"), "{refused}");
    verb(world, &dir, &root, &["check", name, "--item", &id])
        .exited(REFUSED)
        .err_has(&format!("item {id}: no criteria listed"));
    world
        .run_from(&dir, &["plan", "check", &plan])
        .exited(HAS_REFUSALS)
        .out_has("no criteria listed");
}

#[test]
fn the_seam_holds_for_plan_task_rendered_by_the_released_onetaskgraph() {
    let world = World::new("templates-seam");
    host(&world);
    a_rendered_task_holds_until_its_template_changes(&world, BUILT_IN);
}

#[test]
fn the_seam_holds_for_a_host_registered_task_name() {
    let world = World::new("templates-seam-host");
    host(&world);
    a_rendered_task_holds_until_its_template_changes(&world, "follow-up");
}

#[test]
fn require_rendered_refuses_each_node_that_is_not_its_rendering_and_off_refuses_none() {
    let world = World::new("templates-c7");
    let root = host(&world);
    let dir = world.project.clone();
    write(&root.join("plan-task.md.j2"), &task_template("host"));
    let loader = verb(&world, &dir, &root, &["resolve", BUILT_IN, "--json"]).stdout;
    let rendered = answers("Build it.", &["It builds."]);

    // One project per case, since the loader answers with its first refusal.
    let (unrendered, _) = {
        let id = world.plan(
            "unrendered",
            &plan_of(
                "unrendered",
                vec![json!({"id": "build", "persona": "engineer",
                    "task": "## What\nBuild it.\n\n## Acceptance criteria\n- It builds."})],
            ),
        );
        (id, ())
    };
    let (foreign, native) = project(&world, "foreign");
    let other = world.root.join("other.md.j2");
    write(
        &other,
        "## What\n\nOther.\n\n## Acceptance criteria\n\n- It builds.\n",
    );
    otg(
        &world,
        &[
            "task",
            "create",
            crate::harness::STORE_SOURCE,
            "--project",
            &native,
            "--title",
            "feat: foreign",
            "--template",
            &text(&other),
            "--metadata",
            "onepipeline.id=\"build\"",
            "--metadata",
            "onepipeline.persona=\"engineer\"",
            "--no-interactive",
        ],
        None,
    );
    let (changed, native) = project(&world, "changed");
    created(&world, &native, "build", &loader, &rendered, &[]);
    let (fine, native) = project(&world, "fine");
    let (fine_id, _) = created(&world, &native, "build", &loader, &rendered, &[]);
    // `changed` and `fine` were rendered before the host template changed; `fine` is
    // re-rendered after it, and `edited` is rendered after it and then edited by hand.
    write(
        &root.join("plan-task.md.j2"),
        &task_template("host, edited"),
    );
    let loader = verb(&world, &dir, &root, &["resolve", BUILT_IN, "--json"]).stdout;
    otg(
        &world,
        &[
            "task",
            "render",
            &fine_id,
            "--template-loader",
            "-",
            "--no-interactive",
        ],
        Some(&loader),
    );
    let (edited, native) = project(&world, "edited");
    let (edited_id, _) = created(&world, &native, "build", &loader, &rendered, &[]);
    let content = world.store_tasks(&edited)[0]["item"]["content"]
        .as_str()
        .expect("a rendered task has content")
        .replacen("- It builds.", "- It builds, edited by hand.", 1);
    let edited_file = world.root.join("edited.md");
    write(&edited_file, &content);
    otg(
        &world,
        &[
            "task",
            "content",
            "set",
            &edited_id,
            "--file",
            &text(&edited_file),
            "--no-interactive",
        ],
        None,
    );

    // A provenance entry a person broke by hand, in the store's own file: no provenance
    // this build can read.
    // llmlint: ignore-block[tests_mirror_real_usage] onetaskgraph refuses every write to its
    // reserved `onetaskgraph.` metadata namespace through each of its verbs, so a broken
    // provenance entry arises only from editing the item's own file — which is how a person
    // edits a `local-md` store, and is the case this refusal exists for.
    let (malformed, native) = project(&world, "malformed");
    world.write_store_item(
        &format!("tasks/{native}/000-build.md"),
        &format!(
            "---\ntitle: \"feat: malformed\"\nproject: \"{native}\"\nmetadata: {}\n---\n\n\
             ## What\n\nBuild it.\n\n## Acceptance criteria\n\n- It builds.\n",
            json!({
                "onepipeline.id": "build",
                "onepipeline.persona": "engineer",
                "onetaskgraph.template": "not a provenance entry",
            })
        ),
    );

    // llmlint: ignore-end[tests_mirror_real_usage]

    let cases = [
        (&unrendered, RULE_NO_PROVENANCE.to_owned(), true),
        (
            &malformed,
            format!("{RULE_NO_PROVENANCE} this build can read"),
            true,
        ),
        (
            &foreign,
            format!("{RULE_FOREIGN}: {}", other.display()),
            true,
        ),
        (&changed, RULE_TEMPLATE_CHANGED.to_owned(), false),
        (&edited, RULE_BODY_CHANGED.to_owned(), false),
    ];
    for (plan, rule, answers) in &cases {
        let refused = world
            .run_from(
                &dir,
                &[
                    "plan",
                    "check",
                    plan,
                    "--require-rendered",
                    "true",
                    "--template-root",
                    &text(&root),
                ],
            )
            .exited(HAS_REFUSALS)
            .stdout
            .clone();
        assert!(
            refused.contains(&format!("node 'build': {rule}")),
            "{plan}: {refused}"
        );
        assert!(
            refused.contains(&format!("onepipeline template resolve {BUILT_IN} {REMEDY}")),
            "{plan}: {refused}"
        );
        assert_eq!(
            refused.contains("--answers FILE"),
            *answers,
            "{plan}: {refused}"
        );
        // `template check --item` refuses the stored item on the same rule, and `--json`
        // carries the refusal as the engine's, about the node's task.
        let id = world.store_tasks(plan)[0]["id"]
            .as_str()
            .expect("an id")
            .to_owned();
        verb(&world, &dir, &root, &["check", BUILT_IN, "--item", &id])
            .exited(REFUSED)
            .err_has(&format!("item {id}: {rule}"))
            .err_has(REMEDY);
        let json = world.run_from(
            &dir,
            &[
                "plan",
                "check",
                plan,
                "--require-rendered",
                "true",
                "--template-root",
                &text(&root),
                "--json",
            ],
        );
        json.exited(HAS_REFUSALS);
        assert_eq!(json.stderr, "", "{plan}");
        let answered = json.whole();
        assert_eq!(answered["accepted"], json!(false), "{answered}");
        let refusal = &answered["refusals"][0];
        assert_eq!(refusal["source"], "engine", "{answered}");
        assert_eq!(refusal["node"], "build", "{answered}");
        assert_eq!(refusal["field"], "task", "{answered}");
        assert!(
            refusal["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains(rule.as_str())),
            "{answered}"
        );
        // Off, the same plan is refused by nothing.
        world
            .run_from(
                &dir,
                &["plan", "check", plan, "--template-root", &text(&root)],
            )
            .exited(0);
    }
    world
        .run_from(
            &dir,
            &[
                "plan",
                "check",
                &fine,
                "--require-rendered",
                "true",
                "--template-root",
                &text(&root),
            ],
        )
        .exited(0);

    // A variable that is neither `true` nor `false` is refused by name, not read as off.
    let mut command = world.cmd(&[
        "plan",
        "check",
        &unrendered,
        "--template-root",
        &text(&root),
    ]);
    command
        .current_dir(&dir)
        .env(REQUIRE_RENDERED_ENVIRONMENT, "yes");
    world
        .run_on(command, "plan check yes")
        .exited(REFUSED)
        .err_has(&format!("{REQUIRE_RENDERED_ENVIRONMENT} is \"yes\""));

    // `start` refuses by each of its three rungs, and mints no run; `false` on a higher
    // rung turns a lower one off.
    let config = world.root.join("strict.yaml");
    write(
        &config,
        &format!(
            "schema_version: 12\nrequire_rendered: true\ntemplate_root: {}\n",
            text(&root)
        ),
    );
    world
        .run_from(
            &dir,
            &[
                "start",
                &unrendered,
                "--detach",
                "--launch-config",
                &text(&config),
            ],
        )
        .exited(REFUSED)
        .err_has(RULE_NO_PROVENANCE);
    let mut command = world.cmd(&[
        "start",
        &edited,
        "--detach",
        "--template-root",
        &text(&root),
    ]);
    command
        .current_dir(&dir)
        .env(REQUIRE_RENDERED_ENVIRONMENT, "true");
    world
        .run_on(command, "start edited")
        .exited(REFUSED)
        .err_has(RULE_BODY_CHANGED)
        .err_has("onetaskgraph task render");
    world
        .run_from(
            &dir,
            &[
                "start",
                &changed,
                "--detach",
                "--require-rendered",
                "true",
                "--template-root",
                &text(&root),
            ],
        )
        .exited(REFUSED)
        .err_has(RULE_TEMPLATE_CHANGED);
    for run in ["unrendered", "edited", "changed"] {
        assert!(
            !world.runs.join(run).exists(),
            "a refused launch minted {run}"
        );
    }
    world
        .run_from(
            &dir,
            &[
                "start",
                &unrendered,
                "--attach",
                "--launch-config",
                &text(&config),
                "--require-rendered",
                "false",
            ],
        )
        .settled();
    world
        .run_from(
            &dir,
            &[
                "start",
                &fine,
                "--attach",
                "--launch-config",
                &text(&config),
            ],
        )
        .settled();
}

#[test]
fn a_direct_node_resolves_against_the_launch_directory() {
    let world = World::new("templates-direct");
    let root = host(&world);
    let launch = world.root.join("launch");
    let elsewhere = world.project.clone();
    write(
        &launch.join(".onepipeline/templates/plan-task.md.j2"),
        &task_template("the launch directory's own"),
    );
    let loader = verb(&world, &launch, &root, &["resolve", BUILT_IN, "--json"]);
    assert_eq!(loader.whole()["layer"], "repository");
    let (plan, native) = project(&world, "direct");
    created(
        &world,
        &native,
        "build",
        &loader.stdout,
        &answers("Build it.", &["It builds."]),
        &[],
    );
    let args = [
        "plan",
        "check",
        plan.as_str(),
        "--require-rendered",
        "true",
        "--template-root",
        root.to_str().expect("a path"),
    ];
    world.run_from(&launch, &args).exited(0);
    // From another directory the same node resolves to the built-in, whose digest is not
    // the one it was rendered with.
    world
        .run_from(&elsewhere, &args)
        .exited(HAS_REFUSALS)
        .out_has(RULE_TEMPLATE_CHANGED)
        .out_has("built-in layer");
    world
        .run_from(
            &launch,
            &[
                "start",
                &plan,
                "--attach",
                "--require-rendered",
                "true",
                "--template-root",
                &text(&root),
            ],
        )
        .settled();
}

#[test]
fn a_lifecycle_node_resolves_against_its_publication_checkout() {
    let world = World::new("templates-lifecycle");
    let root = host(&world);
    let dir = world.project.clone();
    let repository = world.repository("local-direct", &[]);
    let overridden = repository
        .checkout
        .join(".onepipeline/templates/plan-task.md.j2");
    write(&overridden, &task_template("the service's own"));
    let loader = verb(
        &world,
        &dir,
        &root,
        &[
            "resolve",
            BUILT_IN,
            "--repo",
            &text(&repository.checkout),
            "--json",
        ],
    );
    assert_eq!(loader.whole()["layer"], "repository");
    let (plan, native) = project(&world, "lifecycle");
    created(
        &world,
        &native,
        "service",
        &loader.stdout,
        &answers("Ship it.", &["It ships."]),
        &["--metadata", "onepipeline.repo=\"service\""],
    );
    let args = [
        "plan",
        "check",
        plan.as_str(),
        "--require-rendered",
        "true",
        "--template-root",
        root.to_str().expect("a path"),
    ];
    // Checked from a launch directory holding no override: the node's own checkout is
    // where its repository layer is.
    world.run_from(&dir, &args).exited(0);
    // A changed override is named at the path the caller registered the checkout under —
    // on Windows too, where `onevcs` answers it in its extended-length spelling.
    write(&overridden, &task_template("the service's edited"));
    world
        .run_from(&dir, &args)
        .exited(HAS_REFUSALS)
        .out_has(RULE_TEMPLATE_CHANGED)
        .out_has(&format!("repository layer, {}", overridden.display()));
    std::fs::remove_file(&overridden).expect("the override is removed");
    world
        .run_from(&dir, &args)
        .exited(HAS_REFUSALS)
        .out_has(RULE_TEMPLATE_CHANGED)
        .out_has("built-in layer");

    // Provenance is checked first: a lifecycle node carrying none is refused for that,
    // before anything asks `onevcs` about a repository that does not resolve.
    let unresolved = world.plan(
        "unresolved",
        &plan_of(
            "unresolved",
            vec![json!({
                "id": "ship", "repo": "nowhere", "persona": "engineer", "title": "feat: ship it",
                "task": "## What\nShip it.\n\n## Acceptance criteria\n- It ships.",
            })],
        ),
    );
    world
        .run_from(
            &dir,
            &[
                "plan",
                "check",
                &unresolved,
                "--require-rendered",
                "true",
                "--template-root",
                &text(&root),
            ],
        )
        .exited(HAS_REFUSALS)
        .out_has(&format!("node 'ship': {RULE_NO_PROVENANCE}"))
        .out_lacks("could not be resolved");

    // Provenance that passes, on a node whose repository `onevcs` cannot resolve: refused
    // there, naming the repository, rather than searched as a checkout with no override.
    // The service's own override back as it rendered, so its node passes and this one is
    // the refusal.
    write(&overridden, &task_template("the service's own"));
    let built_in = verb(&world, &dir, &root, &["resolve", BUILT_IN, "--json"]).stdout;
    created(
        &world,
        &native,
        "stranded",
        &built_in,
        "acceptance_criteria: [It ships.]\n",
        &["--metadata", "onepipeline.repo=\"nowhere\""],
    );
    world
        .run_from(&dir, &args)
        .exited(HAS_REFUSALS)
        .out_has("node 'stranded'")
        .out_has("the checkout of nowhere could not be resolved");
}

/// The settings that declare a second source, `board`: `scripted-source`, the real
/// `local-md` plugin served over the store's subprocess plugin protocol, which keeps no
/// answers beside an item it is handed.
fn board(world: &World) -> Vec<(String, String)> {
    let prefix = "ONETASKGRAPH_SOURCES__BOARD";
    vec![
        (format!("{prefix}__PLUGIN"), "subprocess".to_owned()),
        (
            format!("{prefix}__CONFIG__COMMAND"),
            text(&double("scripted-source")),
        ),
        (
            format!("{prefix}__CONFIG__SETTINGS__ROOT"),
            text(&world.root.join("board")),
        ),
        (
            format!("{prefix}__CONFIG__SETTINGS__SCRIPT"),
            text(&world.fakes),
        ),
        (
            format!("{prefix}__CONFIG__SETTINGS__KEY"),
            "board".to_owned(),
        ),
    ]
}

/// One Markdown item of `local-md`'s own layout, holding `content` byte for byte: the
/// plugin reads one line break after the front matter and the one that ends the file as its
/// framing, so the content is written between them exactly as it is.
fn board_item(front: &Value, content: &str) -> String {
    let front = serde_norway::to_string(front).expect("front matter serialises");
    format!("---\n{front}---\n{content}\n")
}

#[test]
fn a_rendered_node_on_a_board_that_keeps_no_answers_loads_under_the_check_until_it_is_edited() {
    let world = World::new("templates-board");
    let root = host(&world);
    let dir = world.project.clone();
    write(&root.join("plan-task.md.j2"), &task_template("host"));
    let loader = verb(&world, &dir, &root, &["resolve", BUILT_IN, "--json"]).stdout;
    let (plan, native) = project(&world, "boarded");
    created(
        &world,
        &native,
        "build",
        &loader,
        &answers("Build it.", &["It builds.", "It ships."]),
        &[],
    );
    let [task] = world
        .store_tasks(&plan)
        .try_into()
        .unwrap_or_else(|tasks: Vec<Value>| panic!("the project holds one task: {tasks:?}"));
    let content = task["item"]["content"]
        .as_str()
        .expect("a rendered task has content")
        .to_owned();
    let metadata = task["item"]["metadata"].clone();
    assert!(
        metadata.get("onetaskgraph.template").is_some(),
        "{metadata}"
    );

    // The board holds the item's content and its metadata — provenance included — and no
    // answers: `scripted-source` serves this folder through the store's own subprocess plugin
    // protocol, the way a hosted board that keeps no answers is reached.
    // llmlint: ignore-block[tests_mirror_real_usage] the item is placed on the board by writing
    // the board's own file rather than by `onetaskgraph project copy`, on the manager's ruling
    // (`docs/contract-divergences.md` entry 93): that verb in onetaskgraph 0.2.47 writes a
    // copied item's content trimmed, which is a store defect an onetaskgraph node is fixing,
    // and what this journey proves is C7 over a board that keeps no answers, which needs the
    // item's bytes there exactly as rendered. Every read and the edit below go through the
    // store's own verbs.
    let board_root = world.root.join("board");
    write(
        &board_root.join("projects").join(format!("{native}.md")),
        &board_item(
            &json!({"title": "boarded", "metadata": {"onepipeline.schema_version": 3}}),
            "",
        ),
    );
    let item = board_root.join("tasks").join(format!("{native}-build.md"));
    let front = json!({
        "title": task["item"]["title"],
        "project": native,
        "metadata": metadata,
    });
    write(&item, &board_item(&front, &content));
    let held = std::fs::read_to_string(&item).expect("the board's item");
    assert!(
        !held.contains("template-answers"),
        "the board keeps answers:\n{held}"
    );
    // llmlint: ignore-end[tests_mirror_real_usage]

    let env = board(&world);
    let on_board = format!("board:{native}");
    let check = |expect: i32| {
        let mut command = world.cmd(&[
            "plan",
            "check",
            &on_board,
            "--require-rendered",
            "true",
            "--template-root",
            &text(&root),
        ]);
        command
            .current_dir(&dir)
            .envs(env.iter().map(|(key, value)| (key, value)))
            // The plan reader asks the default sources for a project's tasks, so the
            // board is named as the one to ask.
            .env("ONETASKGRAPH_DEFAULT_SOURCES", "board");
        world
            .run_on(command, "plan check board")
            .exited(expect)
            .stdout
            .clone()
    };
    check(0);

    // One interior line changed on the board, through the store's own content verb: refused
    // as differing from its rendering.
    let edited = world.root.join("edited-on-board.md");
    write(
        &edited,
        &content.replacen("- It builds.", "- It builds, on the board.", 1),
    );
    otg_with(
        &world,
        &[
            "task",
            "content",
            "set",
            &format!("{on_board}-build"),
            "--file",
            &text(&edited),
            "--no-interactive",
        ],
        None,
        &env,
    );
    let refused = check(HAS_REFUSALS);
    assert!(refused.contains(RULE_BODY_CHANGED), "{refused}");
    assert!(
        refused.contains(&format!("{REMEDY} {on_board}-build --template-loader -")),
        "{refused}"
    );
}

#[test]
fn a_stored_document_is_checked_against_its_host_registered_document_template() {
    let world = World::new("templates-document");
    let root = host(&world);
    let dir = world.project.clone();
    let hosted = root.join("design-doc.md.j2");
    write(&hosted, &document_template("first draft"));
    let loader = verb(&world, &dir, &root, &["resolve", "design-doc", "--json"]).stdout;
    let (_, native) = project(&world, "documented");
    let made = otg(
        &world,
        &[
            "document",
            "create",
            crate::harness::STORE_SOURCE,
            "--project",
            &native,
            "--title",
            "The design",
            "--template-loader",
            "-",
            "--no-interactive",
            "--json",
        ],
        Some(&loader),
    )
    .whole();
    let id = made["items"][0]["id"]
        .as_str()
        .expect("a created document has an id")
        .to_owned();

    // No criteria are asked of a document: its own C7 checks are what it is held to.
    verb(&world, &dir, &root, &["check", "design-doc", "--item", &id])
        .exited(0)
        .out_has(&format!("item {id}: ok"));
    write(&hosted, &document_template("second draft"));
    verb(&world, &dir, &root, &["check", "design-doc", "--item", &id])
        .exited(REFUSED)
        .err_has(RULE_TEMPLATE_CHANGED)
        .err_has(&format!(
            "onepipeline template resolve design-doc --json | onetaskgraph document render {id} \
             --template-loader -"
        ));
}

#[test]
fn nothing_in_this_repository_ships_a_template_but_the_plan_task_base() {
    let listed = std::process::Command::new("git")
        .args(["ls-files", "*.j2", "**/*.j2"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("git lists the tree");
    let files: std::collections::BTreeSet<String> = String::from_utf8_lossy(&listed.stdout)
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        files,
        std::collections::BTreeSet::from(["src/templates/onepipeline/plan-task.md.j2".to_owned()])
    );
}
