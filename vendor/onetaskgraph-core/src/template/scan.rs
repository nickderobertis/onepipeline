// llmlint: ignore-file[code_lands_in_the_domain_that_owns_it] A part of the `template` module; why that module sits in this crate is stated once, at the head of its `mod.rs`.
//! The templates one template names, read off its source before anything renders.
//!
//! minijinja resolves `extends`, `include` and `import` lazily, as the render reaches them,
//! and the declared set has to be known before that: it is what `template variables` reports
//! and what a prompt asks for. So the chain is read here, statically — every tag naming a
//! template by a string literal, or by a list of them, whichever branch it sits in. A tag
//! naming one by an expression cannot be read before rendering, so it is found here by its
//! place in the file instead, and [`hooked`] marks its expression so that a render reports
//! what it named and where — which is how such a file joins the chain at the tag that names
//! it, as [`super::Template::expand`] says.

use std::ops::Range;

/// The function [`hooked`] wraps each naming expression in. It returns its last argument
/// unchanged, so a hooked template renders exactly as the one it was made from.
pub(super) const HOOK: &str = "__onetaskgraph_named";

/// One template a tag names: one name, or a list of candidates of which the first that
/// resolves is the one loaded — minijinja's own rule for `{% include [a, b] %}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Reference {
    /// The names as written, in the order they are tried.
    pub candidates: Vec<String>,
    /// Written with `ignore missing`, so candidates nothing resolves are not an error.
    pub optional: bool,
}

/// One tag of a file that names a template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Named {
    /// Named by string literals, known before rendering.
    Literal(Reference),
    /// Named by an expression: the file's `ordinal`-th such tag, counting from zero, whose
    /// expression is `span` of the source.
    Expression { ordinal: usize, span: Range<usize> },
}

/// Every tag of `source` that names a template, in the order they are written.
pub(super) fn named(source: &str) -> Vec<Named> {
    let mut found = Vec::new();
    let mut ordinal = 0;
    let mut rest = source;
    while let Some(start) = rest.find('{') {
        let after = &rest[start..];
        if let Some(inner) = after.strip_prefix("{#") {
            rest = skip_past(inner, "#}");
        } else if let Some(inner) = after.strip_prefix("{{") {
            rest = skip_past_quoted(inner, "}}");
        } else if let Some(inner) = after.strip_prefix("{%") {
            let (tag, remaining) = tag_body(inner);
            rest = remaining;
            let tag = tag
                .trim_start_matches(['-', '+'])
                .trim_end_matches(['-', '+']);
            let (word, arguments) = split_word(tag);
            match word {
                "raw" => rest = skip_raw(rest),
                "extends" | "import" | "from" | "include" => {
                    if let Some((candidates, after)) = literal_names(arguments)
                        && let Some(optional) = trailer(word, after)
                    {
                        found.push(Named::Literal(Reference {
                            candidates,
                            optional,
                        }));
                    } else if let Some(expression) = expression(word, arguments) {
                        let at = offset(source, expression);
                        found.push(Named::Expression {
                            ordinal,
                            span: at..at + expression.len(),
                        });
                        ordinal += 1;
                    }
                }
                _ => {}
            }
        } else {
            rest = &after[1..];
        }
    }
    found
}

/// `source` with each naming expression wrapped in [`HOOK`], called with `file`, the tag's
/// ordinal and the expression's value — or `source` itself when it names nothing by an
/// expression.
pub(super) fn hooked(source: &str, file: usize) -> String {
    let mut out = String::with_capacity(source.len());
    let mut copied = 0;
    for named in named(source) {
        if let Named::Expression { ordinal, span } = named {
            out.push_str(&source[copied..span.start]);
            out.push_str(&format!(
                "{HOOK}({file}, {ordinal}, ({}))",
                &source[span.clone()]
            ));
            copied = span.end;
        }
    }
    out.push_str(&source[copied..]);
    out
}

fn offset(whole: &str, part: &str) -> usize {
    part.as_ptr().addr() - whole.as_ptr().addr()
}

