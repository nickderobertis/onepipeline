//! The explicit triggers a verb asks for — `onepipeline review` and `onepipeline
//! readjust` — and how each is performed: through the driver of a run something drives,
//! or inline under the ownership lock on a run nothing drives.

use crate::{
    channel::ChannelState,
    error::{Error, Result},
    journal::Journal,
    ledger::RunPaths,
};

/// Which explicit trigger a verb asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Asked {
    /// `onepipeline review`: a readjustment, then discovery.
    Review,
    /// `onepipeline readjust`: the readjustment alone.
    Readjust,
}

impl Asked {
    fn command(self) -> crate::channel::Command {
        match self {
            Self::Review => crate::channel::Command::Review {},
            Self::Readjust => crate::channel::Command::Readjust {},
        }
    }

    fn word(self) -> &'static str {
        match self {
            Self::Review => "review",
            Self::Readjust => "readjust",
        }
    }
}

/// What an explicit trigger answered: performed, with a host read's failure and the
/// drafts whose verdict is still owed, or requested of a driver that has not answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Answered {
    Performed {
        error: Option<String>,
        // llmlint: ignore[invalid_states_unrepresentable] these are node ids read off the folded `RunState`'s own keys, which come from the validated graph and are `String` throughout this crate, which has no node-identity type; they are only printed, never parsed or executed, and no external input reaches this field.
        outstanding: Vec<String>,
    },
    Requested(u64),
}

pub(crate) fn requested(
    paths: &RunPaths,
    session: &str,
    timeout: std::time::Duration,
    asked: Asked,
) -> Result<Answered> {
    let view = crate::views::RunView::open(paths)?;
    let driven = matches!(
        view.liveness(),
        crate::views::DriverLiveness::Driving | crate::views::DriverLiveness::Parked
    );
    if !driven {
        if !view.launch.owned_by(session) {
            return Err(Error::NotOwned {
                run: paths.run.clone(),
                owner: view.launch.owner_label(session),
            });
        }
        let lock = crate::ledger::OwnershipLock::acquire(paths, asked.word())?;
        return undriven(paths, lock, asked);
    }
    let deadline = || {
        std::time::Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| {
                Error::Invalid(format!(
                    "{} reply timeout is too large for this host's clock",
                    asked.word()
                ))
            })
    };
    deadline()?;
    crate::review::failpoint("before-submit")?;
    let channel = ChannelState::new(paths);
    let id = match crate::engine::accept_as(
        paths,
        &channel,
        crate::channel::Author::planner(),
        &[asked.command()],
        asked.word(),
    )? {
        crate::engine::Accepted::Queued(id) => id,
        crate::engine::Accepted::NothingIsDriving(lock) => {
            // The driver may have ended between the liveness read and submission.
            if !view.launch.owned_by(session) {
                return Err(Error::NotOwned {
                    run: paths.run.clone(),
                    owner: view.launch.owner_label(session),
                });
            }
            return undriven(paths, lock, asked);
        }
    };
    let deadline = deadline()?;
    loop {
        if let Some(outcome) = channel.outcome_of(id) {
            return Ok(Answered::Performed {
                error: (!outcome.applied).then(|| {
                    outcome
                        .reason
                        .unwrap_or_else(|| format!("{} failed", asked.word()))
                }),
                outstanding: crate::readjust::outstanding(&crate::checkpoint::fold_and_checkpoint(
                    paths,
                )),
            });
        }
        if std::time::Instant::now() >= deadline {
            return Ok(Answered::Requested(id));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// The pass on a run nothing drives, under its lock: the readjustment and discovery
/// inline, and then — only where the readjustment owes a draft a restack or a retarget —
/// the run driven until those settle.
fn undriven(
    paths: &RunPaths,
    lock: crate::ledger::OwnershipLock,
    asked: Asked,
) -> Result<Answered> {
    let mut projected = crate::checkpoint::Projected::open(paths);
    let mut journal = Journal::open(paths);
    let error = match asked {
        Asked::Review => match crate::review::pass(paths, &mut projected, &mut journal) {
            Ok(()) => None,
            Err(Error::Sibling { message, .. }) => Some(message),
            Err(error) => return Err(error),
        },
        Asked::Readjust => crate::readjust::run(
            paths,
            &mut journal,
            &mut projected,
            crate::payload::ReadjustTrigger::Readjust,
        )?
        .failed(),
    };
    let outstanding = crate::readjust::finish_undriven(paths, lock, projected)?;
    Ok(Answered::Performed { error, outstanding })
}
