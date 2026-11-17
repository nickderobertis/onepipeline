//! What a `--comments` copy does to one task's comments at the destination, by one symmetric
//! rule that holds whichever way the copy runs.
//!
//! The copy makes the destination's **copies** of the source's comments match the source: it
//! adds a copy of each source comment the destination holds none of, edits a copy whose origin
//! comment changed, and deletes a copy whose origin comment the source no longer holds. Three
//! things are never done, and they are what make the rule safe in both directions:
//!
//! - A comment **native** to the destination — one carrying no origin — is never edited or
//!   deleted. A person wrote it there.
//! - A source comment that is itself a copy of a comment **on the destination task** is never
//!   sent back. So copying a local item back to the board it came from carries only the
//!   comments written locally, and never echoes the board's own.
//! - A copy whose origin names a task this source item does not vouch for — neither itself,
//!   nor the item it records as its own origin, nor a task any of its comments was copied
//!   from — is left alone: it came to the destination by some other copy.
//!
//! A comment's identity across copies is its [`CommentOrigin`]'s task and id, and a chained
//! copy keeps the *first* origin: a source comment that is a copy already carries its origin
//! on, so its copy two hops away still names the comment where it was written.

use std::collections::BTreeMap;

use onetaskgraph_plugin_api::{Comment, CommentBody, CommentCopy, CommentOrigin, NativeId};
use serde_json::{Value, json};

use crate::GlobalId;

use super::CommentCounts;

/// One write the plan makes.
#[derive(Clone)]
pub(super) enum Step {
    /// Create a copy of a source comment.
    Add(CommentCopy),
    /// Make the existing copy `id` say what its origin says now.
    Edit {
        /// The copy at the destination.
        id: NativeId,
        /// What it says, and its origin, as the copy leaves them.
        copy: CommentCopy,
        /// The copy as the destination held it, for undoing the edit.
        before: Comment,
    },
    /// Delete the copy `before`, whose origin comment the source no longer holds.
    Delete(Comment),
}

/// Everything one task's comment sync will do, decided before any of it is written.
#[derive(Clone, Default)]
pub(super) struct CommentPlan {
    /// The writes, in the order they are made: deletions, then edits and additions in the
    /// source's own order.
    pub(super) steps: Vec<Step>,
    /// What those writes come to, for the report.
    pub(super) counts: CommentCounts,
    /// The destination's comments as they will read once every step has landed, each as the
    /// entry [`digest_entries`] gives it.
    pub(super) after: Vec<Value>,
}

/// The task a carried comment's origin names, exactly as `CommentOrigin::task` spells it —
/// opaque and unvalidated, so a malformed spelling is matched verbatim like any other rather
/// than parsed, and never confused with the comment's own id beside it in [`OriginKey`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct OriginTask(String);

/// The identity a carried comment is matched on: the task its origin names, and that
/// comment's id there.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct OriginKey {
    /// The task the comment was first written on.
    task: OriginTask,
    /// The comment's id in that task's source.
    comment: NativeId,
}

/// The origin one source comment's copy carries: the origin it was itself copied from, or —
/// for a comment written at the source — the comment there.
pub(super) fn origin_of(source: &GlobalId, comment: &Comment) -> CommentOrigin {
    comment.origin.clone().unwrap_or_else(|| CommentOrigin {
        task: source.to_string(),
        id: comment.id.clone(),
        author: comment.author.clone(),
        created_at: comment.created_at,
        updated_at: comment.updated_at,
        url: comment.url.clone(),
    })
}

/// The identity a copy is matched on: its origin's task and id.
pub(super) fn key(origin: &CommentOrigin) -> OriginKey {
    OriginKey {
        task: OriginTask(origin.task.clone()),
        comment: origin.id.clone(),
    }
}

