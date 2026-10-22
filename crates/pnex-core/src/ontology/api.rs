//! Wire types of `/api/v1/ontology/*` (D176–D187), shared by the backend,
//! the UI and the assistant.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{LinkTypeDef, ObjectTypeDef};

/// Maximum hops of a query traversal (D185: the API stays bounded).
pub const MAX_HOPS: usize = 4;

/// Links between people an introduction path follows (account-mapping.md
/// D338): who knows, recommends, employs or directs whom, and who belongs
/// to a professional group.
pub const INTRODUCTION_LINKS: &[&str] = &["knows", "recommande", "employs", "dirige", "member_of"];
pub const QUERY_LIMIT_MAX: u64 = 500;
pub const TITLE_MAX: usize = 255;

/// An object type as listed: system types carry version 0.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ObjectTypeInput {
    pub def: ObjectTypeDef,
    #[serde(default)]
    pub expected_version: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ObjectTypeVersionView {
    pub version: i32,
    pub def: ObjectTypeDef,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LinkTypeView {
    pub def: LinkTypeDef,
    pub version: i32,
    #[serde(default)]
    pub pack_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LinkTypeInput {
    pub def: LinkTypeDef,
    #[serde(default)]
    pub expected_version: Option<i32>,
}

/// Compact reference to an object (link ends, breadcrumbs, search).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub updated_at: String,
}

/// Create or update an object. For a system object only `properties`
/// (the org's extra properties) is writable; its title is the native one.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LinkInput {
    pub link_type: String,
    pub source_id: String,
    pub target_id: String,
    #[serde(default)]
    pub attributes: Map<String, Value>,
    /// RFC 3339; default now. A link may start in the past (history entry).
    #[serde(default)]
    pub valid_from: Option<String>,
    /// RFC 3339, after `valid_from` and not in the future: the link is
    /// written closed, as history, even while the same pair has an open
    /// link (only open links are unique per pair).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_to: Option<String>,
}

/// Updating a link: its attributes, replaced whole (open or closed link).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LinkUpdate {
    #[serde(default)]
    pub attributes: Map<String, Value>,
}

/// Closing a link: `valid_to` RFC 3339, default now.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LinkClose {
    #[serde(default)]
    pub valid_to: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    #[default]
    Out,
    In,
}

/// One traversal step: follow `link_type` links from the current set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Hop {
    pub link_type: String,
    #[serde(default)]
    pub direction: Direction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
    /// The property is not set (an object never processed).
    Missing,
    /// One of the values of a list (texts as stored).
    In,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PropertyFilter {
    pub key: String,
    pub op: FilterOp,
    #[serde(default)]
    pub value: Value,
}

/// Declarative ontology query (D185): select, then traverse, then page.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
    /// Property to sort on (as text; ISO dates sort right); title when
    /// absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_by: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub descending: bool,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub offset: Option<u64>,
}

/// Last value of a `series` property.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LatestValue {
    pub value: f64,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct QueryRow {
    pub object: ObjectView,
    /// Joined last values, by property key (absent when no data).
    #[serde(default)]
    pub latest: std::collections::BTreeMap<String, LatestValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct QueryResult {
    pub count: i64,
    pub rows: Vec<QueryRow>,
}

/// Local graph of an object (D186): nodes and the links among them.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Neighborhood {
    pub nodes: Vec<ObjectRef>,
    pub links: Vec<LinkView>,
    /// Object id → colour token of its coloured status (D339).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub colors: std::collections::BTreeMap<String, String>,
}

/// One point of a recomposed series (D181).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SeriesPoint {
    pub t: String,
    pub v: f64,
    /// Device that measured the point (object id).
    pub source: String,
}

/// A binding segment: which device fed the property, and when.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SeriesSegment {
    pub device: ObjectRef,
    pub metric: String,
    pub from: String,
    #[serde(default)]
    pub to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ChangeView {
    pub at: String,
    /// `object.create` | `object.update` | `object.archive` | `link.open` |
    /// `link.update` | `link.close`.
    pub action: String,
    pub source_ref: String,
    #[serde(default)]
    pub before: Value,
    #[serde(default)]
    pub after: Value,
}

