//! The host-shaped workload: `onevcs_testing::recovery::HOST`, built from an empty
//! root, every branch's decision recorded by one host read, then aged by `age_host`
//! — the measured session's branches moved and six bases advanced, landing some
//! recorded branches.
//!
//! Every row is checked against the class the fixture **declares** for it, never
//! against another read. Every timed call starts from the aged state: before it, and
//! outside its timing, `AgedHost::restore` puts `ONEVCS_HOME` back and
//! `AgedHost::verify` confirms the scenario, and the acknowledgements file is put back
//! to its own bytes; after it, and outside its timing, what it answered is asserted.
//!
//! The measured session is the fixture's launcher, the key `onepipeline` selects a
//! session's rows by, so its listing is every launcher-labelled row of the host: the
//! moved branches among them, and both branches changing 30,000 paths.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use onevcs_testing::recovery::{age_host, AgedHost, Class, Fixture, HOST};
use serde_json::{json, Value};

use super::telemetry::{self, digest, HostRead, HostWorkload, Sample};
use super::{assemble_runs, binary, install_shim, load1, ms, path, Host, CALLS};

/// The baseline acknowledgements: the fixture's ten acknowledged branches.
const ACKNOWLEDGED: usize = 10;
const REASON: &str = "kept for the host-shaped budget";
const AT: &str = "2026-11-08T00:00:00Z";
const PHASES: &str = "ONEPIPELINE_HOST_PHASES";
const DIAGNOSTICS: &str = "ONEVCS_DECISION_DIAGNOSTICS";
const LAUNCHER_SESSION: &str = "ONEPIPELINE_LAUNCHER_SESSION";

/// What a row must read as, from the fixture's declaration alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Standing {
    /// Counted: nothing says it landed.
    Owed,
    /// Counted: the same subject reached the base with other content.
    Unknown,
    /// Counted: a landing predates further work.
    InPart,
    /// Held by a live session.
    InFlight,
    /// Acknowledged at its tip by the measured session.
    Acknowledged,
    /// Landed, by the fixture's own landing or by a base gain `age_host` made: onevcs
    /// withholds it, so it is on no row and counts nowhere.
    Landed,
}

impl Standing {
    fn name(self) -> &'static str {
        match self {
            Self::Owed => "owed",
            Self::Unknown => "unknown",
            Self::InPart => "in-part",
            Self::InFlight => "in-flight",
            Self::Acknowledged => "acknowledged",
            Self::Landed => "landed",
        }
    }

    fn counted(self) -> bool {
        matches!(self, Self::Owed | Self::Unknown | Self::InPart)
    }
}

/// One declared row.
#[derive(Debug, Clone)]
struct Declared {
    identity: String,
    branch: String,
    tip: String,
    session: String,
    standing: Standing,
    /// Its `landed.state`.
    state: &'static str,
}

/// The aged host and what the journey keeps beside it.
struct Aged {
    host: Host,
    aged: AgedHost,
    declared: Vec<Declared>,
    /// The acknowledgements directory the host and owed scenarios read, and its file's
    /// bytes, restored before every call.
    acknowledgements: PathBuf,
    file: PathBuf,
    baseline: Vec<u8>,
    trace: PathBuf,
}

fn key(identity: &str, branch: &str) -> (String, String) {
    (identity.to_owned(), branch.to_owned())
}

fn entry(identity: &str, branch: &str, tip: &str) -> Value {
    json!({"branch": branch, "identity": identity, "tip": tip, "reason": REASON, "at": AT})
}

/// The rows the fixture declares once aged, with `acknowledged` standing at their tips.
fn declare(
    fixture: &Fixture,
    aged: &AgedHost,
    acknowledged: &BTreeSet<(String, String)>,
) -> Vec<Declared> {
    let moved: BTreeMap<(String, String), String> = aged
        .moved
        .iter()
        .map(|(identity, branch, tip)| (key(identity, branch), tip.as_str().to_owned()))
        .collect();
    let landed: BTreeSet<(String, String)> = aged
        .landed
        .iter()
        .map(|landed| key(&landed.identity, &landed.branch))
        .collect();
    fixture
        .expected
        .iter()
        .map(|expected| {
            let at = key(&expected.identity, &expected.branch);
            let state = match expected.class {
                Class::Unknown => "unknown",
                Class::InPart => "in-part",
                Class::Landed | Class::Retirable => "yes",
                Class::Superseded | Class::Live | Class::No => "no",
            };
            let state = if landed.contains(&at) { "yes" } else { state };
            let standing = if landed.contains(&at)
                || matches!(expected.class, Class::Landed | Class::Retirable)
            {
                Standing::Landed
            } else if expected.class == Class::Live {
                Standing::InFlight
            } else if acknowledged.contains(&at) {
                Standing::Acknowledged
            } else {
                match expected.class {
                    Class::Unknown => Standing::Unknown,
                    Class::InPart => Standing::InPart,
                    _ => Standing::Owed,
                }
            };
            Declared {
                tip: moved
                    .get(&at)
                    .cloned()
                    .unwrap_or_else(|| expected.tip.as_str().to_owned()),
                identity: expected.identity.clone(),
                branch: expected.branch.to_string(),
                session: expected.session.to_string(),
                standing,
                state,
            }
        })
        .collect()
}

