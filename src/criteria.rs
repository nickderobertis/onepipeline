//! The mechanically-checkable half of a node's review bar.
//!
//! A criterion that names both a file and a literal value — *"`config.yaml`
//! carries `complete_dataset: true`"* — is checkable by reading that file, and
//! one shipped negated in the code it named passed a worker, a judge, a monitor
//! and a manager because nobody made the check. It is a grep, so this makes it:
//! when a node settles, each criterion of its task is read against the tree that
//! node's dispatch worked in.
//!
//! **A mismatch is a finding and never a verdict.** A mechanical check that
//! failed a node would be a new way for correct work to be failed on a demand
//! nobody wrote, which is the class of mistake this exists to close — so what a
//! mismatch produces is a non-blocking finding on the node, and the node's own
//! settled outcome is exactly what it would have been.
//!
//! The check reads only what it can be sure of: a criterion naming no file this
//! tree holds, or naming no literal beside it, produces nothing at all. Silence
//! on the criteria it cannot read is the price of never being wrong about the
//! ones it can.

use std::path::{Path, PathBuf};

use crate::plan::Node;

/// The heading a node's per-node review bar is written under.
///
/// The plan schema's own — `docs/contract.md` fixes it as the task's own
/// `## Acceptance criteria`, which is what the judge is handed — so this reads
/// the same section a reviewer reads.
pub(crate) const HEADING: &str = "## Acceptance criteria";

/// The most of one named file this reads.
///
/// A criterion names a source file, and a tree also holds build output and
/// vendored archives that a mistyped path could land on. Past this the file is
/// left unread and the criterion produces nothing, which is the same answer as
/// any other criterion this cannot read.
const MAX_FILE_BYTES: u64 = 1 << 20;

/// One criterion whose named file does not carry the literal it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Finding {
    /// The criterion, as its author wrote it.
    pub criterion: String,
    /// The file that was read, relative to the tree.
    pub file: String,
    /// The literal it does not carry.
    pub literal: String,
}

impl Finding {
    /// What the planner is told, which names all three.
    pub(crate) fn message(&self) -> String {
        format!(
            "criterion check: `{}` does not carry `{}`, which its criterion names — \"{}\". \
             This is a finding and not a verdict: the node settled on its own outcome, and \
             what to do about the mismatch is the planner's call.",
            self.file, self.literal, self.criterion
        )
    }
}

/// Read one node's criteria against the tree its dispatch worked in.
///
/// A node's own task prose and each of its steps': a node with `steps` carries
/// no task of its own, and each step states what that step is held to.
pub(crate) fn of_node(node: &Node, tree: Option<&Path>) -> Vec<Finding> {
    findings(
        node.task.as_deref().into_iter().chain(
            node.steps
                .iter()
                .flatten()
                .filter_map(|step| step.task.as_deref()),
        ),
        tree,
    )
}

/// Read task prose against the tree its dispatch worked in.
///
/// Several pieces of prose because a node with `steps` carries its bar on them:
/// each step's task states what that step is held to, and the node itself states
/// nothing. No tree is no findings — there is nothing to read them against.
pub(crate) fn findings<'a>(
    prose: impl IntoIterator<Item = &'a str>,
    tree: Option<&Path>,
) -> Vec<Finding> {
    let Some(tree) = tree else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for task in prose {
        for criterion in criteria(task) {
            for finding in check(&criterion, tree) {
                if !found.contains(&finding) {
                    found.push(finding);
                }
            }
        }
    }
    found
}

