//! One factor for every interval, poll, backoff, takeover window and deadline this binary
//! waits out, so a journey proves an ordering in a fraction of the production wall clock.
//!
//! [`TIME_SCALE_ENV`] is a **test setting of the debug build**, like the
//! `ONEPIPELINE_TEST_*_GATE` failpoints beside it, and no part of the contract: a build
//! without debug assertions — every published binary — never reads it, so nothing an
//! operator runs can be scaled. A debug build reads it once per process.
//!
//! **Precedence, everywhere.** The scale multiplies a wait's *shipped default* and every
//! fixed wait that has no setting of its own. A wait named exactly — through its own
//! variable, its flag, its config key, or a value a run's records carry from the launch that
//! chose it — is applied as named and never scaled: a value written down is the value meant,
//! so a journey that pins one interval and scales the rest gets exactly the one it pinned.
//!
//! **What it scales** is every wait a journey sits out on a shipped default or a fixed
//! constant: the write-back's floor, default per-item budget and retry schedule; the claim
//! interval, the takeover slack, the claim-read deadline, the fence retry and the settlement
//! landing wait; the cancel grace and the stall threshold; and the release and source polls.
//! A wait no journey sits out — each journey that reaches one names its own value — keeps its
//! shipped behaviour whatever the scale says, so the scale buys no journey anything there and
//! changes nothing it would have to prove: the release surface and holdup, the workspace poll,
//! the maintenance pace, the merge-path and boundary backoffs, the check follow and the fence
//! linger. Neither does it scale a sub-second tick that only paces how soon a change is seen.
//!
//! **Unset, or `1`, is today's behaviour bit for bit**: [`scaled`] and [`scaled_secs`] hand
//! back exactly what they were given. Anything else must be a positive, finite number, and
//! [`check`] refuses one that is not by the variable's name before any verb runs.
//!
//! Whole-second waits — the ones a record states, such as a claim's `lease` or the
//! write-back's arithmetic — are scaled by [`scaled_secs`]: rounded to the nearest second and
//! never below one, so a scaled record is still a whole, positive number of seconds and the
//! refusal that quotes it still reads as arithmetic.

use std::sync::OnceLock;
use std::time::Duration;

use crate::error::{Error, Result};

pub(crate) const TIME_SCALE_ENV: &str = "ONEPIPELINE_TEST_TIME_SCALE";

/// The factor this process applies, or the refusal of the value it was given.
fn factor_or_refusal() -> &'static std::result::Result<f64, String> {
    static FACTOR: OnceLock<std::result::Result<f64, String>> = OnceLock::new();
    FACTOR.get_or_init(read)
}

/// A debug build's one read of the variable.
#[cfg(debug_assertions)]
fn read() -> std::result::Result<f64, String> {
    match std::env::var(TIME_SCALE_ENV) {
        Ok(setting) => parse(Some(&setting)),
        Err(std::env::VarError::NotPresent) => parse(None),
        Err(std::env::VarError::NotUnicode(setting)) => Err(format!(
            "{TIME_SCALE_ENV} must be a positive, finite number, not {setting:?}"
        )),
    }
}

/// A release build reads nothing: its waits are the shipped ones, whatever the environment
/// says.
#[cfg(not(debug_assertions))]
fn read() -> std::result::Result<f64, String> {
    Ok(1.0)
}

/// The factor a setting names: `1` for none, and a refusal naming the variable for anything
/// that is not a positive, finite number.
// Only the debug build's `read` calls it; a release build keeps it for the unit test alone.
#[cfg_attr(not(any(debug_assertions, test)), allow(dead_code))]
fn parse(setting: Option<&str>) -> std::result::Result<f64, String> {
    let Some(setting) = setting else {
        return Ok(1.0);
    };
    match setting.trim().parse::<f64>() {
        Ok(factor) if factor.is_finite() && factor > 0.0 => Ok(factor),
        _ => Err(format!(
            "{TIME_SCALE_ENV} must be a positive, finite number, not '{setting}'"
        )),
    }
}

/// Refuse a setting that is not a factor, before the verb it would have scaled runs.
pub(crate) fn check() -> Result<()> {
    factor_or_refusal()
        .as_ref()
        .map(|_| ())
        .map_err(|refusal| Error::Invalid(refusal.clone()))
}

/// The factor in force: `1` unless a debug build was given another.
fn factor() -> f64 {
    *factor_or_refusal().as_ref().unwrap_or(&1.0)
}

/// `wait` at this process's scale. `Duration::MAX` — a wait this run can never need — stays
/// what it is, and a product too large to represent is `Duration::MAX` rather than a panic.
///
/// A positive wait stays positive under any factor [`check`] accepts: it is never scaled
/// below [`FLOOR`], or below itself where it is shorter still, so a tiny factor shortens a
/// wait to one tick rather than abolishing it.
pub(crate) fn scaled(wait: Duration) -> Duration {
    scale_by(factor(), wait)
}

