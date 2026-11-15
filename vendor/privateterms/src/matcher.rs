//! The one matcher (Specification 1, *Matching*, *Replacement* and *Locators*).
//!
//! It is onevcs's matcher at v0.57.6, kept byte for byte in what it finds and exempts,
//! with spans reported in the original text and the replacement and locator outcomes
//! built on the same scan. The normalization, the word boundary, each [`TermMode`] and
//! the phrase rule are stated in the specification; the conformance suite holds this
//! implementation to them.

use std::collections::HashMap;
use std::ops::Range;

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{ReplacementCarriesTerm, Unbuildable};
use crate::locator::{Locator, LocatorKind};
use crate::normalize::{
    candidate_text_is_stable, fold, in_normalized, normalize, term_key, unfold, Contributors,
    Mapped,
};
use crate::resolve::{Action, ResolvedRule};
use crate::TermMode;

/// One non-exempt hit: the rule found, at a byte span of the text it was found in.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Hit {
    /// The index of the rule in [`Matcher::rules`].
    pub rule: usize,
    /// The UTF-8 byte span of the original (un-normalized) text the hit covers.
    pub span: Range<usize>,
}

/// What [`Matcher::replace`] made of a text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Replaced {
    /// The text with every selected hit rewritten and every other character kept.
    pub text: String,
    /// How many spans were rewritten.
    pub replaced: u32,
    /// The rules the result still carries a non-exempt hit of, ascending. Non-empty is a
    /// refusal: the result must not be published.
    pub remaining: Vec<usize>,
}

/// A set of resolved rules and public phrases, compiled once and asked of any number of
/// texts.
pub struct Matcher {
    rules: Vec<ResolvedRule>,
    /// A conservative candidate search on stable text, ignoring ASCII case.
    candidates: AhoCorasick,
    contributors: Contributors,
    /// The automaton over every rule that ignores case, matched against folded text.
    folded: Option<Automaton>,
    /// The automaton over every case-sensitive rule, matched against normalized text.
    exact: Option<Automaton>,
    /// The automaton over every public phrase, matched against folded text.
    phrases: Option<AhoCorasick>,
}

/// One automaton and, for each of its patterns, the rules that spell it.
struct Automaton {
    automaton: AhoCorasick,
    rules: Vec<Vec<usize>>,
}

/// One accepted hit, in the text it was found in.
struct Found {
    rule: usize,
    /// Whether the span is of the folded text (otherwise of the normalized text).
    in_folded: bool,
    span: (usize, usize),
    exempt: bool,
}

impl Matcher {
    /// Compile `rules`, exempting what the public `phrases` cover. A rule whose term is
    /// empty once normalized is kept in [`rules`](Self::rules) but matches nothing. A
    /// phrase that is empty once normalized is refused, naming no phrase.
    pub fn new(rules: Vec<ResolvedRule>, phrases: &[String]) -> Result<Matcher, Unbuildable> {
        let mut spellings: Vec<String> = Vec::new();
        for phrase in phrases {
            let key = term_key(phrase);
            if key.is_empty() {
                return Err(Unbuildable {
                    reason: "a public phrase is empty once normalized".to_owned(),
                });
            }
            if !spellings.contains(&key) {
                spellings.push(key);
            }
        }
        let phrases = if spellings.is_empty() {
            None
        } else {
            Some(build(&spellings)?)
        };
        let mut folded = Vec::new();
        let mut exact = Vec::new();
        for (index, resolved) in rules.iter().enumerate() {
            let normalized = normalize(&resolved.rule.term);
            if normalized.is_empty() {
                continue;
            }
            if resolved.rule.case_sensitive {
                exact.push((normalized.into_owned(), index));
            } else {
                folded.push((fold(&normalized).into_owned(), index));
            }
        }
        let candidates = AhoCorasickBuilder::new()
            .kind(Some(aho_corasick::AhoCorasickKind::DFA))
            .ascii_case_insensitive(true)
            .build(rules.iter().filter_map(|resolved| {
                let key = term_key(&resolved.rule.term);
                (!key.is_empty()).then_some(key)
            }))
            .map_err(|error| Unbuildable {
                reason: error.to_string(),
            })?;
        let contributors = Contributors::new(rules.iter().map(|r| term_key(&r.rule.term)));
        Ok(Matcher {
            contributors,
            candidates,
            folded: Automaton::build(folded)?,
            exact: Automaton::build(exact)?,
            rules,
            phrases,
        })
    }

