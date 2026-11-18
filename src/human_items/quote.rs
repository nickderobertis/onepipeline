//! `${steps.<node>.<output>}`: a human node's item quoting what a step before it
//! published.
//!
//! A `kind: command` step publishes outputs by writing a JSON object of strings to
//! the file `ONEPIPELINE_STEP_OUTPUTS` names, and its `command-exited` record carries
//! them. A human node's `human_item` title, body and choice descriptions may quote
//! one, and the engine substitutes it when it raises the item. The syntax is not
//! Jinja's `{{ }}` on purpose: a plan template that renders node metadata is itself
//! Jinja, and would render a placeholder away before the engine saw it.
//!
//! The node a placeholder names must be one of the human node's deps, which the plan
//! loader holds. An output the step never published is only known once the step has
//! run, so the engine then raises no item rather than one quoting nothing, and
//! surfaces why to the planner instead.

use std::collections::BTreeMap;
use std::fmt;

/// What opens a placeholder.
const OPEN: &str = "${steps.";

/// One placeholder: the step it quotes and the output of that step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Quoted<'a> {
    /// The node id of the step.
    pub(crate) step: &'a str,
    /// The output's name.
    pub(crate) output: &'a str,
}

impl fmt::Display for Quoted<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{OPEN}{}.{}}}", self.step, self.output)
    }
}

/// Whether `name` can be an output's name in a placeholder: letters, digits, `_`
/// and `-`. The step is what precedes the last `.`, so a name holds none.
fn is_output_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Each placeholder in `text`, with where it sits, in order.
fn spans(text: &str) -> Result<Vec<(std::ops::Range<usize>, Quoted<'_>)>, String> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find(OPEN).map(|at| from + at) {
        let inner_at = at + OPEN.len();
        let Some(close) = text[inner_at..].find('}').map(|close| inner_at + close) else {
            return Err(format!(
                "a `{OPEN}` placeholder is never closed with `}}`; a placeholder is \
                 `{OPEN}<node>.<output>}}`"
            ));
        };
        let inner = &text[inner_at..close];
        let quoted = inner
            .rsplit_once('.')
            .filter(|(step, output)| !step.trim().is_empty() && is_output_name(output))
            .map(|(step, output)| Quoted { step, output })
            .ok_or_else(|| {
                format!(
                    "`{OPEN}{inner}}}` is not a placeholder: one is `{OPEN}<node>.<output>}}`, \
                     the output's name letters, digits, `_` and `-`"
                )
            })?;
        found.push((at..close + 1, quoted));
        from = close + 1;
    }
    Ok(found)
}

/// Each placeholder `text` holds.
///
/// # Errors
///
/// A placeholder never closed, or one that names no step or no well-formed output.
pub(crate) fn quoted(text: &str) -> Result<Vec<Quoted<'_>>, String> {
    Ok(spans(text)?.into_iter().map(|(_, quoted)| quoted).collect())
}

/// `text` with each placeholder replaced by the output it quotes, from `published`:
/// each step's outputs, by node id.
///
/// # Errors
///
/// The first placeholder quoting an output its step did not publish, named in words
/// a surface can carry; and a malformed one, which the loader refuses first.
pub(crate) fn substitute(
    text: &str,
    published: &BTreeMap<String, BTreeMap<String, String>>,
) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut from = 0;
    for (span, quoted) in spans(text)? {
        let value = published
            .get(quoted.step)
            .and_then(|outputs| outputs.get(quoted.output))
            .ok_or_else(|| {
                format!(
                    "`{quoted}` quotes output '{}' of step '{}', which that step did not \
                     publish",
                    quoted.output, quoted.step
                )
            })?;
        out.push_str(&text[from..span.start]);
        out.push_str(value);
        from = span.end;
    }
    out.push_str(&text[from..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn published() -> BTreeMap<String, BTreeMap<String, String>> {
        BTreeMap::from([(
            "copy.doc".to_owned(),
            BTreeMap::from([("url".to_owned(), "https://example.invalid/d".to_owned())]),
        )])
    }

    #[test]
    fn a_placeholder_names_the_step_before_its_last_dot_and_substitutes_its_output() {
        let text = "Read ${steps.copy.doc.url} — then ${x} and {{ jinja }} stay as written.";
        assert_eq!(
            quoted(text).unwrap(),
            [Quoted {
                step: "copy.doc",
                output: "url"
            }]
        );
        assert_eq!(
            substitute(text, &published()).unwrap(),
            "Read https://example.invalid/d — then ${x} and {{ jinja }} stay as written."
        );
    }

    #[test]
    fn a_malformed_or_unpublished_placeholder_is_named() {
        assert!(quoted("${steps.open").unwrap_err().contains("never closed"));
        assert!(quoted("${steps.nodot}")
            .unwrap_err()
            .contains("not a placeholder"));
        assert!(quoted("${steps.a.b c}")
            .unwrap_err()
            .contains("not a placeholder"));
        let missing = substitute("${steps.copy.doc.title}", &published()).unwrap_err();
        assert!(
            missing.contains("output 'title' of step 'copy.doc'"),
            "{missing}"
        );
    }
}