/// A pack (D190) as shipped or exported: YAML on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Pack {
    pub key: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "super::Names::is_empty")]
    pub names: super::Names,
    pub version: String,
    /// Shipped packs this one builds on (their types in its links):
    /// installed first when missing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "super::Names::is_empty")]
    pub descriptions: super::Names,
    #[serde(default)]
    pub types: Vec<ObjectTypeDef>,
    #[serde(default)]
    pub link_types: Vec<LinkTypeDef>,
    /// Page models (pages.md P13, cases.md K8).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub page_templates: Vec<PageTemplate>,
    /// Optional properties added to a type of a required pack or to a
    /// system type (D306).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extends: Vec<TypeExtension>,
    /// Named trees computed on links (D305).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hierarchies: Vec<Hierarchy>,
    /// Imports created in the org at install (D301), never overwritten.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub imports: Vec<super::import::PackImport>,
    /// Registry functions created at install when the org has none of
    /// that name (D316): the pack's rules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub functions: Vec<PackFunction>,
    /// Flow models created as drafts at install when the org has none of
    /// that name (D316); never deployed by the install (D123). A function
    /// node names its function by `function_name` (resolved at install).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flows: Vec<PackFlow>,
    /// Points the org may set in its own layer (D368): thresholds, weights.
    /// Pack functions read them with `pack_setting("<pack>.<key>", default)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub settings: Vec<PackSetting>,
}

/// A point of a pack the org may set (ontology.md D368).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PackSetting {
    pub key: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "super::Names::is_empty")]
    pub names: super::Names,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "super::Names::is_empty")]
    pub descriptions: super::Names,
    pub kind: PackSettingKind,
    /// The pack's value, used while the org sets none.
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub default: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum PackSettingKind {
    Number,
    Text,
    Bool,
}

/// Longest text setting.
pub const PACK_SETTING_TEXT_MAX: usize = 2000;

impl PackSetting {
    /// Machine code of what is wrong with `v` for this setting, if anything.
    pub fn check(&self, v: &Value) -> Option<String> {
        use crate::err_codes::{FIELD_INVALID, FIELD_MAX_LENGTH};
        match (self.kind, v) {
            (PackSettingKind::Number, Value::Number(n)) => {
                let n = n.as_f64()?;
                (self.min.is_some_and(|m| n < m) || self.max.is_some_and(|m| n > m))
                    .then(|| FIELD_INVALID.to_string())
            }
            (PackSettingKind::Text, Value::String(s)) => (s.chars().count()
                > PACK_SETTING_TEXT_MAX)
                .then(|| format!("{FIELD_MAX_LENGTH}:{PACK_SETTING_TEXT_MAX}")),
            (PackSettingKind::Bool, Value::Bool(_)) => None,
            _ => Some(FIELD_INVALID.to_string()),
        }
    }
}

/// A setting as the org sees it: the pack's definition and the value in
/// force (the org's, else the pack's default).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PackSettingView {
    #[serde(flatten)]
    pub def: PackSetting,
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub value: Value,
    /// The org set its own value.
    pub overridden: bool,
}

/// The org's values for some settings of a pack; `null` returns a setting
/// to the pack's default.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PackSettingsInput {
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub values: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PackFunction {
    pub name: String,
    pub language: crate::functions::FunctionLanguage,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub code: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PackFlow {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// A flow graph (`{"nodes": [...]}`).
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub graph: serde_json::Value,
}

/// A tree read on links (D305): at each node, its children are the objects
/// reached by each step in turn. A node with several parents appears under
/// each of them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Hierarchy {
    pub key: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "super::Names::is_empty")]
    pub names: super::Names,
    pub steps: Vec<HierarchyStep>,
    /// Number properties summed over the tree's nodes, each node once
    /// (account-mapping.md D330: current and potential users).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sums: Vec<String>,
    /// What makes a node of the tree covered; the others are its white
    /// space (D330).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<HierarchyCoverage>,
}

/// A node is covered when it has an open link of one of `links` (a contact
/// works there) or one of the `values` of a property (a client site).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct HierarchyCoverage {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<String>,
    /// Property key → values that cover.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub values: std::collections::BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct HierarchyStep {
    /// Link type key.
    pub link: String,
    /// Follow the link from its target to its source.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub inverse: bool,
}

/// A node of a computed hierarchy, flat in depth-first order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct HierarchyNode {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub type_key: String,
    pub title: String,
    pub depth: u32,
    /// Link type that leads here from the parent (empty for the root).
    pub link: String,
    /// Titles of the other parents this node also appears under.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub also_under: Vec<String>,
    /// Its values of coloured properties (D339).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub marks: Vec<NodeMark>,
    /// Not covered, when the hierarchy declares a coverage (D330).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub white_space: bool,
}