/// The fields `onepipeline unpublished`'s document type declares, and each row's.
const DOCUMENT_FIELDS: [&str; 4] = ["rows", "target", "unresolved", "verdict"];
const ROW_FIELDS: [&str; 19] = [
    "acknowledgement",
    "base",
    "branch",
    "change_url",
    "counted",
    "disk",
    "identity",
    "in_flight",
    "land_command",
    "landed",
    "manager_session",
    "node",
    "provenance",
    "recover_command",
    "retirement",
    "run",
    "session",
    "stopped_because",
    "tip",
];

fn keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

/// Check one JSON listing row by row against the declaration, `acknowledged` naming the
/// entries standing, and answer its rows' digest. A row the fixture declares unanswered
/// (`unknown`) is named in `unknown` instead.
fn check_document(
    document: &Value,
    declared: &[Declared],
    acknowledged: &BTreeMap<(String, String), Value>,
    launcher: &str,
    verdict: &str,
) -> String {
    assert_eq!(keys(document), DOCUMENT_FIELDS, "the document's fields");
    assert_eq!(document["verdict"], verdict, "{}", document["unresolved"]);
    assert_eq!(document["unresolved"], json!([]), "nothing is unresolved");
    let rows = document["rows"].as_array().expect("rows");
    let mut seen = BTreeSet::new();
    for row in rows {
        assert_eq!(keys(row), ROW_FIELDS, "a row's fields: {row:#}");
        let at = key(
            row["identity"].as_str().expect("an identity"),
            row["branch"].as_str().expect("a branch"),
        );
        assert!(seen.insert(at.clone()), "{at:?} is listed twice");
        let declared = declared
            .iter()
            .find(|declared| key(&declared.identity, &declared.branch) == at)
            .unwrap_or_else(|| panic!("{at:?} is listed but not declared: {row:#}"));
        check_row(row, declared, acknowledged.get(&at), launcher);
    }
    for declared in declared {
        assert_eq!(
            seen.contains(&key(&declared.identity, &declared.branch)),
            declared.standing != Standing::Landed,
            "{} {} ({}) is listed only where it has not landed",
            declared.identity,
            declared.branch,
            declared.standing.name()
        );
    }
    digest(
        serde_json::to_string(&json!([document["verdict"], document["rows"]]))
            .expect("rows")
            .as_bytes(),
    )
}

fn check_row(row: &Value, declared: &Declared, entry: Option<&Value>, launcher: &str) {
    let standing = match (declared.standing, entry) {
        (Standing::Owed | Standing::Unknown | Standing::InPart, Some(_)) => Standing::Acknowledged,
        (standing, _) => standing,
    };
    let what = format!(
        "{} {} ({}): {row:#}",
        declared.identity,
        declared.branch,
        standing.name()
    );
    assert_eq!(row["tip"], declared.tip.as_str(), "{what}");
    assert_eq!(row["session"], declared.session.as_str(), "{what}");
    assert_eq!(row["manager_session"], launcher, "{what}");
    assert_eq!(row["in_flight"], standing == Standing::InFlight, "{what}");
    assert_eq!(row["counted"], standing.counted(), "{what}");
    assert_eq!(row["landed"]["state"], declared.state, "{what}");
    let recover: Vec<&str> = row["recover_command"]
        .as_array()
        .expect("a recovery command")
        .iter()
        .map(|word| word.as_str().expect("a word"))
        .collect();
    assert_eq!(recover.first(), Some(&"onevcs"), "{what}");
    assert!(recover.contains(&declared.branch.as_str()), "{what}");
    match (standing, entry) {
        (Standing::Acknowledged, Some(entry)) => {
            assert_eq!(&row["acknowledgement"], entry, "{what}");
        }
        _ => assert_eq!(row["acknowledgement"], Value::Null, "{what}"),
    }
}

impl Aged {
    fn identity_of(&self, branch: &str) -> &Declared {
        self.declared
            .iter()
            .find(|declared| declared.branch == branch)
            .expect("a declared branch")
    }

