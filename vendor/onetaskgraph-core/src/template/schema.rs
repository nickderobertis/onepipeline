// llmlint: ignore-file[code_lands_in_the_domain_that_owns_it] A part of the `template` module; why that module sits in this crate is stated once, at the head of its `mod.rs`.
//! The JSON Schema an `object` or `list` variable may declare its answers to: compiled as
//! draft 2020-12, with no remote reference resolved, and an answer held to it after it is
//! held to the variable's type.

use serde_json::Value;

/// Where an answer first fails its variable's schema, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Violation {
    /// The JSON pointer (RFC 6901) of the first failing value inside the answer: `""` for the
    /// answer as a whole.
    pub pointer: String,
    /// What the schema says is wrong there.
    pub problem: String,
}

/// Compile `schema` as draft 2020-12, refusing it as the validator words why.
pub(super) fn compile(schema: &Value) -> Result<jsonschema::Validator, String> {
    jsonschema::draft202012::new(schema).map_err(|error| error.to_string())
}

/// Hold `value` to `schema`, answering the first violation in the validator's order.
///
/// A schema that does not compile is refused where it is declared, so one reaching here
/// compiles; were it not to, the answer is refused rather than let through.
pub(super) fn conform(schema: &Value, value: &Value) -> Result<(), Violation> {
    let validator = compile(schema).map_err(|problem| Violation {
        pointer: String::new(),
        problem,
    })?;
    match validator.iter_errors(value).next() {
        None => Ok(()),
        Some(error) => Err(Violation {
            pointer: error.instance_path().as_str().to_owned(),
            problem: error.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_first_violation_is_named_by_its_json_pointer() {
        let schema = json!({
            "type": "object",
            "required": ["budgets"],
            "properties": {"budgets": {"type": "array", "items": {"type": "object", "required": ["id"]}}}
        });
        assert_eq!(conform(&schema, &json!({"budgets": [{"id": "a"}]})), Ok(()));
        let violation = conform(&schema, &json!({"budgets": [{"id": "a"}, {"name": "b"}]}))
            .expect_err("the second entry has no id");
        assert_eq!(violation.pointer, "/budgets/1");
        assert!(violation.problem.contains("id"), "{}", violation.problem);
        let whole = conform(&schema, &json!({})).expect_err("no budgets");
        assert_eq!(whole.pointer, "");
    }

    #[test]
    fn a_schema_that_does_not_compile_is_refused() {
        assert!(compile(&json!({"type": "no-such-type"})).is_err());
        assert!(compile(&json!({"type": "object"})).is_ok());
    }
}
