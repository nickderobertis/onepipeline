//! What this build links, held to what its own manifest already permits.
//!
//! Three times now a release has shipped a `Cargo.lock` resolving a sibling
//! engine older than `Cargo.toml`'s requirement allowed, and each time a reader
//! met the requirement, concluded the fix was adopted, and acted on a binary
//! that had never contained it. `scripts/linked-engines.sh` is the mechanism
//! that ends that: `just lock-current` fails when the lock is behind, and
//! `just linked-engines` composes the line a release's own notes carry.
//!
//! # Why these drive the script rather than read it
//!
//! The script *is* the deliverable — a weekly workflow and a release job both
//! reach it through a recipe — so what has to be proven is its exit code and
//! its output, and it is run here exactly as they run it. What is substituted
//! is one collaborator: the crates.io sparse index, which decides what a
//! requirement permits *today* and cannot be asked that offline. `--index`
//! takes a directory in the registry's own layout, so these serve a real index
//! tree rather than intercepting anything inside the script.
//!
//! The manifest and the lock are **this repository's own**, unsubstituted. That
//! is what makes a green run here evidence about this tree: the requirements
//! parsed are the real pin block, the resolutions compared are the real ones,
//! and `oneharness-core` really is carried twice — the case where
//! `cargo update -p <name>` is refused as ambiguous.
//!
//! # What this cannot say
//!
//! Only that the lock is behind, never what is in the gap. A floor that matters
//! is still written down by hand, beside
//! `the_linked_oneagentgraph_produces_the_whole_turn_this_crate_relays` in
//! `src/agentgraph.rs` — which is what the check's own refusal tells its reader
//! to do.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::thread;

/// The repository root. `CARGO_MANIFEST_DIR` is the crate root, which here is
/// the repo root — the directory both the recipe and the workflows run from.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The engines the check reports on, in the order it reports them.
///
/// A second copy of the script's own `SIBLINGS`, and gated against it by
/// [`the_engines_this_suite_expects_are_the_engines_the_check_reports_on`]:
/// every other test here asserts that what it expects is present, which on its
/// own would say nothing about an engine added to the script and to nothing
/// else.
const SIBLINGS: [&str; 5] = [
    "oneagentgraph",
    "onevcs",
    "onevcs-testing",
    "onejudge",
    "oneharness-core",
];

/// The path of the script both recipes run.
const SCRIPT: &str = "scripts/linked-engines.sh";