/// The expression a tag names its template by: its arguments less what the tag may carry
/// after one, as minijinja's parser reads them — or `None` when nothing is left.
fn expression<'a>(tag: &str, arguments: &'a str) -> Option<&'a str> {
    let mut expression = arguments.trim_end();
    match tag {
        "include" => {
            // `EXPR [with|without context] [ignore missing [with|without context]]`
            expression = context_marker(expression);
            if let Some(before) =
                strip_word(expression, "missing").and_then(|before| strip_word(before, "ignore"))
            {
                expression = context_marker(before);
            }
        }
        "import" => {
            // `EXPR as NAME [with|without context]`
            let (before, alias) = last_word(context_marker(expression));
            expression = strip_word(before, "as").filter(|_| is_identifier(alias))?;
        }
        "from" => {
            // `EXPR import NAMES`
            expression = &expression[..keyword(expression, "import")?];
        }
        _ => {}
    }
    let expression = expression.trim();
    (!expression.is_empty()).then_some(expression)
}

/// `text` less a trailing `with context` or `without context`.
fn context_marker(text: &str) -> &str {
    strip_word(text, "context")
        .and_then(|before| strip_word(before, "with").or_else(|| strip_word(before, "without")))
        .unwrap_or(text)
}

/// `text` less a trailing `word` standing on its own, or `None` when it does not end so.
fn strip_word<'a>(text: &'a str, word: &str) -> Option<&'a str> {
    let before = text.trim_end().strip_suffix(word)?;
    (!before.ends_with(is_identifier_char)).then_some(before.trim_end())
}

fn last_word(text: &str) -> (&str, &str) {
    let text = text.trim_end();
    text.rfind(char::is_whitespace)
        .map_or(("", text), |at| (&text[..at], &text[at + 1..]))
}

/// Where the first `word` standing on its own outside a string literal starts in `text`.
fn keyword(text: &str, word: &str) -> Option<usize> {
    let mut searched = 0;
    loop {
        let (before, after) = until_unquoted(&text[searched..], word);
        if after.is_empty() && before.len() == text.len() - searched {
            return None;
        }
        let at = searched + before.len();
        searched = at + word.len();
        let standalone = !text[..at].ends_with(is_identifier_char)
            && !text[searched..].starts_with(is_identifier_char);
        if standalone {
            return Some(at);
        }
    }
}

fn is_identifier(text: &str) -> bool {
    !text.is_empty()
        && !text.starts_with(|c: char| c.is_ascii_digit())
        && text.chars().all(is_identifier_char)
}

fn is_identifier_char(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// What follows the first `end` in `text`, or nothing when there is none.
fn skip_past<'a>(text: &'a str, end: &str) -> &'a str {
    text.find(end).map_or("", |at| &text[at + end.len()..])
}

/// What follows the first `end` outside a string literal.
fn skip_past_quoted<'a>(text: &'a str, end: &str) -> &'a str {
    let (_, rest) = until_unquoted(text, end);
    rest
}

/// A tag's body up to its closing `%}`, and what follows it.
fn tag_body(text: &str) -> (&str, &str) {
    until_unquoted(text, "%}")
}

/// Split `text` at the first `end` outside a string literal.
fn until_unquoted<'a>(text: &'a str, end: &str) -> (&'a str, &'a str) {
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in text.char_indices() {
        match quote {
            Some(open) => {
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == open {
                    quote = None;
                }
            }
            None => {
                if character == '"' || character == '\'' {
                    quote = Some(character);
                } else if text[index..].starts_with(end) {
                    return (&text[..index], &text[index + end.len()..]);
                }
            }
        }
    }
    (text, "")
}

/// Skip a `{% raw %}` block to just past its `{% endraw %}`.
fn skip_raw(text: &str) -> &str {
    let mut rest = text;
    while let Some(start) = rest.find("{%") {
        let (tag, remaining) = tag_body(&rest[start + 2..]);
        let tag = tag
            .trim_start_matches(['-', '+'])
            .trim_end_matches(['-', '+']);
        if split_word(tag).0 == "endraw" {
            return remaining;
        }
        rest = remaining;
    }
    ""
}

