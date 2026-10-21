//! A run's **public id**: what a run is named by wherever the destination is not private.
//!
//! A run id is minted from its plan's name, so a plan titled after a private repository
//! mints an id carrying that repository's name, and every public tracker and branch the run
//! writes would name it. The public id is the run id itself where the linked `onevcs`'s
//! public-output check passes the id as metadata bound for a public destination — over every
//! private identity registered on the host, the term scope left out — and otherwise `run-`
//! followed by the first twelve lowercase hex digits of the SHA-256 of the id's UTF-8 bytes,
//! which says nothing about the run.
//!
//! It is decided once, where the run id is minted, and kept in the launch record: a check
//! that cannot answer refuses the launch rather than guessing either way. A record written
//! before there was one reads the same derivation over its id ([`of_record`]).
//!
//! Where the engine uses it: every write-back to a plan store whose source is not declared
//! `visibility: private` names the run by it in `onepipeline.driver.run` and
//! `onepipeline.publishing.run`, and carries it as `onepipeline.name` too where the plan's
//! own name fails the same check; and a branch rendered for a node whose repository is public
//! renders `plan.name` as it under the same condition. A run whose id passes writes exactly
//! what it wrote before.

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::error::{Error, Result};
use crate::ledger::LaunchRecord;

/// What the public-output check is called, in a refusal that names it.
pub(crate) const CHECK: &str = "the public-output check (`onevcs` `check_public_output`)";

/// `run-` and the first twelve lowercase hex digits of the SHA-256 of `run`'s UTF-8 bytes.
pub(crate) fn hashed(run: &str) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(run.as_bytes());
    let hex: String = digest
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("run-{hex}")
}

/// Whether `term` may be named on a public destination, as the linked `onevcs` answers over
/// every private identity registered on this host: `Err` carrying why where it cannot say.
pub(crate) fn passes(term: &str) -> std::result::Result<bool, String> {
    passes_with(term, onevcs::check_public_output)
}

fn passes_with(
    term: &str,
    check: impl FnOnce(onevcs::BoundaryInput) -> onevcs::Result<onevcs::BoundaryVerdict>,
) -> std::result::Result<bool, String> {
    let verdict = check(onevcs::BoundaryInput {
        destination: onevcs::Visibility::Public,
        text: Vec::new(),
        paths: Vec::new(),
        metadata: vec![term.to_owned()],
        scope: onevcs::TermScope::Registry,
    })
    .map_err(|error| error.to_string())?;
    match verdict {
        onevcs::BoundaryVerdict::Pass => Ok(true),
        onevcs::BoundaryVerdict::Refuse { .. } => Ok(false),
        unavailable @ onevcs::BoundaryVerdict::Unavailable { .. } => Err(unavailable
            .reason()
            .unwrap_or_else(|| "it gave no answer".to_owned())),
    }
}

/// What a launch records about how its run is named publicly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Decided {
    /// The run's public id.
    pub(crate) id: String,
    /// Whether the plan's own name fails the check, so a destination that is not private
    /// carries the public id in its place.
    pub(crate) name_withheld: bool,
}

/// Decide `run`'s public id, and whether `name` — the plan's — may be named publicly.
///
/// # Errors
///
/// [`Error::Refused`] naming the check where it cannot answer for either: nothing is
/// guessed, and the launch goes no further.
pub(crate) fn decide(run: &str, name: Option<&str>) -> Result<Decided> {
    decide_with(run, name, passes)
}

fn decide_with(
    run: &str,
    name: Option<&str>,
    mut passes: impl FnMut(&str) -> std::result::Result<bool, String>,
) -> Result<Decided> {
    let refused = |why: String| {
        Error::Refused(format!(
            "{CHECK} could not answer whether run '{run}' may be named on a public \
             destination, so no public id was decided and nothing was launched: {why}"
        ))
    };
    let id = if passes(run).map_err(refused)? {
        run.to_owned()
    } else {
        hashed(run)
    };
    let name_withheld = match name.filter(|name| !name.is_empty()) {
        Some(name) => !passes(name).map_err(refused)?,
        None => false,
    };
    Ok(Decided { id, name_withheld })
}

/// The public ids this process derived for records that hold none, by run id.
static DERIVED: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());

/// `launch`'s public id: the one its launch recorded, or — for a record written before there
/// was one — the same derivation over its id. A read must answer, so where the check cannot,
/// such a record reads the hashed form, which names nothing.
pub(crate) fn of_record(launch: &LaunchRecord) -> String {
    if !launch.public_id.is_empty() {
        return launch.public_id.clone();
    }
    let run = &launch.run_id;
    if let Some(known) = DERIVED
        .lock()
        .ok()
        .and_then(|known| known.get(run).cloned())
    {
        return known;
    }
    let derived = match passes(run) {
        Ok(true) => run.clone(),
        Ok(false) | Err(_) => hashed(run),
    };
    if let Ok(mut known) = DERIVED.lock() {
        known.insert(run.clone(), derived.clone());
    }
    derived
}

