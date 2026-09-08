//! What a lifecycle node's **destination repository** says about it, refused
//! before anything is dispatched.
//!
//! Every journey here drives the compiled binary against a real `onetaskgraph`
//! store and a real `onevcs` registry, and the rules that decide each refusal are
//! the repository's own: a `commit-msg` hook this suite installs into a real
//! checkout and the engine runs the way git runs it, and a policy this world's
//! own `rules.yml` resolves through `onevcs`'s own resolution verbs.

// llmlint: ignore-file[e2e_not_mocked] the refusal and acceptance journeys substitute
// nothing — real binary, real store, real registry, real hook. The could-not-check
// journeys point `ONEPIPELINE_ONEVCS_BIN` at an absent or ill-answering program, which is
// the state under test rather than a stand-in: a real `onevcs` cannot be asked to be one
// that is not installed or answers a shape this build cannot read, the same reason
// `fake-onetaskgraph` exists beside it.

use serde_json::{json, Value};

use crate::harness::{agent, lifecycle, plan_of, Repository, World, REFUSED};

const HAS_REFUSALS: i32 = 1;

/// The variable naming the `onevcs` executable the loader asks.
///
/// Spelled here rather than reached for, because the module that declares it is
/// private: `docs/contract.md` fixes what this crate publishes, and an
/// implementation detail does not join that surface to be named by a test. The
/// same reason `STORE_BINARY_ENV` is a literal in the harness beside it.
const ONEVCS_BINARY_ENV: &str = "ONEPIPELINE_ONEVCS_BIN";

fn answer(run: &crate::harness::Run) -> Value {
    serde_json::from_str(run.stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "`onepipeline {}` did not print one JSON object ({error}):\n{}",
            run.args, run.stdout
        )
    })
}

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

fn engine() -> Value {
    let mut node = lifecycle("engine", &[]);
    node["repo"] = json!("engine");
    node
}

/// The `service` repository a `consumes` journey publishes into, with the
/// `engine` one its dependency lands in registered beside it.
fn two_repositories(world: &World, publication: &str) -> Repository {
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
    two_repositories(&world, "local-direct");

    let project = world.plan(
        "consuming",
        &plan_of("consuming", vec![engine(), consumer(None)]),
    );

    let refused = world.run(&["start", &project, "--detach"]);
    refused
        .exited(REFUSED)
        .err_has("node 'consumer'")
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
    two_repositories(&world, "local-direct");

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
    two_repositories(&world, "change-auto");

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
    two_repositories(&world, "local-direct");
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

/// An `onevcs` this world answers for, so a journey can state what a resolution
/// verb says back.
///
/// It stands in for an *install* rather than for the sibling: a real `onevcs`
/// cannot be asked to answer a shape this build does not read, and what is under
/// test is what the loader does when one does. Each verb's answer is a file this
/// world scripts, so one program serves every arm below.
#[cfg(unix)]
fn onevcs_answering(world: &World) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = world.root.join("onevcs-answering");
    // Shell builtins only — no `cat`, no `sed` — because one journey runs this
    // with an empty `PATH` to be a host without git. The `|| [ -n "$line" ]`
    // is what carries a file whose last line has no newline, which every JSON
    // answer below is.
    std::fs::write(
        &path,
        format!(
            r#"#!/bin/sh
answer() {{
  body=''
  while IFS= read -r line || [ -n "$line" ]; do body="$body$line
"; done < "$1"
  case "$body" in
    '!refuse '*) printf '%s' "${{body#!refuse }}" >&2; exit 1 ;;
    '!bytes'*) printf '\377\376' ; exit 0 ;;
  esac
  printf '%s' "$body"
}}
case "$1 $2" in
  'resolve '*) answer '{fakes}/onevcs.resolve' ;;
  'rules check') answer '{fakes}/onevcs.rules-check' ;;
  *) printf 'no such verb: %s\n' "$*" >&2; exit 1 ;;
esac
"#,
            fakes = world.fakes.display()
        ),
    )
    .expect("the answering onevcs is written");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("it is executable");
    path
}

#[cfg(unix)]
const RULES_CHECK_REFUSES: &str = "!refuse this host has no rules file";