    /// The rules this matcher was built over, in the order they were given.
    pub fn rules(&self) -> &[ResolvedRule] {
        &self.rules
    }

    /// Every rule `text` carries a non-exempt hit of, by its index in
    /// [`rules`](Self::rules), once each and ascending.
    pub fn find(&self, text: &str) -> Vec<usize> {
        let mut found: Vec<usize> = self
            .scan(text)
            .iter()
            .filter(|hit| !hit.exempt)
            .map(|hit| hit.rule)
            .collect();
        found.sort_unstable();
        found.dedup();
        found
    }

    /// Every non-exempt hit in `text`, each rule at each place once, with its span in
    /// `text` itself: ordered by start, then the longer first, then by rule.
    pub fn hits(&self, text: &str) -> Vec<Hit> {
        let found = self.scan(text);
        if !found.iter().any(|hit| !hit.exempt) {
            return Vec::new();
        }
        let mapped = Mapped::new(text);
        let mut hits: Vec<Hit> = found
            .iter()
            .filter(|hit| !hit.exempt)
            .map(|hit| {
                let span = if hit.in_folded {
                    unfold(&mapped.normalized, hit.span)
                } else {
                    hit.span
                };
                let (start, end) = mapped.original(span);
                Hit {
                    rule: hit.rule,
                    span: start..end,
                }
            })
            .collect();
        hits.sort_by(|a, b| {
            (a.span.start, std::cmp::Reverse(a.span.end), a.rule).cmp(&(
                b.span.start,
                std::cmp::Reverse(b.span.end),
                b.rule,
            ))
        });
        hits.dedup();
        hits
    }

    /// How many accepted hits in `text` a public phrase exempted: one for each rule at
    /// each place it was exempted.
    pub fn exempted(&self, text: &str) -> usize {
        if self.phrases.is_none() {
            return 0;
        }
        self.scan(text).iter().filter(|hit| hit.exempt).count()
    }

    /// Rewrite `text` (Specification 1, *Replacement*): in one pass, the leftmost-longest
    /// non-overlapping non-exempt hits whose resolved action is replace are rewritten at
    /// their spans, every other character kept; the result is then matched again, and
    /// [`Replaced::remaining`] names every rule it still carries.
    ///
    /// Where [`check_replacements`](Self::check_replacements) fails, nothing is
    /// rewritten: the text comes back unchanged with `replaced` 0, and `remaining` names
    /// the rules whose replacement carries a hit, so the outcome is still a refusal.
    pub fn replace(&self, text: &str) -> Replaced {
        if let Err(carrying) = self.check_replacements() {
            let mut remaining: Vec<usize> = self
                .rules
                .iter()
                .enumerate()
                .filter(|(_, rule)| match &rule.action {
                    Action::Replace(with) => !self.find(with).is_empty(),
                    Action::Reject => false,
                })
                .map(|(index, _)| index)
                .collect();
            if remaining.is_empty() {
                remaining.push(carrying.rule);
            }
            return Replaced {
                text: text.to_owned(),
                replaced: 0,
                remaining,
            };
        }
        let mut candidates: Vec<Hit> = self
            .hits(text)
            .into_iter()
            .filter(|hit| matches!(self.rules[hit.rule].action, Action::Replace(_)))
            .collect();
        candidates.sort_by(|a, b| {
            (a.span.start, std::cmp::Reverse(a.span.end), a.rule).cmp(&(
                b.span.start,
                std::cmp::Reverse(b.span.end),
                b.rule,
            ))
        });
        let mut out = String::with_capacity(text.len());
        let mut at = 0;
        let mut replaced = 0u32;
        for hit in candidates {
            if hit.span.start < at {
                continue;
            }
            let Action::Replace(with) = &self.rules[hit.rule].action else {
                continue;
            };
            out.push_str(&text[at..hit.span.start]);
            out.push_str(with);
            at = hit.span.end;
            replaced += 1;
        }
        out.push_str(&text[at..]);
        let remaining = self.find(&out);
        Replaced {
            text: out,
            replaced,
            remaining,
        }
    }