    fn entries(&self, bytes: &[u8]) -> BTreeMap<(String, String), Value> {
        let file: Value = serde_json::from_slice(bytes).expect("an acknowledgement file");
        file["acknowledged"]
            .as_array()
            .expect("entries")
            .iter()
            .map(|entry| {
                (
                    key(
                        entry["identity"].as_str().expect("identity"),
                        entry["branch"].as_str().expect("branch"),
                    ),
                    entry.clone(),
                )
            })
            .collect()
    }

    /// The aged state back, confirmed, and the acknowledgements as they were.
    fn restore(&self) {
        self.aged.restore().expect("the aged host restores");
        self.aged
            .verify()
            .expect("the aged host is the declared scenario");
        std::fs::write(&self.file, &self.baseline).expect("the acknowledgements restore");
    }

    fn command(&self, traced: bool) -> Command {
        let mut command = Command::new(binary().0);
        command
            .env_clear()
            .env("PATH", path())
            .env("HOME", &self.host.fixture.root)
            .env("ONEVCS_HOME", &self.host.fixture.home)
            .env(LAUNCHER_SESSION, &self.host.fixture.launcher)
            .current_dir(&self.host.fixture.root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if traced {
            let mut shimmed = std::ffi::OsString::from(&self.host.shim);
            shimmed.push(":");
            shimmed.push(path());
            command
                .env("PATH", shimmed)
                .env(PHASES, self.trace.join("phases"))
                .env(DIAGNOSTICS, self.trace.join("diagnostics"));
        }
        command
    }

    fn host_listing(&self, traced: bool) -> (u64, i32, Value) {
        let mut command = self.command(traced);
        command
            .args([
                "unpublished",
                "--host",
                "--format",
                "json",
                "--acknowledgements",
            ])
            .arg(&self.acknowledgements);
        let started = Instant::now();
        let output = command.output().expect("the host listing runs");
        let took = micros(started);
        let document = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "no listing ({error}): {}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        (took, output.status.code().expect("an exit code"), document)
    }

    fn acknowledge(&self, branch: &str, traced: bool) -> (u64, i32, String) {
        let mut command = self.command(traced);
        command
            .args(["unpublished", "--acknowledge", branch, "--reason", REASON])
            .args([
                "--session",
                &self.host.fixture.launcher,
                "--acknowledgements",
            ])
            .arg(&self.acknowledgements);
        let started = Instant::now();
        let output = command.output().expect("the acknowledgement runs");
        let took = micros(started);
        (
            took,
            output.status.code().expect("an exit code"),
            String::from_utf8(output.stdout).expect("text"),
        )
    }

    /// The traced call's phases and derivations, read back and cleared.
    fn traced(&self) -> (BTreeMap<String, f64>, u64, u64) {
        let line = std::fs::read_to_string(self.trace.join("phases")).expect("a phase line");
        let line: Value = serde_json::from_str(line.trim()).expect("one phase line");
        let mut phases: BTreeMap<String, f64> = line["phases"]
            .as_object()
            .expect("phases")
            .iter()
            .map(|(name, phase)| (name.clone(), phase["wall_ms"].as_f64().expect("wall")))
            .collect();
        for (name, amount) in line["quantities"].as_object().expect("quantities") {
            if let Some(name) = name.strip_suffix("-us") {
                phases.insert(
                    name.to_owned(),
                    amount.as_f64().expect("an amount") / 1000.0,
                );
            }
        }
        phases.insert("whole".to_owned(), line["wall_ms"].as_f64().expect("wall"));
        let derived = std::fs::read_to_string(self.trace.join("diagnostics"))
            .unwrap_or_default()
            .lines()
            .map(|line| {
                serde_json::from_str::<Value>(line).expect("a diagnostics line")["derived"]
                    .as_array()
                    .expect("derived")
                    .len() as u64
            })
            .sum();
        let git = std::fs::read_to_string(&self.host.counted)
            .map(|text| text.lines().count() as u64)
            .unwrap_or(0);
        for file in ["phases", "diagnostics"] {
            let _ = std::fs::remove_file(self.trace.join(file));
        }
        let _ = std::fs::remove_file(&self.host.counted);
        (phases, derived, git)
    }
}

fn micros(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros())
        .expect("fits")
        .max(1)
}

fn sample(calls: Vec<u64>, load: f64) -> Sample {
    let mut sorted = calls.clone();
    sorted.sort_unstable();
    Sample {
        median_us: (sorted[CALLS / 2 - 1] + sorted[CALLS / 2]) / 2,
        max_us: sorted[CALLS - 1],
        calls_us: calls,
        load1: load,
    }
}

