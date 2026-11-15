//! How this crate reads and writes YAML: through serde-saphyr only, every read under the
//! one set of reader options [`options::options`] states.

use serde::de::DeserializeOwned;
use serde::Serialize;

pub(crate) mod options;

/// One YAML document read as `T`, under the hardened options. The reader's own error is
/// dropped by the caller: it can describe the document.
pub(crate) fn from_str<T: DeserializeOwned>(raw: &str) -> Result<T, serde_saphyr::Error> {
    serde_saphyr::from_str_with_options(raw, options::options())
}

/// `value` written as one YAML document.
pub(crate) fn to_string<T: Serialize>(value: &T) -> Result<String, serde_saphyr::ser::Error> {
    serde_saphyr::to_string(value)
}
