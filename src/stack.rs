//! Stacked drafts: placing a landing node on the unlanded work it depends on.
//!
//! A lifecycle node N on identity I has **stack parents**: its direct dependencies
//! on I that settled `done` with their change still unlanded and a branch on the
//! record — a parent settled `change-review-draft`, `change-draft` or
//! `change-open`. A dependency that is an ancestor of another stack parent
//! collapses into that descendant, whose branch already carries it, and a landed
//! dependency is no stack parent at all: its work is on the root.
//!
//! - **None** places N where it always was: the root, or its own `base_branch`.
//! - **One** places N on that parent's branch. Its session is cut from it, and its
//!   change request is opened against it through `onevcs`'s stacked publication.
//! - **Several** places N on a **stack base** — a branch `onevcs` assembles from
//!   the root with each parent merged into it, in N's declared dependency order —
//!   named `<host prefix>stack-base/<plan>/<node>`.
//!
//! An explicit `base_branch` stays the **root**: what a stack base is cut from.
//! `publish: preserve` chains are placed by [`crate::graph::stacking_bases`], and
//! nothing here reads them.
//!
//! Placement is decided from the run's own record, the pass the node starts: the
//! landings it has recorded, and nothing it would have to ask a host for.

use std::collections::{BTreeMap, BTreeSet};

use crate::graph::{Graph, Landing, NodeStatus};
use crate::payload::{SettledStack, StackParent};
use crate::plan::Node;
use crate::projection::RunState;

/// The line that opens the Stack block in a change request's description.
pub(crate) const BLOCK_OPEN: &str = "<!-- onepipeline:stack -->";

/// The line that closes it.
pub(crate) const BLOCK_CLOSE: &str = "<!-- /onepipeline:stack -->";

/// The path segment that opens the stack-base namespace, which is `onevcs`'s
/// own: a branch is a stack base by its name, `<prefix>stack-base/<plan>/<node>`.
const STACK_BASE_NAMESPACE: &str = "stack-base";

/// One stack parent, as the record held it when the node was placed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Parent {
    // llmlint: ignore-block[invalid_states_unrepresentable] the parent's own settlement,
    // as `RunState` folded it: a node id, a branch, a commit and a URL are the strings
    // every neighbouring map of that state carries them as, each checked where it was
    // folded by `vcs::usable`, and the wire record this becomes is the stacked drafts
    // contract's own `{node, branch, head}` of strings.
    /// The parent node.
    pub(crate) node: String,
    /// The branch its settlement recorded.
    pub(crate) branch: String,
    /// The commit its settlement recorded that branch at.
    pub(crate) head: String,
    /// Its change request, where its settlement named one.
    pub(crate) change_url: Option<String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

/// Where a node with stack parents is placed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Stack {
    /// On its one stack parent's branch.
    OnParent(Parent),
    /// On a stack base `onevcs` assembles from `root` and each parent, in order.
    OnStackBase {
        /// The root the base is cut from: the node's own `base_branch`, or the base
        /// the lowest change of the stack was drafted against.
        root: String,
        /// The stack base's branch, `<host prefix>stack-base/<plan>/<node>`.
        branch: String,
        /// The stack parents, two or more, in the node's declared dependency order.
        parents: Vec<Parent>,
    },
}

impl Stack {
    /// The branch the node's session is cut from and its change request is based
    /// on: the one parent's branch, or the stack base.
    pub(crate) fn base(&self) -> &str {
        match self {
            Self::OnParent(parent) => &parent.branch,
            Self::OnStackBase { branch, .. } => branch,
        }
    }

    /// The stack parents, in the node's declared dependency order.
    pub(crate) fn parents(&self) -> &[Parent] {
        match self {
            Self::OnParent(parent) => std::slice::from_ref(parent),
            Self::OnStackBase { parents, .. } => parents,
        }
    }

