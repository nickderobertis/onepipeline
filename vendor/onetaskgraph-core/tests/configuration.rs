//! The configuration layer, the credentials file, and resolution into live sources.
//!
//! The journeys that prove this through the binary live in the `onetaskgraph` crate.
//! What is here is the engine's own surface: the layer algebra a document, the
//! environment and the command line all reduce to, the reads that find them, and the
//! step that turns a validated configuration into sources. Nothing is mocked —
//! discovery runs against real temporary directories, because a fake filesystem
//! would only prove that the fake agrees with itself.

use std::collections::BTreeMap;
use std::path::Path;

use onetaskgraph_core::config::{
    self, ConfigError, Layer, Origin, PROJECT_DOCUMENT_NAME, SECRETS_FILE_VARIABLE,
    SECRETS_RELATIVE_PATH, Setting, SettingPath, USER_DOCUMENT_RELATIVE_PATH, documents, merge,
    read_optional, secrets_path, unflatten, user_document_path, value_from_text, variable_for,
};
use onetaskgraph_core::{
    Config, Environment, OutputFormat, PluginKind, Secrets, plugin_for, resolve,
};
use onetaskgraph_plugin_api::{SecretResolver, SourceName};
use serde_json::{Value, json};
use tempfile::TempDir;

/// A path, for tests that read like the settings they name.
fn path(dotted: &str) -> SettingPath {
    SettingPath::parse(dotted).expect("a literal path")
}

/// One setting attributed to a made-up flag.
fn flag(dotted: &str, value: Value) -> Setting {
    Setting {
        key: path(dotted),
        value,
        origin: Origin::Flag {
            flag: format!("--set {dotted}"),
        },
    }
}

/// One document layer over `document`, attributed to a file called `name`.
fn document(name: &str, document: Value) -> Layer {
    Layer::from_document(Path::new(name).to_path_buf(), &document).expect("a mapping")
}

#[test]
fn a_later_layer_replaces_a_setting_an_earlier_one_made() {
    let merged = merge(&[
        document("a.yaml", json!({"page_size": 25})),
        Layer::new(vec![flag("page_size", json!(9))]),
    ]);

    let page_size = &merged[&path("page_size")];
    assert_eq!(page_size.value, json!(9));
    assert_eq!(
        page_size.origin,
        Origin::Flag {
            flag: "--set page_size".to_owned()
        }
    );
    assert_eq!(merged.len(), 1, "one setting, not two");
}

#[test]
fn a_later_layer_setting_a_leaf_replaces_an_earlier_layers_whole_subtree() {
    let merged = merge(&[
        Layer::new(vec![flag(
            "sources.work.config",
            json!({"root": "/one", "depth": 2}),
        )]),
        Layer::new(vec![flag("sources.work.config.root", json!("/two"))]),
    ]);

    assert_eq!(
        merged.keys().cloned().collect::<Vec<_>>(),
        vec![path("sources.work.config.root")],
        "a leaf below a replaced subtree does not survive beside it"
    );
}

#[test]
fn a_later_layer_setting_a_subtree_replaces_an_earlier_layers_leaves_below_it() {
    let merged = merge(&[
        Layer::new(vec![flag("sources.work.config.root", json!("/one"))]),
        Layer::new(vec![flag("sources.work.config", json!({"root": "/two"}))]),
    ]);

    assert_eq!(
        merged.keys().cloned().collect::<Vec<_>>(),
        vec![path("sources.work.config")]
    );
}

#[test]
fn merging_and_unflattening_rebuild_the_document_the_layers_describe() {
    let merged = merge(&[
        document("a.yaml", json!({"page_size": 25})),
        Layer::new(vec![
            flag("sources.work.plugin", json!("in-memory")),
            flag("sources.work.config.root", json!("/notes")),
        ]),
    ]);

    assert_eq!(
        unflatten(&merged),
        json!({
            "page_size": 25,
            "sources": { "work": { "plugin": "in-memory", "config": { "root": "/notes" } } }
        })
    );
}

#[test]
fn an_empty_plugin_block_survives_flattening_as_a_deliberate_setting() {
    let merged = merge(&[document(
        "a.yaml",
        json!({"sources": {"work": {"plugin": "linear", "config": {}}}}),
    )]);
    assert_eq!(merged[&path("sources.work.config")].value, json!({}));
}

#[test]
fn an_empty_document_contributes_nothing() {
    let layer = Layer::from_document(Path::new("a.yaml").to_path_buf(), &Value::Null)
        .expect("an empty document");
    assert!(layer.settings().is_empty());
}

#[test]
fn a_document_that_is_not_a_mapping_is_refused() {
    let error = Layer::from_document(Path::new("a.yaml").to_path_buf(), &json!([1, 2]))
        .expect_err("a list is not a document");
    assert!(error.to_string().contains("a list"), "{error}");
}

#[test]
fn a_setting_path_with_an_empty_segment_addresses_nothing() {
    let error = SettingPath::parse("sources..plugin").expect_err("an empty segment");
    assert_eq!(error.key(), Some("sources..plugin"));
}

#[test]
fn a_textual_value_is_typed_as_far_as_it_reads() {
    assert_eq!(value_from_text("100"), json!(100));
    assert_eq!(value_from_text("-3"), json!(-3));
    assert_eq!(value_from_text("1.5"), json!(1.5));
    assert_eq!(value_from_text("true"), json!(true));
    assert_eq!(value_from_text("false"), json!(false));
    assert_eq!(value_from_text("/tmp/tasks"), json!("/tmp/tasks"));
    assert_eq!(
        value_from_text("github-projects"),
        json!("github-projects"),
        "a hyphen does not make a value arithmetic"
    );
    assert_eq!(
        value_from_text("inf"),
        json!("inf"),
        "a float that has no JSON number stays the text it was"
    );
}

#[test]
fn a_textual_value_containing_a_comma_is_a_list() {
    assert_eq!(value_from_text("work,notes"), json!(["work", "notes"]));
    assert_eq!(value_from_text("1, 2"), json!([1, 2]));
}

#[test]
fn the_variable_for_a_setting_is_the_one_the_contract_documents() {
    assert_eq!(variable_for(&path("page_size")), "ONETASKGRAPH_PAGE_SIZE");
    assert_eq!(
        variable_for(&path("default_sources")),
        "ONETASKGRAPH_DEFAULT_SOURCES"
    );
    assert_eq!(
        variable_for(&path("sources.work.config.root")),
        "ONETASKGRAPH_SOURCES__WORK__CONFIG__ROOT"
    );
    assert_eq!(
        variable_for(&path("sources.gh-main.plugin")),
        "ONETASKGRAPH_SOURCES__GH_MAIN__PLUGIN",
        "a hyphen in a source name is an underscore in the variable"
    );
    assert_eq!(config::ENVIRONMENT_PREFIX, "ONETASKGRAPH_");
}

#[test]
fn a_variable_that_names_no_path_is_refused_rather_than_ignored() {
    let sandbox = TempDir::new().expect("a temporary directory");
    let environment = Environment::from_pairs([
        ("HOME", sandbox.path().to_string_lossy().to_string()),
        ("ONETASKGRAPH_SOURCES__", "in-memory".to_owned()),
    ]);

    let error = config::load(sandbox.path(), &environment, &Layer::default())
        .expect_err("a variable naming no setting");
    assert_eq!(error.key(), Some("ONETASKGRAPH_SOURCES__"));
}

