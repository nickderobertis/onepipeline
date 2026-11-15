//! The Rust SDK of the private-terms specification.
//!
//! [Specification 1](https://github.com/nickderobertis/privateterms/blob/main/spec/private-terms.md)
//! defines a storage-independent data model — declarations of terms, exceptions,
//! replacements and public phrases, layered nearest first — what matching a text against
//! it means, and its outcomes: detect, replace, reject, and check unavailable. This crate
//! implements it, and the repository's language-neutral conformance suite holds it to it.
//!
//! # The model and its outcomes
//!
//! - [`Declaration`] and its members, validated wherever one is read;
//! - [`resolve`] folds a repository's own rules, the [`Level`]s and a caller's
//!   [`Replacement`] list into [`ResolvedRule`]s, each with its [`Action`];
//! - [`Matcher`] finds ([`Matcher::find`], [`Matcher::hits`]), replaces
//!   ([`Matcher::replace`]) and screens [`Locator`]s ([`Matcher::screen_locators`]);
//!   [`redact_all`] is the locator outcome of an unavailable check.
//!
//! ```
//! use privateterms::{resolve, Action, Matcher, Replacement, TermMode, TermRule};
//!
//! let derived = vec![(
//!     TermRule { term: "quillmoss".into(), mode: TermMode::WholeWord, case_sensitive: false },
//!     "derived".to_owned(),
//! )];
//! let caller = [Replacement::Replace { term: "quillmoss".into(), with: "example".into() }];
//! let rules = resolve(derived, &[], &caller).unwrap();
//! assert_eq!(rules[0].action, Action::Replace("example".into()));
//!
//! let matcher = Matcher::new(rules, &[]).unwrap();
//! let replaced = matcher.replace("see QUILLMOSS-core");
//! assert_eq!(replaced.text, "see example-core");
//! assert!(replaced.remaining.is_empty());
//! ```
//!
//! # The crate's storage bindings
//!
//! The specification defines no file name, format or directory layout. This crate binds
//! the model to disk the way onevcs and onetaskgraph consume it, and these bindings are
//! **the crate's, not the specification's**:
//!
//! - a declaration file is one of [`FILE_NAMES`] — `private-terms.toml`,
//!   `private-terms.yaml` or `private-terms.yml` — each one encoding ([`Format`]) of the
//!   model, read by [`parse`] and written by [`to_toml`] and [`to_yaml`];
//! - two declaration files in one directory are refused as ambiguous ([`read_dir_level`]);
//! - [`discover_levels`] finds a checkout's levels: the repository's own file at its root,
//!   each ancestor directory up to and including the home directory (for a checkout
//!   outside it, up to but excluding the filesystem root), then a fallback directory a
//!   host names, nearest first.
//!
//! YAML is read and written only through `serde-saphyr`, every read under one hardened,
//! bounded set of reader options.
//!
//! # Errors
//!
//! No error this crate produces names a term, a phrase or any declaration content, and a
//! [`DeclarationError`] keeps the file it is about in its own `path` field, which neither
//! its `Display` nor its `Debug` rendering includes.

mod encoding;
mod error;
mod levels;
mod locator;
mod matcher;
mod model;
mod normalize;
mod resolve;
mod yaml;

pub use encoding::{parse, to_toml, to_yaml, Format};
pub use error::{
    DeclarationError, DeclarationErrorKind, Member, ReplacementCarriesTerm, Unbuildable,
};
pub use levels::{discover_levels, read_dir_level, Level, LevelKind, FILE_NAMES};
pub use locator::{Locator, LocatorKind, LOCATOR_UNFIXABLE_REASON};
pub use matcher::{redact_all, Hit, Matcher, Replaced};
pub use model::{Declaration, Exception, ExceptionAction, Replacement};
pub use resolve::{resolve, Action, ResolvedRule, TermMode, TermRule};

/// The README's examples, run as doctests so the documented bindings cannot drift.
#[doc = include_str!("../README.md")]
#[cfg(doctest)]
pub struct ReadmeDoctests;

/// The version of the specification this crate implements.
pub const SPEC_VERSION: u32 = 1;
