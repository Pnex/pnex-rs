//! The mirror's own types read the public contract (external-apps.md
//! Q-A5): every field it relies on exists in `docs/contracts/openapi.json`.

use serde_json::Value;

fn properties<'a>(spec: &'a Value, schema: &str) -> Vec<&'a str> {
    spec["components"]["schemas"][schema]["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("schema {schema} missing from the contract"))
        .keys()
        .map(String::as_str)
        .collect()
}

#[test]
fn the_fields_read_exist_in_the_contract() {
    let raw = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/contracts/openapi.json"
    ))
    .unwrap();
    let spec: Value = serde_json::from_str(&raw).unwrap();
    for (schema, fields) in [
        ("ChangeFeed", &["changes", "next", "more"][..]),
        (
            "FeedEntry",
            &[
                "cursor", "kind", "action", "id", "type_key", "object", "link",
            ],
        ),
        (
            "ObjectView",
            &[
                "id",
                "type_key",
                "title",
                "native_id",
                "version",
                "properties",
                "source_ref",
                "valid_from",
                "valid_to",
                "updated_at",
            ],
        ),
        (
            "LinkView",
            &[
                "id",
                "link_type",
                "source",
                "target",
                "attributes",
                "source_ref",
                "valid_from",
                "valid_to",
            ],
        ),
        ("ObjectRef", &["id"]),
        ("AppView", &["scope"]),
        (
            "AppScope",
            &["types", "link_types", "media_kinds", "media_max_bytes"],
        ),
        ("MediaAssetDto", &["kind", "size_bytes", "content_type"]),
        ("MediaVersionDto", &["version_number", "sha256", "current"]),
    ] {
        let have = properties(&spec, schema);
        for f in fields {
            assert!(
                have.contains(f),
                "{schema}.{f} is gone from the contract: adapt pnex-mirror"
            );
        }
    }
    let paths = spec["paths"].as_object().unwrap();
    for p in [
        "/api/v1/ontology/changes",
        "/api/v1/apps/me",
        "/api/v1/media/{id}",
        "/api/v1/media/{id}/versions",
        "/api/v1/media/{id}/content",
    ] {
        assert!(paths.contains_key(p), "{p} is gone from the contract");
    }
}
