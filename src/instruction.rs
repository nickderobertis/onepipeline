//! The instruction a consumer is given about the release it depends on.
//!
//! **The producer knows it and the consumer does not.** How a dependent moves
//! from a git pin to a released version is a fact about the *producing*
//! repository — which manifest the pin is in, what has to be re-generated
//! alongside it, whether a lock is committed — and a worker in the consuming
//! repository has no way to find it out. So the producer declares it, once, and
//! this crate renders it wherever a consumer is told about that release.
//!
//! One template is rendered at **both** sites, because they serve the two
//! adoption modes. The [`CROSS_REPO_REFERENCES_HEADING`](crate::plan::CROSS_REPO_REFERENCES_HEADING)
//! block is delivered under both, and for a `published` node it is the only place
//! the version ever appears; the [`arrival_note`] is fast-adoption only, because
//! the mechanism that delivers one filters to the nodes that held a git pin. Both
//! render from a [`CrossRepoReference`], which is why every variable is available
//! at both: the row *is* the variables.
//!
//! **A rendered instruction adds no acceptance criterion.** It is delivered
//! inside prose that states it reports observed state — the note carries
//! [`crate::plan::OBSERVED_STATE_FRAMING`] itself, because a note delivered into
//! a running turn is not wrapped in anything, and the block's preamble carries it
//! too. A producer can say what to do with a release; a producer cannot add a bar
//! to a node it does not own.

use serde::{Deserialize, Serialize};

use crate::plan::{CrossRepoReference, OBSERVED_STATE_FRAMING};

/// What a consumer is told when the producer declares no template of its own.
///
/// **The engine's own instruction, in one place.** Both render sites reach it
/// through [`InstructionTemplate::default`] rather than composing a sentence of
/// their own, so a dependency whose producer has not adopted this gets one
/// sentence and gets it identically wherever it is told.
pub const DEFAULT_INSTRUCTION: &str = "Move from the git pin to that released version.";

/// The variables a template may name, at either render site.
///
/// All of them are available in **both** adoption modes. `version` is the one a
/// template has reason to guard on: at a fast node's first render no release has
/// happened, so it is empty there and a template that guards renders the other
/// branch — which is what fast adoption *is*, rather than a gap to close.
pub const VARIABLES: &[&str] = &[
    "dependency",
    "repository",
    "branch",
    "commit",
    "target",
    "version",
];

/// How long a declared instruction may be, in **bytes**.
///
/// It is a paragraph a worker reads beside the dependency it is about, not the
/// procedure behind it — and it is rendered into a task, into a note delivered to
/// a running turn, and into an event payload, so a bound here is a bound on all
/// three. Bytes rather than characters because that is what the payload bound
/// beside it counts, and a bound stated in one unit and enforced in the other is
/// a refusal nobody can predict.
const MAX_INSTRUCTION: usize = 1_000;

/// One producer's declared instruction, checked where it is read.
///
/// A plan is external input, so what may spell a template is decided in the
/// conversion: a name no variable answers to is refused **by that name**, as is a
/// section left open, so a typo fails loudly at the boundary instead of rendering
/// as itself in a worker's task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct InstructionTemplate {
    /// What the producer wrote, which is what round-trips.
    source: String,
    /// What it parsed to, which is what renders.
    segments: Vec<Segment>,
}

impl InstructionTemplate {
    /// This instruction, rendered for one dependency.
    ///
    /// The row carries every variable, so this is the whole of what either site
    /// has to hand over. Trimmed, because a template that guards on a variable
    /// leaves the branch it did not take behind as whitespace.
    pub fn render(&self, of: &CrossRepoReference) -> String {
        let mut rendered = String::with_capacity(self.source.len());
        render_into(&self.segments, of, &mut rendered);
        rendered.trim().to_owned()
    }

    /// The template as its producer declared it.
    pub fn as_str(&self) -> &str {
        &self.source
    }
}

impl Default for InstructionTemplate {
    /// [`DEFAULT_INSTRUCTION`], which names no variable and therefore parses to
    /// itself. Built rather than parsed, so the one fallback in this crate has no
    /// failure path to get wrong.
    fn default() -> Self {
        Self {
            source: DEFAULT_INSTRUCTION.to_owned(),
            segments: vec![Segment::Literal(DEFAULT_INSTRUCTION.to_owned())],
        }
    }
}

impl std::fmt::Display for InstructionTemplate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.source)
    }
}

