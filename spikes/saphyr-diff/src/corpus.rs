//! The real documents the harness reads, gathered from the repository and (read only) the host.

use std::path::{Path, PathBuf};

/// One document: where it came from and its text.
#[derive(Clone)]
pub struct Doc {
    pub name: String,
    pub text: String,
}

#[derive(Default)]
pub struct Corpus {
    pub rules: Vec<Doc>,
    pub launch: Vec<Doc>,
    pub maintenance: Vec<Doc>,
    pub templates: Vec<Doc>,
    pub graphs: Vec<Doc>,
    /// Markdown documents with YAML front matter (shadow stores, example plan store).
    pub markdown: Vec<Doc>,
    pub contract_blocks: Vec<Doc>,
    pub workflows: Vec<Doc>,
    /// Every other committed YAML file, read schema-less only.
    pub other_yaml: Vec<Doc>,
    /// Real node override lists from the host's runs.
    pub node_sets: Vec<String>,
}

fn read(path: &Path, name: String) -> Option<Doc> {
    std::fs::read_to_string(path).ok().map(|text| Doc { name, text })
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>, keep: &dyn Fn(&Path) -> bool) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if matches!(name, "target" | "node_modules" | ".git" | ".nx" | "dist") {
                continue;
            }
            walk(&path, out, keep);
        } else if keep(&path) {
            out.push(path);
        }
    }
}

fn is_yaml(path: &Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()), Some("yaml" | "yml"))
}

/// Every fenced block in `markdown` of `language`.
pub fn fenced(markdown: &str, language: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for line in markdown.lines() {
        match &mut current {
            None if line.trim_start() == format!("```{language}") => current = Some(String::new()),
            Some(body) if line.trim_start().starts_with("```") => {
                out.push(std::mem::take(body));
                current = None;
            }
            Some(body) => {
                body.push_str(line);
                body.push('\n');
            }
            None => {}
        }
    }
    out
}

impl Corpus {
    pub fn gather(repo: &Path, host: Option<&Path>) -> Self {
        let mut c = Corpus::default();
        let rel = |p: &Path, root: &Path, tag: &str| {
            format!("{tag}{}", p.strip_prefix(root).unwrap_or(p).display())
        };

        // Executor rules.
        c.rules.extend(read(&repo.join("examples/executors.yaml"), "repo:examples/executors.yaml".into()));
        // Graphs.
        let mut paths = Vec::new();
        walk(&repo.join("graphs"), &mut paths, &is_yaml);
        for p in &paths {
            c.graphs.extend(read(p, rel(p, repo, "repo:")));
        }
        // Workflows.
        let mut paths = Vec::new();
        walk(&repo.join(".github"), &mut paths, &is_yaml);
        for p in &paths {
            c.workflows.extend(read(p, rel(p, repo, "repo:")));
        }
        // Everything else YAML in the repo.
        let mut paths = Vec::new();
        walk(repo, &mut paths, &is_yaml);
        for p in &paths {
            let r = p.strip_prefix(repo).unwrap_or(p);
            if r.starts_with("graphs") || r.starts_with(".github") || r.starts_with("spikes") || r == Path::new("examples/executors.yaml") {
                continue;
            }
            c.other_yaml.extend(read(p, rel(p, repo, "repo:")));
        }
        // Markdown front matter: example plan store.
        let mut paths = Vec::new();
        walk(&repo.join("examples/plan-store"), &mut paths, &|p| p.extension().is_some_and(|e| e == "md"));
        for p in &paths {
            c.markdown.extend(read(p, rel(p, repo, "repo:")));
        }
        // Contract blocks.
        let contract = std::fs::read_to_string(repo.join("docs/contract.md")).expect("contract");
        for (i, body) in fenced(&contract, "yaml").into_iter().enumerate() {
            c.contract_blocks.push(Doc { name: format!("contract.md yaml block {}", i + 1), text: body });
        }
        for block in &c.contract_blocks {
            let t = &block.text;
            if t.contains("schema_version:") {
                c.launch.push(block.clone());
            } else if t.contains("onepipeline_templates:") {
                c.templates.push(block.clone());
            } else if t.contains("every: 7d") {
                c.maintenance.push(block.clone());
            } else if t.contains("executors:") {
                c.rules.push(block.clone());
            }
        }

        if let Some(host) = host {
            let mut paths = Vec::new();
            walk(&host.join("graphs"), &mut paths, &is_yaml);
            for p in &paths {
                c.graphs.extend(read(p, rel(p, host, "host:")));
            }
            c.maintenance.extend(read(
                &host.join("config/onepipeline.maintenance.yaml"),
                "host:config/onepipeline.maintenance.yaml".into(),
            ));
            c.templates.extend(read(&host.join("templates/templates.yaml"), "host:templates/templates.yaml".into()));
            // Shadow stores: every run's write-back directory.
            let mut paths = Vec::new();
            if let Ok(runs) = std::fs::read_dir(host.join("runs")) {
                let mut runs: Vec<_> = runs.flatten().map(|e| e.path()).collect();
                runs.sort();
                for run in runs {
                    walk(&run.join("writeback"), &mut paths, &|p| p.extension().is_some_and(|e| e == "md"));
                    if let Some(launch) = std::fs::read_to_string(run.join("launch.json"))
                        .ok()
                        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                    {
                        for set in launch["node_sets"].as_array().into_iter().flatten() {
                            if let Some(s) = set.as_str() {
                                if !c.node_sets.contains(&s.to_owned()) {
                                    c.node_sets.push(s.to_owned());
                                }
                            }
                        }
                    }
                }
            }
            for p in &paths {
                c.markdown.extend(read(p, rel(p, host, "host:")));
            }
        }
        c
    }

    pub fn print_counts(&self) {
        println!("\n## corpus");
        for (kind, docs) in [
            ("executor rules", &self.rules),
            ("launch configs (YAML, contract)", &self.launch),
            ("maintenance schedules", &self.maintenance),
            ("templates.yaml", &self.templates),
            ("graph files", &self.graphs),
            ("markdown with front matter", &self.markdown),
            ("contract.md yaml blocks", &self.contract_blocks),
            ("workflow/action files", &self.workflows),
            ("other committed YAML", &self.other_yaml),
        ] {
            let names: Vec<&str> = docs.iter().map(|d| d.name.as_str()).collect();
            if names.len() <= 12 {
                println!("- {kind}: {} {:?}", docs.len(), names);
            } else {
                println!("- {kind}: {}", docs.len());
            }
        }
        println!("- real node_sets from host runs: {:?}", self.node_sets);
    }
}