    /// The record its settlement carries, with each parent's head as `heads`
    /// names it by branch — the stack base's assembly reads them off the origin —
    /// and as the parent's own settlement recorded it otherwise.
    pub(crate) fn settled(&self, heads: &BTreeMap<String, String>) -> SettledStack {
        SettledStack {
            parents: self
                .parents()
                .iter()
                .map(|parent| StackParent {
                    node: parent.node.clone(),
                    branch: parent.branch.clone(),
                    head: heads.get(&parent.branch).unwrap_or(&parent.head).clone(),
                })
                .collect(),
            stack_base: match self {
                Self::OnParent(_) => None,
                Self::OnStackBase { branch, .. } => Some(branch.clone()),
            },
        }
    }

    /// What the base is, in the words both the block and the task use.
    fn drafted_against(&self) -> String {
        match self {
            Self::OnParent(parent) => format!(
                "Drafted against `{}`, the branch of `{}`: {}",
                parent.branch,
                parent.node,
                url_or_none(parent.change_url.as_deref())
            ),
            Self::OnStackBase {
                root,
                branch,
                parents,
            } => {
                let mut said = format!(
                    "Drafted against the stack base `{branch}`, which merges `{root}` and each \
                     change below:"
                );
                for parent in parents {
                    said.push_str(&format!(
                        "\n- `{}` (`{}`): {}",
                        parent.node,
                        parent.branch,
                        url_or_none(parent.change_url.as_deref())
                    ));
                }
                said
            }
        }
    }

    /// The Stack block the engine writes into the node's change request
    /// description, markers included.
    pub(crate) fn block(&self) -> String {
        format!(
            "{BLOCK_OPEN}\n## Stack\n\n{}\n{BLOCK_CLOSE}",
            self.drafted_against()
        )
    }

    /// The section a dispatched worker's task carries: each stack parent's node,
    /// branch and change request, and the base the node is drafted against.
    pub(crate) fn task_section(&self) -> String {
        let mut section = format!(
            "{TASK_HEADING}\n\nThis node's work is drafted on top of changes still in review. \
             Its branch is cut from `{base}`, and its change request is opened against \
             `{base}`, so the work below it is already on the branch: build on it and do not \
             redo it.\n",
            base = self.base()
        );
        for parent in self.parents() {
            section.push_str(&format!(
                "\n- stack parent `{}`: branch `{}`, change request {}",
                parent.node,
                parent.branch,
                url_or_none(parent.change_url.as_deref())
            ));
        }
        match self {
            Self::OnParent(parent) => {
                section.push_str(&format!("\n- base: `{}`", parent.branch));
            }
            Self::OnStackBase { root, branch, .. } => section.push_str(&format!(
                "\n- base: the stack base `{branch}`, merging `{root}` and each stack parent above"
            )),
        }
        section
    }

    /// The section the change request's body drafter reads: the block, as the
    /// engine writes it after the body, for the drafter to know and leave alone.
    pub(crate) fn drafting_section(&self) -> String {
        format!(
            "{TASK_HEADING}\n\nThis change request is drafted against unlanded changes. After \
             your body is written, the engine itself appends the block below to the \
             description, and rewrites it whenever the base moves. Read it for what the \
             change sits on; write no Stack block or `## Stack` section of your own.\n\n{}",
            self.block()
        )
    }
}

/// The heading both task sections open with.
pub(crate) const TASK_HEADING: &str = "## Stack";

fn url_or_none(url: Option<&str>) -> &str {
    url.unwrap_or("(no change request on the record)")
}

/// `description` with `block` in it exactly once: appended where it holds no
/// Stack block, and in place of the text between the markers where it holds one.
/// Nothing outside the markers moves, so a person's edits elsewhere survive.
///
/// A description holding an opening marker with no closing one after it is read
/// as holding no block, and the block is appended: replacing to the end of the
/// description would delete whatever a person wrote after a stray marker.
pub(crate) fn splice(description: &str, block: &str) -> String {
    // llmlint: ignore-block[changed_behavior_has_e2e] `tests/e2e/stacked_drafts.rs` drives the
    // three descriptions a publication meets — empty, a drafted body, and one already holding
    // the block a reviewer wrote around — through the binary and the host. The one case it
    // does not is an opening marker with no closing one, which only a person typing half a
    // marker into the description produces; `a_stray_opening_marker_is_not_read_as_a_block`
    // below holds that it is appended past rather than replaced to the end.
    if let Some(open) = description.find(BLOCK_OPEN) {
        if let Some(close) = description[open..].find(BLOCK_CLOSE) {
            let end = open + close + BLOCK_CLOSE.len();
            return format!("{}{block}{}", &description[..open], &description[end..]);
        }
    } // llmlint: ignore-end[changed_behavior_has_e2e]
    let trimmed = description.trim_end();
    if trimmed.is_empty() {
        block.to_owned()
    } else {
        format!("{trimmed}\n\n{block}")
    }
}

