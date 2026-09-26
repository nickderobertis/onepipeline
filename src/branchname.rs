//! The **branch-name template**: what a branch a lifecycle node's session cuts
//! says.
//!
//! `onevcs` cuts a session's branch at `onevcs/<token>` unless it is handed a
//! name, and a token tells a person reading `git branch` nothing about the work.
//! What a branch should say — the ticket it is for and the piece of the plan it
//! is — is known only here, where a node sits beside the task it was read out
//! of, so this crate **renders** a name and hands it to `onevcs` as
//! `SessionRequest::branch_name`: a name to cut fresh. Everything that makes the
//! proposal a branch — sanitizing it into a ref, putting the host's prefix in
//! front, and taking the first free numeric suffix — is that library's, and none
//! of it is repeated here.
//!
//! One minijinja template, resolved once at the launch — [`FLAG`] beats
//! [`ENVIRONMENT`], which beats the launch config's [`KEY`], which beats
//! [`DEFAULT_TEMPLATE`] — and retained in the launch record, so every driver that
//! adopts the run names branches the way the launch did. A blank value at any of
//! those layers is that launch naming none, and a run naming none proposes no
//! name: its branches are the ones `onevcs` derives.
//!
//! It is external input, refused at its trust boundary. A template that does not
//! parse is refused at the launch naming where it came from, before a run
//! exists. One that fails to render at a node, or renders to nothing, is that
//! node's settlement under `infrastructure-failure`, carrying the template's own
//! error — never a branch cut under some other name.

use serde_json::{json, Value};

use crate::error::{Error, Result};

/// The flag `start` names a template with.
pub const FLAG: &str = "--branch-template";

/// The environment variable naming a template, below the flag.
pub const ENVIRONMENT: &str = "ONEPIPELINE_BRANCH_TEMPLATE";

/// The launch-config key naming a template, below the environment.
pub const KEY: &str = "branch_template";

/// The launch-config schema version [`KEY`] arrived at.
pub const CONFIG_SCHEMA_VERSION: u32 = 11;

/// The template a launch that names none at any layer renders with:
/// `<task key>/<node id>`, falling back to `<plan name>/<node id>` where the
/// node's task has no key.
pub const DEFAULT_TEMPLATE: &str =
    "{% if task.key %}{{ task.key }}{% else %}{{ plan.name }}{% endif %}/{{ node.id }}";

/// One environment, configured the one way both halves need.
///
/// **Semi-strict** about an undefined value: `task.key` is absent where a source
/// has none, so `{% if task.key %}` has to be a question a template may ask —
/// but printing it, or a variable no namespace carries, is an error rather than
/// an empty segment in a branch name nobody asked for.
fn environment() -> minijinja::Environment<'static> {
    let mut environment = minijinja::Environment::new();
    environment.set_undefined_behavior(minijinja::UndefinedBehavior::SemiStrict);
    environment
}

/// Refuse a template that does not parse, naming where it came from.
///
/// Asked at the launch, of whichever layer won, before a run is minted.
///
/// # Errors
///
/// [`Error::Invalid`] naming `whence` and the parser's own error.
pub(crate) fn parses(template: &str, whence: &str) -> Result<()> {
    environment()
        .template_from_str(template)
        .map(|_| ())
        .map_err(|error| {
            Error::Invalid(format!(
                "{whence}: the branch-name template does not parse: {error}"
            ))
        })
}

/// The template a launch renders its branches with, off the first layer that
/// is there: the flag, then [`ENVIRONMENT`], then the launch config's [`KEY`],
/// then [`DEFAULT_TEMPLATE`].
///
/// A layer that is there and blank is this launch naming none — `None` — and
/// stops the search rather than falling through to the layer under it, exactly
/// as every other launch-config field reads. Whatever wins is parsed here, before
/// a run is minted, and refused naming the layer it came from: the flag, the
/// variable, or the key and the file that carried it.
///
/// # Errors
///
/// [`Error::Invalid`] for a template that does not parse, and for a variable
/// this build cannot read as text.
pub(crate) fn resolve(
    flag: Option<&str>,
    configured: Option<&str>,
    config: Option<&std::path::Path>,
) -> Result<Option<String>> {
    let (template, whence) = match flag {
        Some(flag) => (flag.to_owned(), FLAG.to_owned()),
        None => match std::env::var(ENVIRONMENT) {
            Ok(variable) => (variable, ENVIRONMENT.to_owned()),
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err(Error::Invalid(format!(
                    "{ENVIRONMENT} holds something this build cannot read as text — set it \
                     to a template, or unset it"
                )))
            }
            Err(std::env::VarError::NotPresent) => match configured {
                Some(configured) => (
                    configured.to_owned(),
                    match config {
                        Some(path) => format!("`{KEY}` in {}", path.display()),
                        None => format!("`{KEY}`"),
                    },
                ),
                None => (
                    DEFAULT_TEMPLATE.to_owned(),
                    "the shipped default".to_owned(),
                ),
            },
        },
    };
    if template.trim().is_empty() {
        return Ok(None);
    }
    parses(&template, &whence)?;
    Ok(Some(template))
}

