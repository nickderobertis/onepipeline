//! Levels (Specification 1, *Layered declarations and precedence*), and the crate's
//! storage binding of them to directories on disk.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::encoding::{parse, Format};
use crate::error::{DeclarationError, DeclarationErrorKind as Kind};
use crate::model::Declaration;

/// The file names a directory's declaration may have, in the order they are looked for.
///
/// **A binding of this crate, not of the specification**, which names no file. Each is
/// one encoding of the model ([`Format`]): `private-terms.toml` is TOML, and
/// `private-terms.yaml` and `private-terms.yml` are YAML. A directory holding more than
/// one of them is refused as ambiguous ([`read_dir_level`]).
pub const FILE_NAMES: [&str; 3] = [
    "private-terms.toml",
    "private-terms.yaml",
    "private-terms.yml",
];

/// What a level is: a repository's own, a directory's, or a host's fallback.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum LevelKind {
    /// A repository's own declaration, the only one that may declare exceptions.
    Repository,
    /// A directory level between a repository and the home directory.
    Directory,
    /// The fallback a host names, farthest of all.
    Fallback,
}

impl LevelKind {
    /// The kind's kebab-case name: a fixed word, never a locator.
    pub fn name(self) -> &'static str {
        match self {
            Self::Repository => "repository",
            Self::Directory => "directory",
            Self::Fallback => "fallback",
        }
    }
}

/// One level: its kind, the place it was read from, and its declaration. Levels sharing a
/// `dir` are at one distance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Level {
    /// What kind of level it is.
    pub kind: LevelKind,
    /// Where it was read from. A path is a locator: print it only screened.
    pub dir: PathBuf,
    /// What it declares.
    pub declaration: Declaration,
}

fn format_of(name: &str) -> Format {
    if name.ends_with(".toml") {
        Format::Toml
    } else {
        Format::Yaml
    }
}

/// Read the declaration `dir` holds, as a level of `kind`: `None` where it holds none.
///
/// **A binding of this crate, not of the specification.** The file is whichever of
/// [`FILE_NAMES`] exists in `dir`; two or more of them is
/// [`AmbiguousFiles`](Kind::AmbiguousFiles) (the error's `path` is `dir`), a name that is
/// not a regular file or cannot be read is [`Unreadable`](Kind::Unreadable), and the
/// file's content is read by [`parse`]. An error's `path` is the file it is about.
pub fn read_dir_level(dir: &Path, kind: LevelKind) -> Result<Option<Level>, DeclarationError> {
    let mut present = Vec::new();
    for name in FILE_NAMES {
        let path = dir.join(name);
        match std::fs::metadata(&path) {
            Ok(_) => present.push((name, path)),
            Err(failure) if failure.kind() == ErrorKind::NotFound => {}
            // A directory that cannot be listed (or a dangling name it cannot follow) is
            // not a directory without a declaration.
            Err(_) => return Err(DeclarationError::new(Kind::Unreadable).with_path(path)),
        }
    }
    let (name, path) = match present.len() {
        0 => return Ok(None),
        1 => present.remove(0),
        _ => return Err(DeclarationError::new(Kind::AmbiguousFiles).with_path(dir.to_path_buf())),
    };
    let unreadable = || DeclarationError::new(Kind::Unreadable).with_path(path.clone());
    let metadata = std::fs::metadata(&path).map_err(|_| unreadable())?;
    if !metadata.is_file() {
        return Err(unreadable());
    }
    let bytes = std::fs::read(&path).map_err(|_| unreadable())?;
    let declaration =
        parse(&bytes, format_of(name), kind).map_err(|error| error.with_path(path.clone()))?;
    Ok(Some(Level {
        kind,
        dir: dir.to_path_buf(),
        declaration,
    }))
}

/// Every level a checkout sees, nearest first.
///
/// **A binding of this crate, not of the specification**, which leaves how levels are
/// found to each implementation. The order is:
///
/// 1. the repository's own declaration, in `checkout` itself ([`LevelKind::Repository`]);
/// 2. each ancestor directory of `checkout`, nearest first ([`LevelKind::Directory`]) —
///    up to and including `home` when `checkout` is inside `home`, and otherwise up to but
///    excluding the filesystem root;
/// 3. the `fallback` directory a host names, if any ([`LevelKind::Fallback`]).
///
/// A directory holding no declaration contributes no level. The paths are taken as given
/// (pass absolute ones); the first level that cannot be read or is invalid is the error,
/// and the caller's check is then unavailable.
///
/// ```
/// use privateterms::{discover_levels, LevelKind};
///
/// let home = tempfile::tempdir().unwrap();
/// let checkout = home.path().join("work").join("quillmoss");
/// std::fs::create_dir_all(&checkout).unwrap();
/// std::fs::write(checkout.join("private-terms.toml"), "schema_version = 1\n").unwrap();
/// std::fs::write(home.path().join("private-terms.yml"), "schema_version: 2\n").unwrap();
///
/// let levels = discover_levels(&checkout, home.path(), None).unwrap();
/// let kinds: Vec<LevelKind> = levels.iter().map(|level| level.kind).collect();
/// assert_eq!(kinds, [LevelKind::Repository, LevelKind::Directory]);
/// assert_eq!(levels[1].dir, home.path());
/// ```
pub fn discover_levels(
    checkout: &Path,
    home: &Path,
    fallback: Option<&Path>,
) -> Result<Vec<Level>, DeclarationError> {
    let mut levels = Vec::new();
    levels.extend(read_dir_level(checkout, LevelKind::Repository)?);
    let inside_home = checkout.starts_with(home);
    // A checkout that is the home directory has no ancestor up to and including it.
    let ancestors = checkout
        .ancestors()
        .skip(1)
        .take_while(|_| checkout != home);
    for ancestor in ancestors {
        if !inside_home && ancestor.parent().is_none() {
            break;
        }
        if ancestor.as_os_str().is_empty() {
            break;
        }
        levels.extend(read_dir_level(ancestor, LevelKind::Directory)?);
        if inside_home && ancestor == home {
            break;
        }
    }
    if let Some(fallback) = fallback {
        levels.extend(read_dir_level(fallback, LevelKind::Fallback)?);
    }
    Ok(levels)
}