/// The bullets under the task's `## Acceptance criteria` heading.
///
/// One criterion per bullet, with its wrapped continuation lines joined onto it:
/// a criterion written across two lines names its file on one and its literal on
/// the other as often as not, and reading them apart would make the pair
/// unreadable.
fn criteria(task: &str) -> Vec<String> {
    let mut criteria: Vec<String> = Vec::new();
    let mut inside = false;
    for line in task.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("##") {
            inside = trimmed.eq_ignore_ascii_case(HEADING);
            continue;
        }
        if !inside {
            continue;
        }
        match trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            Some(bullet) => criteria.push(bullet.trim().to_owned()),
            // A continuation belongs to the bullet above it; anything before the
            // first bullet is prose introducing the section.
            None if !trimmed.is_empty() => {
                if let Some(last) = criteria.last_mut() {
                    last.push(' ');
                    last.push_str(trimmed);
                }
            }
            None => {}
        }
    }
    criteria
}

/// One criterion, read against the tree.
fn check(criterion: &str, tree: &Path) -> Vec<Finding> {
    let quoted = backticked(criterion);
    let mut files: Vec<(String, String)> = Vec::new();
    let mut literals: Vec<String> = Vec::new();
    for token in quoted {
        match read_named(tree, &token) {
            Some(content) => files.push((token, content)),
            None => literals.push(token),
        }
    }
    if files.is_empty() || literals.is_empty() {
        return Vec::new();
    }
    let mut found = Vec::new();
    for literal in literals {
        // Any one of the named files carrying it satisfies the criterion: a
        // criterion that names two files means the value is in the pair of them,
        // and reporting the one that does not hold it would be a finding about
        // a criterion that is met.
        if files.iter().any(|(_, content)| content.contains(&literal)) {
            continue;
        }
        for (file, _) in &files {
            found.push(Finding {
                criterion: criterion.to_owned(),
                file: file.clone(),
                literal: literal.clone(),
            });
        }
    }
    found
}

/// Every backticked run in one criterion, in the order it wrote them.
///
/// Empty and unterminated runs are dropped: a criterion writing a lone backtick
/// has said nothing this can read.
fn backticked(criterion: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut rest = criterion;
    while let Some(open) = rest.find('`') {
        rest = &rest[open + 1..];
        let Some(close) = rest.find('`') else {
            break;
        };
        let token = rest[..close].trim();
        if !token.is_empty() {
            tokens.push(token.to_owned());
        }
        rest = &rest[close + 1..];
    }
    tokens
}