/// A verb that succeeds and answers bytes that are not UTF-8.
#[cfg(unix)]
const ANSWERS_NOT_UTF8: &str = "!bytes";

/// Write the two answers that world gives, for one arm.
#[cfg(unix)]
fn script_the_answers(world: &World, resolve: &str, rules_check: &str) {
    world.script("onevcs.resolve", resolve);
    world.script("onevcs.rules-check", rules_check);
}

/// A resolution verb whose answer this build cannot read leaves the node
/// **unchecked**, in each of the shapes an answer can be wrong in.
///
/// The arm the crate is most exposed to, because it is the one that changes
/// without this tree changing: `onevcs resolve`'s answer is external input, and
/// the loader reads it into a typed shape rather than probing keys off it. An
/// answer that is not JSON, one missing a field this build reads, one whose
/// `workflow` is outside the sibling's own enum, and one whose identity is there
/// and blank are each a node reported as not checked — never a node waved through
/// — and each says which of them it was.
///
/// The plan is the one the `consumes` journey above refuses, so what a wrong
/// answer costs here is exactly a refusal that would otherwise have been made.
#[cfg(unix)]
#[test]
fn a_resolution_this_build_cannot_read_leaves_the_node_unchecked_rather_than_passed() {
    let world = World::new("destination-unreadable-resolve");
    two_repositories(&world, "local-direct");
    let project = world.plan(
        "consuming",
        &plan_of("consuming", vec![engine(), consumer(None)]),
    );
    let stand_in = onevcs_answering(&world);
    let world = world.with_env(ONEVCS_BINARY_ENV, &stand_in.to_string_lossy());

    let cases: &[(&str, &str, &str)] = &[
        (
            "an answer that is not JSON at all",
            "not json at all",
            "did not answer the shape this build reads",
        ),
        (
            "an answer missing a field this build reads",
            r#"{"identity": "github.com/owner/service", "workflow": "remote"}"#,
            "publication_checkout",
        ),
        (
            "a workflow outside the sibling's own enum",
            r#"{"identity": "i", "workflow": "sideways", "publication_checkout": "/tmp/x"}"#,
            "sideways",
        ),
        (
            "an identity that is there and blank",
            r#"{"identity": "  ", "workflow": "remote", "publication_checkout": "/tmp/x"}"#,
            "states a blank identity",
        ),
        (
            "a checkout that names no place on this host",
            r#"{"identity": "i", "workflow": "remote", "publication_checkout": "service"}"#,
            "publication checkout that is not an absolute path",
        ),
    ];

    for (why, resolve, said) in cases {
        script_the_answers(
            &world,
            resolve,
            "publication: change-auto (from the default)\n",
        );
        let asked = world.run(&["plan", "check", &project]);
        asked
            .exited(0)
            .err_has("node 'consumer'")
            .err_has("the plan loaded without that check having run");
        assert!(
            asked.stderr.contains(said),
            "{why} did not say what was wrong with it — looked for {said:?} in:\n{}",
            asked.stderr
        );
    }
}

/// A policy this build cannot read stops **only** the rule that needed it: the
/// repository's own hook still answers about the title.
///
/// The split this module is built around, driven through the binary. The
/// resolution names a real checkout carrying a real `commit-msg` hook, and the
/// policy comes back on no line this build can read — so the `consumes` rule is
/// reported as unchecked and the title is refused, in one run.
#[cfg(unix)]
#[test]
fn a_policy_this_build_cannot_read_still_lets_the_repositorys_own_hook_answer() {
    let world = World::new("destination-unreadable-policy");
    let service = two_repositories(&world, "local-direct");
    world.commit_msg_hook(&service);

    let mut node = consumer(None);
    node["title"] = json!("refactor(loader): tidy the seam");
    let project = world.plan("consuming", &plan_of("consuming", vec![engine(), node]));
    let stand_in = onevcs_answering(&world);
    script_the_answers(
        &world,
        &json!({
            "identity": "github.com/owner/service",
            "workflow": "remote",
            "publication_checkout": service.checkout.to_string_lossy(),
        })
        .to_string(),
        "repo: service\napprovals: none (from the default)\n",
    );
    let world = world.with_env(ONEVCS_BINARY_ENV, &stand_in.to_string_lossy());

    let asked = world.run(&["plan", "check", &project, "--json"]);
    asked
        .exited(HAS_REFUSALS)
        .err_has("states no `publication:` line")
        .err_has("the plan loaded without that check having run");
    let answered = answer(&asked);
    let refusals = engine_refusals(&answered);
    assert_eq!(refusals.len(), 1, "{answered}");
    assert_eq!(refusals[0]["node"], json!("consumer"), "{answered}");
    assert_eq!(refusals[0]["field"], json!("title"), "{answered}");
    assert!(
        refusals[0]["reason"]
            .as_str()
            .expect("a reason")
            .contains("this repository does not release from 'refactor:'"),
        "{answered}"
    );
}

