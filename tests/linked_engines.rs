//! What this build links, held to what its own manifest already permits.
//!
//! Three times now a release has shipped a `Cargo.lock` resolving a sibling
//! engine older than `Cargo.toml`'s requirement allowed, and each time a reader
//! met the requirement, concluded the fix was adopted, and acted on a binary
//! that had never contained it. `scripts/linked-engines.sh` is the mechanism
//! that ends that: `just engines-current` fails when the lock is behind, and
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

use std::ffi::OsStr;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
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

/// Where the crates.io sparse index files one crate. The registry's own prefix
/// rule has shorter forms for one-, two- and three-character names; the script
/// implements only this one, because no engine it reports on is that short.
fn index_path(name: &str) -> String {
    format!("{}/{}/{name}", &name[0..2], &name[2..4])
}

/// One sparse-index record, shaped the way crates.io actually shapes one.
///
/// The `deps` array is populated, and its entries are named after engines this
/// check reports on. That is the fixture, not decoration around it. A real
/// record embeds one `"name"` per dependency — seven on the first
/// `oneagentgraph` release — so a reader that counts that string across the
/// whole line sees a record with the field on it many times over, and one that
/// takes the first match sees a dependency's name where the crate's belongs.
/// Written `"deps":[]`, as every fixture here was until v0.12.4 released with
/// no record in its notes, this suite proves a shape the registry never serves:
/// the check passed every test in this file while refusing, with exit 3, every
/// answer index.crates.io gave it.
///
/// The nesting is the real one too — an array of objects, an object of arrays,
/// a `null`, and a string carrying a crate name — because each is a place the
/// record's own members could be read out of.
fn index_record(name: &str, version: &str, yanked: bool) -> String {
    format!(
        "{{\"name\":\"{name}\",\"vers\":\"{version}\",\"deps\":[\
         {{\"name\":\"onevcs\",\"req\":\"^0.13\",\"features\":[],\"optional\":false,\
         \"default_features\":true,\"target\":null,\"kind\":\"normal\"}},\
         {{\"name\":\"oneagentgraph\",\"req\":\"^0.3.0\",\"features\":[\"test-doubles\"],\
         \"optional\":true,\"default_features\":true,\"target\":\"cfg(windows)\",\
         \"kind\":\"dev\"}}],\"cksum\":\"0\",\
         \"features\":{{\"test-doubles\":[\"dep:onevcs-testing\"]}},\"yanked\":{yanked},\
         \"rust_version\":\"1.88\",\"pubtime\":\"2026-08-23T05:22:32Z\"}}\n"
    )
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
            .map(|(_, version, yanked)| index_record(name, version, *yanked))
            .collect();
        fs::write(&file, body).expect("a fixture index entry");
    }
    relative
}

/// Whether a candidate is a file this host would actually start — which on Unix
/// is the executable bit and not mere presence, because a `bash` on the `PATH`
/// without it is a file `execvp` skips.
fn runnable(candidate: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(candidate)
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        candidate.is_file()
    }
}

/// Where `bash` resolves on one `PATH`, given the directory Windows keeps its
/// own copy in.
///
/// `Command::new("bash")` is not this. Rust spawns through `CreateProcess`,
/// whose search reaches the system directory *before* `PATH`, and the
/// `bash.exe` Windows keeps there is not a shell at all — it is the WSL
/// launcher, which on a host with no distribution installed writes "Windows
/// Subsystem for Linux has no installed distributions." to stdout and exits 1.
/// Every assertion here reads an exit code and a stream, so that program
/// answering instead of the script turns each one into a report about a shell
/// that never opened the file. That is what `cross (windows-latest)` reported.
///
/// Skipping the Windows directory is not a preference between two shells:
/// Windows ships no POSIX shell there, so nothing skipped is a candidate. It
/// costs nothing anywhere else, where `SystemRoot` is unset and no directory is
/// dropped. Both spellings are tried in every directory because a bare name
/// never names a program on Windows.
///
/// The `PATH` and the Windows directory are parameters rather than read here so
/// that the case only Windows produces is driven on every platform.
fn bash_on(path: &OsStr, system_root: Option<&OsStr>) -> Option<PathBuf> {
    let windows_dir = system_root.map(|root| root.to_string_lossy().to_lowercase());
    std::env::split_paths(path)
        .filter(|dir| {
            windows_dir.as_ref().is_none_or(|root| {
                !dir.to_string_lossy()
                    .to_lowercase()
                    .starts_with(root.as_str())
            })
        })
        .flat_map(|dir| {
            ["", ".exe"]
                .into_iter()
                .map(move |ext| dir.join(format!("bash{ext}")))
        })
        .find(|candidate| runnable(candidate))
}

