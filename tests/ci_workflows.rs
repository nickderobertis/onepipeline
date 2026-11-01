//! What the committed workflows promise a pull request and a push to `main`,
//! read off the workflows themselves.
//!
//! **The fixed context contract.** Branch protection on `main` requires four
//! status-check contexts by their rendered names — `lint`, `test-linux`,
//! `test-windows` and `test-macos` — and GitHub holds that set; nothing here can
//! read it. What this tree can hold is the other half: a job reporting each of
//! those names on every pull request, behind no condition or `needs` edge that
//! could leave it unreported. A job renamed or given a new `if:` would otherwise
//! land unnoticed, and a required context that never reports blocks every merge.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde_json::Value;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn workflow(file: &str) -> Value {
    let path = repo_root().join(".github/workflows").join(file);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()));
    onepipeline_testfakes::yaml::from_str(&text)
        .unwrap_or_else(|e| panic!("{file} does not parse: {e}"))
}

fn workflows() -> BTreeMap<String, Value> {
    fs::read_dir(repo_root().join(".github/workflows"))
        .expect("the workflows directory reads")
        .map(|entry| entry.expect("a workflow entry").file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".yml") || name.ends_with(".yaml"))
        .map(|name| {
            let parsed = workflow(&name);
            (name, parsed)
        })
        .collect()
}

/// The contexts branch protection requires, read from `.github/required-contexts.json`,
/// the tree's one copy of them, which `.github/ci-shape/` reads too. A change to that
/// list is a governance change, applied by `setup_github_governance.py`, and never an
/// edit to match a renamed job.
fn required_contexts() -> Vec<String> {
    let path = repo_root().join(".github/required-contexts.json");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()));
    let parsed: Value = serde_json::from_str(&text).expect("required-contexts.json parses");
    parsed["contexts"]
        .as_array()
        .expect("required-contexts.json lists its contexts")
        .iter()
        .map(|context| context.as_str().expect("a context is a name").to_owned())
        .collect()
}

fn is_required(name: &str) -> bool {
    required_contexts().iter().any(|context| context == name)
}

/// The conditions a required job may carry. Each one either always holds on a
/// pull request or skips the job — and a skipped job reports its context as
/// passed — so none of them can leave a context unreported. A new condition is
/// a new way to be unreported, and has to be argued for here. The second is
/// `test-macos`'s: it skips release-plz's own release pull request.
const ALLOWED_CONDITIONS: &[&str] = &[
    "github.event_name == 'pull_request'",
    "github.event_name == 'pull_request' && !(startsWith(github.head_ref, 'release-plz-') && github.event.pull_request.head.repo.full_name == github.repository)",
];

fn triggers(parsed: &Value) -> &Value {
    parsed
        .get("on")
        .expect("the workflow declares its triggers")
}

fn runs_on_every_pull_request(file: &str, parsed: &Value) -> bool {
    let Some(pull_request) = triggers(parsed).get("pull_request") else {
        return false;
    };
    for filter in ["paths", "paths-ignore", "branches", "branches-ignore"] {
        assert!(
            pull_request.get(filter).is_none(),
            "{file}'s pull_request trigger has a `{filter}` filter, so a required context can go unreported"
        );
    }
    pull_request
        .get("types")
        .and_then(Value::as_array)
        .is_none_or(|types| {
            ["opened", "synchronize", "reopened"]
                .iter()
                .all(|kind| types.iter().any(|t| t.as_str() == Some(kind)))
        })
}

/// The names a job's checks render as: its `name:` (with `matrix.*` expanded),
/// or its key, with each matrix leg's values in parentheses when unnamed.
fn rendered(key: &str, job: &Value) -> Vec<String> {
    let legs: Vec<BTreeMap<String, String>> =
        match job.get("strategy").and_then(|s| s.get("matrix")) {
            None => vec![BTreeMap::new()],
            Some(matrix) => {
                if let Some(include) = matrix.get("include").and_then(Value::as_array) {
                    include
                        .iter()
                        .map(|leg| {
                            leg.as_object()
                                .expect("an include leg is a mapping")
                                .iter()
                                .map(|(k, v)| (k.clone(), scalar(v)))
                                .collect()
                        })
                        .collect()
                } else {
                    let axes: Vec<(String, Vec<String>)> = matrix
                        .as_object()
                        .expect("the matrix is a mapping")
                        .iter()
                        .map(|(k, v)| {
                            let values = v
                                .as_array()
                                .expect("a matrix axis is a list")
                                .iter()
                                .map(scalar)
                                .collect();
                            (k.clone(), values)
                        })
                        .collect();
                    axes.iter()
                        .fold(vec![BTreeMap::new()], |legs, (axis, values)| {
                            legs.iter()
                                .flat_map(|leg| {
                                    values.iter().map(move |value| {
                                        let mut leg = leg.clone();
                                        leg.insert(axis.clone(), value.clone());
                                        leg
                                    })
                                })
                                .collect()
                        })
                }
            }
        };
    legs.iter()
        .map(|leg| match job.get("name").and_then(Value::as_str) {
            Some(name) => leg.iter().fold(name.to_owned(), |name, (axis, value)| {
                name.replace(&format!("${{{{ matrix.{axis} }}}}"), value)
            }),
            None if leg.is_empty() => key.to_owned(),
            None => format!(
                "{key} ({})",
                leg.values().cloned().collect::<Vec<_>>().join(", ")
            ),
        })
        .collect()
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => onepipeline_testfakes::yaml::to_string(other)
            .expect("a scalar renders")
            .trim()
            .to_owned(),
    }
}

