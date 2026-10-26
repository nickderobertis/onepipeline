//! The seeded journal generator: a synthesized run directory at a requested journal
//! size, with a 23-node plan on a local source and this host's event mix.
//!
//! Every figure that shapes the mix is an **aggregate** of this host's real journals,
//! measured read-only by `spike-driver-profile`'s calibration: record counts and byte sizes per source and
//! kind, record-size percentiles, and the skew between agentgraph records' stamps and
//! the pipeline records appended in front of them. No record, text, identifier or path
//! of a real journal is here; every byte this writes comes from the seed.
//!
//! Generation is a pure function of `(size, seed, world paths)`: one size twice from one
//! seed into the same world writes byte-identical files.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// SplitMix64: small, seedable, and the same on every platform.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next() % n
        }
    }
}

/// Words the synthesized prose is drawn from: neutral, and none of them from a journal.
const WORDS: &[&str] = &[
    "the",
    "driver",
    "node",
    "fixture",
    "branch",
    "check",
    "build",
    "test",
    "record",
    "journal",
    "worker",
    "judge",
    "plan",
    "task",
    "edge",
    "budget",
    "measure",
    "change",
    "request",
    "review",
    "commit",
    "gate",
    "lint",
    "coverage",
    "release",
    "binary",
    "harness",
    "double",
    "member",
    "turn",
    "envelope",
    "reply",
    "amend",
    "note",
    "status",
    "watch",
    "monitor",
    "pass",
    "fold",
    "checkpoint",
    "summary",
    "view",
    "run",
    "graph",
    "sample",
    "profile",
    "seed",
    "size",
    "event",
    "stream",
    "label",
    "payload",
    "output",
    "detail",
    "and",
    "of",
    "to",
    "with",
    "from",
    "into",
    "every",
    "each",
    "one",
    "two",
    "keeps",
    "reads",
    "writes",
    "holds",
    "settles",
    "dispatches",
    "publishes",
    "lands",
];

/// Prose of exactly `len` bytes.
pub fn prose(rng: &mut Rng, len: usize) -> String {
    let mut out = String::with_capacity(len + 16);
    while out.len() < len {
        if !out.is_empty() {
            out.push(if rng.below(14) == 0 { '\n' } else { ' ' });
        }
        out.push_str(WORDS[rng.below(WORDS.len() as u64) as usize]);
    }
    out.truncate(len);
    out
}

/// One calibrated kind: its bytes in the calibration journal and its record-size
/// percentiles (p50, p90, p95, p99, max).
pub struct Kind {
    pub source: &'static str,
    pub kind: &'static str,
    pub bytes: u64,
    pub sizes: [u64; 5],
}

/// The calibration: this host's largest journal (59,852,225 bytes, 79,870 records), per
/// kind, as `spike-driver-profile`'s calibration reported it. Only the kinds this generator writes are
/// listed; the bytes of the kinds it leaves out (records that move a run's sessions,
/// pushes, holds and envelope findings, 2.7% of the journal together, each under 1% of
/// it) go to the same source's other kinds under 1%, so each source keeps its share.
pub const CALIBRATION_BYTES: u64 = 59_852_225;
pub const CALIBRATION_SOURCES: [(&str, u64); 3] = [
    ("agentgraph", 50_211_002),
    ("pipeline", 6_378_757),
    ("vcs", 3_262_466),
];
/// The one record that is a per-run singleton: run-started, carrying the plan.
pub const RUN_STARTED_BYTES: u64 = 682_502;

pub const KINDS: &[Kind] = &[
    Kind {
        source: "agentgraph",
        kind: "turn-activity",
        bytes: 26_264_933,
        sizes: [759, 3_155, 4_750, 4_919, 6_696],
    },
    Kind {
        source: "agentgraph",
        kind: "member-heartbeat",
        bytes: 21_383_708,
        sizes: [427, 431, 435, 442, 442],
    },
    Kind {
        source: "agentgraph",
        kind: "turn-started",
        bytes: 1_212_783,
        sizes: [4_712, 4_736, 4_741, 4_756, 4_764],
    },
    Kind {
        source: "agentgraph",
        kind: "turn-message",
        bytes: 558_907,
        sizes: [2_701, 4_668, 4_675, 4_687, 4_694],
    },
    Kind {
        source: "agentgraph",
        kind: "turn-completed",
        bytes: 213_192,
        sizes: [670, 721, 723, 728, 734],
    },
    Kind {
        source: "agentgraph",
        kind: "member-settled",
        bytes: 128_751,
        sizes: [1_330, 1_454, 1_470, 2_221, 2_278],
    },
    Kind {
        source: "agentgraph",
        kind: "member-started",
        bytes: 97_996,
        sizes: [695, 706, 708, 712, 712],
    },
    Kind {
        source: "agentgraph",
        kind: "graph-started",
        bytes: 83_548,
        sizes: [594, 600, 604, 604, 609],
    },
    Kind {
        source: "agentgraph",
        kind: "graph-settled",
        bytes: 57_582,
        sizes: [453, 462, 465, 475, 481],
    },
    Kind {
        source: "pipeline",
        kind: "edit-committed",
        bytes: 4_213_745,
        sizes: [2_851, 82_027, 82_196, 88_845, 98_289],
    },
    Kind {
        source: "pipeline",
        kind: "planner-surfaced",
        bytes: 456_745,
        sizes: [1_052, 2_595, 2_824, 3_308, 3_663],
    },
    Kind {
        source: "pipeline",
        kind: "planner-surface-queued",
        bytes: 287_170,
        sizes: [933, 2_484, 2_705, 2_918, 3_643],
    },
    Kind {
        source: "pipeline",
        kind: "quiet-worker",
        bytes: 21_991,
        sizes: [298, 299, 299, 299, 305],
    },
    Kind {
        source: "vcs",
        kind: "change-check",
        bytes: 2_019_278,
        sizes: [503, 610, 623, 657, 706],
    },
    Kind {
        source: "vcs",
        kind: "fetch",
        bytes: 171_590,
        sizes: [484, 499, 500, 634, 642],
    },
    Kind {
        source: "vcs",
        kind: "gate-run",
        bytes: 136_823,
        sizes: [2_031, 2_833, 2_836, 2_836, 2_836],
    },
    Kind {
        source: "vcs",
        kind: "base-merge",
        bytes: 123_031,
        sizes: [552, 569, 634, 697, 697],
    },
    Kind {
        source: "vcs",
        kind: "change-drafted",
        bytes: 100_325,
        sizes: [583, 589, 590, 590, 595],
    },
    Kind {
        source: "vcs",
        kind: "change-opened",
        bytes: 92_778,
        sizes: [494, 501, 501, 607, 613],
    },
    Kind {
        source: "vcs",
        kind: "change-described",
        bytes: 63_365,
        sizes: [597, 611, 620, 633, 633],
    },
];

