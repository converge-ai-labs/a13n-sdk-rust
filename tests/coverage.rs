mod common;
include!("../src/generated/resource_tests.rs");

#[test]
fn inventory_covers_the_exact_pinned_operations() {
    use std::collections::BTreeSet;
    let document: serde_json::Value =
        serde_json::from_str(include_str!("../contract/openapi.json")).unwrap();
    let mut expected = BTreeSet::new();
    for (path, item) in document["paths"].as_object().unwrap() {
        for method in item.as_object().unwrap().keys() {
            if ["get", "post", "put", "patch", "delete"].contains(&method.as_str()) {
                expected.insert((method.to_uppercase(), path.clone()));
            }
        }
    }
    let actual: BTreeSet<_> = RESOURCE_OPERATIONS
        .iter()
        .map(|(method, path, _, _)| (method.to_string(), path.to_string()))
        .collect();
    assert_eq!(expected, actual);
    assert_eq!(actual.len(), RESOURCE_OPERATIONS.len());
}