/// The `bash` this host runs the check's shell scripts with.
///
/// Refuses by name rather than falling back to the bare `"bash"`: that
/// fallback is exactly the lookup this exists to avoid, and a suite that
/// silently took it would report the WSL launcher's exit code as the script's.
fn bash() -> PathBuf {
    bash_on(
        &std::env::var_os("PATH").unwrap_or_default(),
        std::env::var_os("SystemRoot").as_deref(),
    )
    .expect(
        "a bash on PATH outside the Windows system directory, which is what runs this \
         repository's shell scripts",
    )
}

/// The script, run the way `just engines-current` and `just linked-engines` run
/// it: from the repository root, over this repository's real manifest and lock.
fn linked_engines(args: &[&str]) -> Output {
    Command::new(bash())
        .arg("scripts/linked-engines.sh")
        .args(args)
        .current_dir(repo_root())
        .output()
        .expect("bash runs scripts/linked-engines.sh")
}

/// A recipe, run the way the release job and the weekly workflow run it —
/// through `just`, with the registry named by `ONEPIPELINE_CRATES_INDEX`, which
/// is the override a caller with no option to pass has.
fn recipe(name: &str, index: &str) -> Output {
    Command::new("just")
        .arg(name)
        .env("ONEPIPELINE_CRATES_INDEX", index)
        .current_dir(repo_root())
        .output()
        .expect("just runs this repository's recipes")
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
        // A requirement written as a table goes in verbatim: the check accepts
        // only `name = "..."`, and a table is how a version-looking string
        // reaches this file without being the requirement.
        if engine.requirement.starts_with('{') {
            manifest.push_str(&format!("{} = {}\n", engine.name, engine.requirement));
        } else if !engine.requirement.is_empty() {
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
            .map(|version| index_record(engine.name, version, false))
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
    let cases: [(&str, Engine, &str); 8] = [
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
            "table-pin",
            Engine {
                name: "onejudge",
                requirement: "{ version = \"0.0.7\", path = \"vendor/onejudge\" }",
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
    // Records no `Engine` can express, because its versions always go into
    // well-formed entries filed under its own name. Each is appended to a sound
    // index, so what is refused is the record rather than the file. Dropped
    // silently, either would leave the lines around it answering "the newest
    // release" for a file that had more.
    for (case, record, expected) in [
        (
            "malformed-index-record",
            "{\"name\":\"oneagentgraph\"}",
            "served a 'oneagentgraph' record with no readable name, vers or yanked on it",
        ),
        (
            "foreign-index-record",
            "{\"name\":\"serde\",\"vers\":\"9.9.9\",\"yanked\":false}",
            "served a record for 'serde' under 'oneagentgraph'",
        ),
        // A second `vers` is a release the line also names and this cannot see:
        // taking the first would answer 0.0.1 for a record that went on to say
        // 9.9.9. A `deps` entry's own fields are not this — the record is walked
        // rather than searched, so only the outermost object's members count.
        (
            "twice-versioned-index-record",
            "{\"name\":\"oneagentgraph\",\"vers\":\"0.0.1\",\"vers\":\"9.9.9\",\"yanked\":false}",
            "served a 'oneagentgraph' record carrying name, vers or yanked more than once",
        ),
        // Not JSON at all, which is what a proxy, an error page or a mirror
        // serving its own format looks like from here.
        (
            "unparseable-index-record",
            "oneagentgraph 9.9.9",
            "served a 'oneagentgraph' line that is not one JSON object",
        ),
        // A `yanked` spelled as a string is a flag this cannot read, not a flag
        // that happens to say `false`. Read as one, a yanked release becomes a
        // candidate and the check reports a lock behind a version nobody can
        // resolve.
        (
            "restyped-flag-index-record",
            "{\"name\":\"oneagentgraph\",\"vers\":\"9.9.9\",\"yanked\":\"false\"}",
            "served a 'oneagentgraph' record with no readable name, vers or yanked on it",
        ),
        // A `vers` spelled like one of the reader's own verdicts is a version,
        // not a verdict. The verdict is a field of its own on each answered
        // line, so an index that serves a release called `unreadable` is
        // refused for the version it is rather than reported as a record the
        // reader could not read.
        (
            "verdict-shaped-index-version",
            "{\"name\":\"oneagentgraph\",\"vers\":\"unreadable\",\"yanked\":false}",
            "serves 'oneagentgraph' at 'unreadable', which is not a version this check can order",
        ),
        // A number `[` cannot compare is one `ver_cmp` answers "equal" to
        // everything for, which would order a lock current against a release it
        // never read.
        (
            "unorderable-index-version",
            "{\"name\":\"oneagentgraph\",\"vers\":\"99999999999999999999.0.0\",\"yanked\":false}",
            "at '99999999999999999999.0.0', which is not a version this check can order",
        ),
    ] {
        let fixture = tree(case, &CARET_SHAPES);
        let entry = repo_root()
            .join(&fixture.index)
            .join(index_path("oneagentgraph"));
        let served = fs::read_to_string(&entry).expect("the fixture index entry");
        fs::write(&entry, format!("{served}{record}\n")).expect("one more record in it");
        let run = linked_engines(&fixture.args());
        assert_eq!(
            run.status.code(),
            Some(3),
            "'{case}' leaves the registry's answer one this cannot read, which is neither \
             current nor behind:\n{}",
            said(&run)
        );
        assert!(
            String::from_utf8_lossy(&run.stderr).contains(expected),
            "'{case}' was refused without naming '{expected}':\n{}",
            said(&run)
        );
    }

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

/// The shell this suite runs the check in is never the `bash` Windows keeps in
/// its own system directory.
///
/// That file is the WSL launcher rather than a shell, and `CreateProcess`
/// reaches it before `PATH`. On `cross (windows-latest)` it answered a run of
/// the check with "Windows Subsystem for Linux has no installed distributions."
/// on stdout and exit 1, so the assertion below read an exit code from a
/// program that had never opened the script — a reading about WSL reported as a
/// reading about the lock.
///
/// Every other test here proves the lookup by using it. This one proves the
/// rule that makes it right, over a tree laid out the way that runner's is,
/// because the hosts able to run this suite are the ones where the bug cannot
/// reproduce. Both readings are asserted — with the Windows directory known and
/// with it unknown — so this fails if the skip stops being what chooses.
#[test]
fn the_shell_the_check_runs_in_is_never_the_bash_windows_keeps_in_its_system_directory() {
    let root = repo_root().join("target/linked-engines/shell-lookup");
    let _ = fs::remove_dir_all(&root);
    let windows = root.join("Windows");
    let system32 = windows.join("System32");
    let elsewhere = root.join("Git/usr/bin");

    let mut candidates = Vec::new();
    for dir in [&system32, &elsewhere] {
        fs::create_dir_all(dir).expect("a fixture directory");
        let candidate = dir.join(if cfg!(windows) { "bash.exe" } else { "bash" });
        fs::write(&candidate, "#!/bin/sh\nexit 1\n").expect("a fixture bash");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&candidate, fs::Permissions::from_mode(0o755))
                .expect("a fixture bash this host would start");
        }
        candidates.push(candidate);
    }
    let path = std::env::join_paths([&system32, &elsewhere]).expect("a fixture PATH");

    assert_eq!(
        bash_on(&path, Some(windows.as_os_str())).as_deref(),
        Some(candidates[1].as_path()),
        "the lookup took the `bash` out of the Windows directory, which is the WSL launcher \
         and not a shell this script can be read by"
    );
    assert_eq!(
        bash_on(&path, None).as_deref(),
        Some(candidates[0].as_path()),
        "the Windows directory is not what the lookup skipped, so the assertion above says \
         nothing about the runner it was written for"
    );
}

/// A command line the check cannot use is an argument error, told apart from
/// both a finding and an unreadable tree.
#[test]
fn a_command_line_the_check_cannot_use_is_refused_with_what_to_run_instead() {
    let cases: [(&[&str], &str, i32); 8] = [
        (&["--nonsense"], "unknown option --nonsense", 2),
        (&["--format"], "--format needs a value", 2),
        (&["--index"], "--index needs a value", 2),
        (&["--manifest"], "--manifest needs a value", 2),
        (&["--lock"], "--lock needs a value", 2),
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
        recovered.status.success() && recovered.stderr.is_empty(),
        "one refused request is a registry hiccup, not a stale lock, and not something a \
         weekly job should say anything about:\n{}",
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
    // And says it once. A reading that failed inside a process substitution
    // exits that subshell alone, which left the check to refuse a second time
    // for having found no permitted version — sending a reader whose registry
    // was unreachable to correct a pin that is correct.
    assert!(
        !String::from_utf8_lossy(&unread.stderr).contains("serves no 'oneagentgraph' version"),
        "a registry that never answered was also reported as a registry with nothing in the \
         window, so the reader is told to edit a requirement that is not the problem:\n{}",
        said(&unread)
    );
}

/// Both recipes are entry points to this check rather than restatements of it.
///
/// The weekly workflow and the release job reach it only through `just`, so
/// what they actually run is the recipe: one naming a different script, or the
/// wrong mode, would leave every test above proving a file nothing calls. The
/// fixture index reaches them through `ONEPIPELINE_CRATES_INDEX`, which is the
/// override a caller with no option to pass has.
#[test]
fn both_recipes_are_entry_points_to_this_check() {
    let index = index_serving("through-just", &[]);

    let current = recipe("engines-current", &index);
    assert!(
        current.status.success()
            && String::from_utf8_lossy(&current.stdout).contains("linked engines are current"),
        "`just engines-current` is not the check this suite proves:\n{}",
        said(&current)
    );

    let notes = recipe("linked-engines", &index);
    assert!(
        notes.status.success()
            && String::from_utf8_lossy(&notes.stdout).contains("### Linked engines"),
        "`just linked-engines` is not the note composer this suite proves:\n{}",
        said(&notes)
    );
}

/// The whole chain the release job runs: the recipe, the script, HTTP, and a
/// registry answering the way crates.io does — composing the record from an
/// answer it can read, and refusing one it cannot rather than composing part of
/// one.
///
/// Nothing here is substituted for the release job except the registry's
/// address, which is what `ONEPIPELINE_CRATES_INDEX` exists for. Every other
/// test in this file reaches the script through a directory, which is the
/// affordance a test has rather than the transport a release uses; this is the
/// transport.
///
/// Both halves are the bar, and neither on its own was. v0.12.4 and v0.13.0
/// published with the refusing half working exactly as designed and the
/// composing half refusing every answer the real index gave it, so their notes
/// carried no record at all. A suite proving only the refusal was green through
/// both.
#[test]
fn the_release_recipe_composes_over_http_and_refuses_an_answer_it_cannot_read() {
    let sound = registry(&index_serving("recipe-sound", &[]), 0);
    let composed = recipe("linked-engines", &sound);
    assert!(
        composed.status.success(),
        "the recipe the release job runs refused a registry answering as crates.io does, so \
         that release's notes would carry no record of what it links:\n{}",
        said(&composed)
    );
    let note = String::from_utf8_lossy(&composed.stdout);
    assert!(
        note.contains("### Linked engines"),
        "the recipe succeeded without composing the section the release job appends:\n{}",
        said(&composed)
    );
    for name in SIBLINGS {
        let version = &linked(name)[0];
        assert!(
            note.contains(&format!("| `{name}` | {version} |")),
            "the note the release job would append does not record what `{name}` links, \
             which is the whole of what it is for:\n{}",
            said(&composed)
        );
    }

    // The same tree, with one entry no sparse index could have written. Served
    // over the same transport, so what differs is the answer and not the road.
    let unreadable = index_serving("recipe-unreadable", &[]);
    fs::write(
        repo_root()
            .join(&unreadable)
            .join(index_path("oneagentgraph")),
        "oneagentgraph 0.3.9\n",
    )
    .expect("an index entry in a format the registry does not serve");
    let refused = recipe("linked-engines", &registry(&unreadable, 0));
    assert_eq!(
        refused.status.code(),
        Some(3),
        "an index answering in a shape this cannot read is neither current nor behind, and \
         the recipe has to say so rather than answer over it:\n{}",
        said(&refused)
    );
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("served a 'oneagentgraph' line that is not one JSON object"),
        "the refusal does not name what the registry served:\n{}",
        said(&refused)
    );
    // The release job appends this stdout to a Release body whatever is on it,
    // so a partial table here is a record in the notes that nothing established.
    assert!(
        !String::from_utf8_lossy(&refused.stdout).contains("Linked engines"),
        "the refusal still put part of a record on stdout, which the release job appends to \
         the notes verbatim:\n{}",
        said(&refused)
    );
}

/// The marker the note opens with is the one the release job cuts the old
/// section at.
///
/// Two files hold that string: the script writes it, and `release.yml` trims
/// the existing body from it before appending. Spelled differently in either,
/// nothing fails — a re-run of a release quietly grows a second copy of the
/// table, which is the failure mode this whole change exists to avoid a version
/// of.
#[test]
fn the_marker_the_note_opens_with_is_the_one_the_release_job_replaces() {
    let index = index_serving("marker", &[]);
    let run = linked_engines(&["--index", &index, "--format", "notes"]);
    let notes = String::from_utf8_lossy(&run.stdout);
    let marker = notes
        .lines()
        .next()
        .expect("the note opens with a line of its own");
    assert!(
        marker.starts_with("<!--") && marker.ends_with("-->"),
        "the note does not open with a marker a release job could cut at:\n{}",
        said(&run)
    );

    let release = fs::read_to_string(repo_root().join(".github/workflows/release.yml"))
        .expect("the workflow that appends the note");
    assert!(
        release.contains(&format!("/^{marker}$/")),
        "release.yml trims the old section at a marker that is not the one the note opens \
         with ({marker}), so re-running a release stacks a second table"
    );
}

/// A lock carrying build metadata is ordered by the release under it, and
/// reported by what it actually says.
///
/// The other half of the same rule. Cargo writes `2.4.0+20260823` into
/// `Cargo.lock` when a crate publishes that way, and read literally it is not a
/// version this check can order at all — `ver_cmp` splits on dots and reads
/// `0+20260823` as a number, so the run refused a lockfile cargo had in fact
/// written. Ordered by the release and reported verbatim, because the spec the
/// refusal prints has to name a copy `cargo update` can find.
#[test]
fn a_lock_carrying_build_metadata_is_ordered_by_the_release_under_it() {
    let engines = but(Engine {
        name: "oneagentgraph",
        requirement: "2.1",
        locked: &["2.4.0+20260823"],
        served: &["2.4.0", "2.5.0"],
    });
    let run = linked_engines(&tree("locked-build-metadata", &engines).args());
    assert_eq!(
        run.status.code(),
        Some(1),
        "2.4.0+20260823 orders as 2.4.0, so a lock holding it is behind 2.5.0:\n{}",
        said(&run)
    );
    let report = String::from_utf8_lossy(&run.stderr);
    assert!(
        report.contains(
            "oneagentgraph: links 2.4.0+20260823, but its requirement already permits 2.5.0"
        ),
        "the refusal does not report the version the lock actually spells:\n{}",
        said(&run)
    );
    assert!(
        report.contains("cargo update -p oneagentgraph@2.4.0+20260823"),
        "the refusal prints a spec that names no copy in the lock, so its advice does not \
         run:\n{}",
        said(&run)
    );
}

/// A release carrying build metadata is a release the lock can be behind.
///
/// `2.5.0+20260823` orders as 2.5.0 — cargo ignores build metadata — and
/// crates.io really does serve versions spelled that way. Read as a prerelease
/// and skipped, this check would call a lock at 2.4.0 current against it.
///
/// The requirement here is written with its `^` spelled out, which is the other
/// half of what cargo accepts and what this repository's own pins never use.
#[test]
fn a_release_carrying_build_metadata_is_one_the_lock_can_be_behind() {
    let engines = but(Engine {
        name: "oneagentgraph",
        requirement: "^2.1",
        locked: &["2.4.0"],
        served: &["2.4.0", "2.5.0+20260823"],
    });
    let run = linked_engines(&tree("build-metadata", &engines).args());
    assert_eq!(
        run.status.code(),
        Some(1),
        "2.5.0+20260823 is a stable release `^2.1` permits, so a lock at 2.4.0 is behind \
         it:\n{}",
        said(&run)
    );
    assert!(
        String::from_utf8_lossy(&run.stderr)
            .contains("oneagentgraph: links 2.4.0, but its requirement already permits 2.5.0"),
        "the refusal does not name the release with metadata on it as the one permitted:\n{}",
        said(&run)
    );
}