fn needs(job: &Value) -> Vec<String> {
    match job.get("needs") {
        None => Vec::new(),
        Some(Value::String(one)) => vec![one.clone()],
        Some(Value::Array(many)) => many
            .iter()
            .map(|need| need.as_str().expect("a needed job is named").to_owned())
            .collect(),
        Some(other) => panic!("`needs` is neither a name nor a list: {other:?}"),
    }
}

fn condition(job: &Value) -> Option<String> {
    job.get("if").map(|c| {
        c.as_str()
            .expect("an `if:` is a string")
            .trim()
            .trim_start_matches("${{")
            .trim_end_matches("}}")
            .trim()
            .to_owned()
    })
}

#[test]
fn every_required_context_is_reported_on_every_pull_request() {
    let all = workflows();
    let mut reporters: BTreeMap<String, (String, String)> = BTreeMap::new();
    for (file, parsed) in &all {
        if !runs_on_every_pull_request(file, parsed) {
            continue;
        }
        for (key, job) in parsed["jobs"].as_object().expect("the workflow has jobs") {
            for name in rendered(key, job) {
                assert!(
                    reporters
                        .insert(name.clone(), (file.clone(), key.to_owned()))
                        .is_none(),
                    "two jobs report `{name}` on a pull request"
                );
            }
        }
    }

    let mut required = required_contexts();
    required.sort();
    assert_eq!(required, ["lint", "test-linux", "test-macos", "test-windows"]);
    for context in &required {
        let (file, key) = reporters
            .get(context.as_str())
            .unwrap_or_else(|| panic!("no job reports `{context}` on a pull request"));
        assert_eq!(file, "ci.yml", "`{context}` is reported by {file}");
        let job = &all[file]["jobs"][key.as_str()];
        assert!(
            job.get("strategy").is_none(),
            "`{context}` is a matrix job, whose legs render under other names"
        );
        if let Some(condition) = condition(job) {
            assert!(
                ALLOWED_CONDITIONS.contains(&condition.as_str()),
                "`{context}` ({file} job `{key}`) runs only if `{condition}`, a condition that can leave it unreported"
            );
        }
        assert!(
            needs(job).is_empty(),
            "`{context}` waits on {:?}, whose failure would leave it waiting rather than skipped",
            needs(job)
        );
    }
}

/// The workflows that report no required context: each runs on no pull request's
/// `opened`, `synchronize` or `reopened` event, so none spends a job slot beside
/// the four, and no required job can wait on one (`needs` only reaches jobs of
/// the same workflow).
#[test]
fn no_other_workflow_reports_a_required_context() {
    for (file, parsed) in workflows() {
        if file == "ci.yml" {
            continue;
        }
        // A pull request's closing is no push to it: visual-docs.yml deletes the
        // closed pull request's preview then, and nothing else may run.
        let pull_request = triggers(&parsed).get("pull_request");
        assert!(
            pull_request.is_none_or(|trigger| trigger["types"] == serde_json::json!(["closed"])),
            "{file} runs on a pull request's push beside ci.yml: {pull_request:?}"
        );
        for (key, job) in parsed["jobs"].as_object().expect("the workflow has jobs") {
            for name in rendered(key, job) {
                assert!(
                    !is_required(&name),
                    "{file}'s `{key}` reports `{name}`, a required context"
                );
            }
        }
    }
}

const SMOKE_REPO_KEY: &str = "ONEPIPELINE_SMOKE_REPO";

/// Every string a value carries, nested or not, so a check of "nothing here
/// names a repository" reads `run:`, `env:` and `with:` alike.
fn strings(value: &Value, into: &mut Vec<String>) {
    match value {
        Value::String(s) => into.push(s.clone()),
        Value::Array(items) => items.iter().for_each(|item| strings(item, into)),
        Value::Object(map) => map.iter().for_each(|(key, item)| {
            into.push(key.clone());
            strings(item, into);
        }),
        _ => {}
    }
}

