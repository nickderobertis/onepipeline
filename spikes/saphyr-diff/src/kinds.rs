//! One function per document kind: read, write and error behaviour of serde_norway
//! (today) against serde-saphyr (and saphyr's tree, for schema-less walks).

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::path::Path;
use std::time::Instant;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};

use crate::corpus::{Corpus, Doc};
use crate::diff::{categorize, line_diff, Report, Section};

use oneagentgraph::config::GraphConfig;
use onepipeline::filter::{LaunchConfig, LAUNCH_CONFIG_SCHEMA_VERSIONS_READ};
use onepipeline::maintenance::MaintenanceConfig;
use onepipeline::rules::ExecutorRules;

/// A per-run directory for fixture files the real loaders read: under the dispatch's
/// scratch directory when there is one, else the system temp dir.
fn scratch_dir(name: &str) -> std::path::PathBuf {
    std::env::var_os("ONEPIPELINE_NODE_SCRATCH_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("saphyr-diff-{name}-{}", std::process::id()))
}

// ---------------------------------------------------------------------------------------
// Parsers.

/// The serde-saphyr deserializer configurations tried against today's behaviour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum De {
    Default,
    StrictBooleans,
    NoSnippet,
    LastWins,
    RejectTags,
    MergeAsOrdinary,
    NonFiniteAsString,
    NoSchema,
    StrictNoSchema,
    LegacyOctal,
}

/// Every read configuration the option matrix tries.
pub const ALL_DE: [De; 10] = [
    De::Default,
    De::StrictBooleans,
    De::NoSchema,
    De::StrictNoSchema,
    De::LegacyOctal,
    De::NonFiniteAsString,
    De::RejectTags,
    De::MergeAsOrdinary,
    De::LastWins,
    De::NoSnippet,
];

impl De {
    fn options(self) -> serde_saphyr::Options {
        let mut o = serde_saphyr::Options::default();
        match self {
            De::Default => {}
            De::StrictBooleans => o.strict_booleans = true,
            De::NoSnippet => o.with_snippet = false,
            De::LastWins => o.duplicate_keys = serde_saphyr::DuplicateKeyPolicy::LastWins,
            De::RejectTags => o.reject_unsupported_tags = true,
            De::MergeAsOrdinary => o.merge_keys = serde_saphyr::MergeKeyPolicy::AsOrdinary,
            De::NonFiniteAsString => o.reject_non_finite_typeless_float = false,
            De::NoSchema => o.no_schema = true,
            De::StrictNoSchema => {
                o.strict_booleans = true;
                o.no_schema = true;
            }
            De::LegacyOctal => o.legacy_octal_numbers = true,
        }
        o
    }
}

pub fn norway<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    serde_norway::from_str(text).map_err(|e| e.to_string())
}

pub fn saphyr<T: DeserializeOwned>(text: &str, de: De) -> Result<T, String> {
    serde_saphyr::from_str_with_options(text, de.options()).map_err(|e| e.to_string())
}

/// serde-saphyr without snippets, through `serde_path_to_error` (already a dependency of
/// this crate), formatted as serde_norway formats a nested error: `path: message`.
pub fn saphyr_pathed<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    let mut o = serde_saphyr::Options::default();
    o.with_snippet = false;
    // The closure must fail with serde-saphyr's own error (so it stops and keeps its
    // location); the path is carried out beside it.
    let path = std::cell::RefCell::new(None::<String>);
    let result = serde_saphyr::with_deserializer_from_str_with_options(text, o, |de| {
        serde_path_to_error::deserialize::<_, T>(de).map_err(|e| {
            *path.borrow_mut() = Some(e.path().to_string());
            e.into_inner()
        })
    });
    result.map_err(|e| match path.into_inner() {
        Some(p) if p != "." => format!("{p}: {e}"),
        _ => e.to_string(),
    })
}

/// saphyr 0.2's node tree, flattened into JSON so it compares with the other two. Keys
/// that are not strings are written `<key:...>` so they show rather than collide.
pub fn tree(text: &str) -> Result<Json, String> {
    use saphyr::{LoadableYamlNode, Scalar, Yaml};
    fn conv(y: &Yaml) -> Json {
        match y {
            Yaml::Value(Scalar::Null) => Json::Null,
            Yaml::Value(Scalar::Boolean(b)) => json!(b),
            Yaml::Value(Scalar::Integer(i)) => json!(i),
            Yaml::Value(Scalar::FloatingPoint(f)) => json!(f.into_inner()),
            Yaml::Value(Scalar::String(s)) => json!(s.as_ref()),
            Yaml::Representation(s, _, _) => json!(s.as_ref()),
            Yaml::Sequence(seq) => Json::Array(seq.iter().map(conv).collect()),
            Yaml::Mapping(map) => Json::Object(
                map.iter()
                    .map(|(k, v)| {
                        let key = match k {
                            Yaml::Value(Scalar::String(s)) => s.to_string(),
                            other => format!("<key:{}>", conv(other)),
                        };
                        (key, conv(v))
                    })
                    .collect(),
            ),
            Yaml::Tagged(_, inner) => conv(inner),
            Yaml::Alias(n) => json!(format!("<alias {n}>")),
            Yaml::BadValue => json!("<bad value>"),
        }
    }
    let docs = Yaml::load_from_str(text).map_err(|e| e.to_string())?;
    Ok(docs.first().map(conv).unwrap_or(Json::Null))
}

// ---------------------------------------------------------------------------------------
// Comparisons.

/// Every path at which two JSON values differ, in value or in key order.
fn json_diff(a: &Json, b: &Json, path: &str, out: &mut Vec<String>) {
    match (a, b) {
        (Json::Object(x), Json::Object(y)) => {
            let xk: Vec<&String> = x.keys().collect();
            let yk: Vec<&String> = y.keys().collect();
            for k in &xk {
                match y.get(*k) {
                    Some(v) => json_diff(&x[*k], v, &format!("{path}/{k}"), out),
                    None => out.push(format!("{path}/{k}: before {} | after (absent)", x[*k])),
                }
            }
            for k in &yk {
                if !x.contains_key(*k) {
                    out.push(format!("{path}/{k}: before (absent) | after {}", y[*k]));
                }
            }
            let common_x: Vec<_> = xk.iter().filter(|k| y.contains_key(**k)).collect();
            let common_y: Vec<_> = yk.iter().filter(|k| x.contains_key(**k)).collect();
            if common_x != common_y {
                out.push(format!("{path}: key order before {common_x:?} | after {common_y:?}"));
            }
        }
        (Json::Array(x), Json::Array(y)) if x.len() == y.len() => {
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                json_diff(p, q, &format!("{path}/{i}"), out);
            }
        }
        _ if a == b && a.to_string() == b.to_string() => {}
        _ => out.push(format!("{} : before {} | after {}", if path.is_empty() { "/" } else { path }, a, b)),
    }
}

/// Typed read of one document by both crates.
fn typed<T: DeserializeOwned + PartialEq + Debug>(s: &mut Section, doc: &Doc, label: &str, de: De) -> Option<T> {
    let before = norway::<T>(&doc.text);
    let after = saphyr::<T>(&doc.text, de);
    match (&before, &after) {
        (Ok(x), Ok(y)) if x == y => {}
        (Ok(x), Ok(y)) => s.differ(format!("{label}: typed read differs\nbefore: {x:?}\nafter:  {y:?}"), &doc.name),
        (Ok(_), Err(e)) => s.differ(format!("{label}: today reads it, serde-saphyr ({de:?}) refuses:\n{e}"), &doc.name),
        (Err(e), Ok(_)) => s.differ(format!("{label}: today refuses ({e}), serde-saphyr ({de:?}) reads it"), &doc.name),
        (Err(x), Err(y)) if x == y => {}
        (Err(x), Err(y)) => s.differ(format!("{label}: both refuse, in different words\nbefore: {x}\nafter:  {y}"), &doc.name),
    }
    before.ok()
}

