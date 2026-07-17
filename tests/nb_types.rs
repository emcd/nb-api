use nb_api::{EditMode, SearchMode, TaskStatus};

#[test]
fn edit_mode_canonical_serialization_is_overwrite() {
    // Canonical serialization must be the unambiguous destructive
    // name. This is the primary defense against the vocabulary trap
    // (nb-api:issues/api/6).
    let canonical = serde_json::to_string(&EditMode::Overwrite).unwrap();
    assert_eq!(
        canonical, "\"overwrite\"",
        "EditMode::Overwrite must serialize as the canonical 'overwrite'"
    );
    assert_eq!(
        serde_json::to_string(&EditMode::Append).unwrap(),
        "\"append\""
    );
    assert_eq!(
        serde_json::to_string(&EditMode::Prepend).unwrap(),
        "\"prepend\""
    );
}

#[test]
fn edit_mode_deserializes_canonical_overwrite() {
    let mode: EditMode = serde_json::from_str("\"overwrite\"").unwrap();
    assert_eq!(mode, EditMode::Overwrite);
}

#[test]
fn edit_mode_deserializes_legacy_replace_as_alias() {
    // Backward compatibility: pre-rename payloads that contain
    // mode: "replace" must continue to deserialize as the
    // destructive mode (now called Overwrite).
    let mode: EditMode = serde_json::from_str("\"replace\"").unwrap();
    assert_eq!(mode, EditMode::Overwrite);
}

#[test]
fn edit_mode_deserializes_append_and_prepend() {
    let mode: EditMode = serde_json::from_str("\"append\"").unwrap();
    assert_eq!(mode, EditMode::Append);
    let mode: EditMode = serde_json::from_str("\"prepend\"").unwrap();
    assert_eq!(mode, EditMode::Prepend);
}

#[test]
fn edit_mode_rejects_unknown_values() {
    // Unknown mode values must fail to deserialize. This guards
    // against typo'd strings like "overwrite " (trailing space),
    // "Overwrite" (wrong case), or "delete".
    for bad in ["\"Overwrite\"", "\"REPLACE\"", "\"delete\"", "\"\""] {
        let result: Result<EditMode, _> = serde_json::from_str(bad);
        assert!(
            result.is_err(),
            "EditMode should reject {bad:?}, got {:?}",
            result.ok()
        );
    }
}

#[test]
fn edit_mode_default_is_overwrite() {
    // Pin the default variant to `Overwrite` so any future move
    // of `#[default]` to Append/Prepend is caught here, not as a
    // silent behavior change at consumer sites that rely on the
    // current default.
    assert_eq!(EditMode::default(), EditMode::Overwrite);
}

#[cfg(feature = "schemars")]
#[test]
fn edit_mode_schema_advertises_overwrite_canonical_only() {
    // The derived JSON Schema must advertise only the canonical
    // value `overwrite`. The legacy `replace` alias exists for
    // serde backward compat but MUST NOT appear in the schema
    // (otherwise MCP tool consumers will see both values and the
    // vocabulary trap is re-introduced at the schema layer).
    //
    // Schemars emits enum schemas as `oneOf` with each variant
    // as `{ "type": "string", "const": <variant_value> }`.
    use schemars::schema_for;
    let schema = schema_for!(EditMode);
    let schema_json = serde_json::to_value(&schema).unwrap();
    let one_of = schema_json
        .pointer("/oneOf")
        .and_then(|v| v.as_array())
        .expect("EditMode schema must be a oneOf schema");
    let variant_strings: Vec<&str> = one_of
        .iter()
        .filter_map(|variant| variant.pointer("/const").and_then(|v| v.as_str()))
        .collect();
    assert!(
        variant_strings.contains(&"overwrite"),
        "EditMode schema must advertise canonical 'overwrite'; got {variant_strings:?}"
    );
    assert!(
        variant_strings.contains(&"append"),
        "EditMode schema must advertise 'append'; got {variant_strings:?}"
    );
    assert!(
        variant_strings.contains(&"prepend"),
        "EditMode schema must advertise 'prepend'; got {variant_strings:?}"
    );
    assert!(
        !variant_strings.contains(&"replace"),
        "EditMode schema MUST NOT advertise legacy 'replace' alias; got {variant_strings:?}"
    );
}

#[test]
fn task_status_deserializes_lowercase_values() {
    let status: TaskStatus = serde_json::from_str("\"open\"").unwrap();
    assert_eq!(status, TaskStatus::Open);
    let status: TaskStatus = serde_json::from_str("\"closed\"").unwrap();
    assert_eq!(status, TaskStatus::Closed);
}

#[test]
fn search_mode_deserializes_lowercase_values() {
    let mode: SearchMode = serde_json::from_str("\"any\"").unwrap();
    assert_eq!(mode, SearchMode::Any);
    let mode: SearchMode = serde_json::from_str("\"all\"").unwrap();
    assert_eq!(mode, SearchMode::All);
}
