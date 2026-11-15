//! How an expandable (collapsed-by-default) block is spelled on each kind of destination,
//! decided once.
//!
//! A record held in a folder of Markdown carries an expandable block in **the authored form**:
//!
//! ```text
//! <details>
//! <summary>SUMMARY</summary>
//!
//! BODY
//!
//! </details>
//! ```
//!
//! outside any fenced code block and not indented, `<details>` with no attribute, SUMMARY on one
//! line with `<code>`, `<strong>` and the entities `&amp;`, `&lt;` and `&gt;` as its only markup,
//! one blank line on either side of a BODY of one or more lines, and no block inside another. The
//! README's "Expandable blocks" states it for a consumer, and [`blocks`] is what recognises it.
//!
//! Each destination kind has exactly one [`ExpandableForm`] entry, and every path that writes or
//! reads a block for that kind reads it: [`ExpandableForm::written`] writes the destination's form
//! from the authored one, [`ExpandableForm::read`] reads it back, and [`ExpandableForm::markup`]
//! hands a plugin both as the [`BodyMarkup`] it applies to every body it sends and receives, so
//! no plugin holds a copy of any form. A folder of Markdown and a GitHub issue keep the authored
//! form byte for byte; Linear keeps a collapsible section, `+++ <summary>` … `+++`.
//!
//! A shape that is not the authored form — an attribute on `<details>`, a missing blank line, an
//! indented or fenced block, a nested block, a summary holding another tag — is never translated,
//! and neither is a block whose Linear form would not read back as itself — one written with
//! carriage returns among them: see [`ExpandableForm::written`].

use std::sync::Arc;

use onetaskgraph_plugin_api::{BodyMarkup, SharedBodyMarkup};

// llmlint: ignore-block[code_lands_in_the_domain_that_owns_it] The task contract this module implements requires each destination kind's expandable form to be one entry of `ExpandableForm` in `onetaskgraph-core`, the one place that kind's form is written and read back; no plugin may depend on the engine (AGENTS.md), so a plugin cannot own a form the engine reads, and Linear's form — with what it remembers of blocks it read back as literal text, which is part of reading that form back — reaches the Linear plugin only as the `BodyMarkup` this entry hands it, as GitHub's anchors do in `section_links`.
/// One destination kind's expandable form: the one entry each kind has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpandableForm {
    /// A folder of Markdown: the authored form, byte for byte.
    LocalMd,
    /// A GitHub Projects board's issues: the authored form, byte for byte. The anchor line the
    /// GitHub source writes before each heading (see `section_links`) reaches a heading inside a
    /// block's body as it reaches any other.
    GithubProjects,
    /// Linear issues, projects, documents and comments: a collapsible section, `+++ LSUMMARY`
    /// alone on its line, a blank line, the body, a blank line, and `+++` alone on its line.
    /// LSUMMARY is SUMMARY with `<code>TEXT</code>` as `` `TEXT` ``, `<strong>TEXT</strong>` as
    /// `**TEXT**`, and each entity as the character it stands for, which is how Linear stores it.
    Linear,
}

impl ExpandableForm {
    /// The entry for a source of plugin kind `kind`, or `None` for a kind with no expandable
    /// form — a plugin served over stdio among them — which carries the authored form unchanged.
    #[must_use]
    pub fn for_kind(kind: &str) -> Option<Self> {
        match kind {
            "local-md" => Some(Self::LocalMd),
            "github-projects" => Some(Self::GithubProjects),
            "linear" => Some(Self::Linear),
            _ => None,
        }
    }

    /// `authored` with every authored block written in this kind's form.
    ///
    /// For Linear a block is written as a collapsible section only when that section reads back,
    /// by [`Self::read`], as the very block it was written from. A summary whose plain text holds
    /// a backtick or a `*`, which Linear would read as a code or a strong mark, or a raw `&`, `<`
    /// or `>`, which Linear's stored text cannot tell from an escaped one, fails that test, and
    /// its block is carried as it stands.
    #[must_use]
    pub fn written(self, authored: &str) -> String {
        match self {
            Self::LocalMd | Self::GithubProjects => authored.to_owned(),
            Self::Linear => linear_written(authored, &std::collections::HashSet::new()),
        }
    }

