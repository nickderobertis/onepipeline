//! The **watcher record**: the evidence that a run is being watched.
//!
//! What the record is and what it promises is entry 68 of
//! `docs/contract-divergences.md`, and is not restated here. What is worth saying
//! beside the code is the two rules a maintainer has to keep apart, because
//! nothing in the types enforces them:
//!
//! **What is reported inverts this crate's usual asymmetry.**
//! [`sys::process_may_be_live`] resolves every unknown toward "still working",
//! because for a *driver* the worse error is reporting live work as dead. For a
//! *watch* it is the other one, so every unknown here resolves toward **not
//! watched** — a record that cannot be read, a host that will not report a start
//! token, a pid it will not describe.
//!
//! **What is removed does not.** Deletion is the one act here that cannot be taken
//! back, so it needs evidence the process is *gone* rather than the absence of
//! evidence that it is there: a token that was read and disagrees is a removal, and
//! one this host would not give is not. `WatchStanding::proved_gone` is where the
//! two bars are kept apart, and folding them cost a live watcher its record.
//!
//! Nothing else stands between a watcher dying and its run reading unwatched —
//! no heartbeat, no expiry, no cleanup step — because the deaths that matter have
//! no clean exit. Removing the record on a clean exit is permitted ([`Armed`]) and
//! is never relied upon; the reading side removes nothing at all, exactly as
//! everything in [`crate::views`] reads.

use std::num::NonZeroU32;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ledger::{self, RunPaths, Skipped};
use crate::sys;

/// The schema version of the watcher record.
///
/// The same compatibility statement the summary document makes, and the reason
/// this document is read **closed** — `deny_unknown_fields` — while the launch
/// record beside it is read permissively: a version this build does not write is
/// refused, and a refused record is not a live watch. That resolves in the safe
/// direction here, which is what makes refusing it affordable at all: a build
/// that met a record it did not understand and served it anyway would report a
/// run as watched out of fields that mean something else.
pub const WATCHER_SCHEMA_VERSION: u32 = 1;

/// Read the version, refusing a record this build cannot honestly read.
fn this_version<'de, D: serde::Deserializer<'de>>(reader: D) -> Result<u32, D::Error> {
    let found = u32::deserialize(reader)?;
    if found != WATCHER_SCHEMA_VERSION {
        return Err(serde::de::Error::custom(format!(
            "watcher schema_version {found}, and this build reads {WATCHER_SCHEMA_VERSION}"
        )));
    }
    Ok(found)
}

/// One live watch, as the process taking it wrote itself down.
///
/// Every field is required. There is no absence to be lenient about: a record
/// missing any of these could not be decided against the host at all, and a
/// record that cannot be decided is not a live watch — so accepting one would
/// only move the refusal from the reader to the verdict.
// llmlint: ignore-block[invalid_states_unrepresentable] the run id, the host and the two
// stamps are `String`s for the reason `src/ledger.rs`'s own file-level suppression states,
// and this is that file's records read back: `docs/contract.md` names no `RunId`, no `Host`
// and no timestamp type, so a newtype here would be a public vocabulary the contract did
// not ask for, and a record an older build wrote has to be accepted as it stands rather
// than as this build would mint it. The block covers the four declarations and stops at
// the closing brace, because what is checked is checked: `schema_version` is refused by
// the deserializer, `began_at` is refused unless it is the instant it says it is, `pid` is
// a `NonZeroU32` rather than a checked `u32`, and `started` is compared only through
// `sys::StartToken::matches`, which is what carries the rule that an empty one never
// matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WatcherRecord {
    /// The record's own version, so a reader can refuse one it does not
    /// understand. See [`WATCHER_SCHEMA_VERSION`].
    #[serde(deserialize_with = "this_version")]
    pub schema_version: u32,
    /// The run this watch is of.
    ///
    /// Carried rather than inferred from where the file sits, and checked against
    /// it: a record whose `run_id` is not the run whose directory it is in is not
    /// a watch of that run, however it got there — a copied run root, a restored
    /// backup, a record written by a build pointed at another ledger.
    pub run_id: String,
    /// The watching process.
    pub pid: NonZeroU32,
    /// The host that pid is meaningful on, as `sys::hostname` reads it.
    ///
    /// A pid means nothing across machines, so a record naming another host is
    /// never a live watch here — which is the inverted asymmetry the module
    /// documentation states, and the opposite of what a *driver*'s liveness
    /// reading does with the same fact.
    pub host: String,
    /// That process's start token, as `sys::process_start_token` reads it, and
    /// **empty** where this host would not say.
    ///
    /// The half of the proof a pid cannot give: a pid is reused, so a record
    /// naming one is evidence about the process that took it only while that
    /// process still holds it. Empty never matches, so a host that will not
    /// report a token leaves records that are never live watches — every run on
    /// such a host reads unwatched, which is the honest answer rather than a
    /// defect and resolves in the safe direction.
    ///
    /// It does **not** resolve that way for the sweep, and the two are decided
    /// apart: a record carrying no token is one nothing can judge, so nothing
    /// removes it while the pid it names is live. See
    /// [`WatchStanding::Unproven`].
    pub started: String,
    /// When the watch began, as an RFC 3339 string.
    ///
    /// For a person reading the directory. It decides nothing: an interval is
    /// exactly what liveness here is never read from.
    ///
    /// **Refused unless it is one**, all the same, and not because anything here
    /// parses it. This record is external input — a file on disk, written by some
    /// other process, possibly some other build — and a field documented as an
    /// instant that carries something else is a record this build did not write.
    /// The refusal costs nothing and resolves where every unknown here resolves:
    /// the record is not a live watch, and the run reads unwatched.
    #[serde(deserialize_with = "an_instant")]
    pub began_at: String,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// The schema version of the watch-terms record, read closed exactly as
/// [`WATCHER_SCHEMA_VERSION`] is and for the same reason.
pub const WATCH_TERMS_SCHEMA_VERSION: u32 = 1;

/// Read the terms' version, refusing a record this build cannot honestly read.
fn this_terms_version<'de, D: serde::Deserializer<'de>>(reader: D) -> Result<u32, D::Error> {
    let found = u32::deserialize(reader)?;
    if found != WATCH_TERMS_SCHEMA_VERSION {
        return Err(serde::de::Error::custom(format!(
            "watch-terms schema_version {found}, and this build reads {WATCH_TERMS_SCHEMA_VERSION}"
        )));
    }
    Ok(found)
}

