//! A run's public id: what `status --json` reads it as, that it is decided once at the launch,
//! that a launch whose check cannot answer is refused, and where the engine writes it — the
//! claim, fence and name on a plan store whose source is not declared private, and `plan.name`
//! on a public repository's branch — driven end to end against the compiled binary.
//!
//! The host registers two repositories: `service`, which its rules file declares public, and
//! `zephyrine`, which it declares private, so `zephyrine` is a term no public output may carry.
//! Plan A is named after it; plan B carries a neutral name and writes exactly what it did before
//! there was a public id.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its subprocess
// boundary and nothing inside the crate under test, which is driven as a real compiled binary.
// The public-output check is the linked `onevcs`'s own, over a real registry and real
// repositories on disk, and both plan sources are the real `local-md` plugin of the real store
// the binary links. `harness.rs` carries the full rationale.

// llmlint: ignore-file[expensive_tests_stay_behind_their_own_edge] each journey drives real
// launches through the linked `onevcs` over real git, exercising `publicid`, `driver`,
// `continuation`, `writeback`, `branchname` and `views` together, so the crate is the narrowest
// edge they can honestly sit behind, on the same grounds as `mod publication_boundary`.

use serde_json::{json, Value};

use crate::harness::{agent, held_publication, lifecycle, plan_of, project_id, World};
use crate::publication_boundary::declare_visibility;

/// Plan A's name: the private repository's own.
const A: &str = "zephyrine-rollout";
/// Plan B's name, which names nothing private.
const B: &str = "widget-rollout";
/// The second plan source, which the store declares `visibility: private`.
const SECRET: &str = "secret";

/// `run-` and the first twelve lowercase hex digits of the SHA-256 of `run`'s UTF-8 bytes.
fn hashed(run: &str) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(run.as_bytes());
    let hex: String = digest
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("run-{hex}")
}

/// A world registering `service`, declared public, and `zephyrine`, declared private.
fn registered(world: &World) {
    world.extra_repository("zephyrine");
    declare_visibility(world, &[("service", "public"), ("zephyrine", "private")]);
}

/// `status <run> --json`, validated against `schemas/run-reading.schema.json`.
fn reading(world: &World, run: &str) -> Value {
    let printed = world.run(&["status", run, "--json"]);
    printed.exited(0);
    let reading: Value = serde_json::from_str(printed.stdout.trim())
        .unwrap_or_else(|error| panic!("`status {run} --json` printed no JSON ({error})"));
    let schema: Value = serde_json::from_str(include_str!("../../schemas/run-reading.schema.json"))
        .expect("the committed schema is JSON");
    assert!(
        schema["required"]
            .as_array()
            .expect("a required list")
            .contains(&json!("public_id")),
        "the schema does not declare `public_id` required"
    );
    let validator = jsonschema::validator_for(&schema).expect("the schema compiles");
    let errors: Vec<String> = validator
        .iter_errors(&reading)
        .map(|error| error.to_string())
        .collect();
    assert!(
        errors.is_empty(),
        "`status {run} --json` does not validate: {errors:?}\n{reading}"
    );
    reading
}

/// The launch record of `run`, as the file holds it.
fn record(world: &World, run: &str) -> Value {
    world.run_json(run, "launch.json")
}

/// Write `run`'s launch record back as `record`.
fn rewrite(world: &World, run: &str, record: &Value) {
    std::fs::write(
        world.run_file(run, "launch.json"),
        serde_json::to_string_pretty(record).expect("a record renders"),
    )
    .expect("the launch record is written");
}

