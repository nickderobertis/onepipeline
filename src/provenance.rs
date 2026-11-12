//! Whose words a ruling is: the manager's own, or a relay of the user's.
//!
//! A run takes rulings from one party, its manager, over the planner channel and
//! the human-item store. Some of those rulings are the manager's own judgement and
//! some are the user's words the manager is passing on, and the two do not carry
//! the same authority: **a user's ruling binds**, while a manager's is a strong
//! default a dispatch may decline with its reason through
//! `onepipeline decline-ruling`. [`Provenance`] is that distinction, recorded on
//! every reply envelope, human-item action and journal record of a manager input,
//! and rendered beside every ruling a dispatch reads.
//!
//! It is not the envelope's `author`. `author` names which actor sent an envelope
//! and decides the ops it is granted; `provenance` says whose words they are. An
//! envelope written before the field existed carries none and reads as
//! [`Provenance::Manager`], which is what every such envelope was.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Whose words a ruling is. Serialized as the lowercase word; absent, `manager`.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
    clap::ValueEnum,
)]
#[serde(rename_all = "lowercase")]
pub enum Provenance {
    /// The manager's own ruling: a strong default a dispatch may decline with its
    /// reason. What every record written before this field existed reads as.
    #[default]
    Manager,
    /// The user's words, relayed by the manager: binding.
    User,
}

impl Provenance {
    /// The wire word: `manager` or `user`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Manager => "manager",
            Self::User => "user",
        }
    }

    /// Whether this is the default, [`Provenance::Manager`]: what a record that
    /// omits the field reads as, and so what a writer may leave out.
    #[must_use]
    pub fn is_manager(&self) -> bool {
        *self == Self::Manager
    }
}

impl std::fmt::Display for Provenance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::Provenance;

    #[test]
    fn the_wire_words_are_lowercase_and_absence_is_the_managers() {
        assert_eq!(serde_json::to_value(Provenance::User).unwrap(), "user");
        assert_eq!(
            serde_json::to_value(Provenance::Manager).unwrap(),
            "manager"
        );
        assert_eq!(Provenance::default(), Provenance::Manager);
        assert!(serde_json::from_value::<Provenance>(serde_json::json!("planner")).is_err());
        assert_eq!(Provenance::User.to_string(), "user");
    }
}