/// A host with a configuration home and a project tree.
struct Host {
    root: TempDir,
}

impl Host {
    fn new() -> Self {
        let root = TempDir::new().expect("a temporary directory");
        std::fs::create_dir_all(root.path().join("home/onetaskgraph")).expect("a config home");
        std::fs::create_dir_all(root.path().join("project/deep/deeper")).expect("a project tree");
        Self { root }
    }

    fn environment(&self) -> Environment {
        Environment::from_pairs([(
            "XDG_CONFIG_HOME",
            self.root.path().join("home").to_string_lossy().to_string(),
        )])
    }

    /// `relative` is spelled with `/` whatever the platform, and joined a component at a
    /// time for that reason: `Path::join` on Windows appends such a literal verbatim, so
    /// `home/onetaskgraph/config.yaml` would come back as `…\home/onetaskgraph\config.yaml`
    /// — a second spelling of a path the layer under test names with `\` throughout, which
    /// no assertion comparing the two can be about the layer.
    fn write(&self, relative: &str, text: &str) -> std::path::PathBuf {
        let path = relative
            .split('/')
            .fold(self.root.path().to_path_buf(), |path, component| {
                path.join(component)
            });
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("the directory");
        std::fs::write(&path, text).expect("written");
        path
    }
}

#[test]
fn documents_are_the_user_level_one_then_the_nearest_one_above_the_working_directory() {
    let host = Host::new();
    let user = host.write(
        &format!("home/{USER_DOCUMENT_RELATIVE_PATH}"),
        "page_size: 11\n",
    );
    let project = host.write(
        &format!("project/{PROJECT_DOCUMENT_NAME}"),
        "page_size: 25\n",
    );

    let found = documents(
        &host.root.path().join("project/deep/deeper"),
        &host.environment(),
    )
    .expect("both documents read");

    assert_eq!(
        found.iter().map(|d| d.path.clone()).collect::<Vec<_>>(),
        vec![user, project],
        "lowest precedence first"
    );
}

#[test]
fn a_nearer_document_hides_one_further_up_rather_than_stacking_with_it() {
    let host = Host::new();
    host.write(
        &format!("project/{PROJECT_DOCUMENT_NAME}"),
        "page_size: 25\n",
    );
    let nearer = host.write(
        &format!("project/deep/{PROJECT_DOCUMENT_NAME}"),
        "page_size: 7\n",
    );

    let found = documents(
        &host.root.path().join("project/deep/deeper"),
        &host.environment(),
    )
    .expect("one document read");

    assert_eq!(
        found.iter().map(|d| d.path.clone()).collect::<Vec<_>>(),
        vec![nearer]
    );
}

#[test]
fn no_documents_at_all_is_the_ordinary_case_rather_than_an_error() {
    let host = Host::new();
    let found =
        documents(&host.root.path().join("project"), &host.environment()).expect("nothing to read");
    assert!(found.is_empty());
}

#[test]
fn a_document_that_cannot_be_read_stops_the_run_rather_than_being_skipped() {
    let host = Host::new();
    // A directory where the user-level document belongs: it exists as far as a read is
    // concerned and cannot be read, without needing a permission this test user might
    // not be able to set.
    std::fs::create_dir_all(
        host.root
            .path()
            .join(format!("home/{USER_DOCUMENT_RELATIVE_PATH}")),
    )
    .expect("a directory in the document's place");

    let error = documents(&host.root.path().join("project"), &host.environment())
        .expect_err("a document that cannot be read");
    assert!(matches!(error, ConfigError::Read { .. }), "{error}");
    assert!(error.to_string().contains("config.yaml"), "{error}");
    assert_eq!(error.key(), None);
}

#[test]
fn the_configuration_home_falls_back_to_home_when_xdg_is_unset_or_empty() {
    let from_home = Environment::from_pairs([("HOME", "/home/someone")]);
    assert_eq!(
        user_document_path(&from_home),
        Some(Path::new("/home/someone/.config").join(USER_DOCUMENT_RELATIVE_PATH))
    );

    let empty_xdg = Environment::from_pairs([("XDG_CONFIG_HOME", ""), ("HOME", "/home/someone")]);
    assert_eq!(
        user_document_path(&empty_xdg),
        user_document_path(&from_home)
    );

    assert_eq!(
        user_document_path(&Environment::default()),
        None,
        "a host that says where nothing lives has no user-level document"
    );
}

#[test]
fn the_credentials_path_is_the_override_when_one_is_set_and_the_default_otherwise() {
    let overridden = Environment::from_pairs([
        (SECRETS_FILE_VARIABLE, "/tmp/elsewhere.env"),
        ("HOME", "/home/someone"),
    ]);
    assert_eq!(
        secrets_path(&overridden),
        Some(Path::new("/tmp/elsewhere.env").to_path_buf())
    );

    let default = Environment::from_pairs([("XDG_CONFIG_HOME", "/cfg")]);
    assert_eq!(
        secrets_path(&default),
        Some(Path::new("/cfg").join(SECRETS_RELATIVE_PATH))
    );

    assert_eq!(secrets_path(&Environment::default()), None);
}

#[test]
fn reading_a_file_that_is_not_there_is_nothing_rather_than_an_error() {
    let host = Host::new();
    assert_eq!(
        read_optional(&host.root.path().join("nothing-here")).expect("not an error"),
        None
    );
}

#[test]
fn the_environment_snapshot_never_debug_prints_a_value() {
    let environment = Environment::from_pairs([("LINEAR_API_KEY", "lin_api_CANARY")]);

    let rendered = format!("{environment:?}");
    assert!(!rendered.contains("lin_api_CANARY"), "{rendered}");
    assert!(rendered.contains("LINEAR_API_KEY"), "{rendered}");
    assert!(rendered.contains("<redacted>"), "{rendered}");
}

#[test]
fn the_environment_snapshot_reads_this_process() {
    // Nothing is asserted about a particular variable: what matters is that the
    // capture succeeds and yields the process's own names, which is what the binary
    // hands every layer below it.
    let captured = Environment::from_process();
    assert_eq!(
        captured.names().count(),
        std::env::vars().count(),
        "every variable this process has is captured"
    );
    assert!(captured.non_empty("").is_none());
}

/// A resolver over a credentials file holding `text`, plus `exported`.
fn secrets(host: &Host, text: &str, exported: &[(&str, &str)]) -> Result<Secrets, ConfigError> {
    host.write(&format!("home/{SECRETS_RELATIVE_PATH}"), text);
    let mut pairs: Vec<(String, String)> = vec![(
        "XDG_CONFIG_HOME".to_owned(),
        host.root.path().join("home").to_string_lossy().to_string(),
    )];
    for (name, value) in exported {
        pairs.push(((*name).to_owned(), (*value).to_owned()));
    }
    Secrets::load(Environment::from_pairs(pairs))
}

#[test]
fn the_credentials_file_answers_the_names_it_defines() {
    let host = Host::new();
    let resolved = secrets(
        &host,
        "# a comment\n\nLINEAR_API_KEY=lin_api_one\nexport GH_PROJECTS_TOKEN=\"ghp_two\"\n",
        &[],
    )
    .expect("the file parses");

    assert!(resolved.get("LINEAR_API_KEY").is_some());
    assert!(
        resolved.get("GH_PROJECTS_TOKEN").is_some(),
        "an `export ` prefix and surrounding quotes are both read"
    );
    assert!(resolved.get("NOTHING_DEFINES_THIS").is_none());
}

