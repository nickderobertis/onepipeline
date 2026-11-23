//! The human-items fixture, generated once and reused: [`prepare`] answers a world
//! holding the workload [`generate`] makes at one [`Scale`], from the first of
//!
//! 1. the archive this checkout kept under `target/human-items-fixture/`, when its key
//!    is the key the inputs give now (**reused**);
//! 2. the external store [`STORE_ENV`] names, asked for that key (**restored**);
//! 3. the generator, run there and then (**generated**), whose archive is kept locally
//!    and, once it has passed the shape check below, offered to the store, which saves it
//!    only where it is allowed to (`github-store.sh`: a push to main's `timed` job alone).
//!
//! Each is then unpacked into a fresh world, rewritten from the root it was generated
//! at to that world's (a generated fixture names its root in 322 of its 5,483 files),
//! and checked against the shape its scale states before anything reads it: a wrong or
//! stale fixture is refused rather than measured.
//!
//! # The key
//!
//! [`KEY`] is every input that decides what a fixture holds, by category:
//!
//! - **generator**: `generate.rs`, which makes the workload, and this file, which
//!   archives and relocates it;
//! - **harness**: the items of `tests/e2e/harness.rs` the generator calls, and what they
//!   call to launch and end a run — each item's text, so an edit elsewhere in the
//!   harness, which changes most weeks, regenerates nothing;
//! - **formats**: the shapes the binary writes that the fixture records — the journal's
//!   records (`src/event.rs`), the run root's layout (`RunPaths`), the launch record
//!   (`LaunchRecord`) and the item store's records, paths and check record;
//! - **scale**: the [`Scale`], so a small fixture and the full one never share a key.
//!
//! The binary's other behaviour is not in it: what a read derives from those files (the
//! read index, run checkpoints) is derived data the journey's priming read rebuilds, and
//! the shape check refuses a fixture whose content no longer matches what it states.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use crate::generate::{self, Scale, Shape};
use crate::harness::{World, NO_SESSION_BUS};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// The variable naming the external store's command, which [`prepare`] runs as
/// `<command> fetch <name> <archive>` and `<command> save <name> <archive>`. Unset, the
/// fixture is kept in this checkout alone.
pub const STORE_ENV: &str = "HUMAN_ITEMS_FIXTURE_STORE";

/// A fetch's exit status when the store holds no fixture of that name.
pub const ABSENT: i32 = 3;

/// The manifest each archive carries at its root.
const MANIFEST: &str = "human-items-fixture.json";

/// One category of the key: the file it reads, and the items of it, by the first line's
/// prefix, it reads (none: the whole file).
pub struct KeyInput {
    pub category: &'static str,
    pub file: &'static str,
    pub items: &'static [&'static str],
}

/// Every input of a fixture's key. The items are named by the start of their first line
/// and each must name exactly one item, so a renamed one fails the key rather than
/// dropping out of it.
pub const KEY: &[KeyInput] = &[
    KeyInput {
        category: "generator",
        file: "tests/human_items/generate.rs",
        items: &[],
    },
    KeyInput {
        category: "generator",
        file: "tests/human_items/fixture.rs",
        items: &[],
    },
    KeyInput {
        category: "harness",
        file: "tests/e2e/harness.rs",
        items: &[
            "pub fn agent(",
            "pub fn human(",
            "pub fn plan_of(",
            "    pub fn new(name: &str) -> Self {",
            "    pub fn at(root: &Path, name: &str) -> Self {",
            "    pub fn cmd(&self, args: &[&str]) -> Command {",
            "    pub fn cmd_on(&self, program: &Path, args: &[&str]) -> Command {",
            "    pub fn plan(&self, name: &str, plan: &Value) -> String {",
            "    pub fn plan_in(&self, store: &Path, name: &str, plan: &Value) -> String {",
            "    pub fn paused_and_ended(&self, run: &str) {",
            "    pub fn waiting_driver(&self, run: &str) -> u32 {",
            "    pub fn until_paused(&self, run: &str) {",
            "pub fn end_driver(",
        ],
    },
    KeyInput {
        category: "journal",
        file: "src/event.rs",
        items: &[],
    },
    KeyInput {
        category: "run-root",
        file: "src/ledger.rs",
        items: &["impl RunPaths {"],
    },
    KeyInput {
        category: "launch-record",
        file: "src/ledger.rs",
        items: &["pub struct LaunchRecord {"],
    },
    KeyInput {
        category: "item-store",
        file: "src/human_items.rs",
        items: &[
            "macro_rules! expiry_condition {",
            "pub struct HumanItem, HumanItemWire {",
            "pub struct HumanItemEventPayload, HumanItemEventPayloadWire {",
            "pub struct HumanItemEventBatch {",
            "pub struct HumanItemEventBy {",
        ],
    },
    KeyInput {
        category: "item-store",
        file: "src/human_items/live.rs",
        items: &[
            "pub(crate) const STREAM: &str",
            "    fn dir(&self) -> PathBuf {",
            "    pub(super) fn store_dir(&self) -> PathBuf {",
        ],
    },
    KeyInput {
        category: "item-store",
        file: "src/human_items/expiry.rs",
        items: &["struct Record {", "struct Entry {", "fn record_path("],
    },
];