#[test]
fn status_reads_the_public_id_the_launch_recorded_and_an_older_record_derives_it() {
    let world = World::new("public-id-status");
    world.repository("local-direct", &[]);
    registered(&world);
    for name in [A, B] {
        let project = world.plan(name, &plan_of(name, vec![agent("work", &[])]));
        world.run(&["start", &project, "--attach"]).settled();
    }

    // A's id carries the private repository's name, so its public id is the hashed form; B's
    // passes, so its public id is its run id. Both decided at the launch and recorded there.
    let a = reading(&world, A);
    assert_eq!(a["run_id"], A, "{a}");
    assert_eq!(a["public_id"], json!(hashed(A)), "{a}");
    assert_eq!(record(&world, A)["public_id"], json!(hashed(A)));
    let b = reading(&world, B);
    assert_eq!(b["public_id"], B, "{b}");
    assert_eq!(record(&world, B)["public_id"], B);

    // A record written before there was a public id holds none, and reads the same
    // derivation over its id.
    let launched_a = record(&world, A);
    for (run, expected) in [(A, hashed(A)), (B, B.to_owned())] {
        let mut older = record(&world, run);
        older
            .as_object_mut()
            .expect("a record is a mapping")
            .remove("public_id");
        rewrite(&world, run, &older);
        assert_eq!(
            reading(&world, run)["public_id"],
            json!(expected),
            "a record holding no public id read another derivation"
        );
    }
    rewrite(&world, A, &launched_a);

    // Recorded, not derived on each read: with A's repository no longer registered private,
    // the check would now pass A's id — and A still reads the public id its launch recorded.
    declare_visibility(&world, &[("service", "public"), ("zephyrine", "public")]);
    assert_eq!(reading(&world, A)["public_id"], json!(hashed(A)));
    let mut older = record(&world, A);
    older
        .as_object_mut()
        .expect("a record is a mapping")
        .remove("public_id");
    rewrite(&world, A, &older);
    assert_eq!(
        reading(&world, A)["public_id"],
        A,
        "an older record of A does not derive its public id over the registry as it now stands"
    );
}

#[test]
fn a_launch_whose_public_output_check_cannot_answer_is_refused_before_anything_is_dispatched() {
    let world = World::new("public-id-unanswerable");
    world.repository("local-direct", &[]);
    registered(&world);
    // A third private repository whose committed term declaration cannot be read: every check
    // over the whole registry is unavailable while it stands.
    let broken = world.extra_repository("gamma");
    declare_visibility(
        &world,
        &[
            ("service", "public"),
            ("zephyrine", "private"),
            ("gamma", "private"),
        ],
    );
    std::fs::write(
        broken.checkout.join("private-terms.toml"),
        "schema_version = 1\nterms = [\n",
    )
    .expect("a broken declaration");
    crate::harness::git(&world, &broken.checkout, &["add", "-A"]);
    crate::harness::git(
        &world,
        &broken.checkout,
        &["commit", "-qm", "chore: break it"],
    );

    let project = world.plan(A, &plan_of(A, vec![agent("work", &[])]));
    let refused = world.run(&["start", &project, "--attach"]);
    assert_ne!(refused.code, 0, "{}", refused.stdout);
    refused.err_has("the public-output check");
    assert!(
        world
            .invocations()
            .iter()
            .all(|call| call["tool"] != "oneagentgraph"),
        "a node was dispatched under a launch whose check could not answer"
    );
    let recorded: Vec<String> = std::fs::read_dir(&world.runs)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().join("launch.json").is_file())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        recorded.is_empty(),
        "a refused launch left a run recording a public id: {recorded:?}"
    );
}

/// The plan store's metadata of `project`, as the store holds it now.
fn stored(world: &World, project: &str) -> Value {
    world.store_project(project)["items"][0]["item"]["metadata"].clone()
}

/// The branch `node` of `run` cut, off its session's own `session-opened`.
fn branch_of(world: &World, run: &str, node: &str) -> String {
    world
        .events_of(run, "session-opened")
        .iter()
        .filter(|event| event["labels"]["node"] == node)
        .find_map(|event| event["payload"]["branch"].as_str().map(str::to_owned))
        .unwrap_or_else(|| panic!("node {node} of {run} cut no branch\n{}", world.dump()))
}

/// What a held launch of `project` wrote on its plan store while its one publication was
/// under way — the claim's run, the fence's run and the plan's name — and the branch it cut,
/// once it settled.
struct Written {
    driver_run: Value,
    publishing_run: Value,
    name: Value,
    branch: String,
}

