//! Resolution (Specification 1, *Layered declarations and precedence*): the rules a check
//! matches, each with the action its hits get.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{DeclarationError, DeclarationErrorKind as Kind, Member};
use crate::levels::{Level, LevelKind};
use crate::model::{self, ExceptionAction, Replacement};
use crate::normalize::{fold, normalize, term_key};

/// How a rule's hits are matched.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum TermMode {
    /// Anywhere.
    Substring,
    /// With no word character immediately before or after.
    WholeWord,
    /// A qualified `owner/name`, only where it stands as that name.
    OwnerNameOnly,
}

/// One term, and how it is matched.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
pub struct TermRule {
    /// The term as written.
    pub term: String,
    /// How it is matched.
    pub mode: TermMode,
    /// Whether it is compared exactly rather than in lower case.
    pub case_sensitive: bool,
}

/// What a check does with a rule's hits.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    /// Refuse a text carrying it.
    Reject,
    /// Rewrite its hits to this string.
    Replace(String),
}

/// A rule, the action resolved for it, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ResolvedRule {
    /// The rule.
    pub rule: TermRule,
    /// What its hits get.
    pub action: Action,
    /// Where the rule came from: the origin a caller gave it, or the kind of the level
    /// (`repository`, `directory`, `fallback`) or `caller` that added it — a fixed word,
    /// never a path.
    pub origin: String,
}

/// The origin of a rule a caller's replacement list added.
const CALLER: &str = "caller";

/// Resolve the rules a check matches (Specification 1, *Layered declarations and
/// precedence*).
///
/// `rules` are the repository's own derived rules with their origins, before any
/// exception; `levels` are nearest first; `caller` is the caller's ordered replacement
/// list. Every level is validated for its kind first, and an invalid one is the error.
/// Then:
///
/// 1. the repository's own rules are `rules` and the `terms` of every
///    [`LevelKind::Repository`] level; each exception of those levels narrows the ones
///    carrying its term (drop removes them; whole-word makes a substring rule whole-word;
///    case-sensitive makes them case-sensitive, spelled as the exception spells it,
///    except an owner-name-only rule's spelling; owner-name-only removes them, leaving the
///    term found only through the derivation's qualified `owner/name` rule), and an
///    exception naming none is [`ExceptionNamesNoTerm`](Kind::ExceptionNamesNoTerm);
/// 2. every other level's `terms`, and every replacement entry (any level's, or the
///    caller's) naming a term no rule carries, add a whole-word, not case-sensitive rule
///    — which no exception reaches, so no level removes a rule a farther level carries;
/// 3. each rule's action, per term (normalized and folded): the caller's entries, a later
///    entry winning; then the levels nearest first, where the nearest distance naming the
///    term decides reject or replace and the replacement — levels sharing a `dir` are one
///    distance, and two of them resolving the term differently reject it; then reject.
///
/// Identical rules (one term once normalized — and folded unless case-sensitive — with
/// one mode and case rule) are kept once, the first.
pub fn resolve(
    rules: Vec<(TermRule, String)>,
    levels: &[Level],
    caller: &[Replacement],
) -> Result<Vec<ResolvedRule>, DeclarationError> {
    for level in levels {
        model::for_level(&level.declaration, level.kind)?;
    }
    let mut own: Vec<(TermRule, String)> = rules;
    let repository = || {
        levels
            .iter()
            .filter(|level| level.kind == LevelKind::Repository)
    };
    for level in repository() {
        for term in &level.declaration.terms {
            own.push((whole_word(term), LevelKind::Repository.name().to_owned()));
        }
    }
    for level in repository() {
        for (index, exception) in level.declaration.exceptions.iter().enumerate() {
            let key = term_key(&exception.term);
            let mut named = false;
            own.retain_mut(|(rule, _)| {
                if term_key(&rule.term) != key {
                    return true;
                }
                named = true;
                match exception.action {
                    ExceptionAction::Drop | ExceptionAction::OwnerNameOnly => false,
                    ExceptionAction::WholeWord => {
                        if rule.mode == TermMode::Substring {
                            rule.mode = TermMode::WholeWord;
                        }
                        true
                    }
                    ExceptionAction::CaseSensitive => {
                        if rule.mode != TermMode::OwnerNameOnly {
                            rule.term = exception.term.clone();
                        }
                        rule.case_sensitive = true;
                        true
                    }
                }
            });
            if !named {
                return Err(DeclarationError::at(
                    Kind::ExceptionNamesNoTerm,
                    Member::Exceptions,
                    index,
                ));
            }
        }
    }
    let mut all = own;
    for level in levels
        .iter()
        .filter(|level| level.kind != LevelKind::Repository)
    {
        for term in &level.declaration.terms {
            all.push((whole_word(term), level.kind.name().to_owned()));
        }
    }
    let entries = levels
        .iter()
        .flat_map(|level| {
            level
                .declaration
                .replacements
                .iter()
                .map(move |entry| (entry, level.kind.name()))
        })
        .chain(caller.iter().map(|entry| (entry, CALLER)));
    let mut added = Vec::new();
    for (entry, origin) in entries {
        let key = term_key(entry.term());
        let carried = all
            .iter()
            .chain(added.iter())
            .any(|(rule, _): &(TermRule, String)| term_key(&rule.term) == key);
        if !carried {
            added.push((whole_word(entry.term()), origin.to_owned()));
        }
    }
    all.extend(added);

    let mut resolved: Vec<ResolvedRule> = Vec::new();
    let mut identities: Vec<(TermMode, bool, String)> = Vec::new();
    for (rule, origin) in all {
        let identity = (rule.mode, rule.case_sensitive, identity_key(&rule));
        if identities.contains(&identity) {
            continue;
        }
        identities.push(identity);
        let action = action_for(&term_key(&rule.term), levels, caller);
        resolved.push(ResolvedRule {
            rule,
            action,
            origin,
        });
    }
    Ok(resolved)
}

fn whole_word(term: &str) -> TermRule {
    TermRule {
        term: term.to_owned(),
        mode: TermMode::WholeWord,
        case_sensitive: false,
    }
}

fn identity_key(rule: &TermRule) -> String {
    let normalized = normalize(&rule.term);
    if rule.case_sensitive {
        normalized.into_owned()
    } else {
        fold(&normalized).into_owned()
    }
}

fn entry_action(entry: &Replacement) -> Action {
    match entry {
        Replacement::Replace { with, .. } => Action::Replace(with.clone()),
        Replacement::Reject { .. } => Action::Reject,
    }
}

fn action_for(key: &str, levels: &[Level], caller: &[Replacement]) -> Action {
    if let Some(entry) = caller
        .iter()
        .rev()
        .find(|entry| term_key(entry.term()) == key)
    {
        return entry_action(entry);
    }
    let mut index = 0;
    while index < levels.len() {
        let dir = &levels[index].dir;
        let mut end = index;
        while end < levels.len() && &levels[end].dir == dir {
            end += 1;
        }
        let named: Vec<Action> = levels[index..end]
            .iter()
            .flat_map(|level| level.declaration.replacements.iter())
            .filter(|entry| term_key(entry.term()) == key)
            .map(entry_action)
            .collect();
        if let Some(first) = named.first() {
            return if named.iter().all(|action| action == first) {
                first.clone()
            } else {
                Action::Reject
            };
        }
        index = end;
    }
    Action::Reject
}