/// The contents of the file this token names within the tree, if it names one.
///
/// A token that is absolute, that climbs out of the tree, or that names anything
/// but a plain file of a readable size names no file here — the criterion then
/// carries it as a literal, which is what an ordinary backticked value is.
///
/// **Every component is read without following a link.** A criterion is prose an
/// agent wrote and the tree is one an agent worked in, so an in-tree symlink
/// pointing outside it would be a criterion reading a file the node never
/// touched — and reporting *that* file's contents as this branch's is the one
/// wrong answer this check must not give. `report::retain` refuses a symlink for
/// the same reason: a name that delivers a different file than it states.
fn read_named(tree: &Path, token: &str) -> Option<String> {
    if token.is_empty() || Path::new(token).is_absolute() {
        return None;
    }
    let mut path = PathBuf::from(tree);
    for component in Path::new(token).components() {
        let std::path::Component::Normal(part) = component else {
            return None;
        };
        path.push(part);
        // Each directory on the way as well as the file itself: a link halfway
        // along the path leaves the rest of it outside the tree just as surely.
        if std::fs::symlink_metadata(&path).ok()?.is_symlink() {
            return None;
        }
    }
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return None;
    }
    std::fs::read_to_string(&path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One scratch tree, named after the test that asked for it.
    fn tree(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("onepipeline-criteria-{name}-{}", crate::sys::pid()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch tree");
        for (file, body) in files {
            let path = dir.join(file);
            std::fs::create_dir_all(path.parent().expect("a directory"))
                .expect("the directory is made");
            std::fs::write(path, body).expect("the file is written");
        }
        dir
    }

    /// The one file every case here reads, and the value it holds.
    const ROW: &[(&str, &str)] = &[("journeys.yaml", "complete_dataset: false\n")];

    #[test]
    fn a_named_file_that_contradicts_its_literal_is_a_finding() {
        let dir = tree("contradicts", ROW);
        let found = findings(
            [
                "## What\nship it\n\n## Acceptance criteria\n- the row in `journeys.yaml` is \
                 `complete_dataset: true`.\n",
            ],
            Some(&dir),
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].file, "journeys.yaml");
        assert_eq!(found[0].literal, "complete_dataset: true");
        assert!(found[0].criterion.contains("journeys.yaml"));
        let message = found[0].message();
        assert!(message.contains("journeys.yaml"), "{message}");
        assert!(message.contains("complete_dataset: true"), "{message}");
    }

    #[test]
    fn a_named_file_that_carries_its_literal_is_silent() {
        let dir = tree("carries", &[("journeys.yaml", "complete_dataset: true\n")]);
        assert!(findings(
            ["## Acceptance criteria\n- `journeys.yaml` says `complete_dataset: true`.\n"],
            Some(&dir),
        )
        .is_empty());
    }

    #[test]
    fn a_criterion_naming_no_file_or_no_literal_is_silent() {
        let dir = tree("silent", ROW);
        // No file: every backticked token names nothing this tree holds.
        assert!(findings(
            ["## Acceptance criteria\n- the dataset is `complete_dataset: true`.\n"],
            Some(&dir),
        )
        .is_empty());
        // No literal beside the file.
        assert!(findings(
            ["## Acceptance criteria\n- `journeys.yaml` is updated.\n"],
            Some(&dir),
        )
        .is_empty());
        // No criteria section at all.
        assert!(findings(["## What\ndo it\n"], Some(&dir)).is_empty());
        // Nothing to read it against.
        assert!(findings(
            ["## Acceptance criteria\n- `journeys.yaml` says `complete_dataset: true`.\n"],
            None,
        )
        .is_empty());
    }

    #[test]
    fn only_the_criteria_section_is_read() {
        let dir = tree("section", ROW);
        // The same sentence under another heading is prose, not a bar.
        assert!(findings(
            [
                "## What\nmake `journeys.yaml` say `complete_dataset: true`\n\n\
                 ## Acceptance criteria\n- it ships.\n"
            ],
            Some(&dir),
        )
        .is_empty());
    }

    #[test]
    fn a_wrapped_criterion_is_read_as_one() {
        let dir = tree("wrapped", ROW);
        let found = findings(
            [
                "## Acceptance criteria\n- the shared journey row in `journeys.yaml`\n  \
                 reads `complete_dataset: true`.\n",
            ],
            Some(&dir),
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].criterion.contains("reads"), "{found:?}");
    }

    /// A link inside the tree names a file the node's dispatch never touched, so
    /// the criterion carries the token as a literal instead of reading through
    /// it — reporting that file's contents as this branch's is the one wrong
    /// answer this check must not give.
    #[cfg(unix)]
    #[test]
    fn an_in_tree_symlink_pointing_outside_it_names_no_file() {
        let dir = tree("symlink", ROW);
        let elsewhere = dir.join("elsewhere.yaml");
        std::fs::write(&elsewhere, "complete_dataset: true\n").expect("the outside file");
        let inside = tree("symlink-tree", &[]);
        std::os::unix::fs::symlink(&elsewhere, inside.join("journeys.yaml"))
            .expect("the link is made");
        // The link reads `complete_dataset: true`, so a check that followed it
        // would find the criterion met and say nothing.
        let found = findings(
            ["## Acceptance criteria\n- `journeys.yaml` says `complete_dataset: true`.\n"],
            Some(&inside),
        );
        assert!(
            found.is_empty(),
            "a link was followed rather than left as a literal: {found:?}"
        );
    }

    #[test]
    fn a_token_that_climbs_out_of_the_tree_names_no_file() {
        let dir = tree("climbs", ROW);
        assert!(findings(
            ["## Acceptance criteria\n- `../journeys.yaml` says `complete_dataset: true`.\n"],
            Some(&dir),
        )
        .is_empty());
    }
}