#[test]
fn the_process_environment_beats_the_credentials_file_for_a_name_it_defines() {
    let host = Host::new();
    let resolved = secrets(
        &host,
        "LINEAR_API_KEY=from-the-file\n",
        &[("LINEAR_API_KEY", "from-the-shell")],
    )
    .expect("the file parses");

    let report = resolved.report();
    assert_eq!(report.variables.len(), 1);
    assert_eq!(
        report.variables[0].resolved_from,
        onetaskgraph_core::CredentialLayer::Environment
    );
}

#[test]
fn a_credentials_file_line_that_is_not_an_assignment_names_its_line_and_not_its_value() {
    let host = Host::new();
    let error = secrets(
        &host,
        "LINEAR_API_KEY=canary-value\nnot an assignment\n",
        &[],
    )
    .expect_err("a malformed line");

    let message = error.to_string();
    assert!(message.contains(":2"), "{message}");
    assert!(!message.contains("canary-value"), "{message}");
}

#[test]
fn a_credentials_file_key_that_could_not_be_exported_is_refused() {
    let host = Host::new();
    let error = secrets(&host, "not a name=value\n", &[]).expect_err("an unusable name");
    assert!(error.to_string().contains("not a name"), "{error}");
}

#[test]
fn the_resolver_never_debug_prints_a_value() {
    let host = Host::new();
    let resolved = secrets(&host, "LINEAR_API_KEY=lin_api_CANARY\n", &[]).expect("parses");

    let rendered = format!("{resolved:?}");
    assert!(!rendered.contains("lin_api_CANARY"), "{rendered}");
    assert!(rendered.contains("LINEAR_API_KEY"), "{rendered}");
    assert!(rendered.contains("<redacted>"), "{rendered}");
}

#[test]
fn a_host_that_says_where_nothing_lives_resolves_nothing_and_does_not_fail() {
    let resolved = Secrets::load(Environment::default()).expect("nothing to read");
    assert_eq!(resolved.report().path, None);
    assert!(resolved.report().variables.is_empty());
    assert!(resolved.get("LINEAR_API_KEY").is_none());
}

#[test]
fn a_document_with_nothing_in_it_carries_the_built_in_values() {
    let config = Config::from_document(json!({})).expect("an empty document is a configuration");
    assert_eq!(config.page_size().get(), 50);
    assert_eq!(config.output(), OutputFormat::Text);
    assert!(config.sources().is_empty());
    assert_eq!(config.default_sources(), None);
    assert!(config.selected_sources().is_empty());
}

#[test]
fn omitting_default_sources_selects_every_configured_source_in_name_order() {
    let config = Config::from_document(json!({
        "sources": {
            "work": {"plugin": "in-memory"},
            "notes": {"plugin": "in-memory"},
        }
    }))
    .expect("a configuration");

    assert_eq!(
        config
            .selected_sources()
            .iter()
            .map(SourceName::as_str)
            .collect::<Vec<_>>(),
        vec!["notes", "work"]
    );
}

#[test]
fn one_default_source_named_alone_is_read_as_a_list_of_one() {
    let config = Config::from_document(json!({
        "default_sources": "work",
        "sources": {"work": {"plugin": "in-memory"}},
    }))
    .expect("a configuration");

    assert_eq!(
        config.selected_sources(),
        vec![SourceName::new("work").expect("a name")]
    );
}

#[test]
fn an_unknown_field_names_the_key_it_was_written_at() {
    let error = Config::from_document(json!({"sources": {"work": {"plugn": "in-memory"}}}))
        .expect_err("a mistyped field");
    assert_eq!(error.key(), Some("sources.work.plugn"));
}

#[test]
fn a_default_source_naming_something_unconfigured_is_refused() {
    let error = Config::from_document(json!({"default_sources": ["nope"]}))
        .expect_err("an unconfigured source");
    assert_eq!(error.key(), Some("default_sources"));
    assert!(error.to_string().contains("none are"), "{error}");
}

/// A Rust caller clearing the cache of a source a layer refines and none declares is told
/// which, on one line, rather than that no such source is configured.
#[test]
fn clearing_the_cache_of_an_incomplete_source_names_it_as_incomplete() {
    let config = Config::from_document(json!({"sources": {
        "notes": {"plugin": "in-memory"},
        "partner": {"routes": [{"repositories": ["github.com/example-org/*"], "to": "notes"}]},
    }}))
    .expect("a refinement with nothing to refine still loads");
    let partner = SourceName::new("partner".to_owned()).expect("a usable name");

    let message = onetaskgraph_core::cache::clear(&config, Some(&partner))
        .expect_err("an incomplete source keeps no cache");
    assert!(
        message.starts_with("source \"partner\" is not configured here")
            && message.contains("no configuration layer declares its plugin; next:")
            && !message.contains('\n'),
        "{message}"
    );
}

#[test]
fn a_default_source_that_is_not_a_usable_name_is_refused() {
    let error = Config::from_document(json!({"default_sources": ["Work_1"]}))
        .expect_err("an unusable name");
    assert_eq!(error.key(), Some("default_sources"));
}

/// A configuration naming one source of `kind` over `block`.
fn one_source(kind: &str, block: Value) -> Config {
    source_document(kind, block).expect("a configuration")
}

/// The same one-source document, kept as a `Result` for the blocks that are refused.
///
/// A block a plugin will not accept has no `Config` to be read out of: the check runs
/// inside [`Config::from_document`], so these tests assert on the value that never
/// came into existence rather than on a later call over one that did.
fn source_document(kind: &str, block: Value) -> Result<Config, ConfigError> {
    Config::from_document(json!({"sources": {"work": {"plugin": kind, "config": block}}}))
}

/// A resolver with nothing in it, for sources that need no credential.
fn no_secrets() -> Secrets {
    Secrets::load(Environment::default()).expect("nothing to read")
}

/// The smallest `config:` block `kind` accepts.
///
/// Empty for every plugin that requires nothing, which is most of them; `subprocess` is
/// the exception because a source that is another program is not configured at all until
/// it has been told which program. That is the field being required doing its job, so the
/// block is spelled here rather than the requirement being relaxed to keep one loop
/// uniform.
fn minimal_block(kind: &str) -> Value {
    match kind {
        "subprocess" => json!({"command": "onetaskgraph-source"}),
        "local-md" => json!({"root": "."}),
        _ => json!({}),
    }
}

#[test]
fn every_registered_plugin_declares_a_schema_that_compiles_and_accepts_a_valid_block() {
    for kind in onetaskgraph_core::plugin_kinds() {
        let plugin = plugin_for(kind).expect("the registry names it");
        // `validate_sources` compiles the schema and runs it; a plugin whose schema
        // would not compile panics here rather than in a user's run.
        let config = one_source(plugin.kind(), minimal_block(kind));
        let outcome = onetaskgraph_core::validate_sources(&config);
        assert!(
            outcome.is_ok(),
            "{kind} refuses its own minimal block against its own schema: {outcome:?}"
        );
    }
}

