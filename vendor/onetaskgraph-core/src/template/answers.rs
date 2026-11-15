// llmlint: ignore-file[code_lands_in_the_domain_that_owns_it] A part of the `template` module; why that module sits in this crate is stated once, at the head of its `mod.rs`.
//! The answers a template is rendered from, before they are checked against its variables.

use std::collections::BTreeMap;

use serde_json::Value;

use super::TemplateError;

/// One variable's answer, as it was given.
#[derive(Debug, Clone, PartialEq)]
enum Answer {
    /// A value already typed — from an answers document, or built in code.
    Value(Value),
    /// Text a command line gave (`--var NAME=VALUE`): the literal value for a `string` or
    /// `text` variable, YAML for every other type. Which of the two is decided by the
    /// declaration, so it is kept as text until it meets one.
    Text(String),
    /// Explicitly no answer: over an earlier set of answers, it removes that name's.
    Unset,
}

/// The answers a template is rendered from, by variable name.
///
/// Parsed from an answers document ([`Answers::from_yaml`]) or built in code. Nothing here
/// knows a template's variables: an answer naming no declared variable, or of the wrong
/// type, is refused when the answers meet the template, by [`super::Template::render`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Answers {
    entries: BTreeMap<String, Answer>,
}

impl Answers {
    /// Answers to nothing: rendered from these, every default is taken and every required
    /// variable is refused.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Read an answers document: a top-level YAML mapping from variable name to value.
    ///
    /// An empty document is no answers.
    ///
    /// # Errors
    ///
    /// [`TemplateError::MalformedAnswers`] for a document that is not YAML, or whose top
    /// level is not a mapping with string keys.
    pub fn from_yaml(text: &str) -> Result<Self, TemplateError> {
        let parsed: Value =
            crate::yaml::from_str(text).map_err(|error| TemplateError::MalformedAnswers {
                message: format!("it is not YAML a mapping of answers can be read from: {error}"),
            })?;
        match parsed {
            Value::Null => Ok(Self::new()),
            Value::Object(entries) => Ok(Self {
                entries: entries
                    .into_iter()
                    .map(|(name, value)| (name, Answer::Value(value)))
                    .collect(),
            }),
            _ => Err(TemplateError::MalformedAnswers {
                message: "its top level is not a mapping from variable name to value".to_owned(),
            }),
        }
    }

    /// Answer `name` with `value`.
    pub fn set(&mut self, name: impl Into<String>, value: Value) -> &mut Self {
        self.entries.insert(name.into(), Answer::Value(value));
        self
    }

    /// Answer `name` with text as a command line gives it: taken literally for a `string` or
    /// `text` variable, and read as YAML for any other type.
    pub fn set_text(&mut self, name: impl Into<String>, text: impl Into<String>) -> &mut Self {
        self.entries.insert(name.into(), Answer::Text(text.into()));
        self
    }

    /// Leave `name` unanswered — and, over earlier answers, take away the one they gave.
    pub fn unset(&mut self, name: impl Into<String>) -> &mut Self {
        self.entries.insert(name.into(), Answer::Unset);
        self
    }

    /// These answers with `later` laid over them: a name `later` answers takes its answer,
    /// a name `later` unsets is unanswered, and every other name keeps what it had.
    #[must_use]
    pub fn overlay(&self, later: &Self) -> Self {
        let mut entries = self.entries.clone();
        for (name, answer) in &later.entries {
            entries.insert(name.clone(), answer.clone());
        }
        Self { entries }
    }

    /// Every name answered, in name order — an unset name is not answered.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries
            .iter()
            .filter(|(_, answer)| !matches!(answer, Answer::Unset))
            .map(|(name, _)| name.as_str())
    }

    /// The answer given for `name`, if any: a typed value, or text still to be read.
    pub(super) fn given(&self, name: &str) -> Option<Given<'_>> {
        match self.entries.get(name)? {
            Answer::Value(value) => Some(Given::Value(value)),
            Answer::Text(text) => Some(Given::Text(text)),
            Answer::Unset => None,
        }
    }
}

/// An answer as [`Answers::given`] hands it over.
pub(super) enum Given<'a> {
    /// Already typed.
    Value(&'a Value),
    /// Command-line text, read by the declaration it meets.
    Text(&'a str),
}