    /// Whether every resolved replacement string is free of a non-exempt hit; the first
    /// rule whose replacement carries one where it is not.
    pub fn check_replacements(&self) -> Result<(), ReplacementCarriesTerm> {
        for (rule, resolved) in self.rules.iter().enumerate() {
            if let Action::Replace(with) = &resolved.action {
                if !self.find(with).is_empty() {
                    return Err(ReplacementCarriesTerm { rule });
                }
            }
        }
        Ok(())
    }

    /// Screen the locators a diagnostic would print (Specification 1, *Locators*): each,
    /// in order, is [`Locator::Plain`] where it carries no non-exempt hit and
    /// [`Locator::Redacted`] where it does, the redacted ones numbered from
    /// `first_ordinal` in emission order. A locator is never rewritten.
    pub fn screen_locators(
        &self,
        locators: &[(LocatorKind, &str)],
        first_ordinal: u32,
    ) -> Vec<Locator> {
        let mut ordinal = first_ordinal;
        locators
            .iter()
            .map(|(kind, value)| {
                if self.find(value).is_empty() {
                    Locator::Plain((*value).to_owned())
                } else {
                    let redacted = Locator::Redacted {
                        kind: *kind,
                        ordinal,
                    };
                    ordinal = ordinal.saturating_add(1);
                    redacted
                }
            })
            .collect()
    }

    /// Every accepted hit in normalized or folded text, without building a span map.
    fn scan(&self, text: &str) -> Vec<Found> {
        let mut found = Vec::new();
        if self.folded.is_none() && self.exact.is_none() {
            return found;
        }
        if text.is_ascii() && !self.candidates.is_match(text) {
            return found;
        }
        if !self.contributors.possible(text) {
            return found;
        }
        if !self.candidates.is_match(text) && candidate_text_is_stable(text) {
            return found;
        }
        let normalized = normalize(text);
        let normalized: &str = &normalized;
        let lowered = (self.folded.is_some() || self.phrases.is_some()).then(|| fold(normalized));
        let (in_folded, in_text) = match &lowered {
            Some(lowered) => self.standing(normalized, lowered),
            None => (Vec::new(), Vec::new()),
        };
        if let Some(exact) = &self.exact {
            exact.collect(&self.rules, normalized, &in_text, false, &mut found);
        }
        if let (Some(folded), Some(lowered)) = (&self.folded, &lowered) {
            folded.collect(&self.rules, lowered, &in_folded, true, &mut found);
        }
        found
    }

    /// Every standing occurrence of a public phrase, as byte spans of `lowered` (the
    /// folded text) and of `normalized` (the text a case-sensitive hit is found in).
    ///
    /// An occurrence stands only where it stands in both texts, because folding can
    /// turn a letter into a letter and a mark (`İ` into `i` and U+0307). One whose edge
    /// falls inside a single character's folded form does not stand at all.
    fn standing(&self, normalized: &str, lowered: &str) -> (Spans, Spans) {
        let Some(phrases) = &self.phrases else {
            return (Vec::new(), Vec::new());
        };
        let mut found: Spans = phrases
            .find_overlapping_iter(lowered)
            .filter(|hit| stands_alone(lowered, hit.start(), hit.end()))
            .map(|hit| (hit.start(), hit.end()))
            .collect();
        if normalized.is_ascii() {
            return (found.clone(), found);
        }
        let mut in_text = Vec::new();
        found.retain(|&span| match in_normalized(normalized, span) {
            Some((start, end)) if stands_alone(normalized, start, end) => {
                in_text.push((start, end));
                true
            }
            _ => false,
        });
        (found, in_text)
    }
}

/// Byte spans of one text.
type Spans = Vec<(usize, usize)>;