// Exactly one is the setting that means "unscaled", parsed from the text `1`, so an exact
// comparison is the one that hands back the very duration it was given.
#[allow(clippy::float_cmp)]
fn scale_by(factor: f64, wait: Duration) -> Duration {
    if factor == 1.0 || wait == Duration::MAX || wait.is_zero() {
        return wait;
    }
    Duration::try_from_secs_f64(wait.as_secs_f64() * factor)
        .map_or(Duration::MAX, |product| product.max(FLOOR.min(wait)))
}

/// The shortest a positive wait is scaled to: one millisecond, the unit a run's records time
/// a wait in (`at`, `duration_ms`) and the finest any scaled shipped wait is stated in (the
/// write-back's first 250 ms retry). It is one fixed tick rather than a unit read off each
/// wait, so the scale stays one linear factor above it: `1000 ms` and `1 s` scale alike.
const FLOOR: Duration = Duration::from_millis(1);

/// Whole `seconds` at this process's scale: the nearest whole second, and never below one.
pub(crate) fn scaled_secs(seconds: u64) -> u64 {
    scale_secs_by(factor(), seconds)
}

// The product is rounded and floored at one, so it is never negative; `as` saturates a
// float too large for a `u64` at `u64::MAX`, and a whole number of seconds past 2^53 loses
// precision no wait could notice.
//
// Exactly one is the setting that means "unscaled", parsed from the text `1`, so an exact
// comparison is the one that hands back the very duration it was given.
#[allow(
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn scale_secs_by(factor: f64, seconds: u64) -> u64 {
    if factor == 1.0 {
        return seconds;
    }
    ((seconds as f64 * factor).round() as u64).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_or_one_is_the_shipped_wait_exactly_and_anything_but_a_positive_finite_number_is_refused(
    ) {
        assert_eq!(parse(None), Ok(1.0));
        assert_eq!(parse(Some("1")), Ok(1.0));
        assert_eq!(parse(Some(" 0.1 ")), Ok(0.1));
        assert_eq!(parse(Some("2")), Ok(2.0));
        for refused in ["", "soon", "0", "-0.5", "NaN", "inf", "-inf", "1e400"] {
            let refusal = parse(Some(refused)).expect_err(refused);
            assert!(refusal.contains(TIME_SCALE_ENV), "{refusal}");
        }
        let odd = Duration::from_nanos(299_999_999_937);
        assert_eq!(scale_by(1.0, odd), odd);
        assert_eq!(scale_secs_by(1.0, 300), 300);
        assert_eq!(
            scale_by(0.25, Duration::from_secs(300)),
            Duration::from_secs(75)
        );
        assert_eq!(scale_by(0.1, Duration::MAX), Duration::MAX);
        assert_eq!(scale_by(1e30, Duration::from_secs(1)), Duration::MAX);
        assert_eq!(scale_secs_by(0.1, 12), 1);
        assert_eq!(scale_secs_by(0.1, 2), 1);
        assert_eq!(scale_secs_by(0.25, 10), 3);
        assert_eq!(scale_secs_by(1e30, 10), u64::MAX);
    }

    #[test]
    fn a_positive_wait_stays_at_least_a_millisecond_under_any_accepted_factor() {
        // Every shipped wait `scaled` is handed, from the 300 s claim interval to the 250 ms
        // first write-back retry. Before the floor, the smallest factors rounded each to zero.
        let shipped = [300, 60, 20, 10, 5].map(Duration::from_secs);
        for setting in ["0.25", "0.1", "0.001", "1e-6", "1e-12", "1e-300", "5e-324"] {
            let factor = parse(Some(setting)).expect(setting);
            for wait in shipped.into_iter().chain([Duration::from_millis(250)]) {
                let applied = scale_by(factor, wait);
                assert!(
                    applied >= FLOOR && applied <= wait,
                    "{wait:?} at {setting} became {applied:?}, below one millisecond"
                );
            }
        }
        // Above the floor the product is exact and linear, whatever unit the wait was
        // written in; at it, the floor is the whole wait.
        assert_eq!(
            scale_by(0.25, Duration::from_millis(1_000)),
            Duration::from_millis(250)
        );
        assert_eq!(
            scale_by(0.25, Duration::from_millis(250)),
            Duration::from_micros(62_500)
        );
        assert_eq!(scale_by(1e-12, Duration::from_secs(5)), FLOOR);
        // A wait already under the floor is held at itself, so the floor never lengthens
        // one; a factor above one never meets it, and a zero wait is not made positive.
        assert_eq!(
            scale_by(0.5, Duration::from_micros(10)),
            Duration::from_micros(10)
        );
        assert_eq!(
            scale_by(4.0, Duration::from_secs(5)),
            Duration::from_secs(20)
        );
        assert_eq!(scale_by(0.1, Duration::ZERO), Duration::ZERO);
    }
}