/// Schema-less read into `serde_json::Value` (preserve_order) by both crates, and saphyr's tree.
fn schemaless(s: &mut Section, doc: &Doc, text: &str, label: &str) {
    let before = norway::<Json>(text);
    for (who, after) in [
        ("serde-saphyr Default -> serde_json::Value", saphyr::<Json>(text, De::Default)),
        ("serde-saphyr strict_booleans -> serde_json::Value", saphyr::<Json>(text, De::StrictBooleans)),
        ("saphyr 0.2 tree", tree(text)),
    ] {
        match (&before, &after) {
            (Ok(x), Ok(y)) => {
                let mut out = Vec::new();
                json_diff(x, y, "", &mut out);
                for d in out {
                    s.differ(format!("{label} [{who}]: {d}"), &doc.name);
                }
            }
            (Ok(_), Err(e)) => s.differ(format!("{label} [{who}]: today reads, this refuses: {e}"), &doc.name),
            (Err(e), Ok(_)) => s.differ(format!("{label} [{who}]: today refuses ({e}), this reads"), &doc.name),
            (Err(_), Err(_)) => {}
        }
    }
}

/// Typed emission by both crates, byte for byte, and serde-saphyr's own round trip.
fn emitted<T: Serialize + DeserializeOwned + PartialEq + Debug>(s: &mut Section, doc: &str, value: &T, label: &str) {
    let before = serde_norway::to_string(value).expect("norway emits");
    let after = serde_saphyr::to_string(value).expect("saphyr emits");
    if before != after {
        for (category, example) in categorize(&before, &after) {
            s.differ(format!("{label}: {category}\n{example}"), doc);
        }
    }
    for (reader, back) in [("serde-saphyr", saphyr::<T>(&after, De::Default)), ("serde_norway", norway::<T>(&after))] {
        match back {
            Ok(back) if &back == value => {}
            Ok(back) => s.differ(format!("{label}: serde-saphyr output re-read by {reader} is a different value: {back:?}"), doc),
            Err(e) => s.differ(format!("{label}: serde-saphyr output does not re-read by {reader}: {e}"), doc),
        }
    }
}

/// What the user reads from a real loader: today's text, and the same with the serde part
/// replaced by serde-saphyr's — the code after serde is shared, so where both crates read the
/// same typed value the whole outcome is the same.
fn whole_load<T: PartialEq + Debug>(
    s: &mut Section,
    doc: &str,
    real: Result<(), String>,
    before: Result<T, String>,
    after: Result<T, String>,
) {
    let projected = match (&real, &before, &after) {
        (_, Ok(x), Ok(y)) if x == y => real.clone(),
        (Err(full), Err(b), Err(a)) => Err(full.replace(b.as_str(), a.as_str())),
        (_, _, Ok(_)) => Err("<serde-saphyr reads a different value; see the typed read>".into()),
        (_, _, Err(a)) => Err(a.clone()),
    };
    if projected != real {
        s.differ(format!("what the loader reports\nbefore: {real:?}\nafter:  {projected:?}"), doc);
    }
}

// ---------------------------------------------------------------------------------------
// Kinds.

pub fn executor_rules(c: &Corpus, r: &mut Report) {
    let s = r.section("executor rules (ExecutorRules)");
    let mut docs = c.rules.clone();
    // The flow-style fixtures tests/contract.rs and src/rules.rs read.
    docs.push(Doc {
        name: "tests/contract.rs flow fixture".into(),
        text: "executors: [{name: local, type: local}]\nrules: [{when: {node_label: {step: implement}}, use: local}]\n".into(),
    });
    for doc in &docs {
        s.compared += 1;
        let rules: Option<ExecutorRules> = typed(s, doc, "read", De::Default);
        schemaless(s, doc, &doc.text, "schema-less");
        if let Some(rules) = rules {
            let after: Option<ExecutorRules> = saphyr(&doc.text, De::Default).ok();
            let v1 = rules.validate().map_err(|e| e.to_string());
            if let Some(after) = after {
                let v2 = after.validate().map_err(|e| e.to_string());
                if v1 != v2 {
                    s.differ(format!("validate(): before {v1:?} after {v2:?}"), &doc.name);
                }
            }
            emitted(s, &doc.name, &rules, "write (to_string round trip, tests only)");
        }
    }
}