/// A value of a coloured enum property (account-mapping.md D339).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NodeMark {
    pub property: String,
    pub value: String,
    /// One of the palette tokens (`green`, `amber`…).
    pub color: String,
    /// Label of the value by language (D370), empty = the value itself.
    #[serde(default, skip_serializing_if = "super::Names::is_empty")]
    pub labels: super::Names,
}

/// How many distinct nodes of the tree hold a coloured value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MarkCount {
    pub property: String,
    pub value: String,
    pub color: String,
    /// Label of the value by language (D370), empty = the value itself.
    #[serde(default, skip_serializing_if = "super::Names::is_empty")]
    pub labels: super::Names,
    pub count: u32,
}

/// A summed number property over the tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PropertyTotal {
    pub property: String,
    pub total: f64,
}

/// `GET /ontology/objects/{id}/hierarchy`: the hierarchies of the installed
/// packs, and the tree of the one asked (the first by default).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct HierarchyView {
    pub hierarchies: Vec<Hierarchy>,
    /// Key of the hierarchy computed; empty when none is installed.
    pub key: String,
    pub nodes: Vec<HierarchyNode>,
    /// The depth or node cap cut the tree.
    pub truncated: bool,
    /// Coverage of the tree, computed at read time, never stored (D330).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub counts: Vec<MarkCount>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub totals: Vec<PropertyTotal>,
    /// Distinct nodes not covered (`white_space` on each of them).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_coverage: bool,
    #[serde(default)]
    pub white_space: u32,
}

/// Properties a pack adds to a type it does not own (D306).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TypeExtension {
    #[serde(rename = "type")]
    pub type_key: String,
    pub properties: Vec<super::PropertyDef>,
}

/// A page model shipped by a pack. `body` is a Markdown subset: `#`–`###`
/// headings, `-` and `1.` list items, paragraphs (no inline marks).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PageTemplate {
    pub key: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "super::Names::is_empty")]
    pub names: super::Names,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "super::Names::is_empty")]
    pub descriptions: super::Names,
    pub body: String,
    /// Object type the template is filled from (`page_render`
    /// placeholders, account-mapping.md D340); none = a blank page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Link type from the page (`doc`) to that object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

/// `POST /ontology/objects/{id}/pages`: a page from a template, filled from
/// the object.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ObjectPageInput {
    /// Key of a page template whose subject is the object's type.
    pub template: String,
    /// Default: the template's name and the object's title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Language of the enum value labels written in the page (`fr-FR`…);
    /// default English.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
}

/// Objects to purge from an archived case (cases.md K16).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CasePurgeInput {
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CasePurged {
    pub purged: usize,
}

/// New page of a case, optionally from a template of an installed pack.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CasePageInput {
    pub title: String,
    #[serde(default)]
    pub template: Option<String>,
}

/// A pack the server offers, and whether the org installed it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PackView {
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub names: super::Names,
    pub version: String,
    pub description: String,
    #[serde(default)]
    pub descriptions: super::Names,
    #[serde(default)]
    pub installed_version: Option<String>,
    /// Settings of an installed pack (D368), empty otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub settings: Vec<PackSettingView>,
    /// Answer of an install: the functions and flows the org edited, kept
    /// as they are instead of the pack's version (D368).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept: Vec<String>,
}

/// Opening a case: from scratch, or from a dated alert (cases-workspace.md
/// K20): its first event is then the alert, with the flow that raised it
/// and the flow's neighbours attached as context.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CaseOpenInput {
    pub title: String,
    #[serde(default)]
    pub from: Option<CaseFrom>,
    /// Opens it as a sub-case of this case (cases-graph-sigma.md GS16).
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub parent_id: Option<String>,
}

/// A sub-case, as its parent's workspace lists it (GS16).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CaseChild {
    pub case: ObjectRef,
    /// Members of the sub-case (its events and what they involve, its
    /// undated pieces).
    pub members: usize,
    /// Those members that are members of the parent too.
    pub shared: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CaseFrom {
    /// `notification` | `event`.
    pub kind: String,
    /// RFC 3339 instant of the alert.
    pub at: String,
    /// Flow that raised it.
    #[serde(default)]
    pub flow_id: Option<i64>,
}