impl TryFrom<String> for InstructionTemplate {
    type Error = String;

    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        if value.trim().is_empty() {
            return Err("a release instruction cannot be blank".to_owned());
        }
        if value.len() > MAX_INSTRUCTION {
            return Err(format!(
                "a release instruction is at most {MAX_INSTRUCTION} bytes, and this one is {}",
                value.len()
            ));
        }
        // A newline is what makes it a paragraph; every other control character
        // renders as something other than what it is in a table cell, in a task,
        // and in an event payload alike.
        if let Some(control) = value.chars().find(|c| c.is_control() && *c != '\n') {
            return Err(format!(
                "a release instruction cannot carry the control character {control:?}"
            ));
        }
        // **The one thing a multi-line instruction must not do.** It is rendered
        // inside prose that says it reports observed state and adds no
        // acceptance criteria, and a line opening a Markdown section of its own
        // ends that prose: a producer could write `## Acceptance criteria` into
        // a consuming node's task and add a bar to a node it does not own. So a
        // heading is refused here, at the boundary, rather than escaped at each
        // of the two render sites.
        if let Some(heading) = value.lines().find(|line| opens_a_section(line)) {
            return Err(format!(
                "a release instruction cannot open a section of its own, and the line \
                 {heading:?} does; it is rendered inside prose stating that it reports observed \
                 state and adds no acceptance criteria"
            ));
        }
        let segments = parse(&value)?;
        Ok(Self {
            source: value,
            segments,
        })
    }
}

impl From<InstructionTemplate> for String {
    fn from(template: InstructionTemplate) -> Self {
        template.source
    }
}

/// Whether one line would open a Markdown section where it is rendered.
///
/// Both spellings, because both end the prose the instruction is rendered inside:
/// an ATX heading (`## …`) and a setext underline, which turns the line *above*
/// it into a heading and is also how a horizontal rule is written.
fn opens_a_section(line: &str) -> bool {
    let line = line.trim();
    if line.starts_with('#') {
        return true;
    }
    let underline = |c: char| line.len() >= 3 && line.chars().all(|each| each == c);
    underline('=') || underline('-')
}

/// One piece of a parsed template.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    /// Text, rendered as itself.
    Literal(String),
    /// `{{name}}`, rendered as that variable's value.
    Variable(&'static str),
    /// `{{#name}}…{{/name}}` and `{{^name}}…{{/name}}`: the guard a template
    /// needs to say one thing where a version is known and another where it is
    /// not.
    Section {
        /// The variable the guard is on.
        name: &'static str,
        /// Which way round the guard reads.
        guard: Guard,
        /// What renders when it holds.
        inner: Vec<Segment>,
    },
}

/// Which way round one guard reads.
///
/// The two spellings a template has, named rather than carried as a bare flag:
/// `true` beside a variable name says nothing about which of them it is, and the
/// parser stacks the same value while a section is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Guard {
    /// `{{#name}}`: render this where the variable **has** a value.
    WhenSet,
    /// `{{^name}}`: render this where it does not — which for `version` is a
    /// fast node before any release has happened.
    WhenUnset,
}

impl Guard {
    /// Whether this guard holds for a variable worth `value`.
    fn holds(self, value: &str) -> bool {
        match self {
            Guard::WhenSet => !value.is_empty(),
            Guard::WhenUnset => value.is_empty(),
        }
    }
}

/// The variable one tag names, or a refusal naming it and the ones there are.
fn variable(name: &str) -> std::result::Result<&'static str, String> {
    VARIABLES
        .iter()
        .copied()
        .find(|known| *known == name)
        .ok_or_else(|| {
            format!(
                "a release instruction names no variable `{name}`; it may name {}",
                VARIABLES
                    .iter()
                    .map(|known| format!("`{known}`"))
                    .collect::<Vec<String>>()
                    .join(", ")
            )
        })
}