#[test]
fn resolving_builds_the_sources_a_configuration_names_in_name_order() {
    let config = Config::from_document(json!({
        "sources": {
            "work": {"plugin": "in-memory", "config": {}},
            "archive": {"plugin": "in-memory", "config": {}},
        }
    }))
    .expect("a configuration");

    let resolved = resolve(&config, &no_secrets()).expect("both sources build");
    assert_eq!(
        resolved
            .iter()
            .map(|source| source.name().as_str())
            .collect::<Vec<_>>(),
        vec!["archive", "work"]
    );
    // The kind is read back off the source rather than stored beside it, so the pair
    // cannot disagree: a `ResolvedSource` cannot claim a kind its source does not report.
    assert_eq!(resolved[0].kind(), "in-memory");
    assert_eq!(resolved[0].kind(), resolved[0].source().kind());
}

#[test]
fn a_plugin_this_build_does_not_have_is_refused_by_the_key_that_names_it() {
    // Refused while the configuration is being built, not later while it is being
    // resolved: `SourceConfig::plugin` is a `PluginKind`, so a configuration naming a
    // plugin this build does not have never comes into existence to be resolved.
    let error =
        Config::from_document(json!({"sources": {"work": {"plugin": "jira", "config": {}}}}))
            .expect_err("no such plugin");
    assert_eq!(error.key(), Some("sources.work.plugin"));
    assert!(error.to_string().contains("in-memory"), "{error}");
}

/// Unix only: an environment entry holding bytes that are not UTF-8 is something only
/// a byte-oriented `OsString` can express, and Windows environment blocks are UTF-16.
#[cfg(unix)]
#[test]
fn a_snapshot_sorts_what_this_build_can_read_from_what_it_cannot() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    // A process is handed its environment by whoever spawned it, so bytes that are not
    // UTF-8 are an input from outside rather than something that cannot happen.
    let not_utf8 = OsString::from_vec(vec![0x66, 0xff, 0x6f]);
    let environment = Environment::from_os_pairs([
        (
            OsString::from("ONETASKGRAPH_PAGE_SIZE"),
            OsString::from("70"),
        ),
        (OsString::from("ONETASKGRAPH_OUTPUT"), not_utf8.clone()),
        (not_utf8, OsString::from("json")),
    ]);

    assert_eq!(environment.get("ONETASKGRAPH_PAGE_SIZE"), Some("70"));
    assert_eq!(
        environment.unusable().collect::<Vec<_>>(),
        vec!["ONETASKGRAPH_OUTPUT"],
        "a value that is not Unicode keeps its name, so the layer can refuse it by name"
    );
    assert_eq!(
        environment.names().collect::<Vec<_>>(),
        vec!["ONETASKGRAPH_PAGE_SIZE"],
        "a name that is not Unicode is dropped: nothing in this product could spell it"
    );

    let rendered = format!("{environment:?}");
    assert!(rendered.contains("ONETASKGRAPH_OUTPUT"), "{rendered}");
    assert!(
        !rendered.contains("70"),
        "a Debug carries no value: {rendered}"
    );
}

/// Unix only, for the same reason as the snapshot test above.
///
/// A reserved variable is not a setting, so a value this build cannot read is not a
/// setting it cannot read: refusing here would refuse the whole invocation over a
/// variable whose value this layer never looks at. `ONETASKGRAPH_LIVE_SEAT_DIR` holds a
/// path, which is where bytes that are not UTF-8 actually turn up.
#[cfg(unix)]
#[test]
fn a_reserved_variable_whose_value_is_not_unicode_is_not_a_setting_either() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let not_utf8 = OsString::from_vec(vec![0x66, 0xff, 0x6f]);
    let environment = Environment::from_os_pairs([
        (OsString::from("ONETASKGRAPH_LIVE_SEAT_DIR"), not_utf8),
        (
            OsString::from("ONETASKGRAPH_PAGE_SIZE"),
            OsString::from("70"),
        ),
    ]);

    let config = config::load(Path::new("/nowhere"), &environment, &Layer::default())
        .expect("a reserved variable is not a setting this build must be able to read");
    assert_eq!(
        config.config.page_size().get(),
        70,
        "the settings beside it are still read"
    );
}

/// Unix only, for the same reason as the snapshot test above.
#[cfg(unix)]
#[test]
fn a_setting_variable_whose_value_is_not_unicode_is_refused_by_name() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let not_utf8 = OsString::from_vec(vec![0x66, 0xff, 0x6f]);
    let environment =
        Environment::from_os_pairs([(OsString::from("ONETASKGRAPH_PAGE_SIZE"), not_utf8.clone())]);

    let error = config::load(Path::new("/nowhere"), &environment, &Layer::default())
        .expect_err("a setting this build cannot read");
    assert_eq!(error.key(), Some("ONETASKGRAPH_PAGE_SIZE"));

    // A variable this product never asked for is nobody's business.
    let ignored = Environment::from_os_pairs([(OsString::from("SOMETHING_ELSE"), not_utf8)]);
    config::load(Path::new("/nowhere"), &ignored, &Layer::default())
        .expect("a variable this build does not read is not a setting it must refuse");
}

#[test]
fn every_plugin_kind_names_the_kind_its_own_plugin_reports() {
    // `PluginKind::as_str` spells each name rather than asking the plugin for it, so
    // that matching a document's `plugin:` costs no allocation. This is what keeps the
    // two spellings from drifting apart.
    for kind in PluginKind::ALL {
        assert_eq!(
            kind.as_str(),
            kind.plugin().kind(),
            "{kind:?} names itself differently from the plugin it builds"
        );
        assert_eq!(PluginKind::parse(kind.as_str()), Some(kind));
    }
    assert_eq!(PluginKind::parse("jira"), None);
    assert_eq!(
        PluginKind::ALL.map(PluginKind::as_str).to_vec(),
        onetaskgraph_core::plugin_kinds()
    );
}

#[test]
fn a_blocks_own_field_is_checked_against_the_plugins_schema_before_the_source_is_built() {
    // A valid block reaches Linear's factory and fails only because no credential was
    // supplied. A mistyped field must instead be reported by schema validation first.
    let reached_build = resolve(&one_source("linear", json!({})), &no_secrets())
        .expect_err("the credential is deliberately absent");
    assert!(
        reached_build.to_string().contains("LINEAR_API_KEY"),
        "a valid block reaches the factory: {reached_build}"
    );

    let refused = source_document("linear", json!({"api_key_env": 7}))
        .expect_err("a field this plugin does not declare");
    assert_eq!(refused.key(), Some("sources.work.config.api_key_env"));
    assert!(
        !refused.to_string().contains("LINEAR_API_KEY"),
        "the block is checked before the factory is called: {refused}"
    );
}

#[test]
fn a_block_a_plugin_refuses_names_the_source_it_belongs_to() {
    let config = one_source(
        "in-memory",
        json!({"tasks": [{"id": "T-1", "title": "one",
        "status": {"category": "todo", "name": "Todo"}, "labels": [], "project": "P-9"}]}),
    );

    let error = resolve(&config, &no_secrets()).expect_err("a task under a project that is absent");
    assert_eq!(error.key(), Some("sources.work"));
    assert!(error.to_string().contains("P-9"), "{error}");
}