/// An object attached to an event of the case, with its role (K21).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CaseAttachment {
    /// The `involves` link, to change its role or close it.
    pub link_id: i64,
    pub object: ObjectRef,
    pub role: String,
    #[serde(default)]
    pub detail: Option<String>,
}

/// An event of the case and what it involves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CaseEvent {
    pub event: ObjectView,
    pub attachments: Vec<CaseAttachment>,
}

/// The case as the workspace shows it (K22): its events, oldest first,
/// with their attachments, and its undated pieces. Members are derived.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CaseWorkspace {
    pub case: ObjectView,
    pub events: Vec<CaseEvent>,
    pub undated: Vec<ObjectRef>,
    /// Areas held by members (`geo_area` values, K29), for the map.
    #[serde(default)]
    pub zones: Vec<CaseZone>,
    /// Members with a place on the map: a `geo_point` value, else the centre
    /// of their area, else the POI's coordinates.
    #[serde(default)]
    pub places: Vec<CasePlace>,
    /// Enclosing cases, nearest first (GS16).
    #[serde(default)]
    pub parents: Vec<ObjectRef>,
    /// Sub-cases (GS16).
    #[serde(default)]
    pub children: Vec<CaseChild>,
}

/// A located member of a case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CasePlace {
    pub object: ObjectRef,
    pub lon: f64,
    pub lat: f64,
}

/// An area of a case member.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CaseZone {
    pub object: ObjectRef,
    /// The `geo_area` value: a circle or a GeoJSON polygon.
    pub area: Value,
}

/// New event of a case with its first attachments, in one transaction.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CaseEventInput {
    pub title: String,
    /// `fuzzy_time` value (`from`, `to`, `precision`).
    pub occurred: Value,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub attach: Vec<CaseAttachInput>,
}

/// Attach an object to an event of the case (`event_id`), or put it among
/// the undated pieces (no `event_id`). The role defaults by object type.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CaseAttachInput {
    #[serde(default)]
    pub event_id: Option<String>,
    pub object_id: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
}

/// Detach an object from an event (`event_id`: it stays in the case, undated
/// if nothing else dates it), or remove it from the case (no `event_id`; for
/// an event, its pieces stay in the case).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CaseDetachInput {
    #[serde(default)]
    pub event_id: Option<String>,
    pub object_id: String,
}

/// A track file to import onto a case member (K26): GPX or CSV text. A dry
/// run only previews; times without offset need `time_zone` (IANA name).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TrackImportInput {
    pub object_id: String,
    pub file_name: String,
    /// `gpx` | `csv`.
    pub format: String,
    pub content: String,
    #[serde(default)]
    pub time_zone: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
}

/// An imported track, kept as case evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TrackImport {
    pub id: String,
    pub object: ObjectRef,
    pub file_name: String,
    pub format: String,
    pub sha256: String,
    #[serde(default)]
    pub time_zone: Option<String>,
    pub points: usize,
    pub rejected: usize,
    pub t_start: String,
    pub t_end: String,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub imported_at: String,
}

/// What an import reads (or read): clean points, rejected rows with their
/// reason (the first ones), the span, and the import once written.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TrackImportPreview {
    pub points: usize,
    pub rejected_count: usize,
    pub rejected: Vec<crate::track::Rejected>,
    #[serde(default)]
    pub t_start: Option<String>,
    #[serde(default)]
    pub t_end: Option<String>,
    /// Some times carry no offset: a time zone is required.
    pub needs_time_zone: bool,
    pub sha256: String,
    #[serde(default)]
    pub import: Option<TrackImport>,
}

/// Detection request (K30): RFC 3339 bounds, defaulting like signals.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DetectQuery {
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
}

/// Outcome of a detection run: proposals new to this case (a rejected one is
/// never proposed again), and whether the position history was readable.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CaseDetected {
    pub from: String,
    pub to: String,
    pub available: bool,
    pub truncated: bool,
    pub inserted: usize,
}

/// A detection proposed to the case (K30): retained (it became `event_id`),
/// rejected (kept, never proposed again) or still proposed.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CaseProposal {
    pub id: String,
    /// `co_presence` | `stop` | `signal_loss`.
    pub kind: String,
    /// `<kind>@<version>`.
    pub detector: String,
    /// Effective thresholds of the run.
    pub params: Value,
    pub t_start: String,
    #[serde(default)]
    pub t_end: Option<String>,
    /// Members concerned, in fingerprint order.
    pub refs: Vec<ObjectRef>,
    /// Where (`at: [lon, lat]`) and measures (`min_distance_m`).
    pub payload: Value,
    /// `proposed` | `retained` | `rejected`.
    pub status: String,
    #[serde(default)]
    pub reject_reason: Option<String>,
    #[serde(default)]
    pub event_id: Option<String>,
}

