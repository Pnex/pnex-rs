//! Applications declared in an org (external-apps.md D433): an external
//! program built on the ontology, dressed around a service account, with
//! the scope it reads (types, link types, media). Pure validation here;
//! the server checks what needs the org (types exist, account free).
//! An application may also have screens built in PneX (app-builder.md D506):
//! its dashboards, ordered, with variables shared between them.

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
/// Most screens of one application.
pub const SCREENS_MAX: usize = 30;
/// Most variables of one application.
pub const VARIABLES_MAX: usize = 50;
/// Longest default value of a variable.
pub const VARIABLE_VALUE_MAX: usize = 255;
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

/// One screen of an application (D507): a dashboard it owns.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AppScreen {
    pub key: String,
    pub name: String,
    /// Its name per language, for a screen a pack ships.
    #[serde(default, skip_serializing_if = "crate::ontology::Names::is_empty")]
    pub names: crate::ontology::Names,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// The dashboard holding its versions, edited like any dashboard.
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub dashboard_id: String,
    /// The version readers see (Q-B2); `None` = never published.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_version: Option<i64>,
}

/// What a variable holds (D509).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum VariableKind {
    #[default]
    Text,
    Number,
    Date,
    Object,
    Enum,
}

/// A value shared by the widgets of an application (D509). Its value lives
/// in the reader's session, never in the database; it is a value, never an
/// expression (D512).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AppVariable {
    pub name: String,
    #[serde(default)]
    pub kind: VariableKind,
    /// Type of an `object` variable, or owner of the property of an `enum`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_key: Option<String>,
    /// The enum property whose values an `enum` variable takes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub property: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

/// What a widget of a screen reads and writes (D508, D510, D511). Which
/// fields apply depends on the widget type.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AppWidgetOptions {
    /// The widget's title per language, for a screen a pack ships.
    #[serde(default, skip_serializing_if = "crate::ontology::Names::is_empty")]
    pub titles: crate::ontology::Names,
    /// `object_table`: the object set read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_set: Option<String>,
    /// Parameter of the set → variable that fills it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
    /// `object_table`, `object_fields`: the properties shown.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<String>,
    /// The variable written (`input`, a row of `object_table`, a bar of
    /// `range_bars`: the `enum` variable of the property it groups by) or
    /// read (`object_fields`, `object_form`, `action_button`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variable: Option<String>,
    /// `object_table`: the screen a row opens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
    /// `object_fields`, `object_form`: the type shown or created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_type: Option<String>,
    /// `action_button`: `<type>.<action>`, in the application's list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

/// What the builder of an application changes (Q-B1): the scope and the
/// permitted actions stay with [`AppInput`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AppBuildInput {
    /// In display order. A new key adds a screen, a missing key removes
    /// it; `dashboard_id` and `published_version` are the server's.
    #[serde(default)]
    pub screens: Vec<AppScreenInput>,
    #[serde(default)]
    pub home: Option<String>,
    #[serde(default)]
    pub variables: Vec<AppVariable>,
    #[serde(default)]
    pub object_sets: BTreeMap<String, ObjectSetDef>,
    pub expected_version: i32,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AppScreenInput {
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub icon: Option<String>,
}

/// `POST /apps/{id}/publish`: readers now see the current version of every
/// screen, on the application version it was read on.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AppPublishInput {
    pub expected_version: i32,
}

/// A screen as its readers see it: the published version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AppScreenView {
    pub key: String,
    pub name: String,
    pub version: i64,
    pub layout: crate::viz::DashboardLayout,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AppView {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub key: String,
    pub name: String,
    /// Its name per language, for an application a pack ships.
    #[serde(default, skip_serializing_if = "crate::ontology::Names::is_empty")]
    pub names: crate::ontology::Names,
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
    /// Screens built in PneX, in display order (D507).
    #[serde(default)]
    pub screens: Vec<AppScreen>,
    /// Key of the first screen shown; the first one when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
    #[serde(default)]
    pub variables: Vec<AppVariable>,
    /// Actions its screens may invoke, `<type>.<action>` (D440).
    #[serde(default)]
    pub actions: Vec<String>,
    pub version: i32,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub updated_at: String,
}

/// An application a pack ships (app-builder.md D513), ready to use: created
/// at install when the org has none of that key, its screens published.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PackApp {
    pub key: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "crate::ontology::Names::is_empty")]
    pub names: crate::ontology::Names,
    #[serde(default)]
    pub description: String,
    pub scope: AppScope,
    #[serde(default)]
    pub object_sets: BTreeMap<String, ObjectSetDef>,
    #[serde(default)]
    pub actions: Vec<String>,
    #[serde(default)]
    pub variables: Vec<AppVariable>,
    #[serde(default)]
    pub home: Option<String>,
    #[serde(default)]
    pub screens: Vec<PackAppScreen>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PackAppScreen {
    pub key: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "crate::ontology::Names::is_empty")]
    pub names: crate::ontology::Names,
    pub layout: crate::viz::DashboardLayout,
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
    /// Actions its screens may invoke, `<type>.<action>` (D440).
    #[serde(default)]
    pub actions: Vec<String>,
    #[serde(default)]
    pub expected_version: Option<i32>,
}