/// What one live watch was armed to do: when it gives up, what it returns on,
/// and whose it is — written beside its [`WatcherRecord`], under the lease's own
/// file name in `watch-terms/`, and removed and swept with it.
///
/// A second document rather than more fields on the lease, because an engine
/// before this one reads every file in `watchers/` closed: a lease that grew a
/// field would read to it as a record it cannot parse, and every run it watched
/// would read unwatched to it. The lease is written byte-identical to what that
/// engine wrote, and this is what a wake budget is decided from. Entry 96 of
/// `docs/contract-divergences.md` states the record and the rule over it.
///
/// Every field is required, `null` included where a field is nullable: a record
/// missing one is not the terms of any watch this build armed, and is refused
/// rather than read as the absence of a deadline or of a session.
// llmlint: ignore-block[invalid_states_unrepresentable] the run id, the start token, the
// instant and the session are `String`s for the reason the lease's own block above states —
// this is a record another build may have written, read back as it stands, and the contract
// names no type for any of them. What is checked is checked at the boundary: the version,
// `deadline` as an RFC 3339 instant, each `until` word as a condition `--until` takes, and
// `session` as a session that names somebody.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WatchTerms {
    /// The record's own version. See [`WATCH_TERMS_SCHEMA_VERSION`].
    #[serde(deserialize_with = "this_terms_version")]
    pub schema_version: u32,
    /// The run the watch is of — the lease's own.
    pub run_id: String,
    /// The watching process — the lease's own.
    pub pid: NonZeroU32,
    /// That process's start token — the lease's own.
    pub started: String,
    /// When the watch gives up on the clock, as an RFC 3339 instant; `null` for
    /// `--timeout none`, which never does. `--timeout 0` records the arming
    /// instant.
    #[serde(deserialize_with = "a_deadline")]
    pub deadline: Option<String>,
    /// Every condition the watch returns on, spelled as `--until` takes them —
    /// always including `settled` and `nothing-driving`, which every watch
    /// returns on.
    #[serde(deserialize_with = "conditions")]
    pub until: Vec<String>,
    /// The non-blank `ONEPIPELINE_LAUNCHER_SESSION` the watch was armed under,
    /// or `null` where none was.
    #[serde(deserialize_with = "a_session")]
    pub session: Option<String>,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// Read a deadline: an RFC 3339 instant, or `null`, and required either way.
fn a_deadline<'de, D: serde::Deserializer<'de>>(reader: D) -> Result<Option<String>, D::Error> {
    let found = Option::<String>::deserialize(reader)?;
    if let Some(deadline) = &found {
        if instant_millis(deadline).is_none() {
            return Err(serde::de::Error::custom(format!(
                "deadline is '{deadline}', which is not an RFC 3339 instant"
            )));
        }
    }
    Ok(found)
}

/// Read the conditions: each one a word `--until` takes.
fn conditions<'de, D: serde::Deserializer<'de>>(reader: D) -> Result<Vec<String>, D::Error> {
    let found = Vec::<String>::deserialize(reader)?;
    for word in &found {
        word.parse::<crate::cli::WatchUntil>().map_err(|_| {
            serde::de::Error::custom(format!(
                "until names '{word}', which is not a condition `--until` takes"
            ))
        })?;
    }
    Ok(found)
}

/// Read a session: one that names somebody, or `null`.
fn a_session<'de, D: serde::Deserializer<'de>>(reader: D) -> Result<Option<String>, D::Error> {
    let found = Option::<String>::deserialize(reader)?;
    if found
        .as_deref()
        .is_some_and(|session| session.trim().is_empty())
    {
        return Err(serde::de::Error::custom(
            "session is blank, which names nobody; a watch armed under no session records null",
        ));
    }
    Ok(found)
}

/// An RFC 3339 instant as milliseconds since the Unix epoch, or nothing where
/// `text` is not one.
///
/// Decided by [`is_rfc3339`] first, so what this admits is exactly what that
/// admits; the arithmetic after it reads fields that check has already bounded.
/// The fraction is truncated to the millisecond, the precision everything this
/// crate writes carries.
pub(crate) fn instant_millis(text: &str) -> Option<i128> {
    if !is_rfc3339(text) {
        return None;
    }
    let (date, rest) = text.split_once(['T', 't'])?;
    let mut ymd = date.split('-').map(|part| part.parse::<i128>().ok());
    let (year, month, day) = (ymd.next()??, ymd.next()??, ymd.next()??);
    let (clock, offset_minutes) = match rest.find(['Z', 'z']) {
        Some(at) => (&rest[..at], 0),
        None => {
            let at = rest.rfind(['+', '-'])?;
            let (clock, zone) = rest.split_at(at);
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let (hours, minutes) = zone[1..].split_once(':')?;
            (
                clock,
                sign * (hours.parse::<i128>().ok()? * 60 + minutes.parse::<i128>().ok()?),
            )
        }
    };
    let (clock, fraction) = clock.split_once('.').unwrap_or((clock, "0"));
    let mut hms = clock.split(':').map(|part| part.parse::<i128>().ok());
    let (hour, minute, second) = (hms.next()??, hms.next()??, hms.next()??);
    let millis: i128 = format!("{fraction:0<3}")[..3].parse().ok()?;
    let days = days_from_civil(year, month, day);
    Some(
        ((days * 86_400 + hour * 3_600 + minute * 60 + second - offset_minutes * 60) * 1_000)
            + millis,
    )
}

/// Howard Hinnant's `days_from_civil`: a proleptic Gregorian date to days since
/// 1970-01-01 — the inverse of the one `sys` renders instants with.
fn days_from_civil(year: i128, month: i128, day: i128) -> i128 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = of_era * 365 + of_era / 4 - of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Read an RFC 3339 date and time, refusing anything that is not one.
fn an_instant<'de, D: serde::Deserializer<'de>>(reader: D) -> Result<String, D::Error> {
    let found = String::deserialize(reader)?;
    if !is_rfc3339(&found) {
        return Err(serde::de::Error::custom(format!(
            "began_at is '{found}', which is not an RFC 3339 instant"
        )));
    }
    Ok(found)
}

