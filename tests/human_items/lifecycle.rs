//! The fixture's lifecycle and the read index's recoveries, over [`Scale::SMALL`]: the
//! same generator, key and store interface as the full fixture, at a scale that
//! generates in seconds.
//!
//! Only the external store is doubled — a script that keeps archives in a directory and
//! logs every call — so what is proven is [`fixture::prepare`] itself: generated once,
//! reused, restored from the store into another root and read there, regenerated when
//! any category of its key moves, and refused when it is not the shape it states.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::fixture::{self, Provenance, Spec};
use crate::generate::Scale;
use crate::harness::World;
use crate::{counted_journey, Journey, EXPECTED_SMALL};

/// One edit per category of the key: the file, and the line it is made after (none: at
/// the end of the file). Stated here rather than read from [`fixture::KEY`], so a key
/// that leaves a category out fails this test instead of agreeing with it.
const EDITS: [(&str, &str, Option<&str>); 6] = [
    ("generator", "tests/human_items/generate.rs", None),
    (
        "harness",
        "tests/e2e/harness.rs",
        Some("pub fn plan_of(name: &str, nodes: Vec<Value>) -> Value {"),
    ),
    ("journal", "src/event.rs", None),
    ("run-root", "src/ledger.rs", Some("impl RunPaths {")),
    (
        "launch-record",
        "src/ledger.rs",
        Some("pub struct LaunchRecord {"),
    ),
    (
        "item-store",
        "src/human_items.rs",
        Some("pub struct HumanItemEventBy {"),
    ),
];

/// A scratch directory of this test's own, removed when it goes.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("human-items-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A store that keeps each archive as `<dir>/<name>.tar.gz` and logs `<verb> <name>`.
fn store_double(dir: &Path) -> PathBuf {
    let held = dir.join("held");
    std::fs::create_dir_all(&held).unwrap();
    let script = dir.join("store");
    onepipeline_testfakes::executable(
        &script,
        format!(
            "#!/bin/sh\nheld='{held}'\necho \"$1 $2\" >> '{log}'\ncase \"$1\" in\n  \
             fetch) [ -f \"$held/$2.tar.gz\" ] || exit {absent}; cp \"$held/$2.tar.gz\" \"$3\" ;;\n  \
             save) cp \"$3\" \"$held/$2.tar.gz\" ;;\n  *) exit 2 ;;\nesac\n",
            held = held.display(),
            log = dir.join("calls").display(),
            absent = fixture::ABSENT,
        ),
    );
    script
}

fn calls(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("calls"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Insert a comment after `after` in `file` under `root`, or at its end.
fn edit(root: &Path, file: &str, after: Option<&str>) {
    let path = root.join(file);
    let text = std::fs::read_to_string(&path).unwrap();
    let edited = match after {
        None => format!("{text}\n// An edit the key must see.\n"),
        Some(line) => {
            assert_eq!(text.matches(line).count(), 1, "{file}: `{line}`");
            text.replacen(
                line,
                &format!("{line}\n    // An edit the key must see."),
                1,
            )
        }
    };
    std::fs::write(&path, edited).unwrap();
}

/// Every file under `root` that names `needle`.
fn naming(root: &Path, needle: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                pending.push(path);
            } else if std::fs::read(&path)
                .is_ok_and(|b| b.windows(needle.len()).any(|w| w == needle.as_bytes()))
            {
                found.push(path);
            }
        }
    }
    found
}