/// The stack base a node with several stack parents is placed on.
pub(crate) fn stack_base_branch(prefix: &str, plan: &str, node: &str) -> String {
    format!("{prefix}{STACK_BASE_NAMESPACE}/{plan}/{node}")
}

/// Where `node` is placed: `None` for a node with no stack parents, which is
/// placed as it always was, and the [`Stack`] it is placed on otherwise.
///
/// `same_identity` answers whether two `repo` spellings name one repository. A
/// stack base is named under the branch prefix `onevcs` reported putting on this
/// run's branches ([`RunState::branch_prefix`]), and under none where the run has
/// cut no branch it could have reported one on.
///
/// Refused — the node is not placed and not dispatched — where the record names a
/// stack parent's branch and no commit for it, or several stack parents and no
/// root to assemble their stack base on: placing the node anyway would draft it
/// against work the run cannot name.
pub(crate) fn placement(
    state: &RunState,
    statuses: &BTreeMap<String, NodeStatus>,
    node: &Node,
    plan: &str,
    same_identity: &dyn Fn(&str, &str) -> bool,
) -> Result<Option<Stack>, String> {
    if node.publish == crate::plan::Publish::Preserve {
        return Ok(None);
    }
    let Some(repo) = node.repo.as_deref() else {
        return Ok(None);
    };
    let mut candidates: Vec<&str> = Vec::new();
    for dep in &node.deps {
        if candidates.contains(&dep.as_str()) {
            continue;
        }
        let Some(parent) = state.graph.get(dep) else {
            continue;
        };
        let Some(parent_repo) = parent.repo.as_deref() else {
            continue;
        };
        if statuses.get(dep) != Some(&NodeStatus::Done)
            || state.landings.get(dep) != Some(&Landing::Unlanded)
            || !state.branches.contains_key(dep)
        {
            continue;
        }
        if parent_repo != repo && !same_identity(repo, parent_repo) {
            continue;
        }
        candidates.push(dep);
    }
    // A candidate an ancestor of another is carried by that one's branch.
    let kept: Vec<&str> = candidates
        .iter()
        .copied()
        .filter(|candidate| {
            !candidates.iter().any(|other| {
                other != candidate && ancestors(&state.graph, other).contains(candidate)
            })
        })
        .collect();
    // llmlint: ignore-block[changed_behavior_has_e2e] both refusals need a record this build
    // does not write: every lifecycle settlement it records `done` and unlanded carries the
    // publication's `head`, and every one carries the `base` its session opened on, which is
    // what a root is read from. Reaching either means a journal an earlier build wrote or a
    // person edited, which would prove the fixture rather than the code.
    let mut parents = Vec::with_capacity(kept.len());
    for id in &kept {
        let Some(head) = state.heads.get(*id) else {
            return Err(format!(
                "node '{}' has stack parent '{id}', which is done and unlanded with no commit \
                 recorded for its branch, so nothing says what to draft it against",
                node.id
            ));
        };
        parents.push(Parent {
            node: (*id).to_owned(),
            branch: state.branches[*id].clone(),
            head: head.clone(),
            change_url: state.change_urls.get(*id).cloned(),
        });
    }
    let mut parents = parents.into_iter();
    let Some(first) = parents.next() else {
        return Ok(None);
    };
    let rest: Vec<Parent> = parents.collect();
    if rest.is_empty() {
        return Ok(Some(Stack::OnParent(first)));
    }
    let Some(root) = node
        .base_branch
        .clone()
        .or_else(|| root_of(state, &first.node, &mut BTreeSet::new()))
    else {
        return Err(format!(
            "node '{}' has stack parents {} and the run's record names no root base to \
             assemble their stack base from",
            node.id,
            kept.join(", ")
        ));
    }; // llmlint: ignore-end[changed_behavior_has_e2e]
    Ok(Some(Stack::OnStackBase {
        root,
        branch: stack_base_branch(
            state.branch_prefix.as_deref().unwrap_or_default(),
            plan,
            &node.id,
        ),
        parents: std::iter::once(first).chain(rest).collect(),
    }))
}