/// The host listing: checked cold and warm, then timed from the aged state.
fn host_listing(aged: &Aged, cold_us: u64) -> HostRead {
    let entries = aged.entries(&aged.baseline);
    let launcher = aged.host.fixture.launcher.clone();
    let load = load1();
    let mut calls = Vec::new();
    let mut answer = None;
    for _ in 0..CALLS {
        aged.restore();
        let (took, code, document) = aged.host_listing(false);
        assert_eq!(code, 7, "the host owes its counted branches");
        let digest = check_document(&document, &aged.declared, &entries, &launcher, "owed");
        assert!(
            answer
                .replace(digest.clone())
                .is_none_or(|was| was == digest),
            "an unchanged aged state answered differently"
        );
        calls.push(took);
    }
    aged.restore();
    aged.host_listing(true);
    let (phases_ms, rederived, git) = aged.traced();
    HostRead {
        read: "unpublished-host".into(),
        verdict_sha256: answer.expect("an answer"),
        warm: sample(calls, load),
        cold_us: Some(cold_us),
        phases_ms,
        rederived,
        git,
    }
}

/// The receipt, the file and the listing printed after one acknowledgement, against
/// what the fixture declares and what the listing printed before it.
fn check_acknowledged(aged: &Aged, branch: &str, code: i32, stdout: &str, before: &[String]) {
    let declared = aged.identity_of(branch);
    let file = std::fs::read(&aged.file).expect("the acknowledgement file");
    let mut entries = aged.entries(&file);
    let recorded = entries
        .remove(&key(&declared.identity, branch))
        .expect("the branch is recorded in the identity holding it");
    assert_eq!(recorded["tip"], declared.tip.as_str());
    assert_eq!(recorded["reason"], REASON);
    assert_eq!(
        entries,
        aged.entries(&aged.baseline),
        "exactly that branch was recorded"
    );
    let receipt = format!(
        "acknowledged {branch} [{}] at {}: {REASON} ({})",
        declared.identity,
        &declared.tip[..12],
        aged.file.display()
    );
    let mut lines = stdout.lines();
    assert_eq!(lines.next(), Some(receipt.as_str()), "{stdout}");
    let after = row_lines(lines.collect::<Vec<_>>().as_slice());
    assert_eq!(after.len(), before.len(), "{stdout}");
    for (was, now) in before.iter().zip(&after) {
        if was.starts_with(&format!("{branch}  [")) {
            assert!(now.starts_with(&format!("{branch}  [{}]", declared.identity)));
            assert!(
                now.ends_with(&format!("acknowledged: {REASON}")),
                "{branch} reads acknowledged and uncounted: {now}"
            );
        } else {
            assert_eq!(now, was, "another row changed");
        }
    }
    let header = format!("{branch} [{}]", declared.identity);
    assert!(
        !stdout
            .lines()
            .any(|line| line == header || line.starts_with(&format!("{header} — "))),
        "{branch} is still printed as counted: {stdout}"
    );
    let still = aged
        .declared
        .iter()
        .filter(|row| row.standing.counted() && row.branch != branch)
        .count();
    assert_eq!(code, if still > 0 { 7 } else { 0 });
}

/// The per-row lines a text listing opens with.
fn row_lines(lines: &[&str]) -> Vec<String> {
    lines
        .iter()
        .take_while(|line| !line.is_empty())
        .map(|line| (*line).to_owned())
        .collect()
}

/// Ten acknowledgements, each of a different counted branch, one changing 30,000 paths;
/// `before` is the session listing's rows as the aged state prints them.
fn acknowledgements(aged: &Aged, branches: &[String], before: &[String]) -> HostRead {
    let load = load1();
    let mut calls = Vec::new();
    for branch in branches {
        aged.restore();
        let (took, code, stdout) = aged.acknowledge(branch, false);
        check_acknowledged(aged, branch, code, &stdout, before);
        calls.push(took);
    }
    aged.restore();
    let (_, code, stdout) = aged.acknowledge(&branches[0], true);
    check_acknowledged(aged, &branches[0], code, &stdout, before);
    let (phases_ms, rederived, git) = aged.traced();
    HostRead {
        read: "acknowledge".into(),
        verdict_sha256: digest(branches.join("\n").as_bytes()),
        warm: sample(calls, load),
        cold_us: None,
        phases_ms,
        rederived,
        git,
    }
}

fn session_text(aged: &Aged) -> (i32, String) {
    let mut command = aged.command(false);
    command
        .args(["unpublished", "--session", &aged.host.fixture.launcher])
        .arg("--acknowledgements")
        .arg(&aged.acknowledgements);
    let output = command.output().expect("the session listing runs");
    (
        output.status.code().expect("an exit code"),
        String::from_utf8(output.stdout).expect("text"),
    )
}