/// A tag's first word and the rest of it.
fn split_word(tag: &str) -> (&str, &str) {
    let tag = tag.trim();
    match tag.find(char::is_whitespace) {
        Some(at) => (&tag[..at], tag[at..].trim_start()),
        None => (tag, ""),
    }
}

/// The names an argument opens with, as a string literal or a list of them, and what follows
/// them — or `None` when it opens with anything else.
fn literal_names(arguments: &str) -> Option<(Vec<String>, &str)> {
    let arguments = arguments.trim_start();
    let Some(list) = arguments.strip_prefix('[') else {
        return string_literal(arguments).map(|(name, after)| (vec![name], after));
    };
    let mut names = Vec::new();
    let mut rest = list;
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix(']') {
            return Some((names, after));
        }
        let (name, after) = string_literal(rest)?;
        names.push(name);
        rest = after.trim_start();
        if let Some(after) = rest.strip_prefix(',') {
            rest = after;
        } else if !rest.starts_with(']') {
            return None;
        }
    }
}

/// Whether what follows a tag's names is exactly what that tag may carry after a name — and
/// so the names are the whole of it — answering whether it made the reference optional.
///
/// Anything else means the names were only the start of an expression — `"a.md" if x else
/// "b.md"`, `"part-" ~ kind` — which names no template before rendering.
fn trailer(tag: &str, after: &str) -> Option<bool> {
    let words: Vec<&str> = after.split_whitespace().collect();
    match (tag, words.as_slice()) {
        ("extends", []) => Some(false),
        ("import", ["as", _alias] | ["as", _alias, "with" | "without", "context"]) => Some(false),
        ("from", ["import", ..]) => Some(false),
        ("include", rest) => {
            let rest = match rest {
                ["with" | "without", "context", rest @ ..] => rest,
                rest => rest,
            };
            let (optional, rest) = match rest {
                ["ignore", "missing", rest @ ..] => (true, rest),
                rest => (false, rest),
            };
            matches!(rest, [] | ["with" | "without", "context"]).then_some(optional)
        }
        _ => None,
    }
}

