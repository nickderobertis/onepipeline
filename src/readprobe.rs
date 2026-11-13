//! Passive, thread-local observations of real journal reads and folds in tests.
use std::cell::RefCell;
use std::path::{Path, PathBuf};

use crate::event::Envelope;

#[derive(Default, Debug)]
pub struct Observed {
    pub parsed: Vec<(PathBuf, u64)>,
    pub folded: Vec<(String, u64)>,
    pub reads: Vec<(PathBuf, u64, u64)>,
}

thread_local! {
    static OBSERVED: RefCell<Option<Observed>> = const { RefCell::new(None) };
}

pub fn start() {
    OBSERVED.with_borrow_mut(|observed| *observed = Some(Observed::default()));
}

pub fn finish() -> Observed {
    OBSERVED.with_borrow_mut(|observed| observed.take().expect("observation was started"))
}

pub fn parsed(path: &Path, offset: u64) {
    OBSERVED.with_borrow_mut(|observed| {
        if let Some(observed) = observed {
            observed.parsed.push((path.to_owned(), offset));
        }
    });
}

pub fn folded(event: &Envelope) {
    OBSERVED.with_borrow_mut(|observed| {
        if let Some(observed) = observed {
            observed.folded.push((event.stream.clone(), event.seq));
        }
    });
}

pub fn read(path: &Path, offset: u64, bytes: u64) {
    OBSERVED.with_borrow_mut(|observed| {
        if let Some(observed) = observed {
            observed.reads.push((path.to_owned(), offset, bytes));
        }
    });
}