/// Whether `text` is an RFC 3339 date and time.
///
/// `YYYY-MM-DDThh:mm:ss`, then an optional fraction, then `Z` or an offset — the
/// separator and the zone marker in either case, which the grammar allows and
/// which this crate's own writer does not produce.
///
/// **The ranges as well as the shape**, down to the day being one its month has:
/// `2026-99-99T99:99:99Z` has the shape and is not an instant, and a check that
/// took it would be punctuation with a date's name on it. Leap seconds are the one
/// place the grammar is wider than a clock — `23:59:60` is a second RFC 3339 admits
/// — so `60` is allowed there and nowhere else.
///
/// Kept here rather than reached for from a dependency because it is the only place
/// this crate parses an instant at all: every other timestamp it holds it *writes*.
pub(crate) fn is_rfc3339(text: &str) -> bool {
    let Some((date, rest)) = text.split_once(['T', 't']) else {
        return false;
    };
    let [year, month, day] = date.split('-').collect::<Vec<_>>()[..] else {
        return false;
    };
    let (Some(year), Some(month), Some(day)) = (
        number(year, 4, 0..=9_999),
        number(month, 2, 1..=12),
        number(day, 2, 1..=31),
    ) else {
        return false;
    };
    if day > days_in(month, year) {
        return false;
    }
    let (clock, zone) = match rest.find(['Z', 'z', '+']) {
        Some(at) => rest.split_at(at),
        // A negative offset, whose sign is also the separator inside the date —
        // which is behind us, so the last one in what is left is the zone's.
        None => match rest.rfind('-') {
            Some(at) => rest.split_at(at),
            None => return false,
        },
    };
    if !matches!(zone, "Z" | "z") {
        let Some((hours, minutes)) = zone
            .strip_prefix('+')
            .or_else(|| zone.strip_prefix('-'))
            .and_then(|offset| offset.split_once(':'))
        else {
            return false;
        };
        if number(hours, 2, 0..=23).is_none() || number(minutes, 2, 0..=59).is_none() {
            return false;
        }
    }
    // The fraction is optional and any number of digits.
    let (clock, fraction) = clock.split_once('.').unwrap_or((clock, "0"));
    let [hour, minute, second] = clock.split(':').collect::<Vec<_>>()[..] else {
        return false;
    };
    number(hour, 2, 0..=23).is_some()
        && number(minute, 2, 0..=59).is_some()
        // llmlint: ignore-block[boundary_inputs_validated] `60` is admitted at any minute on
        // purpose, and narrowing it is not a check this or any build can make. A leap
        // second is inserted at the end of a UTC minute, but RFC 3339 renders an instant
        // in an *offset*, so the same second is a legal `18:29:60-05:30` — restricting the
        // value to `23:59` would refuse instants the grammar allows. Deciding it properly
        // needs the leap-second table for the date, which is unpublished for any future
        // one. The remaining slack is a second-value of `60` on a minute that had no leap
        // second, in a field this build never reads back: `began_at` is there for a person
        // looking at the directory, and every reading this verb makes is of the pid, the
        // host and the start token beside it.
        && number(second, 2, 0..=60).is_some()
        // llmlint: ignore-end[boundary_inputs_validated]
        && !fraction.is_empty()
        && fraction.chars().all(|c| c.is_ascii_digit())
}

/// One fixed-width decimal field of an instant, within the range the grammar gives
/// it.
///
/// The width is checked as well as the value, because RFC 3339 fixes it: `9-1-1`
/// is not a date, and a parse alone would take it.
fn number(text: &str, width: usize, allowed: std::ops::RangeInclusive<u32>) -> Option<u32> {
    if text.len() != width || !text.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    text.parse().ok().filter(|value| allowed.contains(value))
}

/// How many days that month of that year has.
///
/// The proleptic Gregorian rule, which is the calendar RFC 3339 dates are in.
fn days_in(month: u32, year: u32) -> u32 {
    match month {
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Whether one recorded watch is watching its run **now**, and where it is not,
/// why not.
///
/// Seven answers rather than a flag, because a reader that renders a run's
/// watchers has to say which of them it is: a record naming another host, one
/// whose process this host has proved is gone, and one whose pid has since been
/// handed to something else are three different things to tell an operator, and
/// only one of them means the watch was ever running here.
///
/// The last two are the sharpest distinction on the type, and the one thing on it
/// that a **writer** reads. Everything but [`Live`](Self::Live) reports the run
/// unwatched, which is where every unknown here resolves; but only some of them
/// are evidence that the recorded process is *gone*, and deleting a record is the
/// one act that cannot be taken back, and only some of these are evidence of one.
/// `proved_gone` — private, because what a *writer* may remove is not a promise
/// this crate makes to a reader — is where that is decided, and it is named in
/// plain code rather than linked for the reason `src/cli.rs` states: rustdoc
/// refuses a public item's link to a private one under this repository's denied
/// warnings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WatchStanding {
    /// The process it names is watching the run.
    Live,
    /// Its `run_id` is not the run whose directory it sits in, so it is not a
    /// watch of this run at all.
    AnotherRun,
    /// It names another host, where its pid means nothing.
    AnotherHost,
    /// This host has proved the process is gone.
    ProcessGone,
    /// The process has terminated and is only waiting for its parent to reap it.
    ///
    /// Not folded into [`ProcessGone`](Self::ProcessGone), because it is the one
    /// death that answers every other reading as life — see
    /// `sys::process_terminated_awaiting_parent` — and it is the state a watch
    /// killed by a parent that goes on running sits in.
    AwaitingItsParent,
    /// A start token read now **is not** the one recorded: the pid has been handed
    /// to another process, so the process that wrote this record is gone.
    ///
    /// A positive reading, and that is what separates it from
    /// [`Unproven`](Self::Unproven): two tokens were in hand and they disagreed.
    NotThatProcess,
    /// Nothing here can say whether the pid is the process that recorded it: this
    /// host would not report a start token now, or the record carries none.
    ///
    /// **An absence of evidence, and never a death certificate.** Deliberately not
    /// folded into [`NotThatProcess`](Self::NotThatProcess), which is what folding
    /// them cost: one failed reading of a live watcher's start would have swept its
    /// record away permanently, and no later successful reading could bring it
    /// back — so a supervisor whose watch was working would read the run unwatched
    /// and arm a second one, which is this verb's own failure mode inverted. The run
    /// reads unwatched on this standing, because that fails in the safe direction;
    /// the record stays, because deletion destroys the only thing that could ever
    /// have said otherwise.
    Unproven,
}

impl WatchStanding {
    /// Whether this standing is a watch that is watching.
    pub fn is_live(self) -> bool {
        matches!(self, Self::Live)
    }

    /// The phrase a view puts on the line, in the words a reader acts on.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Live => "watching",
            Self::AnotherRun => "its record names another run",
            Self::AnotherHost => "its record names another host",
            Self::ProcessGone => "its process is gone",
            Self::AwaitingItsParent => "its process ended and is waiting to be reaped",
            Self::NotThatProcess => "its pid is not the process that recorded it",
            Self::Unproven => "nothing can say whether it is the process that recorded it",
        }
    }

    /// Whether this build **proved** the recorded process is gone, on this host
    /// and for this run.
    ///
    /// What a writer may remove, and nothing else. The bar is deliberately higher
    /// than the bar for reporting a run unwatched, and the two must not be read off
    /// one another: *unwatched* is what an absence of evidence honestly answers,
    /// because being wrong there costs one re-armed watch — while *removal* is the
    /// one act that cannot be taken back, so it needs evidence the process is gone
    /// rather than merely the absence of evidence that it is there. A reading this
    /// host could not take is not a death certificate.
    ///
    /// So three standings qualify and four do not. A record naming another host or
    /// another run is not proved dead but merely not ours to judge; a record this
    /// build could not read at all is not judged, and never reaches this; and
    /// [`Unproven`](Self::Unproven) is the case this distinction exists for — a
    /// live watcher whose start this host would not report, whose record a sweep
    /// would otherwise erase for ever.
    fn proved_gone(self) -> bool {
        matches!(
            self,
            Self::ProcessGone | Self::AwaitingItsParent | Self::NotThatProcess
        )
    }
}