#[test]
fn loading_layers_the_documents_the_environment_and_the_flags_in_that_order() {
    let host = Host::new();
    host.write(
        &format!("home/{USER_DOCUMENT_RELATIVE_PATH}"),
        "page_size: 11\noutput: json\n",
    );
    host.write(
        &format!("project/{PROJECT_DOCUMENT_NAME}"),
        "page_size: 25\nsources:\n  work:\n    plugin: in-memory\n",
    );

    let mut pairs: Vec<(String, String)> = vec![
        (
            "XDG_CONFIG_HOME".to_owned(),
            host.root.path().join("home").to_string_lossy().to_string(),
        ),
        ("ONETASKGRAPH_PAGE_SIZE".to_owned(), "70".to_owned()),
    ];
    pairs.push((
        "ONETASKGRAPH_SOURCES__WORK__CONFIG__CAPABILITIES__MAX_PAGE_SIZE".to_owned(),
        "5".to_owned(),
    ));

    let loaded = config::load(
        &host.root.path().join("project/deep"),
        &Environment::from_pairs(pairs),
        &Layer::new(vec![flag("page_size", json!(9))]),
    )
    .expect("the stack loads");

    assert_eq!(loaded.config.page_size().get(), 9, "the flag is on top");
    assert_eq!(
        loaded.config.output(),
        OutputFormat::Json,
        "a setting only the user-level document mentions still lands"
    );

    let origins: BTreeMap<String, String> = loaded
        .effective
        .settings
        .iter()
        .map(|setting| (setting.key.to_string(), setting.origin.to_string()))
        .collect();
    assert_eq!(origins["page_size"], "flag --set page_size");
    assert_eq!(
        origins["sources.work.config.capabilities.max_page_size"],
        "environment ONETASKGRAPH_SOURCES__WORK__CONFIG__CAPABILITIES__MAX_PAGE_SIZE"
    );
    assert!(origins["sources.work.plugin"].starts_with("file "));
    assert_eq!(origins["default_sources"], "default");
}

#[test]
fn a_document_that_will_not_parse_names_the_file_and_the_position() {
    let host = Host::new();
    host.write(
        &format!("project/{PROJECT_DOCUMENT_NAME}"),
        "page_size: [\n",
    );

    let error = config::load(
        &host.root.path().join("project"),
        &host.environment(),
        &Layer::default(),
    )
    .expect_err("a malformed document");

    assert!(matches!(error, ConfigError::Syntax { .. }), "{error}");
    assert!(error.to_string().contains(PROJECT_DOCUMENT_NAME), "{error}");
}

#[test]
fn the_effective_configuration_renders_a_line_per_setting_naming_its_layer() {
    let host = Host::new();
    host.write(
        &format!("project/{PROJECT_DOCUMENT_NAME}"),
        "page_size: 25\n",
    );
    host.write(
        &format!("home/{SECRETS_RELATIVE_PATH}"),
        "LINEAR_API_KEY=x\n",
    );

    let loaded = config::load(
        &host.root.path().join("project"),
        &host.environment(),
        &Layer::default(),
    )
    .expect("the stack loads");

    let rendered = loaded.effective.render_text();
    assert!(rendered.contains("page_size"), "{rendered}");
    assert!(rendered.contains(PROJECT_DOCUMENT_NAME), "{rendered}");
    assert!(rendered.contains("output"), "{rendered}");
    assert!(rendered.contains("default"), "{rendered}");
    assert!(
        rendered.contains("LINEAR_API_KEY  resolved from the secrets file"),
        "{rendered}"
    );
    assert!(!rendered.contains("=x"), "no value reaches the rendering");
}

#[test]
fn the_effective_configuration_says_so_when_there_is_no_credentials_file_to_look_in() {
    let host = Host::new();
    let loaded = config::load(
        &host.root.path().join("project"),
        &host.environment(),
        &Layer::default(),
    )
    .expect("the stack loads");

    assert!(
        loaded
            .effective
            .render_text()
            .contains("(it defines no variables, or is not there)")
    );
}

#[test]
fn a_host_with_no_configuration_home_still_loads() {
    let host = Host::new();
    let loaded = config::load(
        &host.root.path().join("project"),
        &Environment::default(),
        &Layer::default(),
    )
    .expect("nothing configured is a configuration");

    assert_eq!(loaded.config.page_size().get(), 50);
    assert!(
        loaded
            .effective
            .render_text()
            .contains("neither XDG_CONFIG_HOME nor HOME is set")
    );
}

#[test]
fn a_document_root_of_any_other_shape_is_refused_by_what_it_is() {
    for (root, described) in [
        (json!(true), "a boolean"),
        (json!(7), "a number"),
        (json!("page_size: 25"), "a string"),
        (json!([1, 2]), "a list"),
    ] {
        let error = Layer::from_document(Path::new("a.yaml").to_path_buf(), &root)
            .expect_err("only a mapping is a document");
        assert!(error.to_string().contains(described), "{error}");
    }
}

#[test]
fn a_setting_serializes_with_its_key_as_the_dotted_path_a_user_typed() {
    let rendered = serde_json::to_value(flag("sources.work.config.root", json!("/notes")))
        .expect("a setting renders");
    assert_eq!(rendered["key"], "sources.work.config.root");
    assert_eq!(rendered["origin"]["layer"], "flag");
}

#[test]
fn a_number_too_large_for_a_signed_integer_is_still_a_number() {
    assert_eq!(
        value_from_text("9223372036854775808"),
        json!(9_223_372_036_854_775_808_u64)
    );
}

#[test]
fn the_credentials_path_variable_is_not_read_as_a_setting_called_secrets_file() {
    let host = Host::new();
    host.write("elsewhere.env", "LINEAR_API_KEY=x\n");

    let loaded = config::load(
        &host.root.path().join("project"),
        &Environment::from_pairs([(
            SECRETS_FILE_VARIABLE,
            host.root
                .path()
                .join("elsewhere.env")
                .to_string_lossy()
                .to_string(),
        )]),
        &Layer::default(),
    )
    .expect("the override is a path, not a setting");

    assert!(
        !loaded
            .effective
            .settings
            .iter()
            .any(|setting| setting.key.to_string() == "secrets_file")
    );
    assert_eq!(loaded.secrets.report().variables.len(), 1);
}

#[test]
fn the_resolver_hands_back_the_exported_value_when_the_process_environment_defines_one() {
    let host = Host::new();
    let resolved = secrets(
        &host,
        "LINEAR_API_KEY=from-the-file\n",
        &[("LINEAR_API_KEY", "from-the-shell")],
    )
    .expect("the file parses");

    let answered = resolved.get("LINEAR_API_KEY").expect("a value");
    assert_eq!(
        secrecy::ExposeSecret::expose_secret(&answered),
        "from-the-shell"
    );
}

#[test]
fn the_effective_configuration_says_which_layer_answers_for_each_credential() {
    let host = Host::new();
    host.write(
        &format!("home/{SECRETS_RELATIVE_PATH}"),
        "LINEAR_API_KEY=from-the-file\nGH_PROJECTS_TOKEN=also-from-the-file\n",
    );

    let loaded = config::load(
        &host.root.path().join("project"),
        &Environment::from_pairs([
            (
                "XDG_CONFIG_HOME".to_owned(),
                host.root.path().join("home").to_string_lossy().to_string(),
            ),
            ("LINEAR_API_KEY".to_owned(), "from-the-shell".to_owned()),
        ]),
        &Layer::default(),
    )
    .expect("the stack loads");

    let rendered = loaded.effective.render_text();
    assert!(
        rendered.contains("LINEAR_API_KEY     resolved from the environment"),
        "{rendered}"
    );
    assert!(
        rendered.contains("GH_PROJECTS_TOKEN  resolved from the secrets file"),
        "{rendered}"
    );
    assert!(!rendered.contains("from-the-file"), "{rendered}");
    assert!(!rendered.contains("from-the-shell"), "{rendered}");
}