/// An `owner/name` whose name ends in the smoke's scratch suffix: the shape a
/// hard-coded scratch repository takes in a workflow.
fn scratch_slugs(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || "/-_.".contains(c)))
        .filter(|token| {
            let mut parts = token.split('/');
            matches!(
                (parts.next(), parts.next(), parts.next()),
                (Some(owner), Some(name), None) if !owner.is_empty() && name.ends_with("-smoke")
            )
        })
        .map(str::to_owned)
        .collect()
}

/// The smoke's scratch repository is configuration: the step that runs the
/// journey maps the Actions secret of the key's name into its environment
/// under that same name, every other mention of the key anywhere in the
/// workflows is that same mapping, and no step in any workflow spells a
/// scratch repository's identity itself.
#[test]
fn the_smoke_step_reads_its_scratch_repository_from_the_secret_of_the_same_name() {
    let secret = format!("${{{{ secrets.{SMOKE_REPO_KEY} }}}}");
    let ci = workflow("ci.yml");
    let steps = ci["jobs"]["test-linux"]["steps"]
        .as_array()
        .expect("ci.yml's test-linux job has steps");
    let runs: Vec<&Value> = steps
        .iter()
        .filter(|step| {
            step.get("run")
                .and_then(Value::as_str)
                .is_some_and(|run| run.contains("just ci-test-linux"))
        })
        .collect();
    let [run] = runs[..] else {
        panic!("ci.yml's test-linux job runs `just ci-test-linux`, which runs the smoke, in exactly one step: {runs:?}")
    };
    assert_eq!(
        run.get("env")
            .and_then(|env| env.get(SMOKE_REPO_KEY))
            .map(scalar),
        Some(secret.clone()),
        "the step running the smoke maps the {SMOKE_REPO_KEY} secret into its \
         environment as {SMOKE_REPO_KEY}: {run:?}"
    );
    // The secret the step reads is one `gh-secrets.json` provisions, so the
    // manifest and its consumer cannot name two different keys.
    let manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(repo_root().join("gh-secrets.json")).expect("gh-secrets.json reads"),
    )
    .expect("gh-secrets.json parses");
    assert!(
        manifest["secrets"]
            .as_array()
            .expect("gh-secrets.json lists its secrets")
            .iter()
            .any(|secret| secret["name"] == SMOKE_REPO_KEY),
        "gh-secrets.json does not provision the {SMOKE_REPO_KEY} secret the smoke step reads"
    );

    for (file, parsed) in workflows() {
        let mut env_blocks = vec![parsed.get("env")];
        for (_, job) in parsed["jobs"].as_object().into_iter().flatten() {
            env_blocks.push(job.get("env"));
            for step in job
                .get("steps")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                env_blocks.push(step.get("env"));
            }
        }
        for env in env_blocks.into_iter().flatten() {
            if let Some(value) = env.get(SMOKE_REPO_KEY) {
                assert_eq!(
                    scalar(value),
                    secret,
                    "{file} sets {SMOKE_REPO_KEY} to something other than its secret"
                );
            }
        }
        let mut said = Vec::new();
        strings(&parsed, &mut said);
        let named: Vec<String> = said.iter().flat_map(|s| scratch_slugs(s)).collect();
        assert!(
            named.is_empty(),
            "{file} hard-codes a scratch repository identity rather than reading \
             {SMOKE_REPO_KEY}: {named:?}"
        );
    }
}

#[test]
fn a_hard_coded_scratch_repository_is_recognised() {
    assert_eq!(
        scratch_slugs("ONEPIPELINE_SMOKE_REPO=hiddenco/quietharbor-smoke just smoke-real"),
        ["hiddenco/quietharbor-smoke"]
    );
    assert!(scratch_slugs("owner/name with a name ending in -smoke").is_empty());
    assert!(scratch_slugs("tests/smoke/main.rs and actions/checkout@v4").is_empty());
}