pub fn launch_configs(c: &Corpus, r: &mut Report) {
    let s = r.section("launch configs (LaunchConfig, YAML and JSON)");
    let mut docs = c.launch.clone();
    for v in LAUNCH_CONFIG_SCHEMA_VERSIONS_READ {
        docs.push(Doc { name: format!("bare schema_version: {v}"), text: format!("schema_version: {v}\n") });
    }
    // JSON spellings: what an operator writes as JSON, pretty and compact, of each YAML doc.
    for doc in c.launch.clone() {
        if let Ok(v) = norway::<Json>(&doc.text) {
            docs.push(Doc { name: format!("{} as pretty JSON", doc.name), text: serde_json::to_string_pretty(&v).unwrap() });
            docs.push(Doc { name: format!("{} as compact JSON", doc.name), text: serde_json::to_string(&v).unwrap() });
        }
        if let Ok(cfg) = norway::<LaunchConfig>(&doc.text) {
            docs.push(Doc { name: format!("{} re-written by serde_json", doc.name), text: serde_json::to_string_pretty(&cfg).unwrap() });
        }
    }
    // The newer-schema refusal, in both spellings.
    for (name, text) in [
        ("schema 9 YAML naming node_sets", "schema_version: 9\nnode_sets: [\"members.worker.persona=x\"]\n"),
        ("schema 9 YAML naming a blank dag_sets", "schema_version: 9\ndag_sets:\n"),
        ("schema 9 JSON naming dag_sets", "{\"schema_version\": 9, \"dag_sets\": []}"),
        ("schema 10 JSON naming node_sets", "{\"schema_version\": 10, \"node_sets\": [\"a=b\"]}"),
        ("unread version", "schema_version: 99\n"),
        ("unknown key", "schema_version: 2\nfilterz: {}\n"),
        ("JSON with a tab-indented body", "{\n\t\"schema_version\": 2,\n\t\"pr_author_graph\": \"./g.yaml\"\n}\n"),
        ("JSON with unicode escape", "{\"schema_version\": 11, \"branch_template\": \"\\u00e9/{{ node.id }}\"}"),
    ] {
        docs.push(Doc { name: name.into(), text: text.into() });
    }
    let dir = scratch_dir("launch");
    std::fs::create_dir_all(&dir).unwrap();
    for (i, doc) in docs.iter().enumerate() {
        s.compared += 1;
        let cfg: Option<LaunchConfig> = typed(s, doc, "serde read", De::Default);
        schemaless(s, doc, &doc.text, "schema-less (newer-schema check's Value)");
        // The whole load: today's real `LaunchConfig::load`, and the same text with the serde
        // refusal swapped for serde-saphyr's (every later step reads the same typed value).
        let path = dir.join(format!("{i}.yaml"));
        std::fs::write(&path, &doc.text).unwrap();
        let real = LaunchConfig::load(&path).map(|_| ()).map_err(|e| e.to_string());
        whole_load(s, &doc.name, real, norway::<LaunchConfig>(&doc.text), saphyr::<LaunchConfig>(&doc.text, De::Default));
        if let Some(cfg) = cfg {
            emitted(s, &doc.name, &cfg, "write (to_string round trip, tests only)");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

pub fn maintenance(c: &Corpus, r: &mut Report) {
    let s = r.section("maintenance schedules (MaintenanceConfig)");
    for doc in &c.maintenance {
        s.compared += 1;
        let cfg: Option<MaintenanceConfig> = typed(s, doc, "read", De::Default);
        schemaless(s, doc, &doc.text, "schema-less");
        if let Some(cfg) = cfg {
            emitted(s, &doc.name, &cfg, "write (to_string round trip)");
        }
    }
}

/// `src/templates.rs`'s private `RegistrationFile`, copied verbatim.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Declared {
    role: onepipeline::templates::Role,
    description: String,
}
#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistrationFile {
    onepipeline_templates: u32,
    templates: BTreeMap<String, Declared>,
}

pub fn templates(c: &Corpus, r: &mut Report) {
    let s = r.section("templates.yaml (RegistrationFile)");
    for doc in &c.templates {
        s.compared += 1;
        let _: Option<RegistrationFile> = typed(s, doc, "read", De::Default);
        schemaless(s, doc, &doc.text, "schema-less");
    }
}

/// Today's hop: YAML -> serde_json::Value -> overrides -> serde_norway::Value -> GraphConfig.
fn hop_today(text: &str, sets: &[String]) -> Result<GraphConfig, String> {
    let mut v: Json = norway(text)?;
    apply(&mut v, sets)?;
    serde_norway::from_value(serde_norway::to_value(&v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

fn apply(v: &mut Json, sets: &[String]) -> Result<(), String> {
    let o = sets
        .iter()
        .map(|s| oneagentgraph::run::parse_set(s).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    oneagentgraph::run::apply_overrides(v, &o).map_err(|e| e.to_string())
}

pub fn graphs(c: &Corpus, r: &mut Report) {
    let mut sets_list: Vec<Vec<String>> = vec![vec![], vec!["members.worker.persona=engineer".into()]];
    for s in &c.node_sets {
        sets_list.push(vec![s.clone(), "members.worker.persona=engineer".into()]);
    }
    {
        let s = r.section("graph files, direct typed read (GraphConfig: testfakes, tests)");
        for doc in &c.graphs {
            s.compared += 1;
            if let Some(g) = typed::<GraphConfig>(s, doc, "read", De::Default) {
                let v = oneagentgraph::config::validate(&g).map_err(|e| e.to_string());
                if let Ok(after) = saphyr::<GraphConfig>(&doc.text, De::Default) {
                    let w = oneagentgraph::config::validate(&after).map_err(|e| e.to_string());
                    if v != w {
                        s.differ(format!("validate(): before {v:?} after {w:?}"), &doc.name);
                    }
                }
                emitted(s, &doc.name, &g, "write (to_string, tests/e2e/channel.rs)");
            }
        }
    }
    let s = r.section("graph files, Value hop with overrides (graph.rs, dispatchenv.rs)");
    for doc in &c.graphs {
        schemaless(s, doc, &doc.text, "schema-less (the Value overrides apply to)");
        for sets in &sets_list {
            s.compared += 1;
            let name = format!("{} sets {sets:?}", doc.name);
            let before = hop_today(&doc.text, sets);
            let after_json = saphyr::<Json>(&doc.text, De::Default).and_then(|mut v| {
                apply(&mut v, sets)?;
                Ok(v)
            });
            let a = after_json.clone().and_then(|v| serde_json::from_value::<GraphConfig>(v).map_err(|e| e.to_string()));
            let b = after_json.and_then(|v| {
                let text = serde_saphyr::to_string(&v).map_err(|e| e.to_string())?;
                saphyr::<GraphConfig>(&text, De::Default)
            });
            for (how, after) in [("serde_json::from_value", a), ("serde-saphyr to_string + from_str", b)] {
                if before != after {
                    s.differ(format!("hop via {how}: before {before:?}\nafter {after:?}"), &name);
                }
            }
            if sets.is_empty() {
                let direct = norway::<GraphConfig>(&doc.text);
                if direct != before {
                    s.differ("today's hop differs from today's direct read".into(), &name);
                }
            }
        }
    }
}

fn front(text: &str) -> Option<&str> {
    text.strip_prefix("---\n").and_then(|rest| rest.split_once("\n---\n")).map(|(f, _)| f)
}

fn origin(front: &Json) -> Result<Option<String>, String> {
    match &front["metadata"]["onetaskgraph.origin"] {
        Json::Null => Ok(None),
        Json::String(s) => Ok(Some(s.clone())),
        other => Err(format!("its origin is {other}, not an id")),
    }
}

pub fn front_matter(c: &Corpus, r: &mut Report) {
    let s = r.section("shadow-store front matter read (shadow_origin, Value)");
    let mut origins = 0;
    for doc in &c.markdown {
        let Some(f) = front(&doc.text) else { continue };
        s.compared += 1;
        schemaless(s, doc, f, "front matter");
        let before = norway::<Json>(f).map(|v| origin(&v));
        let after = saphyr::<Json>(f, De::Default).map(|v| origin(&v));
        if before.as_ref().is_ok_and(|o| o.as_ref().is_ok_and(Option::is_some)) {
            origins += 1;
        }
        if before != after {
            s.differ(format!("shadow_origin: before {before:?} after {after:?}"), &doc.name);
        }
    }
    s.notes.push(format!("{origins} documents carry an onetaskgraph.origin"));
}

pub fn contract_blocks(c: &Corpus, r: &mut Report) {
    let s = r.section("docs/contract.md yaml blocks (schema-less)");
    for doc in &c.contract_blocks {
        s.compared += 1;
        schemaless(s, doc, &doc.text, "block");
    }
    let s = r.section("other committed YAML (schema-less, for completeness)");
    for doc in &c.other_yaml {
        s.compared += 1;
        schemaless(s, doc, &doc.text, "document");
    }
}

/// serde-saphyr serializer configurations tried against today's bytes.
pub const NOFOLD: &str = "folded_wrap_chars=MAX";
pub const NOFOLD12: &str = "folded_wrap_chars=MAX,yaml_12 (code strips the `%YAML 1.2` header)";
fn ser_variants() -> Vec<(&'static str, serde_saphyr::SerializerOptions)> {
    let d = serde_saphyr::SerializerOptions::default;
    let mut v = vec![("default", d())];
    let mut o = d();
    o.folded_wrap_chars = usize::MAX;
    v.push((NOFOLD, o));
    let mut o = d();
    o.folded_wrap_chars = usize::MAX;
    o.yaml_12 = true;
    v.push((NOFOLD12, o));
    let mut o = d();
    o.prefer_block_scalars = false;
    v.push(("prefer_block_scalars=false", o));
    let mut o = d();
    o.yaml_12 = true;
    v.push(("yaml_12 (header left in)", o));
    let mut o = d();
    o.folded_wrap_chars = usize::MAX;
    o.compact_list_indent = false;
    v.push(("folded_wrap_chars=MAX,compact_list_indent=false", o));
    let mut o = d();
    o.quote_all = true;
    v.push(("quote_all", o));
    let mut o = d();
    o.folded_wrap_chars = usize::MAX;
    v.push((NOFOLD_STRIP, o));
    let mut o = d();
    o.folded_wrap_chars = usize::MAX;
    o.yaml_12 = true;
    v.push((NOFOLD12_STRIP, o));
    v
}

pub const NOFOLD_STRIP: &str = "folded_wrap_chars=MAX + code strips blank-line indentation in literal blocks";
pub const NOFOLD12_STRIP: &str =
    "folded_wrap_chars=MAX,yaml_12 (header stripped) + code strips blank-line indentation in literal blocks";

/// The code alternative for the blank-line difference: inside a literal block scalar, a line
/// holding only spaces, no more of them than the block's indentation, is an empty line of the
/// block (YAML 1.2 §8.1.1.2), so writing it as `""` reads back the same string. A
/// whitespace-only line with *more* spaces than the indentation carries content (the spaces
/// after the indentation are part of the string) and is left exactly as it is.
pub fn strip_blank_block_lines(text: &str) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let indent_of = |l: &str| l.len() - l.trim_start_matches(' ').len();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        out.push_str(line);
        i += 1;
        let body = line.trim_end_matches('\n').trim_end();
        let header = body.rsplit_once(": ").map(|(_, h)| h).or_else(|| body.trim_start().strip_prefix("- ")).unwrap_or(body.trim_start());
        let is_literal = header.starts_with('|')
            && header[1..].chars().all(|c| c == '-' || c == '+' || c.is_ascii_digit())
            && !body.trim_start().starts_with('"')
            && !body.trim_start().starts_with('\'');
        if !is_literal {
            continue;
        }
        let parent = indent_of(line);
        // The block's indentation: an explicit indicator, else its first non-blank line's.
        let explicit = header[1..].chars().find(|c| c.is_ascii_digit()).map(|c| parent + c.to_digit(10).unwrap() as usize);
        let block = explicit.or_else(|| {
            lines[i..]
                .iter()
                .find(|l| !l.trim().is_empty())
                .map(|l| indent_of(l))
                .filter(|n| *n > parent)
        });
        let Some(block) = block else { continue };
        while i < lines.len() {
            let l = lines[i];
            let content = l.trim_end_matches('\n');
            let blank = content.chars().all(|c| c == ' ');
            if !blank && indent_of(l) < block {
                break;
            }
            if blank && content.len() <= block {
                out.push_str(&l[content.len()..]);
            } else {
                out.push_str(l);
            }
            i += 1;
        }
    }
    out
}

/// One emission under one variant, with the code change the variant's label names.
pub fn emit<T: Serialize>(value: &T, label: &str, opts: serde_saphyr::SerializerOptions) -> String {
    let out = serde_saphyr::to_string_with_options(value, opts).unwrap();
    let out = if label == NOFOLD12 || label == NOFOLD12_STRIP {
        out.strip_prefix("%YAML 1.2\n---\n").map(str::to_owned).unwrap_or(out)
    } else {
        out
    };
    if label == NOFOLD_STRIP || label == NOFOLD12_STRIP {
        strip_blank_block_lines(&out)
    } else {
        out
    }
}

/// The variants whose every difference is listed (the others are counted only).
fn listed(label: &str) -> bool {
    matches!(label, "default") || label == NOFOLD || label == NOFOLD12 || label == NOFOLD_STRIP || label == NOFOLD12_STRIP
}

pub fn writeback_emission(c: &Corpus, scratch: &Path, r: &mut Report) {
    let variants = ser_variants();
    let mut identical: BTreeMap<&str, usize> = BTreeMap::new();
    let mut on_disk_same = 0;
    let mut compared = 0;
    let mut stores: BTreeMap<String, Vec<(String, Json, String)>> = BTreeMap::new();
    let mut appendix = Vec::new();
    #[allow(clippy::type_complexity)]
    let mut categories: BTreeMap<(String, String), (Vec<String>, Vec<String>)> = BTreeMap::new();
    {
        let s = r.section("write-back shadow documents, emitted bytes (writeback.rs `document`)");
        for doc in &c.markdown {
            let Some(f) = front(&doc.text) else { continue };
            let Ok(value) = norway::<Json>(f) else { continue };
            compared += 1;
            s.compared += 1;
            let body = doc.text.split_once("\n---\n").map(|(_, b)| b.to_owned()).unwrap_or_default();
            let before = serde_norway::to_string(&value).unwrap();
            if format!("{f}\n") == before {
                on_disk_same += 1;
            }
            for (label, opts) in &variants {
                let after = emit(&value, label, opts.clone());
                if after == before {
                    *identical.entry(label).or_default() += 1;
                } else if listed(label) {
                    appendix.push(format!("[{label}] {} (- today, + serde-saphyr)\n{}", doc.name, line_diff(&before, &after)));
                    let mut seen = std::collections::BTreeSet::new();
                    for (category, example) in categorize(&before, &after) {
                        let entry = categories.entry((label.to_string(), category.clone())).or_insert_with(|| (Vec::new(), Vec::new()));
                        if seen.insert(category) {
                            entry.0.push(doc.name.clone());
                        }
                        if entry.1.len() < 3 && !entry.1.contains(&example) {
                            entry.1.push(example);
                        }
                    }
                }
                if listed(label) {
                    match saphyr::<Json>(&after, De::Default) {
                        Ok(back) if back.to_string() == value.to_string() => {}
                        other => s.differ(format!("[{label}] re-read by serde-saphyr differs: {other:?}"), &doc.name),
                    }
                    match norway::<Json>(&after) {
                        Ok(back) if back.to_string() == value.to_string() => {}
                        other => s.differ(format!("[{label}] re-read by serde_norway differs: {other:?}"), &doc.name),
                    }
                }
            }
            // Host shadow stores: projects/<p>.md and tasks/<p>/<t>.md under runs/<run>/writeback.
            if let Some(rest) = doc.name.strip_prefix("host:runs/") {
                if let Some((run, rel)) = rest.split_once("/writeback/") {
                    stores.entry(run.to_owned()).or_default().push((rel.to_owned(), value.clone(), body));
                }
            }
        }
        for ((label, category), (docs, examples)) in std::mem::take(&mut categories) {
            let what = format!("[{label}] {category}\nexamples (- today, + serde-saphyr):\n{}", examples.join("\n~\n"));
            for d in docs {
                s.differ(what.clone(), &d);
            }
        }
        s.notes.push(format!(
            "{on_disk_same}/{compared} on-disk front matters are byte-identical to serde_norway re-emitting them (the rest were edited in place by local-md)"
        ));
        for (label, _) in &variants {
            s.notes.push(format!(
                "serde-saphyr [{label}] byte-identical to today on {}/{compared}",
                identical.get(label).copied().unwrap_or(0)
            ));
        }
    }
    r.appendix.extend(appendix);
    local_md(stores, scratch, r);
}

/// Write each run's shadow store twice — today's bytes and serde-saphyr's — and read and
/// edit both in place with the released onetaskgraph local-md source.
fn local_md(stores: BTreeMap<String, Vec<(String, Json, String)>>, scratch: &Path, r: &mut Report) {
    use onetaskgraph_local_md::{LocalMdConfig, LocalMdSource};
    use onetaskgraph_plugin_api::{MetadataKey, NativeId, StatusCategory, TaskSource};
    let s = r.section("write-back shadow documents, read back and edited by onetaskgraph-local-md 0.4.0");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let mut tasks = 0;
    let mut projects = 0;
    for (run, docs) in &stores {
        let mut sources = Vec::new();
        for (_who, emit) in [
            ("today", Box::new(|v: &Json| serde_norway::to_string(v).unwrap()) as Box<dyn Fn(&Json) -> String>),
            ("saphyr default", Box::new(|v: &Json| serde_saphyr::to_string(v).unwrap())),
            (NOFOLD12, Box::new(|v: &Json| {
                let (label, opts) = ser_variants().into_iter().find(|(l, _)| *l == NOFOLD12).unwrap();
                emit(v, label, opts)
            })),
            (NOFOLD12_STRIP, Box::new(|v: &Json| {
                let (label, opts) = ser_variants().into_iter().find(|(l, _)| *l == NOFOLD12_STRIP).unwrap();
                emit(v, label, opts)
            })),
        ] {
            let root = scratch.join(format!("store{}", sources.len())).join(run);
            for sub in ["projects", "tasks", "documents"] {
                std::fs::create_dir_all(root.join(sub)).unwrap();
            }
            for (rel, value, body) in docs {
                let path = root.join(rel);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, format!("---\n{}---\n{body}", emit(value))).unwrap();
            }
            let config: LocalMdConfig = serde_json::from_value(json!({"root": root})).unwrap();
            sources.push((root.clone(), LocalMdSource::new(config).unwrap()));
        }
        let norm = |v: Json, root: &Path| -> String { v.to_string().replace(&root.display().to_string(), "<root>") };
        for (rel, _, _) in docs {
            let name = format!("host:runs/{run}/writeback/{rel}");
            let id = rel.trim_end_matches(".md");
            let read = |i: usize| -> Result<String, String> {
                let (root, src) = &sources[i];
                if let Some(id) = id.strip_prefix("tasks/") {
                    rt.block_on(src.get_task(&NativeId(id.into())))
                        .map(|t| norm(serde_json::to_value(t).unwrap(), root))
                        .map_err(|e| e.to_string())
                } else {
                    let id = id.strip_prefix("projects/").unwrap_or(id);
                    rt.block_on(src.get_project(&NativeId(id.into())))
                        .map(|p| norm(serde_json::to_value(p).unwrap(), root))
                        .map_err(|e| e.to_string())
                }
            };
            s.compared += 1;
            if rel.starts_with("tasks/") { tasks += 1 } else { projects += 1 }
            let x = read(0);
            for (i, who) in [(1, "saphyr default"), (2, NOFOLD12), (3, NOFOLD12_STRIP)] {
                let y = read(i);
                if x != y {
                    s.differ(format!("[{who}] local-md read differs\nbefore: {x:?}\nafter:  {y:?}"), &name);
                }
                if let Err(e) = &y {
                    s.differ(format!("[{who}] local-md cannot read serde-saphyr's document: {e}"), &name);
                }
            }
            // Edit in place, on every store: status, then one metadata entry.
            if let Some(tid) = id.strip_prefix("tasks/") {
                let tid = NativeId(tid.into());
                let mut edits = Vec::new();
                for (root, src) in &sources {
                    let path = root.join(rel);
                    let pre = std::fs::read_to_string(&path).unwrap();
                    let st = rt.block_on(src.set_task_status(&tid, StatusCategory::Cancelled)).map(|_| ()).map_err(|e| e.to_string());
                    let key = MetadataKey::try_from("onepipeline.spike".to_owned()).unwrap();
                    let md = rt
                        .block_on(src.set_task_metadata(&tid, &key, &json!({"a": [1, "on"], "b": "yes"})))
                        .map(|_| ())
                        .map_err(|e| e.to_string());
                    let post = std::fs::read_to_string(&path).unwrap();
                    let after_read = rt.block_on(src.get_task(&tid)).map(|t| norm(serde_json::to_value(t).unwrap(), root)).map_err(|e| e.to_string());
                    edits.push((st, md, line_diff(&pre, &post), after_read));
                }
                for (i, who) in [(1, "saphyr default"), (2, NOFOLD12), (3, NOFOLD12_STRIP)] {
                    if edits[0] != edits[i] {
                        s.differ(
                            format!(
                                "[{who}] local-md in-place edit differs\nbefore: status {:?} metadata {:?}\n  edit diff:\n{}\nafter:  status {:?} metadata {:?}\n  edit diff:\n{}\n  re-read equal: {}",
                                edits[0].0, edits[0].1, edits[0].2, edits[i].0, edits[i].1, edits[i].2, edits[0].3 == edits[i].3
                            ),
                            &name,
                        );
                    }
                    if edits[i].0.is_err() || edits[i].1.is_err() || edits[i].3.is_err() {
                        s.differ(format!("[{who}] local-md edit of serde-saphyr's document failed: {:?}", edits[i]), &name);
                    }
                }
            }
        }
    }
    s.notes.push(format!("{} stores, {tasks} tasks and {projects} projects read and edited", stores.len()));
}

/// `src/boundary.rs` reads this host's onevcs branches file (read only here) into
/// `onevcs::branches::BranchesFile`; not in the task's list, but a production read.
pub fn branches_file(r: &mut Report) {
    let s = r.section("onevcs branches file (boundary.rs, BranchesFile)");
    let mut docs = Vec::new();
    if let Some(path) = onevcs::branches::default_path().ok().filter(|p| p.is_file()) {
        docs.extend(std::fs::read_to_string(&path).ok().map(|text| Doc { name: format!("host:{}", path.display()), text }));
    }
    for (name, text) in [
        ("fixture: prefix left blank", "version: 1\nprefix:\n"),
        ("fixture: prefix absent", "version: 1\n"),
        ("fixture: unknown key", "version: 1\nprefx: nick/\n"),
    ] {
        docs.push(Doc { name: name.into(), text: text.into() });
    }
    for doc in &docs {
        s.compared += 1;
        let _: Option<onevcs::branches::BranchesFile> = typed(s, doc, "read", De::Default);
    }
}

pub fn ci_workflows(c: &Corpus, r: &mut Report) {
    let s = r.section("tests/ci_workflows.rs (workflow walks, `on:` key, scalar())");
    for doc in &c.workflows {
        s.compared += 1;
        let today: serde_norway::Value = match serde_norway::from_str(&doc.text) {
            Ok(v) => v,
            Err(e) => {
                s.differ(format!("today cannot parse: {e}"), &doc.name);
                continue;
            }
        };
        let via = if today.get("on").is_some() {
            "\"on\" string key"
        } else if today.get(serde_norway::Value::Bool(true)).is_some() {
            "Bool(true) key"
        } else {
            "no trigger key"
        };
        s.notes.push(format!("{}: today the triggers are found by {via}", doc.name));
        for (who, after) in [
            ("serde-saphyr Default", saphyr::<Json>(&doc.text, De::Default)),
            ("serde-saphyr strict_booleans", saphyr::<Json>(&doc.text, De::StrictBooleans)),
            ("saphyr tree", tree(&doc.text)),
        ] {
            match after {
                Ok(v) => {
                    let found = v.get("on").is_some();
                    s.notes.push(format!("{}: [{who}] `.get(\"on\")` finds the triggers: {found}", doc.name));
                }
                Err(e) => s.differ(format!("[{who}] cannot parse: {e}"), &doc.name),
            }
        }
        schemaless(s, doc, &doc.text, "workflow");
        // scalar(): every non-string scalar, rendered by each crate.
        let mut scalars = Vec::new();
        collect_scalars(&today, &mut scalars);
        for sc in scalars {
            let before = serde_norway::to_string(&sc).unwrap().trim().to_owned();
            let j = serde_json::to_value(&sc).unwrap();
            let after = serde_saphyr::to_string(&j).unwrap().trim().to_owned();
            if before != after {
                s.differ(format!("scalar() render: before {before:?} after {after:?}"), &doc.name);
            }
        }
    }
}

fn collect_scalars(v: &serde_norway::Value, out: &mut Vec<serde_norway::Value>) {
    use serde_norway::Value as V;
    match v {
        V::Mapping(m) => m.iter().for_each(|(_, v)| collect_scalars(v, out)),
        V::Sequence(s) => s.iter().for_each(|v| collect_scalars(v, out)),
        V::Tagged(t) => collect_scalars(&t.value, out),
        V::String(_) => {}
        other => {
            if !out.contains(other) {
                out.push(other.clone())
            }
        }
    }
}

/// The malformed cases the repository's tests assert on, with the substring each asserts.
pub fn malformed(r: &mut Report) {
    let s = r.section("errors on malformed documents (asserted substrings)");
    type Check = fn(&str, De) -> Result<(), String>;
    fn p<T: DeserializeOwned>(text: &str) -> Result<(), String> {
        saphyr_pathed::<T>(text).map(|_| ())
    }
    fn t<T: DeserializeOwned>(text: &str, de: De) -> Result<(), String> {
        saphyr::<T>(text, de).map(|_| ())
    }
    fn n<T: DeserializeOwned>(text: &str) -> Result<(), String> {
        norway::<T>(text).map(|_| ())
    }
    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, &str, &str, fn(&str) -> Result<(), String>, Check, fn(&str) -> Result<(), String>)> = vec![
        ("maintenance.rs/e2e/contract: top-level unknown", "version: 1\ndefault:\n  every: 7d\nat: \"03:00\"\n", "unknown field `at`", n::<MaintenanceConfig>, t::<MaintenanceConfig>, p::<MaintenanceConfig>),
        ("maintenance.rs: nested unknown", "version: 1\ndefault:\n  every: 7d\n  at: never\n", "unknown field `at`", n::<MaintenanceConfig>, t::<MaintenanceConfig>, p::<MaintenanceConfig>),
        ("maintenance.rs: missing match", "version: 1\ndefault:\n  every: 7d\nrules:\n  - every: 1d\n", "missing field `match`", n::<MaintenanceConfig>, t::<MaintenanceConfig>, p::<MaintenanceConfig>),
        ("maintenance.rs: empty match", "version: 1\ndefault:\n  every: 7d\nrules:\n  - match: {}\n    every: 1d\n", "`match` names no field", n::<MaintenanceConfig>, t::<MaintenanceConfig>, p::<MaintenanceConfig>),
        ("maintenance.rs: match repo", "version: 1\ndefault:\n  every: 7d\nrules:\n  - match: {repo: onevcs}\n    every: 1d\n", "`match` names `repo`", n::<MaintenanceConfig>, t::<MaintenanceConfig>, p::<MaintenanceConfig>),
        ("maintenance.rs: bad unit", "version: 1\ndefault:\n  every: 7x\n", "not a unit letter", n::<MaintenanceConfig>, t::<MaintenanceConfig>, p::<MaintenanceConfig>),
        ("maintenance.rs: bare number", "version: 1\ndefault:\n  every: 7\n", "`every` holds 7", n::<MaintenanceConfig>, t::<MaintenanceConfig>, p::<MaintenanceConfig>),
        ("e2e/templates.rs: entry colour", "onepipeline_templates: 1\ntemplates:\n  x:\n    role: task\n    description: d\n    colour: red\n", "unknown field `colour`", n::<RegistrationFile>, t::<RegistrationFile>, p::<RegistrationFile>),
        ("e2e/templates.rs: top extra", "onepipeline_templates: 1\nextra: 1\ntemplates: {}\n", "unknown field `extra`", n::<RegistrationFile>, t::<RegistrationFile>, p::<RegistrationFile>),
        ("e2e/templates.rs: missing description", "onepipeline_templates: 1\ntemplates:\n  x:\n    role: task\n", "missing field `description`", n::<RegistrationFile>, t::<RegistrationFile>, p::<RegistrationFile>),
        ("contract.rs: missing role", "onepipeline_templates: 1\ntemplates:\n  probe:\n    description: probe\n", "missing field `role`", n::<RegistrationFile>, t::<RegistrationFile>, p::<RegistrationFile>),
        ("contract.rs: rules mx_load1", "executors:\n  - {name: local, type: local, mx_load1: 8.0}\nrules:\n  - use: local\n", "mx_load1", n::<ExecutorRules>, t::<ExecutorRules>, p::<ExecutorRules>),
        ("launch: unknown key", "schema_version: 2\nfilterz: {}\n", "unknown field `filterz`", n::<LaunchConfig>, t::<LaunchConfig>, p::<LaunchConfig>),
        ("graph: unknown key", "version: 1\nname: x\nmembrs: {}\n", "unknown field `membrs`", n::<GraphConfig>, t::<GraphConfig>, p::<GraphConfig>),
        ("front matter: unclosed flow", "title: [unclosed\n", "", n::<Json>, t::<Json>, p::<Json>),
        ("front matter: duplicate key", "title: a\ntitle: b\n", "", n::<Json>, t::<Json>, p::<Json>),
        ("rules: wrong type", "executors: 3\nrules: []\n", "invalid type", n::<ExecutorRules>, t::<ExecutorRules>, p::<ExecutorRules>),
        ("templates: wrong role variant", "onepipeline_templates: 1\ntemplates:\n  x:\n    role: tusk\n    description: d\n", "unknown variant `tusk`", n::<RegistrationFile>, t::<RegistrationFile>, p::<RegistrationFile>),
    ];
    let dir = scratch_dir("malformed");
    std::fs::create_dir_all(&dir).unwrap();
    for (i, (name, text, needle, before, after, pathed)) in cases.into_iter().enumerate() {
        s.compared += 1;
        let b = before(text);
        // Today's real loader, where the document kind has one.
        let path = dir.join(format!("{i}/templates.yaml"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        let real: Option<Result<(), String>> = if name.starts_with("maintenance") {
            Some(MaintenanceConfig::load("--maintenance-config", &path).map(|_| ()).map_err(|e| e.to_string()))
        } else if name.contains("templates") || name.starts_with("contract.rs: missing role") {
            let root = path.parent().unwrap().to_path_buf();
            Some(
                onepipeline::verbs::template_list(&onepipeline::templates::TemplateOptions {
                    template_root: Some(root.clone()),
                    working_dir: root,
                    ..Default::default()
                })
                .map(|_| ())
                .map_err(|e| e.to_string()),
            )
        } else if name.contains("rules") {
            Some(ExecutorRules::load(&path).map(|_| ()).map_err(|e| e.to_string()))
        } else if name.starts_with("launch") {
            Some(LaunchConfig::load(&path).map(|_| ()).map_err(|e| e.to_string()))
        } else {
            None
        };
        for de in [De::Default, De::NoSnippet, De::LastWins] {
            if de == De::LastWins && !name.contains("duplicate") {
                continue;
            }
            let a = after(text, de);
            let holds = |r: &Result<(), String>| r.as_ref().err().is_some_and(|e| e.contains(needle));
            let mut line = match (&b, &a) {
                (Err(x), Err(y)) => format!(
                    "serde error before: {x}\nserde error after [{de:?}]:\n{y}\nasserted substring {needle:?} present: before {} after {}",
                    holds(&b),
                    holds(&a)
                ),
                _ => format!("outcome differs: before {b:?} after [{de:?}] {a:?}"),
            };
            if let (Some(Err(full)), Err(x), Err(y)) = (&real, &b, &a) {
                line.push_str(&format!(
                    "\nloader message before: {full}\nloader message after:\n{}",
                    full.replace(x.as_str(), y.as_str())
                ));
            }
            s.differ(line, name);
        }
        let a = pathed(text);
        let line = match (&b, &a) {
            (Err(x), Err(y)) if x == y => format!("[NoSnippet + serde_path_to_error] identical to today: {x}"),
            (Err(x), Err(y)) => format!(
                "serde error before: {x}\nserde error after [NoSnippet + serde_path_to_error, `path: message`]: {y}\nasserted substring {needle:?} present after: {}",
                y.contains(needle)
            ),
            _ => format!("outcome differs: before {b:?} after [pathed] {a:?}"),
        };
        s.differ(line, name);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

pub fn timing(c: &Corpus) {
    let all: Vec<&Doc> = c
        .graphs
        .iter()
        .chain(&c.workflows)
        .chain(&c.other_yaml)
        .chain(&c.maintenance)
        .chain(&c.templates)
        .chain(&c.rules)
        .collect();
    let largest = all.iter().max_by_key(|d| d.text.len()).expect("a document");
    let largest_host = all.iter().filter(|d| d.name.starts_with("host:")).max_by_key(|d| d.text.len());
    println!("\n## parse time");
    for doc in std::iter::once(*largest).chain(largest_host.copied()) {
        println!("document: {} ({} bytes)", doc.name, doc.text.len());
        let n = 1000;
        let time = |f: &dyn Fn()| {
            for _ in 0..20 {
                f();
            }
            let mut samples: Vec<f64> = (0..n)
                .map(|_| {
                    let t = Instant::now();
                    f();
                    t.elapsed().as_secs_f64() * 1e6
                })
                .collect();
            samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
            (samples[0], samples[n / 2])
        };
        let text = doc.text.as_str();
        for (label, f) in [
            ("serde_norway -> serde_json::Value", &(|| drop(norway::<Json>(text).unwrap())) as &dyn Fn()),
            ("serde_norway -> serde_norway::Value", &|| drop(serde_norway::from_str::<serde_norway::Value>(text).unwrap())),
            ("serde-saphyr -> serde_json::Value", &|| drop(saphyr::<Json>(text, De::Default).unwrap())),
            ("serde-saphyr (no snippet, no budget) -> serde_json::Value", &|| {
                let mut o = serde_saphyr::Options::default();
                o.with_snippet = false;
                o.budget = None;
                drop(serde_saphyr::from_str_with_options::<Json>(text, o).unwrap())
            }),
            ("serde-saphyr (emit_comments off) -> serde_json::Value", &|| {
                let mut o = serde_saphyr::Options::default();
                o.emit_comments = false;
                drop(serde_saphyr::from_str_with_options::<Json>(text, o).unwrap())
            }),
            ("saphyr 0.2 -> Yaml tree (+ conversion to JSON)", &|| drop(tree(text).unwrap())),
        ] {
            let (min, median) = time(f);
            println!("- {label}: min {min:.1} µs, median {median:.1} µs ({n} runs)");
        }
        if text.contains("members:") {
            let (m, p) = time(&|| drop(norway::<GraphConfig>(text).unwrap()));
            println!("- serde_norway -> GraphConfig: min {m:.1} µs, median {p:.1} µs");
            let (m, p) = time(&|| drop(saphyr::<GraphConfig>(text, De::Default).unwrap()));
            println!("- serde-saphyr -> GraphConfig: min {m:.1} µs, median {p:.1} µs");
        }
    }
}

/// Supplementary probes: scalar spellings, duplicate keys and string shapes the real corpus
/// does not happen to contain but which a user-written front matter, schedule or config can.
pub fn probes(r: &mut Report) {
    let s = r.section("probes: schema-less reads of spellings absent from the corpus");
    let spellings = [
        "yes", "no", "on", "off", "y", "n", "True", "TRUE", "~", "null", "", "0o17", "017", "0x1F", "1_000",
        "+1", ".inf", ".nan", "1e3", "1.0", "2026-10-12", "2026-09-23T04:00:35+00:00", "12:30", "1:20:30",
        "18446744073709551616", "-0", "0.30000000000000004", "!!str 1", "!custom x", "'quoted'",
    ];
    for sp in spellings {
        let text = format!("v: {sp}\n");
        s.compared += 1;
        let b = norway::<Json>(&text);
        for de in [De::Default, De::StrictBooleans, De::RejectTags, De::NonFiniteAsString] {
            if de == De::RejectTags && !sp.starts_with('!') || de == De::NonFiniteAsString && !sp.starts_with('.') {
                continue;
            }
            let a = saphyr::<Json>(&text, de);
            if b.as_ref().map(Json::to_string) != a.as_ref().map(Json::to_string) {
                s.differ(format!("`v: {sp}` [{de:?}]: before {b:?} after {a:?}"), "probe");
            }
        }
        let t = tree(&text);
        if b.as_ref().map(Json::to_string) != t.as_ref().map(Json::to_string) {
            s.differ(format!("`v: {sp}` [saphyr tree]: before {b:?} after {t:?}"), "probe");
        }
    }
    for (name, text) in [
        ("duplicate key", "title: a\ntitle: b\n"),
        ("integer key", "1: a\n"),
        ("boolean key", "true: a\n"),
        ("null key", "~: a\n"),
        ("anchor and alias", "a: &x [1, 2]\nb: *x\n"),
        ("merge key", "base: &b {x: 1}\nchild:\n  <<: *b\n  y: 2\n"),
        ("two documents", "a: 1\n---\nb: 2\n"),
        ("empty document", ""),
        ("comment only", "# nothing\n"),
        ("byte-order mark", "\u{feff}a: 1\n"),
        ("CRLF line ends", "a: 1\r\nb: two\r\n"),
        ("tab after colon", "a:\t1\n"),
        ("complex key", "? [a, b]\n: c\n"),
    ] {
        s.compared += 1;
        let b = norway::<Json>(text);
        for de in [De::Default, De::LastWins, De::MergeAsOrdinary] {
            if de == De::LastWins && name != "duplicate key" || de == De::MergeAsOrdinary && name != "merge key" {
                continue;
            }
            let a = saphyr::<Json>(text, de);
            if b.as_ref().map(Json::to_string) != a.as_ref().map(Json::to_string) {
                s.differ(format!("{name} {text:?} [{de:?}]:\nbefore {b:?}\nafter  {a:?}"), "probe");
            }
        }
        let t = tree(text);
        if b.as_ref().map(Json::to_string) != t.as_ref().map(Json::to_string) {
            s.differ(format!("{name} {text:?} [saphyr tree]:\nbefore {b:?}\nafter  {t:?}"), "probe");
        }
    }

    let s = r.section("probes: typed reads of spellings into String / u32 / bool / Option<String>");
    #[derive(Debug, PartialEq, Deserialize)]
    struct S {
        v: String,
    }
    #[derive(Debug, PartialEq, Deserialize)]
    struct U {
        v: u32,
    }
    #[derive(Debug, PartialEq, Deserialize)]
    struct B {
        v: bool,
    }
    #[derive(Debug, PartialEq, Deserialize)]
    struct O {
        v: Option<String>,
    }
    for sp in ["yes", "on", "1", "1.0", "~", "2026-10-12", "017", "0x10", "\"5\"", "+5", "True", "", "null"] {
        let text = format!("v: {sp}\n");
        s.compared += 1;
        macro_rules! cmp {
            ($t:ty) => {{
                let b = norway::<$t>(&text).map(|x| format!("{x:?}"));
                for de in [De::Default, De::StrictBooleans] {
                    let a = saphyr::<$t>(&text, de).map(|x| format!("{x:?}"));
                    match (&b, &a) {
                        (Ok(x), Ok(y)) if x == y => {}
                        (Err(_), Err(_)) => {}
                        _ => s.differ(
                            format!("`v: {sp}` into {} [{de:?}]: before {b:?}\nafter {a:?}", stringify!($t)),
                            "probe",
                        ),
                    }
                }
            }};
        }
        cmp!(S);
        cmp!(U);
        cmp!(B);
        cmp!(O);
    }

    let s = r.section("probes: emitted bytes of string and value shapes absent from the corpus");
    let long = "word ".repeat(25).trim_end().to_owned();
    let values: Vec<(&str, Json)> = vec![
        ("yes", json!("yes")), ("no", json!("no")), ("on", json!("on")), ("off", json!("off")), ("y", json!("y")),
        ("true", json!("true")), ("null", json!("null")), ("~", json!("~")), ("empty", json!("")),
        ("leading space", json!("  lead")), ("trailing space", json!("trail ")), ("1", json!("1")),
        ("1.0", json!("1.0")), ("0x1F", json!("0x1F")), ("017", json!("017")), ("1e3", json!("1e3")),
        ("date", json!("2026-10-12")), ("timestamp", json!("2026-09-23T04:00:35.037866+00:00")),
        ("12:30", json!("12:30")), ("colon space", json!("a: b")), ("dash", json!("- x")), ("hash", json!("#x")),
        ("space hash", json!("x #y")), ("apostrophe", json!("it's")), ("double quote", json!("say \"hi\"")),
        ("two lines", json!("line1\nline2")), ("two lines + newline", json!("line1\nline2\n")),
        ("trailing blank lines", json!("trailing\n\n")), ("blank line inside", json!("a\n\nb\n")),
        ("tab", json!("\ttab")), ("unicode", json!("é — ü")), ("emoji", json!("🎉 done")),
        ("long", json!(long)), ("braces", json!("{x}")), ("brackets", json!("[x]")), ("star", json!("*x")),
        ("ampersand", json!("&x")), ("bang", json!("!x")), ("percent", json!("%x")), ("at", json!("@x")),
        ("backtick", json!("`x`")), ("pipe", json!("|")), ("gt", json!(">")), ("backslash", json!("back\\slash")),
        ("bell", json!("\u{7}")), ("float 1.0", json!(1.0)), ("float 1e300", json!(1e300)), ("float 0.1", json!(0.1)),
        ("i64 min", json!(i64::MIN)), ("u64 max", json!(u64::MAX)), ("bool", json!(true)), ("null value", Json::Null),
        ("empty map", json!({})), ("empty list", json!([])), ("nested empties", json!({"a": {}, "b": []})),
        ("list of maps", json!([{"a": 1, "b": 2}, {"c": 3}])), ("list of lists", json!([[1, 2], [3]])),
        ("map in list in map", json!({"k": [{"x": [1]}]})), ("key needing quotes", json!({"a: b": 1, "on": 2, "": 3})),
        // Whitespace the blank-line postprocess must preserve.
        ("spaces-only line inside", json!("a\n   \nb\n")), ("two blank lines inside", json!("a\n\n\nb\n")),
        ("leading-space first line", json!("  lead\n\nx\n")), ("trailing space-only line", json!("x\n \n")),
        ("tab-only line inside", json!("tab\n\t\nend\n")), ("trailing spaces on a line", json!("a  \n\nb\n")),
        ("blank line, no final newline", json!("a\n\nb")), ("only newlines", json!("\n\n")),
        ("nested multi-line in list", json!([{"body": "## What\n\nx\n\n## Why\ny"}, "p\n\nq"])),
        ("multi-line key-like content", json!("k: |\n\n  v\n")),
    ];
    for (name, v) in values {
        s.compared += 1;
        let doc = json!({ "k": v });
        let before = serde_norway::to_string(&doc).unwrap();
        for (label, opts) in ser_variants().into_iter().filter(|(l, _)| listed(l)) {
            let after = emit(&doc, label, opts);
            if before != after {
                s.differ(format!("{name} [{label}]\nbefore: {before:?}\nafter:  {after:?}"), "probe");
            }
            let strict = saphyr::<Json>(&after, De::StrictBooleans).map(|x| x.to_string());
            if strict != Ok(doc.to_string()) {
                s.differ(format!("{name} [{label}] does not round-trip through a strict_booleans reader: wrote {after:?}; reads {strict:?}"), "probe");
            }
            let back_s = saphyr::<Json>(&after, De::Default).map(|x| x.to_string());
            let back_n = norway::<Json>(&after).map(|x| x.to_string());
            if back_s != Ok(doc.to_string()) || back_n != Ok(doc.to_string()) {
                s.differ(
                    format!("{name} [{label}] does not round-trip: wrote {after:?}; serde-saphyr reads {back_s:?}; serde_norway reads {back_n:?}"),
                    "probe",
                );
            }
        }
        if norway::<Json>(&before).map(|x| x.to_string()) != Ok(doc.to_string()) {
            s.notes.push(format!("{name}: today's own output does not round-trip through today: {before:?}"));
        }
    }
}


/// Every read configuration against today, for each scalar and key difference the probes
/// found, so a claim that no option reproduces today rests on a measurement.
pub fn option_matrix(r: &mut Report) {
    let s = r.section("option matrix: each probe difference under every measured serde-saphyr read option");
    #[derive(Debug, PartialEq, Deserialize)]
    struct S {
        v: String,
    }
    #[derive(Debug, PartialEq, Deserialize)]
    struct U {
        v: u32,
    }
    #[derive(Debug, PartialEq, Deserialize)]
    struct B {
        v: bool,
    }
    type Probe = (&'static str, String, fn(&str) -> Result<String, String>, fn(&str, De) -> Result<String, String>);
    fn nj(t: &str) -> Result<String, String> { norway::<Json>(t).map(|v| v.to_string()) }
    fn sj(t: &str, d: De) -> Result<String, String> { saphyr::<Json>(t, d).map(|v| v.to_string()) }
    fn ns(t: &str) -> Result<String, String> { norway::<S>(t).map(|v| format!("{v:?}")) }
    fn ss(t: &str, d: De) -> Result<String, String> { saphyr::<S>(t, d).map(|v| format!("{v:?}")) }
    fn nu(t: &str) -> Result<String, String> { norway::<U>(t).map(|v| format!("{v:?}")) }
    fn su(t: &str, d: De) -> Result<String, String> { saphyr::<U>(t, d).map(|v| format!("{v:?}")) }
    fn nb(t: &str) -> Result<String, String> { norway::<B>(t).map(|v| format!("{v:?}")) }
    fn sb(t: &str, d: De) -> Result<String, String> { saphyr::<B>(t, d).map(|v| format!("{v:?}")) }
    let mut probes: Vec<Probe> = Vec::new();
    for sp in ["yes", "no", "on", "off", "y", "n", "017", "1_000", "0x1F", ".inf", ".nan", "18446744073709551616",
               "!custom x", "2026-10-12", "12:30", "1e3"] {
        probes.push(("Value", format!("v: {sp}\n"), nj, sj));
    }
    for text in ["~: a\n", "title: a\ntitle: b\n", "base: &b {x: 1}\nchild:\n  <<: *b\n  y: 2\n"] {
        probes.push(("Value", text.to_owned(), nj, sj));
    }
    for sp in ["", "~", "null", "1", "1.0", "yes", "017", "2026-10-12"] {
        probes.push(("String field", format!("v: {sp}\n"), ns, ss));
    }
    probes.push(("u32 field", "v: \"5\"\n".to_owned(), nu, su));
    probes.push(("u32 field", "v: 017\n".to_owned(), nu, su));
    for sp in ["yes", "on", "True"] {
        probes.push(("bool field", format!("v: {sp}\n"), nb, sb));
    }
    for (into, text, before, after) in probes {
        s.compared += 1;
        let b = before(&text);
        let mut same = Vec::new();
        let mut other = Vec::new();
        for de in ALL_DE {
            let a = after(&text, de);
            // A refusal reproduces a refusal (wording is the error findings' subject).
            let reproduces = match (&b, &a) {
                (Ok(x), Ok(y)) => x == y,
                (Err(_), Err(_)) => true,
                _ => false,
            };
            if reproduces {
                same.push(format!("{de:?}"));
            } else {
                other.push(format!("{de:?} -> {}", match &a {
                    Ok(v) => v.clone(),
                    Err(e) => format!("refused: {}", e.lines().next().unwrap_or("")),
                }));
            }
        }
        if !other.is_empty() {
            s.differ(
                format!(
                    "{text:?} into {into}: today {}\nreproduced by: {}\nnot reproduced by:\n  {}",
                    match &b { Ok(v) => v.clone(), Err(e) => format!("refused: {e}") },
                    if same.is_empty() { "(none of the measured options)".to_owned() } else { same.join(", ") },
                    other.join("\n  ")
                ),
                "probe",
            );
        }
    }
    s.notes.push(format!("options measured: {:?}", ALL_DE));
}
