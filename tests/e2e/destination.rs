//! What a lifecycle node's **destination repository** says about it, refused
//! before anything is dispatched.
//!
//! Every journey here drives the compiled binary against a real `onetaskgraph`
//! store and a real `onevcs` registry, and the rules that decide each refusal are
//! the repository's own: a `commit-msg` hook this suite installs into a real
//! checkout and the engine runs the way git runs it, and a policy this world's
//! own `rules.yml` resolves through `onevcs`'s own resolution verbs.

// llmlint: ignore-file[e2e_not_mocked] nothing is substituted: the binary is the compiled
// one, the store is the real `onetaskgraph`, the registry and its rules file are the real
// `onevcs`'s, and the subject policy is a real hook script in a real checkout. The one
// journey that names a binary that is not there names one deliberately — that is the
// could-not-check arm, and an absent executable is the thing under test.

use serde_json::{json, Value};

use crate::harness::{agent, lifecycle, plan_of, Repository, World, REFUSED};

/// `plan check`'s exit status when something refused.
const HAS_REFUSALS: i32 = 1;

/// The variable naming the `onevcs` executable the loader asks.
///
/// Spelled here rather than reached for, because the module that declares it is
/// private: `docs/contract.md` fixes what this crate publishes, and an
/// implementation detail does not join that surface to be named by a test. The
/// same reason `STORE_BINARY_ENV` is a literal in the harness beside it.
const ONEVCS_BINARY_ENV: &str = "ONEPIPELINE_ONEVCS_BIN";

/// The one JSON object `plan check --json` prints.
fn answer(run: &crate::harness::Run) -> Value {
    serde_json::from_str(run.stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "`onepipeline {}` did not print one JSON object ({error}):\n{}",
            run.args, run.stdout
        )
    })
}

/// The engine's own refusals out of that object.
fn engine_refusals(answered: &Value) -> Vec<Value> {
    answered["refusals"]
        .as_array()
        .unwrap_or_else(|| panic!("the answer has no refusals list: {answered}"))
        .iter()
        .filter(|refusal| refusal["source"] == json!("engine"))
        .cloned()
        .collect()
}

/// A consumer node in `service` that consumes the `engine` node's release.
///
/// The dependency lands in the *other* repository, which is the condition a
/// `consumes` is written under at all.
fn consumer(adoption: Option<&str>) -> Value {
    let mut node = lifecycle("consumer", &["engine"]);
    node["consumes"] = json!({"engine": "crate"});
    if let Some(adoption) = adoption {
        node["adoption"] = json!(adoption);
    }
    node
}

/// The node whose release the consumer waits for, in its own repository.
fn engine() -> Value {
    let mut node = lifecycle("engine", &[]);
    node["repo"] = json!("engine");
    node
}

/// The two repositories a `consumes` journey needs, and the plan they carry.
fn consuming(world: &World, publication: &str) -> Repository {
    let service = world.repository(publication, &[]);
    world.extra_repository("engine");
    service
}

/// A title its destination repository's own hook turns down is refused where the
/// plan is read — naming the node, the title, and what that hook said — and
/// nothing is dispatched.
///
/// The whole cost this closes: the node ran, its judge passed it, and only then
/// did the publication meet a subject the repository does not release from.
///
/// Unix-only because the hook is a shell script and this engine runs it directly,
/// exactly as `onevcs` runs it at the publication — `Command::new` on Windows
/// starts no `#!` script, so on that platform the hook is one this build reports
/// it could not ask rather than one it read a verdict from. Matching the sibling
/// is the point: a refusal here that the publication would not make is worse than
/// no refusal at all.
#[cfg(unix)]
#[test]
fn a_title_the_destination_repositorys_own_hook_rejects_is_refused_before_any_dispatch() {
    let world = World::new("destination-title");
    let service = world.repository("change-auto", &[]);
    world.commit_msg_hook(&service);

    let mut node = lifecycle("tidy", &[]);
    node["title"] = json!("refactor(loader): tidy the seam");
    let project = world.plan("titled", &plan_of("titled", vec![node]));

    let refused = world.run(&["start", &project, "--detach"]);
    refused
        .exited(REFUSED)
        .err_has("node 'tidy'")
        .err_has("commit-msg")
        .err_has("refactor(loader): tidy the seam")
        // The hook's own sentence, carried whole rather than restated: what makes
        // this repository's policy the one source is that the engine reports what
        // it said.
        .err_has("this repository does not release from 'refactor:'");
    assert!(
        world.invocations().is_empty(),
        "a plan refused at load dispatched something anyway: {:?}",
        world.invocations()
    );

    // And the same refusal, to a program, naming the field that has to change.
    let checked = world.run(&["plan", "check", &project, "--json"]);
    checked.exited(HAS_REFUSALS);
    let answered = answer(&checked);
    assert_eq!(answered["accepted"], json!(false), "{answered}");
    let refusals = engine_refusals(&answered);
    assert_eq!(refusals.len(), 1, "{answered}");
    assert_eq!(refusals[0]["node"], json!("tidy"), "{answered}");
    assert_eq!(refusals[0]["field"], json!("title"), "{answered}");
    assert!(
        refusals[0]["reason"]
            .as_str()
            .expect("a reason")
            .contains("this repository does not release from 'refactor:'"),
        "{answered}"
    );
}

/// The same repository, the same hook: a title it **does** release from loads.
///
/// The other half of the first journey, and the one that says the refusal is the
/// repository's rule rather than a bar on titles.
#[cfg(unix)]
#[test]
fn the_same_hook_passes_a_title_that_repository_releases_from() {
    let world = World::new("destination-title-ok");
    let service = world.repository("change-auto", &[]);
    world.commit_msg_hook(&service);

    // `lifecycle` titles a node `feat: ship <id>`, which is what this repository
    // cuts a release for.
    let project = world.plan("titled", &plan_of("titled", vec![lifecycle("ship", &[])]));
    world.run(&["plan", "check", &project]).exited(0);
}

