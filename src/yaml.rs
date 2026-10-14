//! The one YAML reader and writer the engine reads and writes its documents through.
//!
//! Every document is read under one set of `serde-saphyr` options and written under one, so
//! an executor-rules file, a launch config, a schedule, a registration and a graph each read
//! as the others do, and a shadow store document reads back the way it was written. What is
//! configured here is the fleet's: `true`/`false` alone are booleans (so `on:` and `yes` stay
//! strings), `<<` is an ordinary key, and an error is one line ending in its line and column.
//! Nothing here names a `serde-saphyr` type in a public item; callers see a `String` refusal.

use serde::de::DeserializeOwned;
use serde::Serialize;

/// Read one YAML document as `T`, refused with the parser's own message: what failed, and the
/// line and column it failed at.
pub(crate) fn from_str<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    let options = serde_saphyr::options! {
        strict_booleans: true,
        merge_keys: serde_saphyr::MergeKeyPolicy::AsOrdinary,
        with_snippet: false,
    };
    serde_saphyr::from_str_with_options(text, options).map_err(|error| error.to_string())
}

/// Write `value` as one block-style YAML document, ending in a newline.
///
/// A long string stays on its one line rather than folding to `>-`: a reader that edits the
/// document in place reads it the same either way, and a person reading it reads one line.
pub(crate) fn to_string<T: Serialize>(value: &T) -> Result<String, String> {
    let options = serde_saphyr::ser_options! {
        folded_wrap_chars: usize::MAX,
    };
    serde_saphyr::to_string_with_options(value, options).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use serde_json::{json, Value};

    use crate::filter::LaunchConfig;
    use crate::maintenance::MaintenanceConfig;
    use crate::rules::ExecutorRules;

    /// What each document below read as on the base this reader replaced, captured there
    /// through the same loaders and committed.
    const EXPECTED: &str = include_str!("../tests/fixtures/real-documents/expected.json");

    fn repo() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    fn fixture(relative: &str) -> PathBuf {
        repo().join("tests/fixtures/real-documents").join(relative)
    }

    /// Every fenced `yaml` block of `markdown`, in order.
    fn fenced_yaml(markdown: &str) -> Vec<String> {
        let mut blocks = Vec::new();
        let mut open: Option<String> = None;
        for line in markdown.lines() {
            match open.as_mut() {
                None if line.trim_start() == "```yaml" => open = Some(String::new()),
                Some(_) if line.trim_start().starts_with("```") => {
                    blocks.extend(open.take());
                }
                Some(block) => {
                    block.push_str(line);
                    block.push('\n');
                }
                None => {}
            }
        }
        blocks
    }

    /// A directory of this test's own, holding `file` as `text`.
    fn written(dir: &Path, file: &str, text: &str) -> PathBuf {
        std::fs::create_dir_all(dir).expect("a scratch directory");
        let path = dir.join(file);
        std::fs::write(&path, text).expect("the document is written");
        path
    }

    fn launch(path: &Path) -> Value {
        json!(LaunchConfig::load(path)
            .unwrap_or_else(|error| panic!("{} is refused: {error}", path.display())))
    }

    fn maintenance(path: &Path) -> Value {
        json!(MaintenanceConfig::load("--maintenance-config", path)
            .unwrap_or_else(|error| panic!("{} is refused: {error}", path.display())))
    }

    fn rules(path: &Path) -> Value {
        json!(ExecutorRules::load(path)
            .unwrap_or_else(|error| panic!("{} is refused: {error}", path.display())))
    }

    fn registry(root: &Path) -> Value {
        let registry = crate::templates::Registry::load(Some(root))
            .unwrap_or_else(|error| panic!("{} is refused: {error}", root.display()));
        registry
            .names()
            .iter()
            .map(|registered| {
                json!({
                    "name": registered.name,
                    "role": registered.role,
                    "description": registered.description,
                })
            })
            .collect()
    }

    fn graph(path: &Path) -> Value {
        let reference = path.to_str().expect("a UTF-8 path");
        json!(
            crate::graph::validate_graph_sets(reference, &[], "real document")
                .unwrap_or_else(|error| panic!("{reference} is refused: {error}"))
        )
    }

    fn plan(store: &Path, project: &str) -> Value {
        let read = crate::taskgraph::Store::at(store)
            .read_plan(&project.parse().expect("a qualified project"))
            .unwrap_or_else(|_| panic!("{project} does not read"));
        json!({
            "plan": read.plan,
            "metadata": read.metadata,
            "project_metadata": read.project_metadata,
        })
    }

    /// Every committed real document, read through the loader a run reads it with, as the
    /// values it read.
    fn read_real_documents(scratch: &Path) -> BTreeMap<String, Value> {
        let mut read = BTreeMap::new();
        read.insert(
            "examples/executors.yaml".to_owned(),
            rules(&repo().join("examples/executors.yaml")),
        );
        for name in ["graphs/dag-scope.yaml", "graphs/node-scope.yaml"] {
            read.insert(name.to_owned(), graph(&repo().join(name)));
        }
        for project in ["plans:single-node", "plans:tracked-release"] {
            read.insert(
                format!("examples/plan-store {project}"),
                plan(&repo().join("examples/plan-store"), project),
            );
        }
        read.insert(
            "host onepipeline.maintenance.yaml".to_owned(),
            maintenance(&fixture("onepipeline.maintenance.yaml")),
        );
        read.insert(
            "host templates.yaml".to_owned(),
            registry(&fixture("templates")),
        );
        let contract =
            std::fs::read_to_string(repo().join("docs/contract.md")).expect("the contract");
        for (index, block) in fenced_yaml(&contract).iter().enumerate() {
            let name = format!("docs/contract.md yaml block {}", index + 1);
            let dir = scratch.join(format!("block-{}", index + 1));
            let value = if block.contains("schema_version:") {
                let as_json: Value = super::from_str(block).expect("the block is YAML");
                let rendered = serde_json::to_string_pretty(&as_json).expect("it renders");
                read.insert(
                    format!("{name} as JSON"),
                    launch(&written(&dir, "launch.json", &rendered)),
                );
                launch(&written(&dir, "launch.yaml", block))
            } else if block.contains("onepipeline_templates:") {
                written(&dir, "templates.yaml", block);
                registry(&dir)
            } else if block.contains("every: 7d") {
                maintenance(&written(&dir, "maintenance.yaml", block))
            } else if block.contains("executors:") {
                rules(&written(&dir, "executors.yaml", block))
            } else {
                panic!("{name} is a YAML block no loader here reads: {block}")
            };
            read.insert(name, value);
        }
        read
    }

    /// Each committed real document — the executor rules example, the shipped graphs, the
    /// example plan store, a redacted copy of this host's maintenance schedule and template
    /// registration, and the contract's own blocks, its launch configs also as the JSON they
    /// render to — reads through its loader to the values the base's reader read.
    #[test]
    fn every_real_document_reads_as_the_reader_it_replaced_read_it() {
        let scratch = std::env::temp_dir().join(format!(
            "onepipeline-yaml-real-documents-{}",
            crate::sys::pid()
        ));
        let _ = std::fs::remove_dir_all(&scratch);
        let read = read_real_documents(&scratch);
        let _ = std::fs::remove_dir_all(&scratch);
        let expected: BTreeMap<String, Value> =
            serde_json::from_str(EXPECTED).expect("the expected values are JSON");
        assert_eq!(
            read.keys().collect::<Vec<_>>(),
            expected.keys().collect::<Vec<_>>(),
            "the documents read are not the documents captured"
        );
        for (name, value) in &read {
            assert_eq!(
                value, &expected[name],
                "{name} reads differently than it did"
            );
        }
    }
}