    /// `stored`, as this kind's backend holds it, with every block in this kind's form read back
    /// into the authored form: exactly what [`Self::written`] translates, and a section a person
    /// made in the tracker whose authored block would be written as that same section.
    #[must_use]
    pub fn read(self, stored: &str) -> String {
        match self {
            Self::LocalMd | Self::GithubProjects => stored.to_owned(),
            Self::Linear => rewritten(stored, &linear_regions(stored), |region| {
                let Shape::Section(section) = &region.shape else {
                    return None;
                };
                let block = authored_block(section)?;
                (linear_section(&block).as_ref() == Some(section))
                    .then(|| block.render(region.ending))
            }),
        }
    }

    /// The markup this kind's bodies carry for their blocks to be expandable there, when the kind
    /// spells a block otherwise than the authored form: only Linear does.
    #[must_use]
    pub fn markup(self) -> Option<SharedBodyMarkup> {
        match self {
            Self::Linear => Some(Arc::new(LinearSections::default())),
            Self::LocalMd | Self::GithubProjects => None,
        }
    }
}

// llmlint: ignore-end[code_lands_in_the_domain_that_owns_it]

/// One expandable block in the authored form: made only by [`authored_parts`], which holds it to
/// the form, and by [`authored_block`], which reads it out of a Linear section.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Block {
    /// SUMMARY, as written between `<summary>` and `</summary>`.
    summary: String,
    /// BODY: every line between the blank line after the summary and the blank line before
    /// `</details>`, joined by line breaks.
    body: String,
}

impl Block {
    /// The block in the authored form, its last line ended by `ending`.
    fn render(&self, ending: &str) -> String {
        format!(
            "<details>\n<summary>{}</summary>\n\n{}\n\n</details>{ending}",
            self.summary, self.body
        )
    }
}

/// How many blocks of `content` are in the authored form. A block-like shape that is not the
/// form — see the module documentation — is not one, and nor is anything inside it.
#[must_use]
pub fn blocks(content: &str) -> usize {
    authored_regions(content)
        .iter()
        .filter(|region| matches!(region.shape, Shape::Block(_)))
        .count()
}

/// The lines from one opening line to its closing one, and what they hold when they are
/// exactly one block of a form.
struct Region {
    /// The first and last line, by index.
    lines: (usize, usize),
    /// The last line's own line break.
    ending: &'static str,
    /// What they are.
    shape: Shape,
}

/// What the lines of one [`Region`] are.
enum Shape {
    /// Exactly one block in the authored form.
    Block(Block),
    /// Exactly one section in Linear's form.
    Section(Section),
    /// Neither: a shape that is not a form, carried as it stands.
    Other,
}

/// One line of a body: its text without its line break, and the break.
struct Line<'a> {
    text: &'a str,
    ending: &'static str,
}

/// `content` cut into lines, each with its own line break.
fn lines(content: &str) -> Vec<Line<'_>> {
    content
        .split_inclusive('\n')
        .map(|line| {
            let text = line.trim_end_matches(['\n', '\r']);
            let ending = match &line[text.len()..] {
                "\r\n" => "\r\n",
                "\n" => "\n",
                "\r" => "\r",
                _ => "",
            };
            Line { text, ending }
        })
        .collect()
}

/// A fence's marker and width, when `line` — indented at most three spaces — opens or closes
/// one.
fn fence_of(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let marker = trimmed.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let width = trimmed.chars().take_while(|c| *c == marker).count();
    (width >= 3).then_some((marker, width))
}