/// A repository that declares **no** `commit-msg` hook states no subject policy,
/// so there is nothing to refuse and the node loads.
///
/// The promise that a plan launching correctly today still launches: most
/// repositories have no such hook, and acquiring one by being read here would
/// refuse every one of them.
#[test]
fn a_repository_with_no_commit_msg_hook_refuses_no_title() {
    let world = World::new("destination-no-hook");
    world.repository("change-auto", &[]);

    let mut node = lifecycle("tidy", &[]);
    node["title"] = json!("refactor(loader): tidy the seam");
    let project = world.plan("titled", &plan_of("titled", vec![node]));
    world.run(&["plan", "check", &project]).exited(0);
}

/// A non-empty `consumes` on an identity that opens no change request is refused
/// where the plan is read — naming the node, the identity, the workflow, the
/// policy and the targets — and nothing is dispatched.
///
/// The structural one: the draft that holds this node's temporary pin is a state
/// of a change request, and a `local-direct` publication opens none, so `onevcs`
/// refuses the publication outright at the last step of the node.
#[test]
fn a_consumes_on_a_repository_that_opens_no_change_request_is_refused_before_any_dispatch() {
    let world = World::new("destination-consumes");
    consuming(&world, "local-direct");

    let project = world.plan(
        "consuming",
        &plan_of("consuming", vec![engine(), consumer(None)]),
    );

    let refused = world.run(&["start", &project, "--detach"]);
    refused
        .exited(REFUSED)
        .err_has("node 'consumer'")
        // The identity, the workflow, the policy, and the targets it asked to
        // consume.
        .err_has("github.com/owner/service")
        .err_has("workflow: remote")
        .err_has("local-direct")
        .err_has("engine=crate");
    assert!(
        world.invocations().is_empty(),
        "a plan refused at load dispatched something anyway: {:?}",
        world.invocations()
    );

    let checked = world.run(&["plan", "check", &project, "--json"]);
    checked.exited(HAS_REFUSALS);
    let answered = answer(&checked);
    let refusals = engine_refusals(&answered);
    assert_eq!(refusals.len(), 1, "{answered}");
    assert_eq!(refusals[0]["node"], json!("consumer"), "{answered}");
    assert_eq!(refusals[0]["field"], json!("consumes"), "{answered}");
}

/// The **`published`** arm of the same repository loads, and that is not an
/// oversight.
///
/// A `published` node is not started until every release it consumes has arrived,
/// so it holds no temporary pin, asks for no draft, and publishes under
/// `local-direct` exactly as it publishes anywhere. Refusing it would take a
/// working capability away.
#[test]
fn a_published_node_consuming_on_that_same_repository_still_loads() {
    let world = World::new("destination-consumes-published");
    consuming(&world, "local-direct");

    let project = world.plan(
        "consuming",
        &plan_of("consuming", vec![engine(), consumer(Some("published"))]),
    );
    world.run(&["plan", "check", &project]).exited(0);
}

/// A repository that **does** open a change request has something to draft, so
/// the same node loads there.
#[test]
fn a_consumes_on_a_repository_that_opens_a_change_request_loads() {
    let world = World::new("destination-consumes-change");
    consuming(&world, "change-auto");

    let project = world.plan(
        "consuming",
        &plan_of("consuming", vec![engine(), consumer(None)]),
    );
    world.run(&["plan", "check", &project]).exited(0);
}

/// An identity this host cannot resolve is **not** a refusal — and not a silent
/// pass either: the plan loads and the loader says the check never ran.
///
/// Both halves matter. Refusing here would refuse every plan for a repository
/// this host has not registered, which is a plan that launches correctly today;
/// loading in silence would let it load looking as though it had cleared a bar
/// nobody applied.
#[test]
fn a_node_whose_identity_this_host_cannot_resolve_loads_and_says_it_was_not_checked() {
    let world = World::new("destination-unresolvable");

    let mut node = lifecycle("tidy", &[]);
    node["repo"] = json!("github.com/owner/nobody-registered-this");
    // A title the hook in the journeys above turns down, and a `consumes` that
    // would be refused on a `local-direct` identity: neither is reachable without
    // an identity to ask.
    node["title"] = json!("refactor(loader): tidy the seam");
    let project = world.plan(
        "unresolvable",
        &plan_of("unresolvable", vec![agent("engine", &[]), node]),
    );

    world
        .run(&["plan", "check", &project])
        .exited(0)
        .err_has("node 'tidy'")
        .err_has("github.com/owner/nobody-registered-this")
        .err_has("the plan loaded without that check having run");
}

/// A host with no resolution verb reports every lifecycle node as one it could
/// not check, and loads the plan.
///
/// The arm that would otherwise be invisible: an absent `onevcs` executable makes
/// the two refusals unaskable, and "nobody was asked" must never read as "the
/// repository was satisfied". The refusal that *would* have been made here is the
/// `consumes` one — this is the very plan the journey above refuses.
#[test]
fn a_host_with_no_resolution_verb_says_the_node_was_not_checked_rather_than_passing_it() {
    let world = World::new("destination-no-verb");
    consuming(&world, "local-direct");
    let project = world.plan(
        "consuming",
        &plan_of("consuming", vec![engine(), consumer(None)]),
    );

    let absent = world.root.join("no-onevcs-here");
    let world = world.with_env(ONEVCS_BINARY_ENV, &absent.to_string_lossy());
    world
        .run(&["plan", "check", &project])
        .exited(0)
        .err_has("node 'consumer'")
        .err_has("the plan loaded without that check having run")
        .err_has("could not be run");
}
