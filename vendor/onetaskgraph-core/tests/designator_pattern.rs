//! The drift gate between the pattern the schema bundle publishes for a designator and what the
//! contract type accepts: both SDKs validate against the first, and the store reads with the
//! second, so a string one admits and the other refuses is a stored edge one reader drops.

use onetaskgraph_core::schema_bundle;
use onetaskgraph_plugin_api::Designator;

/// Every sample is accepted by the published pattern exactly when `Designator` accepts it.
#[test]
fn the_published_designator_pattern_accepts_exactly_what_the_type_does() {
    let bundle = schema_bundle();
    // Both places the bundle publishes it: the designator a dependency read reports unresolved,
    // and the opaque form of a stored endpoint.
    let reported = bundle["roots"]["QueryResponseOfQualifiedEdge"]["$defs"]["Designator"].clone();
    let stored =
        bundle["roots"]["DependencyEdge"]["$defs"]["DependencyEndpoint"]["oneOf"][2]["properties"]
            ["designator"]
            .clone();
    assert!(
        reported.is_object() && stored.is_object(),
        "the bundle publishes the designator in both places"
    );
    let validators =
        [reported, stored].map(|schema| jsonschema::validator_for(&schema).expect("a schema"));
    let hex = "0123456789abcdef".repeat(4);
    let samples = [
        format!("linear/{hex}"),
        format!("github-projects/{hex}"),
        format!("x/{hex}"),
        format!("9/{hex}"),
        format!("Linear/{hex}"),
        format!("-linear/{hex}"),
        format!("linear/{}", &hex[1..]),
        format!("linear/{}", hex.to_uppercase()),
        format!("linear/{hex}0"),
        format!("lin_ear/{hex}"),
        "alias/planning_vault-2026".to_owned(),
        "alias/short".to_owned(),
        format!("alias/{}", "a".repeat(64)),
        format!("alias/{}", "a".repeat(65)),
        "alias/has.dots.in.it.ok".to_owned(),
        "no-slash".to_owned(),
        String::new(),
    ];
    for sample in samples {
        let typed = Designator::try_from(sample.clone()).is_ok();
        for validator in &validators {
            let published = validator.is_valid(&serde_json::Value::String(sample.clone()));
            assert_eq!(
                published, typed,
                "{sample:?}: pattern {published}, type {typed}"
            );
        }
    }
}