#[test]
fn a_source_name_that_breaks_the_pattern_names_the_key_it_was_written_at() {
    let error = Config::from_document(json!({"sources": {"Work_1": {"plugin": "in-memory"}}}))
        .expect_err("an unusable source name");
    assert_eq!(error.key(), Some("sources.Work_1"));
    assert!(
        error.to_string().contains("^[a-z0-9][a-z0-9-]*$"),
        "{error}"
    );
}

#[test]
fn a_default_source_naming_nothing_configured_lists_the_ones_that_are() {
    let error = Config::from_document(json!({
        "default_sources": ["nope"],
        "sources": {"work": {"plugin": "in-memory"}, "notes": {"plugin": "in-memory"}},
    }))
    .expect_err("an unconfigured source");
    assert!(error.to_string().contains("notes, work"), "{error}");
}

#[test]
fn default_sources_written_as_nothing_is_the_same_as_omitting_it() {
    let config = Config::from_document(json!({"default_sources": Value::Null}))
        .expect("nothing is not a bad value");
    assert_eq!(config.default_sources(), None);
}

#[test]
fn a_document_that_is_not_an_object_at_all_is_refused_at_its_root() {
    let error = Config::from_document(json!(7)).expect_err("a number is not a configuration");
    assert_eq!(error.key(), Some("the document's root"));
}

#[test]
fn a_field_a_plugin_declares_a_schema_for_is_refused_by_the_field_that_is_wrong() {
    let error = source_document("in-memory", json!({"capabilities": {"projekts": "native"}}))
        .expect_err("a mistyped field inside a declared block");

    assert_eq!(
        error.key(),
        Some("sources.work.config.capabilities.projekts"),
        "the unexpected field is lifted out of the object the validator blamed"
    );
    assert!(error.to_string().contains("in-memory"), "{error}");
}

#[test]
fn a_declared_field_given_the_wrong_kind_of_value_is_refused_at_that_field() {
    let error = source_document("in-memory", json!({"tasks": "one"}))
        .expect_err("a list field given a string");
    assert_eq!(error.key(), Some("sources.work.config.tasks"));
}

#[test]
fn a_resolved_source_debug_prints_its_name_and_kind_and_none_of_its_work() {
    let config = one_source(
        "in-memory",
        json!({"tasks": [{"id": "T-1", "title": "a private title",
            "status": {"category": "todo", "name": "Todo"}, "labels": []}]}),
    );
    let resolved = resolve(&config, &no_secrets()).expect("the source builds");

    let rendered = format!("{:?}", resolved[0]);
    assert!(rendered.contains("work"), "{rendered}");
    assert!(rendered.contains("in-memory"), "{rendered}");
    assert!(
        !rendered.contains("a private title"),
        "a source's Debug is not a rendering of the work it holds: {rendered}"
    );
}

#[test]
fn the_output_a_run_asked_for_is_read_from_every_layer_that_still_reads() {
    // What a run whose configuration did not load owes its failure in: the format the
    // layers that can still be read ask for, in the precedence `load` gives them.
    let host = Host::new();
    let project = host.root.path().join("project");
    let json_flag = Layer::new(vec![flag("output", json!("json"))]);
    let text_flag = Layer::new(vec![flag("output", json!("text"))]);

    // Nothing asks: text, whatever else fails.
    assert_eq!(
        config::requested_output(Some(&project), &host.environment(), &Layer::default()),
        OutputFormat::Text
    );

    // A project document that will not parse contributes nothing, and hides nothing above it.
    host.write(
        &format!("project/{PROJECT_DOCUMENT_NAME}"),
        "sources:\n  work:\n   plugin: [not a plugin\n",
    );
    assert_eq!(
        config::requested_output(Some(&project), &host.environment(), &json_flag),
        OutputFormat::Json
    );

    // A readable user document below it still counts, and a flag still beats it.
    host.write(
        &format!("home/{USER_DOCUMENT_RELATIVE_PATH}"),
        "output: json\nsources: 7\n",
    );
    assert_eq!(
        config::requested_output(Some(&project), &host.environment(), &Layer::default()),
        OutputFormat::Json
    );
    assert_eq!(
        config::requested_output(Some(&project), &host.environment(), &text_flag),
        OutputFormat::Text
    );

    // A project document that cannot be read at all — a directory where the file should
    // be — drops that document alone, never the readable user document beside it.
    let obstructed = project.join(PROJECT_DOCUMENT_NAME);
    std::fs::remove_file(&obstructed).expect("the unparseable project document");
    std::fs::create_dir(&obstructed).expect("a directory in the document's place");
    assert_eq!(
        config::requested_output(Some(&project), &host.environment(), &Layer::default()),
        OutputFormat::Json
    );
    // And the user document needs no working directory to be found.
    assert_eq!(
        config::requested_output(None, &host.environment(), &Layer::default()),
        OutputFormat::Json
    );

    // The environment, over the documents and under the flags — and with no working
    // directory to search from, the layers that need none still answer.
    let exported = Environment::from_pairs([("ONETASKGRAPH_OUTPUT", "json")]);
    assert_eq!(
        config::requested_output(None, &exported, &Layer::default()),
        OutputFormat::Json
    );
    assert_eq!(
        config::requested_output(None, &exported, &text_flag),
        OutputFormat::Text
    );

    // A value no format answers to is not a format asked for.
    assert_eq!(
        config::requested_output(
            None,
            &Environment::default(),
            &Layer::new(vec![flag("output", json!("xml"))])
        ),
        OutputFormat::Text
    );
}

fn loaded_root(host: &Host, working_directory: &Path) -> String {
    let loaded = config::load(working_directory, &host.environment(), &Layer::default())
        .expect("the configuration loads");
    loaded.config.sources()[&SourceName::new("notes").unwrap()].config()["root"]
        .as_str()
        .expect("a root is a string")
        .to_owned()
}

#[test]
fn a_relative_root_a_project_document_supplies_is_rebased_on_that_documents_directory() {
    let host = Host::new();
    let document = host.write(
        &format!("project/{PROJECT_DOCUMENT_NAME}"),
        "sources:\n  notes:\n    plugin: local-md\n    config:\n      root: store\n",
    );

    assert_eq!(
        loaded_root(&host, &host.root.path().join("project/deep/deeper")),
        document
            .parent()
            .expect("a document has a directory")
            .join("store")
            .to_string_lossy()
            .to_string(),
        "the document's own directory, not the one two levels below it the run started in"
    );
}

#[test]
fn a_relative_root_the_user_level_document_supplies_is_rebased_on_the_configuration_home() {
    let host = Host::new();
    let document = host.write(
        &format!("home/{USER_DOCUMENT_RELATIVE_PATH}"),
        "sources:\n  notes:\n    plugin: local-md\n    config:\n      root: store\n",
    );

    assert_eq!(
        loaded_root(&host, &host.root.path().join("project")),
        document
            .parent()
            .expect("a document has a directory")
            .join("store")
            .to_string_lossy()
            .to_string(),
        "every document is rebased on its own directory, not only the project's"
    );
}