/// A publication checkout git cannot answer for is a title this build could not
/// put to anything, and the plan loads.
///
/// The other half of the hook's own failure surface: where the checkout is no
/// repository, there is no hooks directory to find and so no verdict to read —
/// which is not the same answer as a repository that stated no policy.
#[cfg(unix)]
#[test]
fn a_checkout_git_cannot_answer_for_leaves_the_title_unchecked() {
    let world = World::new("destination-bad-checkout");
    world.repository("change-auto", &[]);
    let project = world.plan("titled", &plan_of("titled", vec![lifecycle("ship", &[])]));

    let nowhere = world.root.join("not-a-repository");
    std::fs::create_dir_all(&nowhere).expect("a directory that is no repository");
    let stand_in = onevcs_answering(&world);
    script_the_answers(
        &world,
        &json!({
            "identity": "github.com/owner/service",
            "workflow": "remote",
            "publication_checkout": nowhere.to_string_lossy(),
        })
        .to_string(),
        "publication: change-auto (from the default)\n",
    );
    world
        .with_env(ONEVCS_BINARY_ENV, &stand_in.to_string_lossy())
        .run(&["plan", "check", &project])
        .exited(0)
        .err_has("the plan loaded without that check having run")
        .err_has("git does not answer for the publication checkout");
}

/// A hook git would skip states no policy; a hook that cannot be started states
/// no verdict either — and the two are reported differently.
///
/// Both are the repository's own file in a real checkout. The first is git's own
/// test: a `commit-msg` without the executable bit is one git does not run, so the
/// repository has no subject policy and the title loads in silence. The second is
/// a hook that *is* executable and cannot be started at all, which is a node this
/// build could not check, and says so.
#[cfg(unix)]
#[test]
fn a_hook_git_would_skip_and_a_hook_that_cannot_start_are_told_apart() {
    use std::os::unix::fs::PermissionsExt;

    let world = World::new("destination-hook-unrunnable");
    let service = world.repository("change-auto", &[]);
    world.commit_msg_hook(&service);
    let hook = crate::harness::commit_msg_hook(&world);

    let mut node = lifecycle("tidy", &[]);
    node["title"] = json!("refactor(loader): tidy the seam");
    let project = world.plan("titled", &plan_of("titled", vec![node]));

    // Present and not executable: git skips it, so the repository states nothing.
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o644))
        .expect("the hook is made one git would skip");
    world
        .run(&["plan", "check", &project])
        .exited(0)
        .err_lacks("the plan loaded without that check having run");

    // Executable, and no interpreter to start it with: nobody was asked.
    std::fs::write(&hook, "#!/nonexistent/interpreter\nexit 0\n").expect("the hook is rewritten");
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))
        .expect("the hook is executable");
    world
        .run(&["plan", "check", &project])
        .exited(0)
        .err_has("the plan loaded without that check having run")
        .err_has("could not be run");
}

/// A node that states its **own** change-request policy consumes without being
/// refused, whatever its repository's rules resolve to.
///
/// `onevcs` lets a node narrow its repository's policy, and narrowing to a
/// `change-*` one is exactly what gives its draft something to be a state of. The
/// refusal is about the policy the node actually publishes under rather than the
/// one its repository would have chosen for it.
#[test]
fn a_node_that_names_its_own_change_policy_consumes_without_being_refused() {
    let world = World::new("destination-narrowed");
    two_repositories(&world, "local-direct");

    let mut node = consumer(None);
    node["merge_policy"] = json!("change-open");
    let project = world.plan("consuming", &plan_of("consuming", vec![engine(), node]));
    world.run(&["plan", "check", &project]).exited(0);
}