/// A kind's record size at quantile `u` of its calibrated percentiles: piecewise
/// linear between them, the lower half held just under the median, as the records
/// below it are.
fn size_at(u: f64, sizes: &[u64; 5]) -> u64 {
    let [p50, p90, p95, p99, max] = sizes.map(|s| s as f64);
    let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
    let size = if u < 0.5 {
        lerp(p50 * 0.9, p50, u / 0.5)
    } else if u < 0.9 {
        lerp(p50, p90, (u - 0.5) / 0.4)
    } else if u < 0.95 {
        lerp(p90, p95, (u - 0.9) / 0.05)
    } else if u < 0.99 {
        lerp(p95, p99, (u - 0.95) / 0.04)
    } else {
        lerp(p99, max, (u - 0.99) / 0.01)
    };
    size.round() as u64
}

/// Milliseconds since the epoch as the journal's RFC 3339 stamp, `…T…:…:….mmmZ`.
pub fn stamp(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rem / 3600,
        (rem / 60) % 60,
        rem % 60
    )
}

/// Where the history ends: every synthesized record is stamped before it, so a driver
/// adopting the run today writes after all of it.
pub const HISTORY_ENDS_MS: i64 = 1_790_812_800_000; // 2026-10-01T00:00:00Z

pub const NODES: usize = 23;
/// The history: nodes `t01`..`t17` dispatched and settled done.
pub const HISTORY_NODES: usize = 17;
/// The four nodes a driver adopting the run dispatches at once.
pub const LIVE_NODES: [&str; 4] = [
    "t18-synthesized-workload-node",
    "t19-synthesized-workload-node",
    "t20-synthesized-workload-node",
    "t21-synthesized-workload-node",
];
/// A node the history settled `complete-but-draft`: its change is a draft waiting on
/// a release that has not happened, as this host's long runs' nodes wait on other
/// repositories' releases. It can still move, so a driver whose four live dispatches
/// have settled stays with the run, dispatching nothing; the node behind it waits.
pub const DRAFTED: &str = "t22-synthesized-workload-node";
pub const LAST: &str = "t23-synthesized-workload-node";

/// A node's id: as long as this host's node ids run, so every record that names one is
/// the calibrated size.
pub fn node_id(index: usize) -> String {
    format!("t{:02}-synthesized-workload-node", index + 1)
}

/// The plan: 23 nodes, its task prose sized so its run-started record is the calibrated
/// per-run singleton's size. Returned as the store is given it.
pub fn plan(rng: &mut Rng, run: &str) -> Value {
    // The run-started record's envelope, labels, paths and per-task scaffolding come to
    // about 6 KB; the rest is the 23 tasks' prose.
    let per_task = ((RUN_STARTED_BYTES - 6_000) / NODES as u64) as usize - 120;
    let mut tasks = Vec::new();
    for index in 0..NODES {
        let id = node_id(index);
        let body = prose(rng, per_task);
        let task = format!(
            "## What\nDo {id}.\n{body}\n\n## Why\nSo the run can settle.\n\n## Acceptance criteria\n- {id} is done."
        );
        let deps: Vec<String> = match index {
            // The live four each wait on two settled history nodes.
            17..=20 => vec![node_id(index - 17), node_id(index - 10)],
            21 => vec![],
            22 => LIVE_NODES
                .iter()
                .chain([&DRAFTED])
                .map(|id| id.to_string())
                .collect(),
            // A history node waits on the one before it in its lane of four.
            i if (4..HISTORY_NODES).contains(&i) => vec![node_id(i - 4)],
            _ => vec![],
        };
        let mut node = json!({"id": id, "task": task, "persona": "engineer"});
        if !deps.is_empty() {
            node["deps"] = json!(deps);
        }
        tasks.push(node);
    }
    json!({
        "schema_version": onepipeline::plan::PLAN_SCHEMA_VERSION,
        "goal": {"text": format!("Deliver {run}")},
        "name": run,
        "concurrency": 4,
        "tasks": tasks,
    })
}