/// Decide what makes `held` — the destination task's comments, `destination` its qualified id
/// when it exists yet — carry copies of `comments`, the source item `source`'s, in step.
///
/// `recorded` is the origin the source item itself records, when it records one: a copy of a
/// copy vouches for the comments it was copied with.
pub(super) fn plan(
    source: &GlobalId,
    recorded: Option<&GlobalId>,
    comments: &[Comment],
    destination: Option<&GlobalId>,
    held: &[Comment],
) -> CommentPlan {
    let destination = destination.map(ToString::to_string);
    let mut wanted: Vec<CommentCopy> = Vec::new();
    for comment in comments {
        let origin = origin_of(source, comment);
        // Never sent back to the task it came from.
        if destination.as_deref() == Some(origin.task.as_str()) {
            continue;
        }
        // An empty comment — an attachment on Linear, a body a person cleared — is one no
        // source this product drives will take, so it has no copy to keep in step.
        let Ok(body) = CommentBody::new(comment.body.clone()) else {
            continue;
        };
        if wanted.iter().any(|held| key(&held.origin) == key(&origin)) {
            continue;
        }
        wanted.push(CommentCopy { body, origin });
    }
    let mut vouched: Vec<String> = vec![source.to_string()];
    vouched.extend(recorded.map(ToString::to_string));
    vouched.extend(wanted.iter().map(|copy| copy.origin.task.clone()));

    let mut copies: BTreeMap<OriginKey, &Comment> = BTreeMap::new();
    for comment in held {
        if let Some(origin) = &comment.origin {
            copies.entry(key(origin)).or_insert(comment);
        }
    }
    let mut plan = CommentPlan::default();
    let wanted_keys: Vec<OriginKey> = wanted.iter().map(|copy| key(&copy.origin)).collect();
    // What the destination reads as afterwards: everything it holds that this plan keeps, in
    // the order it holds it, then the copies this plan adds.
    for comment in held {
        match &comment.origin {
            Some(origin)
                if vouched.contains(&origin.task) && !wanted_keys.contains(&key(origin)) =>
            {
                plan.steps.push(Step::Delete(comment.clone()));
                plan.counts.deleted += 1;
            }
            Some(origin) => {
                let entry = match wanted.iter().find(|copy| key(&copy.origin) == key(origin)) {
                    Some(copy) => copy_entry(&copy.origin, copy.body.as_str()),
                    None => copy_entry(origin, &comment.body),
                };
                plan.after.push(entry);
            }
            None => plan.after.push(native_entry(comment)),
        }
    }
    for copy in wanted {
        match copies.get(&key(&copy.origin)) {
            Some(existing)
                if existing.body == copy.body.as_str()
                    && existing.origin.as_ref() == Some(&copy.origin) =>
            {
                plan.counts.unchanged += 1;
            }
            Some(existing) => {
                plan.counts.edited += 1;
                plan.steps.push(Step::Edit {
                    id: existing.id.clone(),
                    copy,
                    before: (*existing).clone(),
                });
            }
            None => {
                plan.counts.added += 1;
                plan.after
                    .push(copy_entry(&copy.origin, copy.body.as_str()));
                plan.steps.push(Step::Add(copy));
            }
        }
    }
    plan
}

/// `after` — a destination's comments as [`digest_entries`] gives them — with the copy of
/// `origin` saying `body`, as a copy finished its section links after it was planned.
pub(super) fn reword(after: &mut [Value], origin: &CommentOrigin, body: &str) {
    let entry = copy_entry(origin, body);
    for held in after.iter_mut() {
        if held.get("origin") == entry.get("origin") {
            held.clone_from(&entry);
        }
    }
}

/// One copy as the destination digest counts it: its origin's identity and what it says.
fn copy_entry(origin: &CommentOrigin, body: &str) -> Value {
    json!({"origin": [origin.task, origin.id.0], "body": body})
}

/// One comment native to the destination as the digest counts it: its id and what it says.
fn native_entry(comment: &Comment) -> Value {
    json!({"id": comment.id.0, "body": comment.body})
}

