use oneharness_core::sdk_schema;

fn main() -> Result<(), serde_json::Error> {
    let bundle = sdk_schema::bundle();
    println!("{}", serde_json::to_string_pretty(&bundle)?);
    Ok(())
}