/// Render a template over the variables a node's branch is named from.
///
/// `variables` is the whole namespace — `task`, `plan`, `node` and `run` — as
/// `docs/contract.md` states it. A name that is blank once rendered is refused
/// here too, since a branch cannot be named nothing.
///
/// # Errors
///
/// The renderer's own error, as text, for a template that does not parse or
/// does not render over these variables, and a sentence of its own for one that
/// renders empty.
pub fn render(template: &str, variables: &Value) -> std::result::Result<String, String> {
    let rendered = environment()
        .render_str(template, variables)
        .map_err(|error| error.to_string())?;
    if rendered.trim().is_empty() {
        return Err("it rendered an empty name".to_owned());
    }
    Ok(rendered)
}

/// What one run names its branches with: the template the launch resolved and
/// the plan and run it renders over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Naming {
    /// The template, as the launch record retains it.
    pub template: String,
    /// `plan.name`: `onepipeline.name`, else the project's own title.
    pub plan_name: String,
    /// `plan.id`: the qualified project id the run was launched with.
    pub plan_id: String,
    /// `run`: the run id.
    pub run: String,
}

impl Naming {
    /// What `launch`'s run names its branches with, or `None` for a run whose
    /// launch named no template — including every run launched before there was
    /// one — which proposes no name, so `onevcs` derives each as it always has.
    ///
    /// `plan.name` is read off the plan the launch recorded, which the store's
    /// mapping has already given the project's own title where
    /// `onepipeline.name` states none; a record this build cannot read falls back
    /// to the project's native id rather than to no name at all.
    pub fn of_run(
        paths: &crate::ledger::RunPaths,
        launch: &crate::ledger::LaunchRecord,
    ) -> Option<Self> {
        let template = launch.branch_template()?.to_owned();
        let plan_name = crate::ledger::read_json::<crate::plan::Plan>(&paths.plan())
            .ok()
            .and_then(|plan| plan.name)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| {
                launch
                    .project
                    .split_once(':')
                    .map_or(launch.project.as_str(), |(_, native)| native)
                    .to_owned()
            });
        Some(Self {
            template,
            plan_name,
            plan_id: launch.project.clone(),
            run: paths.run.clone(),
        })
    }

    /// The namespace a node's branch is rendered over.
    ///
    /// `task.key` is left out rather than written null where the node's task has
    /// none, so a template asks for it with `{% if task.key %}`.
    fn variables(&self, node: &crate::plan::Node) -> Value {
        let record = node.task_record.as_ref();
        let mut task = json!({
            "id": record.map_or("", |record| record.id.as_str()),
            "title": record.map_or("", |record| record.title.as_str()),
            "delivers": node.delivers,
        });
        if let Some(key) = record.and_then(|record| record.key.as_deref()) {
            task["key"] = json!(key);
        }
        json!({
            "task": task,
            "plan": {"name": self.plan_name, "id": self.plan_id},
            "node": {"id": node.id},
            "run": self.run,
        })
    }

    /// The name `node`'s session proposes its branch at.
    ///
    /// # Errors
    ///
    /// A sentence naming the node and the template, carrying the renderer's own
    /// error — the settlement's detail as it stands.
    pub fn render(&self, node: &crate::plan::Node) -> std::result::Result<String, String> {
        render(&self.template, &self.variables(node)).map_err(|error| {
            format!(
                "the branch-name template could not name the branch of node '{}', so no \
                 branch was cut: {error} (template: {})",
                node.id, self.template
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Node, TaskRecord};

    fn naming(template: &str) -> Naming {
        Naming {
            template: template.to_owned(),
            plan_name: "demo".to_owned(),
            plan_id: "plans:demo".to_owned(),
            run: "demo-1".to_owned(),
        }
    }

    fn node(key: Option<&str>) -> Node {
        Node {
            id: "build".into(),
            delivers: vec!["tickets:ENG-1".into()],
            task_record: Some(TaskRecord {
                id: "tasks/build.md".into(),
                key: key.map(str::to_owned),
                title: "Build it".into(),
            }),
            ..Node::default()
        }
    }

    #[test]
    fn the_default_names_a_keyed_task_by_its_key_and_any_other_by_its_plan() {
        let naming = naming(DEFAULT_TEMPLATE);
        assert_eq!(
            naming.render(&node(Some("ENG-123"))).as_deref(),
            Ok("ENG-123/build")
        );
        assert_eq!(naming.render(&node(None)).as_deref(), Ok("demo/build"));
        // A node no task was read for has no key either.
        let bare = Node {
            id: "added".into(),
            ..Node::default()
        };
        assert_eq!(naming.render(&bare).as_deref(), Ok("demo/added"));
    }

    #[test]
    fn a_template_printing_a_key_the_task_does_not_have_fails_naming_the_node() {
        let why = naming("{{ task.key }}/x")
            .render(&node(None))
            .expect_err("an undefined key is not an empty segment");
        assert!(why.contains("node 'build'"), "{why}");
        assert!(why.contains("{{ task.key }}/x"), "{why}");
        assert!(why.contains("undefined"), "{why}");
    }

    #[test]
    fn a_template_rendering_nothing_fails() {
        let why = naming("{% if false %}x{% endif %}  ")
            .render(&node(None))
            .expect_err("an empty name");
        assert!(why.contains("empty name"), "{why}");
    }

    #[test]
    fn a_template_that_does_not_parse_is_refused_naming_where_it_came_from() {
        let refused = parses("{{ task.key", FLAG).expect_err("unterminated");
        let message = refused.to_string();
        assert!(message.contains(&format!("{FLAG}: ")), "{message}");
        assert!(message.contains("does not parse"), "{message}");
        parses(DEFAULT_TEMPLATE, KEY).expect("the default parses");
    }
}