/// The text of the one item of `source` whose first line starts with `start`: its
/// attributes, and every line to the brace closing it at its own indentation.
pub fn item(source: &str, start: &str) -> Result<String, String> {
    let lines: Vec<&str> = source.lines().collect();
    let found: Vec<usize> = (0..lines.len())
        .filter(|&n| lines[n].starts_with(start))
        .collect();
    let [first] = found[..] else {
        return Err(format!(
            "{} items start with `{start}`, not one",
            found.len()
        ));
    };
    let indent = &lines[first][..lines[first].len() - lines[first].trim_start().len()];
    let mut begin = first;
    while begin > 0 && lines[begin - 1].trim_start().starts_with("#[") {
        begin -= 1;
    }
    if lines[first].trim_end().ends_with(';') {
        return Ok(lines[begin..=first].join("\n"));
    }
    let close = (first + 1..lines.len())
        .find(|&n| {
            lines[n].starts_with(indent) && matches!(&lines[n][indent.len()..], "}" | "};" | "})")
        })
        .ok_or_else(|| format!("the item starting `{start}` is never closed"))?;
    Ok(lines[begin..=close].join("\n"))
}

/// The key of a fixture at `scale`, read from the [`KEY`] inputs under `root`.
pub fn key(root: &Path, scale: Scale) -> Result<String, String> {
    let mut hash = Sha256::new();
    hash.update(b"human-items-fixture 1\n");
    for input in KEY {
        let path = root.join(input.file);
        let source = std::fs::read_to_string(&path)
            .map_err(|e| format!("the key reads {}: {e}", path.display()))?;
        hash.update(format!("{} {}\n", input.category, input.file));
        if input.items.is_empty() {
            hash.update(source.as_bytes());
        }
        for start in input.items {
            let text = item(&source, start).map_err(|e| format!("{}: {e}", input.file))?;
            hash.update(text.as_bytes());
            hash.update(b"\n");
        }
    }
    hash.update(format!("scale {scale:?}\n"));
    Ok(hash
        .finalize()
        .iter()
        .take(12)
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// The name a fixture of `key` is stored under: the `human-items-fixture-<OS>-<arch>-`
/// family, then its key.
pub fn name(key: &str) -> String {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "X64",
        "aarch64" => "ARM64",
        other => other,
    };
    format!("human-items-fixture-Linux-{arch}-{key}")
}

/// Where and how one fixture is prepared.
pub struct Spec {
    pub scale: Scale,
    /// The checkout the key's inputs are read from.
    pub inputs: PathBuf,
    /// Where this checkout keeps its archives, one directory per scale beneath it.
    pub local: PathBuf,
    /// The external store's command, if any.
    pub store: Option<PathBuf>,
}

impl Spec {
    /// This checkout's fixture at `scale`, kept under its `target/`, with the store the
    /// environment names.
    pub fn of_this_checkout(scale: Scale) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        Self {
            scale,
            local: root.join("target/human-items-fixture"),
            inputs: root,
            store: std::env::var_os(STORE_ENV)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
        }
    }
}

/// Where a prepared fixture came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    Reused,
    Restored,
    Generated,
}