/// Which lines of `lines` sit inside a fenced code block, the fence lines themselves included.
fn fenced(lines: &[Line<'_>]) -> Vec<bool> {
    let mut fence: Option<(char, usize)> = None;
    lines
        .iter()
        .map(|line| match (fence, fence_of(line.text)) {
            (None, Some(opened)) => {
                fence = Some(opened);
                true
            }
            (Some((open, wide)), Some((marker, width))) if open == marker && width >= wide => {
                fence = None;
                true
            }
            (Some(_), _) => true,
            (None, None) => false,
        })
        .collect()
}

/// Every region of `content` an unindented `<details` line outside fenced code opens, through
/// the `</details>` line that closes it, a `<details` line between them nesting one level deeper.
/// A region that is exactly one authored block carries it; any other, the nested shape among
/// them, carries none, and nothing inside it is a block either.
fn authored_regions(content: &str) -> Vec<Region> {
    let lines = lines(content);
    let fenced = fenced(&lines);
    let opens = |index: usize| {
        !fenced[index]
            && (lines[index].text == "<details>" || lines[index].text.starts_with("<details "))
    };
    let closes = |index: usize| !fenced[index] && lines[index].text == "</details>";
    let mut regions = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if !opens(index) {
            index += 1;
            continue;
        }
        let mut depth = 0_usize;
        let mut close = None;
        for at in index..lines.len() {
            if opens(at) {
                depth += 1;
            } else if closes(at) {
                depth -= 1;
                if depth == 0 {
                    close = Some(at);
                    break;
                }
            }
        }
        let Some(close) = close else {
            index += 1;
            continue;
        };
        let nested = (index + 1..close).any(opens);
        // A block written with carriage returns would not read back from Linear byte for byte —
        // Linear keeps none — so it is no block to translate.
        let returned = lines[index..=close]
            .iter()
            .any(|line| line.ending != "\n" && !line.ending.is_empty());
        let block = (!nested && !returned && lines[index].text == "<details>")
            .then(|| authored_parts(&lines[index..=close]))
            .flatten()
            .filter(|block| summary_markup(&block.summary).is_some());
        regions.push(Region {
            lines: (index, close),
            ending: lines[close].ending,
            shape: block.map_or(Shape::Other, Shape::Block),
        });
        index = close + 1;
    }
    regions
}