/// One record under a run's watcher directory, and what this host makes of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watch {
    /// The record as it was written.
    pub record: WatcherRecord,
    /// Whether the process it names is watching the run now.
    pub standing: WatchStanding,
}

/// Every watch recorded on one run, and every record that could not be read.
///
/// The two halves are returned together for the reason a run listing keeps its
/// own [`Skipped`] beside its rows: a reader told only what
/// could be read cannot tell an incomplete answer from a negative one. Here that
/// distinction is the whole point of the surface — a record this build cannot
/// read is **not** a live watch, so a run whose watcher evidence is incomplete
/// reads unwatched, and the reader is owed the reason rather than left to infer
/// it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Watchers {
    /// The records that read, in file-name order.
    pub watches: Vec<Watch>,
    /// The records that did not, each with the reason it was refused.
    pub refused: Vec<Skipped>,
}

impl Watchers {
    /// Read every watcher record on one run, deciding each against this host now.
    pub fn of(paths: &RunPaths) -> Self {
        Leases::of(paths).watchers
    }

    /// Whether anything is watching the run.
    pub fn any_live(&self) -> bool {
        self.watches.iter().any(|watch| watch.standing.is_live())
    }

    /// Why nothing is watching the run, in the words a view puts on its line.
    ///
    /// Composed from what was actually found rather than from a fixed phrase,
    /// because the three states a reader acts on differently all read as
    /// "unwatched": a run nobody ever armed a watch on, one whose watcher died,
    /// and one whose watcher evidence this build could not read at all.
    pub fn why_not_watched(&self) -> String {
        if self.watches.is_empty() && self.refused.is_empty() {
            return "nothing has recorded a watch on it".to_string();
        }
        let mut said: Vec<String> = self
            .watches
            .iter()
            .map(|watch| format!("pid {}: {}", watch.record.pid, watch.standing.as_str()))
            .collect();
        said.extend(
            self.refused
                .iter()
                .map(|refused| format!("a record that cannot be read: {}", refused.reason)),
        );
        said.join("; ")
    }
}

/// Every lease on one run with the terms recorded beside each, as the wake
/// budget reads them.
///
/// Crate-private and apart from [`Watchers`], whose shape is published: the
/// terms are what `unwatched` and `stop-guard` decide a budget from, and they are
/// read from the same walk, so a lease and its terms are the ones one reading
/// found together.
pub(crate) struct Leases {
    /// The leases, exactly as [`Watchers::of`] answers them.
    pub(crate) watchers: Watchers,
    /// The terms recorded beside each of [`Watchers::watches`], index for index.
    pub(crate) terms: Vec<TermsRead>,
}

/// What sits beside one lease in `watch-terms/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TermsRead {
    /// Nothing: the lease an engine before the terms record leaves.
    Absent,
    /// A document this build refused, with the reason.
    Unreadable(String),
    /// The terms, as written.
    Read(WatchTerms),
}

/// What a wake budget makes of one live lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Wake {
    /// Its terms are this lease's, name the asking session, give up within the
    /// budget and return on a surface: it will wake the session in time.
    Within,
    /// Its terms positively fail the budget, in their own words.
    Fails(String),
    /// Nothing here can say whether it will wake the session in time, and why.
    Unknown(String),
}

impl Leases {
    /// Read every lease on one run and the terms beside each, deciding each lease
    /// against this host now.
    pub(crate) fn of(paths: &RunPaths) -> Self {
        let mut watchers = Watchers::default();
        let mut terms = Vec::new();
        for (path, read) in records(paths) {
            match read {
                Ok(record) => {
                    let standing = standing_of(&record, &paths.run);
                    terms.push(terms_beside(paths, &path));
                    watchers.watches.push(Watch { record, standing });
                }
                Err(reason) => watchers.refused.push(Skipped { path, reason }),
            }
        }
        Self { watchers, terms }
    }

    /// What `budget_ms` makes of each **live** lease, in file-name order, for a
    /// session asking at `now_ms`.
    pub(crate) fn wakes(&self, session: &str, budget_ms: i128, now_ms: i128) -> Vec<Wake> {
        self.watchers
            .watches
            .iter()
            .zip(&self.terms)
            .filter(|(watch, _)| watch.standing.is_live())
            .map(|(watch, terms)| wake(&watch.record, terms, session, now_ms + budget_ms))
            .collect()
    }
}

