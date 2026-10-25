//! Applications declared in an org (external-apps.md D433): an external
//! program built on the ontology, dressed around a service account, with
//! the scope it reads (types, link types, media). Pure validation here;
//! the server checks what needs the org (types exist, account free).

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::err_codes::{FIELD_INVALID, FIELD_MAX_LENGTH, FIELD_REQUIRED};
use crate::ontology::api::OntologyQuery;
use crate::ontology::schema::valid_key;

pub const NAME_MAX: usize = 120;
pub const DESCRIPTION_MAX: usize = 2000;
/// Most keys in one scope list.
pub const SCOPE_LIST_MAX: usize = 200;
/// Largest media an application may ask to copy (2 GiB).
pub const MEDIA_BYTES_MAX: i64 = 2 * 1024 * 1024 * 1024;
/// Most object sets of one application.
pub const OBJECT_SETS_MAX: usize = 50;
/// Media kinds of the library (`media_assets.kind`).
pub const MEDIA_KINDS: &[&str] = &[
    "photo",
    "panorama",
    "floorplan",
    "splat",
    "model",
    "table",
    "document",
    "audio",
    "video",
    "mesh",
];

/// What an application reads. The change feed and the object sets keep to
/// it (D433, Q-A1); the media fields tell its copy what to download.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AppScope {
    /// Object types (system types included, `media_asset` for media).
    #[serde(default)]
    pub types: Vec<String>,
    #[serde(default)]
    pub link_types: Vec<String>,
    /// Media kinds to copy (`photo`, `document`…); empty = none.
    #[serde(default)]
    pub media_kinds: Vec<String>,
    /// Largest media to copy, in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_max_bytes: Option<i64>,
}

/// A named query of an application (D434): an ontology query (D185) whose
/// filter values and text may name parameters, `"$code"`, given when it
/// runs. Parameters are text and only ever fill values, never keys.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ObjectSetDef {
    #[serde(default)]
    pub description: String,
    pub query: OntologyQuery,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AppView {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// The service account the application signs in with.
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub service_account_id: Option<String>,
    pub scope: AppScope,
    /// Named queries, run by `GET /apps/me/object-sets/{name}`.
    #[serde(default)]
    pub object_sets: BTreeMap<String, ObjectSetDef>,
    pub version: i32,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub updated_at: String,
}

/// Creating (`expected_version` absent) or replacing an application.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AppInput {
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub service_account_id: Option<String>,
    #[serde(default)]
    pub scope: AppScope,
    #[serde(default)]
    pub object_sets: BTreeMap<String, ObjectSetDef>,
    #[serde(default)]
    pub expected_version: Option<i32>,
}

/// Field errors of an input, `{field: token}`; empty = valid.
pub fn check(input: &AppInput) -> BTreeMap<String, String> {
    let mut errors = BTreeMap::new();
    let mut err = |field: &str, token: String| {
        errors.entry(field.to_string()).or_insert(token);
    };
    if !valid_key(&input.key) {
        err(
            "key",
            if input.key.is_empty() {
                FIELD_REQUIRED.into()
            } else {
                FIELD_INVALID.into()
            },
        );
    }
    let name = input.name.trim();
    if name.is_empty() {
        err("name", FIELD_REQUIRED.into());
    } else if name.chars().count() > NAME_MAX {
        err("name", format!("{FIELD_MAX_LENGTH}:{NAME_MAX}"));
    }
    if input.description.chars().count() > DESCRIPTION_MAX {
        err(
            "description",
            format!("{FIELD_MAX_LENGTH}:{DESCRIPTION_MAX}"),
        );
    }
    let s = &input.scope;
    for (field, list) in [
        ("scope.types", &s.types),
        ("scope.link_types", &s.link_types),
    ] {
        if list.len() > SCOPE_LIST_MAX || list.iter().any(|k| !valid_key(k)) {
            err(field, FIELD_INVALID.into());
        }
    }
    if s.media_kinds.len() > MEDIA_KINDS.len()
        || s.media_kinds
            .iter()
            .any(|k| !MEDIA_KINDS.contains(&k.as_str()))
    {
        err("scope.media_kinds", FIELD_INVALID.into());
    }
    if s.media_max_bytes
        .is_some_and(|b| !(1..=MEDIA_BYTES_MAX).contains(&b))
    {
        err("scope.media_max_bytes", FIELD_INVALID.into());
    }
    if input.object_sets.len() > OBJECT_SETS_MAX {
        err(
            "object_sets",
            format!("{FIELD_MAX_LENGTH}:{OBJECT_SETS_MAX}"),
        );
    }
    for (name, set) in &input.object_sets {
        if !valid_key(name) {
            err(&format!("object_sets.{name}"), FIELD_INVALID.into());
        } else if set.description.chars().count() > DESCRIPTION_MAX {
            err(
                &format!("object_sets.{name}.description"),
                format!("{FIELD_MAX_LENGTH}:{DESCRIPTION_MAX}"),
            );
        } else if set.query.type_key.as_deref().is_none_or(str::is_empty) {
            err(
                &format!("object_sets.{name}.type_key"),
                FIELD_REQUIRED.into(),
            );
        }
    }
    errors
}

