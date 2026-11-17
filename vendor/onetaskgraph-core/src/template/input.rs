// llmlint: ignore-file[code_lands_in_the_domain_that_owns_it] A part of the `template` module; why that module sits in this crate is stated once, at the head of its `mod.rs`.
//! Which template to render: a file, or a loader document a caller supplies (contract C3b).
//!
//! A caller that holds its own layering of templates resolves it itself and states the result
//! here — the entry to render, the directories and the `(name, source)` pairs its chain
//! resolves over, and optionally the digest it expects. This product reads exactly that and
//! never resolves a caller's layers or runs a caller's command: a caller depends on it, never
//! the reverse, and a recorded `reference` is only ever a string it records and reports.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{Sha256Digest, Template, TemplateError, TemplateLoader};

/// A template loader document: exactly what to render, stated by a caller.
///
/// ```json
/// {"reference": "<string>", "entry": "<name>", "search_path": ["<absolute dir>"],
///  "templates": [{"name": "<name>", "source": "<text>"}], "digest": "sha256:<hex>"}
/// ```
///
/// `reference` is what an item rendered from it records as its provenance `template`,
/// verbatim; `entry` is loaded over the `search_path` directories and then the `templates`
/// pairs, as a [`TemplateLoader`] searches; and `digest`, when present, must be the digest
/// that chain computes. Every other key is ignored.
#[derive(Debug, Clone, PartialEq)]
pub struct LoaderDocument {
    reference: String,
    entry: String,
    search_path: Vec<PathBuf>,
    templates: Vec<(String, String)>,
    digest: Option<Sha256Digest>,
}

