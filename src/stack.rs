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
        // llmlint: ignore-block[invalid_states_unrepresentable] both are the `String`s
        // `onevcs::StackBaseRequest` takes them as — the contract the stack base is
        // assembled through fixes that shape — and `onevcs` holds each to git's ref-name
        // grammar where it reads them, refusing the assembly otherwise.
        /// The root the base is cut from: the node's own `base_branch`, or the base
        /// the lowest change of the stack was drafted against.
        root: String,
        /// The stack base's branch, `<host prefix>stack-base/<plan>/<node>`.
        branch: String,
        // llmlint: ignore-end[invalid_states_unrepresentable]
        /// The stack parents, in the node's declared dependency order.
        parents: Several,
    },
}

/// Two or more stack parents: what a stack base merges, and the only way one is
/// held, so a stack base of fewer is not a value this crate can make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Several(Vec<Parent>);

impl Several {
    /// `parents`, where there are at least two of them, and them back otherwise.
    pub(crate) fn of(parents: Vec<Parent>) -> Result<Self, Vec<Parent>> {
        if parents.len() >= 2 {
            Ok(Self(parents))
        } else {
            Err(parents)
        }
    }

    /// The parents, in order.
    pub(crate) fn as_slice(&self) -> &[Parent] {
        &self.0
    }
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
            Self::OnStackBase { parents, .. } => parents.as_slice(),
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
                for parent in parents.as_slice() {
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
/// Nothing outside the markers moves, so a person's edits elsewhere survive: an
/// appended block follows every byte of the description, trailing whitespace
/// included, after only the newlines needed for a blank line before it.
///
/// A description holding an opening marker with no closing one after it is read
/// as holding no block, and the block is appended: replacing to the end of the
/// description would delete whatever a person wrote after a stray marker.
pub(crate) fn splice(description: &str, block: &str) -> String {
    // llmlint: ignore-block[changed_behavior_has_e2e] `tests/e2e/stacked_drafts.rs` drives the
    // descriptions a publication meets — empty, a drafted body, one already holding the block
    // a reviewer wrote around, and one a reviewer rewrote without it, ending in whitespace —
    // through the binary and the host. The one case it
    // does not is an opening marker with no closing one, which only a person typing half a
    // marker into the description produces; `a_stray_opening_marker_is_not_read_as_a_block`
    // below holds that it is appended past rather than replaced to the end.
    if let Some(open) = description.find(BLOCK_OPEN) {
        if let Some(close) = description[open..].find(BLOCK_CLOSE) {
            let end = open + close + BLOCK_CLOSE.len();
            return format!("{}{block}{}", &description[..open], &description[end..]);
        }
    } // llmlint: ignore-end[changed_behavior_has_e2e]
    let separator = if description.is_empty() || description.ends_with("\n\n") {
        ""
    } else if description.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    format!("{description}{separator}{block}")
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
        // A dependency absorbed into another node's draft stands on that node's branch.
        let mut dep = dep.as_str();
        let mut seen = BTreeSet::new();
        while let Some(into) = state.absorbed.get(dep) {
            if !seen.insert(dep.to_owned()) {
                break;
            }
            dep = into.as_str();
        }
        let Some(parent) = state.graph.get(dep) else {
            continue;
        };
        let dep = parent.id.as_str();
        if candidates.contains(&dep) {
            continue;
        }
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
    let parents = match Several::of(parents) {
        Ok(several) => several,
        // One, or none: a node every dependency of which was dropped places as before.
        Err(mut fewer) => return Ok(fewer.pop().map(Stack::OnParent)),
    };
    let first = &parents.as_slice()[0];
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
        parents,
    }))
}

/// A draft a restack owes: a node settled on a stack one of whose parents'
/// branches the run has since seen move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Owed {
    // llmlint: ignore-block[invalid_states_unrepresentable] the node and the branch are
    // `RunState`'s own, as `Parent`'s are above: a node id the graph holds and a branch its
    // settlement recorded, each checked where it was folded by `vcs::usable`, and handed on
    // as the `String`s the session request and the journal record take them as.
    /// The node.
    pub(crate) node: String,
    /// The branch its settlement recorded, which the restack continues.
    pub(crate) branch: String,
    // llmlint: ignore-end[invalid_states_unrepresentable]
    /// Where it is restacked: its recorded stack, each parent named by its
    /// lineage's newest node and at the head its branch stands at now.
    pub(crate) stack: Stack,
    /// Why.
    pub(crate) cause: crate::payload::RestackCause,
}

