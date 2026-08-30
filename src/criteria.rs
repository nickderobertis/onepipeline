//! The mechanically checkable half of a node's acceptance criteria.
//!
//! A settling lifecycle node has two things at once: a branch, checked out in
//! the worktree its session opened, and criteria that are prose. Where one of
//! those criteria names **a literal value in a named file**, the comparison is
//! the cheapest check there is — read the file on the branch and look — and
//! nothing was making it. A criterion reading "the row is `complete_dataset:
//! true` in `tests/shared.rs`" has shipped against a branch whose file read
//! `complete_dataset: false`, past a worker, a judge, a monitor and a manager,
//! because every one of them read the prose rather than the file.
//!
//! Two bounds, and both are the point.
//!
//! **What this produces is a finding, never a verdict.** A mechanical check that
//! failed a node would be a new way for correct work to be failed on a demand
//! nobody wrote. The node settles exactly as it would have; the finding names
//! the criterion, the file, the literal it expected and what the file holds
//! instead, and a manager decides.
//!
//! **It recognises only what it can be sure of, and says nothing about the
//! rest.** [`checkable`] parses a criterion into "this named file contains this
//! literal" or declines it, and a declined criterion is not a finding, not a
//! warning, and not a record: it is silence. A checker that guessed would raise
//! false findings on sound work, and a tier that cries wolf is one a reader
//! learns to skim. Missing a checkable criterion is recoverable; inventing a
//! mismatch trains the reader to ignore the ones that are real.
//!
//! A file the branch will not give up — absent, unreadable, a directory, not
//! text — is neither of those answers. It is [`Answer::Unread`], the check
//! declining to answer, kept apart from a match and a mismatch for the same
//! reason "not answered" is never "not released" elsewhere in this engine.

use std::path::Path;

use crate::plan::Node;

/// One criterion parsed into the one shape this module can answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Checkable {
    /// The criterion as the plan wrote it, so a finding quotes the bar rather
    /// than this module's reading of it.
    pub criterion: String,
    /// The file it named, relative to the node's branch.
    pub file: String,
    /// The literal it said that file holds.
    pub literal: String,
}

/// What reading the branch answered.
///
/// Three and not two: a file that cannot be read is not a file that disagrees,
/// and a reader who could not tell them apart would chase a mismatch that was
/// never measured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Answer {
    /// The file holds the literal.
    Match,
    /// The file was read and does not hold it.
    Mismatch {
        /// What it holds instead, for the finding to quote.
        holds: String,
    },
    /// The check could not read the file, so it compared nothing.
    Unread {
        /// Why, in the words of whatever refused.
        reason: String,
    },
}

impl Answer {
    /// The word the run's own record carries this answer under.
    pub(crate) const fn as_str(&self) -> &'static str {
        match self {
            Self::Match => "match",
            Self::Mismatch { .. } => "mismatch",
            Self::Unread { .. } => "unread",
        }
    }
}

/// The heading a task states its bar under.
const CRITERIA_HEADING: &str = "## Acceptance criteria";

/// Every criterion of one node this module can answer.
///
/// The node's own task, the amendment binding it — which `plan` documents as
/// part of the bar rather than as advice — and each step's task, because the
/// steps of one lifecycle node all work on the one branch this reads.
pub(crate) fn checkable_of(node: &Node) -> Vec<Checkable> {
    let steps = node
        .steps
        .iter()
        .flatten()
        .filter_map(|step| step.task.as_deref());
    let mut found: Vec<Checkable> = Vec::new();
    for task in node
        .task
        .as_deref()
        .into_iter()
        .chain(node.amendment.as_deref())
        .chain(steps)
    {
        for check in checkable(task) {
            // A node and its steps can restate one criterion, and a criterion
            // compared twice is two findings for one thing said once.
            if !found.contains(&check) {
                found.push(check);
            }
        }
    }
    found
}

/// The criteria of one task document this module can answer.
///
/// Only the `## Acceptance criteria` section: what a task says elsewhere is
/// context, and a `## What` narrating the change is not a bar anybody agreed to.
pub(crate) fn checkable(task: &str) -> Vec<Checkable> {
    criteria_in(task)
        .into_iter()
        .filter_map(|criterion| parse(&criterion))
        .collect()
}

/// Read one criterion off the branch.
///
/// `root` is the worktree the node's session opened, which is that branch
/// checked out. A path is resolved under it and never above it — [`parse`]
/// declines a criterion naming an absolute path or one climbing out — so this
/// reads the node's own work and nothing else on the machine.
pub(crate) fn answer(root: &Path, check: &Checkable) -> Answer {
    match std::fs::read_to_string(root.join(&check.file)) {
        Err(error) => Answer::Unread {
            reason: error.to_string(),
        },
        Ok(text) if text.contains(&check.literal) => Answer::Match,
        Ok(text) => Answer::Mismatch {
            holds: holds(&text, &check.literal),
        },
    }
}