/// Whether the plan store's source holding `launch`'s project is declared
/// `visibility: private`, by the store's own configuration, by launch directory and project.
static DECLARED_PRIVATE: Mutex<BTreeMap<(std::path::PathBuf, String), bool>> =
    Mutex::new(BTreeMap::new());

/// Whether `launch`'s plan store holds its project in a source the store declares
/// `visibility: private`. A configuration that cannot be read declares nothing private.
fn home_declared_private(launch: &LaunchRecord) -> bool {
    let Ok(project) = launch.project.parse::<crate::taskgraph::QualifiedId>() else {
        return false;
    };
    let key = (launch.dir.clone(), project.as_str().to_owned());
    if let Some(known) = DECLARED_PRIVATE
        .lock()
        .ok()
        .and_then(|known| known.get(&key).copied())
    {
        return known;
    }
    let declared =
        crate::taskgraph::Store::at(&launch.dir).source_declared_private(project.source());
    if let Ok(mut known) = DECLARED_PRIVATE.lock() {
        known.insert(key, declared);
    }
    declared
}

/// What `launch`'s run is named by on its own plan store: its public id where the store does
/// not declare the project's source private, and its run id where it does.
pub(crate) fn on_store(launch: &LaunchRecord) -> String {
    if home_declared_private(launch) {
        launch.run_id.clone()
    } else {
        of_record(launch)
    }
}

/// What `launch`'s plan is named by on its own plan store, given its name: the public id where
/// the launch found the name fails the check and the store does not declare the project's
/// source private, and the name itself otherwise.
pub(crate) fn name_on_store(launch: &LaunchRecord, name: &str) -> String {
    if launch.name_withheld && !home_declared_private(launch) {
        of_record(launch)
    } else {
        name.to_owned()
    }
}

/// The run under `root` a store names `named` — the run of that id, or else the one whose
/// launch recorded `named` as its public id — or `None` where this host holds neither.
pub(crate) fn run_named(root: &std::path::Path, named: &str) -> Option<String> {
    if root.join(named).is_dir() {
        return Some(named.to_owned());
    }
    std::fs::read_dir(root)
        .ok()?
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| {
            crate::ledger::read_json::<LaunchRecord>(&entry.path().join("launch.json")).ok()
        })
        .find(|launch| launch.public_id == named)
        .map(|launch| launch.run_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hashed_form_is_run_and_twelve_hex_digits_of_the_ids_sha256() {
        // SHA-256("abc"), FIPS 180-2's own example.
        let id = hashed("abc");
        assert_eq!(id, "run-ba7816bf8f01");
        assert_eq!(id.len(), "run-".len() + 12);
    }

    #[test]
    fn an_id_the_check_passes_is_its_own_public_id_and_one_it_refuses_is_hashed() {
        let decided = decide_with("public-plan", Some("public plan"), |_| Ok(true))
            .expect("the check answered");
        assert_eq!(
            decided,
            Decided {
                id: "public-plan".to_owned(),
                name_withheld: false
            }
        );
        let decided = decide_with("secret-plan", Some("secret plan"), |_| Ok(false))
            .expect("the check answered");
        assert_eq!(decided.id, hashed("secret-plan"));
        assert!(decided.name_withheld);
        // No name is nothing to withhold, and is never asked.
        let decided = decide_with("secret-plan", None, |term| {
            assert_eq!(term, "secret-plan");
            Ok(false)
        })
        .expect("the check answered");
        assert!(!decided.name_withheld);
    }

    #[test]
    fn a_check_that_cannot_answer_refuses_naming_the_check() {
        let refused = decide_with("x", None, |_| Err("the registry could not be read".into()))
            .expect_err("no answer is no public id");
        let message = refused.to_string();
        assert!(message.contains(CHECK), "{message}");
        assert!(
            message.contains("the registry could not be read"),
            "{message}"
        );
        let refused = decide_with("x", Some("name"), |term| {
            if term == "x" {
                Ok(true)
            } else {
                Err("no answer".into())
            }
        })
        .expect_err("a name the check cannot judge refuses too");
        assert!(refused.to_string().contains(CHECK));
    }

    #[test]
    fn an_unavailable_verdict_and_an_error_are_not_answers() {
        let unavailable = passes_with("x", |_| {
            Ok(onevcs::BoundaryVerdict::Unavailable {
                reason: onevcs::boundary::Unavailability::Registry,
            })
        });
        assert!(unavailable.is_err(), "{unavailable:?}");
        assert_eq!(
            passes_with("x", |input| {
                assert_eq!(input.metadata, vec!["x".to_owned()]);
                assert!(input.scope.is_registry());
                assert_eq!(input.destination, onevcs::Visibility::Public);
                Ok(onevcs::BoundaryVerdict::Pass)
            }),
            Ok(true)
        );
        assert_eq!(
            passes_with("x", |_| Ok(onevcs::BoundaryVerdict::Refuse {
                surface: onevcs::boundary::Surface::Metadata
            })),
            Ok(false)
        );
    }
}