#[test]
fn the_fixture_is_generated_once_reused_restored_elsewhere_and_regenerated_when_its_key_moves() {
    let scratch = Scratch::new("lifecycle");
    let inputs = scratch.0.join("inputs");
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR"));
    for file in fixture::KEY
        .iter()
        .map(|i| i.file)
        .chain(EDITS.iter().map(|e| e.1))
    {
        let to = inputs.join(file);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(checkout.join(file), &to).unwrap();
    }
    let store = store_double(&scratch.0);
    let mut spec = Spec {
        scale: Scale::SMALL,
        inputs: inputs.clone(),
        local: scratch.0.join("local"),
        store: Some(store),
    };
    let runs = Scale::SMALL.shape().runs;

    // Generated once, saved to the store, and read.
    let first = fixture::prepare(&spec).unwrap();
    assert_eq!(first.provenance, Provenance::Generated);
    assert_eq!(first.launched, runs);
    let name = fixture::name(&first.key);
    assert_eq!(
        calls(&scratch.0),
        [format!("fetch {name}"), format!("save {name}")]
    );
    counted_journey(&first, &EXPECTED_SMALL);
    drop(first);

    // Unchanged inputs: the kept archive, no run launched and the store not asked.
    let again = fixture::prepare(&spec).unwrap();
    assert_eq!((again.provenance, again.launched), (Provenance::Reused, 0));
    assert_eq!(calls(&scratch.0).len(), 2, "the store is not asked");
    drop(again);

    // Nothing kept here: restored from the store into another root, which no file of it
    // names, and read there with the same counts.
    std::fs::remove_dir_all(&spec.local).unwrap();
    let restored = fixture::prepare(&spec).unwrap();
    assert_eq!(
        (restored.provenance, restored.launched),
        (Provenance::Restored, 0)
    );
    assert_eq!(calls(&scratch.0).last(), Some(&format!("fetch {name}")));
    assert_ne!(restored.generated_at, restored.world.root);
    let generated_at = restored.generated_at.display().to_string();
    assert_eq!(
        naming(&restored.world.root, &generated_at),
        Vec::<PathBuf>::new()
    );
    assert!(!naming(
        &restored.world.root,
        &restored.world.root.display().to_string()
    )
    .is_empty());
    counted_journey(&restored, &EXPECTED_SMALL);
    drop(restored);

    // An edit to the harness outside the items the generator uses moves nothing.
    edit(&inputs, "tests/e2e/harness.rs", None);
    let unmoved = fixture::prepare(&spec).unwrap();
    assert_eq!(
        unmoved.provenance,
        Provenance::Reused,
        "an unused harness edit"
    );
    drop(unmoved);

    // One edit to each category alone, then the scale: each regenerates.
    let mut keys = vec![name];
    for (category, file, after) in EDITS {
        edit(&inputs, file, after);
        let moved = fixture::prepare(&spec).unwrap();
        assert_eq!(
            (moved.provenance, moved.launched),
            (Provenance::Generated, runs),
            "an edit to the {category} category regenerates the fixture"
        );
        keys.push(fixture::name(&moved.key));
    }
    spec.scale = Scale {
        events: Scale::SMALL.events + 1,
        ..Scale::SMALL
    };
    let rescaled = fixture::prepare(&spec).unwrap();
    assert_eq!(
        rescaled.provenance,
        Provenance::Generated,
        "a change of scale"
    );
    keys.push(fixture::name(&rescaled.key));
    drop(rescaled);
    let distinct: std::collections::BTreeSet<_> = keys.iter().collect();
    assert_eq!(
        distinct.len(),
        keys.len(),
        "every edit moved the key: {keys:?}"
    );

    // A stored fixture that is not the shape it states — a run gone — is refused before
    // anything reads it.
    spec.scale = Scale::SMALL;
    let current = fixture::name(&fixture::key(&inputs, Scale::SMALL).unwrap());
    let held = scratch.0.join("held").join(format!("{current}.tar.gz"));
    let opened = scratch.0.join("opened");
    std::fs::create_dir_all(&opened).unwrap();
    tar(&[
        "-C",
        &opened.display().to_string(),
        "-xzf",
        &held.display().to_string(),
    ]);
    let run = std::fs::read_dir(opened.join("runs"))
        .unwrap()
        .flatten()
        .find(|e| e.path().join("launch.json").is_file())
        .expect("a run")
        .path();
    std::fs::remove_dir_all(run).unwrap();
    tar(&[
        "-C",
        &opened.display().to_string(),
        "-czf",
        &held.display().to_string(),
        ".",
    ]);
    std::fs::remove_dir_all(&spec.local).unwrap();
    let refused = fixture::prepare(&spec)
        .err()
        .expect("a fixture a run short is refused");
    assert!(refused.contains("not the workload it states"), "{refused}");
    assert!(refused.contains("(1, 400,"), "{refused}");
}

fn tar(args: &[&str]) {
    let status = std::process::Command::new("tar")
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "tar {args:?}");
}

/// `list --json` with its telemetry.
fn listed(world: &World, telemetry: &Path) -> (Value, Value) {
    let _ = std::fs::remove_file(telemetry);
    let output = world
        .cmd(&["human-items", "list", "--json"])
        .env("ONEPIPELINE_HUMAN_ITEMS_TELEMETRY", telemetry)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reads: Vec<Value> = std::fs::read_to_string(telemetry)
        .unwrap()
        .lines()
        .filter_map(|l| {
            serde_json::from_str::<Value>(l)
                .unwrap()
                .get("read")
                .cloned()
        })
        .collect();
    assert_eq!(reads.len(), 1, "{reads:?}");
    (
        serde_json::from_slice(&output.stdout).unwrap(),
        reads[0].clone(),
    )
}

