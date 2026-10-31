//! Export the engine-owned human-item schemas to stdout.
fn main() {
    println!(
        "{}",
        serde_json::to_string_pretty(&onepipeline::human_items::schemas())
            .expect("schemas serialize")
    );
}