/// The plan as a run holds it: each task carrying the record the store keeps it as.
fn as_run_holds(plan: &Value, board: &str) -> Value {
    let mut plan = plan.clone();
    for (index, task) in plan["tasks"].as_array_mut().unwrap().iter_mut().enumerate() {
        let id = task["id"].as_str().unwrap().to_owned();
        task["task_record"] = json!({"id": format!("{board}/{index:03}-{id}"), "title": ""});
    }
    plan
}

/// Where a generated run lives and what its launch record names.
pub struct Paths {
    pub runs: PathBuf,
    pub project: PathBuf,
    pub dag_graph: PathBuf,
    pub node_graph: PathBuf,
    /// The plan's project in the world's store, `plans:<board>`.
    pub store_project: String,
    pub session: String,
    pub host: String,
}

#[derive(Debug)]
pub struct Generated {
    pub dir: PathBuf,
    pub records: u64,
    pub bytes: u64,
    pub envelopes: u64,
    pub episodes: u64,
    pub stalls: Vec<(i64, i64)>,
    pub history: (String, String),
}

/// One record before it is written: when the driver appended it, and its envelope.
struct Pending {
    append_ms: i64,
    own: bool,
    order: u64,
    record: Value,
}

/// The driver's stalls: windows in which it is inside one long pass and appends
/// nothing, then appends one record of its own and every relayed record emitted
/// meanwhile, so a relayed record's skew is how long it waited. Each entry is the
/// fraction of the history spent in stalls of that length, solved from the
/// calibration's skew percentiles (p90 789 s, p95 1,596 s, p99 2,759 s, max 5,264 s) as
/// a mixture of the uniform lags a stall gives; the longest is a little past the
/// calibration's greatest skew, because the first record it holds back is emitted up
/// to one heartbeat after it starts.
const STALLS: [(f64, i64); 3] = [(0.021, 5_280_000), (0.0866, 2_700_000), (0.061, 1_200_000)];

/// One dispatch of a history node: its stream and its window.
struct Episode {
    node: usize,
    stream: String,
    from: i64,
    to: i64,
    beats: i64,
}