#[test]
fn the_pr_mac_lane_preserves_the_three_former_mac_proofs() {
    let ci = workflow("ci.yml");
    // The pull request's one Mac job runs its proofs through its recipe, whose
    // target list (`scripts/ci-job.sh`) holds the suite and both installs.
    let lane = &ci["jobs"]["test-macos"];
    assert_eq!(lane["runs-on"].as_str(), Some("macos-latest"));
    let commands = lane["steps"]
        .as_array()
        .expect("test-macos has steps")
        .iter()
        .filter_map(|step| step["run"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(commands.contains("just ci-test-macos"));
    let script = fs::read_to_string(repo_root().join("scripts/ci-job.sh")).expect("ci-job.sh");
    let mac_targets = script
        .lines()
        .find(|line| {
            line.trim_start()
                .starts_with("test-windows | test-macos) targets=")
        })
        .expect("ci-job.sh lists the macOS job's targets");
    for target in ["test-quick", "install-smoke", "install-documented"] {
        assert!(mac_targets.contains(target), "test-macos omitted {target}");
    }
    let mut jobs: Vec<&String> = ci["jobs"].as_object().expect("ci.yml has jobs").keys().collect();
    jobs.sort();
    assert_eq!(
        jobs,
        ["lint", "test-linux", "test-macos", "test-windows"],
        "ci.yml runs a job beside the four"
    );
}

/// Every pull-request leg that runs the e2e tier keeps the per-test profile
/// records it wrote, whatever the leg's verdict, from the directory the harness
/// writes them to: an upload naming any other path would publish nothing, and
/// `scripts/test-profile-report.mjs` would read a platform as having no records.
/// No leg moves the records elsewhere through the recorder's environment
/// variable, a leg that wrote none says so in its run, and every matrix leg
/// keeps them under a name of its own: `upload-artifact` refuses a second
/// artifact of a name a run already holds, so two shards sharing one would
/// lose the second shard's records.
#[test]
fn every_test_leg_keeps_the_profile_records_from_where_the_harness_writes_them() {
    use onepipeline_testfakes::profile::{DIR_ENV, RECORD_DIR};
    let ci = workflow("ci.yml");
    assert!(
        !onepipeline_testfakes::yaml::to_string(&ci)
            .unwrap()
            .contains(DIR_ENV),
        "ci.yml sets {DIR_ENV}, so its records are not where its uploads look"
    );
    let mut names = Vec::new();
    for key in ["test-linux", "test-windows", "test-macos"] {
        let steps = ci["jobs"][key]["steps"]
            .as_array()
            .unwrap_or_else(|| panic!("job {key} has no steps"));
        let kept: Vec<&Value> = steps
            .iter()
            .filter(|step| {
                // test-windows builds and tests in its Dev Drive checkout, so its
                // path names that checkout first.
                scalar(&step["uses"]).starts_with("actions/upload-artifact@")
                    && scalar(&step["with"]["path"])
                        .trim_start_matches("${{ env.DEV_DRIVE_WORKSPACE }}/")
                        .trim_end_matches('/')
                        == RECORD_DIR
            })
            .collect();
        assert_eq!(
            kept.len(),
            1,
            "job {key} keeps {RECORD_DIR} {} times, not once",
            kept.len()
        );
        let step = kept[0];
        assert!(
            scalar(&step["if"]).starts_with("always()"),
            "job {key}: {step}"
        );
        assert_eq!(
            scalar(&step["with"]["if-no-files-found"]),
            "warn",
            "job {key}: {step}"
        );
        assert!(
            scalar(&step["with"]["name"]).starts_with("test-profile-"),
            "job {key}: {step}"
        );
        let mut artifact = ci["jobs"][key].clone();
        artifact["name"] = step["with"]["name"].clone();
        names.extend(rendered(key, &artifact));
    }
    let mut distinct = names.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        names.len(),
        "two legs keep their records under one artifact name: {names:?}"
    );
    assert!(
        names.iter().all(|name| !name.contains("${{")),
        "an artifact name is not rendered from its leg's matrix: {names:?}"
    );
}

/// The e2e project's test targets declare the records their run writes as Nx
/// outputs, at the directory the harness writes them to, so a cached run
/// restores them: an output naming any other path would restore nothing.
#[test]
fn the_e2e_test_targets_declare_the_profile_records_as_outputs() {
    use onepipeline_testfakes::profile::RECORD_DIR;
    let path = repo_root().join("tests/e2e/project.json");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()));
    let project: Value = serde_json::from_str(&text).expect("tests/e2e/project.json parses");
    let records = format!("{{workspaceRoot}}/{RECORD_DIR}/e2e");
    for target in ["test", "test-quick"] {
        let outputs = project["targets"][target]["outputs"]
            .as_array()
            .unwrap_or_else(|| panic!("the e2e {target} target declares no outputs"));
        assert!(
            outputs
                .iter()
                .any(|output| output.as_str() == Some(&records)),
            "the e2e {target} target does not declare {records} among {outputs:?}"
        );
    }
}

/// The Windows leg is one job, `test-windows`, running the whole suite: nothing
/// hands it a share of a partition.
#[test]
fn the_windows_leg_runs_whole_in_one_job() {
    let ci = workflow("ci.yml");
    let job = &ci["jobs"]["test-windows"];
    let text = onepipeline_testfakes::yaml::to_string(job).unwrap();
    assert!(
        !text.contains("ONEPIPELINE_TEST_PARTITION") && job.get("strategy").is_none(),
        "test-windows runs a share of the suite: {text}"
    );
}