/// Every **other** way a policy can fail to arrive, driven through the binary.
///
/// A verb that refuses after resolving, one that states a policy the sibling's own
/// type does not know, one whose stdout is not UTF-8, and one whose line this
/// build reads no policy off at all: none of them is a repository that opens a
/// change request, so none may pass the `consumes` rule. The plan is the one the
/// `consumes` journey refuses, so what each of these costs is exactly that
/// refusal.
#[cfg(unix)]
#[test]
fn a_rules_check_this_build_cannot_read_a_policy_off_leaves_the_node_unchecked() {
    let world = World::new("destination-policy-arms");
    let service = two_repositories(&world, "local-direct");
    let project = world.plan(
        "consuming",
        &plan_of("consuming", vec![engine(), consumer(None)]),
    );
    let stand_in = onevcs_answering(&world);
    let resolved = json!({
        "identity": "github.com/owner/service",
        "workflow": "remote",
        "publication_checkout": service.checkout.to_string_lossy(),
    })
    .to_string();
    let world = world.with_env(ONEVCS_BINARY_ENV, &stand_in.to_string_lossy());

    for (rules_check, said) in [
        (RULES_CHECK_REFUSES, "refused: this host has no rules file"),
        (
            "publication: sideways (from rule 1)\n",
            "states a publication policy this build does not know, 'sideways'",
        ),
        (ANSWERS_NOT_UTF8, "answered bytes that are not UTF-8"),
        (
            "publication: local-direct, from rule 1\n",
            "line this build does not read",
        ),
    ] {
        script_the_answers(&world, &resolved, rules_check);
        let asked = world.run(&["plan", "check", &project]);
        asked
            .exited(0)
            .err_has("node 'consumer'")
            .err_has("the plan loaded without that check having run");
        assert!(
            asked.stderr.contains(said),
            "the arm did not say what went wrong — looked for {said:?} in:\n{}",
            asked.stderr
        );
    }
}

/// A host with no `git` cannot be asked where a repository keeps its hooks, so the
/// title is one this build could not check.
///
/// Where the hook lives is git's own answer — `core.hooksPath` and all — so a host
/// that cannot start git has no way to reach the file that states the policy. The
/// `PATH` this journey runs under holds nothing, which is the only honest way to
/// be a host without one; every other program the run needs is named absolutely.
#[cfg(unix)]
#[test]
fn a_host_that_cannot_start_git_leaves_the_title_unchecked() {
    let world = World::new("destination-no-git");
    let service = world.repository("change-auto", &[]);
    let project = world.plan("titled", &plan_of("titled", vec![lifecycle("ship", &[])]));
    let stand_in = onevcs_answering(&world);
    script_the_answers(
        &world,
        &json!({
            "identity": "github.com/owner/service",
            "workflow": "remote",
            "publication_checkout": service.checkout.to_string_lossy(),
        })
        .to_string(),
        "publication: change-auto (from the default)\n",
    );
    world
        .with_env(ONEVCS_BINARY_ENV, &stand_in.to_string_lossy())
        .with_env("PATH", "")
        .run(&["plan", "check", &project])
        .exited(0)
        .err_has("the plan loaded without that check having run")
        .err_has("git could not be run in the publication checkout");
}

/// A hook the filesystem will not answer for is not a hook that is absent.
///
/// The distinction that decides whether a repository's policy is applied at all:
/// read as "no hook", a repository that does state one would have none applied and
/// nobody would be told. A hooks directory this process may not look inside is the
/// state that separates them.
#[cfg(unix)]
#[test]
fn a_hook_the_filesystem_will_not_answer_for_is_not_one_that_is_absent() {
    use std::os::unix::fs::PermissionsExt;

    let world = World::new("destination-hook-unreadable");
    let service = world.repository("change-auto", &[]);
    world.commit_msg_hook(&service);
    let hooks = crate::harness::commit_msg_hook(&world)
        .parent()
        .expect("the hook has a directory")
        .to_path_buf();

    let project = world.plan("titled", &plan_of("titled", vec![lifecycle("ship", &[])]));
    std::fs::set_permissions(&hooks, std::fs::Permissions::from_mode(0o000))
        .expect("the hooks directory is closed");
    let asked = world.run(&["plan", "check", &project]);
    // Restored before the assertions, so a failure does not leave the world
    // undeletable and bury this journey's own report under a cleanup panic.
    std::fs::set_permissions(&hooks, std::fs::Permissions::from_mode(0o755))
        .expect("the hooks directory is reopened");
    asked
        .exited(0)
        .err_has("the plan loaded without that check having run")
        .err_has("cannot be read");
}