#[test]
fn an_absolute_root_a_document_supplies_reaches_its_plugin_exactly_as_written() {
    let host = Host::new();
    let absolute = host.root.path().join("elsewhere");
    host.write(
        &format!("project/{PROJECT_DOCUMENT_NAME}"),
        &format!(
            "sources:\n  notes:\n    plugin: local-md\n    config:\n      root: {}\n",
            absolute.display()
        ),
    );

    assert_eq!(
        loaded_root(&host, &host.root.path().join("project")),
        absolute.to_string_lossy().to_string()
    );
}

#[test]
fn a_relative_root_the_environment_supplies_reaches_its_plugin_exactly_as_written() {
    let host = Host::new();
    host.write(
        &format!("project/{PROJECT_DOCUMENT_NAME}"),
        "sources:\n  notes:\n    plugin: local-md\n    config:\n      root: store\n",
    );
    let environment = Environment::from_pairs([
        (
            "XDG_CONFIG_HOME",
            host.root.path().join("home").to_string_lossy().to_string(),
        ),
        (
            "ONETASKGRAPH_SOURCES__NOTES__CONFIG__ROOT",
            "store".to_owned(),
        ),
    ]);

    let loaded = config::load(
        &host.root.path().join("project"),
        &environment,
        &Layer::default(),
    )
    .expect("the configuration loads");
    assert_eq!(
        loaded.config.sources()[&SourceName::new("notes").unwrap()].config()["root"],
        json!("store"),
        "there is no document behind a variable to rebase it on, so the process working \
         directory goes on being what it means"
    );
}

#[test]
fn a_root_behind_the_subprocess_seam_is_left_to_the_child_with_the_documents_directory() {
    let host = Host::new();
    host.write(
        &format!("project/{PROJECT_DOCUMENT_NAME}"),
        "sources:\n  notes:\n    plugin: subprocess\n    config:\n      command: /bin/true\n      \
         settings:\n        kind: local-md\n        config:\n          root: store\n",
    );

    let loaded = config::load(
        &host.root.path().join("project"),
        &host.environment(),
        &Layer::default(),
    )
    .expect("the configuration loads");
    assert_eq!(
        loaded.config.sources()[&SourceName::new("notes").unwrap()].config()["settings"]["config"]
            ["root"],
        json!("store"),
        "what a `settings:` block holds belongs to a plugin this build may never have \
         compiled, so the engine does not claim to know which of its fields is a path"
    );
    assert_eq!(
        loaded.config.sources()[&SourceName::new("notes").unwrap()].document_dir(),
        Some(host.root.path().join("project").as_path()),
        "the child is told instead which document's directory the block is measured from"
    );
}

#[test]
fn a_subprocess_block_the_environment_reaches_into_carries_no_documents_directory() {
    let host = Host::new();
    host.write(
        &format!("project/{PROJECT_DOCUMENT_NAME}"),
        "sources:\n  notes:\n    plugin: subprocess\n    config:\n      command: /bin/true\n      \
         settings:\n        kind: local-md\n        config:\n          root: store\n",
    );
    let environment = Environment::from_pairs([
        (
            "XDG_CONFIG_HOME",
            host.root.path().join("home").to_string_lossy().to_string(),
        ),
        (
            "ONETASKGRAPH_SOURCES__NOTES__CONFIG__SETTINGS__CONFIG__ROOT",
            "store".to_owned(),
        ),
    ]);

    let loaded = config::load(
        &host.root.path().join("project"),
        &environment,
        &Layer::default(),
    )
    .expect("the configuration loads");
    assert_eq!(
        loaded.config.sources()[&SourceName::new("notes").unwrap()].document_dir(),
        None,
        "a relative path from a variable is measured from the working directory, so no \
         document's directory may be sent to be measured from instead"
    );
}

#[test]
fn a_subprocess_block_two_documents_each_supply_part_of_carries_no_documents_directory() {
    // One directory cannot answer for paths two documents wrote, and choosing either would
    // measure the other's from a place nobody wrote them in.
    let host = Host::new();
    host.write(
        &format!("home/{USER_DOCUMENT_RELATIVE_PATH}"),
        "sources:\n  notes:\n    plugin: subprocess\n    config:\n      command: /bin/true\n      \
         settings:\n        kind: local-md\n",
    );
    host.write(
        &format!("project/{PROJECT_DOCUMENT_NAME}"),
        "sources:\n  notes:\n    config:\n      settings:\n        config:\n          \
         root: store\n",
    );

    let loaded = config::load(
        &host.root.path().join("project"),
        &host.environment(),
        &Layer::default(),
    )
    .expect("the configuration loads");
    let notes = &loaded.config.sources()[&SourceName::new("notes").unwrap()];
    assert_eq!(
        notes.config()["settings"],
        json!({"kind": "local-md", "config": {"root": "store"}}),
        "both documents reached the block"
    );
    assert_eq!(notes.document_dir(), None);
}

/// A document under a directory whose name is not valid UTF-8, configuring one source.
#[cfg(target_os = "linux")]
fn under_a_directory_that_is_not_valid_utf8(host: &Host, document: &str) -> std::path::PathBuf {
    use std::ffi::{OsStr, OsString};
    use std::os::unix::ffi::OsStrExt;

    let mut name = OsString::from("project-");
    name.push(OsStr::from_bytes(&[0xff]));
    let directory = host.root.path().join(name);
    std::fs::create_dir_all(&directory).expect("a directory whose name is not valid UTF-8");
    std::fs::write(directory.join(PROJECT_DOCUMENT_NAME), document).expect("a document under it");
    directory
}

/// Linux-only for the reason
/// `a_document_under_a_directory_that_is_not_valid_utf8_refuses_the_setting_it_cannot_resolve`
/// gives: it is the one platform of the three that can hold such a directory.
#[cfg(target_os = "linux")]
#[test]
fn a_subprocess_source_under_a_directory_that_is_not_valid_utf8_is_refused_before_it_is_spawned() {
    let host = Host::new();
    // A program that does not exist: the refusal has to come before anything is spawned,
    // or this would fail as an unrunnable program instead.
    let directory = under_a_directory_that_is_not_valid_utf8(
        &host,
        "sources:\n  notes:\n    plugin: subprocess\n    config:\n      \
         command: onetaskgraph-no-such-plugin-program\n      settings:\n        \
         kind: local-md\n        config:\n          root: store\n",
    );

    let error = config::load(&directory, &host.environment(), &Layer::default())
        .expect_err("a document directory that cannot be sent is refused, never dropped");

    let ConfigError::Setting { key, message, next } = &error else {
        panic!("the block that cannot be told its directory is refused: {error}");
    };
    assert_eq!(
        key, "sources.notes.config.settings",
        "the refusal names the block whose paths it could not place"
    );
    assert!(message.contains("not valid UTF-8"), "{message}");
    assert!(!next.is_empty(), "and says what to change: {error}");
}

#[test]
fn only_the_markdown_folder_declares_a_document_relative_field() {
    // Every plugin is asked rather than a table of which ones answer being kept here, so
    // the assertion is over the whole registry: a plugin that starts declaring one without
    // a journey for it fails here.
    for plugin in onetaskgraph_core::registry() {
        let expected: &[&str] = if plugin.kind() == onetaskgraph_local_md::KIND {
            onetaskgraph_local_md::DOCUMENT_RELATIVE_FIELDS
        } else {
            &[]
        };
        assert_eq!(
            plugin.document_relative_paths(),
            expected,
            "`{}` declares a document-relative field with no rule stated for it",
            plugin.kind()
        );
    }
}