/// Read one template's segments, or say why it is not one.
fn parse(source: &str) -> std::result::Result<Vec<Segment>, String> {
    let mut done: Vec<Segment> = Vec::new();
    let mut open: Vec<(&'static str, Guard, Vec<Segment>)> = Vec::new();
    let mut literal = String::new();
    let mut rest = source;
    loop {
        let Some(at) = rest.find("{{") else {
            literal.push_str(rest);
            break;
        };
        literal.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        let Some(end) = after.find("}}") else {
            return Err("a release instruction opens `{{` and never closes it".to_owned());
        };
        let tag = after[..end].trim().to_owned();
        rest = &after[end + 2..];
        flush(&mut literal, &mut done, &mut open);
        if let Some(name) = tag.strip_prefix('/') {
            let name = variable(name.trim())?;
            match open.pop() {
                Some((opened, guard, inner)) if opened == name => push(
                    Segment::Section {
                        name: opened,
                        guard,
                        inner,
                    },
                    &mut done,
                    &mut open,
                ),
                Some((opened, ..)) => {
                    return Err(format!(
                        "a release instruction closes `{{{{/{name}}}}}` where `{opened}` is open"
                    ))
                }
                None => {
                    return Err(format!(
                        "a release instruction closes `{{{{/{name}}}}}`, which it never opened"
                    ))
                }
            }
        } else if let Some(name) = tag.strip_prefix('#') {
            open.push((variable(name.trim())?, Guard::WhenSet, Vec::new()));
        } else if let Some(name) = tag.strip_prefix('^') {
            open.push((variable(name.trim())?, Guard::WhenUnset, Vec::new()));
        } else {
            push(Segment::Variable(variable(&tag)?), &mut done, &mut open);
        }
    }
    flush(&mut literal, &mut done, &mut open);
    if let Some((name, ..)) = open.last() {
        return Err(format!(
            "a release instruction opens `{{{{#{name}}}}}` and never closes it"
        ));
    }
    Ok(done)
}

/// Add one segment to whatever is being read: the open section, or the template.
fn push(
    segment: Segment,
    done: &mut Vec<Segment>,
    open: &mut [(&'static str, Guard, Vec<Segment>)],
) {
    match open.last_mut() {
        Some((.., inner)) => inner.push(segment),
        None => done.push(segment),
    }
}

/// Add the literal read so far, if there is any, and start the next one.
fn flush(
    literal: &mut String,
    done: &mut Vec<Segment>,
    open: &mut [(&'static str, Guard, Vec<Segment>)],
) {
    if !literal.is_empty() {
        push(Segment::Literal(std::mem::take(literal)), done, open);
    }
}

/// What one variable is worth for one dependency.
///
/// A cell the run could not observe is **empty** rather than absent — the same
/// rule the row itself is built under — so a template naming it renders nothing
/// there and a template guarding on it takes the other branch.
fn value_of<'a>(of: &'a CrossRepoReference, name: &str) -> &'a str {
    match name {
        "dependency" => &of.dependency,
        "repository" => &of.repository,
        "branch" => &of.branch,
        "commit" => &of.commit,
        "target" => &of.release_target,
        // Every name reaching here came through `variable`, so the only one left
        // is the last. A name nothing answered would be a variable this crate
        // published and forgot to read, which the module's own test refuses.
        _ => &of.version,
    }
}

fn render_into(segments: &[Segment], of: &CrossRepoReference, out: &mut String) {
    for segment in segments {
        match segment {
            Segment::Literal(text) => out.push_str(text),
            Segment::Variable(name) => out.push_str(value_of(of, name)),
            Segment::Section { name, guard, inner } => {
                if guard.holds(value_of(of, name)) {
                    render_into(inner, of, out);
                }
            }
        }
    }
}

/// The instructions one set of dependencies renders to, in the order they are
/// named.
///
/// **Deduplicated**, because the common case is several dependencies whose
/// producers declare nothing: they render one sentence between them, and a worker
/// reading it three times would read three of them as three different
/// instructions. One that renders to nothing — a template whose every guard took
/// the empty branch — is left out rather than rendered as a blank paragraph.
pub(crate) fn instructions(of: &[CrossRepoReference]) -> Vec<String> {
    let mut rendered: Vec<String> = Vec::new();
    for reference in of {
        let instruction = reference.instruction.render(reference);
        if !instruction.is_empty() && !rendered.contains(&instruction) {
            rendered.push(instruction);
        }
    }
    rendered
}

/// The note a fast-adoption node is sent when the releases it was waiting on
/// arrive.
///
/// It **adds no bar**: it reports observed state and says what to do with it, and
/// it says so itself — a note delivered into a running turn is delivered as it is
/// written, with nothing around it to frame it. One function, called both where
/// the note is delivered and where a journalled delivery is folded back, so a
/// note replayed from the record is the note that was sent.
pub fn arrival_note(arrived: &[CrossRepoReference]) -> String {
    let arrivals: Vec<String> = arrived
        .iter()
        .map(|arrival| {
            format!(
                "- {} — {} {}",
                arrival.repository, arrival.release_target, arrival.version
            )
            .trim_end()
            .to_owned()
        })
        .collect();
    let note = format!(
        "The releases this node was waiting on have arrived. \
         {OBSERVED_STATE_FRAMING}\n\n{}",
        arrivals.join("\n"),
    );
    let instructions = instructions(arrived);
    if instructions.is_empty() {
        return note;
    }
    format!("{note}\n\n{}", instructions.join("\n\n"))
}
