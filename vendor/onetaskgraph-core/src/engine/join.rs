//! Asking every source at once.
//!
//! The engine addresses one, several or every configured source, and it must do so
//! concurrently: a query over Linear and a folder of Markdown must not cost the sum of
//! the two, and a slow source must not hold a fast one back. That needs one thing a
//! futures library would otherwise be pulled in for — driving a `Vec` of futures at
//! once — so it is written here instead.
//!
//! It is deliberately small and runtime-agnostic. `deny.toml` refuses every cache and
//! index crate in this workspace for the invariant's sake, and each dependency added to
//! reach one combinator is a dependency the supply-chain gate then has to reason about.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Drive every future concurrently, answering in the order they were given.
pub(crate) fn join_all<F: Future>(futures: Vec<F>) -> JoinAll<F> {
    let answers = futures.iter().map(|_| None).collect();
    JoinAll {
        running: futures
            .into_iter()
            .map(|future| Some(Box::pin(future)))
            .collect(),
        answers,
    }
}

/// Every future in flight together, and the answers already in.
pub(crate) struct JoinAll<F: Future> {
    /// Emptied as each future completes, because polling a completed future is undefined
    /// behaviour on the caller's part and this one polls everything on every wake-up.
    running: Vec<Option<Pin<Box<F>>>>,
    /// Positionally matched to `running`, which is what makes the answers come back in
    /// the order the futures were given rather than the order they finished.
    answers: Vec<Option<F::Output>>,
}

/// Moving this future moves two `Vec` headers and nothing else: each driven future is
/// pinned inside its own box on the heap, and each answer sits behind the same
/// indirection. So nothing here depends on staying at a fixed address, which is what
/// `Unpin` means — and saying so is what lets [`Future::poll`] below reach its own fields
/// without `unsafe`.
impl<F: Future> Unpin for JoinAll<F> {}

impl<F: Future> Future for JoinAll<F> {
    type Output = Vec<F::Output>;

    /// Poll every future that has not finished, passing this task's own waker to each.
    ///
    /// Polling all of them on every wake-up rather than tracking which one woke us is
    /// what keeps this small enough to be worth owning: with one future per configured
    /// source the redundant polls are a handful, and the alternative is per-future waker
    /// plumbing whose only benefit shows up at a scale this engine never reaches.
    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        // Reachable without `unsafe` because of the `Unpin` above.
        let this = self.get_mut();
        let mut waiting = false;
        for (slot, answer) in this.running.iter_mut().zip(this.answers.iter_mut()) {
            let Some(future) = slot.as_mut() else {
                continue;
            };
            match future.as_mut().poll(context) {
                Poll::Ready(value) => {
                    *answer = Some(value);
                    *slot = None;
                }
                Poll::Pending => waiting = true,
            }
        }
        if waiting {
            return Poll::Pending;
        }
        Poll::Ready(
            this.answers
                .iter_mut()
                .map(|answer| answer.take().expect("every future answered"))
                .collect(),
        )
    }
}
