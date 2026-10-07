//! The result a `reported` budget's command hands `onebudgetspec`: one JSON object, written to
//! the file `ONEBUDGETSPEC_RESULT` names. Built here and nowhere else, so the journey that writes
//! it and `tests/budgets.rs`, which has the pinned checker's own library parse it, hold one shape.

use serde_json::{json, Value};

/// The result of a measurement that came to `value`, with `detail` saying how.
pub fn reported(value: usize, detail: &str) -> Value {
    json!({"value": value, "detail": detail})
}
