//! The crate's storage binding of a declaration to bytes: TOML and YAML, one encoding of
//! the model in either form. Not part of the specification, which defines no format.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{DeclarationError, DeclarationErrorKind as Kind};
use crate::model::{self, Declaration};
use crate::yaml;
use crate::LevelKind;

/// The two encodings this crate binds a declaration to.
///
/// **A binding of this crate, not of the specification.** Both encode the model member
/// for member: `schema_version`, `terms`, `exceptions` (each `{term, action}`),
/// `replacements` (each `{term, with = "…"}` or `{term, reject = true}`) and `phrases`.
/// In TOML, `exceptions` and `replacements` are arrays of tables (`[[replacements]]`); in
/// YAML they are lists of maps. The two spellings below parse to the same
/// [`Declaration`]:
///
/// ```
/// use privateterms::{parse, Format, LevelKind};
///
/// let toml = br#"
/// schema_version = 2
/// terms = ["quillmoss"]
/// phrases = ["quillmoss-bridge"]
///
/// [[replacements]]
/// term = "quillmoss"
/// with = "example-project"
///
/// [[replacements]]
/// term = "vantorel"
/// reject = true
/// "#;
/// let yaml = br#"
/// schema_version: 2
/// terms: [quillmoss]
/// phrases: [quillmoss-bridge]
/// replacements:
///   - term: quillmoss
///     with: example-project
///   - term: vantorel
///     reject: true
/// "#;
/// let from_toml = parse(toml, Format::Toml, LevelKind::Directory).unwrap();
/// let from_yaml = parse(yaml, Format::Yaml, LevelKind::Directory).unwrap();
/// assert_eq!(from_toml, from_yaml);
/// ```
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Format {
    /// `private-terms.toml`.
    Toml,
    /// `private-terms.yaml` or `private-terms.yml`.
    Yaml,
}

/// Read one declaration for a level of `kind` from `bytes` in `format`.
///
/// **A binding of this crate, not of the specification**: the specification defines the
/// model and what is invalid in it; how bytes become a model value is this crate's. A
/// document is refused whole — there is never a partial [`Declaration`] — when it is
/// larger than 1 MiB, is not UTF-8, does not parse in its format (a YAML document is read
/// through one hardened reader that refuses a duplicate key, a tag, a document nested past
/// its depth bound and one whose aliases expand past their bound, and that reads `yes`,
/// `no`, `on` and `off` as strings), or is an invalid declaration (Specification 1, *The
/// declaration*), and `exceptions` is refused unless `kind` is
/// [`LevelKind::Repository`]. No error carries the document's content.
///
/// ```
/// use privateterms::{parse, DeclarationErrorKind, Format, LevelKind};
///
/// let refused = parse(b"schema_version = 1\nphrases = [\"x\"]\n", Format::Toml, LevelKind::Directory);
/// assert_eq!(refused.unwrap_err().kind, DeclarationErrorKind::MemberRequiresVersion2);
/// ```
pub fn parse(
    bytes: &[u8],
    format: Format,
    kind: LevelKind,
) -> Result<Declaration, DeclarationError> {
    if bytes.len() > yaml::options::MAX_BYTES {
        return Err(DeclarationError::new(Kind::TooLarge));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| DeclarationError::new(Kind::Malformed))?;
    // What either reader says about a document it refused can quote the document, so it
    // is dropped: the error names the problem only.
    let value: Value = match format {
        Format::Toml => {
            let table: toml::Table =
                toml::from_str(text).map_err(|_| DeclarationError::new(Kind::Malformed))?;
            serde_json::to_value(table).map_err(|_| DeclarationError::new(Kind::Malformed))?
        }
        Format::Yaml => yaml::from_str(text).map_err(|_| DeclarationError::new(Kind::Malformed))?,
    };
    let declaration = model::declaration(&value)?;
    model::for_level(&declaration, kind)?;
    Ok(declaration)
}

/// `declaration` written as a `private-terms.toml` document, which [`parse`] reads back to
/// the same value. **A binding of this crate, not of the specification.**
///
/// ```
/// use privateterms::{parse, to_toml, Declaration, Format, LevelKind};
///
/// let declaration = Declaration {
///     schema_version: 1,
///     terms: vec!["quillmoss".into()],
///     exceptions: vec![],
///     replacements: vec![],
///     phrases: vec![],
/// };
/// let written = to_toml(&declaration);
/// assert_eq!(written, "schema_version = 1\nterms = [\"quillmoss\"]\n");
/// assert_eq!(parse(written.as_bytes(), Format::Toml, LevelKind::Repository).unwrap(), declaration);
/// ```
pub fn to_toml(declaration: &Declaration) -> String {
    // Every member is a string, a list of strings, an integer or a list of string-valued
    // tables, all of which TOML represents, so this cannot fail.
    toml::to_string(declaration).unwrap_or_default()
}

/// `declaration` written as a `private-terms.yaml` document, which [`parse`] reads back to
/// the same value. **A binding of this crate, not of the specification.**
///
/// ```
/// use privateterms::{parse, to_yaml, Declaration, Format, LevelKind, Replacement};
///
/// let declaration = Declaration {
///     schema_version: 2,
///     terms: vec!["yes".into()],
///     exceptions: vec![],
///     replacements: vec![Replacement::Reject { term: "vantorel".into() }],
///     phrases: vec![],
/// };
/// let written = to_yaml(&declaration);
/// assert_eq!(parse(written.as_bytes(), Format::Yaml, LevelKind::Directory).unwrap(), declaration);
/// ```
pub fn to_yaml(declaration: &Declaration) -> String {
    yaml::to_string(declaration).unwrap_or_default()
}
