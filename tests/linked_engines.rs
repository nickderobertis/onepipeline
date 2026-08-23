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
use std::path::PathBuf;
use std::process::{Command, Output};

/// The repository root. `CARGO_MANIFEST_DIR` is the crate root, which here is
/// the repo root — the directory both the recipe and the workflows run from.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The engines the check reports on, in the order it reports them. The same
/// list the script holds; asserted against its output below, so a sibling added
/// to one and not the other shows up as a missing row rather than as silence.
const SIBLINGS: [&str; 5] = [
    "oneagentgraph",
    "onevcs",
    "onevcs-testing",
    "onejudge",
    "oneharness-core",
];

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