/// The human listing: one line per owed branch with its recovery command, none for
/// the others, and the count line's totals as declared; answers its row lines.
fn check_text(aged: &Aged) -> Vec<String> {
    let (code, text) = session_text(aged);
    assert_eq!(code, 7, "{text}");
    let entries = aged.entries(&aged.baseline);
    let standing = |declared: &Declared| {
        if declared.standing.counted()
            && entries.contains_key(&key(&declared.identity, &declared.branch))
        {
            Standing::Acknowledged
        } else {
            declared.standing
        }
    };
    let lines: Vec<&str> = text.lines().collect();
    let listed = row_lines(&lines);
    let mut counted = 0;
    let mut in_flight = 0;
    let mut acknowledged = 0;
    for declared in &aged.declared {
        let header = format!("{} [{}]", declared.branch, declared.identity);
        let at = lines
            .iter()
            .position(|line| *line == header || line.starts_with(&format!("{header} — ")));
        match standing(declared) {
            standing if standing.counted() => {
                counted += 1;
                let at = at.unwrap_or_else(|| panic!("{header} is not printed: {text}"));
                let land = lines[at + 1..]
                    .iter()
                    .take_while(|line| line.starts_with("    "))
                    .find(|line| line.starts_with("    land it:"))
                    .unwrap_or_else(|| panic!("{header} prints no recovery command: {text}"));
                assert!(land.contains(&declared.branch), "{land}");
            }
            standing => {
                assert!(
                    at.is_none(),
                    "{header} ({}) is printed as owed",
                    standing.name()
                );
                in_flight += usize::from(standing == Standing::InFlight);
                acknowledged += usize::from(standing == Standing::Acknowledged);
            }
        }
    }
    assert_eq!(acknowledged, ACKNOWLEDGED);
    assert_eq!(in_flight, HOST.in_flight);
    let count = format!(
        "owed: {counted} counted of {} preserved unpublished branch(es); {in_flight} in flight, \
         {acknowledged} acknowledged",
        listed.len()
    );
    assert!(
        lines.last().is_some_and(|line| line.starts_with(&count)),
        "{count}: {text}"
    );
    listed
}

/// The guard's three scenarios at the host's shape.
fn stop_guard(aged: &Aged, every: &Path) -> Vec<HostRead> {
    let registry = aged.host.fixture.home.join("registry.json");
    let counted: Vec<&Declared> = aged
        .declared
        .iter()
        .filter(|declared| {
            declared.standing.counted()
                && !aged
                    .entries(&aged.baseline)
                    .contains_key(&key(&declared.identity, &declared.branch))
        })
        .collect();
    let check = |scenario: &str, stdout: &str| match scenario {
        "none" => assert_eq!(stdout, "", "none answers silence"),
        "owed" => {
            let decision: Value = serde_json::from_str(stdout.trim()).expect("a decision");
            assert_eq!(decision["decision"], "block");
            let reason = decision["reason"].as_str().expect("a reason");
            for declared in &counted {
                assert!(
                    reason.contains(&format!("{} [", declared.branch)),
                    "{} is not named: {reason}",
                    declared.branch
                );
            }
            assert!(
                !reason.contains("settled-"),
                "an own run is reported: {reason}"
            );
        }
        "unreadable" => {
            let decision: Value = serde_json::from_str(stdout.trim()).expect("a decision");
            assert_eq!(decision["decision"], "block");
            let reason = decision["reason"].as_str().expect("a reason");
            assert!(reason.contains("is unanswered"), "{reason}");
            assert!(reason.contains("the recovery read failed"), "{reason}");
        }
        other => panic!("no scenario {other}"),
    };
    telemetry::SCENARIOS
        .iter()
        .map(|&scenario| {
            let acknowledgements = if scenario == "none" {
                every
            } else {
                aged.acknowledgements.as_path()
            };
            let prepare = || {
                aged.restore();
                if scenario == "unreadable" {
                    std::fs::write(&registry, "not a registry").expect("unreadable");
                }
            };
            let load = load1();
            let mut calls = Vec::new();
            let mut answer: Option<String> = None;
            for _ in 0..CALLS {
                prepare();
                let (took, stdout) = aged.host.call(acknowledgements, &path());
                check(scenario, &stdout);
                if let Some(answer) = &answer {
                    assert_eq!(
                        &stdout, answer,
                        "an unchanged aged state answered differently"
                    );
                }
                answer = Some(stdout);
                calls.push(u64::try_from(took.as_micros()).expect("fits"));
            }
            prepare();
            let mut shimmed = std::ffi::OsString::from(&aged.host.shim);
            shimmed.push(":");
            shimmed.push(path());
            let mut command = aged.host.guard(acknowledgements, &shimmed);
            command
                .env(PHASES, aged.trace.join("phases"))
                .env(DIAGNOSTICS, aged.trace.join("diagnostics"));
            let mut child = command.spawn().expect("the guard starts");
            {
                use std::io::Write;
                child
                    .stdin
                    .take()
                    .expect("stdin")
                    .write_all(aged.host.payload().as_bytes())
                    .expect("the payload");
            }
            let output = child.wait_with_output().expect("the guard runs");
            let answer = answer.expect("an answer");
            assert_eq!(String::from_utf8_lossy(&output.stdout), answer.as_str());
            let (phases_ms, rederived, git) = aged.traced();
            HostRead {
                read: format!("stop-guard-{scenario}"),
                verdict_sha256: digest(answer.as_bytes()),
                warm: sample(calls, load),
                cold_us: None,
                phases_ms,
                rederived,
                git,
            }
        })
        .collect()
}

