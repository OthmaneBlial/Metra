use std::collections::BTreeSet;

use serde_json::Value;

const MATRIX: &str = include_str!("../compat/exiftool-compatibility.json");
const CAPABILITY_FIELDS: [&str; 6] = [
    "read",
    "write",
    "create",
    "delete",
    "lossless_rewrite",
    "streaming",
];

fn compatibility_matrix() -> Value {
    serde_json::from_str(MATRIX).expect("compatibility matrix should be valid JSON")
}

fn status_name<'a>(value: &'a Value, context: &str) -> &'a str {
    let status = value
        .as_str()
        .unwrap_or_else(|| panic!("{context} should be a string status"));
    assert!(
        matches!(
            status,
            "supported" | "mostly-supported" | "partial" | "planned" | "unsupported"
        ),
        "{context} has unknown status {status:?}"
    );
    status
}

fn object_keys(value: &Value, context: &str) -> BTreeSet<String> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("{context} should be an object"))
        .keys()
        .cloned()
        .collect()
}

#[test]
fn compatibility_matrix_tracks_public_format_capabilities() {
    let matrix = compatibility_matrix();
    assert_eq!(matrix["schema_version"], 1);
    assert_eq!(
        matrix["project_version"].as_str(),
        Some(env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(
        matrix["policy"].as_str(),
        Some("Only behavior covered by tests is marked supported.")
    );

    let formats = &matrix["formats"];
    let matrix_formats = object_keys(formats, "formats");
    let api_formats: BTreeSet<String> = metra::format_capabilities_all()
        .iter()
        .map(|capabilities| capabilities.format.to_string())
        .collect();
    assert_eq!(matrix_formats, api_formats);

    for capabilities in metra::format_capabilities_all() {
        let format_name = capabilities.format.to_string();
        let entry = &formats[&format_name];
        let serialized = serde_json::to_value(capabilities)
            .expect("public format capabilities should serialize");
        for field in CAPABILITY_FIELDS {
            let actual = status_name(&entry[field], &format!("{format_name}.{field}"));
            let expected = serialized[field]
                .as_str()
                .expect("public capability status should serialize as a string");
            assert_eq!(
                actual, expected,
                "compatibility matrix drift for {format_name}.{field}"
            );
        }
    }
}

#[test]
fn compatibility_matrix_sections_use_known_statuses() {
    let matrix = compatibility_matrix();
    for section in ["metadata_families", "maker_notes", "cli"] {
        let value = &matrix[section];
        let object = value
            .as_object()
            .unwrap_or_else(|| panic!("{section} should be an object"));
        assert!(!object.is_empty(), "{section} should not be empty");
        for (name, status) in object {
            status_name(status, &format!("{section}.{name}"));
        }
    }
}