/// Retaining a proposal: the title of the event, written by the UI in the
/// user's language.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ProposalRetain {
    pub title: String,
}

/// Rejecting a proposal, with an optional reason (K45).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ProposalReject {
    #[serde(default)]
    pub reason: Option<String>,
}

/// Signals request (K30): RFC 3339 bounds, defaulting to the span of the
/// case events, then to the last 30 days.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema, utoipa::IntoParams))]
#[cfg_attr(feature = "openapi", into_params(parameter_in = Query))]
pub struct SignalsQuery {
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
}

/// One raw signal around the case members, computed on read; retaining it
/// makes an event. `lane` is the member it concerns.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SignalItem {
    pub at: String,
    /// `link_open` | `link_close` | `change` | `range` | `notification` | `occurred`.
    pub kind: String,
    pub lane: String,
    #[serde(default)]
    pub link: Option<LinkView>,
    #[serde(default)]
    pub change: Option<ChangeView>,
    /// A named interval scoped on a member (D182).
    #[serde(default)]
    pub range: Option<crate::time_range::TimeRange>,
    /// A notification sent by a member flow (D86 journal).
    #[serde(default)]
    pub notification: Option<crate::notify::NotifyDelivery>,
    /// `occurred`: a `fuzzy_time` property of a member, at its precision;
    /// `title` is the member's title.
    #[serde(default)]
    pub fuzzy: Option<Value>,
    #[serde(default)]
    pub title: Option<String>,
}

/// Raw signals of a case over a window, oldest first.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CaseSignals {
    pub from: String,
    pub to: String,
    pub items: Vec<SignalItem>,
    /// False when the change journal (O2) could not be read.
    pub changes_available: bool,
    /// True when a source hit its cap: narrow the window.
    pub truncated: bool,
}

/// A claim as the hypotheses board shows it (cases.md K8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BoardClaim {
    pub claim: ObjectRef,
    #[serde(default)]
    pub status: Option<String>,
    /// Open `sourced_by` links of the claim.
    pub sources: usize,
    /// Why it is corroborated below two origins (K7), when written.
    #[serde(default)]
    pub justification: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BoardHypothesis {
    pub hypothesis: ObjectRef,
    #[serde(default)]
    pub status: Option<String>,
    pub supports: Vec<BoardClaim>,
    pub contradicts: Vec<BoardClaim>,
}

/// Hypotheses with the claims for and against them, plus the member claims
/// that need attention: no source, or contradicted by another claim. No
/// score: the human concludes (K8).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CaseBoard {
    pub hypotheses: Vec<BoardHypothesis>,
    pub unsourced: Vec<BoardClaim>,
    pub contradicted: Vec<BoardClaim>,
}

/// Tracks request (cases-workspace.md K27): an RFC 3339 window, the
/// duration one timeline pixel stands for (drives the generalisation), and
/// the gap beyond which two observations are a signal loss.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema, utoipa::IntoParams))]
#[cfg_attr(feature = "openapi", into_params(parameter_in = Query))]
pub struct CaseTracksQuery {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub resolution_ms: Option<i64>,
    #[serde(default)]
    pub signal_gap_s: Option<i64>,
}

/// Track of one case member. Each segment is gap-free; each point is
/// `[lon, lat, alt|null, dt_ms]` with `dt_ms` relative to `t0_ms`, so the
/// map keeps its precision in float32 (kepler.gl school).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CaseTrack {
    pub object: ObjectRef,
    pub t0_ms: i64,
    pub segments: Vec<Vec<(f64, f64, Option<f64>, i64)>>,
    /// Points read before generalisation.
    pub raw_points: usize,
}

/// Tracks of the case members over a window. `available` = false when the
/// position history cannot be read (O2 absent or never provisioned).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CaseTracks {
    pub from: String,
    pub to: String,
    pub available: bool,
    pub truncated: bool,
    pub tracks: Vec<CaseTrack>,
}

/// Integrity line of a `piece` member (K12).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PieceEntry {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub captured_at: Option<String>,
}