impl Owed {
    /// Each stack parent's branch and the head the restack brings the node onto.
    pub(crate) fn onto(&self) -> crate::payload::Onto {
        let onto: Vec<crate::payload::RestackOnto> = self
            .stack
            .parents()
            .iter()
            .map(|parent| crate::payload::RestackOnto {
                branch: parent.branch.clone(),
                head: parent.head.clone(),
            })
            .collect();
        // A stack holds at least one parent by construction — `Stack::OnParent` one,
        // `Several` two or more — so the list is never empty.
        onto.try_into()
            .unwrap_or_else(|why: String| unreachable!("a stack's parents: {why}"))
    }
}

/// Every draft a restack owes now, in graph order.
///
/// A lifecycle node settled `done` with its change unlanded and its stack on the
/// record is owed one once any of its stack parents' branches stands at a commit
/// other than the one its stack recorded — which the run sees off that parent's
/// accepted push, before the publication that made it has settled. So a draft
/// waits for its parents' pushes and never for their checks, and each restack
/// that pushes a merge makes the drafts stacked on *it* owed in turn, up the
/// stack. A node `busy` answers for — one with a dispatch or a restack in flight —
/// is left for the pass after it settles.
///
/// Read from the run's own record alone, so it asks no host. A recorded stack
/// base whose root the record cannot name owes nothing here: assembling it again
/// would assemble it on a guess.
pub(crate) fn restacks_owed(
    state: &RunState,
    statuses: &BTreeMap<String, NodeStatus>,
    busy: &dyn Fn(&str) -> bool,
) -> Vec<Owed> {
    let mut owed = Vec::new();
    for node in state.graph.iter() {
        let id = node.id.as_str();
        // A draft merged into another's branch, or closed, has no change request left
        // to republish onto.
        if node.repo.is_none()
            || statuses.get(id) != Some(&NodeStatus::Done)
            || state.landings.get(id) != Some(&Landing::Unlanded)
            || state.absorbed.contains_key(id)
            || state.closed_changes.contains(id)
            || busy(id)
        {
            continue;
        }
        let (Some(branch), Some(recorded)) = (state.branches.get(id), state.stacks.get(id)) else {
            continue;
        };
        let mut moved = None;
        let parents: Vec<Parent> = recorded
            .parents
            .iter()
            .map(|parent| {
                let newest = newest_of(state, &parent.node);
                let head = match state.branch_heads.get(&parent.branch) {
                    Some(now) if *now != parent.head => {
                        moved.get_or_insert(newest.clone());
                        now.clone()
                    }
                    _ => parent.head.clone(),
                };
                Parent {
                    change_url: state
                        .change_urls
                        .get(&newest)
                        .or_else(|| state.change_urls.get(&parent.node))
                        .cloned(),
                    node: newest,
                    branch: parent.branch.clone(),
                    head,
                }
            })
            .collect();
        let Some(moved) = moved else {
            continue;
        };
        // llmlint: ignore-block[changed_behavior_has_e2e] the two arms that owe nothing — a
        // stack base whose root the record cannot name, and a stack base and parents that
        // disagree — need a settlement this build never writes: every stacked settlement it
        // records names its base, from which a root is read, and a stack base exactly when it
        // has several parents. Reaching either end to end means a hand-edited journal, which
        // would prove the fixture; `a_draft_owes_a_restack_only_on_a_record_it_can_restack_on`
        // holds both over a run state, beside the owed case every restack journey drives.
        let stack =
            match (&recorded.stack_base, Several::of(parents)) {
                (None, Err(mut one)) if one.len() == 1 => Stack::OnParent(one.remove(0)),
                (Some(stack_base), Ok(parents)) => {
                    let Some(root) = node.base_branch.clone().or_else(|| {
                        root_of(state, &parents.as_slice()[0].node, &mut BTreeSet::new())
                    }) else {
                        continue;
                    };
                    Stack::OnStackBase {
                        root,
                        branch: stack_base.clone(),
                        parents,
                    }
                }
                // A record whose stack base and parents disagree is not one this build
                // writes, and restacking on it would be restacking on a guess.
                _ => continue,
            }; // llmlint: ignore-end[changed_behavior_has_e2e]
               // A parent whose own restack moved it says so, and every other move is the
               // rework that republished it.
        let cause = if state.restacking.contains_key(&moved) || state.restacks.contains_key(&moved)
        {
            crate::payload::RestackCause::Restack
        } else {
            crate::payload::RestackCause::Rework
        };
        owed.push(Owed {
            node: id.to_owned(),
            branch: branch.clone(),
            stack,
            cause,
        });
    }
    owed
}

/// The newest node of `id`'s lineage: what every supersession since it names.
fn newest_of(state: &RunState, id: &str) -> String {
    let mut at = id.to_owned();
    let mut seen = BTreeSet::new();
    while let Some(next) = state.superseded.get(&at) {
        if !seen.insert(at.clone()) {
            break;
        }
        at = next.clone();
    }
    at
}