/// The block `lines` — `<details>` through `</details>` — are, when the lines between hold the
/// summary line, a blank line, a body whose first and last lines hold something, and a blank line.
fn authored_parts(lines: &[Line<'_>]) -> Option<Block> {
    let [_, summary, blank, body @ .., after, _] = lines else {
        return None;
    };
    let summary = summary
        .text
        .strip_prefix("<summary>")?
        .strip_suffix("</summary>")?;
    let (first, last) = (body.first()?, body.last()?);
    (blank.text.is_empty()
        && after.text.is_empty()
        && !first.text.trim().is_empty()
        && !last.text.trim().is_empty()
        && !summary.contains("</summary>"))
    .then(|| Block {
        summary: summary.to_owned(),
        body: joined(body),
    })
}

/// `lines` joined by their own line breaks, the last one's left off.
fn joined(lines: &[Line<'_>]) -> String {
    let mut text = String::new();
    for (index, line) in lines.iter().enumerate() {
        text.push_str(line.text);
        if index + 1 < lines.len() {
            text.push_str(if line.ending.is_empty() {
                "\n"
            } else {
                line.ending
            });
        }
    }
    text
}

/// `content` with each region `replace` answers for put in that answer's place, every other line
/// as it was.
fn rewritten(
    content: &str,
    regions: &[Region],
    replace: impl Fn(&Region) -> Option<String>,
) -> String {
    let lines = lines(content);
    let mut text = String::with_capacity(content.len());
    let mut regions = regions.iter().peekable();
    let mut index = 0;
    while index < lines.len() {
        if let Some(region) = regions.next_if(|region| region.lines.0 == index)
            && let Some(replaced) = replace(region)
        {
            text.push_str(&replaced);
            index = region.lines.1 + 1;
            continue;
        }
        text.push_str(lines[index].text);
        text.push_str(lines[index].ending);
        index += 1;
    }
    text
}

/// The text of `region` of `lines`, its last line's break left off.
fn region_text(lines: &[Line<'_>], region: &Region) -> String {
    joined(&lines[region.lines.0..=region.lines.1])
}

/// One run of a summary's markup.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Run {
    /// Text, its entities decoded.
    Text(String),
    /// `<code>TEXT</code>`, TEXT decoded.
    Code(String),
    /// `<strong>TEXT</strong>`, TEXT decoded.
    Strong(String),
}

/// SUMMARY's runs, when it is the authored form's summary: non-empty, with `<code>` and
/// `<strong>` — neither nested in the other, their TEXT holding no tag, no backtick and no `*` —
/// as its only tags, and `&` only in `&amp;`, `&lt;` and `&gt;`.
fn summary_markup(summary: &str) -> Option<Vec<Run>> {
    if summary.trim().is_empty() {
        return None;
    }
    let mut runs = Vec::new();
    let mut rest = summary;
    let mut text = String::new();
    while let Some(character) = rest.chars().next() {
        let tagged = [("<code>", "</code>"), ("<strong>", "</strong>")]
            .into_iter()
            .find(|(open, _)| rest.starts_with(open));
        if let Some((open, close)) = tagged {
            let inner_end = rest[open.len()..].find(close)?;
            let inner = &rest[open.len()..open.len() + inner_end];
            if inner.is_empty() || inner.contains(['<', '>', '`', '*']) {
                return None;
            }
            let inner = decoded(inner)?;
            if !text.is_empty() {
                runs.push(Run::Text(std::mem::take(&mut text)));
            }
            runs.push(if open == "<code>" {
                Run::Code(inner)
            } else {
                Run::Strong(inner)
            });
            rest = &rest[open.len() + inner_end + close.len()..];
            continue;
        }
        if matches!(character, '<' | '>') {
            return None;
        }
        if character == '&' {
            let (entity, stands_for) = ENTITIES
                .into_iter()
                .find(|(entity, _)| rest.starts_with(entity))?;
            text.push(stands_for);
            rest = &rest[entity.len()..];
            continue;
        }
        text.push(character);
        rest = &rest[character.len_utf8()..];
    }
    if !text.is_empty() {
        runs.push(Run::Text(text));
    }
    Some(runs)
}

/// The three entities a summary writes, and the character each stands for.
const ENTITIES: [(&str, char); 3] = [("&amp;", '&'), ("&lt;", '<'), ("&gt;", '>')];

/// `text` with each of [`ENTITIES`] decoded, when every `&` in it begins one.
fn decoded(text: &str) -> Option<String> {
    let mut plain = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(character) = rest.chars().next() {
        if character == '&' {
            let (entity, stands_for) = ENTITIES
                .into_iter()
                .find(|(entity, _)| rest.starts_with(entity))?;
            plain.push(stands_for);
            rest = &rest[entity.len()..];
        } else {
            plain.push(character);
            rest = &rest[character.len_utf8()..];
        }
    }
    Some(plain)
}

/// `text` with `&`, `<` and `>` escaped as the authored summary writes them.
fn escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// llmlint: ignore-block[code_lands_in_the_domain_that_owns_it] This region is Linear's form alone — its section, how it is parsed back, how it is written, and the `BodyMarkup` that hands it to the Linear plugin. The task contract this module implements requires each destination kind's expandable form to be one entry of `ExpandableForm` in `onetaskgraph-core`, the one place that kind's form is written and read back; no plugin may depend on the engine (AGENTS.md), so a plugin cannot own a form the engine reads, and Linear's form — with what it remembers of blocks it read back as literal text, which is part of reading that form back — reaches the Linear plugin only as the `BodyMarkup` this entry hands it, as GitHub's anchors do in `section_links`.
/// A Linear collapsible section: its summary as Linear stores it, and its body.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Section {
    summary: String,
    body: String,
}

impl Section {
    /// The section as Linear's Markdown spells it, its last line ended by `ending`.
    fn render(&self, ending: &str) -> String {
        format!("+++ {}\n\n{}\n\n+++{ending}", self.summary, self.body)
    }
}

/// Every region of `stored` a `+++ <summary>` line outside fenced code opens, through the `+++`
/// line that closes it, an inner `+++ ` line nesting one level deeper. A region that is exactly
/// one section, its first and last body lines holding something, carries it.
fn linear_regions(stored: &str) -> Vec<Region> {
    let lines = lines(stored);
    let fenced = fenced(&lines);
    let opens = |index: usize| !fenced[index] && lines[index].text.starts_with("+++ ");
    let closes = |index: usize| !fenced[index] && lines[index].text == "+++";
    let mut regions = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if !opens(index) {
            index += 1;
            continue;
        }
        let mut depth = 0_usize;
        let mut close = None;
        for at in index..lines.len() {
            if opens(at) {
                depth += 1;
            } else if closes(at) {
                depth -= 1;
                if depth == 0 {
                    close = Some(at);
                    break;
                }
            }
        }
        let Some(close) = close else {
            index += 1;
            continue;
        };
        let section = match &lines[index..=close] {
            [header, blank, body @ .., after, _]
                if blank.text.is_empty()
                    && after.text.is_empty()
                    && body
                        .first()
                        .is_some_and(|line| !line.text.trim().is_empty())
                    && body.last().is_some_and(|line| !line.text.trim().is_empty()) =>
            {
                let summary = &header.text["+++ ".len()..];
                (!summary.trim().is_empty()).then(|| Section {
                    summary: summary.to_owned(),
                    body: joined(body),
                })
            }
            _ => None,
        };
        regions.push(Region {
            lines: (index, close),
            ending: lines[close].ending,
            shape: section.map_or(Shape::Other, Shape::Section),
        });
        index = close + 1;
    }
    regions
}

