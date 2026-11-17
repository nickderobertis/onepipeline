// The digest the schema-bundle golden records each schema by, written once and included by
// the two tests that verify a row of it: `tests/engine.rs`, for what the engine emits, and the
// binary's own `crates/onetaskgraph/src/main.rs`, for the roots it adds. Two spellings of it
// would be two goldens that could digest one schema two ways.

/// One root's schema rendered so that two equal documents render equally.
///
/// Object keys sorted at every depth, so nothing about the order `schemars` happened to
/// build a map in reaches the digest. Arrays keep their order, because in JSON Schema an
/// array's order is part of what it says.
fn canonical(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(fields) => {
            let mut keys: Vec<&String> = fields.keys().collect();
            keys.sort_unstable();
            let rendered: Vec<String> = keys
                .iter()
                .map(|key| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(key).expect("a key renders"),
                        canonical(&fields[*key])
                    )
                })
                .collect();
            format!("{{{}}}", rendered.join(","))
        }
        serde_json::Value::Array(items) => {
            let rendered: Vec<String> = items.iter().map(canonical).collect();
            format!("[{}]", rendered.join(","))
        }
        other => serde_json::to_string(other).expect("a scalar renders"),
    }
}

/// A change-detecting digest of one root's emitted schema.
///
/// FNV-1a over the canonical rendering above, written out here rather than taken from a
/// crate: what this has to catch is a schema that changed without the version moving, and
/// any digest that changes when its input does catches that. It defends against nothing
/// adversarial and does not pretend to — whoever can edit a row of the table above can edit
/// the digest beside it, which the table already says of itself.
fn digest(schema: &serde_json::Value) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in canonical(schema).as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