/// The destination's comments as the digest counts them, in no order the digest depends on:
/// each copy by its origin's identity and body, each native comment by its id and body.
///
/// The times and authors are left out on purpose. A copy's own are the destination's, which
/// a hosted service sets itself, and its origin's are carried exactly where the origin's
/// identity is — so leaving them out costs nothing a copy needs and spares the digest every
/// difference in how precisely a source spells an instant.
pub(super) fn digest_entries(held: &[Comment]) -> Vec<Value> {
    held.iter()
        .map(|comment| match &comment.origin {
            Some(origin) => copy_entry(origin, &comment.body),
            None => native_entry(comment),
        })
        .collect()
}

/// `entries` in the one order the digest reads them in.
pub(super) fn ordered(mut entries: Vec<Value>) -> Vec<Value> {
    entries.sort_by_key(ToString::to_string);
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use onetaskgraph_plugin_api::SourceName;

    fn id(source: &str, native: &str) -> GlobalId {
        GlobalId::new(
            SourceName::new(source).expect("a source name"),
            NativeId(native.to_owned()),
        )
    }

    fn comment(id: &str, body: &str, origin: Option<CommentOrigin>) -> Comment {
        Comment {
            id: NativeId(id.to_owned()),
            author: Some("someone".to_owned()),
            created_at: None,
            updated_at: None,
            body: body.to_owned(),
            url: None,
            origin,
        }
    }

    fn from(task: &str, comment: &str) -> CommentOrigin {
        CommentOrigin {
            task: task.to_owned(),
            id: NativeId(comment.to_owned()),
            author: Some("someone".to_owned()),
            created_at: None,
            updated_at: None,
            url: None,
        }
    }

    #[test]
    fn adds_edits_deletes_and_leaves_native_comments_alone() {
        let source = id("board", "I-1");
        let destination = id("notes", "t.md");
        let comments = [comment("c1", "first", None), comment("c2", "edited", None)];
        let held = [
            comment("n1", "a person wrote this here", None),
            comment("d2", "before", Some(from("board:I-1", "c2"))),
            comment("d3", "gone at the source", Some(from("board:I-1", "c3"))),
        ];
        let plan = plan(&source, None, &comments, Some(&destination), &held);
        assert_eq!(
            plan.counts,
            CommentCounts {
                added: 1,
                edited: 1,
                deleted: 1,
                unchanged: 0
            }
        );
        assert!(!plan.steps.iter().any(|step| matches!(
            step,
            Step::Delete(gone) | Step::Edit { before: gone, .. } if gone.id.0 == "n1"
        )));
    }

    #[test]
    fn never_sends_back_a_copy_of_the_destinations_own_comment() {
        let source = id("notes", "t.md");
        let destination = id("board", "I-1");
        let comments = [
            comment("c1", "the board wrote this", Some(from("board:I-1", "b1"))),
            comment("c2", "written locally", None),
        ];
        let held = [comment("b1", "the board wrote this", None)];
        let plan = plan(
            &source,
            Some(&destination),
            &comments,
            Some(&destination),
            &held,
        );
        assert_eq!(plan.counts.added, 1);
        assert_eq!(plan.counts.deleted + plan.counts.edited, 0);
        assert!(
            matches!(&plan.steps[..], [Step::Add(copy)] if copy.body.as_str() == "written locally")
        );
    }

    #[test]
    fn a_chained_copy_keeps_the_first_origin_and_leaves_foreign_copies() {
        let source = id("notes", "t.md");
        let destination = id("second", "I-9");
        let comments = [comment(
            "c1",
            "from the board",
            Some(from("board:I-1", "b1")),
        )];
        let held = [comment(
            "x",
            "another copy",
            Some(from("elsewhere:E-1", "e1")),
        )];
        let plan = plan(
            &source,
            Some(&id("board", "I-1")),
            &comments,
            Some(&destination),
            &held,
        );
        assert!(matches!(&plan.steps[..], [Step::Add(copy)] if copy.origin.task == "board:I-1"));
        assert_eq!(plan.counts.deleted, 0);
    }
}