/// One string literal at the start of `text`, unescaped, and what follows it.
///
/// This finds only where the literal ends; what it spells is minijinja's own reading of it,
/// so a name this scan reads and the name the render asks for are one decoding, whatever
/// escapes it uses.
fn string_literal(text: &str) -> Option<(String, &str)> {
    let mut characters = text.char_indices();
    let (_, open) = characters.next()?;
    if open != '"' && open != '\'' {
        return None;
    }
    let mut escaped = false;
    for (index, character) in characters {
        if escaped {
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == open {
            let literal = &text[..=index];
            let value = minijinja::Environment::new()
                .compile_expression(literal)
                .and_then(|expression| expression.eval(()))
                .ok()?;
            return value
                .as_str()
                .map(|name| (name.to_owned(), &text[index + 1..]));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(source: &str) -> Vec<(Vec<String>, bool)> {
        named(source)
            .into_iter()
            .filter_map(|named| match named {
                Named::Literal(reference) => Some((reference.candidates, reference.optional)),
                Named::Expression { .. } => None,
            })
            .collect()
    }

    fn expressions(source: &str) -> Vec<(usize, &str)> {
        named(source)
            .into_iter()
            .filter_map(|named| match named {
                Named::Literal(_) => None,
                Named::Expression { ordinal, span } => Some((ordinal, &source[span])),
            })
            .collect()
    }

    fn one(name: &str) -> (Vec<String>, bool) {
        (vec![name.to_owned()], false)
    }

    #[test]
    fn every_kind_of_tag_is_read_in_the_order_it_is_written() {
        let source = "{% extends \"base.md\" %}\n\
                      {%- include 'part.md' -%}\n\
                      {% import \"macros.md\" as m %}\n\
                      {% from 'helpers.md' import one, two %}\n\
                      {% include ['a.md', \"b.md\"] ignore missing %}\n\
                      {% import 'ctx.md' as c with context %}";
        assert_eq!(
            names(source),
            [
                one("base.md"),
                one("part.md"),
                one("macros.md"),
                one("helpers.md"),
                (vec!["a.md".to_owned(), "b.md".to_owned()], true),
                one("ctx.md"),
            ]
        );
    }

    #[test]
    fn only_a_trailer_outside_the_name_makes_an_include_optional() {
        let source = "{% include 'ignore missing.md' %}\n\
                      {% include 'p.md' ignore missing with context %}\n\
                      {% include 'q.md' without context %}\n\
                      {% include 'r.md' with context ignore missing %}";
        assert_eq!(
            names(source),
            [
                one("ignore missing.md"),
                (vec!["p.md".to_owned()], true),
                one("q.md"),
                (vec!["r.md".to_owned()], true),
            ]
        );
    }

    #[test]
    fn comments_raw_blocks_expressions_and_strings_name_nothing() {
        let source = "{# {% include 'commented.md' %} #}\n\
                      {% raw %}{% include 'raw.md' %}{% endraw %}\n\
                      {{ '{% include \"quoted.md\" %}' }}\n\
                      {% include kind ~ '.md' %}\n\
                      {% include 'prefix-' ~ kind %}\n\
                      {% if x %}{% include 'branch.md' %}{% endif %}\n\
                      {% include 'a.md' if x else 'b.md' %}\n\
                      {% extends 'base-' ~ kind ~ '.md' %}";
        assert_eq!(names(source), [one("branch.md")]);
    }

    #[test]
    fn an_expression_tag_is_found_by_its_place_less_what_the_tag_carries_after_it() {
        let source = "{% extends 'base-' ~ kind ~ '.md' %}\n\
                      {% include 'x' ~ kind %}{% include 'literal.md' %}\n\
                      {%- include parts ignore missing with context -%}\n\
                      {% include name_with without context %}\n\
                      {% import kind ~ '.md' as m with context %}\n\
                      {% from 'important-' ~ kind import one as two %}\n\
                      {% include 'a.md' if x else 'b.md' %}";
        assert_eq!(
            expressions(source),
            [
                (0, "'base-' ~ kind ~ '.md'"),
                (1, "'x' ~ kind"),
                (2, "parts"),
                (3, "name_with"),
                (4, "kind ~ '.md'"),
                (5, "'important-' ~ kind"),
                (6, "'a.md' if x else 'b.md'"),
            ]
        );
    }

    #[test]
    fn a_hooked_template_wraps_each_expression_and_nothing_else() {
        let source = "{% include 'literal.md' %}{% include kind ~ '.md' ignore missing %}";
        assert_eq!(
            hooked(source, 3),
            format!(
                "{{% include 'literal.md' %}}{{% include {HOOK}(3, 0, (kind ~ '.md')) ignore missing %}}"
            )
        );
        assert_eq!(hooked("{{ x }}", 0), "{{ x }}");
    }

    #[test]
    fn every_form_of_a_naming_tag_still_parses_and_renders_the_same_once_hooked() {
        let source = "{% if false %}{% extends 'base-' ~ kind %}{% endif %}\
                      {%- include 'x' ~ kind -%}\
                      {% include parts ignore missing with context %}\
                      {% include parts with context ignore missing %}\
                      {% include name_with without context %}\
                      {% import 'x' ~ kind as m with context %}{{ m.a() }}\
                      {% from 'x' ~ kind import a as b %}{{ b() }}\
                      {% include 'x.md' if kind else 'y.md' %}";
        let render = |source: &str| {
            let mut environment = minijinja::Environment::new();
            environment.add_function(HOOK, |_: usize, _: usize, value: minijinja::Value| value);
            environment.set_loader(|name| {
                Ok((name == "x.md").then(|| "{% macro a() %}A{% endmacro %}".to_owned()))
            });
            environment
                .render_str(source, minijinja::context! {kind => ".md", parts => ["none.md", "x.md"], name_with => "x.md"})
                .expect("it renders")
        };
        let hooked = hooked(source, 0);
        assert_eq!(hooked.matches(HOOK).count(), 8, "{hooked}");
        assert_eq!(render(&hooked), render(source));
    }
}