/// A host with nowhere to write the message leaves the title unchecked.
///
/// git hands a `commit-msg` hook a *file*, so a host whose temporary directory
/// cannot hold one has no way to ask the question at all.
#[cfg(unix)]
#[test]
fn a_host_that_cannot_write_the_message_leaves_the_title_unchecked() {
    let world = World::new("destination-no-tmpdir");
    let service = world.repository("change-auto", &[]);
    world.commit_msg_hook(&service);
    let project = world.plan("titled", &plan_of("titled", vec![lifecycle("ship", &[])]));

    let not_a_directory = world.root.join("tmp-is-a-file");
    std::fs::write(&not_a_directory, "").expect("a file where a directory would go");
    world
        .with_env("TMPDIR", &not_a_directory.to_string_lossy())
        .run(&["plan", "check", &project])
        .exited(0)
        .err_has("the plan loaded without that check having run")
        .err_has("could not be written to");
}

/// A hook a signal ended reports as that rather than as an exit status it never
/// reached.
///
/// It refused the title — the publication would be refused too — and a report
/// naming "exit 0" for it would be this build inventing a verdict the hook never
/// gave.
#[cfg(unix)]
#[test]
fn a_hook_a_signal_ended_is_reported_as_that_rather_than_as_an_exit_status() {
    let world = World::new("destination-hook-signalled");
    let service = world.repository("change-auto", &[]);
    world.commit_msg_hook(&service);
    std::fs::write(
        crate::harness::commit_msg_hook(&world),
        "#!/bin/sh\nkill -TERM $$\n",
    )
    .expect("the hook is rewritten to end on a signal");

    let project = world.plan("titled", &plan_of("titled", vec![lifecycle("ship", &[])]));
    let checked = world.run(&["plan", "check", &project, "--json"]);
    checked.exited(HAS_REFUSALS);
    let answered = answer(&checked);
    let refusals = engine_refusals(&answered);
    assert_eq!(refusals.len(), 1, "{answered}");
    assert_eq!(refusals[0]["field"], json!("title"), "{answered}");
    let reason = refusals[0]["reason"].as_str().expect("a reason");
    assert!(reason.contains("killed by a signal"), "{reason}");
    assert!(
        !reason.contains("exit "),
        "a hook a signal ended was reported as an exit status it never reached: {reason}"
    );
}

/// A verb that succeeds and answers bytes that are not UTF-8 leaves the node
/// **unchecked**.
///
/// The one wrong answer a lossy decode would have hidden: replacement characters
/// in the JSON a shape is read from, or on the line a policy is read off, can
/// parse as something nobody said. Refused as an answer this build cannot read,
/// which is a node reported as not checked rather than one waved through.
#[cfg(unix)]
#[test]
fn a_verb_that_answers_bytes_that_are_not_utf8_leaves_the_node_unchecked() {
    let world = World::new("destination-not-utf8");
    two_repositories(&world, "local-direct");
    let project = world.plan(
        "consuming",
        &plan_of("consuming", vec![engine(), consumer(None)]),
    );
    let stand_in = onevcs_answering(&world);
    script_the_answers(
        &world,
        ANSWERS_NOT_UTF8,
        "publication: change-auto (from the default)\n",
    );
    world
        .with_env(ONEVCS_BINARY_ENV, &stand_in.to_string_lossy())
        .run(&["plan", "check", &project])
        .exited(0)
        .err_has("node 'consumer'")
        .err_has("the plan loaded without that check having run")
        .err_has("answered bytes that are not UTF-8");
}
