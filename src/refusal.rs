//! What a loader refusal is *about*, beside the sentence it prints.
//!
//! `onepipeline start` prints a refusal and exits; the sentence is the whole of
//! what a person needs. `onepipeline plan check` answers a *program* — a
//! consumer's own registered check reads the same list this crate refuses from —
//! so each refusal also names the node it is about and the field it is about,
//! either of which may be absent.
//!
//! The sentence is carried whole rather than recomposed, so the two readings
//! cannot drift: what `plan check` reports as a `reason` is the identical string
//! `start` prints, and [`From<Refusal> for Error`](Error) is the only way one
//! becomes the other.

use crate::error::Error;

/// One refusal the plan loader made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal {
    /// The node it is about, where it is about one. A task carrying no node id
    /// is named by the store id of the task instead — that is the only name it
    /// has, and both ends of the mistake are the author's to fix.
    pub node: Option<String>,
    /// The plan field it is about, where one field is what has to change.
    pub field: Option<String>,
    /// The sentence, exactly as `onepipeline start` prints it.
    pub message: String,
}

impl Refusal {
    /// A refusal about the plan as a whole.
    pub(crate) fn plain(message: impl Into<String>) -> Self {
        Self {
            node: None,
            field: None,
            message: message.into(),
        }
    }

    /// A refusal about one node, whose sentence names the node itself.
    pub(crate) fn about(node: &str, message: impl Into<String>) -> Self {
        Self {
            node: Some(node.to_owned()),
            field: None,
            message: message.into(),
        }
    }

    /// A refusal about one node, rendered `node '<id>': <what>` — the sentence
    /// this crate has always printed for one.
    pub(crate) fn node(id: &str, what: impl AsRef<str>) -> Self {
        Self::about(id, format!("node '{id}': {}", what.as_ref()))
    }

    /// Name the field that has to change.
    #[must_use]
    pub(crate) fn field(mut self, field: &str) -> Self {
        self.field = Some(field.to_owned());
        self
    }
}

impl From<Refusal> for Error {
    fn from(refusal: Refusal) -> Self {
        Self::Invalid(refusal.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_node_refusal_prints_the_sentence_the_engine_has_always_printed() {
        let refusal =
            Refusal::node("build", "a direct agent node needs a persona").field("persona");
        assert_eq!(
            refusal.message,
            "node 'build': a direct agent node needs a persona"
        );
        assert_eq!(refusal.node.as_deref(), Some("build"));
        assert_eq!(refusal.field.as_deref(), Some("persona"));
        assert_eq!(
            Error::from(refusal).to_string(),
            "invalid: node 'build': a direct agent node needs a persona"
        );
    }

    #[test]
    fn a_plan_refusal_is_about_no_node_and_no_field() {
        let refusal = Refusal::plain("concurrency must be at least 1");
        assert_eq!(refusal.node, None);
        assert_eq!(refusal.field, None);
        assert_eq!(refusal.message, "concurrency must be at least 1");
    }
}
