//! What goes wrong while loading a configuration, and what to do about it.

use std::path::{Path, PathBuf};

/// A configuration this product will not run on.
///
/// Every variant names the thing a user has to go and change and says what to change
/// it to. That is the whole point: an unknown field, a bad value, an unknown plugin
/// name and an unusable source name are all *usage* errors here, never settings that
/// get quietly dropped.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// A configuration document exists but could not be read.
    #[error(
        "could not read {}: {message}\n\
         next: make that file readable, or remove it so the layer beneath it is used.",
        path.display()
    )]
    Read {
        /// The document that could not be read.
        path: PathBuf,
        /// What the filesystem said.
        message: String,
    },

    /// A configuration document is not valid YAML.
    #[error(
        "{}: not valid YAML: {message}\n\
         next: correct the syntax at the position named above, then re-run.",
        path.display()
    )]
    Syntax {
        /// The document that would not parse.
        path: PathBuf,
        /// What the parser said, position included.
        message: String,
    },

    /// One setting is unknown, or its value is not usable.
    ///
    /// `key` is the dotted path a user can search their configuration for and is also
    /// the path the environment-variable and `--set` spellings are derived from, so
    /// one name locates the problem at whichever layer set it.
    #[error("{key}: {message}\nnext: {next}")]
    Setting {
        /// The dotted path of the offending setting.
        key: String,
        /// What is wrong with it.
        message: String,
        /// The concrete next action.
        next: String,
    },
}

impl ConfigError {
    /// A [`ConfigError::Setting`] over `key`.
    pub(crate) fn setting(
        key: impl Into<String>,
        message: impl Into<String>,
        next: impl Into<String>,
    ) -> Self {
        Self::Setting {
            key: key.into(),
            message: message.into(),
            next: next.into(),
        }
    }

    /// A [`ConfigError::Read`] over `path`.
    pub(crate) fn read(path: &Path, error: &std::io::Error) -> Self {
        Self::Read {
            path: path.to_path_buf(),
            message: error.to_string(),
        }
    }

    /// The dotted key this error names, when it names one.
    #[must_use]
    pub fn key(&self) -> Option<&str> {
        match self {
            Self::Setting { key, .. } => Some(key),
            Self::Read { .. } | Self::Syntax { .. } => None,
        }
    }
}