/// `block` as a Linear section, when its summary is the authored form's and its Linear summary
/// can be spelled — its text holding no backtick and no `*`, which Linear reads as marks, and
/// no space at either end.
fn linear_section(block: &Block) -> Option<Section> {
    // Linear keeps no space at either end of a section's summary.
    if block.summary.trim() != block.summary {
        return None;
    }
    let mut summary = String::new();
    for run in summary_markup(&block.summary)? {
        match run {
            Run::Text(text) if text.contains(['`', '*']) => return None,
            Run::Text(text) => summary.push_str(&text),
            Run::Code(text) => {
                summary.push('`');
                summary.push_str(&text);
                summary.push('`');
            }
            Run::Strong(text) => {
                summary.push_str("**");
                summary.push_str(&text);
                summary.push_str("**");
            }
        }
    }
    Some(Section {
        summary,
        body: block.body.clone(),
    })
}

/// `section`'s rendering read back as one section, which a rendering of one always is.
fn section_parts(section: &Section) -> Option<Section> {
    linear_regions(&section.render(""))
        .into_iter()
        .next()
        .and_then(|region| match region.shape {
            Shape::Section(section) => Some(section),
            Shape::Block(_) | Shape::Other => None,
        })
}

/// The authored block a Linear section reads back as: its summary's backtick spans as `<code>`,
/// its `**` spans as `<strong>`, and every `&`, `<` and `>` of its text escaped again, because
/// Linear stores entities decoded.
fn authored_block(section: &Section) -> Option<Block> {
    let mut summary = String::new();
    let mut rest = section.summary.as_str();
    while let Some(character) = rest.chars().next() {
        let marked = [("`", "<code>", "</code>"), ("**", "<strong>", "</strong>")]
            .into_iter()
            .find_map(|(mark, open, close)| {
                let inner = rest.strip_prefix(mark)?;
                let end = inner.find(mark)?;
                (end > 0).then(|| (mark.len() * 2 + end, open, &inner[..end], close))
            });
        if let Some((taken, open, inner, close)) = marked {
            summary.push_str(open);
            summary.push_str(&escaped(inner));
            summary.push_str(close);
            rest = &rest[taken..];
            continue;
        }
        summary.push_str(&escaped(&character.to_string()));
        rest = &rest[character.len_utf8()..];
    }
    let block = Block {
        summary,
        body: section.body.clone(),
    };
    (summary_markup(&block.summary).is_some()).then_some(block)
}