fn raise(world: &World, input: &Value) {
    let path = world
        .root
        .join(format!("{}.json", input["dedup_key"].as_str().unwrap()));
    std::fs::write(&path, input.to_string()).unwrap();
    let output = world
        .cmd(&["human-items", "raise", "--input", path.to_str().unwrap()])
        .env("ONEPIPELINE_LAUNCHER_SESSION", "manager-00")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn question(key: &str, expiry: Value) -> Value {
    json!({
        "schema_version": 1, "dedup_key": key, "kind": "question",
        "title": format!("Question {key}"), "body": "Asked after the index was saved.",
        "priority": "low", "owner": {"session": "manager-00", "run": null, "plan": null},
        "block": {"scope": "nothing"}, "approval": null, "expiry": expiry,
    })
}

/// What the removed full-scale recovery reads asserted, over a small fixture: a deleted
/// or corrupt index is rebuilt by folding every run record and answers what the
/// journals hold; a tail behind a saved index is consumed by one reader; a due check
/// runs once.
#[test]
fn recoveries_fold_every_run_record_and_a_tail_and_a_due_check_are_consumed_once() {
    let prepared = fixture::prepare(&Spec {
        store: None,
        ..Spec::of_this_checkout(Scale::SMALL)
    })
    .unwrap();
    let world = &prepared.world;
    let shape = &prepared.shape;
    let authoritative = Journey::new(world, PathBuf::from(env!("CARGO_BIN_EXE_onepipeline")))
        .prime(shape)
        .unwrap();
    let telemetry = world.root.join("recovery.ndjson");
    let index = world.runs.join("human-items/index.json");
    // A checkpoint is a cache the journal regenerates, and a read rewrites none: with
    // them gone, each rebuild folds every run from its first record.
    for entry in std::fs::read_dir(&world.runs).unwrap().flatten() {
        let _ = std::fs::remove_file(entry.path().join("checkpoint.json"));
    }
    std::fs::remove_file(&index).unwrap();
    let rebuilt = |why: &str| {
        let (after, read) = listed(world, &telemetry);
        assert_eq!(after, authoritative, "{why}: the rebuilt answer");
        assert_eq!(read["mode"], "cold", "{why}: {read}");
        assert_eq!(
            read["run_events_replayed"], shape.run_records,
            "{why}: every run record"
        );
        assert_eq!(read["runs_rederived"], shape.runs, "{why}: every run");
        read["rebuilt_because"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    assert!(rebuilt("deleted").contains("no index"));
    std::fs::write(&index, b"{\"version\": 1, \"checksum\": \"not it\"}").unwrap();
    let corrupt = rebuilt("corrupt");
    assert!(
        !corrupt.is_empty(),
        "a corrupt index says why it was rebuilt"
    );

    // Ten item events behind a saved index: the first reader catches them up, the next
    // finds nothing new.
    let saved = std::fs::read(&index).unwrap();
    for n in 0..10 {
        raise(world, &question(&format!("tail-{n}"), json!([])));
    }
    std::fs::write(&index, &saved).unwrap();
    let (tailed, first) = listed(world, &telemetry);
    let (_, second) = listed(world, &telemetry);
    assert_eq!(
        (&first["mode"], &first["item_events_replayed"]),
        (&json!("tail"), &json!(10))
    );
    assert_eq!(second["item_events_replayed"], 0, "{second}");
    assert_eq!(tailed.as_array().unwrap().len(), shape.open_items + 10);

    // A check due at once runs on the next read, and not on the one after.
    let check =
        json!([{"when": "check", "command": ["true"], "cwd": world.root.display().to_string()}]);
    raise(world, &question("due-check", check));
    let (_, due) = listed(world, &telemetry);
    let (_, after) = listed(world, &telemetry);
    assert_eq!(
        (due["checks_run"].clone(), after["checks_run"].clone()),
        (json!(1), json!(0))
    );
}

/// `github-store.sh fetch|save NAME ARCHIVE` with `gh` stood in by a script that answers
/// one recorded listing and one artifact's zip, and logs what it was asked.
fn github_store(scratch: &Path, args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/human_items/github-store.sh");
    let mut command = std::process::Command::new(script);
    command
        .args(args)
        .env(
            "PATH",
            format!(
                "{}:{}",
                scratch.join("bin").display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .env("GITHUB_REPOSITORY", "owner/repo")
        .env("STAND_IN", scratch)
        .env_remove("HUMAN_ITEMS_FIXTURE_SAVE_DIR")
        .env_remove("HUMAN_ITEMS_FIXTURE_BRANCH");
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().unwrap()
}

#[test]
fn the_github_store_restores_the_newest_artifact_main_saved_and_saves_only_where_a_job_may() {
    let scratch = Scratch::new("github-store");
    let dir = &scratch.0;
    let name = "human-items-fixture-Linux-X64-0123456789ab";
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    onepipeline_testfakes::executable(
        &bin.join("gh"),
        "#!/bin/sh\necho \"$*\" >> \"$STAND_IN/gh.calls\"\n[ -z \"$GH_FAILS\" ] || exit 1\n\
         case \"$2\" in\n  *artifacts/*/zip) cat \"$STAND_IN/artifact-$(echo \"$2\" | sed 's#.*/artifacts/\\([0-9]*\\)/zip#\\1#').zip\" ;;\n  \
         *) cat \"$STAND_IN/listing.json\" ;;\nesac\n",
    );
    let artifact = |id: u64, branch: &str, created: &str, expired: bool| {
        json!({"id": id, "name": name, "expired": expired, "created_at": created,
               "workflow_run": {"head_branch": branch}})
    };
    let listing = |artifacts: Vec<Value>| {
        std::fs::write(
            dir.join("listing.json"),
            json!({"artifacts": artifacts}).to_string(),
        )
        .unwrap();
    };
    // Artifact 2 is the newest unexpired one main saved: 3 is newer but expired, 4 newer
    // but a pull request's branch.
    listing(vec![
        artifact(1, "main", "2026-11-01T00:00:00Z", false),
        artifact(2, "main", "2026-11-10T00:00:00Z", false),
        artifact(3, "main", "2026-11-12T00:00:00Z", true),
        artifact(4, "feature", "2026-11-13T00:00:00Z", false),
    ]);
    let payload = dir.join(format!("{name}.tar.gz"));
    std::fs::write(&payload, b"artifact two").unwrap();
    let zipped = std::process::Command::new("python3")
        .args(["-c", "import sys, zipfile\nwith zipfile.ZipFile(sys.argv[1], 'w') as z: z.write(sys.argv[2], arcname=sys.argv[3])"])
        .arg(dir.join("artifact-2.zip"))
        .arg(&payload)
        .arg(format!("{name}.tar.gz"))
        .status()
        .unwrap();
    assert!(zipped.success());
    let archive = dir.join("restored.tar.gz");
    let archive_arg = archive.display().to_string();
    let fetched = github_store(dir, &["fetch", name, &archive_arg], &[]);
    assert!(
        fetched.status.success(),
        "{}",
        String::from_utf8_lossy(&fetched.stderr)
    );
    assert_eq!(std::fs::read(&archive).unwrap(), b"artifact two");
    let calls = std::fs::read_to_string(dir.join("gh.calls")).unwrap();
    assert!(
        calls.contains(&format!("artifacts?name={name}&per_page=100")),
        "{calls}"
    );
    assert!(calls.contains("artifacts/2/zip"), "{calls}");

    // None main saved and unexpired: the fixture's absence, not a failure.
    std::fs::remove_file(&archive).unwrap();
    listing(vec![
        artifact(3, "main", "2026-11-12T00:00:00Z", true),
        artifact(4, "feature", "2026-11-13T00:00:00Z", false),
    ]);
    let absent = github_store(dir, &["fetch", name, &archive_arg], &[]);
    assert_eq!(absent.status.code(), Some(fixture::ABSENT));
    assert!(!archive.exists());
    // A branch named for the store reads that branch's.
    let named = github_store(
        dir,
        &["fetch", name, &archive_arg],
        &[("HUMAN_ITEMS_FIXTURE_BRANCH", "feature")],
    );
    assert_eq!(
        named.status.code(),
        Some(1),
        "artifact 4 has no zip to serve"
    );
    // GitHub not answering is never read as an absence.
    let unanswered = github_store(dir, &["fetch", name, &archive_arg], &[("GH_FAILS", "1")]);
    assert!(!unanswered.status.success());
    assert_ne!(unanswered.status.code(), Some(fixture::ABSENT));

    // A job with no save directory, as every pull request's, saves nothing.
    let declined = github_store(dir, &["save", name, &payload.display().to_string()], &[]);
    assert!(declined.status.success());
    assert!(String::from_utf8_lossy(&declined.stdout).contains("not saved"));
    let staged = dir.join("staged");
    let saved = github_store(
        dir,
        &["save", name, &payload.display().to_string()],
        &[("HUMAN_ITEMS_FIXTURE_SAVE_DIR", staged.to_str().unwrap())],
    );
    assert!(
        saved.status.success(),
        "{}",
        String::from_utf8_lossy(&saved.stderr)
    );
    assert_eq!(
        std::fs::read(staged.join(format!("{name}.tar.gz"))).unwrap(),
        b"artifact two"
    );
    assert_eq!(
        std::fs::read_to_string(staged.join("name")).unwrap().trim(),
        name
    );
    let refused = github_store(
        dir,
        &["save", "other-name", &payload.display().to_string()],
        &[],
    );
    assert_eq!(
        refused.status.code(),
        Some(2),
        "a name outside the family is refused"
    );
}