/// A fixture unpacked into a world of its own and checked against its shape.
pub struct Prepared {
    pub world: FixtureWorld,
    pub provenance: Provenance,
    pub key: String,
    pub shape: Shape,
    /// The root it was generated at, which no file of it names any longer.
    pub generated_at: PathBuf,
    /// Runs the generator launched for it: none unless it was generated.
    pub launched: usize,
    /// Seconds taken to reuse, restore or generate the archive, and to unpack it.
    pub obtained_s: f64,
    pub unpacked_s: f64,
    /// The archive's size in bytes.
    pub archive_bytes: u64,
}

/// A world over an unpacked fixture. Its runs were ended where it was generated, so it
/// removes them before the world's own teardown, which would stop each in turn.
pub struct FixtureWorld(pub World);

impl Drop for FixtureWorld {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0.runs);
    }
}

impl std::ops::Deref for FixtureWorld {
    type Target = World;
    fn deref(&self) -> &World {
        &self.0
    }
}

/// Reuse, restore or generate the fixture `spec` states, then unpack and check it.
pub fn prepare(spec: &Spec) -> Result<Prepared, String> {
    let began = Instant::now();
    let key = key(&spec.inputs, spec.scale)?;
    let name = name(&key);
    let dir = spec.local.join(spec.scale.name());
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let archive = dir.join(format!("{name}.tar.gz"));
    let mut launched = 0;
    let provenance = if archive.is_file() {
        Provenance::Reused
    } else if fetch(spec.store.as_deref(), &name, &archive)? {
        Provenance::Restored
    } else {
        launched = generated(spec.scale, &key, &archive)?;
        Provenance::Generated
    };
    // One archive per scale is kept: an earlier key's is stale by construction.
    for entry in std::fs::read_dir(&dir)
        .map_err(|e| e.to_string())?
        .flatten()
    {
        if entry.path() != archive {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let obtained_s = began.elapsed().as_secs_f64();
    let unpacking = Instant::now();
    let world = FixtureWorld(World::new("human-items-fixture"));
    unpack(&archive, &world.root)?;
    let manifest: Value = read_json(&world.root.join(MANIFEST))?;
    let generated_at = generation_root(&manifest)?;
    relocate(&world.root, &generated_at)?;
    let unpacked_s = unpacking.elapsed().as_secs_f64();
    let archive_bytes = std::fs::metadata(&archive).map_or(0, |m| m.len());
    eprintln!(
        "human-items fixture {name}: {provenance:?} in {obtained_s:.1} s ({launched} runs \
         launched), {archive_bytes} bytes, unpacked and relocated in {unpacked_s:.1} s"
    );
    let shape = spec.scale.shape();
    if manifest["key"] != json!(key) || manifest["scale"] != json!(format!("{:?}", spec.scale)) {
        return Err(format!(
            "the fixture {name} carries the key {} at {}, not {key} at {:?}",
            manifest["key"], manifest["scale"], spec.scale
        ));
    }
    check_files(&world, &shape)?;
    // Offered to the store only once it is the shape it states.
    if provenance == Provenance::Generated {
        save(spec.store.as_deref(), &name, &archive)?;
    }
    Ok(Prepared {
        world,
        provenance,
        key,
        shape,
        generated_at,
        launched,
        obtained_s,
        unpacked_s,
        archive_bytes,
    })
}

/// The root a manifest says its fixture was generated at, refused unless it is a world's
/// root — an absolute `…/op-<pid>-<n>`, as [`World::new`] names one — since every file
/// naming it is rewritten.
fn generation_root(manifest: &Value) -> Result<PathBuf, String> {
    let root = manifest["root"]
        .as_str()
        .ok_or("the fixture's manifest names no root")?;
    let path = PathBuf::from(root);
    let named = path
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_prefix("op-"))
        .and_then(|n| n.split_once('-'))
        .is_some_and(|(pid, nth)| {
            [pid, nth]
                .iter()
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        });
    if path.is_absolute() && path.components().count() > 2 && named {
        Ok(path)
    } else {
        Err(format!(
            "the fixture's manifest names `{root}` as its root, which is no world's root"
        ))
    }
}

/// Ask the store for `name` into `archive`: whether it held it. A store that cannot
/// answer is said, and the fixture is generated instead, which is always correct.
fn fetch(store: Option<&Path>, name: &str, archive: &Path) -> Result<bool, String> {
    let Some(store) = store else {
        return Ok(false);
    };
    let partial = archive.with_extension("partial");
    let status = Command::new(store)
        // The real `gh` this may run asks no session bus for a token, even before any world
        // has set this for the process.
        .env(NO_SESSION_BUS.0, NO_SESSION_BUS.1)
        .args(["fetch", name])
        .arg(&partial)
        .status()
        .map_err(|e| format!("the fixture store {} would not start: {e}", store.display()))?;
    match status.code() {
        Some(0) if partial.is_file() => {
            std::fs::rename(&partial, archive).map_err(|e| e.to_string())?;
            Ok(true)
        }
        Some(ABSENT) => Ok(false),
        _ => {
            let _ = std::fs::remove_file(&partial);
            eprintln!(
                "human-items fixture {name}: the store could not answer ({status}); generating it"
            );
            Ok(false)
        }
    }
}

/// Offer `archive` to the store as `name`.
fn save(store: Option<&Path>, name: &str, archive: &Path) -> Result<(), String> {
    let Some(store) = store else {
        return Ok(());
    };
    let status = Command::new(store)
        // The real `gh` this may run asks no session bus for a token, even before any world
        // has set this for the process.
        .env(NO_SESSION_BUS.0, NO_SESSION_BUS.1)
        .args(["save", name])
        .arg(archive)
        .status()
        .map_err(|e| format!("the fixture store {} would not start: {e}", store.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "the fixture store refused to save {name}: {status}"
        ))
    }
}