/// A document under a directory this layer cannot write down.
///
/// Linux-only, because of the three platforms this repository's merge path runs it is the
/// one that can *hold* such a directory. A Windows path is a sequence of UTF-16 code units
/// rather than of bytes, so `std::os::unix::ffi` is not there to build the name and there
/// is no such name to build; and macOS is the unix that refuses the name at the
/// filesystem — `create_dir_all` fails there with `Illegal byte sequence` before this test
/// reaches the layer at all, which is the same refusal `tests/plugin.rs` in
/// `onetaskgraph-local-md` records beside its own non-UTF-8 filename. The behaviour under
/// test is not platform-specific — what is platform-specific is being able to build the
/// input.
#[cfg(target_os = "linux")]
#[test]
fn a_document_under_a_directory_that_is_not_valid_utf8_refuses_the_setting_it_cannot_resolve() {
    let host = Host::new();
    let directory = under_a_directory_that_is_not_valid_utf8(
        &host,
        "sources:\n  plans:\n    plugin: local-md\n    config:\n      root: plans\n",
    );

    let error = config::load(&directory, &host.environment(), &Layer::default())
        .expect_err("a resolved path that cannot be written down is refused, never replaced");

    match &error {
        ConfigError::Setting { key, next, .. } => {
            assert_eq!(
                key, "sources.plans.config.root",
                "the refusal names the one setting it could not resolve"
            );
            assert!(!next.is_empty(), "and says what to change: {error}");
        }
        other => panic!("the unresolvable setting is what is refused, not: {other}"),
    }
}

/// A combined route spelled by `--set`, by the environment and in a document loads as one
/// entry and places exactly as the others do.
#[test]
fn a_combined_route_spelled_by_flags_or_the_environment_places_as_a_documents_does() {
    use onetaskgraph_core::config::Placement;
    use onetaskgraph_plugin_api::{Classification, Repository};

    let host = Host::new();
    host.write(
        &format!("project/{PROJECT_DOCUMENT_NAME}"),
        "sources:\n  \
         site:\n    plugin: in-memory\n  \
         public:\n    plugin: in-memory\n  \
         vault:\n    plugin: in-memory\n    visibility: private\n",
    );
    let project = host.root.path().join("project");
    let spelled = [
        (
            "sources.site.routes.0.repositories",
            "github.com/example-org/*",
        ),
        ("sources.site.routes.0.classification", "private"),
        ("sources.site.routes.0.to", "vault"),
        (
            "sources.site.routes.1.repositories",
            "github.com/example-org/*",
        ),
        ("sources.site.routes.1.to", "public"),
    ];
    let by_flags = config::load(
        &project,
        &host.environment(),
        &Layer::new(
            spelled
                .iter()
                .map(|(key, text)| flag(key, value_from_text(text)))
                .collect(),
        ),
    )
    .expect("routes set by flags load");
    let mut pairs = vec![(
        "XDG_CONFIG_HOME".to_owned(),
        host.root.path().join("home").to_string_lossy().to_string(),
    )];
    pairs.extend(
        spelled
            .iter()
            .map(|(key, text)| (variable_for(&path(key)), (*text).to_owned())),
    );
    let by_environment = config::load(&project, &Environment::from_pairs(pairs), &Layer::default())
        .expect("routes set by the environment load");
    let by_document = Config::from_document(json!({"sources": {
        "site": {"plugin": "in-memory", "routes": [
            {"repositories": ["github.com/example-org/*"], "classification": "private",
             "to": "vault"},
            {"repositories": ["github.com/example-org/*"], "to": "public"},
        ]},
        "public": {"plugin": "in-memory"},
        "vault": {"plugin": "in-memory", "visibility": "private"},
    }}))
    .expect("a document's routes load");

    let site = SourceName::new("site".to_owned()).expect("a name");
    let api = [Repository::try_from("github.com/example-org/api".to_owned()).expect("an origin")];
    let document = by_document.routes();
    for loaded in [&by_flags, &by_environment] {
        let routes = loaded.config.routes();
        assert_eq!(routes.of(&site), document.of(&site));
        let entry = &routes.of(&site)[0];
        assert_eq!(entry.classification(), Some(Classification::Private));
        assert_eq!(entry.repositories().len(), 1);
        for classification in [Classification::Private, Classification::Public] {
            assert_eq!(
                routes.place_classified(&site, classification, &api),
                document.place_classified(&site, classification, &api)
            );
        }
        assert_eq!(
            routes.place_classified(&site, Classification::Private, &api),
            Placement {
                destination: SourceName::new("vault".to_owned()).expect("a name"),
                route: Some(0)
            }
        );
        assert_eq!(
            routes.place(&site, &api),
            Placement {
                destination: SourceName::new("public".to_owned()).expect("a name"),
                route: Some(1)
            }
        );
    }
}

/// The README's example of the three kinds of route entry is what the loader reads, and
/// places as the bullets beneath it say: `example-org/special`'s private work at
/// `example-special`, the rest of `example-org`'s at `example-private`, any other private work
/// at `vault`, and a public item of `example-org` by a repository entry wherever it is written.
#[test]
fn the_readmes_combined_route_example_loads_and_places_as_it_says() {
    use onetaskgraph_core::config::Placement;
    use onetaskgraph_plugin_api::{Classification, Repository};

    let readme =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../README.md"))
            .expect("the README is readable");
    let section = readme
        .split_once("## Routing, and plans that span sources")
        .expect("the README has a routing section")
        .1;
    let example = section
        .split_once("a **combined** entry naming both")
        .expect("the routing section names the combined entry")
        .1
        .split_once("```yaml\n")
        .expect("the combined entry has an example")
        .1
        .split_once("```")
        .expect("the example is closed")
        .0;
    let mut routes: Value = serde_saphyr::from_str(example).expect("the example is YAML");
    let mut entries = routes["routes"].take();
    // A public item of `example-org`, placed by a repository entry written below the
    // combined ones, as the README's second bullet says.
    entries
        .as_array_mut()
        .expect("a list of entries")
        .push(json!({"repositories": ["github.com/example-org/*"], "to": "example-public"}));
    let private = json!({"plugin": "in-memory", "visibility": "private"});
    let loaded = Config::from_document(json!({"sources": {
        "site": {"plugin": "in-memory", "routes": entries},
        "example-special": private,
        "example-private": private,
        "vault": private,
        "example-team": {"plugin": "in-memory"},
        "example-public": {"plugin": "in-memory"},
    }}))
    .expect("the README's example loads");

    let routes = loaded.routes();
    let site = SourceName::new("site".to_owned()).expect("a name");
    let at = |name: &str, route: u32| Placement {
        destination: SourceName::new(name.to_owned()).expect("a name"),
        route: Some(route),
    };
    let repository =
        |name: &str| [Repository::try_from(format!("github.com/{name}")).expect("an origin")];
    for (classification, named, placed) in [
        (
            Classification::Private,
            "example-org/special",
            at("example-special", 0),
        ),
        (
            Classification::Private,
            "example-org/api",
            at("example-private", 1),
        ),
        (Classification::Private, "example-team/api", at("vault", 2)),
        (
            Classification::Public,
            "example-team/api",
            at("example-team", 3),
        ),
        (
            Classification::Public,
            "example-org/special",
            at("example-public", 4),
        ),
    ] {
        assert_eq!(
            routes.place_classified(&site, classification, &repository(named)),
            placed,
            "{classification:?} {named}"
        );
    }
}
