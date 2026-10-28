//! Cross-cutting organization layer (D42) — vocabulary shared
//! backend ↔ front, serde-only (wasm32-safe).
//!
//! Three separate mechanisms: **labels** (GIN-queryable classification),
//! **containment** (tree, a single parent), **edges** (links + placement).
//! Effective labels resolved at read (own + ancestors).
//! Containment and relation validity are declared as data by the types in
//! [`crate::ontology`]; the engine never matches on kinds.
//! Device decoupling: organizing never writes
//! to `device_registries` nor to the firmware.

// ─────────────────────────── ResourceRef ───────────────────────────

/// Polymorphic identity — (org, kind, id), `id` = stringified PK.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ResourceRef {
    pub org_id: i64,
    pub kind: String,
    pub id: String,
}

impl ResourceRef {
    pub fn new(org_id: i64, kind: &str, id: impl std::fmt::Display) -> Self {
        Self {
            org_id,
            kind: kind.to_string(),
            id: id.to_string(),
        }
    }
}

impl std::fmt::Display for ResourceRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.kind, self.id)
    }
}

// ─────────────────────────── kinds vivants ───────────────────────────

/// Registered concepts. A new system type = 1 constant + 1 entry in
/// `ontology::system_object_types` + 1 backend resolver.
pub const KIND_DEVICE: &str = "device";
pub const KIND_MEDIA_ASSET: &str = "media_asset";
pub const KIND_DASHBOARD: &str = "dashboard";
pub const KIND_TOUR: &str = "tour";
pub const KIND_MAP_PIN: &str = "map_pin";
pub const KIND_FLOW: &str = "flow";
/// Collaborative page (pages.md #21).
pub const KIND_DOC: &str = "doc";
/// Pure organization container (name + emoji), contains all kinds.
pub const KIND_FOLDER: &str = "folder";

/// Live kinds, for `valid_kind` and the registry enumeration.
pub const KINDS: [&str; 8] = [
    KIND_DEVICE,
    KIND_MEDIA_ASSET,
    KIND_DASHBOARD,
    KIND_TOUR,
    KIND_MAP_PIN,
    KIND_FLOW,
    KIND_DOC,
    KIND_FOLDER,
];

pub fn valid_kind(kind: &str) -> bool {
    KINDS.contains(&kind)
}

// ─────────────────────────── labelable kinds ───────────────────────────

/// Kinds that are not ontology types but still carry labels (D218): labels
/// are a plain search aid, they need nothing but the row to exist.
pub const KIND_FUNCTION: &str = "function";
pub const KIND_FIRMWARE_PROJECT: &str = "firmware_project";
pub const KIND_CONTROL: &str = "control";
pub const KIND_MEDIA_STREAM: &str = "media_stream";
pub const KIND_ML_MODEL: &str = "ml_model";
pub const KIND_NOTIFY_CHANNEL: &str = "notify_channel";
pub const KIND_NOTIFY_TEMPLATE: &str = "notify_template";
pub const KIND_ANNOTATION_LAYER: &str = "annotation_layer";

/// Label-only kinds (the ontology types — [`KINDS`] and org types — are
/// labelable too, through the registry).
pub const LABEL_ONLY_KINDS: [&str; 8] = [
    KIND_FUNCTION,
    KIND_FIRMWARE_PROJECT,
    KIND_CONTROL,
    KIND_MEDIA_STREAM,
    KIND_ML_MODEL,
    KIND_NOTIFY_CHANNEL,
    KIND_NOTIFY_TEMPLATE,
    KIND_ANNOTATION_LAYER,
];

// ─────────────────────────── relations ───────────────────────────

/// Pano/media placed on a POI (absorbs viz_links, D39 → D42).
pub const REL_PLACED_ON: &str = "placed_on";

// ─────────────────────────── labels ───────────────────────────

/// `{"site": "serre", "critique": null}` — tag nu = `None`.
pub type LabelSet = std::collections::BTreeMap<String, Option<String>>;