/// The parameter a value names: `"$code"` → `code`.
fn param_of(v: &str) -> Option<&str> {
    v.strip_prefix('$').filter(|p| valid_key(p))
}

/// Parameters a query names, sorted.
pub fn params_of(q: &OntologyQuery) -> Vec<String> {
    let mut out: Vec<String> = q
        .filters
        .iter()
        .flat_map(|f| match &f.value {
            Value::String(s) => vec![s.as_str()],
            Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        })
        .chain(q.text.as_deref())
        .filter_map(param_of)
        .map(str::to_string)
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The query with its parameters filled; `Err(name)` = a parameter not
/// given.
pub fn bind(q: &OntologyQuery, given: &HashMap<String, String>) -> Result<OntologyQuery, String> {
    let fill = |s: &str| -> Result<Option<String>, String> {
        match param_of(s) {
            None => Ok(None),
            Some(p) => given.get(p).cloned().map(Some).ok_or_else(|| p.to_string()),
        }
    };
    let mut q = q.clone();
    for f in &mut q.filters {
        match &mut f.value {
            Value::String(s) => {
                if let Some(v) = fill(s)? {
                    *s = v;
                }
            }
            Value::Array(a) => {
                for x in a.iter_mut() {
                    if let Some(v) = x.as_str().map(fill).transpose()?.flatten() {
                        *x = Value::String(v);
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(t) = q.text.as_deref().map(fill).transpose()?.flatten() {
        q.text = Some(t);
    }
    Ok(q)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> AppInput {
        AppInput {
            key: "reader".into(),
            name: "Reader".into(),
            scope: AppScope {
                types: vec!["article".into(), "media_asset".into()],
                link_types: vec!["ecrit_par".into()],
                media_kinds: vec!["photo".into(), "document".into()],
                media_max_bytes: Some(10_000_000),
            },
            ..Default::default()
        }
    }

    fn set(json: serde_json::Value) -> ObjectSetDef {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn parameters_fill_values_only() {
        let s = set(serde_json::json!({"query": {
            "type_key": "article",
            "text": "$q",
            "filters": [
                {"key": "lang", "op": "eq", "value": "$lang"},
                {"key": "tag", "op": "in", "value": ["$tag", "fixed", "$$x"]},
                {"key": "$key", "op": "exists"}
            ]
        }}));
        assert_eq!(params_of(&s.query), vec!["lang", "q", "tag"]);
        let given: HashMap<String, String> = [("lang", "fr"), ("q", "moteur"), ("tag", "rust")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let q = bind(&s.query, &given).unwrap();
        assert_eq!(q.text.as_deref(), Some("moteur"));
        assert_eq!(q.filters[0].value, "fr");
        assert_eq!(
            q.filters[1].value,
            serde_json::json!(["rust", "fixed", "$$x"])
        );
        assert_eq!(q.filters[2].key, "$key", "a key is never a parameter");
        let mut missing = given.clone();
        missing.remove("lang");
        assert_eq!(bind(&s.query, &missing), Err("lang".to_string()));
    }

    #[test]
    fn an_object_set_names_its_type() {
        let mut i = input();
        i.object_sets
            .insert("all".into(), set(serde_json::json!({"query": {}})));
        assert_eq!(check(&i)["object_sets.all.type_key"], "required");
    }

    #[test]
    fn a_valid_declaration_passes() {
        assert!(check(&input()).is_empty());
    }

    #[test]
    fn each_field_is_checked() {
        let mut i = input();
        i.key = "Bad Key".into();
        i.name = " ".into();
        i.scope.types.push("Not-A-Key".into());
        i.scope.media_kinds.push("hologram".into());
        i.scope.media_max_bytes = Some(0);
        let e = check(&i);
        assert_eq!(e["key"], "invalid");
        assert_eq!(e["name"], "required");
        assert_eq!(e["scope.types"], "invalid");
        assert_eq!(e["scope.media_kinds"], "invalid");
        assert_eq!(e["scope.media_max_bytes"], "invalid");
        i = input();
        i.name = "x".repeat(NAME_MAX + 1);
        assert_eq!(check(&i)["name"], "max_length:120");
    }
}