/// `authored` with every authored block written as a Linear section — but for a block whose
/// whole text is in `literal`, which is carried as it stands.
fn linear_written(authored: &str, literal: &std::collections::HashSet<String>) -> String {
    let lines = lines(authored);
    rewritten(authored, &authored_regions(authored), |region| {
        let Shape::Block(block) = &region.shape else {
            return None;
        };
        if literal.contains(&region_text(&lines, region)) {
            return None;
        }
        let section = linear_section(block)?;
        (authored_block(&section_parts(&section)?)? == *block)
            .then(|| section.render(region.ending))
    })
}

/// Linear's collapsible sections, as the Linear source applies them to every body it sends and
/// receives.
///
/// What Linear stores of a body it was sent can hold a block in the authored form as literal text:
/// a shape the authored form does not recognise — `<details>` straight followed by a list, say —
/// that Linear's own rewrite sets apart with the blank line the form wants, so it reads back as an
/// authored block though Linear shows it as paragraphs. Written back as it reads, as a copy's
/// follow-up writes what the destination stored, it would be translated into a section the author
/// never wrote. So each block a read finds held as literal text is remembered by its text, for
/// as long as this source lives, and a later write carries it as it stands: a body read from
/// Linear and written back is what Linear stored.
#[derive(Default)]
struct LinearSections {
    /// The text of every block in the authored form a read found Linear holding as literal text.
    literal: std::sync::Mutex<std::collections::HashSet<String>>,
}

impl BodyMarkup for LinearSections {
    fn written(&self, content: &str) -> String {
        let literal = self
            .literal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        linear_written(content, &literal)
    }

    fn read(&self, content: &str) -> String {
        let lines = lines(content);
        let held: Vec<String> = authored_regions(content)
            .iter()
            .filter(|region| matches!(region.shape, Shape::Block(_)))
            .map(|region| region_text(&lines, region))
            .collect();
        if !held.is_empty() {
            self.literal
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend(held);
        }
        ExpandableForm::Linear.read(content)
    }
}
// llmlint: ignore-end[code_lands_in_the_domain_that_owns_it]

#[cfg(test)]
mod tests {
    use super::{ExpandableForm, blocks};

    const AUTHORED: &str = "Intro.\n\n<details>\n<summary>Run <code>copy --dry-run</code> &amp; \
                            <strong>read</strong> &lt;it&gt;</summary>\n\n- one\n- two\n\n\
                            </details>\n\nAfter.\n";
    const LINEAR: &str =
        "Intro.\n\n+++ Run `copy --dry-run` & **read** <it>\n\n- one\n- two\n\n+++\n\nAfter.\n";

    #[test]
    fn the_authored_form_is_written_as_a_linear_section_and_read_back_byte_for_byte() {
        assert_eq!(ExpandableForm::Linear.written(AUTHORED), LINEAR);
        assert_eq!(ExpandableForm::Linear.read(LINEAR), AUTHORED);
        for kind in [ExpandableForm::LocalMd, ExpandableForm::GithubProjects] {
            assert_eq!(kind.written(AUTHORED), AUTHORED);
            assert_eq!(kind.read(LINEAR), LINEAR);
        }
        assert_eq!(blocks(AUTHORED), 1);
    }

