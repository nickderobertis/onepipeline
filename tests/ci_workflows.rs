//! What the committed workflows promise a pull request, read off the workflows
//! themselves.
//!
//! Two promises live in YAML that no other check in this tree parses:
//!
//! 1. **The fixed context contract.** Branch protection on `main` requires a set
//!    of status-check contexts by their rendered names, and GitHub holds that
//!    set — nothing here can read it. What this tree can hold is the other half:
//!    a job reporting each of those names, on every pull request, behind no
//!    condition or `needs` edge that could leave it unreported. A job renamed,
//!    a matrix leg dropped, or a new `if:` would otherwise land unnoticed, and a
//!    required context that never reports blocks every merge.
//! 2. **Where the broader tier runs.** The release-plz release pull request runs
//!    one full sweep (`release-sweep.yml`), and every other pull request the
//!    affected tier (`ci.yml`'s `gate`). Which runs is decided by each job's
//!    `if:`, so the job selection is evaluated here against a synthetic event of
//!    each kind.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde_norway::Value;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn workflow(file: &str) -> Value {
    let path = repo_root().join(".github/workflows").join(file);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()));
    serde_norway::from_str(&text).unwrap_or_else(|e| panic!("{file} does not parse: {e}"))
}

/// Every workflow file this tree commits, by file name.
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

/// The contexts branch protection requires, as read at plan time. A change to
/// this list is a governance change, applied by `setup_github_governance.py`
/// and never by editing this test to match a renamed job.
// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] the source is
// GitHub's branch protection, which no offline test can read, and its drift gate
// is `setup_github_governance.py --verify`, run by governance rather than by this
// suite. What this list holds is the other side of that contract — that the
// committed workflows still report every one of these names on a pull request —
// which only the tree can answer. The visual-docs context is listed because the
// protection read at plan time requires it, whatever AGENTS.md calls advisory;
// reconciling the two is that same governance verification's.
const REQUIRED: &[&str] = &[
    "gate",
    "pr-title",
    "llmlint",
    "msrv",
    "deny",
    "wheel",
    "cross (macos-latest)",
    "cross (windows-latest)",
    "install (ubuntu-latest)",
    "install (macos-latest)",
    "install (windows-latest)",
    "install-documented (ubuntu-latest)",
    "install-documented (macos-latest)",
    "install-documented (windows-latest)",
    "smoke",
    "release-compat",
    "visual-docs / report (x86_64, shots/current, shots/verify, Visual docs)",
];
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

/// The one context reported by a job of a reusable workflow defined in another
/// repository, whose rendered name this tree cannot compute: the calling job
/// and its pinned workflow are what is held instead.
const VISUAL_DOCS: &str = "visual-docs / report (x86_64, shots/current, shots/verify, Visual docs)";

/// The conditions a required job may carry. Each one either always holds on a
/// pull request or skips the job — and a skipped job reports its context as
/// passed — so none of them can leave a context unreported. A new condition is
/// a new way to be unreported, and has to be argued for here.
const ALLOWED_CONDITIONS: &[&str] = &[
    "needs.changes.outputs.crate == 'true'",
    "needs.changes.outputs.crate == 'true' && github.event_name == 'pull_request'",
    "github.event_name == 'pull_request'",
];

