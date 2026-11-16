//! The errors this crate returns. Every one names the problem and never a term, a
//! phrase, a replacement or any other declaration content, and none renders a path:
//! a [`DeclarationError`] carries the file it is about as a separate
//! [`path`](DeclarationError::path) field, which a consumer prints only as a screened
//! [`Locator`](crate::Locator).

use std::fmt;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What is wrong with a declaration, or with reading one.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum DeclarationErrorKind {
    /// The declaration could not be read at all (an I/O failure, or not a regular file).
    Unreadable,
    /// The bytes are not a document of the declared encoding, or a member has the wrong
    /// shape (a list where a string belongs, a duplicate key, a tag, a reader bound).
    Malformed,
    /// The document is larger than this crate reads.
    TooLarge,
    /// The declaration carries a member the model does not define.
    UnknownMember,
    /// The model version is missing, not a whole number, or neither 1 nor 2.
    UnsupportedVersion,
    /// A member the model adds at version 2 appears in a version-1 declaration.
    MemberRequiresVersion2,
    /// A term, exception term, replacement term or replacement string is empty once
    /// normalized.
    EmptyTerm,
    /// A phrase is empty once normalized.
    EmptyPhrase,
    /// Two entries of one member name one term.
    DuplicateEntry,
    /// A replacement entry both replaces and rejects, or does neither.
    ReplacementShape,
    /// `exceptions` appears in a declaration that is not a repository's own.
    ExceptionsOutsideRepository,
    /// An exception names no term the repository's own rules carry.
    ExceptionNamesNoTerm,
    /// One directory holds more than one declaration file (a storage binding's refusal).
    AmbiguousFiles,
}

impl DeclarationErrorKind {
    /// The kind's stable kebab-case name, as it serializes.
    pub fn name(self) -> &'static str {
        match self {
            Self::Unreadable => "unreadable",
            Self::Malformed => "malformed",
            Self::TooLarge => "too-large",
            Self::UnknownMember => "unknown-member",
            Self::UnsupportedVersion => "unsupported-version",
            Self::MemberRequiresVersion2 => "member-requires-version-2",
            Self::EmptyTerm => "empty-term",
            Self::EmptyPhrase => "empty-phrase",
            Self::DuplicateEntry => "duplicate-entry",
            Self::ReplacementShape => "replacement-shape",
            Self::ExceptionsOutsideRepository => "exceptions-outside-repository",
            Self::ExceptionNamesNoTerm => "exception-names-no-term",
            Self::AmbiguousFiles => "ambiguous-files",
        }
    }

    fn describe(self) -> &'static str {
        match self {
            Self::Unreadable => "it cannot be read",
            Self::Malformed => "it is not a well-formed declaration",
            Self::TooLarge => "it is larger than a declaration may be",
            Self::UnknownMember => "it carries a member the model does not define",
            Self::UnsupportedVersion => "its model version is missing or is neither 1 nor 2",
            Self::MemberRequiresVersion2 => "it uses a version-2 member under model version 1",
            Self::EmptyTerm => "a term or replacement is empty once normalized",
            Self::EmptyPhrase => "a phrase is empty once normalized",
            Self::DuplicateEntry => "two entries name one term",
            Self::ReplacementShape => {
                "a replacement entry must either replace with a string or reject, exactly one"
            }
            Self::ExceptionsOutsideRepository => {
                "only a repository's own declaration may declare exceptions"
            }
            Self::ExceptionNamesNoTerm => "an exception names no term the repository carries",
            Self::AmbiguousFiles => "one directory holds more than one declaration file",
        }
    }
}

/// A declaration member, named by its fixed model name.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Member {
    /// `schema_version`.
    SchemaVersion,
    /// `terms`.
    Terms,
    /// `exceptions`.
    Exceptions,
    /// `replacements`.
    Replacements,
    /// `phrases`.
    Phrases,
}

impl Member {
    /// The member's model name.
    pub fn name(self) -> &'static str {
        match self {
            Self::SchemaVersion => "schema_version",
            Self::Terms => "terms",
            Self::Exceptions => "exceptions",
            Self::Replacements => "replacements",
            Self::Phrases => "phrases",
        }
    }
}

/// A declaration that is refused, or could not be read.
///
/// `Display` and `Debug` render the kind, the member and the entry number only: never
/// the [`path`](Self::path), which can itself carry a private term, and never any
/// declaration content.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeclarationError {
    /// What is wrong.
    pub kind: DeclarationErrorKind,
    /// The member the problem is in, where it is in one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<Member>,
    /// The 1-based entry of that member, where one entry is at fault.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<u32>,
    /// The file (or, for [`AmbiguousFiles`](DeclarationErrorKind::AmbiguousFiles), the
    /// directory) the declaration was read from, where it was read from one. Print it
    /// only as a screened locator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
}

impl DeclarationError {
    /// An error of `kind`, about no particular member, read from no file.
    pub fn new(kind: DeclarationErrorKind) -> Self {
        DeclarationError {
            kind,
            member: None,
            entry: None,
            path: None,
        }
    }

    pub(crate) fn in_member(kind: DeclarationErrorKind, member: Member) -> Self {
        DeclarationError {
            member: Some(member),
            ..Self::new(kind)
        }
    }

    pub(crate) fn at(kind: DeclarationErrorKind, member: Member, index: usize) -> Self {
        DeclarationError {
            member: Some(member),
            entry: Some(u32::try_from(index + 1).unwrap_or(u32::MAX)),
            ..Self::new(kind)
        }
    }

    pub(crate) fn with_path(mut self, path: PathBuf) -> Self {
        self.path = Some(path);
        self
    }
}

impl fmt::Display for DeclarationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a private-terms declaration is refused: ")?;
        match (self.member, self.entry) {
            (Some(member), Some(entry)) => write!(f, "entry {entry} of `{}`: ", member.name())?,
            (Some(member), None) => write!(f, "`{}`: ", member.name())?,
            _ => {}
        }
        write!(f, "{} ({})", self.kind.describe(), self.kind.name())
    }
}

impl fmt::Debug for DeclarationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeclarationError")
            .field("kind", &self.kind)
            .field("member", &self.member)
            .field("entry", &self.entry)
            .field("path", &self.path.as_ref().map(|_| "<withheld>"))
            .finish()
    }
}

impl std::error::Error for DeclarationError {}

/// Why a [`Matcher`](crate::Matcher) could not be built.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Unbuildable {
    /// What refused: a phrase empty once normalized, or the automaton's size limit. It
    /// never quotes a term or a phrase.
    pub reason: String,
}

impl fmt::Display for Unbuildable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the matcher cannot be built: {}", self.reason)
    }
}

impl std::error::Error for Unbuildable {}

/// A resolved replacement string that itself carries a non-exempt hit, found before
/// anything was replaced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReplacementCarriesTerm {
    /// The index, in [`Matcher::rules`](crate::Matcher::rules), of the rule whose
    /// replacement carries the hit.
    pub rule: usize,
}

impl fmt::Display for ReplacementCarriesTerm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the replacement resolved for rule {} itself carries a private term, so nothing is replaced",
            self.rule
        )
    }
}

impl std::error::Error for ReplacementCarriesTerm {}