impl LoaderDocument {
    /// Every key a loader document is read for, in the order the contract states them — the
    /// reader below takes its keys from here, and the documentation's examples are held to it.
    /// Every other key is ignored.
    pub const KEYS: [&'static str; 5] =
        ["reference", "entry", "search_path", "templates", "digest"];

    /// Read a loader document from its JSON text.
    ///
    /// # Errors
    ///
    /// [`TemplateError::MalformedLoader`] naming what is wrong: text that is not a JSON
    /// object, a missing or empty `reference`, a missing or empty `entry`, a `search_path`
    /// that is not a list of absolute directories, `templates` that are not `{name, source}`
    /// string pairs, or a `digest` that is not a string.
    pub fn from_json(text: &str) -> Result<Self, TemplateError> {
        let malformed = |message: String| TemplateError::MalformedLoader { message };
        let parsed: Value = serde_json::from_str(text)
            .map_err(|error| malformed(format!("it is not JSON: {error}")))?;
        let Value::Object(document) = parsed else {
            return Err(malformed("its top level is not a JSON object".to_owned()));
        };
        let text_at = |key: &str| -> Result<Option<String>, TemplateError> {
            match document.get(key) {
                None => Ok(None),
                Some(Value::String(text)) => Ok(Some(text.clone())),
                Some(_) => Err(malformed(format!("`{key}` is not a string"))),
            }
        };
        let [
            reference_key,
            entry_key,
            search_key,
            templates_key,
            digest_key,
        ] = Self::KEYS;
        let reference = text_at(reference_key)?
            .filter(|reference| !reference.is_empty())
            .ok_or_else(|| malformed(format!("`{reference_key}` is missing or empty")))?;
        let entry = text_at(entry_key)?
            .filter(|entry| !entry.is_empty())
            .ok_or_else(|| malformed(format!("`{entry_key}` is missing or empty")))?;
        let digest = text_at(digest_key)?
            .map(|digest| {
                Sha256Digest::parse(digest)
                    .map_err(|problem| malformed(format!("`{digest_key}`: {problem}")))
            })
            .transpose()?;
        let search_path = match document.get(search_key) {
            None => Vec::new(),
            Some(Value::Array(directories)) => directories
                .iter()
                .enumerate()
                .map(|(index, directory)| match directory {
                    Value::String(directory) if Path::new(directory).is_absolute() => {
                        Ok(PathBuf::from(directory))
                    }
                    Value::String(directory) => Err(malformed(format!(
                        "`{search_key}[{index}]` is {directory:?}, which is not an absolute \
                         directory"
                    ))),
                    _ => Err(malformed(format!(
                        "`{search_key}[{index}]` is not a string"
                    ))),
                })
                .collect::<Result<_, _>>()?,
            Some(_) => return Err(malformed(format!("`{search_key}` is not a list"))),
        };
        let templates = match document.get(templates_key) {
            None => Vec::new(),
            Some(Value::Array(pairs)) => pairs
                .iter()
                .enumerate()
                .map(|(index, pair)| {
                    let field = |key: &str| match pair.get(key) {
                        // A name resolves nothing when it is empty, and an empty source is no
                        // template: neither is a pair a chain can use.
                        Some(Value::String(text)) if !text.is_empty() => Ok(text.clone()),
                        _ => Err(malformed(format!(
                            "`{templates_key}[{index}]` has no non-empty string `{key}`; each \
                             entry is \
                             {{\"name\": <name>, \"source\": <text>}}"
                        ))),
                    };
                    Ok((field("name")?, field("source")?))
                })
                .collect::<Result<_, TemplateError>>()?,
            Some(_) => return Err(malformed(format!("`{templates_key}` is not a list"))),
        };
        Ok(Self {
            reference,
            entry,
            search_path,
            templates,
            digest,
        })
    }

    /// A loader document naming `entry`, recorded as `reference`, over nothing yet.
    ///
    /// # Errors
    ///
    /// [`TemplateError::MalformedLoader`] for an empty `reference` or `entry`, as
    /// [`LoaderDocument::from_json`] refuses one.
    pub fn new(
        reference: impl Into<String>,
        entry: impl Into<String>,
    ) -> Result<Self, TemplateError> {
        let (reference, entry) = (reference.into(), entry.into());
        for (key, value) in [("reference", &reference), ("entry", &entry)] {
            if value.is_empty() {
                return Err(TemplateError::MalformedLoader {
                    message: format!("`{key}` is missing or empty"),
                });
            }
        }
        Ok(Self {
            reference,
            entry,
            search_path: Vec::new(),
            templates: Vec::new(),
            digest: None,
        })
    }

    /// Add `directory` to the end of the search path.
    ///
    /// # Errors
    ///
    /// [`TemplateError::MalformedLoader`] for a directory that is not absolute, as
    /// [`LoaderDocument::from_json`] refuses one: a relative one would resolve against
    /// whatever directory the render happens to run in.
    pub fn with_directory(mut self, directory: impl Into<PathBuf>) -> Result<Self, TemplateError> {
        let directory = directory.into();
        if !directory.is_absolute() {
            return Err(TemplateError::MalformedLoader {
                message: format!(
                    "the search directory {} is not an absolute directory",
                    directory.display()
                ),
            });
        }
        self.search_path.push(directory);
        Ok(self)
    }

    /// Register a template's source under `name`, searched after every directory.
    ///
    /// # Errors
    ///
    /// [`TemplateError::MalformedLoader`] for an empty `name` or `source`, as
    /// [`LoaderDocument::from_json`] refuses one.
    pub fn with_template(
        mut self,
        name: impl Into<String>,
        source: impl Into<String>,
    ) -> Result<Self, TemplateError> {
        let (name, source) = (name.into(), source.into());
        if name.is_empty() || source.is_empty() {
            return Err(TemplateError::MalformedLoader {
                message: "a registered template has an empty name or source".to_owned(),
            });
        }
        self.templates.push((name, source));
        Ok(self)
    }

    /// Expect the chain to compute `digest`.
    #[must_use]
    pub fn with_digest(mut self, digest: Sha256Digest) -> Self {
        self.digest = Some(digest);
        self
    }

    /// The directories the entry's chain resolves over, in order.
    #[must_use]
    pub fn search_path(&self) -> &[PathBuf] {
        &self.search_path
    }

    /// What an item rendered from this document records as its provenance `template`.
    #[must_use]
    pub fn reference(&self) -> &str {
        &self.reference
    }

    /// Load the entry over the search path and the registered pairs, and hold its digest to
    /// the stated one.
    ///
    /// # Errors
    ///
    /// Every refusal of [`TemplateLoader::load_name`] — an unreadable or missing search path
    /// directory among them, named — and [`TemplateError::LoaderDigest`] naming both digests
    /// when a stated one is not the one the chain computes.
    pub fn load(&self) -> Result<Template, TemplateError> {
        let loader = self
            .search_path
            .iter()
            .fold(TemplateLoader::new(), |loader, directory| {
                loader.with_directory(directory)
            });
        let loader = self
            .templates
            .iter()
            .fold(loader, |loader, (name, source)| {
                loader.with_template(name, source)
            });
        let template = loader.load_name(&self.entry)?;
        if let Some(stated) = &self.digest
            && stated.as_str() != template.digest()
        {
            return Err(TemplateError::LoaderDigest {
                stated: stated.to_string(),
                computed: template.digest().to_owned(),
            });
        }
        Ok(template)
    }
}

/// Which template a create or a regenerate renders.
#[derive(Debug, Clone, PartialEq)]
pub enum TemplateInput {
    /// A template file, whose chain resolves over `search_path`. Recorded by its absolute
    /// path, which a later regenerate re-reads as a file.
    File {
        /// The template file.
        path: PathBuf,
        /// The directories its `extends`, `include` and `import` names resolve in.
        search_path: Vec<PathBuf>,
    },
    /// A template a caller states as a loader document. Recorded by the document's
    /// `reference`, which is never turned into a location.
    Loader(LoaderDocument),
}

impl TemplateInput {
    /// Load the template.
    ///
    /// # Errors
    ///
    /// As [`TemplateLoader::load_path`] for a file, and [`LoaderDocument::load`] for a loader
    /// document.
    pub fn load(&self) -> Result<Template, TemplateError> {
        match self {
            Self::File { path, search_path } => search_path
                .iter()
                .fold(TemplateLoader::new(), |loader, directory| {
                    loader.with_directory(directory)
                })
                .load_path(path),
            Self::Loader(document) => document.load(),
        }
    }

    /// What an item rendered from this template records as its provenance `template`: a
    /// file's absolute path, or a loader document's `reference`.
    #[must_use]
    pub fn reference(&self) -> String {
        match self {
            Self::File { path, .. } => std::path::absolute(path)
                .unwrap_or_else(|_| path.clone())
                .display()
                .to_string(),
            Self::Loader(document) => document.reference().to_owned(),
        }
    }
}
