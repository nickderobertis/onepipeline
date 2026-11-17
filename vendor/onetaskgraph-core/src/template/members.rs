// llmlint: ignore-file[code_lands_in_the_domain_that_owns_it] A part of the `template` module; why that module sits in this crate is stated once, at the head of its `mod.rs`.
//! A project template's `members`: the plan graph one `project create` makes beside the
//! project — each member a task rendered from a task template of its own, and the edges between
//! them.
//!
//! [`MemberDeclaration`] is the one declaration of a member's shape, which front matter is read
//! into and both SDKs are generated from:
//!
//! ```yaml
//! members:
//!   - id: "{{ slug }}-build"            # the member's native id
//!     title: "Build {{ goal }}"
//!     template: {file: build.md}        # or {loader: build.loader.json}
//!     answers: {goal: "{{ goal }}"}     # the member template's answers
//!     depends_on: ["{{ slug }}-design"] # other members' ids: each a `blocks` edge
//!     metadata: {myapp.size: 3}         # caller-owned keys
//!     repositories: [github.com/acme/widgets]
//! ```
//!
//! Every string in a member — at any depth of its answers and metadata — is rendered from the
//! project template's own resolved answers before anything else reads it, under the same strict
//! rules as a template body. A rendered answer that was written as a string is read the way
//! `--var` reads one, by the type of the variable it answers; any other value is taken as it is.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Answers, LoaderDocument, RenderedTemplate, TemplateError, TemplateInput, environment};

/// Which task template a member's body is rendered from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum MemberTemplate {
    /// A template file: an absolute path, or one relative to the directory the project template
    /// was read from and then its search path.
    File(String),
    /// A template loader document's file, resolved as `file` is.
    Loader(String),
}

/// One member of a plan graph, as a project template's front matter declares it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemberDeclaration {
    /// The member task's native id, and how other members' `depends_on` name it.
    pub id: String,
    /// Its title.
    pub title: String,
    /// The task template its body is rendered from.
    pub template: MemberTemplate,
    /// The answers its template is rendered with, by variable name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(!skip_serializing_if)]
    pub answers: BTreeMap<String, Value>,
    /// The ids of the members it depends on: each one a `blocks` edge, written before it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(!skip_serializing_if)]
    pub depends_on: Vec<String>,
    /// Caller-owned metadata keys it is created with.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(!skip_serializing_if)]
    pub metadata: BTreeMap<String, Value>,
    /// The repositories it concerns, as normalized origins.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(!skip_serializing_if)]
    pub repositories: Vec<String>,
}

/// A member rendered and ready to create: its declaration with every string rendered, the
/// template its body came from, and the rendering.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedMember {
    /// The declaration, every string of it rendered from the project template's answers.
    pub declaration: MemberDeclaration,
    /// The template it was rendered from.
    pub input: TemplateInput,
    /// Its body, rendered from its own template with its answers.
    pub rendered: RenderedTemplate,
}

/// Read `members` out of front matter, already converted to JSON, refusing by key.
pub(super) fn read(file: &str, value: Value) -> Result<Vec<MemberDeclaration>, TemplateError> {
    let Value::Array(entries) = value else {
        return Err(TemplateError::malformed(
            file,
            Some("members"),
            "is not a list of member declarations",
        ));
    };
    entries
        .into_iter()
        .enumerate()
        .map(|(index, entry)| {
            serde_json::from_value::<MemberDeclaration>(entry).map_err(|error| {
                TemplateError::malformed(
                    file,
                    Some(&format!("members[{index}]")),
                    format!(
                        "is not a member declaration: {error}; a member is {{id, title, \
                         template: {{file}} or {{loader}}, answers, depends_on, metadata, \
                         repositories}}"
                    ),
                )
            })
        })
        .collect()
}