/// The root a settled node's work stands on: its own `base_branch`, the root of
/// the stack it was drafted on, or the base its session opened on.
fn root_of(state: &RunState, id: &str, seen: &mut BTreeSet<String>) -> Option<String> {
    if !seen.insert(id.to_owned()) {
        return None;
    }
    if let Some(base) = state
        .graph
        .get(id)
        .and_then(|node| node.base_branch.clone())
    {
        return Some(base);
    }
    match state.stacks.get(id).and_then(|stack| stack.parents.first()) {
        Some(lowest) => root_of(state, &lowest.node.clone(), seen),
        None => state.bases.get(id).cloned(),
    }
}

/// Every node `id` depends on, directly or not.
fn ancestors<'a>(graph: &'a Graph, id: &str) -> BTreeSet<&'a str> {
    let mut found = BTreeSet::new();
    let mut pending: Vec<&str> = graph
        .get(id)
        .map(|node| node.deps.iter().map(String::as_str).collect())
        .unwrap_or_default();
    while let Some(next) = pending.pop() {
        if let Some(node) = graph.get(next) {
            if found.insert(node.id.as_str()) {
                pending.extend(node.deps.iter().map(String::as_str));
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack(parents: &[(&str, &str)], stack_base: Option<&str>) -> Stack {
        let mut parents: Vec<Parent> = parents
            .iter()
            .map(|(node, branch)| Parent {
                node: (*node).into(),
                branch: (*branch).into(),
                head: "abc".into(),
                change_url: Some(format!("https://github.com/o/r/pull/{node}")),
            })
            .collect();
        match stack_base {
            None => Stack::OnParent(parents.remove(0)),
            Some(branch) => Stack::OnStackBase {
                root: "main".into(),
                branch: branch.into(),
                parents,
            },
        }
    }

    #[test]
    fn a_description_without_a_block_has_it_appended_after_its_text() {
        let block = stack(&[("a", "feat/a")], None).block();
        let spliced = splice("## What\nb\n\n", &block);
        assert_eq!(spliced, format!("## What\nb\n\n{block}"));
        assert_eq!(splice("", &block), block);
    }

    #[test]
    fn a_description_holding_a_block_has_only_the_block_replaced() {
        let old = stack(&[("a", "feat/a")], None).block();
        let new = stack(&[("a", "feat/a"), ("c", "feat/c")], Some("stack-base/p/e")).block();
        let description = format!("before\n\n{old}\n\nafter, written by a person");
        let spliced = splice(&description, &new);
        assert_eq!(
            spliced,
            format!("before\n\n{new}\n\nafter, written by a person")
        );
        assert_eq!(spliced.matches(BLOCK_OPEN).count(), 1);
        assert_eq!(splice(&spliced, &new), spliced, "splicing is idempotent");
    }

    #[test]
    fn a_stray_opening_marker_is_not_read_as_a_block() {
        let block = stack(&[("a", "feat/a")], None).block();
        let description = format!("text {BLOCK_OPEN} and more");
        assert_eq!(
            splice(&description, &block),
            format!("{description}\n\n{block}")
        );
    }

    #[test]
    fn the_block_names_the_base_and_each_parent_s_change_request() {
        let one = stack(&[("a", "feat/a")], None).block();
        assert!(
            one.starts_with(BLOCK_OPEN) && one.ends_with(BLOCK_CLOSE),
            "{one}"
        );
        assert!(one.contains("`feat/a`") && one.contains("pull/a"), "{one}");
        let several = stack(&[("a", "feat/a"), ("c", "feat/c")], Some("stack-base/p/e")).block();
        assert!(several.contains("`stack-base/p/e`"), "{several}");
        assert!(
            several.contains("pull/a") && several.contains("pull/c"),
            "{several}"
        );
    }
}
