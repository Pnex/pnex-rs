//! Wire types of `/api/v1/ontology/*` (D176–D187), shared by the backend,
//! the UI and the assistant.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{LinkTypeDef, ObjectTypeDef};

/// Maximum hops of a query traversal (D185: the API stays bounded).
pub const MAX_HOPS: usize = 4;
pub const QUERY_LIMIT_MAX: u64 = 500;
pub const TITLE_MAX: usize = 255;

/// An object type as listed: system types carry version 0.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObjectTypeView {
    pub def: ObjectTypeDef,
    pub version: i32,
    #[serde(default)]
    pub pack_key: Option<String>,
    /// Live objects of the type in the org.
    #[serde(default)]
    pub object_count: i64,
}

/// Create (`expected_version` absent) or new version of an org type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObjectTypeInput {
    pub def: ObjectTypeDef,
    #[serde(default)]
    pub expected_version: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObjectTypeVersionView {
    pub version: i32,
    pub def: ObjectTypeDef,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkTypeView {
    pub def: LinkTypeDef,
    pub version: i32,
    #[serde(default)]
    pub pack_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkTypeInput {
    pub def: LinkTypeDef,
    #[serde(default)]
    pub expected_version: Option<i32>,
}

/// Compact reference to an object (link ends, breadcrumbs, search).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectRef {
    pub id: String,
    pub type_key: String,
    pub title: String,
    /// Key of the native row for system types (e.g. a device registry id),
    /// used by the UI to deep-link the specialised page.
    #[serde(default)]
    pub native_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObjectView {
    pub id: String,
    pub type_key: String,
    pub title: String,
    pub native_id: String,
    pub system: bool,
    pub properties: Map<String, Value>,
    #[serde(default)]
    pub type_version: Option<i32>,
    pub version: i32,
    #[serde(default)]
    pub source_ref: Option<String>,
    pub valid_from: String,
    #[serde(default)]
    pub valid_to: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Create or update an object. For a system object only `properties`
/// (the org's extra properties) is writable; its title is the native one.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ObjectInput {
    #[serde(default)]
    pub type_key: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub properties: Map<String, Value>,
    #[serde(default)]
    pub expected_version: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkView {
    pub id: i64,
    pub link_type: String,
    pub source: ObjectRef,
    pub target: ObjectRef,
    pub attributes: Map<String, Value>,
    pub valid_from: String,
    #[serde(default)]
    pub valid_to: Option<String>,
    #[serde(default)]
    pub source_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LinkInput {
    pub link_type: String,
    pub source_id: String,
    pub target_id: String,
    #[serde(default)]
    pub attributes: Map<String, Value>,
    /// RFC 3339; default now. A link may start in the past (history entry).
    #[serde(default)]
    pub valid_from: Option<String>,
}

/// Closing a link: `valid_to` RFC 3339, default now.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LinkClose {
    #[serde(default)]
    pub valid_to: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    #[default]
    Out,
    In,
}

/// One traversal step: follow `link_type` links from the current set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hop {
    pub link_type: String,
    #[serde(default)]
    pub direction: Direction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterOp {
    Eq,
    Ne,
    Lt,
    Lte,
    Gt,
    Gte,
    /// Case-insensitive substring of a text property.
    Contains,
    Exists,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PropertyFilter {
    pub key: String,
    pub op: FilterOp,
    #[serde(default)]
    pub value: Value,
}

/// Declarative ontology query (D185): select, then traverse, then page.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct OntologyQuery {
    /// Start set: objects of this type (all types when absent).
    #[serde(default)]
    pub type_key: Option<String>,
    /// Start set: these object ids.
    #[serde(default)]
    pub ids: Vec<String>,
    /// Case-insensitive title search.
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub filters: Vec<PropertyFilter>,
    /// `name` or `name:value`, effective labels (D42).
    #[serde(default)]
    pub label: Option<String>,
    /// Descendants of this object in the containment tree.
    #[serde(default)]
    pub within: Option<String>,
    #[serde(default)]
    pub traverse: Vec<Hop>,
    /// RFC 3339: objects and links valid at that instant (default now).
    #[serde(default)]
    pub as_of: Option<String>,
    /// `series` properties whose last value is joined (D185).
    #[serde(default)]
    pub latest: Vec<String>,
    #[serde(default)]
    pub include_archived: bool,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub offset: Option<u64>,
}

/// Last value of a `series` property.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LatestValue {
    pub value: f64,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryRow {
    pub object: ObjectView,
    /// Joined last values, by property key (absent when no data).
    #[serde(default)]
    pub latest: std::collections::BTreeMap<String, LatestValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryResult {
    pub count: i64,
    pub rows: Vec<QueryRow>,
}

/// Local graph of an object (D186): nodes and the links among them.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Neighborhood {
    pub nodes: Vec<ObjectRef>,
    pub links: Vec<LinkView>,
}

/// One point of a recomposed series (D181).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeriesPoint {
    pub t: String,
    pub v: f64,
    /// Device that measured the point (object id).
    pub source: String,
}

/// A binding segment: which device fed the property, and when.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeriesSegment {
    pub device: ObjectRef,
    pub metric: String,
    pub from: String,
    #[serde(default)]
    pub to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SeriesView {
    pub property: String,
    #[serde(default)]
    pub unit: Option<String>,
    pub segments: Vec<SeriesSegment>,
    pub points: Vec<SeriesPoint>,
}

/// One change of an object or a link (D184), from the O2 `object_changes`
/// stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeView {
    pub at: String,
    /// `object.create` | `object.update` | `object.archive` | `link.open` |
    /// `link.close`.
    pub action: String,
    pub source_ref: String,
    #[serde(default)]
    pub before: Value,
    #[serde(default)]
    pub after: Value,
}

/// A pack (D190) as shipped or exported: YAML on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pack {
    pub key: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub types: Vec<ObjectTypeDef>,
    #[serde(default)]
    pub link_types: Vec<LinkTypeDef>,
}

/// A pack the server offers, and whether the org installed it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackView {
    pub key: String,
    pub name: String,
    pub version: String,
    pub description: String,
    #[serde(default)]
    pub installed_version: Option<String>,
}