/// What a file holds where it does not hold the literal.
///
/// The line naming the same key, where the literal is a `key: value` and the
/// file has one — which is the case this whole check exists for, and the one
/// where "what it holds instead" is a fact rather than a shrug. Where there is
/// no such line, that absence is the answer and is said as one.
fn holds(text: &str, literal: &str) -> String {
    let key = literal
        .split_once([':', '='])
        .map_or(literal, |(key, _)| key)
        .trim();
    let named = (!key.is_empty())
        .then(|| text.lines().find(|line| line.contains(key)))
        .flatten();
    match named {
        Some(line) => format!("`{}`", one_line(line.trim())),
        None => format!("nothing naming `{}`", one_line(key)),
    }
}

/// A quoted fragment, bounded so a finding stays one readable sentence.
fn one_line(text: &str) -> String {
    const LIMIT: usize = 200;
    let flattened = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match flattened.char_indices().nth(LIMIT) {
        None => flattened,
        Some((at, _)) => format!("{}…", &flattened[..at]),
    }
}

/// The criteria of one task, as bullets, in the order they were written.
///
/// A bullet wrapped over several lines is one criterion: a plan is prose a
/// person wrote, and a bar split by a line break is still one bar.
fn criteria_in(task: &str) -> Vec<String> {
    let mut criteria: Vec<String> = Vec::new();
    let mut inside = false;
    for line in task.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("##") {
            inside = trimmed == CRITERIA_HEADING;
            continue;
        }
        if !inside {
            continue;
        }
        match trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            Some(bullet) => criteria.push(bullet.trim().to_string()),
            // An indented continuation belongs to the bullet above it; a blank
            // line, or prose at the margin, ends the one being read.
            None if trimmed.is_empty() || line.starts_with(char::is_alphanumeric) => {}
            None => {
                if let Some(last) = criteria.last_mut() {
                    last.push(' ');
                    last.push_str(trimmed);
                }
            }
        }
    }
    criteria
}

/// One criterion, parsed into a file and a literal — or declined.
///
/// Declined is the common answer and the safe one. The shape recognised is
/// exactly two backticked spans, one of them a path and the other not, in a
/// sentence that is not a negation: `the row in ` + a path + ` is ` + a literal.
/// Anything else — one span, three spans, two paths, two literals, "no longer
/// contains" — is prose this module has no business ruling on.
fn parse(criterion: &str) -> Option<Checkable> {
    if negated(criterion) {
        return None;
    }
    let spans: Vec<&str> = criterion
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::trim)
        .collect();
    // Exactly two, so which span is the file and which is the literal is read
    // off the sentence rather than guessed at.
    let [first, second] = spans[..] else {
        return None;
    };
    let (file, literal) = match (path_shaped(first), path_shaped(second)) {
        (true, false) => (first, second),
        (false, true) => (second, first),
        // Two paths, or none: nothing here says which is the value.
        _ => return None,
    };
    (!literal.is_empty()).then(|| Checkable {
        criterion: criterion.to_string(),
        file: file.to_string(),
        literal: literal.to_string(),
    })
}

/// Whether a criterion says something is *not* so.
///
/// A criterion demanding an absence is satisfied by the very reading — the file
/// does not hold the literal — that this module would otherwise report as a
/// mismatch, so a negated one is declined rather than answered backwards. The
/// words are matched whole: "nothing" is not "not", and "cannot" is not "no".
fn negated(criterion: &str) -> bool {
    const WORDS: &[&str] = &[
        "no", "not", "never", "neither", "nor", "nothing", "none", "without", "cannot",
    ];
    const PHRASES: &[&str] = &["rather than", "instead of"];
    let lowered = criterion.to_lowercase();
    PHRASES.iter().any(|phrase| lowered.contains(phrase))
        || lowered
            .split(|c: char| !c.is_alphanumeric() && c != '\'')
            .any(|word| WORDS.contains(&word) || word.ends_with("n't"))
}