/// The list the script itself reports on, read out of the script.
fn siblings_the_script_reports() -> Vec<String> {
    let script = fs::read_to_string(repo_root().join(SCRIPT)).expect("the check's own source");
    let line = script
        .lines()
        .find(|line| line.starts_with("SIBLINGS=("))
        .expect("the check names the engines it reports on in one array");
    line.trim_start_matches("SIBLINGS=(")
        .trim_end_matches(')')
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// Every version of one package this build's own `Cargo.lock` resolves.
///
/// Read from the lock rather than written down, because a fixture index has to
/// serve at least what the lock holds for "current" to mean anything — and a
/// version copied here would make these tests pass over a lock that had moved.
fn linked(name: &str) -> Vec<String> {
    let lock = fs::read_to_string(repo_root().join("Cargo.lock")).expect("this build's lockfile");
    let mut lines = lock.lines().peekable();
    let mut found = Vec::new();
    while let Some(line) = lines.next() {
        if line != format!("name = \"{name}\"") {
            continue;
        }
        let version = lines
            .peek()
            .and_then(|next| next.strip_prefix("version = \""))
            .and_then(|rest| rest.strip_suffix('"'))
            .unwrap_or_else(|| panic!("the lock's `{name}` entry is followed by its version"));
        found.push(version.to_string());
    }
    assert!(!found.is_empty(), "this build links `{name}`");
    found
}

/// Where the crates.io sparse index files one crate, by the registry's own
/// prefix rule. Every sibling's name is four characters or longer, so only that
/// arm is exercised — the script carries the shorter ones for the day one is.
fn index_path(name: &str) -> String {
    format!("{}/{}/{name}", &name[0..2], &name[2..4])
}

/// A sparse-index tree that serves every version this build links, plus
/// `extra`, as `(crate, version, yanked)`.
///
/// Returned as a path **relative** to the repository root: the script is run
/// from there, and a relative path is one no shell has to translate on the
/// Windows leg of the gate.
fn index_serving(case: &str, extra: &[(&str, &str, bool)]) -> String {
    let relative = format!("target/linked-engines/{case}");
    let root = repo_root().join(&relative);
    let _ = fs::remove_dir_all(&root);

    let mut entries: Vec<(String, String, bool)> = SIBLINGS
        .iter()
        .flat_map(|name| {
            linked(name)
                .into_iter()
                .map(move |version| (name.to_string(), version, false))
        })
        .collect();
    entries.extend(
        extra
            .iter()
            .map(|(name, version, yanked)| (name.to_string(), version.to_string(), *yanked)),
    );

    for name in SIBLINGS {
        let file = root.join(index_path(name));
        fs::create_dir_all(file.parent().expect("an index entry has a directory"))
            .expect("a fixture index directory");
        let body: String = entries
            .iter()
            .filter(|(crate_name, _, _)| crate_name == name)
            .map(|(_, version, yanked)| {
                format!(
                    "{{\"name\":\"{name}\",\"vers\":\"{version}\",\"deps\":[],\
                     \"cksum\":\"0\",\"features\":{{}},\"yanked\":{yanked}}}\n"
                )
            })
            .collect();
        fs::write(&file, body).expect("a fixture index entry");
    }
    relative
}

/// The script, run the way `just lock-current` and `just linked-engines` run
/// it: from the repository root, over this repository's real manifest and lock.
fn linked_engines(args: &[&str]) -> Output {
    Command::new("bash")
        .arg("scripts/linked-engines.sh")
        .args(args)
        .current_dir(repo_root())
        .output()
        .expect("bash runs scripts/linked-engines.sh")
}

/// Everything the run said, so an assertion's failure names the whole report
/// rather than the half it looked in.
fn said(output: &Output) -> String {
    format!(
        "exit {:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

/// A lock holding nothing back passes, and says what it holds.
///
/// The index here serves three newer `oneagentgraph` releases that are *not*
/// candidates — one yanked, one outside `^0.3.0`, one a prerelease — so a check
/// that took the highest number it saw would fail this rather than pass it.
#[test]
fn the_currency_check_passes_when_every_engine_is_the_newest_its_requirement_permits() {
    let index = index_serving(
        "current",
        &[
            ("oneagentgraph", "0.3.99", true),
            ("oneagentgraph", "0.4.0", false),
            ("oneagentgraph", "0.3.98-rc.1", false),
        ],
    );
    let run = linked_engines(&["--index", &index]);
    assert!(
        run.status.success(),
        "a lock resolving the newest release each requirement permits is current, but the \
         check refused it:\n{}",
        said(&run)
    );
    let report = String::from_utf8_lossy(&run.stdout);
    for name in SIBLINGS {
        let version = &linked(name)[0];
        assert!(
            report.contains(&format!("{name} {version}")),
            "the check passed without saying what it found for `{name}`, so a green run is \
             not evidence about this engine:\n{}",
            said(&run)
        );
    }
}

/// A lock behind what the manifest permits is refused, engine by engine, with
/// the command that fixes each — and with the place a floor gets written down.
#[test]
fn the_currency_check_names_every_engine_the_lock_holds_behind_its_own_requirement() {
    let stale_graph = &linked("oneagentgraph")[0];
    let stale_harness = linked("oneharness-core")
        .into_iter()
        .find(|version| version.starts_with("0.8."))
        .expect("the workspace pins an `oneharness-core` 0.8 copy for onejudge's reader");
    let index = index_serving(
        "stale",
        &[
            ("oneagentgraph", "0.3.100", false),
            ("oneharness-core", "0.8.100", false),
        ],
    );

    let run = linked_engines(&["--index", &index]);
    assert_eq!(
        run.status.code(),
        Some(1),
        "a lock two engines behind what the manifest permits has to fail the check:\n{}",
        said(&run)
    );
    let report = String::from_utf8_lossy(&run.stderr);
    assert!(
        report.contains(&format!(
            "oneagentgraph: links {stale_graph}, but its requirement already permits 0.3.100"
        )),
        "the refusal does not name what oneagentgraph resolves and what it could:\n{}",
        said(&run)
    );
    assert!(
        report.contains(&format!("cargo update -p oneagentgraph@{stale_graph}")),
        "the refusal does not carry the update that fixes oneagentgraph:\n{}",
        said(&run)
    );
    // The twice-carried engine: `cargo update -p oneharness-core` is refused as
    // ambiguous in this workspace, so the spec printed has to name the copy.
    assert!(
        report.contains(&format!(
            "oneharness-core: links {stale_harness}, but its requirement already permits 0.8.100"
        )),
        "the refusal skipped the engine this workspace carries twice, which is the one a \
         generic check is most likely to drop:\n{}",
        said(&run)
    );
    assert!(
        report.contains(&format!("cargo update -p oneharness-core@{stale_harness}")),
        "the refusal names an `oneharness-core` spec that is not qualified by the copy it \
         means, so running it would be refused as ambiguous:\n{}",
        said(&run)
    );
    assert!(
        report.contains("the_linked_oneagentgraph_produces_the_whole_turn_this_crate_relays"),
        "the refusal does not send its reader to where this repository records *why* a floor \
         matters, which is the half a generic check cannot express:\n{}",
        said(&run)
    );
}

/// The qualified spec the refusal prints is one `cargo` accepts, where the bare
/// name is not.
///
/// Driven against the real workspace and the real `cargo`, because the claim is
/// about that program's package-id grammar rather than about this repository's
/// output. The two `oneharness-core` copies are deliberate and `Cargo.toml` says
/// why; if that ever stops being true this fails, which is the right place to
/// find out that the ambiguity this guards against is gone.
#[test]
fn the_update_spec_this_check_prints_for_a_twice_carried_engine_is_one_cargo_accepts() {
    let copies = linked("oneharness-core");
    assert!(
        copies.len() > 1,
        "this workspace no longer carries `oneharness-core` twice, so there is no ambiguous \
         spec left to guard against: {copies:?}"
    );

    let update = |spec: &str| {
        Command::new(env!("CARGO"))
            .args(["update", "--dry-run", "--offline", "-p", spec])
            .current_dir(repo_root())
            .output()
            .expect("cargo runs an update dry run")
    };

    let bare = update("oneharness-core");
    assert!(
        !bare.status.success() && String::from_utf8_lossy(&bare.stderr).contains("ambiguous"),
        "cargo accepted the bare spec, so the qualification below proves nothing:\n{}",
        said(&bare)
    );

    let qualified = update(&format!("oneharness-core@{}", copies[0]));
    assert!(
        qualified.status.success(),
        "cargo refused the qualified spec this check prints, so its advice does not run:\n{}",
        said(&qualified)
    );
}

/// A release's notes record the version of every engine that release links.
#[test]
fn the_release_note_records_the_version_of_every_engine_the_build_links() {
    let index = index_serving("notes-current", &[]);
    let run = linked_engines(&["--index", &index, "--format", "notes"]);
    assert!(
        run.status.success(),
        "composing the release note failed:\n{}",
        said(&run)
    );
    let notes = String::from_utf8_lossy(&run.stdout);
    for name in SIBLINGS {
        for version in linked(name) {
            assert!(
                notes.contains(&format!("| `{name}` | {version} |")),
                "the note does not record that this build links `{name}` {version}, which \
                 leaves the published SBOM the only answer to what a release links:\n{}",
                said(&run)
            );
        }
    }
    assert!(
        notes.contains("Every linked engine is the newest its own requirement permits."),
        "the note does not say the resolutions are current, so a reader cannot tell a \
         checked claim from an unchecked one:\n{}",
        said(&run)
    );
}

/// Where a linked engine is behind, the notes say so on the spot — which is the
/// whole point: the reader who met the requirement and drew the wrong
/// conclusion was reading the release, not the lock.
#[test]
fn the_release_note_says_so_where_a_linked_engine_is_behind_the_requirement() {
    let stale_graph = &linked("oneagentgraph")[0];
    let index = index_serving("notes-stale", &[("oneagentgraph", "0.3.100", false)]);
    let run = linked_engines(&["--index", &index, "--format", "notes"]);
    assert!(
        run.status.success(),
        "the note is composed for a behind release too — it is the release that most needs \
         one:\n{}",
        said(&run)
    );
    let notes = String::from_utf8_lossy(&run.stdout);
    assert!(
        notes.contains(&format!(
            "| `oneagentgraph` | **{stale_graph}** | `0.3.0` | **0.3.100** — this release is \
             behind it |"
        )),
        "the note records the version without saying it is behind what the requirement \
         permits:\n{}",
        said(&run)
    );
    assert!(
        notes.contains(&format!(
            "- `oneagentgraph` links {stale_graph}; the requirement already permitted 0.3.100."
        )),
        "the note carries no warning naming the engine that is behind, so a reader still has \
         to diff the lock to learn it:\n{}",
        said(&run)
    );
    assert!(
        !notes.contains("Every linked engine is the newest its own requirement permits."),
        "the note claims every engine is current while recording one that is not:\n{}",
        said(&run)
    );
}

/// The engines this suite expects are the engines the check reports on.
///
/// Both lists are maintained by hand — the script's, because it is what decides
/// what is read, and this one, because a suite that derived its expectation
/// from the subject would assert nothing. This is what makes the pair a mirror
/// rather than two lists: an engine added to either alone fails here.
#[test]
fn the_engines_this_suite_expects_are_the_engines_the_check_reports_on() {
    assert_eq!(
        siblings_the_script_reports(),
        SIBLINGS.map(str::to_string).to_vec(),
        "{SCRIPT} and this suite disagree about which engines are checked, so one of them is \
         reporting on an engine nothing proves"
    );
}

/// One engine's whole story in a tree of a test's own making: what the manifest
/// requires, what the lock resolved, and what the registry serves.
struct Engine {
    name: &'static str,
    requirement: &'static str,
    locked: &'static [&'static str],
    served: &'static [&'static str],
}

/// The paths a made-up tree is driven through, relative to the repository root
/// — relative because the script is run from there, and a relative path is one
/// no shell has to translate on the Windows leg of the gate.
struct Tree {
    manifest: String,
    lock: String,
    index: String,
}

impl Tree {
    /// The arguments that point the check at this tree instead of the
    /// repository's own.
    fn args(&self) -> Vec<&str> {
        vec![
            "--manifest",
            &self.manifest,
            "--lock",
            &self.lock,
            "--index",
            &self.index,
        ]
    }
}

/// A manifest, lock and sparse index written from `engines`.
///
/// The real files answer for this repository and cannot be made to answer for
/// anything else: they carry one requirement shape per engine and whatever the
/// registry happens to serve. The readings below — the other caret shapes cargo
/// defines, a manifest missing a pin, a lock cargo did not write — exist only
/// in a tree a test builds.
fn tree(case: &str, engines: &[Engine]) -> Tree {
    let relative = format!("target/linked-engines/{case}");
    let root = repo_root().join(&relative);
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("a fixture tree");

    let mut manifest = String::from("[workspace.dependencies]\n");
    let mut lock = String::from("version = 4\n");
    for engine in engines {
        if !engine.requirement.is_empty() {
            manifest.push_str(&format!("{} = \"{}\"\n", engine.name, engine.requirement));
        }
        for version in engine.locked {
            lock.push_str(&format!(
                "\n[[package]]\nname = \"{}\"\nversion = \"{version}\"\n",
                engine.name
            ));
        }
        if engine.served.is_empty() {
            continue;
        }
        let file = root.join("index").join(index_path(engine.name));
        fs::create_dir_all(file.parent().expect("an index entry has a directory"))
            .expect("a fixture index directory");
        let body: String = engine
            .served
            .iter()
            .map(|version| {
                format!(
                    "{{\"name\":\"{}\",\"vers\":\"{version}\",\"deps\":[],\
                     \"cksum\":\"0\",\"features\":{{}},\"yanked\":false}}\n",
                    engine.name
                )
            })
            .collect();
        fs::write(&file, body).expect("a fixture index entry");
    }
    fs::write(root.join("Cargo.toml"), manifest).expect("a fixture manifest");
    fs::write(root.join("Cargo.lock"), lock).expect("a fixture lockfile");

    Tree {
        manifest: format!("{relative}/Cargo.toml"),
        lock: format!("{relative}/Cargo.lock"),
        index: format!("{relative}/index"),
    }
}

/// A sound tree, whose five engines between them take every caret shape cargo
/// defines that this repository's own pins do not.
///
/// Each one is served a release *just* outside its window — 3.0.0 against
/// `^2.1`, 1.0.0 against `^0`, 0.0.4 against `^0.0.3`, 0.1.0 against `^0.0`,
/// 2.0.0 against `^1` — so a window computed by any other rule reports the
/// engine as behind, or stops finding the locked copy at all.
const CARET_SHAPES: [Engine; 5] = [
    Engine {
        name: "oneagentgraph",
        requirement: "2.1",
        locked: &["2.4.0"],
        served: &["2.4.0", "3.0.0"],
    },
    Engine {
        name: "onevcs",
        requirement: "0",
        locked: &["0.9.9"],
        served: &["0.9.9", "1.0.0"],
    },
    Engine {
        name: "onevcs-testing",
        requirement: "0.0.3",
        locked: &["0.0.3"],
        served: &["0.0.3", "0.0.4"],
    },
    Engine {
        name: "onejudge",
        requirement: "0.0",
        locked: &["0.0.7"],
        served: &["0.0.7", "0.1.0"],
    },
    Engine {
        name: "oneharness-core",
        requirement: "1",
        locked: &["1.2.3"],
        served: &["1.2.3", "2.0.0"],
    },
];

/// `CARET_SHAPES` with one engine replaced, so a refusal test states only the
/// one thing it is about.
fn but(replacement: Engine) -> [Engine; 5] {
    CARET_SHAPES.map(|engine| {
        if engine.name == replacement.name {
            Engine { ..replacement }
        } else {
            engine
        }
    })
}

/// The windows this check computes are cargo's own caret rules.
///
/// This repository pins every engine as `^0.Y` or `^0.Y.Z`, so the other four
/// shapes — a non-zero major, a bare major, `^0.0`, `^0.0.Z` — are reachable
/// only through a tree like this one. Getting any of them wrong is how a check
/// reports a currency it never established.
#[test]
fn the_windows_this_check_computes_are_cargos_own_caret_rules() {
    let tree = tree("caret-shapes", &CARET_SHAPES);
    let run = linked_engines(&tree.args());
    assert!(
        run.status.success(),
        "every engine here resolves the newest release its own caret permits, and the one \
         above each window is outside it:\n{}",
        said(&run)
    );
    let report = String::from_utf8_lossy(&run.stdout);
    for engine in &CARET_SHAPES {
        assert!(
            report.contains(&format!("{} {}", engine.name, engine.locked[0])),
            "the check passed without reporting `{}`, whose requirement is `{}`:\n{}",
            engine.name,
            engine.requirement,
            said(&run)
        );
    }
}

/// Everything the check refuses to answer over, rather than answering wrongly.
///
/// Each of these is a tree it cannot read — a pin it does not model, a lock
/// cargo did not write, a registry that has nothing to say — and each exits 3,
/// which is this script's "no reading", distinct from the 1 that means an
/// engine really is behind. A check that guessed at any of them would report a
/// currency nothing established, which is the failure it exists to prevent.
#[test]
fn a_tree_the_check_cannot_read_is_refused_rather_than_answered() {
    let cases: [(&str, Engine, &str); 7] = [
        (
            "no-pin",
            Engine {
                name: "onejudge",
                requirement: "",
                locked: &["0.0.7"],
                served: &["0.0.7"],
            },
            "has no requirement in [workspace.dependencies]",
        ),
        (
            "unmodelled-operator",
            Engine {
                name: "onevcs",
                requirement: ">=0.9",
                locked: &["0.9.9"],
                served: &["0.9.9"],
            },
            "is a requirement shape this check does not model",
        ),
        (
            "nothing-in-window",
            Engine {
                name: "onevcs",
                requirement: "0",
                locked: &["1.5.0"],
                served: &["0.9.9"],
            },
            "resolves no 'onevcs' that '0' permits",
        ),
        (
            "index-has-nothing",
            Engine {
                name: "oneagentgraph",
                requirement: "2.1",
                locked: &["2.4.0"],
                served: &["3.0.0"],
            },
            "serves no 'oneagentgraph' version that '2.1' permits",
        ),
        (
            "no-index-entry",
            Engine {
                name: "oneagentgraph",
                requirement: "2.1",
                locked: &["2.4.0"],
                served: &[],
            },
            "no index entry for 'oneagentgraph'",
        ),
        (
            "unorderable-lock",
            Engine {
                name: "onejudge",
                requirement: "0.0",
                locked: &["0.0"],
                served: &["0.0.7"],
            },
            "resolves 'onejudge' at '0.0', which is not a version this check can order",
        ),
        (
            "unorderable-index",
            Engine {
                name: "onejudge",
                requirement: "0.0",
                locked: &["0.0.7"],
                served: &["0.0.7", "0.0"],
            },
            "serves 'onejudge' at '0.0', which is not a version this check can order",
        ),
    ];

    for (case, engine, expected) in cases {
        let tree = tree(case, &but(engine));
        let run = linked_engines(&tree.args());
        assert_eq!(
            run.status.code(),
            Some(3),
            "'{case}' is a tree the check cannot read, which is neither current nor behind:\n{}",
            said(&run)
        );
        assert!(
            String::from_utf8_lossy(&run.stderr).contains(expected),
            "'{case}' was refused without naming '{expected}', so its reader is told a \
             reading failed but not which one:\n{}",
            said(&run)
        );
    }
}

/// A command line the check cannot use is an argument error, told apart from
/// both a finding and an unreadable tree.
#[test]
fn a_command_line_the_check_cannot_use_is_refused_with_what_to_run_instead() {
    let cases: [(&[&str], &str, i32); 6] = [
        (&["--nonsense"], "unknown option --nonsense", 2),
        (&["--format"], "--format needs a value", 2),
        (&["--index"], "--index needs a value", 2),
        (&["--format", "sideways"], "unknown format 'sideways'", 2),
        (
            &["--manifest", "target/linked-engines/absent.toml"],
            "no manifest at",
            3,
        ),
        (
            &["--lock", "target/linked-engines/absent.lock"],
            "no lockfile at",
            3,
        ),
    ];

    for (args, expected, code) in cases {
        let run = linked_engines(args);
        assert_eq!(
            run.status.code(),
            Some(code),
            "`{args:?}` should exit {code}:\n{}",
            said(&run)
        );
        let report = String::from_utf8_lossy(&run.stderr);
        assert!(
            report.contains(expected),
            "`{args:?}` was refused without naming '{expected}':\n{}",
            said(&run)
        );
        // An argument error names the whole usage; an unreadable path names the
        // one option that would have pointed somewhere else.
        assert!(
            report.contains("ACTION: "),
            "`{args:?}` was refused without saying what to do instead:\n{}",
            said(&run)
        );
    }
}

/// A sparse index served over HTTP, which is the transport the real recipes
/// use — a directory is the affordance a test has, not the thing being proven.
///
/// `refusals` requests are answered `503` before it serves anything, so the
/// retry the script does can be driven both ways: a registry that hiccups once
/// and one that never answers. The listener outlives the test; nextest gives
/// each test its own process, which is what ends it.
fn registry(index: &str, refusals: usize) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("an ephemeral port");
    let base = format!(
        "http://{}",
        listener.local_addr().expect("the bound address")
    );
    let root = repo_root().join(index);
    thread::spawn(move || {
        let mut refused = 0;
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = String::new();
            let peer = stream
                .try_clone()
                .expect("the request side of the connection");
            if BufReader::new(peer).read_line(&mut request).is_err() {
                continue;
            }
            let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
            let response = if refused < refusals {
                refused += 1;
                "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_string()
            } else {
                match fs::read_to_string(root.join(path.trim_start_matches('/'))) {
                    Ok(body) => format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    ),
                    Err(_) => {
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_string()
                    }
                }
            };
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    base
}

/// A registry that hiccups is retried, and one that never answers is reported
/// as unread rather than as a finding.
///
/// The retry exists because a registry read fails for reasons that have nothing
/// to do with the lock, and this job runs on a weekly schedule and on every
/// release — a single refused request turning either red would train its reader
/// to ignore it.
#[test]
fn a_registry_that_hiccups_is_retried_and_one_that_never_answers_is_not_a_finding() {
    let tree = tree("over-http", &CARET_SHAPES);

    let recovered = linked_engines(&[
        "--manifest",
        &tree.manifest,
        "--lock",
        &tree.lock,
        "--index",
        &registry(&tree.index, 1),
    ]);
    assert!(
        recovered.status.success(),
        "one refused request is a registry hiccup, not a stale lock:\n{}",
        said(&recovered)
    );

    let unread = linked_engines(&[
        "--manifest",
        &tree.manifest,
        "--lock",
        &tree.lock,
        "--index",
        &registry(&tree.index, usize::MAX),
    ]);
    assert_eq!(
        unread.status.code(),
        Some(3),
        "a registry that never answers leaves the question unasked, which is neither current \
         nor behind:\n{}",
        said(&unread)
    );
    assert!(
        String::from_utf8_lossy(&unread.stderr).contains("did not serve 'oneagentgraph'"),
        "the refusal does not name the registry read that failed:\n{}",
        said(&unread)
    );
}
