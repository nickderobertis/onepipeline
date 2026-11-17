//! Pure scheduling for same-prefix batch runs: one harness driven over N prompts
//! that share a cacheable prefix (the same `--system` + model + tool config, so
//! the provider's prompt cache keys on a byte-identical request prefix). The
//! runner spawns; this module only decides the *order* the calls are issued in,
//! so the policy is testable without a real cache.
//!
//! Two strategies, each expressed as a list of execution *waves* — sets of prompt
//! indices run concurrently, with a barrier between successive waves:
//! - `speed` fans every prompt out at once (one wave): minimum wall-clock, but
//!   each call may race to *write* the shared prefix to cache before any exists.
//! - `min-tokens` issues exactly one warm-up call first (its own wave) to *write*
//!   the prefix, then — once it completes — fans the rest out so they *read* the
//!   warmed prefix instead of each re-writing it: fewer tokens at some wall-clock
//!   cost. The barrier between the warm-up and the fan-out is the whole point, so
//!   it lives here in the schedule, not in the spawning layer.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How a batch of same-prefix prompts is scheduled across the parallel runner.
/// Also accepted as a CLI value (`--batch-strategy`, parsed in the `oneharness`
/// binary) — the parsing lives there so this core crate stays free of `clap`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum BatchStrategy {
    /// Fire all prompts at once (a single wave): minimum wall-clock. Every call
    /// may race to *write* the shared prefix to the provider cache (none exists
    /// yet), so this optimizes latency, not tokens. The default.
    Speed,
    /// Warm-then-fan: run one prompt first to *write* the shared prefix to cache,
    /// wait for it, then fan the rest out concurrently so they *read* the warmed
    /// prefix instead of each re-writing it. Optimizes cost at some wall-clock
    /// expense.
    MinTokens,
}

impl BatchStrategy {
    /// Every strategy, for the CLI's possible-value list.
    pub const ALL: [BatchStrategy; 2] = [BatchStrategy::Speed, BatchStrategy::MinTokens];

    /// The CLI/JSON token for this strategy.
    pub fn as_str(self) -> &'static str {
        match self {
            BatchStrategy::Speed => "speed",
            BatchStrategy::MinTokens => "min-tokens",
        }
    }

    /// Parse a CLI token into a strategy; `None` for an unrecognized token.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "speed" => Some(BatchStrategy::Speed),
            "min-tokens" => Some(BatchStrategy::MinTokens),
            _ => None,
        }
    }
}

/// The execution waves for `n` prompts under `strategy`. Each inner vec is a set
/// of 0-based prompt indices to run concurrently; the waves run in order with a
/// barrier between them. `speed` is a single wave of everything; `min-tokens`
/// issues exactly one warm-up call (index 0) and, once it completes, a second
/// wave with the rest. Empty when `n == 0`. The union of all waves is always
/// `0..n` with no index repeated, so a caller can place every prompt's outcome.
pub fn waves(strategy: BatchStrategy, n: usize) -> Vec<Vec<usize>> {
    if n == 0 {
        return Vec::new();
    }
    match strategy {
        BatchStrategy::Speed => vec![(0..n).collect()],
        BatchStrategy::MinTokens => {
            if n == 1 {
                // Nothing to warm for: one call is both the write and the answer.
                vec![vec![0]]
            } else {
                vec![vec![0], (1..n).collect()]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strategy_token_round_trips_for_every_variant() {
        for s in BatchStrategy::ALL {
            assert_eq!(BatchStrategy::parse(s.as_str()), Some(s));
        }
        assert_eq!(BatchStrategy::parse("nope"), None);
        // The serialized form matches the CLI token (kebab-case).
        assert_eq!(
            serde_json::to_string(&BatchStrategy::MinTokens).unwrap(),
            "\"min-tokens\""
        );
        assert_eq!(
            serde_json::to_string(&BatchStrategy::Speed).unwrap(),
            "\"speed\""
        );
    }

    #[test]
    fn speed_is_always_a_single_wave_of_everything() {
        assert_eq!(waves(BatchStrategy::Speed, 1), vec![vec![0]]);
        assert_eq!(waves(BatchStrategy::Speed, 3), vec![vec![0, 1, 2]]);
        assert_eq!(waves(BatchStrategy::Speed, 5), vec![vec![0, 1, 2, 3, 4]]);
    }

    #[test]
    fn min_tokens_warms_one_then_fans_the_rest() {
        // One prompt: a single call (no separate warm-up to wait on).
        assert_eq!(waves(BatchStrategy::MinTokens, 1), vec![vec![0]]);
        // Two+: exactly one warm-up wave, then the remainder fanned out.
        assert_eq!(waves(BatchStrategy::MinTokens, 2), vec![vec![0], vec![1]]);
        assert_eq!(
            waves(BatchStrategy::MinTokens, 4),
            vec![vec![0], vec![1, 2, 3]]
        );
        // The defining property: the first wave issues exactly one call.
        assert_eq!(waves(BatchStrategy::MinTokens, 4)[0].len(), 1);
    }

    #[test]
    fn zero_prompts_yields_no_waves_for_either_strategy() {
        assert!(waves(BatchStrategy::Speed, 0).is_empty());
        assert!(waves(BatchStrategy::MinTokens, 0).is_empty());
    }

    #[test]
    fn waves_cover_every_index_exactly_once() {
        // The placement contract the runner relies on: every prompt appears in
        // exactly one wave, so no outcome slot is left unfilled or double-written.
        for strategy in BatchStrategy::ALL {
            for n in 0..6 {
                let mut seen: Vec<usize> = waves(strategy, n).into_iter().flatten().collect();
                seen.sort_unstable();
                assert_eq!(
                    seen,
                    (0..n).collect::<Vec<_>>(),
                    "strategy {strategy:?} n {n}"
                );
            }
        }
    }
}