/// The root a settled node's work stands on: its own `base_branch`, the root of
/// the stack it was drafted on, or the base its session opened on.
pub(crate) fn root_of(state: &RunState, id: &str, seen: &mut BTreeSet<String>) -> Option<String> {
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
                parents: Several::of(parents).expect("a stack base of two parents"),
            },
        }
    }

    /// A run state holding `nodes`, each lifecycle node settled `done` and unlanded
    /// on `me/<id>` at `old-<id>`.
    fn settled(nodes: Vec<Node>) -> (RunState, BTreeMap<String, NodeStatus>) {
        let mut state = RunState::default();
        let mut statuses = BTreeMap::new();
        for node in nodes {
            let id = node.id.clone();
            state.graph.insert(node);
            statuses.insert(id.clone(), NodeStatus::Done);
            state.landings.insert(id.clone(), Landing::Unlanded);
            state.branches.insert(id.clone(), format!("me/{id}"));
            state.heads.insert(id.clone(), format!("old-{id}"));
            state
                .branch_heads
                .insert(format!("me/{id}"), format!("old-{id}"));
        }
        (state, statuses)
    }

    fn node(id: &str, deps: &[&str]) -> Node {
        Node {
            id: id.into(),
            repo: Some("service".into()),
            deps: deps.iter().map(|dep| (*dep).to_owned()).collect(),
            ..Node::default()
        }
    }

    fn recorded(parents: &[&str], stack_base: Option<&str>) -> SettledStack {
        SettledStack {
            parents: parents
                .iter()
                .map(|id| StackParent {
                    node: (*id).into(),
                    branch: format!("me/{id}"),
                    head: format!("old-{id}"),
                })
                .collect(),
            stack_base: stack_base.map(str::to_owned),
        }
    }

    /// A draft owes a restack once a stack parent's branch moves, and only onto a
    /// record it can be restacked on: one parent, or a stack base over several
    /// whose root the record names. A stack base whose root nothing names, and a
    /// stack base beside a single parent, owe nothing.
    #[test]
    fn a_draft_owes_a_restack_only_on_a_record_it_can_restack_on() {
        let (mut state, statuses) = settled(vec![
            node("a", &[]),
            node("d", &[]),
            node("b", &["a"]),
            node("e", &["a", "d"]),
        ]);
        state.stacks.insert("b".into(), recorded(&["a"], None));
        state
            .stacks
            .insert("e".into(), recorded(&["a", "d"], Some("me/stack-base/p/e")));
        assert!(restacks_owed(&state, &statuses, &|_| false).is_empty());

        state.branch_heads.insert("me/a".into(), "new-a".into());
        // No root on the record for `e`'s stack base: only `b` is owed.
        let owed = restacks_owed(&state, &statuses, &|_| false);
        assert_eq!(
            owed.iter()
                .map(|owed| owed.node.as_str())
                .collect::<Vec<_>>(),
            ["b"]
        );
        assert_eq!(owed[0].onto().as_slice()[0].head, "new-a");
        assert_eq!(owed[0].cause, crate::payload::RestackCause::Rework);
        // With its root named, `e` is owed too, onto its stack base.
        state.bases.insert("a".into(), "main".into());
        let owed = restacks_owed(&state, &statuses, &|_| false);
        assert_eq!(owed.len(), 2);
        assert!(matches!(
            &owed[1].stack,
            Stack::OnStackBase { root, branch, .. } if root == "main" && branch == "me/stack-base/p/e"
        ));
        // A stack base recorded beside one parent disagrees with itself: nothing.
        state
            .stacks
            .insert("e".into(), recorded(&["a"], Some("me/stack-base/p/e")));
        assert_eq!(restacks_owed(&state, &statuses, &|_| false).len(), 1);
        // And a node something is busy with is left for later.
        assert!(restacks_owed(&state, &statuses, &|node| node == "b").is_empty());
    }

    #[test]
    fn a_description_without_a_block_has_it_appended_after_its_text() {
        let block = stack(&[("a", "feat/a")], None).block();
        let spliced = splice("## What\nb\n\n", &block);
        assert_eq!(spliced, format!("## What\nb\n\n{block}"));
        assert_eq!(splice("", &block), block);
        assert_eq!(splice("text", &block), format!("text\n\n{block}"));
        assert_eq!(splice("text\n", &block), format!("text\n\n{block}"));
        // Trailing whitespace is the description's own, and stays.
        let trailing = "text  \n\n \t\n";
        assert_eq!(splice(trailing, &block), format!("{trailing}\n{block}"));
        assert_eq!(splice(" ", &block), format!(" \n\n{block}"));
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