/// A whole case in one document (K33, schema `pnex.case/1`): its events
/// with their attachments, its undated pieces, the hypotheses board and the
/// pieces manifest, as they are now.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CaseExport {
    pub schema: String,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub exported_at: String,
    pub workspace: CaseWorkspace,
    pub board: CaseBoard,
    pub pieces: Vec<PieceEntry>,
    /// Members of type `person`: the UI warns before handing it out (K16).
    pub persons: usize,
}

/// Shortest chain of links between two objects, ends included.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ObjectPath {
    pub path: Vec<ObjectRef>,
}

/// Objects reachable from an object through open links (impact analysis).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ObjectImpact {
    pub objects: Vec<ObjectRef>,
}

/// Expansion of a node of the case graph (K31). Without `pick`: the
/// neighbours counted by link type and object type; with it: those
/// neighbours and their links. `exclude` = nodes already shown.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ExpandInput {
    pub object_id: String,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub pick: Option<Vec<ExpandFacet>>,
}

/// Neighbours of one link type and one object type.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ExpandFacet {
    pub link_type: String,
    pub type_key: String,
    #[serde(default)]
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ExpandResult {
    pub facets: Vec<ExpandFacet>,
    pub nodes: Vec<ObjectRef>,
    pub links: Vec<LinkView>,
    /// More neighbours than returned.
    pub truncated: bool,
}

/// Positions of the case graph's nodes, in layout coordinates, as the team
/// arranged them by hand (K31). Empty = the automatic layout.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CaseLayout {
    pub positions: std::collections::BTreeMap<String, (f64, f64)>,
}

/// A logged read of persons' tracks (K38): who, for what (`tracks`,
/// `detect`, `snapshot`), which objects (`gone` = purged since), which window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CaseRead {
    pub at: String,
    pub who: Option<String>,
    pub purpose: String,
    pub objects: Vec<ObjectRef>,
    #[serde(default)]
    pub gone: usize,
    pub from: String,
    pub to: String,
}

/// A case re-created from an export (K33), in the same org: its events and
/// their attachments; attachments and pieces whose object is gone (or lives
/// in another org) are listed, not re-created.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CaseImported {
    pub case: ObjectRef,
    pub events: usize,
    pub missing: Vec<ObjectRef>,
}

/// What a frozen state holds (K33): the export of the case, its reviewed
/// detections and its tracks over the derived window, generalised.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CaseSnapshotContent {
    pub export: CaseExport,
    /// Retained or rejected proposals (pending ones are not a state).
    pub proposals: Vec<CaseProposal>,
    pub tracks: Option<CaseTracks>,
    /// Tracks left out: over the size cap.
    #[serde(default)]
    pub tracks_omitted: bool,
}

impl CaseSnapshotContent {
    /// Canonical bytes: object keys sorted at every level, no whitespace.
    // shortcut: keys sort by UTF-8 bytes and numbers print as serde_json
    // does, not the RFC 8785 forms; equal for our ASCII keys, and the
    // fingerprint stays reproducible since the same code reads it back.
    pub fn canonical(&self) -> String {
        fn sorted(v: serde_json::Value) -> serde_json::Value {
            use serde_json::Value as V;
            match v {
                V::Object(m) => {
                    let mut kv: Vec<_> = m.into_iter().collect();
                    kv.sort_by(|a, b| a.0.cmp(&b.0));
                    V::Object(kv.into_iter().map(|(k, v)| (k, sorted(v))).collect())
                }
                V::Array(a) => V::Array(a.into_iter().map(sorted).collect()),
                v => v,
            }
        }
        serde_json::to_string(&sorted(serde_json::to_value(self).unwrap_or_default()))
            .unwrap_or_default()
    }
}

/// A frozen state of a case: never modified, fingerprinted.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CaseSnapshot {
    pub seq: i32,
    pub label: Option<String>,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub taken_at: String,
    /// Display name of who froze it, if still known.
    pub taken_by: Option<String>,
    /// SHA-256 of `content`'s bytes.
    pub sha256: String,
    pub size_bytes: usize,
}

/// `POST /cases/{id}/snapshots`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CaseSnapshotInput {
    #[serde(default)]
    pub label: Option<String>,
}

/// A frozen state with its content: the canonical JSON of a
/// [`CaseSnapshotContent`], byte for byte what was fingerprinted.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CaseSnapshotFull {
    pub snapshot: CaseSnapshot,
    pub content: String,
}