/// Whether a backticked span names a file on the branch.
///
/// A path has no whitespace and either a directory separator or a lettered
/// extension. Deliberately not a version: `0.17.5` ends in a dot and a digit and
/// is a value, while `Cargo.toml` ends in a dot and letters and is a file — so
/// "the version in `Cargo.toml` is `0.17.5`" reads as the one file and the one
/// literal it is, rather than as two paths this module then declines.
///
/// An absolute path, and one climbing out of the worktree with `..`, are not
/// files on the branch: the check reads the node's own work, so a criterion
/// naming anything else is declined here rather than resolved and read.
fn path_shaped(span: &str) -> bool {
    if span.is_empty()
        || span.chars().any(char::is_whitespace)
        // Escapes, spelled so that every host reads a criterion the same way
        // rather than each one reading its own separators: a leading `/`, a
        // segment climbing out, a backslash, and a colon — which is a Windows
        // drive prefix on the host that has them and is nothing a criterion here
        // means by a *file* anywhere else.
        || span.starts_with('/')
        || span.contains('\\')
        || span.contains(':')
        || span.split('/').any(|segment| segment == "..")
    {
        return false;
    }
    let lettered_extension = span
        .rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .is_some_and(|(stem, extension)| {
            !stem.is_empty()
                && !extension.is_empty()
                && extension.chars().all(|c| c.is_ascii_alphabetic())
        });
    span.contains('/') || lettered_extension
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A task stating one criterion, so a case reads as the plan that would
    /// carry it.
    fn task(criterion: &str) -> String {
        format!("## What\nShip it.\n\n## Acceptance criteria\n\n- {criterion}\n")
    }

    #[test]
    fn a_criterion_naming_a_file_and_a_literal_is_the_one_shape_this_reads() {
        let found = checkable(&task(
            "the shared journey row in `tests/e2e/shared.rs` is `complete_dataset: true`",
        ));
        assert_eq!(
            found,
            vec![Checkable {
                criterion: "the shared journey row in `tests/e2e/shared.rs` is \
                            `complete_dataset: true`"
                    .to_string(),
                file: "tests/e2e/shared.rs".to_string(),
                literal: "complete_dataset: true".to_string(),
            }]
        );
    }

    #[test]
    fn the_file_and_the_literal_are_read_off_the_sentence_in_either_order() {
        let found = checkable(&task("`0.17.5` is the version `Cargo.toml` declares"));
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].file, "Cargo.toml");
        assert_eq!(found[0].literal, "0.17.5");
    }

    #[test]
    fn a_criterion_this_cannot_parse_is_silence() {
        for prose in [
            // Neither half.
            "the run is faster than it was",
            // A file and no literal.
            "`src/engine.rs` is tidier",
            // A literal and no file.
            "the row is `complete_dataset: true`",
            // Three spans: which two are the pair is a guess.
            "`src/a.rs` and `src/b.rs` both hold `version: 1`",
            // Two files.
            "`src/a.rs` matches `src/b.rs`",
            // Two literals.
            "`version: 1` is not `version: 2`",
            // A negation, which this reading would answer backwards.
            "`src/engine.rs` no longer holds `unwrap()`",
            "`src/engine.rs` does not hold `panic!(`",
            "`src/engine.rs` holds `expect(` rather than `unwrap(`",
            // A path out of the worktree is not a file on the branch.
            "`/etc/passwd` holds `root: yes`",
            "`../elsewhere/notes.md` holds `state: done`",
            // The same escapes as a host that spells them its own way would.
            "`C:\\elsewhere\\notes.md` holds `state: done`",
            "`C:notes.md` holds `state: done`",
            // Backticks with nothing between them.
            "`` is `state: done`",
        ] {
            assert_eq!(
                checkable(&task(prose)),
                vec![],
                "read a bar out of: {prose}"
            );
        }
    }

    #[test]
    fn only_the_acceptance_criteria_section_is_a_bar() {
        let task = "## What\nThe row in `notes.md` is `state: done`.\n\n\
                    ## Acceptance criteria\n\n- it ships\n\n\
                    ## Additional info\n\n- `other.md` holds `state: done`\n";
        assert_eq!(checkable(task), vec![]);
    }

    #[test]
    fn a_criterion_wrapped_over_lines_is_one_criterion() {
        let task = "## Acceptance criteria\n\n- the row in `notes.md`\n  is `state: done`\n";
        let found = checkable(task);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].literal, "state: done");
        assert!(
            found[0]
                .criterion
                .contains("the row in `notes.md` is `state: done`"),
            "the criterion was not rejoined: {found:?}"
        );
    }

    #[test]
    fn a_node_states_its_bar_in_its_task_its_amendment_and_its_steps_and_says_each_once() {
        let node = Node {
            id: "service".into(),
            task: Some(task("`notes.md` holds `state: done`")),
            amendment: Some(task("`version.txt` holds `v: 2`")),
            steps: Some(vec![
                crate::plan::Step {
                    id: "one".into(),
                    task: Some(task("`notes.md` holds `state: done`")),
                    ..crate::plan::Step::default()
                },
                crate::plan::Step {
                    id: "two".into(),
                    task: Some(task("`rows.csv` holds `count: 3`")),
                    ..crate::plan::Step::default()
                },
            ]),
            ..Node::default()
        };
        let files: Vec<String> = checkable_of(&node)
            .into_iter()
            .map(|check| check.file)
            .collect();
        assert_eq!(files, ["notes.md", "version.txt", "rows.csv"]);
    }

    #[test]
    fn a_file_that_holds_the_literal_matches_and_one_that_does_not_says_what_it_holds() {
        let dir = tempdir("holds");
        std::fs::write(dir.join("notes.md"), "state: done\n").expect("the file writes");
        let check = |literal: &str| Checkable {
            criterion: format!("`notes.md` holds `{literal}`"),
            file: "notes.md".into(),
            literal: literal.into(),
        };
        assert_eq!(answer(&dir, &check("state: done")), Answer::Match);
        assert_eq!(
            answer(&dir, &check("state: shipped")),
            Answer::Mismatch {
                holds: "`state: done`".into()
            }
        );
        // A literal the file has no line for at all: the absence is the answer.
        assert_eq!(
            answer(&dir, &check("owner: nobody")),
            Answer::Mismatch {
                holds: "nothing naming `owner`".into()
            }
        );
    }

    #[test]
    fn a_file_the_branch_will_not_give_up_is_neither_answer() {
        let dir = tempdir("unread");
        std::fs::create_dir(dir.join("rows.md")).expect("a directory where a file was named");
        let check = Checkable {
            criterion: "`rows.md` holds `state: done`".into(),
            file: "rows.md".into(),
            literal: "state: done".into(),
        };
        let answered = answer(&dir, &check);
        assert_eq!(answered.as_str(), "unread", "{answered:?}");
        // And a file that is not there at all: nothing was compared, so nothing
        // is reported as a comparison.
        let absent = Checkable {
            file: "gone.md".into(),
            ..check
        };
        assert_eq!(answer(&dir, &absent).as_str(), "unread");
    }

    #[test]
    fn what_a_file_holds_is_bounded_to_one_readable_line() {
        let dir = tempdir("bounded");
        let long = format!("state: {}\n", "x".repeat(500));
        std::fs::write(dir.join("notes.md"), &long).expect("the file writes");
        let Answer::Mismatch { holds } = answer(
            &dir,
            &Checkable {
                criterion: "`notes.md` holds `state: done`".into(),
                file: "notes.md".into(),
                literal: "state: done".into(),
            },
        ) else {
            panic!("a file holding another value is a mismatch");
        };
        assert!(holds.ends_with("…`") || holds.ends_with('…'), "{holds}");
        assert!(holds.chars().count() < 220, "{holds}");
    }

    /// The three answers this module spells are the three the divergence record
    /// proposes.
    ///
    /// They are private vocabulary, so `tests/contract.rs` — which drives the
    /// public surface — cannot reach them, and the entry that proposes them is
    /// the only place they are written down. Both directions: an answer added
    /// here without a line there fails, and so does one the entry names that
    /// this module no longer spells.
    #[test]
    fn the_three_answers_are_the_ones_the_divergence_record_names() {
        let record = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/contract-divergences.md"),
        )
        .expect("the divergence record ships");
        let entry = record
            .split("\n## ")
            .find(|entry| entry.starts_with("47."))
            .expect("the record still carries entry 47");
        let block = entry
            .split("```json")
            .nth(1)
            .and_then(|rest| rest.split("```").next())
            .expect("entry 47 carries the json block this test drives");
        let named: Vec<String> = serde_json::from_str::<serde_json::Value>(block)
            .ok()
            .and_then(|block| serde_json::from_value(block["answers"].clone()).ok())
            .expect("entry 47 names its answers");
        // Spelled by a match rather than by a list, so an answer added to this
        // enum has to be spelled here as well as there.
        let spelled = |answer: &Answer| match answer {
            Answer::Match => "match",
            Answer::Mismatch { .. } => "mismatch",
            Answer::Unread { .. } => "unread",
        };
        let mine: Vec<String> = [
            Answer::Match,
            Answer::Mismatch {
                holds: String::new(),
            },
            Answer::Unread {
                reason: String::new(),
            },
        ]
        .iter()
        .map(|answer| {
            assert_eq!(spelled(answer), answer.as_str(), "{answer:?}");
            answer.as_str().to_string()
        })
        .collect();
        assert_eq!(mine, named);
    }

    /// A scratch directory of one case's own, named after it so two cases
    /// running in the same millisecond cannot write into each other's.
    fn tempdir(case: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "onepipeline-criteria-{}-{case}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }
}