fn launched_held(
    world: &World,
    held: &crate::harness::HeldPublication,
    run: &str,
    project: &str,
) -> Written {
    held.rearm();
    world.script("ship.work", &format!("shipped by {run}\n"));
    world.run(&["start", project, "--detach"]).exited(0);
    world.until("the publication to be under way", |_| held.holding());
    world.until("the fence to land on the store", |world| {
        !stored(world, project)["onepipeline.publishing"].is_null()
    });
    let metadata = stored(world, project);
    held.release();
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    let projected = std::fs::read_to_string(world.run_file(run, "writeback-projections.jsonl"))
        .unwrap_or_default();
    assert!(
        !projected.contains("boundary") && !projected.contains("refused"),
        "a write-back of {run} was refused: {projected}"
    );
    Written {
        driver_run: metadata["onepipeline.driver"]["run"].clone(),
        publishing_run: metadata["onepipeline.publishing"]["run"].clone(),
        name: metadata["onepipeline.name"].clone(),
        branch: branch_of(world, run, "ship"),
    }
}

#[test]
fn a_store_not_declared_private_and_a_public_branch_carry_the_public_id_and_nothing_else_changes() {
    let world = World::new("public-id-writes");
    let secret_root = world.root.join("secret-store");
    std::fs::create_dir_all(&secret_root).expect("a source root");
    // A second source, declared private; declaring a visibility switches the store's boundary
    // on, so every write to the plans source, which is not declared private, is checked.
    let world = world
        .with_env("ONETASKGRAPH_SOURCES__SECRET__PLUGIN", "local-md")
        .with_env(
            "ONETASKGRAPH_SOURCES__SECRET__CONFIG__ROOT",
            &secret_root.to_string_lossy(),
        )
        .with_env("ONETASKGRAPH_SOURCES__SECRET__VISIBILITY", "private")
        .with_env("ONETASKGRAPH_DEFAULT_SOURCES", "plans,secret");
    let held = held_publication(&world, &world.fakes.join("push.go"));
    world.repository("local-direct", &held.argv());
    registered(&world);
    // One node, publishing into `service`, the public repository, whose push is held so that
    // the fence its publication writes can be read while it stands. The project states its
    // name as `onepipeline.name`, which is the key the write-back keeps in step.
    let plan = |name: &str| {
        let mut plan = plan_of(name, vec![lifecycle("ship", &[])]);
        plan["onepipeline.name"] = json!(name);
        plan
    };

    // Plan A, homed on the source that is not declared private.
    let on_plans = world.plan(A, &plan(A));
    let public = launched_held(&world, &held, A, &on_plans);
    assert_eq!(
        public.driver_run,
        json!(hashed(A)),
        "the claim names A's run"
    );
    assert_eq!(
        public.publishing_run,
        json!(hashed(A)),
        "the fence names A's run"
    );
    assert_eq!(public.name, json!(hashed(A)), "the store names A's plan");
    assert!(
        public.branch.contains(&hashed(A)) && !public.branch.contains("zephyrine"),
        "a public repository's branch names A: {}",
        public.branch
    );

    // Plan A again, homed on the source declared private: it carries the run id and the
    // plan's own name, as it always did; its branch is still a public repository's.
    let again = format!("{A}-2");
    world.plan_in(&secret_root, A, &plan(A));
    let on_secret = format!("{SECRET}:{}", project_id(A));
    let private = launched_held(&world, &held, &again, &on_secret);
    assert_eq!(private.driver_run, json!(again));
    assert_eq!(private.publishing_run, json!(again));
    assert_eq!(private.name, A);
    assert!(
        private.branch.contains(&hashed(&again)) && !private.branch.contains("zephyrine"),
        "a public repository's branch names A: {}",
        private.branch
    );

    // Plan B passes the check, so it writes exactly what a run wrote before there was a
    // public id: its run id, its own name, and a branch named after it.
    let neutral = world.plan(B, &plan(B));
    let unchanged = launched_held(&world, &held, B, &neutral);
    assert_eq!(unchanged.driver_run, B);
    assert_eq!(unchanged.publishing_run, B);
    assert_eq!(unchanged.name, B);
    assert!(
        unchanged.branch.ends_with(&format!("{B}/ship")),
        "{}",
        unchanged.branch
    );
}