/// Generate the fixture in a world of its own and archive it as `archive`: the runs
/// the generator launched.
fn generated(scale: Scale, key: &str, archive: &Path) -> Result<usize, String> {
    let world = FixtureWorld(World::new("human-items-fixture"));
    let runs = generate::generate(&world, scale);
    std::fs::write(
        world.root.join(MANIFEST),
        serde_json::to_vec_pretty(&json!({
            "key": key,
            "scale": format!("{scale:?}"),
            "root": world.root.display().to_string(),
        }))
        .unwrap(),
    )
    .map_err(|e| e.to_string())?;
    let partial = archive.with_extension("partial");
    let output = Command::new("tar")
        .arg("-C")
        .arg(&world.root)
        .arg("-czf")
        .arg(&partial)
        .arg(".")
        .output()
        .map_err(|e| format!("tar would not start: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "tar could not archive the fixture: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    std::fs::rename(&partial, archive).map_err(|e| e.to_string())?;
    Ok(runs.len())
}

/// `tar` with `args` over `archive`, answering what it printed.
fn listed(args: &str, archive: &Path) -> Result<String, String> {
    let output = Command::new("tar")
        .arg(args)
        .arg(archive)
        .output()
        .map_err(|e| format!("tar would not start: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!(
            "tar could not list {}: {}",
            archive.display(),
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

/// Unpack `archive` into `root`, refused first unless every entry is a regular file or a
/// directory named inside it — no link, device, absolute name or `..` — as every archive
/// [`generated`] writes is.
fn unpack(archive: &Path, root: &Path) -> Result<(), String> {
    for name in listed("-tzf", archive)?.lines() {
        let inside = name
            .strip_prefix("./")
            .is_some_and(|rest| !rest.split('/').any(|part| part == ".."));
        if !(name == "./" || inside) {
            return Err(format!(
                "{} holds `{name}`, which is not named inside the fixture",
                archive.display()
            ));
        }
    }
    for entry in listed("-tvzf", archive)?.lines() {
        if !(entry.starts_with('-') || entry.starts_with('d')) {
            return Err(format!(
                "{} holds `{entry}`, which is neither a file nor a directory",
                archive.display()
            ));
        }
    }
    let output = Command::new("tar")
        .arg("-C")
        .arg(root)
        .arg("-xzf")
        .arg(archive)
        .output()
        .map_err(|e| format!("tar would not start: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "tar could not unpack {}: {}",
            archive.display(),
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

/// Rewrite every file and link under `root` that names `from` to name `root` instead.
pub fn relocate(root: &Path, from: &Path) -> Result<(), String> {
    let from = from.display().to_string();
    let to = root.display().to_string();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir)
            .map_err(|e| e.to_string())?
            .flatten()
        {
            let path = entry.path();
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            let named = path.display().to_string();
            // The new root may begin with the old one's text (`op-1-2` beneath `op-1-28`).
            let below_root = named.strip_prefix(&to).unwrap_or(&named);
            if names(below_root.as_bytes(), &from) {
                return Err(format!(
                    "{} is named for its generation root",
                    path.display()
                ));
            }
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_symlink() {
                let target = std::fs::read_link(&path).map_err(|e| e.to_string())?;
                let spelled = target.to_string_lossy();
                if let Some(moved) = replaced(spelled.as_bytes(), from.as_bytes(), to.as_bytes()) {
                    std::fs::remove_file(&path).map_err(|e| e.to_string())?;
                    std::os::unix::fs::symlink(String::from_utf8_lossy(&moved).as_ref(), &path)
                        .map_err(|e| e.to_string())?;
                }
            } else {
                let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
                if let Some(text) = replaced(&bytes, from.as_bytes(), to.as_bytes()) {
                    std::fs::write(&path, text).map_err(|e| e.to_string())?;
                }
            }
        }
    }
    Ok(())
}

/// Where `root` is named in `bytes`: each occurrence not followed by a digit, so a root
/// `…/op-1-2` is never found inside another world's `…/op-1-28`.
fn occurrences(bytes: &[u8], root: &[u8]) -> Vec<usize> {
    if root.is_empty() || bytes.len() < root.len() {
        return Vec::new();
    }
    (0..=bytes.len() - root.len())
        .filter(|&at| {
            &bytes[at..at + root.len()] == root
                && !bytes.get(at + root.len()).is_some_and(u8::is_ascii_digit)
        })
        .collect()
}

/// Whether `bytes` name the world root `root`.
pub fn names(bytes: &[u8], root: &str) -> bool {
    !occurrences(bytes, root.as_bytes()).is_empty()
}

/// `bytes` with every naming of the root `from` replaced by `to`, where they hold one.
fn replaced(bytes: &[u8], from: &[u8], to: &[u8]) -> Option<Vec<u8>> {
    let found = occurrences(bytes, from);
    if found.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut next = 0;
    for at in found {
        if at < next {
            continue;
        }
        out.extend_from_slice(&bytes[next..at]);
        out.extend_from_slice(to);
        next = at + from.len();
    }
    out.extend_from_slice(&bytes[next..]);
    Some(out)
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The item ids and the event count of each, from the item store's journal.
pub fn histories(world: &World) -> Result<std::collections::BTreeMap<String, usize>, String> {
    let journal = world.runs.join("human-items/events.ndjson");
    let text =
        std::fs::read_to_string(&journal).map_err(|e| format!("{}: {e}", journal.display()))?;
    let mut histories = std::collections::BTreeMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let record: Value = serde_json::from_str(line).map_err(|e| format!("{line}: {e}"))?;
        let id = record["payload"]["item_id"]
            .as_str()
            .ok_or_else(|| format!("an item event names no item: {line}"))?;
        *histories.entry(id.to_owned()).or_insert(0) += 1;
    }
    Ok(histories)
}

/// The id the store gives the item raised under `dedup_key`.
pub fn item_id(dedup_key: &str) -> String {
    let digest = Sha256::digest(dedup_key.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("hi-{}", &hex[..20])
}

/// What the fixture's files must hold, read without the binary: its runs, their
/// records and the longest item history.
fn check_files(world: &World, shape: &Shape) -> Result<(), String> {
    let mut runs = 0;
    let mut records = 0u64;
    for entry in std::fs::read_dir(&world.runs)
        .map_err(|e| e.to_string())?
        .flatten()
    {
        let journal = entry.path().join("events.jsonl");
        if !entry.path().join("launch.json").is_file() {
            continue;
        }
        runs += 1;
        let text =
            std::fs::read_to_string(&journal).map_err(|e| format!("{}: {e}", journal.display()))?;
        records += text.lines().filter(|l| !l.trim().is_empty()).count() as u64;
    }
    let histories = histories(world)?;
    let longest = histories.get(&item_id(generate::LONGEST_KEY)).copied();
    let most = histories.values().copied().max();
    let held = (runs, records, longest, most);
    let stated = (
        shape.runs,
        shape.run_records,
        Some(shape.longest_history),
        Some(shape.longest_history),
    );
    if held == stated {
        Ok(())
    } else {
        Err(format!(
            "the restored fixture is not the workload it states: (runs, run records, the \
             longest item's history, the most events of any item) are {held:?}, not {stated:?}"
        ))
    }
}