    #[test]
    fn a_shape_that_is_not_the_authored_form_is_carried_as_it_stands() {
        for shape in [
            "<details open>\n<summary>S</summary>\n\nBody.\n\n</details>\n",
            "<details>\n<summary>S</summary>\nBody.\n\n</details>\n",
            "```\n<details>\n<summary>S</summary>\n\nBody.\n\n</details>\n```\n",
            "  <details>\n  <summary>S</summary>\n\n  Body.\n\n  </details>\n",
            "<details>\n<summary>S</summary>\n\nOuter.\n\n<details>\n<summary>I</summary>\n\n\
             Inner.\n\n</details>\n\n</details>\n",
            "<details>\n<summary>S <em>x</em></summary>\n\nBody.\n\n</details>\n",
            "<details>\n<summary>A `tick`</summary>\n\nBody.\n\n</details>\n",
            "<details>\n<summary>A * star</summary>\n\nBody.\n\n</details>\n",
            "<details>\n<summary>Q & A</summary>\n\nBody.\n\n</details>\n",
            "<details>\n<summary> S</summary>\n\nBody.\n\n</details>\n",
            "<details>\r\n<summary>S</summary>\r\n\r\nBody.\r\n\r\n</details>\r\n",
        ] {
            assert_eq!(ExpandableForm::Linear.written(shape), shape, "{shape}");
        }
    }

    #[test]
    fn a_section_made_in_linear_reads_back_as_an_authored_block_and_its_end_is_kept() {
        assert_eq!(
            ExpandableForm::Linear.read("+++ Made in Linear\n\nBody.\n\n+++"),
            "<details>\n<summary>Made in Linear</summary>\n\nBody.\n\n</details>"
        );
        // Linear's own nested section stays as it is inside the block it reads back as.
        let nested = "+++ Outer\n\nOuter.\n\n+++ Inner\n\nInner.\n\n+++\n\n+++\n";
        let read = ExpandableForm::Linear.read(nested);
        assert_eq!(
            read,
            "<details>\n<summary>Outer</summary>\n\nOuter.\n\n+++ Inner\n\nInner.\n\n+++\n\n\
             </details>\n"
        );
        assert_eq!(ExpandableForm::Linear.written(&read), nested);
        // An empty summary, as Linear writes after a stray `>>>`, is no block.
        let empty = "+++ \n\nBody.\n\n+++";
        assert_eq!(ExpandableForm::Linear.read(empty), empty);
    }

    /// The engine hands a source one markup, so no kind may need a section form's and an
    /// expandable form's both.
    #[test]
    fn no_kind_carries_both_a_section_markup_and_an_expandable_one() {
        for kind in [
            "local-md",
            "github-projects",
            "linear",
            "in-memory",
            "stdio",
        ] {
            let section = crate::section_links::SectionForm::for_kind(kind)
                .and_then(crate::section_links::SectionForm::markup);
            let expandable = ExpandableForm::for_kind(kind).and_then(ExpandableForm::markup);
            assert!(section.is_none() || expandable.is_none(), "{kind}");
        }
    }

    /// A block Linear holds as literal text reads back as it is stored, and is written back so,
    /// though its text is in the authored form: Linear rewrote a shape that was not.
    #[test]
    fn a_block_linear_holds_as_literal_text_is_written_back_as_it_was_stored() {
        let markup = ExpandableForm::Linear.markup().expect("Linear's markup");
        let literal =
            "Before.\n\n<details>\n<summary>Held as text</summary>\n\n* a list\n\n</details>";
        let read = markup.read(literal);
        assert_eq!(read, literal);
        assert_eq!(
            markup.written(&format!("{read}\n\nA slot.")),
            format!("{literal}\n\nA slot.")
        );
        // Another block, never read as text, is written as a section still.
        assert_eq!(
            markup.written("<details>\n<summary>New</summary>\n\nBody.\n\n</details>"),
            "+++ New\n\nBody.\n\n+++"
        );
    }

    #[test]
    fn a_closing_line_inside_a_fenced_body_does_not_end_the_block() {
        let authored = "<details>\n<summary>Code</summary>\n\n```text\n</details>\n+++\n```\n\n\
                        </details>\n";
        let linear = ExpandableForm::Linear.written(authored);
        assert_eq!(linear, "+++ Code\n\n```text\n</details>\n+++\n```\n\n+++\n");
        assert_eq!(ExpandableForm::Linear.read(&linear), authored);
    }
}