/// `on:` parses as the boolean `true` under YAML 1.1, which serde_norway follows.
fn triggers(parsed: &Value) -> &Value {
    parsed
        .get("on")
        .or_else(|| parsed.get(Value::Bool(true)))
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
        .and_then(Value::as_sequence)
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
                if let Some(include) = matrix.get("include").and_then(Value::as_sequence) {
                    include
                        .iter()
                        .map(|leg| {
                            leg.as_mapping()
                                .expect("an include leg is a mapping")
                                .iter()
                                .map(|(k, v)| (k.as_str().expect("a key").to_owned(), scalar(v)))
                                .collect()
                        })
                        .collect()
                } else {
                    let axes: Vec<(String, Vec<String>)> = matrix
                        .as_mapping()
                        .expect("the matrix is a mapping")
                        .iter()
                        .map(|(k, v)| {
                            let values = v
                                .as_sequence()
                                .expect("a matrix axis is a list")
                                .iter()
                                .map(scalar)
                                .collect();
                            (k.as_str().expect("an axis name").to_owned(), values)
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
        other => serde_norway::to_string(other)
            .expect("a scalar renders")
            .trim()
            .to_owned(),
    }
}

fn needs(job: &Value) -> Vec<String> {
    match job.get("needs") {
        None => Vec::new(),
        Some(Value::String(one)) => vec![one.clone()],
        Some(Value::Sequence(many)) => many
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
        for (key, job) in parsed["jobs"].as_mapping().expect("the workflow has jobs") {
            let key = key.as_str().expect("a job key");
            for name in rendered(key, job) {
                reporters.insert(name, (file.clone(), key.to_owned()));
            }
        }
    }

    for context in REQUIRED.iter().filter(|c| **c != VISUAL_DOCS) {
        let (file, key) = reporters
            .get(*context)
            .unwrap_or_else(|| panic!("no job reports `{context}` on a pull request"));
        let jobs = &all[file]["jobs"];
        let job = &jobs[key.as_str()];
        if let Some(condition) = condition(job) {
            assert!(
                ALLOWED_CONDITIONS.contains(&condition.as_str()),
                "`{context}` ({file} job `{key}`) runs only if `{condition}`, a condition that can leave it unreported"
            );
        }
        // Each job it waits on has to run on every pull request too, or a
        // failure upstream would leave this context waiting rather than skipped.
        for upstream in needs(job) {
            let upstream_job = &jobs[upstream.as_str()];
            assert!(
                !upstream_job.is_null(),
                "`{context}` needs `{upstream}`, which {file} does not define"
            );
            assert_eq!(
                condition(upstream_job),
                None,
                "`{context}` needs `{upstream}`, which runs conditionally"
            );
            assert!(
                needs(upstream_job).is_empty(),
                "`{context}` needs `{upstream}`, which waits on another job in turn"
            );
        }
    }

    let visual = &all["visual-docs.yml"];
    assert!(runs_on_every_pull_request("visual-docs.yml", visual));
    let caller = &visual["jobs"]["visual-docs"];
    assert!(
        caller["uses"]
            .as_str()
            .is_some_and(|uses| uses.contains("visual-docs-reusable.yml@")),
        "`{VISUAL_DOCS}` is reported by the reusable workflow `visual-docs.yml`'s `visual-docs` job calls"
    );
    assert_eq!(
        condition(caller),
        None,
        "the visual-docs caller runs conditionally"
    );
    assert!(
        needs(caller).is_empty(),
        "the visual-docs caller waits on another job"
    );
}

/// The jobs that report a context nobody requires, and that no required job may
/// wait on: the release sweep, the suppressions comment, and the three workflows
/// AGENTS.md records as advisory.
#[test]
fn no_required_job_waits_on_an_advisory_one() {
    let all = workflows();
    for (file, key) in [
        ("release-sweep.yml", "release-sweep"),
        ("notignored.yml", "suppressions"),
    ] {
        let job = &all[file]["jobs"][key];
        assert!(!job.is_null(), "{file} has no `{key}` job");
        for name in rendered(key, job) {
            assert!(
                !REQUIRED.contains(&name.as_str()),
                "{file}'s `{key}` reports `{name}`, a required context"
            );
        }
    }
    // `needs` only reaches jobs of the same workflow, so a required job waits on
    // an advisory one only if the two share a file — which none of these do.
    for (file, parsed) in &all {
        let jobs = parsed["jobs"].as_mapping().expect("the workflow has jobs");
        for (key, job) in jobs {
            let key = key.as_str().expect("a job key");
            if rendered(key, job)
                .iter()
                .any(|name| REQUIRED.contains(&name.as_str()))
            {
                for upstream in needs(job) {
                    let upstream_names = rendered(&upstream, &parsed["jobs"][upstream.as_str()]);
                    assert!(
                        upstream == "changes"
                            || upstream_names.iter().any(|name| REQUIRED.contains(&name.as_str())),
                        "{file}'s required `{key}` waits on `{upstream}`, which is not itself required"
                    );
                }
            }
        }
    }
    for advisory in [
        "release-sweep.yml",
        "notignored.yml",
        "engine-currency.yml",
        "published-smoke.yml",
    ] {
        assert!(
            all.contains_key(advisory),
            "{advisory} is a workflow of its own"
        );
    }
}

/// A GitHub Actions expression, evaluated over the handful of forms the job
/// selection uses: `context.path`, `'literal'`, `==`, `!=`, `&&`, `||`,
/// `startsWith(a, b)` and parentheses. Anything else panics, so a condition
/// rewritten past what this reads fails here rather than evaluating wrongly.
fn evaluate(expression: &str, github: &BTreeMap<&str, &str>) -> bool {
    #[derive(Debug, Clone, PartialEq)]
    enum Token {
        Str(String),
        Ident(String),
        Op(&'static str),
    }
    let mut tokens = Vec::new();
    let chars: Vec<char> = expression.chars().collect();
    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        if c.is_whitespace() {
            at += 1;
        } else if c == '\'' {
            let end = chars[at + 1..]
                .iter()
                .position(|&q| q == '\'')
                .expect("a closed string literal")
                + at
                + 1;
            tokens.push(Token::Str(chars[at + 1..end].iter().collect()));
            at = end + 1;
        } else if let Some(op) = ["==", "!=", "&&", "||"]
            .into_iter()
            .find(|op| chars[at..].iter().take(2).collect::<String>() == *op)
        {
            tokens.push(Token::Op(op));
            at += 2;
        } else if let Some(op) = ["(", ")", ","].into_iter().find(|op| c.to_string() == *op) {
            tokens.push(Token::Op(op));
            at += 1;
        } else if c.is_alphanumeric() || c == '_' {
            let end = chars[at..]
                .iter()
                .position(|&n| !(n.is_alphanumeric() || n == '_' || n == '.' || n == '-'))
                .map_or(chars.len(), |n| n + at);
            tokens.push(Token::Ident(chars[at..end].iter().collect()));
            at = end;
        } else {
            panic!("`{expression}` has a character this evaluator does not read: {c:?}");
        }
    }

    struct Parser<'a> {
        tokens: Vec<Token>,
        at: usize,
        github: &'a BTreeMap<&'a str, &'a str>,
    }
    #[derive(Debug, PartialEq)]
    enum Val {
        Str(String),
        Bool(bool),
    }
    impl Parser<'_> {
        fn peek(&self) -> Option<&Token> {
            self.tokens.get(self.at)
        }
        fn eat(&mut self, op: &'static str) -> bool {
            if self.peek() == Some(&Token::Op(op)) {
                self.at += 1;
                true
            } else {
                false
            }
        }
        fn or(&mut self) -> Val {
            let mut left = self.and();
            while self.eat("||") {
                let right = self.and();
                left = Val::Bool(truthy(&left) || truthy(&right));
            }
            left
        }
        fn and(&mut self) -> Val {
            let mut left = self.compare();
            while self.eat("&&") {
                let right = self.compare();
                left = Val::Bool(truthy(&left) && truthy(&right));
            }
            left
        }
        fn compare(&mut self) -> Val {
            let left = self.primary();
            if self.eat("==") {
                let right = self.primary();
                return Val::Bool(left == right);
            }
            if self.eat("!=") {
                let right = self.primary();
                return Val::Bool(left != right);
            }
            left
        }
        fn primary(&mut self) -> Val {
            let token = self.tokens.get(self.at).cloned().expect("an operand");
            self.at += 1;
            match token {
                Token::Str(s) => Val::Str(s),
                Token::Op("(") => {
                    let inner = self.or();
                    assert!(self.eat(")"), "an unclosed parenthesis");
                    inner
                }
                Token::Ident(name) if name == "startsWith" => {
                    assert!(self.eat("("), "startsWith takes arguments");
                    let haystack = self.or();
                    assert!(self.eat(","), "startsWith takes two arguments");
                    let needle = self.or();
                    assert!(self.eat(")"), "startsWith takes two arguments");
                    match (haystack, needle) {
                        (Val::Str(h), Val::Str(n)) => {
                            Val::Bool(h.to_lowercase().starts_with(&n.to_lowercase()))
                        }
                        other => panic!("startsWith over non-strings: {other:?}"),
                    }
                }
                Token::Ident(path) => {
                    let field = path
                        .strip_prefix("github.")
                        .unwrap_or_else(|| panic!("`{path}` is not a github context field"));
                    Val::Str(self.github.get(field).copied().unwrap_or("").to_owned())
                }
                Token::Op(op) => panic!("an operand was expected, not `{op}`"),
            }
        }
    }
    fn truthy(value: &Val) -> bool {
        match value {
            Val::Bool(b) => *b,
            Val::Str(s) => !s.is_empty(),
        }
    }
    let mut parser = Parser {
        tokens,
        at: 0,
        github,
    };
    let value = parser.or();
    assert_eq!(
        parser.at,
        parser.tokens.len(),
        "`{expression}` has tokens this evaluator did not read"
    );
    truthy(&value)
}

