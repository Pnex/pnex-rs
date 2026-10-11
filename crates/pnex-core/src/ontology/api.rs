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

/// Timeline request of a case (cases.md K6); bounds RFC 3339, defaulting
/// to the case window, then to the last 30 days.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TimelineQuery {
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
}

/// One dated element of a timeline. `lane` is the member object it belongs
/// to (the case itself for its own events).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TimelineItem {
    pub at: String,
    /// `link_open` | `link_close` | `change` | `range` | `notification`.
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
    /// `occurred`: a `fuzzy_time` property of a member (cases.md K5), drawn
    /// as an interval at its precision; `title` is the member's title.
    #[serde(default)]
    pub fuzzy: Option<Value>,
    #[serde(default)]
    pub title: Option<String>,
}

/// Opening a case (cases.md K3): from scratch, or from a dated event whose
/// window and first members are filled in.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CaseOpenInput {
    pub title: String,
    #[serde(default)]
    pub from: Option<CaseFrom>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CaseFrom {
    /// `notification` | `event`.
    pub kind: String,
    /// RFC 3339 instant of the triggering event.
    pub at: String,
    /// Flow that raised it: becomes the subject, its linked objects the context.
    #[serde(default)]
    pub flow_id: Option<i64>,
    /// Window half-width around `at`; default 120, at most 7 days.
    #[serde(default)]
    pub margin_minutes: Option<i64>,
}

/// Members (open `in_case` links) and the merged timeline, oldest first.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CaseTimeline {
    pub from: String,
    pub to: String,
    pub members: Vec<LinkView>,
    pub items: Vec<TimelineItem>,
    /// False when the change journal (O2) could not be read.
    pub changes_available: bool,
    /// True when a source hit its cap: narrow the window.
    pub truncated: bool,
}

/// A claim as the hypotheses board shows it (cases.md K8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoardClaim {
    pub claim: ObjectRef,
    #[serde(default)]
    pub status: Option<String>,
    /// Open `sourced_by` links of the claim.
    pub sources: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
pub struct CaseBoard {
    pub hypotheses: Vec<BoardHypothesis>,
    pub unsourced: Vec<BoardClaim>,
    pub contradicted: Vec<BoardClaim>,
}

/// Export request (cases.md K12): the case as it stood at `as_of`
/// (RFC 3339), default its freeze instant, else now.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CaseExportQuery {
    #[serde(default)]
    pub as_of: Option<String>,
}

/// Where a member stands at the snapshot instant: its own `geo_point`
/// (`via` empty) or the place reached by a location link open then.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CasePlace {
    pub object: ObjectRef,
    pub lat: f64,
    pub lon: f64,
    #[serde(default)]
    pub via: Option<ObjectRef>,
    /// Location link type (`placed_at`, `located_at`, ...), empty for a
    /// property.
    #[serde(default)]
    pub link_type: String,
}

/// The case replayed at `as_of` (cases.md K14): members present then, the
/// links valid then among them, and their places.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CaseSnapshot {
    pub as_of: String,
    pub graph: Neighborhood,
    pub places: Vec<CasePlace>,
}

/// One member at the export instant: its `in_case` link and the object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportMember {
    pub link: LinkView,
    pub object: ObjectView,
}

/// Integrity line of a `piece` member (K12).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PieceEntry {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub captured_at: Option<String>,
}

/// A whole case in one document: members, the links between them, the
/// timeline, the board and the pieces manifest, all read at `as_of`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaseExport {
    pub exported_at: String,
    pub as_of: String,
    pub case: ObjectView,
    pub members: Vec<ExportMember>,
    pub links: Vec<LinkView>,
    pub timeline: CaseTimeline,
    pub board: CaseBoard,
    pub pieces: Vec<PieceEntry>,
    /// Members of type `person`: the UI warns before handing it out (K16).
    pub persons: usize,
}