/// After an acknowledgement, over the aged state: the warm host listing answers what a
/// cold one does, and the branches `age_host` landed read landed, each gain's and no
/// other.
fn warm_is_cold(aged: &Aged, branch: &str) {
    aged.restore();
    let (_, code, _) = aged.acknowledge(branch, false);
    assert_eq!(code, 7);
    let (_, warm_code, warm) = aged.host_listing(false);
    let acknowledged = std::fs::read(&aged.file).expect("the file");
    aged.restore();
    std::fs::write(&aged.file, &acknowledged).expect("the acknowledgement stands");
    std::fs::remove_dir_all(aged.host.fixture.home.join("decisions")).expect("no record");
    aged.host.clear_caches();
    let (_, cold_code, cold) = aged.host_listing(false);
    assert_eq!(warm_code, cold_code);
    assert_eq!(warm["verdict"], cold["verdict"]);
    assert_eq!(
        warm["rows"], cold["rows"],
        "a warm read answers what a cold one does"
    );
    let entries = aged.entries(&acknowledged);
    check_document(
        &cold,
        &aged.declared,
        &entries,
        &aged.host.fixture.launcher,
        "owed",
    );
    // A landed branch is withheld: in each identity whose base gained, exactly the
    // branches its gain landed are missing from the preserved ones, and no other.
    let listed: BTreeSet<(String, String)> = cold["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| {
            key(
                row["identity"].as_str().expect("identity"),
                row["branch"].as_str().expect("branch"),
            )
        })
        .collect();
    let mut gained = BTreeMap::new();
    for (at, gain) in HOST.base_gains.iter().enumerate() {
        let identity = aged
            .declared
            .iter()
            .find(|declared| declared.branch == format!("preserved/i{at}-0"))
            .expect("the identity's first branch")
            .identity
            .clone();
        let missing: BTreeSet<String> = aged
            .declared
            .iter()
            .filter(|declared| {
                declared.identity == identity && declared.branch.starts_with("preserved/")
            })
            .filter(|declared| !listed.contains(&key(&declared.identity, &declared.branch)))
            .map(|declared| declared.branch.clone())
            .collect();
        let landed: BTreeSet<String> = aged
            .aged
            .landed
            .iter()
            .filter(|landed| landed.identity == identity)
            .map(|landed| landed.branch.to_string())
            .collect();
        assert_eq!(
            missing, landed,
            "the {}-commit gain lands {:?}",
            gain.commits, landed
        );
        assert_eq!(
            landed.len(),
            gain.landing,
            "the {}-commit gain",
            gain.commits
        );
        gained.insert(gain.commits, landed.len());
    }
    assert_eq!(
        gained,
        BTreeMap::from([(3, 0), (8, 0), (25, 1), (40, 0), (300, 3), (600, 2)]),
        "the 25-, 300- and 600-commit gains land 1, 3 and 2 recorded branches, the others none"
    );
}