fn build(spellings: &[String]) -> Result<AhoCorasick, Unbuildable> {
    AhoCorasickBuilder::new()
        .match_kind(MatchKind::Standard)
        .build(spellings)
        .map_err(|error| Unbuildable {
            reason: error.to_string(),
        })
}

impl Automaton {
    fn build(patterns: Vec<(String, usize)>) -> Result<Option<Automaton>, Unbuildable> {
        if patterns.is_empty() {
            return Ok(None);
        }
        let mut spellings: Vec<String> = Vec::new();
        let mut rules: Vec<Vec<usize>> = Vec::new();
        let mut known: HashMap<String, usize> = HashMap::new();
        for (spelling, rule) in patterns {
            match known.get(&spelling) {
                Some(&at) => rules[at].push(rule),
                None => {
                    known.insert(spelling.clone(), spellings.len());
                    spellings.push(spelling);
                    rules.push(vec![rule]);
                }
            }
        }
        Ok(Some(Automaton {
            automaton: build(&spellings)?,
            rules,
        }))
    }

    /// Every accepted hit this automaton finds in `text`, where `phrases` are the
    /// standing public phrase occurrences located in that same text.
    fn collect(
        &self,
        rules: &[ResolvedRule],
        text: &str,
        phrases: &[(usize, usize)],
        in_folded: bool,
        found: &mut Vec<Found>,
    ) {
        for hit in self.automaton.find_overlapping_iter(text) {
            let (start, end) = (hit.start(), hit.end());
            for &rule in &self.rules[hit.pattern().as_usize()] {
                if !accepts(rules[rule].rule.mode, text, start, end) {
                    continue;
                }
                found.push(Found {
                    rule,
                    in_folded,
                    span: (start, end),
                    exempt: phrases.iter().any(|&(ps, pe)| ps <= start && end <= pe),
                });
            }
        }
    }
}

/// Whether a phrase occurrence at `start..end` of `text` stands on its own.
fn stands_alone(text: &str, start: usize, end: usize) -> bool {
    let mut before = text[..start].chars().rev();
    let clear_before = match before.next() {
        None => true,
        Some('.') => !before.next().is_some_and(is_word),
        Some(c) => !(is_word(c) || c == '-'),
    };
    let mut after = text[end..].chars();
    let clear_after = match after.next() {
        None => true,
        Some('.') => !after.next().is_some_and(is_word),
        Some(c) => !(is_word(c) || c == '-'),
    };
    clear_before && clear_after
}

/// Whether a hit at `start..end` of `text` is one `mode` accepts.
fn accepts(mode: TermMode, text: &str, start: usize, end: usize) -> bool {
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();
    match mode {
        TermMode::Substring => true,
        TermMode::WholeWord => !before.is_some_and(is_word) && !after.is_some_and(is_word),
        TermMode::OwnerNameOnly => {
            !before.is_some_and(continues_a_name)
                && match after {
                    None => true,
                    Some('.') => {
                        let rest = &text[end + 1..];
                        match rest.strip_prefix("git") {
                            Some(tail) => !tail.chars().next().is_some_and(continues_a_name),
                            None => !rest.chars().next().is_some_and(is_word),
                        }
                    }
                    Some(c) => !continues_a_name(c),
                }
        }
    }
}

/// A word character: a Unicode letter or digit, or `_`.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// A character an owner or a repository name continues with.
fn continues_a_name(c: char) -> bool {
    is_word(c) || c == '-' || c == '.'
}

/// Redact every locator (Specification 1, *Locators*, the unavailable outcome): each
/// becomes [`Locator::Redacted`], numbered from `first_ordinal` in order.
pub fn redact_all(locators: &[(LocatorKind, &str)], first_ordinal: u32) -> Vec<Locator> {
    let mut ordinal = first_ordinal;
    locators
        .iter()
        .map(|(kind, _)| {
            let redacted = Locator::Redacted {
                kind: *kind,
                ordinal,
            };
            ordinal = ordinal.saturating_add(1);
            redacted
        })
        .collect()
}