/// Write the run directory `<runs>/<run>` for a journal of about `size` bytes, the
/// store's plan having been written already under `paths.store_project`.
pub fn generate(paths: &Paths, run: &str, plan_doc: &Value, size: u64, seed: u64) -> Generated {
    let mut rng = Rng::new(seed.wrapping_mul(1_000_003).wrapping_add(size));
    let dir = paths.runs.join(run);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("channel")).unwrap();
    let board = paths
        .store_project
        .split_once(':')
        .map_or("", |(_, board)| board);
    let held = as_run_holds(plan_doc, board);
    let budget = size.saturating_sub(RUN_STARTED_BYTES).max(100_000);

    // Per-kind byte targets, of the journal beside the run-started singleton. A kind
    // holding at least 1% of the calibration's bytes gets its calibrated share; the rest
    // of its source's share — which includes the bytes of the kinds this generator
    // leaves out, and comes to the seventeen history nodes' fixed records too — is shared
    // among that source's smaller kinds by their calibrated bytes, so every source keeps
    // its calibrated share as well. A kind's sizes are its calibrated percentiles taken
    // at evenly spaced quantiles in a seeded order, then scaled so the kind meets its
    // target exactly: at a few records a heavy-tailed kind would otherwise land far from
    // its share by the luck of one draw. Heartbeats are not scaled, because nothing in
    // one is free text: their size is their labels', which are this host's lengths.
    let other = (CALIBRATION_BYTES - RUN_STARTED_BYTES) as f64;
    // The seventeen history nodes' and the drafted node's ready, dispatched and settled
    // records: about 730 B a node, the ready one about 225 B of it. In a journal so
    // small that they alone would crowd the pipeline source past its tolerance beside
    // its edits — 1 MB, where everything but the run-started singleton is 0.3 MB — the
    // history nodes' ready records are left out, as a run's journal may well not carry
    // one for every node. Nothing changes at a size where they fit.
    let pipeline_target =
        (CALIBRATION_SOURCES[1].1 - RUN_STARTED_BYTES) as f64 / other * budget as f64;
    let edits_target = 4_213_745.0 / other * budget as f64;
    let lean = edits_target + (HISTORY_NODES + 1) as f64 * 730.0 > 0.97 * pipeline_target;
    let fixed_pipeline = if lean {
        HISTORY_NODES as f64 * 505.0 + 730.0
    } else {
        (HISTORY_NODES + 1) as f64 * 730.0
    };
    let mut sizes: BTreeMap<&str, Vec<u64>> = BTreeMap::new();
    for (source, source_bytes) in CALIBRATION_SOURCES {
        let source_bytes = if source == "pipeline" {
            source_bytes - RUN_STARTED_BYTES
        } else {
            source_bytes
        };
        let source_target = source_bytes as f64 / other * budget as f64;
        let mine: Vec<&Kind> = KINDS.iter().filter(|k| k.source == source).collect();
        let big = |k: &Kind| k.bytes as f64 / other >= 0.01;
        let big_target: f64 = mine
            .iter()
            .filter(|k| big(k))
            .map(|k| k.bytes as f64 / other * budget as f64)
            .sum();
        let small_bytes: f64 = mine
            .iter()
            .filter(|k| !big(k))
            .map(|k| k.bytes as f64)
            .sum();
        let fixed = if source == "pipeline" {
            fixed_pipeline
        } else {
            0.0
        };
        let small_target = (source_target - big_target - fixed).max(0.0);
        // In a journal so small that the fixed records crowd the source past its share,
        // the large kinds give up to 9% of theirs, which keeps both inside the tolerance.
        let overflow = (big_target + fixed - source_target).max(0.0);
        let give = if big_target > 0.0 {
            (overflow / big_target).min(0.09)
        } else {
            0.0
        };
        for kind in mine {
            let target = if big(kind) {
                kind.bytes as f64 / other * budget as f64 * (1.0 - give)
            } else {
                small_target * kind.bytes as f64 / small_bytes
            };
            // The mean the quantile model gives, so a count of records at it meets the
            // target with the sizes barely scaled.
            let model_mean = (0..1_000)
                .map(|i| size_at((i as f64 + 0.5) / 1_000.0, &kind.sizes) as f64)
                .sum::<f64>()
                / 1_000.0;
            let n = (target / model_mean).round() as usize;
            let n = if kind.kind == "edit-committed" {
                n.max(1)
            } else {
                n
            };
            let mut quantiles: Vec<f64> = (0..n).map(|i| (i as f64 + 0.5) / n as f64).collect();
            for i in (1..quantiles.len()).rev() {
                quantiles.swap(i, rng.below(i as u64 + 1) as usize);
            }
            let mut drawn: Vec<u64> = quantiles.iter().map(|&u| size_at(u, &kind.sizes)).collect();
            if kind.kind != "member-heartbeat" && !drawn.is_empty() {
                let scale = target / drawn.iter().sum::<u64>() as f64;
                for size in &mut drawn {
                    *size = ((*size as f64) * scale).round() as u64;
                }
            }
            sizes.insert(kind.kind, drawn);
        }
    }

    // The history: every graph-started record opens one dispatch of one history node,
    // in four lanes. A dispatch's one heartbeating member beats every 15 s, so a
    // dispatch lasts as long as its share of the heartbeats; the lanes are spread over a
    // history long enough for about 3.2 of them to be heartbeating at once, as this
    // host's journals show, and never shorter than twice the longest stall.
    let beats = sizes["member-heartbeat"].len() as i64;
    let episodes_n = sizes["graph-started"].len().max(HISTORY_NODES);
    let weights: Vec<f64> = (0..episodes_n).map(|_| 0.5 + rng.unit()).collect();
    let weight_sum: f64 = weights.iter().sum();
    let mut carried = 0.0;
    let episode_beats: Vec<i64> = weights
        .iter()
        .map(|w| {
            carried += w / weight_sum * beats as f64;
            let n = carried.floor();
            carried -= n;
            n as i64
        })
        .collect();
    let busy_ms: i64 = episode_beats.iter().map(|n| n * 15_000 + 30_000).sum();
    let span = (busy_ms * 10 / 32).max(2 * STALLS[0].1 + 3_600_000);
    let start = HISTORY_ENDS_MS - span;
    let end = HISTORY_ENDS_MS - 60_000;
    let mut windows = Vec::new();
    for lane in 0..4usize {
        let mine: Vec<usize> = (lane..episodes_n).step_by(4).collect();
        let lane_busy: i64 = mine
            .iter()
            .map(|&e| episode_beats[e] * 15_000 + 30_000)
            .sum();
        let idle = (end - start - 120_000 - lane_busy).max(0);
        let gap = idle / (mine.len() as i64 + 1);
        let mut at = start + 60_000;
        for &e in &mine {
            at += (gap as f64 * (0.5 + rng.unit())) as i64;
            let length = episode_beats[e] * 15_000 + 30_000;
            windows.push((at, at + length, e));
            at += length;
        }
    }
    windows.sort();
    let episodes: Vec<Episode> = windows
        .iter()
        .map(|&(from, to, e)| Episode {
            node: ((((from - start) as f64) / span as f64) * HISTORY_NODES as f64) as usize
                % HISTORY_NODES,
            stream: format!("node-scope-{from}-{}", 100_000 + rng.below(3_900_000)),
            from,
            to: to.min(end - 1_000),
            beats: episode_beats[e],
        })
        .collect();

    // Relayed agentgraph records, each with its emission instant.
    struct Relayed {
        ts: i64,
        episode: usize,
        kind: &'static str,
        member: Option<&'static str>,
        size: u64,
    }
    let mut relayed: Vec<Relayed> = Vec::new();
    let mut beat_sizes = sizes["member-heartbeat"].clone().into_iter();
    for (index, episode) in episodes.iter().enumerate() {
        for i in 0..episode.beats {
            let Some(size) = beat_sizes.next() else { break };
            let ts = episode.from + 15_000 * (i + 1) + rng.below(300) as i64;
            relayed.push(Relayed {
                ts: ts.min(episode.to - 3),
                episode: index,
                kind: "member-heartbeat",
                member: Some("worker"),
                size,
            });
        }
    }
    for kind in KINDS
        .iter()
        .filter(|k| k.source == "agentgraph" && k.kind != "member-heartbeat")
    {
        for (nth, &size) in sizes[kind.kind].iter().enumerate() {
            // Opening and closing records go one to an episode first, in order.
            let index = match kind.kind {
                "graph-started" | "member-started" | "graph-settled" | "member-settled"
                    if nth < episodes.len() =>
                {
                    nth
                }
                _ => rng.below(episodes.len() as u64) as usize,
            };
            let episode = &episodes[index];
            let width = (episode.to - episode.from).max(10_000);
            let ts = match kind.kind {
                "graph-started" => episode.from,
                "member-started" => episode.from + 1,
                "turn-started" => episode.from + 2 + rng.below(width as u64 / 2) as i64,
                "member-settled" => episode.to - 2,
                "graph-settled" => episode.to - 1,
                _ => episode.from + 3 + rng.below(width as u64 - 6) as i64,
            };
            let member = match kind.kind {
                "graph-started" | "graph-settled" => None,
                _ => Some("worker"),
            };
            relayed.push(Relayed {
                ts,
                episode: index,
                kind: kind.kind,
                member,
                size,
            });
        }
    }
    relayed.sort_by_key(|r| (r.ts, r.episode));

    // Stalls, the longest placed first and always, starting at a relayed record's
    // emission so the journal's greatest skew is that stall's length.
    let mut stalls: Vec<(i64, i64)> = Vec::new();
    let first_ok: Vec<i64> = relayed
        .iter()
        .map(|r| r.ts)
        .filter(|&ts| ts > start + 60_000 && ts + STALLS[0].1 < end - 60_000)
        .collect();
    if !first_ok.is_empty() {
        // Where the history has records to spare, anywhere; where it is short, at the
        // record whose stall would hold back the fewest others, so one long stall does
        // not stand for the whole of a small journal's skew.
        let at = if span > 20 * STALLS[0].1 {
            first_ok[rng.below(first_ok.len() as u64) as usize]
        } else {
            let times: Vec<i64> = relayed.iter().map(|r| r.ts).collect();
            *first_ok
                .iter()
                .min_by_key(|&&ts| {
                    times.partition_point(|&t| t < ts + STALLS[0].1)
                        - times.partition_point(|&t| t < ts)
                })
                .unwrap()
        } - 1;
        stalls.push((at, at + STALLS[0].1));
    }
    for (index, (fraction, length)) in STALLS.iter().enumerate() {
        let wanted = ((span as f64 * fraction) / *length as f64).round() as usize;
        let mut placed = usize::from(index == 0 && !stalls.is_empty());
        let mut tries = 0;
        while placed < wanted && tries < 100_000 {
            tries += 1;
            let from = start + 60_000 + rng.below((span - length - 180_000).max(1) as u64) as i64;
            let to = from + length;
            if to >= end
                || stalls
                    .iter()
                    .any(|&(a, b)| from < b + 60_000 && to + 60_000 > a)
            {
                continue;
            }
            stalls.push((from, to));
            placed += 1;
        }
    }
    stalls.sort();
    let stalled = |ts: i64| {
        stalls
            .iter()
            .find(|&&(from, to)| ts >= from && ts < to)
            .map(|&(_, to)| to)
    };

    let mut pending: Vec<Pending> = Vec::new();
    let mut order = 0u64;
    let mut seqs: BTreeMap<usize, u64> = BTreeMap::new();
    for r in relayed {
        let episode = &episodes[r.episode];
        let seq = seqs.entry(r.episode).or_insert(0);
        *seq += 1;
        let node = node_id(episode.node);
        let mut labels = json!({"run_id": episode.stream, "node": node});
        if let Some(member) = r.member {
            labels["member"] = json!(member);
        }
        labels["persona"] = json!("engineer");
        labels["onepipeline.run_id"] = json!(run);
        labels["onepipeline.node"] = json!(node);
        labels["onepipeline.persona"] = json!("engineer");
        if matches!(
            r.kind,
            "turn-activity" | "turn-started" | "turn-message" | "turn-completed"
        ) {
            labels["session"] = json!(format!("{}.worker", episode.stream));
        }
        let mut record = json!({
            "v": 1, "ts": stamp(r.ts), "stream": episode.stream, "seq": *seq, "source": "agentgraph",
            "kind": r.kind, "labels": labels, "payload": agentgraph_payload(r.kind, r.ts, &mut rng), "artifacts": [],
        });
        pad(&mut record, r.size, &mut rng);
        let append = stalled(r.ts).unwrap_or(r.ts + rng.below(1_500) as i64);
        order += 1;
        pending.push(Pending {
            append_ms: append,
            own: false,
            order,
            record,
        });
    }

    // Relayed vcs records, on each episode's own session stream, written by the
    // session's own library rather than relayed after a pass.
    let mut vcs: Vec<(i64, usize, &'static str, u64)> = Vec::new();
    for kind in KINDS.iter().filter(|k| k.source == "vcs") {
        for &size in &sizes[kind.kind] {
            let index = rng.below(episodes.len() as u64) as usize;
            let episode = &episodes[index];
            let ts = episode.from + rng.below((episode.to - episode.from).max(1) as u64) as i64;
            vcs.push((ts, index, kind.kind, size));
        }
    }
    vcs.sort();
    let mut vcs_seqs: BTreeMap<usize, u64> = BTreeMap::new();
    for (ts, index, kind, size) in vcs {
        let episode = &episodes[index];
        let seq = vcs_seqs.entry(index).or_insert(0);
        *seq += 1;
        let session = format!(
            "s-{:012x}",
            (index as u64 + 1).wrapping_mul(0x9E37_79B9) & 0xFFFF_FFFF_FFFF
        );
        let significance =
            ["progress", "progress", "progress", "progress", "milestone"][rng.below(5) as usize];
        let phase =
            ["review", "review", "review", "development", "integrate"][rng.below(5) as usize];
        let mut record = json!({
            "v": 1, "ts": stamp(ts), "stream": session, "seq": *seq, "source": "vcs", "kind": kind,
            "phase": phase,
            "labels": {"run_id": run, "node": node_id(episode.node), "persona": "engineer",
                       "identity": "service", "session": session, "significance": significance},
            "payload": vcs_payload(kind, &mut rng), "artifacts": [],
        });
        pad(&mut record, size, &mut rng);
        let append = stalled(ts).unwrap_or(ts);
        order += 1;
        pending.push(Pending {
            append_ms: append,
            own: false,
            order,
            record,
        });
    }

    // The driver's own records: the run's start, each history node's life, the amends
    // the run's envelopes committed, the surfaces it raised, and one record at the end of
    // every stall. A driver inside a stall writes nothing until it ends.
    let mut own: Vec<(i64, &'static str, Value, Value, u64)> = Vec::new();
    for node in 0..HISTORY_NODES {
        let mine: Vec<&Episode> = episodes.iter().filter(|e| e.node == node).collect();
        let (from, to) = match (mine.first(), mine.last()) {
            (Some(first), Some(last)) => (first.from, last.to),
            _ => {
                let at = start + (span / HISTORY_NODES as i64) * node as i64 + 1_000;
                (at, at + 60_000)
            }
        };
        let labels = json!({"run_id": run, "node": node_id(node)});
        if !lean {
            own.push((from - 300, "node-ready", labels.clone(), json!({}), 0));
        }
        own.push((
            from - 200,
            "node-dispatched",
            labels.clone(),
            json!({"persona": "engineer", "attempt": 1}),
            0,
        ));
        own.push((
            to + 300,
            "node-settled",
            labels,
            json!({"status": "done"}),
            0,
        ));
    }
    // The drafted node: dispatched in the history's last quarter, its change left a
    // draft for a release.
    let drafted_at = start + span * 3 / 4;
    let labels = json!({"run_id": run, "node": DRAFTED});
    own.push((drafted_at, "node-ready", labels.clone(), json!({}), 0));
    own.push((
        drafted_at + 100,
        "node-dispatched",
        labels.clone(),
        json!({"persona": "engineer", "attempt": 1}),
        0,
    ));
    own.push((
        drafted_at + (span / 8).min(3_600_000),
        "node-settled",
        labels,
        json!({"status": "complete-but-draft", "outcome": "change-draft"}),
        0,
    ));
    let amends = sizes["edit-committed"].clone();
    let mut amend_times: Vec<i64> = amends
        .iter()
        .map(|_| start + 120_000 + rng.below((span - 240_000) as u64) as i64)
        .collect();
    amend_times.sort();
    let mut envelopes = Vec::new();
    for (id, (&size, &ts)) in amends.iter().zip(&amend_times).enumerate() {
        // Each amend is of a node not yet dispatched, carrying its whole task.
        let target = [
            LAST,
            LIVE_NODES[0],
            LIVE_NODES[1],
            LIVE_NODES[2],
            LIVE_NODES[3],
        ][rng.below(5) as usize];
        // The text appears twice in the record, in the command and in its operation, so
        // it is sized to half of what the record's scaffolding leaves.
        let payload = |text: &str| {
            json!({
                "author": "planner", "command": {"op": "amend", "id": target, "text": text},
                "operations": [{"kind": "task-amended", "node": target, "text": text}],
                "operation_kinds": ["task-amended"], "envelope": id,
            })
        };
        let scaffold =
            format!("## What\nDo {target}.\n\n\n## Acceptance criteria\n- {target} is done.");
        let empty = serde_json::to_string(&payload(&scaffold)).unwrap().len() + 260;
        let body = prose(&mut rng, (size as usize).saturating_sub(empty) / 2);
        let text =
            format!("## What\nDo {target}.\n{body}\n\n## Acceptance criteria\n- {target} is done.");
        envelopes.push((
            id as u64,
            json!({"op": "amend", "id": target, "text": text}),
        ));
        own.push((
            ts,
            "edit-committed",
            json!({"run_id": run}),
            payload(&text),
            0,
        ));
    }
    for &size in &sizes["planner-surface-queued"] {
        let ts = start + 60_000 + rng.below((span - 120_000) as u64) as i64;
        own.push((
            ts,
            "planner-surface-queued",
            json!({"run_id": run}),
            json!({"kind": "finding", "source": "monitor", "blocking": false, "message": ""}),
            size,
        ));
    }
    for &size in &sizes["planner-surfaced"] {
        let ts = start + 60_000 + rng.below((span - 120_000) as u64) as i64;
        own.push((ts, "planner-surfaced", json!({"run_id": run}), json!({"kind": "finding", "source": "monitor", "blocking": false, "message": "", "queued_at": stamp(ts - 2_000)}), size));
    }
    let quiet = sizes["quiet-worker"].clone();
    let ends: Vec<i64> = stalls.iter().map(|&(_, to)| to).collect();
    for index in 0..quiet.len().max(ends.len()) {
        let ts = ends
            .get(index)
            .copied()
            .unwrap_or_else(|| start + 60_000 + rng.below((span - 120_000) as u64) as i64);
        let node = node_id(rng.below(HISTORY_NODES as u64) as usize);
        own.push((
            ts,
            "quiet-worker",
            json!({"run_id": run, "node": node}),
            json!({"persona": "engineer", "quiet_for_seconds": 1_800, "threshold_seconds": 1_800}),
            quiet.get(index).copied().unwrap_or(0),
        ));
    }
    let mut own: Vec<(i64, i64, &'static str, Value, Value, u64)> = own
        .into_iter()
        .map(|(ts, kind, labels, payload, size)| {
            (stalled(ts).unwrap_or(ts), ts, kind, labels, payload, size)
        })
        .collect();
    own.sort_by_key(|r| (r.0, r.1));
    // run-started, on its launching process's own stream, then one driver's stream.
    let launcher = format!("{}-{}", paths.host, 1_000 + rng.below(9_000));
    let driver = format!("{}-{}", paths.host, 10_000 + rng.below(90_000));
    order += 1;
    pending.push(Pending {
        append_ms: start,
        own: true,
        order,
        record: json!({
            "v": 2, "ts": stamp(start), "stream": launcher, "seq": 0, "source": "pipeline", "kind": "run-started",
            "labels": {"run_id": run},
            "payload": {"plan": held, "graph": paths.dag_graph, "dir": paths.project, "heartbeat_interval": 1_800, "owed_until_closed": true},
            "artifacts": [],
        }),
    });
    for (seq, (append, _, kind, labels, payload, size)) in own.into_iter().enumerate() {
        let mut record = json!({
            "v": 2, "ts": stamp(append), "stream": driver, "seq": seq, "source": "pipeline", "kind": kind,
            "labels": labels, "payload": payload, "artifacts": [],
        });
        if size > 0 {
            pad(&mut record, size, &mut rng);
        }
        order += 1;
        pending.push(Pending {
            append_ms: append,
            own: true,
            order,
            record,
        });
    }

    // The file's order is the append order; a driver's own record at a stall's end is
    // appended ahead of the burst it relays after its pass.
    pending.sort_by_key(|p| (p.append_ms, !p.own, p.order));
    let mut journal = std::io::BufWriter::with_capacity(
        1 << 20,
        std::fs::File::create(dir.join("events.jsonl")).unwrap(),
    );
    let mut bytes = 0u64;
    for entry in &pending {
        let line = serde_json::to_string(&entry.record).unwrap();
        bytes += line.len() as u64 + 1;
        journal.write_all(line.as_bytes()).unwrap();
        journal.write_all(b"\n").unwrap();
    }
    journal.flush().unwrap();

    std::fs::write(
        dir.join("plan.json"),
        serde_json::to_string_pretty(&held).unwrap(),
    )
    .unwrap();
    write_channel(&dir, &envelopes);
    write_launch(&dir, paths, run, start);

    Generated {
        dir,
        records: pending.len() as u64,
        bytes,
        envelopes: envelopes.len() as u64,
        episodes: episodes.len() as u64,
        stalls,
        history: (stamp(start), stamp(end)),
    }
}

fn agentgraph_payload(kind: &str, ts: i64, rng: &mut Rng) -> Value {
    match kind {
        "turn-activity" => match rng.below(3) {
            0 => {
                json!({"kind": "tool_call", "name": "bash", "detail": "", "tool_call_id": format!("toolu_{}", rng.below(999)), "index": rng.below(400)})
            }
            1 => {
                json!({"kind": "tool_result", "name": "bash", "detail": "", "output": "", "tool_call_id": format!("toolu_{}", rng.below(999)), "index": rng.below(400)})
            }
            _ => {
                json!({"kind": "reasoning", "name": null, "detail": "", "output": "", "index": rng.below(400)})
            }
        },
        "turn-started" => {
            json!({"turn": 1 + rng.below(6), "role": "assistant", "instruction": "", "started_at": stamp(ts), "origin": "task"})
        }
        "turn-message" => {
            json!({"turn": 1 + rng.below(6), "role": "assistant", "text": "", "truncated": false})
        }
        "turn-completed" => json!({"turn": 1 + rng.below(6), "role": "assistant",
            "usage": {"input_tokens": rng.below(90_000), "output_tokens": rng.below(9_000), "cache_read_tokens": rng.below(900_000), "cache_write_tokens": rng.below(9_000), "cost_usd": 0.0},
            "started_at": stamp(ts - 600_000), "finished_at": stamp(ts)}),
        "member-started" => {
            json!({"runner": "library", "engine": "oneharness", "config": "", "worktree": ""})
        }
        "member-settled" => {
            json!({"completed": true, "verdict": [], "completion_reason": "", "report_path": null})
        }
        "graph-started" => json!({"graph": "", "name": "node-scope", "dir": ""}),
        "graph-settled" => json!({"exit_code": 0, "members": {"worker": "settled"}}),
        _ => json!({}),
    }
}

fn vcs_payload(kind: &str, rng: &mut Rng) -> Value {
    match kind {
        "change-check" => {
            json!({"name": format!("check-{}", rng.below(12)), "status": "completed", "from_status": "in_progress", "state": "success", "conclusion": "success", "required": true})
        }
        "fetch" => json!({"checkout": "", "remote": "origin"}),
        "gate-run" => {
            json!({"gate": "pre-push", "attempt": 1, "verdict": "passed", "seconds": rng.below(900), "started_at": "", "ended_at": "", "checks": []})
        }
        "base-merge" => json!({"base": "main", "branch": "", "decision": "merged", "because": ""}),
        "change-drafted" => {
            json!({"base": "main", "id": rng.below(2_000), "kind": "draft", "url": ""})
        }
        "change-opened" => {
            json!({"author": "", "base": "main", "host": "github", "id": rng.below(2_000), "url": ""})
        }
        "change-described" => {
            json!({"artifact": "", "base": "main", "id": rng.below(2_000), "title": "", "url": ""})
        }
        _ => json!({}),
    }
}

/// Grow the record's one free-text field until its line is `size` bytes.
fn pad(record: &mut Value, size: u64, rng: &mut Rng) {
    let base = serde_json::to_string(record).unwrap().len() as u64;
    if size <= base {
        return;
    }
    let field = match record["kind"].as_str().unwrap_or_default() {
        "turn-activity" => "detail",
        "turn-started" => "instruction",
        "turn-message" => "text",
        "member-settled" => "completion_reason",
        "member-started" => "worktree",
        "graph-started" => "dir",
        "planner-surfaced" | "planner-surface-queued" => "message",
        "change-check" => "from_status",
        "fetch" => "checkout",
        "gate-run" => "ended_at",
        "base-merge" => "because",
        "change-drafted" | "change-opened" | "change-described" => "url",
        "quiet-worker" => "persona",
        _ => return,
    };
    let current = record["payload"][field]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let text = format!("{current}{}", prose(rng, (size - base) as usize));
    record["payload"][field] = Value::String(text);
    // A newline is two bytes once escaped: trim back to the size asked for.
    loop {
        let over = serde_json::to_string(record).unwrap().len() as i64 - size as i64;
        if over <= 0 {
            break;
        }
        let mut text = record["payload"][field].as_str().unwrap().to_owned();
        let keep = text
            .len()
            .saturating_sub((over as usize).div_ceil(2).max(1));
        text.truncate(keep);
        record["payload"][field] = Value::String(text);
    }
}

/// The channel's queue: every amend the history committed arrived as one envelope,
/// was claimed, and answered.
fn write_channel(dir: &Path, envelopes: &[(u64, Value)]) {
    let channel = dir.join("channel");
    let mut commands = String::new();
    let mut outcomes = String::new();
    for (id, command) in envelopes {
        commands.push_str(
            &serde_json::to_string(&json!({"id": id, "author": "planner", "commands": [command]}))
                .unwrap(),
        );
        commands.push('\n');
        outcomes.push_str(&serde_json::to_string(&json!({"id": id, "applied": true, "results": [{"index": 0, "op": "amend", "outcome": "applied"}]})).unwrap());
        outcomes.push('\n');
    }
    std::fs::write(channel.join("commands.jsonl"), commands).unwrap();
    std::fs::write(channel.join("command-outcomes.jsonl"), outcomes).unwrap();
    std::fs::write(
        channel.join("commands-cursor.json"),
        envelopes.len().to_string(),
    )
    .unwrap();
    std::fs::write(
        channel.join("deliveries.json"),
        "{\n  \"schema_version\": 1,\n  \"deliveries\": []\n}",
    )
    .unwrap();
}

/// The launch record of a driver that is gone: its pid stamped with a start no process
/// on this host has, so an adopter finds nothing driving the run.
fn write_launch(dir: &Path, paths: &Paths, run: &str, started: i64) {
    let launch = json!({
        "run_id": run,
        "project": paths.store_project,
        "dir": paths.project,
        "graph": paths.dag_graph,
        "graph_run": format!("dag-scope-{started}-4194000"),
        "observer_runs": [format!("dag-scope-{started}-4194000")],
        "node_graph": paths.node_graph,
        "launcher": "e2e",
        "session": paths.session,
        "pid": 4_194_000,
        "host": paths.host,
        "started": "linux-proc-stat:1",
        "started_at": stamp(started),
        "heartbeat_interval": 1_800,
        "writeback_item_budget": 12,
        "adoptions": 0,
        "branch_template": "{% if task.key %}{{ task.key }}{% else %}{{ plan.name }}{% endif %}/{{ node.id }}",
        "oneharness_sessions": dir.join("oneharness-sessions.jsonl"),
    });
    std::fs::write(
        dir.join("launch.json"),
        serde_json::to_string_pretty(&launch).unwrap(),
    )
    .unwrap();
}