/// What one live lease's terms say about waking `session` by `latest_ms`.
///
/// Every conjunct is asked, and one that fails is a positive failure whatever
/// the others say — so a terms record naming no session but recording no
/// deadline fails on the deadline, and only one whose every other conjunct holds
/// is left unknown by the session it does not name.
fn wake(record: &WatcherRecord, terms: &TermsRead, session: &str, latest_ms: i128) -> Wake {
    let pid = record.pid;
    let terms = match terms {
        TermsRead::Absent => {
            return Wake::Unknown(format!(
                "pid {pid}: its watch recorded no terms, as an engine before the wake budget \
                 leaves it, so whether it wakes this session in time cannot be said"
            ))
        }
        TermsRead::Unreadable(reason) => {
            return Wake::Unknown(format!(
                "pid {pid}: its terms record cannot be read, so whether it wakes this session \
                 in time cannot be said: {reason}"
            ))
        }
        TermsRead::Read(terms) => terms,
    };
    if (terms.run_id.as_str(), terms.pid, terms.started.as_str())
        != (record.run_id.as_str(), record.pid, record.started.as_str())
    {
        return Wake::Fails(format!(
            "pid {pid}: its terms record is not this watch's — it names run '{}', pid {}, \
             started '{}'",
            terms.run_id, terms.pid, terms.started
        ));
    }
    let mut fails: Vec<String> = Vec::new();
    match &terms.deadline {
        None => fails.push("it has no deadline (`--timeout none`)".to_owned()),
        Some(deadline) => {
            if instant_millis(deadline).is_none_or(|at| at > latest_ms) {
                fails.push(format!("its deadline {deadline} is past the wake budget"));
            }
        }
    }
    if !terms.until.iter().any(|word| word == "surface") {
        fails.push(format!(
            "it does not return on a surface (`--until {}`)",
            terms.until.join(" ")
        ));
    }
    match &terms.session {
        Some(named) if named != session => {
            fails.push(format!("it is another session's ('{named}')"));
        }
        _ => {}
    }
    if !fails.is_empty() {
        return Wake::Fails(format!("pid {pid}: {}", fails.join("; ")));
    }
    if terms.session.is_none() {
        return Wake::Unknown(format!(
            "pid {pid}: its terms name no session, so whether it wakes this session cannot be \
             said"
        ));
    }
    Wake::Within
}

/// The terms beside the lease at `lease`, read closed.
fn terms_beside(paths: &RunPaths, lease: &std::path::Path) -> TermsRead {
    let Some(name) = lease.file_name() else {
        return TermsRead::Absent;
    };
    match std::fs::read_to_string(paths.watch_terms().join(name)) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(terms) => TermsRead::Read(terms),
            Err(error) => TermsRead::Unreadable(error.to_string()),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => TermsRead::Absent,
        Err(error) => TermsRead::Unreadable(error.to_string()),
    }
}

/// Every entry under one run's watcher directory, in file-name order, each read
/// as a record or refused with a reason.
///
/// One walk behind both the reader and the writer's sweep, so what a sweep may
/// remove is decided by exactly the reading a reader would have taken of it —
/// and so the file a record came from is in hand for the one caller that needs
/// it, without putting a path on the surface a consumer renders.
///
/// A directory that is not there holds no watches and is no refusal: a run
/// nothing has ever watched never had one written.
fn records(paths: &RunPaths) -> Vec<(PathBuf, Result<WatcherRecord, String>)> {
    let dir = paths.watchers();
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        // A directory that will not open at all. Reached by a watch that could not
        // write its record — something else is at that path — which
        // `tests/e2e/unwatched.rs` drives through the binary.
        Err(error) => {
            return vec![(
                dir,
                Err(format!("the watcher directory cannot be read: {error}")),
            )]
        }
    };
    let mut read: Vec<(PathBuf, Result<WatcherRecord, String>)> = entries
        .map(|entry| match entry {
            Ok(entry) => {
                let path = entry.path();
                let record = read_record(&path);
                (path, record)
            }
            // llmlint: ignore-block[changed_behavior_has_e2e] an entry the filesystem lists
            // and then refuses to describe is a host condition no portable journey can set
            // — `src/ledger.rs`'s own listing carries the same suppression for the same
            // arm. What it must not do is drop the entry, which would report an incomplete
            // look at the directory as a complete one; the two answers a user *can* reach,
            // a directory that will not open and a record that will not parse, are both
            // driven through the binary in `tests/e2e/unwatched.rs`.
            Err(error) => (
                dir.clone(),
                Err(format!(
                    "an entry under the watcher directory cannot be read: {error}"
                )),
            ),
            // llmlint: ignore-end[changed_behavior_has_e2e]
        })
        .collect();
    read.sort_by(|a, b| a.0.cmp(&b.0));
    read
}

/// One record, or the reason it is not one.
///
/// Every failure is a *reason* rather than an absence, because a record that
/// cannot be read is not a live watch and the reader is owed the difference
/// between a directory holding nothing and a directory holding something this
/// build refused.
fn read_record(path: &std::path::Path) -> Result<WatcherRecord, String> {
    let text = std::fs::read_to_string(path).map_err(|error| format!("{error}"))?;
    serde_json::from_str(&text).map_err(|error| format!("{error}"))
}

/// What this host makes of one record, over the six conditions in order.
///
/// The order is not incidental. The run and the host come first because a pid
/// read on the wrong ledger or the wrong machine is not evidence at all; the
/// zombie reading comes after the pid check and before the token, because it is
/// the one state both of its neighbours answer as life.
fn standing_of(record: &WatcherRecord, run: &str) -> WatchStanding {
    if record.run_id != run {
        return WatchStanding::AnotherRun;
    }
    if record.host != sys::hostname() {
        return WatchStanding::AnotherHost;
    }
    let pid = record.pid.get();
    if !sys::process_may_be_live(pid) {
        return WatchStanding::ProcessGone;
    }
    if sys::process_terminated_awaiting_parent(pid) {
        return WatchStanding::AwaitingItsParent;
    }
    token_standing(sys::process_start_token(pid).as_ref(), &record.started)
}