/// Render every member of `template`'s front matter from the project's resolved `answers`,
/// hold their graph to itself, and render each member's body from its own template.
///
/// Relative member template paths are looked for in `base`, in order. The members come back
/// in an order that creates each after every member it depends on, declaration order breaking
/// ties.
///
/// # Errors
///
/// [`TemplateError::Malformed`] for a graph that names a member twice, names a member none
/// declares in `depends_on`, depends on itself or goes round a cycle; and
/// [`TemplateError::Member`] naming the member for every refusal of rendering one — a string
/// that does not render, a template that cannot be found or loaded, and every refusal of its
/// answers, a schema's among them.
pub fn prepare(
    file: &str,
    members: &[MemberDeclaration],
    answers: &BTreeMap<String, Value>,
    base: &[PathBuf],
) -> Result<Vec<PreparedMember>, TemplateError> {
    let mut rendered = Vec::with_capacity(members.len());
    for (index, member) in members.iter().enumerate() {
        rendered.push(render_declaration(index, member, answers)?);
    }
    let order = ordered(file, &rendered)?;
    let mut prepared = Vec::with_capacity(rendered.len());
    for index in order {
        let declaration = rendered[index].clone();
        let named = |error: TemplateError| TemplateError::Member {
            member: declaration.id.clone(),
            error: Box::new(error),
        };
        let input = input_of(&declaration.template, base).map_err(named)?;
        let template = input.load().map_err(named)?;
        let mut given = Answers::new();
        for (name, value) in &declaration.answers {
            match value {
                Value::String(text) => given.set_text(name.clone(), text.clone()),
                other => given.set(name.clone(), other.clone()),
            };
        }
        let body = template.render(&given).map_err(named)?;
        prepared.push(PreparedMember {
            declaration,
            input,
            rendered: body,
        });
    }
    Ok(prepared)
}

/// `member` with every string in it rendered from `answers`.
fn render_declaration(
    index: usize,
    member: &MemberDeclaration,
    answers: &BTreeMap<String, Value>,
) -> Result<MemberDeclaration, TemplateError> {
    let environment = environment();
    let label = if member.id.contains("{{") || member.id.contains("{%") {
        format!("members[{index}]")
    } else {
        member.id.clone()
    };
    let render = |field: &str, text: &str| -> Result<String, TemplateError> {
        environment
            .render_str(text, answers)
            .map_err(|error| TemplateError::Member {
                member: label.clone(),
                error: Box::new(TemplateError::Render {
                    file: None,
                    line: None,
                    name: None,
                    message: format!(
                        "its `{field}` does not render: {}",
                        error
                            .detail()
                            .map_or_else(|| error.kind().to_string(), str::to_owned)
                    ),
                }),
            })
    };
    let value = |field: &str, value: &Value| render_value(value, &|text| render(field, text));
    Ok(MemberDeclaration {
        id: render("id", &member.id)?,
        title: render("title", &member.title)?,
        template: match &member.template {
            MemberTemplate::File(path) => MemberTemplate::File(render("template.file", path)?),
            MemberTemplate::Loader(path) => {
                MemberTemplate::Loader(render("template.loader", path)?)
            }
        },
        answers: member
            .answers
            .iter()
            .map(|(name, answer)| Ok((name.clone(), value("answers", answer)?)))
            .collect::<Result<_, TemplateError>>()?,
        depends_on: member
            .depends_on
            .iter()
            .map(|id| render("depends_on", id))
            .collect::<Result<_, _>>()?,
        metadata: member
            .metadata
            .iter()
            .map(|(key, entry)| Ok((key.clone(), value("metadata", entry)?)))
            .collect::<Result<_, TemplateError>>()?,
        repositories: member
            .repositories
            .iter()
            .map(|origin| render("repositories", origin))
            .collect::<Result<_, _>>()?,
    })
}

