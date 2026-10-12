//! Differential harness: serde_norway (today) against serde-saphyr 1.3 and saphyr 0.2,
//! over this repository's real YAML documents and its own types.
//!
//! Run: `cargo run --release --manifest-path spikes/saphyr-diff/Cargo.toml [-- HOST_ROOT]`
//! HOST_ROOT defaults to `/home/nick/projects/ai-orchestrator` and is read only.
//! Scratch files go under `$ONEPIPELINE_NODE_SCRATCH_DIR` (else the system temp dir).

mod corpus;
mod diff;
mod kinds;

use std::path::PathBuf;

fn main() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("repo");
    let host = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/nick/projects/ai-orchestrator"));
    let host = host.is_dir().then_some(host);
    let scratch = std::env::var_os("ONEPIPELINE_NODE_SCRATCH_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("saphyr-diff-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("scratch");

    println!("# saphyr-diff: serde_norway 0.9.42 (today) vs serde-saphyr 1.3.0 / saphyr 0.2.1");
    println!("repo: {}", repo.display());
    println!(
        "host: {}",
        host.as_ref().map_or("(absent)".into(), |h| h.display().to_string())
    );
    let corpus = corpus::Corpus::gather(&repo, host.as_deref());
    corpus.print_counts();

    let mut report = diff::Report::default();
    kinds::executor_rules(&corpus, &mut report);
    kinds::launch_configs(&corpus, &mut report);
    kinds::maintenance(&corpus, &mut report);
    kinds::templates(&corpus, &mut report);
    kinds::graphs(&corpus, &mut report);
    kinds::front_matter(&corpus, &mut report);
    kinds::contract_blocks(&corpus, &mut report);
    kinds::writeback_emission(&corpus, &scratch, &mut report);
    kinds::ci_workflows(&corpus, &mut report);
    kinds::malformed(&mut report);
    kinds::probes(&mut report);
    kinds::timing(&corpus);
    report.print();
    let _ = std::fs::remove_dir_all(&scratch);
}