/// The last of the six conditions, taken apart from the host that answers it: what
/// a start token **read now** says about a token recorded earlier.
///
/// A function of the two readings and nothing else, because this is the point that
/// decides whether a record may be deleted, and the difference it draws is between
/// an answer and the absence of one. On the platform this crate's tier runs on, a
/// live process's `/proc/<pid>/stat` is always readable, so `None` for a live pid
/// cannot be staged by a journey here — and a regression folding it back into
/// [`WatchStanding::NotThatProcess`] would erase a live watcher's record for ever.
/// Separating it is what lets that case be driven directly; see this module's own
/// tests.
fn token_standing(read: Option<&sys::StartToken>, recorded: &str) -> WatchStanding {
    match read {
        // Two readings in hand and they agree: this is the process that wrote it.
        Some(token) if token.matches(recorded) => WatchStanding::Live,
        // Two readings in hand and they disagree: the pid has been handed on, so the
        // process that wrote this record is gone. That is a *reading*, which is what
        // makes it the one answer here that admits removal.
        Some(_) if !recorded.is_empty() => WatchStanding::NotThatProcess,
        // Everything else is an absence of evidence rather than evidence of an
        // absence — this host would not say what the process's start is now, or the
        // record was written where it would not say then. The run reads unwatched,
        // and the record stays.
        _ => WatchStanding::Unproven,
    }
}

/// The record one live watch keeps under the run it is watching, removed when the
/// watch returns.
///
/// A guard rather than two calls, so the one path that *can* clean up does — and
/// nothing depends on it: a watch killed, crashed, or left on a closed terminal
/// runs no destructor, which is exactly why liveness is decided by reading the
/// host rather than by the record's presence.
pub(crate) struct Armed {
    /// The document this watch wrote, or nothing where it could not write one.
    path: Option<PathBuf>,
    /// The terms written beside it, or nothing where they could not be.
    terms: Option<PathBuf>,
}

impl Armed {
    /// Record that this process is watching `paths`, sweeping away the records
    /// this host can prove are not live watches.
    ///
    /// **Best effort, and silent.** A runs root this process may not write costs
    /// a reader the knowledge that this watch exists, and costs the watch itself
    /// nothing — so a failure here neither refuses the verb nor writes to a
    /// stream a supervisor is reading events on. The sweep is what keeps a run
    /// that has been watched a thousand times from holding a thousand records,
    /// and it removes only what `WatchStanding::proved_gone` admits.
    ///
    /// The terms go down **before** the lease and come away **after** it, so no
    /// reader finds this lease without its terms: a lease with none is what an
    /// older engine's watch leaves, and it reads as an unknown.
    pub(crate) fn arm(paths: &RunPaths, request: &crate::watch::Request) -> Self {
        Self::sweep(paths);
        let pid = sys::pid();
        let record = WatcherRecord {
            schema_version: WATCHER_SCHEMA_VERSION,
            run_id: paths.run.clone(),
            // A pid of zero is not a process on any platform this builds for, and
            // this one is asking about itself. `1` keeps the type honest where a
            // host answers absurdly rather than putting a checked `u32` back on
            // the record.
            pid: NonZeroU32::new(pid).unwrap_or(NonZeroU32::MIN),
            host: sys::hostname(),
            started: sys::process_start_token(pid)
                .map(|token| token.recorded().to_string())
                .unwrap_or_default(),
            began_at: sys::now_rfc3339(),
        };
        let path = paths.watcher(pid, &nonce());
        let terms = path
            .file_name()
            .map(|name| paths.watch_terms().join(name))
            .filter(|terms_path| {
                ledger::write_json(terms_path, &terms_of(&record, request)).is_ok()
            });
        let path = ledger::write_json(&path, &record).ok().map(|()| path);
        let mut armed = Self { path, terms };
        // Terms with no lease beside them are nobody's: a lease that could not be
        // written takes its terms with it.
        if armed.path.is_none() {
            if let Some(terms) = armed.terms.take() {
                let _ = std::fs::remove_file(terms);
            }
        }
        armed
    }