/// `value` with every string in it, at any depth, rendered by `render`.
fn render_value(
    value: &Value,
    render: &dyn Fn(&str) -> Result<String, TemplateError>,
) -> Result<Value, TemplateError> {
    Ok(match value {
        Value::String(text) => Value::String(render(text)?),
        Value::Array(entries) => Value::Array(
            entries
                .iter()
                .map(|entry| render_value(entry, render))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(entries) => Value::Object(
            entries
                .iter()
                .map(|(key, entry)| Ok((key.clone(), render_value(entry, render)?)))
                .collect::<Result<_, TemplateError>>()?,
        ),
        scalar => scalar.clone(),
    })
}

/// The order to create `members` in: each after every member it depends on, declaration
/// order breaking ties — refusing a graph that does not hold together.
fn ordered(file: &str, members: &[MemberDeclaration]) -> Result<Vec<usize>, TemplateError> {
    let refused = |message: String| TemplateError::malformed(file, Some("members"), message);
    let mut at: HashMap<&str, usize> = HashMap::new();
    for (index, member) in members.iter().enumerate() {
        if member.id.trim().is_empty() {
            return Err(refused(format!("member {index} renders an empty `id`")));
        }
        if at.insert(member.id.as_str(), index).is_some() {
            return Err(refused(format!("name the member {:?} twice", member.id)));
        }
    }
    for member in members {
        for needed in &member.depends_on {
            if needed == &member.id {
                return Err(refused(format!("make {:?} depend on itself", member.id)));
            }
            if !at.contains_key(needed.as_str()) {
                return Err(refused(format!(
                    "make {:?} depend on {needed:?}, which no member is",
                    member.id
                )));
            }
        }
    }
    let mut placed: HashSet<usize> = HashSet::new();
    let mut order = Vec::with_capacity(members.len());
    while order.len() < members.len() {
        let next = (0..members.len()).find(|index| {
            !placed.contains(index)
                && members[*index]
                    .depends_on
                    .iter()
                    .all(|needed| placed.contains(&at[needed.as_str()]))
        });
        let Some(next) = next else {
            let left: Vec<&str> = (0..members.len())
                .filter(|index| !placed.contains(index))
                .map(|index| members[index].id.as_str())
                .collect();
            return Err(refused(format!(
                "go round a cycle of dependencies among {}",
                left.join(", ")
            )));
        };
        placed.insert(next);
        order.push(next);
    }
    Ok(order)
}

/// The template a member names, looked for in `base` when its path is relative.
///
/// # Errors
///
/// [`TemplateError::NotFound`] for a path no base directory holds, and every refusal of
/// reading a loader document.
pub fn input_of(
    template: &MemberTemplate,
    base: &[PathBuf],
) -> Result<TemplateInput, TemplateError> {
    let (MemberTemplate::File(named) | MemberTemplate::Loader(named)) = template;
    let found = locate(named, base)?;
    let search_path: Vec<PathBuf> = found
        .parent()
        .map(Path::to_path_buf)
        .into_iter()
        .chain(base.iter().cloned())
        .collect();
    match template {
        MemberTemplate::File(_) => Ok(TemplateInput::File {
            path: found,
            search_path,
        }),
        MemberTemplate::Loader(_) => {
            let text =
                std::fs::read_to_string(&found).map_err(|error| TemplateError::Unreadable {
                    name: found.display().to_string(),
                    message: error.to_string(),
                })?;
            LoaderDocument::from_json(&text).map(TemplateInput::Loader)
        }
    }
}

fn locate(named: &str, base: &[PathBuf]) -> Result<PathBuf, TemplateError> {
    let path = Path::new(named);
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    base.iter()
        .map(|directory| directory.join(path))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| TemplateError::NotFound {
            name: named.to_owned(),
            referenced_from: Some("members".to_owned()),
            searched: base
                .iter()
                .map(|directory| directory.display().to_string())
                .collect(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn member(id: &str, depends_on: &[&str]) -> MemberDeclaration {
        MemberDeclaration {
            id: id.to_owned(),
            title: id.to_owned(),
            template: MemberTemplate::File("t.md".to_owned()),
            answers: BTreeMap::new(),
            depends_on: depends_on.iter().map(|id| (*id).to_owned()).collect(),
            metadata: BTreeMap::new(),
            repositories: Vec::new(),
        }
    }

    #[test]
    fn members_are_ordered_after_what_they_depend_on_and_a_broken_graph_is_refused() {
        let members = [
            member("c", &["a", "b"]),
            member("a", &[]),
            member("b", &["a"]),
        ];
        assert_eq!(ordered("p.md", &members).expect("a graph"), vec![1, 2, 0]);
        for (members, says) in [
            (vec![member("a", &[]), member("a", &[])], "twice"),
            (vec![member("a", &["z"])], "which no member is"),
            (vec![member("a", &["a"])], "on itself"),
            (vec![member("a", &["b"]), member("b", &["a"])], "cycle"),
        ] {
            let refused = ordered("p.md", &members).expect_err("refused");
            assert!(refused.to_string().contains(says), "{refused}");
        }
    }

    #[test]
    fn every_string_of_a_member_is_rendered_from_the_project_answers() {
        let mut declared = member("{{ slug }}-a", &["{{ slug }}-b"]);
        declared.answers.insert(
            "goal".into(),
            json!({"nested": ["{{ goal }}"], "count": 3, "text": "Goal {{ goal }}"}),
        );
        declared
            .metadata
            .insert("myapp.of".into(), json!("{{ slug }}"));
        let answers: BTreeMap<String, Value> =
            [("slug".into(), json!("p")), ("goal".into(), json!("ship"))].into();
        let rendered = render_declaration(0, &declared, &answers).expect("renders");
        assert_eq!(rendered.id, "p-a");
        assert_eq!(rendered.depends_on, vec!["p-b"]);
        assert_eq!(
            rendered.answers["goal"],
            json!({"nested": ["ship"], "count": 3, "text": "Goal ship"})
        );
        assert_eq!(rendered.metadata["myapp.of"], json!("p"));
        let refused = render_declaration(0, &member("{{ nothing }}", &[]), &answers)
            .expect_err("an undefined name");
        assert!(
            matches!(&refused, TemplateError::Member { member, .. } if member == "members[0]"),
            "{refused}"
        );
    }
}