/// Field errors of what a builder sends (structure only; the server checks
/// what needs the org).
pub fn check_build(input: &AppBuildInput) -> BTreeMap<String, String> {
    let mut errors = BTreeMap::new();
    if input.screens.len() > SCREENS_MAX {
        errors.insert(
            "screens".into(),
            format!("{FIELD_MAX_LENGTH}:{SCREENS_MAX}"),
        );
    }
    let mut keys = std::collections::HashSet::new();
    for (i, s) in input.screens.iter().enumerate() {
        // `build` is the builder's address under `/apps/<key>/`.
        if !valid_key(&s.key) || s.key == "build" || !keys.insert(s.key.as_str()) {
            errors.insert(format!("screens.{i}.key"), FIELD_INVALID.into());
        }
        let name = s.name.trim();
        if name.is_empty() {
            errors.insert(format!("screens.{i}.name"), FIELD_REQUIRED.into());
        } else if name.chars().count() > NAME_MAX {
            errors.insert(
                format!("screens.{i}.name"),
                format!("{FIELD_MAX_LENGTH}:{NAME_MAX}"),
            );
        }
    }
    if input.home.as_deref().is_some_and(|h| !keys.contains(h)) {
        errors.insert("home".into(), FIELD_INVALID.into());
    }
    if input.variables.len() > VARIABLES_MAX {
        errors.insert(
            "variables".into(),
            format!("{FIELD_MAX_LENGTH}:{VARIABLES_MAX}"),
        );
    }
    let mut names = std::collections::HashSet::new();
    for (i, v) in input.variables.iter().enumerate() {
        if !valid_key(&v.name) || !names.insert(v.name.as_str()) {
            errors.insert(format!("variables.{i}.name"), FIELD_INVALID.into());
        }
        if v.default
            .as_deref()
            .is_some_and(|d| d.chars().count() > VARIABLE_VALUE_MAX)
        {
            errors.insert(
                format!("variables.{i}.default"),
                format!("{FIELD_MAX_LENGTH}:{VARIABLE_VALUE_MAX}"),
            );
        }
        if matches!(v.kind, VariableKind::Object | VariableKind::Enum) && v.type_key.is_none() {
            errors.insert(format!("variables.{i}.type_key"), FIELD_REQUIRED.into());
        }
        if v.kind == VariableKind::Enum && v.property.is_none() {
            errors.insert(format!("variables.{i}.property"), FIELD_REQUIRED.into());
        }
    }
    errors
}

/// A set's own checks, shared by the declaration and the builder.
pub fn check_object_sets(
    sets: &BTreeMap<String, ObjectSetDef>,
    errors: &mut BTreeMap<String, String>,
) {
    if sets.len() > OBJECT_SETS_MAX {
        errors
            .entry("object_sets".to_string())
            .or_insert(format!("{FIELD_MAX_LENGTH}:{OBJECT_SETS_MAX}"));
    }
    for (name, set) in sets {
        let e = if !valid_key(name) {
            Some((format!("object_sets.{name}"), FIELD_INVALID.to_string()))
        } else if set.description.chars().count() > DESCRIPTION_MAX {
            Some((
                format!("object_sets.{name}.description"),
                format!("{FIELD_MAX_LENGTH}:{DESCRIPTION_MAX}"),
            ))
        } else if set.query.type_key.as_deref().is_none_or(str::is_empty) {
            Some((
                format!("object_sets.{name}.type_key"),
                FIELD_REQUIRED.to_string(),
            ))
        } else {
            None
        };
        if let Some((k, v)) = e {
            errors.entry(k).or_insert(v);
        }
    }
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
    if input.actions.len() > SCOPE_LIST_MAX {
        err("actions", format!("{FIELD_MAX_LENGTH}:{SCOPE_LIST_MAX}"));
    }
    check_object_sets(&input.object_sets, &mut errors);
    errors
}

/// The parameter a value names: `"$code"` → `code`.
fn param_of(v: &str) -> Option<&str> {
    // `$today` and `$now` are resolved by the server, never given.
    v.strip_prefix('$')
        .filter(|p| valid_key(p) && !matches!(*p, "today" | "now"))
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
    fn a_build_names_unique_screens_and_variables() {
        let input: AppBuildInput = serde_json::from_value(serde_json::json!({
            "screens": [{"key": "list", "name": "List"}, {"key": "list", "name": " "}],
            "home": "elsewhere",
            "variables": [{"name": "Bad"}, {"name": "who", "kind": "object"},
                {"name": "lang", "kind": "enum", "type_key": "article"}],
            "expected_version": 1
        }))
        .unwrap();
        let e = check_build(&input);
        assert_eq!(e["screens.1.key"], "invalid");
        assert_eq!(e["screens.1.name"], "required");
        assert_eq!(e["home"], "invalid");
        assert_eq!(e["variables.0.name"], "invalid");
        assert_eq!(e["variables.1.type_key"], "required");
        assert_eq!(e["variables.2.property"], "required");
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