    /// Remove the records under this run that this host can prove are not live
    /// watches, and nothing else.
    ///
    /// A record it could not read is left exactly where it is. This process has
    /// proved nothing about it, and a writer that swept what it could not read
    /// would be deleting another build's evidence to tidy its own directory.
    fn sweep(paths: &RunPaths) {
        for (path, read) in records(paths) {
            if read.is_ok_and(|record| standing_of(&record, &paths.run).proved_gone()) {
                if let Some(name) = path.file_name() {
                    let _ = std::fs::remove_file(paths.watch_terms().join(name));
                }
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

impl Drop for Armed {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = std::fs::remove_file(path);
        }
        if let Some(terms) = self.terms.take() {
            let _ = std::fs::remove_file(terms);
        }
    }
}

/// The terms one watch is armed under, from the lease it writes beside them and
/// the request it resolved.
fn terms_of(record: &WatcherRecord, request: &crate::watch::Request) -> WatchTerms {
    let mut until: Vec<String> = Vec::new();
    for condition in request
        .until
        .iter()
        .map(ToString::to_string)
        .chain(["settled".to_owned(), "nothing-driving".to_owned()])
    {
        if !until.contains(&condition) {
            until.push(condition);
        }
    }
    WatchTerms {
        schema_version: WATCH_TERMS_SCHEMA_VERSION,
        run_id: record.run_id.clone(),
        pid: record.pid,
        started: record.started.clone(),
        deadline: match request.timeout {
            crate::cli::WatchTimeout::Bounded(seconds) => Some(sys::rfc3339_from_millis(
                sys::now_millis()
                    .saturating_add(seconds.saturating_mul(1_000))
                    // The latest instant RFC 3339's four-digit year can spell, so a
                    // wait no clock reaches is still a deadline a reader can read.
                    .min(LATEST_INSTANT_MS),
            )),
            crate::cli::WatchTimeout::Unbounded => None,
        },
        until,
        session: std::env::var(sys::LAUNCHER_SESSION_ENV)
            .ok()
            .filter(|session| !session.trim().is_empty()),
    }
}

/// `9999-12-31T23:59:59.999Z`, in epoch milliseconds.
const LATEST_INSTANT_MS: u64 = 253_402_300_799_999;

/// What tells one watch's record from another's in a file name, as hexadecimal
/// characters.
///
/// Two watches must not write one file, and a pid the kernel has handed round
/// again must not land on its predecessor's record. Both halves are for that: the
/// counter tells two watches armed in one process apart, and the seed tells this
/// process's watches from those of whatever held the pid before it.
///
/// **A clock this host would not read is not time zero.** The seed rather than the
/// clock is what a name is told apart by — the clock is one contributor to it and
/// never the only one — so a clock nobody read contributes nothing instead of a
/// value nobody took. Substituting zero there would leave the name a function of a
/// counter that restarts with the process, which is an identity unique per
/// *process* rather than per watch; the predecessor's record is no defence against
/// that, since one whose owner this host cannot disprove is deliberately kept
/// ([`WatchStanding::proved_gone`]).
///
/// Staleness is never decided by a name. What this buys is only that two live
/// records are two files.
pub(crate) fn nonce() -> String {
    static MINTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = MINTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    minted(
        seq,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|since| since.as_nanos()),
    )
}

/// The name itself, out of the counter and whatever the clock gave.
///
/// Taken apart from the host that answers it for the reason [`token_standing`] is:
/// a clock this host will not read cannot be staged by a journey on a host whose
/// clock works, and it is the exact case the value this replaces got wrong. Handed
/// the reading as a value, the case is drivable — see this module's own tests.
///
/// The seed is a fresh [`std::collections::hash_map::RandomState`], whose
/// documented property is the whole of what is wanted here: two of them are
/// unlikely to produce the same result for the same values. It is seeded from the
/// host's own randomness rather than from anything this process could fail to
/// read, so unlike the clock it has no absent answer to fold.
fn minted(seq: u64, clock: Option<u128>) -> String {
    use std::hash::{BuildHasher, Hasher};

    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(seq);
    // Contributed only where it was read. A reading nobody took is not a reading
    // of zero, and writing one in would put every watch on a clockless host back
    // on the one name its counter gives.
    if let Some(nanos) = clock {
        hasher.write_u128(nanos);
    }
    let seeded = hasher.finish();
    format!("{seeded:016x}{seq:04x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A start token this host **would not give** is not a mismatch, and the two
    /// are decided apart at the point that decides removal.
    ///
    /// The case the correction was for, driven where it can be driven at all: on
    /// Linux a live process's `/proc/<pid>/stat` is always readable, so no journey
    /// in this repository can stage a live pid whose current token comes back
    /// `None` — `tests/e2e/unwatched.rs` says so where it drives the half it can.
    /// This is the other half, and it is the one a regression would land on: fold
    /// `None` back into [`WatchStanding::NotThatProcess`] and the sweep erases the
    /// record of a watcher that is alive and watching, with no later reading able
    /// to bring it back.
    ///
    /// The token is **this process's own**, read from the host rather than
    /// constructed, so what the comparison is given is the same kind of value the
    /// verb gives it.
    #[test]
    fn a_token_this_host_would_not_give_is_not_a_mismatch() {
        let mine = sys::process_start_token(sys::pid())
            .expect("this host reports the start of the process asking");
        let recorded = mine.recorded().to_string();

        // Two readings that agree.
        assert_eq!(
            token_standing(Some(&mine), &recorded),
            WatchStanding::Live,
            "a token that matches the record is the process that wrote it"
        );
        // Two readings that disagree: a pid handed to another process.
        let standing = token_standing(Some(&mine), "linux-proc-stat:1");
        assert_eq!(standing, WatchStanding::NotThatProcess);
        assert!(
            standing.proved_gone(),
            "a mismatch is what a sweep is for: the pid is held by something else"
        );
        // No reading at all, against a record that carries one.
        let standing = token_standing(None, &recorded);
        assert_eq!(
            standing,
            WatchStanding::Unproven,
            "a token this host would not give was read as a mismatch, which is a live \
             watcher's record erased on a reading nobody took"
        );
        assert!(
            !standing.proved_gone(),
            "a sweep would remove the record of a watcher this host merely could not \
             judge, and nothing could ever put it back"
        );
        assert!(!standing.is_live(), "and it is still not a live watch");
        // And a record that carries no token, whatever was read.
        for read in [Some(&mine), None] {
            let standing = token_standing(read, "");
            assert_eq!(standing, WatchStanding::Unproven);
            assert!(
                !standing.proved_gone(),
                "a record with nothing to judge was swept"
            );
        }
    }

    /// The name this build gives a record is the one entry 68 states.
    ///
    /// The entry is the only place the file name is written down — the contract
    /// names none of this — and a second party reading that directory finds the
    /// records by that shape. So the shape is driven from the entry's own block
    /// rather than restated here: the directory, the pid before the separator, a
    /// nonce of at least the hexadecimal characters it asks for, and the suffix.
    #[test]
    fn the_divergence_entry_names_the_file_this_build_writes() {
        let block = crate::unwatched::tests::block();
        let paths = RunPaths::under(std::path::Path::new("/runs"), "gated");
        let minted = nonce();
        let path = paths.watcher(4_242, &minted);

        assert_eq!(
            path.parent().and_then(|dir| dir.file_name()),
            block["record_directory"].as_str().map(std::ffi::OsStr::new),
            "entry 68 names a different directory than this build writes into"
        );
        let named = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("the record has a name");
        let shape = block["record_name"]
            .as_str()
            .expect("entry 68 names the file it writes");
        let (before, after) = shape
            .split_once("<nonce>")
            .expect("the entry's shape names the nonce");
        let before = before.replace("<pid>", "4242");
        let held = named
            .strip_prefix(&before)
            .and_then(|rest| rest.strip_suffix(after))
            .unwrap_or_else(|| {
                panic!("`{named}` is not the `{shape}` entry 68 states, for pid 4242")
            });
        assert_eq!(
            held, minted,
            "the name carries something other than the nonce"
        );
        let least = usize::try_from(
            block["nonce_hex_at_least"]
                .as_u64()
                .expect("entry 68 says how long the nonce is at least"),
        )
        .expect("a length");
        assert!(
            held.len() >= least && held.chars().all(|c| c.is_ascii_hexdigit()),
            "`{held}` is not the at-least-{least} hexadecimal characters entry 68 asks for"
        );

        assert_ne!(nonce(), nonce());
    }

    /// A name is told apart by something that cannot be absent, so a clock this
    /// host would not read takes nothing away from it.
    ///
    /// The regression this stands on is exact. The value this replaced was the
    /// wall clock with a zero written in where it could not be read, beside a
    /// counter that restarts with the process — so on a host whose clock answers
    /// nothing, every first watch of every process minted the *same* name, and the
    /// identity was unique per process where it has to be unique per watch. Driven
    /// here rather than through a journey because no journey on a host whose clock
    /// works can stage the reading: [`minted`] takes it as a value for exactly
    /// that.
    ///
    /// Both halves are asserted, because both are the defect: the same counter
    /// with **no** clock, which is the host that broke it, and the same counter
    /// with the **same** clock, which is what two processes on a stopped one see.
    #[test]
    fn two_watches_that_read_no_clock_at_all_are_still_two_names() {
        assert_ne!(
            minted(7, None),
            minted(7, None),
            "a clock this host would not read left the name a function of the counter, and a \
             counter restarts with the process"
        );
        assert_ne!(
            minted(0, Some(1_757_000_000_000_000_000)),
            minted(0, Some(1_757_000_000_000_000_000)),
            "two watches that read one instant composed one name"
        );
        // And it is still the shape entry 68 states, whatever the clock said.
        for name in [minted(0, None), minted(u64::MAX, Some(u128::MAX))] {
            assert!(
                name.len() >= 8 && name.chars().all(|c| c.is_ascii_hexdigit()),
                "`{name}` is not the hexadecimal name entry 68 asks for"
            );
        }
    }

    /// The instant this crate's own writer produces is one, and the shapes RFC
    /// 3339 also allows are too.
    ///
    /// The reason this is a test rather than a glance: the check is what stands
    /// between a record and a reader, and one written too tightly would refuse a
    /// record another build wrote correctly — which reads as "nothing is watching
    /// this run" and re-arms a watch that was already there.
    #[test]
    fn an_instant_is_one_in_every_shape_the_grammar_allows() {
        for shape in [
            "2026-09-09T18:21:04.123Z",
            "2026-09-09T18:21:04Z",
            "2026-09-09t18:21:04z",
            "2026-09-09T18:21:04+02:00",
            "2026-09-09T18:21:04-06:30",
            "2026-09-09T18:21:04.000000001-06:30",
            // The three the ranges have to admit rather than round off: the leap
            // second the grammar has, and a leap day in a year that has one — a
            // century among them, which the Gregorian rule keeps.
            "2026-12-31T23:59:60Z",
            "2024-02-29T18:21:04Z",
            "2000-02-29T18:21:04Z",
        ] {
            assert!(is_rfc3339(shape), "`{shape}` is an RFC 3339 instant");
        }
        assert!(
            is_rfc3339(&sys::now_rfc3339()),
            "this crate's own writer does not produce one: {}",
            sys::now_rfc3339()
        );
    }

    /// An instant reads as the moment it names, whatever offset it is spelled in,
    /// and as exactly the moment this crate's own writer rendered.
    ///
    /// The deadline a wake budget is compared against is read by this, so an
    /// offset read the wrong way round would pass a watch that gives up an hour
    /// late or fail one that is on time.
    #[test]
    fn an_instant_reads_as_the_moment_it_names() {
        assert_eq!(instant_millis("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(instant_millis("1970-01-01T01:00:00+01:00"), Some(0));
        assert_eq!(instant_millis("1969-12-31T18:30:00-05:30"), Some(0));
        assert_eq!(instant_millis("1970-01-01T00:00:01.5z"), Some(1_500));
        assert_eq!(instant_millis("1970-01-01T00:00:00.123456Z"), Some(123));
        for millis in [0, 951_782_400_000, 1_790_700_724_947, LATEST_INSTANT_MS] {
            assert_eq!(
                instant_millis(&sys::rfc3339_from_millis(millis)),
                Some(i128::from(millis)),
                "{}",
                sys::rfc3339_from_millis(millis)
            );
        }
        assert_eq!(instant_millis("2026-02-30T00:00:00Z"), None);
        assert_eq!(instant_millis("soon"), None);
    }

    /// And what is not one is refused, including the near misses.
    #[test]
    fn what_is_not_an_instant_is_refused() {
        for shape in [
            "",
            "now",
            "2026-09-09",
            "18:21:04Z",
            "2026-9-9T18:21:04Z",
            "2026-09-09T18:21Z",
            "2026-09-09T18:21:04",
            "2026-09-09T18:21:04.Z",
            "2026-09-09T18:21:04+2:00",
            "2026-09-09Txx:21:04Z",
            "../../etc/passwd",
            // The shape, without being an instant: a month, a day, an hour, a
            // minute, a second and an offset no clock has, and a day its own month
            // does not.
            "2026-99-09T18:21:04Z",
            "2026-09-99T18:21:04Z",
            "2026-09-09T99:21:04Z",
            "2026-09-09T18:99:04Z",
            "2026-09-09T18:21:61Z",
            "2026-09-09T18:21:04+99:00",
            "2026-09-09T18:21:04+02:99",
            "2026-02-30T18:21:04Z",
            "2025-02-29T18:21:04Z",
            "2100-02-29T18:21:04Z",
        ] {
            assert!(!is_rfc3339(shape), "`{shape}` is not an RFC 3339 instant");
        }
    }

    /// A record whose stamp is not an instant is refused as a record, which is
    /// what puts the check at the boundary rather than beside it.
    #[test]
    fn a_record_whose_stamp_is_not_an_instant_is_not_a_record() {
        let document = |began_at: &str| {
            serde_json::json!({
                "schema_version": WATCHER_SCHEMA_VERSION,
                "run_id": "gated",
                "pid": 4_242,
                "host": "a-host",
                "started": "linux-proc-stat:1",
                "began_at": began_at,
            })
        };
        serde_json::from_value::<WatcherRecord>(document("2026-09-09T18:21:04.123Z"))
            .expect("a record this build's own writer would have written");
        let refusal = serde_json::from_value::<WatcherRecord>(document("some time yesterday"))
            .expect_err("a stamp that is not an instant is refused");
        assert!(
            refusal.to_string().contains("began_at"),
            "the refusal does not say what it refused: {refusal}"
        );
    }
}