/// One branch whose comparison git refuses: the host listing answers it `unknown` with
/// every other row present, the guard names it as an undecidable branch, and another
/// branch is still acknowledged.
fn refused_comparison(aged: &Aged, other: &str) {
    const REFUSED: &str = "preserved/i9-0";
    aged.restore();
    let declared = aged.identity_of(REFUSED).clone();
    let repo = aged.host.fixture.root.join("checkouts/h9");
    let git = |args: &[&str], input: Option<&str>| -> String {
        let mut child = Command::new("git")
            .args(args)
            .current_dir(&repo)
            .env(
                "GIT_CONFIG_GLOBAL",
                aged.host.fixture.root.join(".gitconfig"),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("git runs");
        {
            use std::io::Write;
            let mut stdin = child.stdin.take().expect("stdin");
            if let Some(input) = input {
                stdin.write_all(input.as_bytes()).expect("git input");
            }
        }
        let output = child.wait_with_output().expect("git runs");
        assert!(output.status.success(), "git {args:?}");
        String::from_utf8(output.stdout)
            .expect("text")
            .trim()
            .to_owned()
    };
    // A further commit whose tree git cannot read: the branch moved, so onevcs derives
    // it again, and its comparison with the base is refused.
    let blob = git(
        &["hash-object", "-w", "--stdin"],
        Some("a comparison git refuses\n"),
    );
    let listing = git(&["ls-tree", &declared.tip], None);
    let tree = git(
        &["mktree"],
        Some(&format!("{listing}\n100644 blob {blob}\trefused.txt\n")),
    );
    let tip = git(
        &[
            "commit-tree",
            &tree,
            "-p",
            &declared.tip,
            "-m",
            "feat: refused",
        ],
        None,
    );
    git(
        &["update-ref", &format!("refs/heads/{REFUSED}"), &tip],
        None,
    );
    let object = repo.join(".git/objects").join(&tree[..2]).join(&tree[2..]);
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&object, std::fs::Permissions::from_mode(0o000)).expect("unreadable");
    assert!(
        std::fs::File::open(&object).is_err(),
        "the tree object must be unreadable; run this journey as an ordinary user"
    );

    let mut refused = aged.declared.clone();
    for row in &mut refused {
        if row.branch == REFUSED {
            row.tip = tip.clone();
            row.state = "unknown";
            row.standing = Standing::Unknown;
        }
    }
    let entries = aged.entries(&aged.baseline);
    let launcher = aged.host.fixture.launcher.clone();
    let (_, code, document) = aged.host_listing(false);
    assert_eq!(code, 7);
    check_document(&document, &refused, &entries, &launcher, "owed");
    let row = document["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["branch"] == REFUSED)
        .expect("the refused branch is listed");
    let why = row["stopped_because"].as_str().expect("why");
    assert!(why.contains("could not be decided"), "{why}");

    // The guard names it as it names any branch whose landing cannot be decided.
    let (_, stdout) = aged.host.call(&aged.acknowledgements, &path());
    let decision: Value = serde_json::from_str(stdout.trim()).expect("a decision");
    assert_eq!(decision["decision"], "block");
    let reason = decision["reason"].as_str().expect("a reason");
    let line = format!("{REFUSED}  [{}]  landed unknown", declared.identity);
    assert!(reason.contains(&line), "{line}: {reason}");
    assert!(reason.contains(&format!("{REFUSED} [{}] — {why}", declared.identity)));

    // And another branch is still acknowledged, exactly as before.
    let (code, text) = session_text(aged);
    assert_eq!(code, 7);
    let before = row_lines(text.lines().collect::<Vec<_>>().as_slice());
    let (_, code, stdout) = aged.acknowledge(other, false);
    check_acknowledged(aged, other, code, &stdout, &before);

    std::fs::set_permissions(&object, std::fs::Permissions::from_mode(0o444)).expect("readable");
    git(
        &[
            "update-ref",
            &format!("refs/heads/{REFUSED}"),
            &declared.tip,
        ],
        None,
    );
}

/// The host-shaped workload end to end: its fixture, built from empty, decided, aged,
/// validated, timed, counted, removed.
pub(super) fn workload(root: PathBuf, fixture: Fixture, preparation_ms: &mut u64) -> HostWorkload {
    let preparation = Instant::now();
    let runs = root.join("runs");
    assemble_runs(&runs, &fixture.launcher);
    let host = Host {
        runs,
        state: root.join("state"),
        owed: root.join("acknowledged-ten"),
        none: root.join("acknowledged-every-counted"),
        shim: root.join("shim"),
        counted: root.join("git-calls"),
        fixture,
    };
    install_shim(&host.shim, &host.counted);
    host.refresh_runs();
    let trace = root.join("trace");
    std::fs::create_dir_all(&trace).expect("a trace directory");
    std::fs::create_dir_all(&host.owed).expect("an acknowledgements directory");
    *preparation_ms += ms(preparation);

    // The cold first read records every branch's decision.
    let cold = {
        let started = Instant::now();
        let output = Command::new(binary().0)
            .args([
                "unpublished",
                "--host",
                "--format",
                "json",
                "--acknowledgements",
            ])
            .arg(&host.owed)
            .env_clear()
            .env("PATH", path())
            .env("HOME", &host.fixture.root)
            .env("ONEVCS_HOME", &host.fixture.home)
            .current_dir(&host.fixture.root)
            .output()
            .expect("the cold read runs");
        let took = micros(started);
        assert_eq!(
            output.status.code(),
            Some(7),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        took
    };
    let aging = Instant::now();
    let aged = age_host(&host.fixture).expect("the host ages");
    aged.verify()
        .expect("the aged host is the declared scenario");
    *preparation_ms += ms(aging);
    assert_eq!(aged.moved.len(), HOST.session_branches);

    // Ten acknowledged branches, owed and neither moved nor landed by the aging.
    let unaged = declare(&host.fixture, &aged, &BTreeSet::new());
    let moved: BTreeSet<&str> = aged.moved.iter().map(|(_, branch, _)| &**branch).collect();
    let owed: Vec<&Declared> = unaged
        .iter()
        .filter(|declared| declared.standing == Standing::Owed)
        .collect();
    let chosen: Vec<&Declared> = owed
        .iter()
        .filter(|declared| {
            !moved.contains(declared.branch.as_str())
                && !declared.branch.starts_with("preserved/i7-")
        })
        .take(ACKNOWLEDGED)
        .copied()
        .collect();
    assert_eq!(chosen.len(), ACKNOWLEDGED);
    let file = host
        .owed
        .join(format!("{}.json", digest(host.fixture.launcher.as_bytes())));
    let baseline = json!({
        "version": 1,
        "acknowledged": chosen
            .iter()
            .map(|declared| entry(&declared.identity, &declared.branch, &declared.tip))
            .collect::<Vec<_>>(),
    })
    .to_string()
    .into_bytes();
    std::fs::write(&file, &baseline).expect("the acknowledgements");
    let acknowledged: BTreeSet<(String, String)> = chosen
        .iter()
        .map(|declared| key(&declared.identity, &declared.branch))
        .collect();
    // The none scenario's: every counted branch, at its aged tip.
    std::fs::create_dir_all(&host.none).expect("a directory");
    std::fs::write(
        host.none
            .join(format!("{}.json", digest(host.fixture.launcher.as_bytes()))),
        json!({
            "version": 1,
            "acknowledged": unaged
                .iter()
                .filter(|declared| declared.standing.counted())
                .map(|declared| entry(&declared.identity, &declared.branch, &declared.tip))
                .collect::<Vec<_>>(),
        })
        .to_string(),
    )
    .expect("every counted branch acknowledged");
    // The ten acknowledged: the branch whose comparison reaches git, then nine others.
    let mut samples: Vec<String> = vec!["preserved/i7-0".to_owned()];
    samples.extend(
        owed.iter()
            .filter(|declared| {
                !acknowledged.contains(&key(&declared.identity, &declared.branch))
                    && declared.branch != "preserved/i7-0"
            })
            .map(|declared| declared.branch.clone())
            .take(CALLS - 1),
    );
    assert_eq!(samples.len(), CALLS);
    let none = host.none.clone();
    let declared = declare(&host.fixture, &aged, &acknowledged);
    let mut class_counts = BTreeMap::new();
    for declared in &declared {
        *class_counts
            .entry(declared.standing.name().to_owned())
            .or_insert(0) += 1;
    }
    let aged = Aged {
        acknowledgements: host.owed.clone(),
        host,
        aged,
        declared,
        file,
        baseline,
        trace,
    };

    // Every answer established before any timing counts.
    aged.restore();
    let (code, document) = aged.host.listing(&aged.acknowledgements);
    assert_eq!(code, 7);
    let entries = aged.entries(&aged.baseline);
    let launcher = aged.host.fixture.launcher.clone();
    check_document(&document, &aged.declared, &entries, &launcher, "owed");
    aged.restore();
    let before = check_text(&aged);
    warm_is_cold(&aged, &samples[1]);

    let mut reads = vec![
        host_listing(&aged, cold),
        acknowledgements(&aged, &samples, &before),
    ];
    reads.extend(stop_guard(&aged, &none));
    refused_comparison(&aged, &samples[2]);
    for read in &reads {
        eprintln!(
            "stop-verdict host-shaped {}: median {}us max {}us at load1 {}; phases {:?}; \
             rederived {}; git {}{}",
            read.read,
            read.warm.median_us,
            read.warm.max_us,
            read.warm.load1,
            read.phases_ms,
            read.rederived,
            read.git,
            read.cold_us
                .map_or(String::new(), |cold| format!("; cold first read {cold}us")),
        );
    }
    let moved = aged.aged.moved.len();
    let landed = aged.aged.landed.len();
    drop(aged);
    std::fs::remove_dir_all(&root).expect("the workload is removed");
    HostWorkload {
        shape: telemetry::host_shape(),
        class_counts,
        moved,
        landed,
        reads,
    }
}