/// Whether a job runs for an event: its `if:` evaluated, or `true` without one.
fn selected(job: &Value, github: &BTreeMap<&str, &str>) -> bool {
    condition(job).is_none_or(|c| evaluate(&c, github))
}

#[test]
fn the_release_pull_request_runs_the_full_sweep_and_every_other_pull_request_the_affected_tier() {
    let sweep_workflow = workflow("release-sweep.yml");
    assert!(
        triggers(&sweep_workflow).get("pull_request").is_some(),
        "the release sweep runs on pull requests"
    );
    let sweep = &sweep_workflow["jobs"]["release-sweep"];
    let gate = &workflow("ci.yml")["jobs"]["gate"];
    let runs = |job: &Value, step_run: &str| {
        job["steps"]
            .as_sequence()
            .expect("the job has steps")
            .iter()
            .filter_map(|step| step.get("run").and_then(Value::as_str))
            .any(|run| run.contains(step_run))
    };
    // What each job is: the sweep runs `just check` — `run-many` over every
    // gate-eligible project — and the gate `just check-affected`.
    assert!(
        runs(sweep, "just check 2>&1"),
        "the release sweep runs `just check`"
    );
    assert!(
        runs(gate, "just check-affected 2>&1"),
        "the gate runs `just check-affected`"
    );
    assert!(
        !runs(gate, "just check 2>&1"),
        "the gate also runs the full sweep"
    );

    // release-plz opens its release pull request from a `release-plz-<timestamp>`
    // branch; anything else is an ordinary pull request.
    let release = BTreeMap::from([
        ("event_name", "pull_request"),
        ("head_ref", "release-plz-2026-10-06T12-00-00Z"),
        ("base_ref", "main"),
    ]);
    let ordinary = BTreeMap::from([
        ("event_name", "pull_request"),
        ("head_ref", "nick/some-feature"),
        ("base_ref", "main"),
    ]);
    let push = BTreeMap::from([("event_name", "push"), ("ref", "refs/heads/main")]);

    assert!(
        selected(sweep, &release),
        "the release PR does not select the sweep"
    );
    assert!(
        !selected(sweep, &ordinary),
        "an ordinary PR selects the sweep"
    );
    assert!(!selected(sweep, &push), "a push selects the sweep");
    for (what, event) in [
        ("release PR", &release),
        ("ordinary PR", &ordinary),
        ("push", &push),
    ] {
        assert!(
            selected(gate, event),
            "the affected-tier gate does not run on a {what}"
        );
    }
}

/// The `just check` the sweep runs reaches every gate-eligible project: each
/// one's `check` is what `run-many -t check` runs, and the live and network
/// tiers' `check` stops at format and lint, never their journey.
#[test]
fn the_sweep_reaches_every_project_but_the_live_tiers_journeys() {
    let justfile = fs::read_to_string(repo_root().join("justfile")).expect("the justfile reads");
    let check = justfile
        .split("\ncheck: ")
        .nth(1)
        .expect("the justfile has `check`");
    assert!(
        check
            .lines()
            .take(3)
            .any(|line| line.contains("scripts/nx.sh run-many -t check")),
        "`check` is not a run-many sweep:\n{check}"
    );
    for (path, journey) in [
        ("tests/smoke/project.json", "smoke"),
        ("tests/release_channel/project.json", "release-compat"),
    ] {
        let project: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(repo_root().join(path)).expect("the project reads"),
        )
        .expect("the project parses");
        let reaches = project["targets"]["check"]["dependsOn"].to_string();
        assert!(
            !reaches.contains(&format!("\"{journey}\"")) && !reaches.contains("\"test\""),
            "{path}'s `check` reaches its live journey: {reaches}"
        );
    }
}
