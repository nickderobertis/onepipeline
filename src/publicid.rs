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
//! It is decided once, where the run id is minted, and kept in the launch record. Where the
//! check cannot answer, the id is the hashed form and the plan's name is withheld: neither
//! names anything, so the launch goes on rather than one unreadable private declaration
//! stopping every launch on the host. A record written before there was one reads the same
//! derivation over its id ([`of_record`]).
//!
//! Where the engine uses it: every write-back to a plan store whose source is not declared
//! `visibility: private` names the run by it in `onepipeline.driver.run` and
//! `onepipeline.publishing.run`, and carries it as `onepipeline.name` too where the plan's
//! own name fails the same check; and a branch rendered for a node whose repository is public
//! renders `plan.name` as it under the same condition. A run whose id passes writes exactly
//! what it wrote before.

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::ledger::LaunchRecord;

/// A run's public id, which only this module makes: the run id the check passed, the hashed
/// form of one, or the one a launch recorded — so a run id nothing checked is never handed
/// where a public id is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PublicId(String);

impl PublicId {
    /// The id as it is written.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// The id as it is written, owned.
    pub(crate) fn into_string(self) -> String {
        self.0
    }
}

/// `run-` and the first twelve lowercase hex digits of the SHA-256 of `run`'s UTF-8 bytes.
pub(crate) fn hashed(run: &str) -> PublicId {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(run.as_bytes());
    let hex: String = digest
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    PublicId(format!("run-{hex}"))
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

/// Whether a plan's own name may be named where the destination is not private.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanName {
    /// The check passed it, or the plan has none: it is written as it is.
    Named,
    /// The check refused it or could not answer: the public id is written in its place.
    Withheld,
}

/// What a launch records about how its run is named publicly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Decided {
    /// The run's public id.
    pub(crate) id: PublicId,
    /// Whether the plan's own name may be named where the destination is not private.
    pub(crate) plan_name: PlanName,
}

/// Decide `run`'s public id, and whether `name` — the plan's — may be named publicly.
///
/// Only a pass names either: where the check refuses or cannot answer, the id is the hashed
/// form and the name is withheld, so nothing that may be private is guessed onto a public
/// destination, and the launch is never refused for it.
pub(crate) fn decide(run: &str, name: Option<&str>) -> Decided {
    decide_with(run, name, passes)
}

fn decide_with(
    run: &str,
    name: Option<&str>,
    mut passes: impl FnMut(&str) -> std::result::Result<bool, String>,
) -> Decided {
    let id = if passes(run) == Ok(true) {
        PublicId(run.to_owned())
    } else {
        hashed(run)
    };
    let plan_name = if name
        .filter(|name| !name.is_empty())
        .is_some_and(|name| passes(name) != Ok(true))
    {
        PlanName::Withheld
    } else {
        PlanName::Named
    };
    Decided { id, plan_name }
}

/// The public ids this process derived for records that hold none, by run id.
static DERIVED: Mutex<BTreeMap<String, PublicId>> = Mutex::new(BTreeMap::new());

/// `launch`'s public id: the one its launch recorded, or — for a record written before there
/// was one — the same derivation over its id. A read must answer, so where the check cannot,
/// such a record reads the hashed form, which names nothing.
pub(crate) fn of_record(launch: &LaunchRecord) -> PublicId {
    if !launch.public_id.is_empty() {
        return PublicId(launch.public_id.clone());
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
        Ok(true) => PublicId(run.clone()),
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
        of_record(launch).into_string()
    }
}

/// What `launch`'s plan is named by on its own plan store, given its name: the public id where
/// the launch found the name fails the check and the store does not declare the project's
/// source private, and the name itself otherwise.
pub(crate) fn name_on_store(launch: &LaunchRecord, name: &str) -> String {
    if launch.name_withheld && !home_declared_private(launch) {
        of_record(launch).into_string()
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
        assert_eq!(id.as_str(), "run-ba7816bf8f01");
        assert_eq!(id.as_str().len(), "run-".len() + 12);
    }

    #[test]
    fn an_id_the_check_passes_is_its_own_public_id_and_one_it_refuses_is_hashed() {
        let decided = decide_with("public-plan", Some("public plan"), |_| Ok(true));
        assert_eq!(
            decided,
            Decided {
                id: PublicId("public-plan".to_owned()),
                plan_name: PlanName::Named
            }
        );
        let decided = decide_with("secret-plan", Some("secret plan"), |_| Ok(false));
        assert_eq!(decided.id, hashed("secret-plan"));
        assert_eq!(decided.plan_name, PlanName::Withheld);
        // No name is nothing to withhold, and is never asked.
        let decided = decide_with("secret-plan", None, |term| {
            assert_eq!(term, "secret-plan");
            Ok(false)
        });
        assert_eq!(decided.plan_name, PlanName::Named);
    }

    #[test]
    fn a_check_that_cannot_answer_hashes_the_id_and_withholds_the_name() {
        let unanswered = decide_with("x", Some("name"), |_| {
            Err("the registry could not be read".into())
        });
        assert_eq!(
            unanswered,
            Decided {
                id: hashed("x"),
                plan_name: PlanName::Withheld
            }
        );
        // A name the check cannot judge is withheld even where the id passes.
        let decided = decide_with("x", Some("name"), |term| {
            if term == "x" {
                Ok(true)
            } else {
                Err("no answer".into())
            }
        });
        assert_eq!(
            decided,
            Decided {
                id: PublicId("x".to_owned()),
                plan_name: PlanName::Withheld
            }
        );
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