/// Classification cap (labels ≠ catch-all: free-form stays in `metadata`).
pub const LABELS_MAX: usize = 32;
/// Name: `[a-z0-9_-]`, 1..=64.
pub const LABEL_NAME_MAX: usize = 64;
/// Value: trimmed non-empty, ≤ 255 chars (or bare tag = `None`).
pub const LABEL_VALUE_MAX: usize = 255;

/// Valid label name — closed charset, `naming.rs` school.
pub fn valid_label_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= LABEL_NAME_MAX
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Valid label value after trim (bare tag allows `None`).
pub fn valid_label_value(value: &str) -> bool {
    !value.is_empty() && value.chars().count() <= LABEL_VALUE_MAX
}

/// List filter `"site:serre"` / `"critique"` (bare tag).
/// `(None, Some(token))` = invalid, with the field token (`required` for an
/// empty filter, else `invalid`).
pub fn parse_label_filter(raw: &str) -> (Option<(String, Option<String>)>, Option<String>) {
    let raw = raw.trim();
    if raw.is_empty() {
        return (None, Some(crate::err_codes::FIELD_REQUIRED.into()));
    }
    match raw.split_once(':') {
        Some((name, value)) => {
            let value = value.trim();
            if !valid_label_name(name) {
                return (None, Some(crate::err_codes::FIELD_INVALID.into()));
            }
            if !valid_label_value(value) {
                return (None, Some(crate::err_codes::FIELD_INVALID.into()));
            }
            (Some((name.to_string(), Some(value.to_string()))), None)
        }
        None => {
            if !valid_label_name(raw) {
                return (None, Some(crate::err_codes::FIELD_INVALID.into()));
            }
            (Some((raw.to_string(), None)), None)
        }
    }
}

// ─────────────────────────── tests ───────────────────────────

/// One step of a location breadcrumb: a site (`map_pin`) or a folder.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LocationNode {
    pub kind: String,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub emoji: Option<String>,
}

/// Where a resource sits in the sites tree: its site, then the folders from
/// the site down to the resource. An object attached to several sites
/// (media, tour, dashboard) has one entry per site.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ResourceLocation {
    pub site: LocationNode,
    #[serde(default)]
    pub folders: Vec<LocationNode>,
}

/// `GET /api/v1/resources/{kind}/{id}/location`.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ResourceLocations {
    pub locations: Vec<ResourceLocation>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_et_kinds() {
        let r = ResourceRef::new(7, KIND_MEDIA_ASSET, "3f2e…-uuid");
        assert_eq!(r.kind, KIND_MEDIA_ASSET);
        assert_eq!(r.to_string(), "media_asset/3f2e…-uuid");
        assert!(valid_kind(KIND_DEVICE));
        assert!(valid_kind(KIND_FOLDER));
        assert!(!valid_kind("sites"));
        assert!(!valid_kind(""));
    }

    #[test]
    fn validation_labels() {
        assert!(valid_label_name("site"));
        assert!(valid_label_name("sous-reseau_2"));
        assert!(!valid_label_name("Site")); // uppercase forbidden (normalized lower)
        assert!(!valid_label_name("avec espace"));
        assert!(!valid_label_name(""));
        assert!(!valid_label_name(&"x".repeat(65)));
        assert!(valid_label_value("serre"));
        assert!(!valid_label_value(""));
        assert!(!valid_label_value(&"x".repeat(256)));
    }

    #[test]
    fn filtres_labels() {
        let (f, err) = parse_label_filter("site:serre");
        assert_eq!(f, Some(("site".into(), Some("serre".into()))));
        assert!(err.is_none());
        let (f, err) = parse_label_filter("critique");
        assert_eq!(f, Some(("critique".into(), None)));
        assert!(err.is_none());
        let (f, err) = parse_label_filter("");
        assert!(f.is_none() && err.is_some());
        let (f, err) = parse_label_filter("Site:Serre");
        assert!(f.is_none() && err.is_some(), "non-normalized name rejected");
    }
}
