//! The pipeline document (pipeline-engine.md D934–D936): closed steps, each
//! reading a working table by name and writing one. The backend parses the
//! YAML into JSON; this module checks it.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::ontology::import::{LinkMap, ObjectMap};

pub const KEY_MAX: usize = 63;
pub const STEPS_MAX: usize = 60;
pub const HOSTS_MAX: usize = 20;
pub const STRING_MAX: usize = 2048;
pub const SQL_MAX_BYTES: usize = 64 * 1024;
pub const COLUMNS_MAX: usize = 200;
pub const AXES_MAX: usize = 4;
pub const VALUES_MAX: usize = 1000;
pub const TESTS_MAX: usize = 20;
pub const AFTER_MAX: usize = 20;
/// Largest body one fetch spools to disk, and largest archive entry.
pub const FETCH_MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Largest streamed file (`stream: true`): read as it arrives, never kept.
pub const STREAM_MAX_BYTES: u64 = 64 * 1024 * 1024 * 1024;
/// Parts of one streamed file at most.
pub const PARTS_MAX: usize = 100;

/// A pipeline as written.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Hosts the pipeline may call (D942); every fetched address must carry one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
    pub steps: Vec<Step>,
    /// When it runs on its own: one run per this duration (`5m`, `1h`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<String>,
    /// Large files (a day of AIS, a year of sales): runs only for an org
    /// whose table quota a platform administrator set.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub large: bool,
    /// Keys of pipelines it runs after, besides the tables it reads (D946).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
    /// `<pack>.<key>` of settings that must hold a value: while one is
    /// empty, a run reads and writes nothing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tests: Vec<TestCase>,
}

/// One closed step (D936).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    Enumerate(Enumerate),
    Fetch(Fetch),
    Browse(Browse),
    Extract(Extract),
    Unpack(Unpack),
    Read(Read),
    Sql(Sql),
    Call(Call),
    Write(Write),
}

/// Rows from settings, periods, objects or a range, crossed (D936).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Enumerate {
    #[serde(rename = "as")]
    pub as_: String,
    /// Crosses the axes with each row of this table (the hours of each day);
    /// an axis may then name the row's columns in its `from` and `to`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_each: Option<String>,
    pub cross: Vec<Axis>,
    /// Keep a state per row between runs (D937).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub track: bool,
    /// Rows a run takes at most; the others wait for the next run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_run: Option<u32>,
    /// What a row that wrote nothing means (D937).
    #[serde(default)]
    pub empty: Empty,
    /// `retry`: days before such a row is asked again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_days: Option<u32>,
}

/// One axis: exactly one of `values`, `setting`, `periods`, `objects`, `range`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Axis {
    /// Column name of this axis in the rows.
    #[serde(rename = "as")]
    pub as_: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    /// `<pack>.<key>` of a comma-separated setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setting: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub periods: Option<Period>,
    /// First period, `YYYY[-MM[-DD[THH]]]`, or relative to the run:
    /// `this_year`, `this_month`, `today`, `-12m`, `-7d`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// `<pack>.<key>` of a setting holding the first period; `from` when empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_setting: Option<String>,
    /// Values matching one of these globs are left out (`bis:*`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// Keeps the part of each value before this separator
    /// (`VIXCLS:diff:volatility` → `VIXCLS`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split: Option<String>,
    /// Setting values in upper or lower case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case: Option<Case>,
    /// Leaves out the period not finished yet.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub closed_only: bool,
    /// End of the periods, excluded: same forms as `from`, or a template of
    /// the row (`{{day_end}}`). Now when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// Columns derived from the value by globs, the first that matches
    /// (`scale: [["*JPY", "3"], ["??????", "5"]]`); a value no glob of a
    /// column matches is left out.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub derive: BTreeMap<String, Vec<[String; 2]>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objects: Option<ObjectsAxis>,
    /// Integers from the first to the last, both included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<[i64; 2]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Case {
    Upper,
    Lower,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Period {
    Year,
    Month,
    Day,
    Hour,
}

/// The values of a property on the org's live objects of a type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectsAxis {
    #[serde(rename = "type")]
    pub type_key: String,
    pub property: String,
    /// Only the objects whose property holds one of the values
    /// (`{ status: [active] }`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub only: BTreeMap<String, Vec<String>>,
    /// Only the objects holding an open link of one of these types, either
    /// way (a certifier's network, not every company known).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub linked: Vec<String>,
    /// Only the objects (with `linked`: linked to one) whose property holds
    /// a code of a setting; an empty setting filters nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codes: Option<CodesFilter>,
    /// Column receiving the object's title besides its value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Other properties of the object, each in a column of its name (empty
    /// when the object has none). The axis property should be unique.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<String>,
}

/// Codes of a comma-separated setting compared, upper-cased, to a property.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodesFilter {
    /// `<pack>.<key>`.
    pub setting: String,
    pub property: String,
}

impl ObjectsAxis {
    /// How a run's inputs and a test case name these values:
    /// `type.property`, then `?property=value,value` per filter.
    pub fn key(&self) -> String {
        let mut k = format!("{}.{}", self.type_key, self.property);
        for (p, values) in &self.only {
            k.push_str(&format!("?{p}={}", values.join(",")));
        }
        if !self.linked.is_empty() {
            k.push_str(&format!("?linked={}", self.linked.join(",")));
        }
        if let Some(c) = &self.codes {
            k.push_str(&format!("?{}~{}", c.property, c.setting));
        }
        if !self.columns.is_empty() {
            k.push_str(&format!("?columns={}", self.columns.join(",")));
        }
        k
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    #[default]
    Get,
    Post,
}

/// What an empty answer means (D937).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Empty {
    /// Nothing to read there, for good.
    Settled,
    /// Asked again after `retry_days`.
    #[default]
    Retry,
}

/// One HTTP request per row (D936), or one when `for_each` is absent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fetch {
    #[serde(rename = "as")]
    pub as_: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_each: Option<String>,
    /// Address template.
    pub url: String,
    #[serde(default)]
    pub method: Method,
    /// `post`: column of the row holding the form fields (an object).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form_from: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
    /// One call per this duration on the host (D938), `250ms`, `5m`…
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every: Option<String>,
    /// Asks page after page, each with its number in a parameter, until an
    /// answer holds no record or the last page allowed. Rows get `page`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<Pages>,
    /// A `404` is an empty answer, not a failure.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub missing_is_empty: bool,
    /// Headers sent, templates of the row and settings (`User-Agent` with
    /// the org's declared contact). Never a secret.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// A key of the org's vault (D942), sent only to the document's hosts.
    /// Only a pipeline shipped in a pack may name one: an org's own document
    /// reaches a secret through a connector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<FetchSecret>,
    /// Same bytes as the last run: the rest of the pipeline is skipped.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub skip_if_unchanged: bool,
    /// Address template of the SHA-256 published next to the file
    /// (`<hex>  <name>`): a file that differs is refused; a missing one is
    /// read unchecked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksum_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    /// The file is these parts joined in order, each named by `{{part}}` in
    /// the address (a tar split in pieces of 2 GB); a missing part after the
    /// first ends it. Streamed only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<String>,
    /// Read as it arrives and never kept: the steps after are one `unpack`
    /// of a tar and one `read` of its entries, run on the stream. Only in a
    /// `large` document.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stream: bool,
}

/// Page numbers of a JSON answer (D936).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pages {
    /// Query parameter holding the page number.
    pub param: String,
    /// Number of the first page (some sources count from 0).
    #[serde(default = "first_page")]
    pub from: u32,
    /// JSON pointer of the records array; empty or absent = the last page.
    pub until: String,
    /// Pages asked at most per row.
    pub max: u32,
}

/// Pages a fetch may ask per row.
pub const PAGES_MAX: u32 = 1000;

fn first_page() -> u32 {
    1
}

/// Where a vault key goes: exactly one of a query parameter or a header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FetchSecret {
    /// Name of the org's vault secret, sent as it is.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Or a token the org's user and password get first (OAuth2 password
    /// grant), sent in `header`; it never reaches a working table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<TokenGrant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    /// Put before the key in the header (`Token`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
}

/// How a fetch gets its token: a form posted to `url` with the org's
/// vault secrets `user` and `password` (as `username` and `password`) and
/// fixed `fields`; the token is the answer's JSON field `field`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenGrant {
    pub url: String,
    pub user: String,
    pub password: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, String>,
    #[serde(default = "token_field")]
    pub field: String,
}

fn token_field() -> String {
    "access_token".into()
}

/// Navigation actions (site-recipes.md D323) played per row (D936).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Browse {
    #[serde(rename = "as")]
    pub as_: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_each: Option<String>,
    /// Host of the site; must be one of the document's `hosts`.
    pub site: String,
    /// `required`: plays on a browser agent of the org (D782).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    pub steps: Vec<crate::site_recipe::Step>,
}

/// Rows drawn from the files of a table: exactly one of the kinds.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Extract {
    #[serde(rename = "as")]
    pub as_: String,
    pub from: String,
    /// Links of a page: `href` of the elements this selector finds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<String>,
    /// The fields and action of the form this selector finds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form: Option<String>,
    /// JSON pointer of an array of records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json: Option<String>,
    /// Pattern with named groups; one row per match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
    /// Keeps the rows whose column matches a glob.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub r#where: BTreeMap<String, String>,
    /// Keeps the one row with the highest value of this column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_by: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Archive {
    /// From the first bytes.
    #[default]
    Auto,
    Zip,
    Tar,
    Gzip,
    Lzma,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unpack {
    #[serde(rename = "as")]
    pub as_: String,
    pub from: String,
    #[serde(default)]
    pub format: Archive,
    /// Globs of the entries kept; empty = all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    Csv,
    Json,
    Xml,
    Html,
    Ical,
    Lines,
    Binary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnType {
    #[default]
    Text,
    Integer,
    Decimal,
    Float,
    Bool,
    Date,
    Timestamp,
    /// The value as JSON text (`at: "/"` = the whole record): an import's
    /// mapping reads it as nested fields (`rows: <column>`).
    Json,
}

/// A column of a read: where it is and what it holds (D939).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Column {
    pub name: String,
    /// CSV: header name or 0-based position, or a list of them joined by a
    /// space (a date and a time in two cells); JSON: pointer; XML: path;
    /// binary: byte offset. Absent = `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<serde_json::Value>,
    #[serde(default, rename = "type")]
    pub kind: ColumnType,
    /// chrono format of a date or a timestamp; RFC 3339 when absent;
    /// `epoch_s`, `epoch_ms`, `epoch_us`, or `epoch` (the unit read from the
    /// number of digits).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// IANA zone (`America/New_York`) of a time written without an offset,
    /// daylight saving included; a fixed offset (`-05:00`) only when the
    /// source never moves its clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// Decimal places kept, exact (no float).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<u32>,
    /// Binary: `u8`…`u64`, `i32`, `f32`, `f64`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary: Option<String>,
    /// Binary: the stored integer divided by 10^`divide`; a number or a
    /// template of the row (`{{scale}}`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub divide: Option<String>,
    /// Binary timestamp: the instant (RFC 3339, or a template of the row,
    /// `{{hour_start}}`) the stored count of `format` units is added to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// An empty cell is null; refused otherwise.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
    /// Cells read as empty, compared without case (`unknown`, `n/a`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub null_if: Vec<String>,
    /// Lowest and highest number kept; outside, the cell is null when the
    /// column is optional, else the row does not read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<[f64; 2]>,
    /// Cell → value before it is typed (`True: sell`); a cell not listed
    /// takes the value of `"*"` when there is one (empty = no value), else
    /// stays as it is.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub map: BTreeMap<String, String>,
    /// The same value on every row of a file instead of a cell (`null` = a
    /// null of the type); a text may be a template of the file's row
    /// (`{{instrument}}`).
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub value: Option<serde_json::Value>,
}

/// Files → typed rows, by a format reader (D939).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Read {
    #[serde(rename = "as")]
    pub as_: String,
    pub from: String,
    pub format: Format,
    /// Glob of the entry read, when `from` holds several files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    /// CSV: whether the first line names the columns.
    #[serde(default)]
    pub header: Header,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delimiter: Option<char>,
    /// `utf-8` or `latin1`; guessed when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,
    /// JSON pointer or XML path of the records. In JSON a `*` segment walks
    /// every value of an object or element of an array
    /// (`/facts/*/*/units/*/*`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub records: Option<String>,
    /// JSON: names of the keys the `*` segments walk through, in order; a
    /// column reads one by its name (`at: concept`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<String>,
    /// JSON: `records` is an object of arrays of one length, a record per
    /// position (`{"form": [..], "date": [..]}`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub columnar: bool,
    /// HTML: selector of the table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
    /// Binary: bytes of one record, and `big` or `little`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record_size: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endian: Option<String>,
    /// Empty = every column as text, named as found.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<Column>,
    /// Keeps the rows whose column (by its name above) matches one of the
    /// globs, before it is typed.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub r#where: BTreeMap<String, Vec<String>>,
    /// A row that does not read is left out instead of failing the file
    /// (a position "not available" in a feed of millions).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub skip_invalid: bool,
    /// CSV: `false` when quotes are plain characters (raw tab-separated
    /// exports), every line then splits on the delimiter alone.
    #[serde(default = "yes", skip_serializing_if = "Clone::clone")]
    pub quoted: bool,
}

fn yes() -> bool {
    true
}

/// A field written `null` is `Some(Null)`, an absent one `None`.
fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<serde_json::Value>, D::Error> {
    serde_json::Value::deserialize(d).map(Some)
}

/// Whether the first CSV line names the columns: `true`, `false`, or `auto`
/// (a line whose typed cells do not read as their type).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Header {
    #[default]
    Yes,
    No,
    Auto,
}

impl<'de> Deserialize<'de> for Header {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match serde_json::Value::deserialize(d)? {
            serde_json::Value::Bool(true) => Ok(Header::Yes),
            serde_json::Value::Bool(false) => Ok(Header::No),
            serde_json::Value::String(s) if s == "auto" => Ok(Header::Auto),
            _ => Err(serde::de::Error::custom("header: true, false or auto")),
        }
    }
}

/// A DataFusion query on working tables and tables of the org (D572).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sql {
    #[serde(rename = "as")]
    pub as_: String,
    /// SQL name → table of the org, besides the working tables.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tables: BTreeMap<String, String>,
    pub sql: String,
}

/// A Starlark function of the org or the pack, the last resort (D940).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Call {
    #[serde(rename = "as")]
    pub as_: String,
    pub from: String,
    pub function: String,
}

/// Where the rows end (D941): exactly one target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Write {
    pub from: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<TableTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objects: Option<ObjectsTarget>,
    /// Time ranges of the org (D182), upserted by `external_id`: columns
    /// `external_id`, `label`, `planned_start`, `planned_end`, `category`,
    /// `source_url`, `description`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_ranges: Option<RangesTarget>,
}

/// The org's time ranges as a write target; nothing to set yet.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RangesTarget {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableTarget {
    pub name: String,
    /// `replace`: each write is the whole table (the table an import syncs);
    /// `append` by default.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replace: bool,
}

/// The ontology sync (D577) of the D301 mapping.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectsTarget {
    pub objects: Vec<ObjectMap>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<LinkMap>,
    /// `snapshot` closes the objects absent from the rows.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub snapshot: bool,
}

/// A replay test (D945): recorded answers, and what must come out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestCase {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Settings of the run, `<pack>.<key>` → value.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub settings: BTreeMap<String, String>,
    /// Values of the org's live objects, `<type>.<property>` → values.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub objects: BTreeMap<String, Vec<String>>,
    /// Same key → value → the axis' title and other columns.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, BTreeMap<String, BTreeMap<String, String>>>,
    /// Today's date of the run, `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub today: Option<String>,
    /// Address → fixture file of the pack; an address not listed answers 404.
    pub given: BTreeMap<String, String>,
    /// Working table → what it holds.
    pub expect: BTreeMap<String, Expect>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<String>,
    /// Rows that must be found, by some of their columns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contains: Vec<BTreeMap<String, serde_json::Value>>,
}

/// A problem of a document: where, and a machine token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Violation {
    pub path: String,
    pub token: String,
}

impl Step {
    pub fn name(&self) -> &'static str {
        match self {
            Step::Enumerate(_) => "enumerate",
            Step::Fetch(_) => "fetch",
            Step::Browse(_) => "browse",
            Step::Extract(_) => "extract",
            Step::Unpack(_) => "unpack",
            Step::Read(_) => "read",
            Step::Sql(_) => "sql",
            Step::Call(_) => "call",
            Step::Write(_) => "write",
        }
    }

    /// The working table it writes; `None` for `write`.
    pub fn output(&self) -> Option<&str> {
        Some(match self {
            Step::Enumerate(s) => &s.as_,
            Step::Fetch(s) => &s.as_,
            Step::Browse(s) => &s.as_,
            Step::Extract(s) => &s.as_,
            Step::Unpack(s) => &s.as_,
            Step::Read(s) => &s.as_,
            Step::Sql(s) => &s.as_,
            Step::Call(s) => &s.as_,
            Step::Write(_) => return None,
        })
    }

    /// The working tables it reads by name (a `sql` names them in its text).
    pub fn inputs(&self) -> Vec<&str> {
        match self {
            Step::Enumerate(s) => s.for_each.as_deref().into_iter().collect(),
            Step::Sql(_) => Vec::new(),
            Step::Fetch(s) => s.for_each.as_deref().into_iter().collect(),
            Step::Browse(s) => s.for_each.as_deref().into_iter().collect(),
            Step::Extract(s) => vec![&s.from],
            Step::Unpack(s) => vec![&s.from],
            Step::Read(s) => vec![&s.from],
            Step::Call(s) => vec![&s.from],
            Step::Write(s) => vec![&s.from],
        }
    }

    /// Whether the server runs it (it needs the network, the vault or the
    /// ontology); the worker runs the others (D943).
    pub fn on_server(&self) -> bool {
        matches!(
            self,
            Step::Enumerate(_) | Step::Fetch(_) | Step::Browse(_) | Step::Write(_)
        )
    }
}

/// A working table, column or SQL name: a letter or `_`, then letters,
/// digits and `_`.
/// A column name: as [`valid_name`], capitals allowed, so that a table keeps
/// the names of its source (`Numero_Fiche`) and a file loaded by hand maps the
/// same.
pub fn valid_column(s: &str) -> bool {
    let mut c = s.chars();
    s.len() <= KEY_MAX
        && c.next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && c.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub fn valid_name(s: &str) -> bool {
    let mut c = s.chars();
    s.len() <= KEY_MAX
        && c.next().is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && c.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn valid_setting(s: &str) -> bool {
    s.split_once('.')
        .is_some_and(|(a, b)| valid_name(a) && valid_name(b))
}

/// Host of an address template whose host is written out; `None` when the
/// host comes from a value (`{{url}}`) or the address is not http(s).
pub fn literal_host(template: &str) -> Option<String> {
    let rest = template
        .strip_prefix("https://")
        .or_else(|| template.strip_prefix("http://"))?;
    let host: String = rest
        .chars()
        .take_while(|c| !matches!(c, '/' | ':' | '?' | '#'))
        .collect();
    (!host.is_empty() && !host.contains("{{")).then(|| host.to_ascii_lowercase())
}

/// Whether a duration (`250ms`, `30s`, `5m`, `1h`) is well formed; its value in ms.
pub fn duration_ms(s: &str) -> Option<u64> {
    let s = s.trim();
    let (n, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit())?);
    let n: u64 = n.parse().ok()?;
    Some(match unit {
        "ms" => n,
        "s" => n * 1000,
        "m" => n * 60_000,
        "h" => n * 3_600_000,
        _ => return None,
    })
}

impl Document {
    /// Empty when the document can run.
    /// The `unpack` (tar) and `read` a streamed fetch feeds, each the only
    /// step reading the table before it, the read with its columns declared.
    pub fn streamed(&self, fetch: &str) -> Option<(&Unpack, &Read)> {
        let only = |table: &str| {
            let mut readers = self.steps.iter().filter(|s| s.inputs().contains(&table));
            let one = readers.next();
            readers.next().is_none().then_some(one).flatten()
        };
        let Some(Step::Unpack(u)) = only(fetch) else {
            return None;
        };
        let Some(Step::Read(r)) = only(&u.as_) else {
            return None;
        };
        (u.format == Archive::Tar && !r.columns.is_empty()).then_some((u, r))
    }

    pub fn violations(&self) -> Vec<Violation> {
        let mut out = Vec::new();
        let mut bad = |path: String, token: &str| {
            out.push(Violation {
                path,
                token: token.into(),
            })
        };
        if !valid_name(&self.key) {
            bad("key".into(), "invalid");
        }
        if self.name.chars().count() > 255 {
            bad("name".into(), "max_length:255");
        }
        if self.hosts.len() > HOSTS_MAX {
            bad("hosts".into(), &format!("max_items:{HOSTS_MAX}"));
        }
        for (i, h) in self.hosts.iter().enumerate() {
            if crate::site_recipe::site_host(h).is_none() || h.contains('/') {
                bad(format!("hosts.{i}"), "invalid");
            }
        }
        if self.after.len() > AFTER_MAX || self.after.iter().any(|a| !valid_name(a)) {
            bad("after".into(), "invalid");
        }
        if self.requires.len() > 8
            || self.requires.iter().any(|k| {
                k.split_once('.')
                    .is_none_or(|(p, n)| !valid_name(p) || !valid_name(n))
            })
        {
            bad("requires".into(), "invalid");
        }
        if self.steps.is_empty() {
            bad("steps".into(), "required");
        }
        if self.steps.len() > STEPS_MAX {
            bad("steps".into(), &format!("max_items:{STEPS_MAX}"));
        }
        if !self.steps.iter().any(|s| matches!(s, Step::Write(_))) {
            bad("steps".into(), "write_required");
        }
        let hosts: HashSet<String> = self
            .hosts
            .iter()
            .map(|h| h.trim().to_ascii_lowercase())
            .collect();
        let mut seen: HashSet<&str> = HashSet::new();
        for (i, step) in self.steps.iter().enumerate() {
            let at = format!("steps.{i}.{}", step.name());
            for input in step.inputs() {
                if !seen.contains(input) {
                    bad(format!("{at}.from"), &format!("unknown_table:{input}"));
                }
            }
            if let Some(o) = step.output() {
                if !valid_name(o) {
                    bad(format!("{at}.as"), "invalid");
                } else if !seen.insert(o) {
                    bad(format!("{at}.as"), "duplicate");
                }
            }
            // shortcut: one tracked enumeration, first, drives the rows of a run;
            // lift it when a source needs two levels of state.
            if matches!(step, Step::Enumerate(e) if e.track) && i != 0 {
                bad(format!("{at}.track"), "first_step_only");
            }
            step_violations(step, &at, &hosts, &mut bad);
            if let Step::Fetch(f) = step {
                let part_ok =
                    |p: &String| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric());
                if !f.parts.is_empty()
                    && (!f.stream
                        || f.parts.len() > PARTS_MAX
                        || !f.parts.iter().all(part_ok)
                        || !crate::pipeline::template::names(&f.url)
                            .iter()
                            .any(|n| n == "part"))
                {
                    bad(format!("{at}.parts"), "invalid");
                }
                if f.stream
                    && (!self.large
                        || self.streamed(&f.as_).is_none()
                        || f.method != Method::Get
                        || f.pages.is_some()
                        || f.checksum_url.is_some()
                        || f.skip_if_unchanged
                        || f.secret.is_some())
                {
                    bad(format!("{at}.stream"), "invalid");
                }
            }
        }
        if self.tests.len() > TESTS_MAX {
            bad("tests".into(), &format!("max_items:{TESTS_MAX}"));
        }
        for (i, t) in self.tests.iter().enumerate() {
            if t.given.is_empty() {
                bad(format!("tests.{i}.given"), "required");
            }
            for name in t.expect.keys() {
                if !seen.contains(name.as_str()) {
                    bad(
                        format!("tests.{i}.expect"),
                        &format!("unknown_table:{name}"),
                    );
                }
            }
            if t.today
                .as_deref()
                .is_some_and(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_err())
            {
                bad(format!("tests.{i}.today"), "invalid");
            }
        }
        out
    }
}

fn long(s: &str) -> bool {
    s.len() > STRING_MAX
}

fn step_violations(
    step: &Step,
    at: &str,
    hosts: &HashSet<String>,
    bad: &mut impl FnMut(String, &str),
) {
    match step {
        Step::Enumerate(e) => {
            if e.cross.is_empty() {
                bad(format!("{at}.cross"), "required");
            }
            if e.cross.len() > AXES_MAX {
                bad(format!("{at}.cross"), &format!("max_items:{AXES_MAX}"));
            }
            if e.per_run == Some(0) {
                bad(format!("{at}.per_run"), "invalid");
            }
            if (e.per_run.is_some() || e.retry_days.is_some()) && !e.track {
                bad(format!("{at}.track"), "required");
            }
            if e.retry_days.is_some() && e.empty == Empty::Settled {
                bad(format!("{at}.retry_days"), "retry_only");
            }
            let mut names = HashSet::new();
            for (j, a) in e.cross.iter().enumerate() {
                let p = format!("{at}.cross.{j}");
                if !valid_name(&a.as_) || !names.insert(a.as_.as_str()) {
                    bad(format!("{p}.as"), "invalid");
                }
                let kinds = [
                    !a.values.is_empty(),
                    a.setting.is_some(),
                    a.periods.is_some(),
                    a.objects.is_some(),
                    a.range.is_some(),
                ];
                if kinds.iter().filter(|k| **k).count() != 1 {
                    bad(p.clone(), "one_of:values,setting,periods,objects,range");
                }
                if a.values.len() > VALUES_MAX || a.values.iter().any(|v| long(v)) {
                    bad(format!("{p}.values"), &format!("max_items:{VALUES_MAX}"));
                }
                for s in [&a.setting, &a.from_setting].into_iter().flatten() {
                    if !valid_setting(s) {
                        bad(format!("{p}.setting"), "invalid");
                    }
                }
                if let Some(per) = a.periods {
                    if a.from.is_none() && a.from_setting.is_none() {
                        bad(format!("{p}.from"), "required");
                    }
                    if a.from.as_deref().is_some_and(|f| {
                        !f.contains("{{")
                            && super::periods::parse_start_at(
                                per,
                                f,
                                chrono::NaiveDateTime::default(),
                            )
                            .is_none()
                    }) {
                        bad(format!("{p}.from"), "invalid");
                    }
                } else if a.from.is_some() || a.from_setting.is_some() || a.closed_only {
                    bad(format!("{p}.from"), "periods_only");
                }
                if let Some(o) = &a.objects {
                    if !valid_name(&o.type_key)
                        || !valid_name(&o.property)
                        || o.only.len() > 4
                        || o.only.iter().any(|(p, v)| !valid_name(p) || v.is_empty())
                        || o.linked.len() > 8
                        || o.linked.iter().any(|l| !valid_name(l))
                        || o.codes
                            .as_ref()
                            .is_some_and(|c| !valid_name(&c.property) || !c.setting.contains('.'))
                        || o.title.as_ref().is_some_and(|t| !valid_name(t))
                        || o.columns.len() > 8
                        || o.columns.iter().any(|c| !valid_name(c))
                    {
                        bad(format!("{p}.objects"), "invalid");
                    }
                }
                if a.derive.keys().any(|k| !valid_name(k))
                    || a.derive
                        .values()
                        .any(|rules| rules.is_empty() || rules.len() > 500)
                {
                    bad(format!("{p}.derive"), "invalid");
                }
                if a.to.is_some() && a.periods.is_none() {
                    bad(format!("{p}.to"), "periods_only");
                }
                if a.range
                    .is_some_and(|[lo, hi]| lo > hi || hi - lo >= 100_000)
                {
                    bad(format!("{p}.range"), "invalid");
                }
            }
        }
        Step::Fetch(f) => {
            if f.url.trim().is_empty() || long(&f.url) {
                bad(format!("{at}.url"), "required");
            }
            match literal_host(&f.url) {
                Some(h) if !hosts.contains(&h) => {
                    bad(format!("{at}.url"), &format!("host_not_declared:{h}"))
                }
                None if !f.url.starts_with("{{") => bad(format!("{at}.url"), "invalid"),
                _ => {}
            }
            if (f.method == Method::Post) != f.form_from.is_some() {
                bad(format!("{at}.form_from"), "post_only");
            }
            if f.pages.as_ref().is_some_and(|p| {
                p.param.is_empty()
                    || !p.until.starts_with('/')
                    || p.max == 0
                    || p.max > PAGES_MAX
                    || p.from > 1
                    || f.method != Method::Get
            }) {
                bad(format!("{at}.pages"), "invalid");
            }
            if f.every.as_deref().is_some_and(|e| duration_ms(e).is_none()) {
                bad(format!("{at}.every"), "invalid");
            }
            if let Some(h) = f.checksum_url.as_deref().and_then(literal_host) {
                if !hosts.contains(&h) {
                    bad(
                        format!("{at}.checksum_url"),
                        &format!("host_not_declared:{h}"),
                    );
                }
            }
            if f.headers
                .keys()
                .any(|h| h.is_empty() || h.len() > 100 || h.eq_ignore_ascii_case("authorization"))
            {
                bad(format!("{at}.headers"), "invalid");
            }
            if let Some(s) = &f.secret {
                if s.param.is_some() == s.header.is_some() {
                    bad(format!("{at}.secret"), "one_of:param,header");
                }
                if s.name.trim().is_empty() == s.token.is_none() {
                    bad(format!("{at}.secret"), "one_of:name,token");
                }
                if let Some(t) = &s.token {
                    let reserved = |k: &String| k == "username" || k == "password";
                    if s.header.is_none()
                        || !t.url.starts_with("https://")
                        || literal_host(&t.url).is_none_or(|h| !hosts.contains(&h))
                        || t.user.trim().is_empty()
                        || t.password.trim().is_empty()
                        || t.fields.len() > 10
                        || t.fields.keys().any(reserved)
                        || t.field.is_empty()
                    {
                        bad(format!("{at}.secret.token"), "invalid");
                    }
                }
            }
            let cap = if f.stream {
                STREAM_MAX_BYTES
            } else {
                FETCH_MAX_BYTES
            };
            if f.max_bytes.is_some_and(|b| b == 0 || b > cap) {
                bad(format!("{at}.max_bytes"), "invalid");
            }
        }
        Step::Browse(b) => {
            match crate::site_recipe::site_host(&b.site) {
                Some(h) if hosts.contains(&h) || hosts.contains(&format!("www.{h}")) => {}
                _ => bad(format!("{at}.site"), "host_not_declared"),
            }
            if b.steps.is_empty() || b.steps.len() > crate::site_recipe::STEPS_MAX {
                bad(format!("{at}.steps"), "invalid");
            }
            if b.session
                .as_deref()
                .is_some_and(|s| s != crate::site_recipe::SESSION_REQUIRED)
            {
                bad(format!("{at}.session"), "invalid");
            }
        }
        Step::Extract(e) => {
            let kinds = [&e.links, &e.form, &e.json, &e.regex];
            if kinds.iter().filter(|k| k.is_some()).count() != 1 {
                bad(at.to_string(), "one_of:links,form,json,regex");
            }
            if kinds.iter().flat_map(|k| k.as_deref()).any(long) {
                bad(at.to_string(), &format!("max_length:{STRING_MAX}"));
            }
            if e.regex.as_deref().is_some_and(|r| !r.contains("(?P<")) {
                bad(format!("{at}.regex"), "named_group_required");
            }
            if e.r#where
                .keys()
                .chain(e.latest_by.iter())
                .any(|c| !valid_column(c))
            {
                bad(format!("{at}.where"), "invalid");
            }
        }
        Step::Unpack(u) => {
            if u.entries.len() > 32 || u.entries.iter().any(|g| g.trim().is_empty() || long(g)) {
                bad(format!("{at}.entries"), "invalid");
            }
        }
        Step::Read(r) => {
            if r.columns.len() > COLUMNS_MAX {
                bad(format!("{at}.columns"), &format!("max_items:{COLUMNS_MAX}"));
            }
            let mut names = HashSet::new();
            for (j, c) in r.columns.iter().enumerate() {
                if !valid_column(&c.name) || !names.insert(c.name.as_str()) {
                    bad(format!("{at}.columns.{j}.name"), "invalid");
                }
                if c.scale.is_some() != (c.kind == ColumnType::Decimal) {
                    let token = if c.scale.is_some() {
                        "decimal_only"
                    } else {
                        "required"
                    };
                    bad(format!("{at}.columns.{j}.scale"), token);
                }
                if c.scale.is_some_and(|s| s > 18) {
                    bad(format!("{at}.columns.{j}.scale"), "invalid");
                }
                if c.binary.is_some() && r.format != Format::Binary {
                    bad(format!("{at}.columns.{j}.binary"), "binary_only");
                }
            }
            let names: HashSet<&str> = r.columns.iter().map(|c| c.name.as_str()).collect();
            for (k, globs) in &r.r#where {
                if !names.contains(k.as_str()) || globs.is_empty() || globs.len() > VALUES_MAX {
                    bad(format!("{at}.where.{k}"), "invalid");
                }
            }
            for (j, c) in r.columns.iter().enumerate() {
                if c.range.is_some_and(|[lo, hi]| !(lo <= hi))
                    || (c.range.is_some()
                        && !matches!(
                            c.kind,
                            ColumnType::Integer | ColumnType::Decimal | ColumnType::Float
                        ))
                {
                    bad(format!("{at}.columns.{j}.range"), "invalid");
                }
            }
            if r.format == Format::Binary {
                if r.record_size.is_none_or(|n| n == 0 || n > 4096) {
                    bad(format!("{at}.record_size"), "required");
                }
                if r.columns.is_empty()
                    || r.columns
                        .iter()
                        .any(|c| c.binary.is_none() && c.value.is_none())
                {
                    bad(format!("{at}.columns"), "binary_type_required");
                }
                if !matches!(r.endian.as_deref(), None | Some("big") | Some("little")) {
                    bad(format!("{at}.endian"), "invalid");
                }
            }
            if !matches!(r.encoding.as_deref(), None | Some("utf-8") | Some("latin1")) {
                bad(format!("{at}.encoding"), "invalid");
            }
        }
        Step::Sql(s) => {
            if s.sql.trim().is_empty() {
                bad(format!("{at}.sql"), "required");
            }
            if s.sql.len() > SQL_MAX_BYTES {
                bad(format!("{at}.sql"), &format!("max_bytes:{SQL_MAX_BYTES}"));
            }
            if s.tables.keys().any(|k| !valid_name(k)) {
                bad(format!("{at}.tables"), "invalid");
            }
        }
        Step::Call(c) => {
            if c.function.trim().is_empty() || c.function.len() > 255 {
                bad(format!("{at}.function"), "required");
            }
        }
        Step::Write(w) => {
            if [
                w.table.is_some(),
                w.objects.is_some(),
                w.time_ranges.is_some(),
            ]
            .iter()
            .filter(|t| **t)
            .count()
                != 1
            {
                bad(at.to_string(), "one_of:table,objects,time_ranges");
            }
            if let Some(t) = &w.table {
                if !valid_name(&t.name) {
                    bad(format!("{at}.table.name"), "invalid");
                }
            }
            if w.objects.as_ref().is_some_and(|o| o.objects.is_empty()) {
                bad(format!("{at}.objects"), "required");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn doc(steps: serde_json::Value) -> Document {
        serde_json::from_value(json!({
            "key": "minute_bars",
            "hosts": ["data.example.io"],
            "steps": steps,
        }))
        .unwrap()
    }

    fn tokens(d: &Document) -> Vec<String> {
        d.violations()
            .into_iter()
            .map(|v| format!("{} {}", v.path, v.token))
            .collect()
    }

    #[test]
    fn a_form_source_by_pair_and_year_is_valid() {
        let d = doc(json!([
            {"enumerate": {"as": "parts", "track": true, "empty": "retry", "retry_days": 7, "cross": [
                {"as": "pair", "setting": "markets.pairs"},
                {"as": "year", "periods": "year", "from": "2000", "closed_only": true}]}},
            {"fetch": {"as": "page", "for_each": "parts", "url": "https://data.example.io/{{pair}}/{{year}}", "every": "5m"}},
            {"extract": {"as": "form", "from": "page", "form": "#file_down"}},
            {"fetch": {"as": "zip", "for_each": "form", "method": "post", "url": "https://data.example.io/get", "form_from": "fields"}},
            {"unpack": {"as": "csv", "from": "zip", "entries": ["*.csv"]}},
            {"read": {"as": "bars", "from": "csv", "format": "csv", "header": false, "delimiter": ";",
                "columns": [{"name": "ts", "at": 0, "type": "timestamp", "format": "%Y%m%d %H%M%S", "timezone": "America/New_York"},
                            {"name": "open", "at": 1, "type": "decimal", "scale": 8}]}},
            {"write": {"from": "bars", "table": {"name": "ohlcv_1m"}}}
        ]));
        assert_eq!(tokens(&d), Vec::<String>::new());
    }

    #[test]
    fn hosts_tables_and_targets_are_checked() {
        let d = doc(json!([
            {"fetch": {"as": "page", "for_each": "nowhere", "url": "https://other.example.io/x"}},
            {"read": {"as": "page", "from": "page", "format": "csv"}},
            {"write": {"from": "page"}}
        ]));
        let t = tokens(&d);
        assert!(
            t.contains(&"steps.0.fetch.from unknown_table:nowhere".into()),
            "{t:?}"
        );
        assert!(t.contains(&"steps.0.fetch.url host_not_declared:other.example.io".into()));
        assert!(t.contains(&"steps.1.read.as duplicate".into()));
        assert!(t.contains(&"steps.2.write one_of:table,objects,time_ranges".into()));
    }

    #[test]
    fn a_pipeline_without_write_or_with_a_bad_axis_is_refused() {
        let d = doc(json!([
            {"enumerate": {"as": "p", "cross": [{"as": "x", "values": ["a"], "range": [1, 2]}]}}
        ]));
        let t = tokens(&d);
        assert!(t.contains(&"steps write_required".into()), "{t:?}");
        assert!(t.contains(
            &"steps.0.enumerate.cross.0 one_of:values,setting,periods,objects,range".into()
        ));
    }

    #[test]
    fn a_stream_feeds_one_tar_and_one_read() {
        let steps = |extra: serde_json::Value| {
            let mut s = vec![
                json!({"fetch": {"as": "f", "url": "https://data.example.io/x.a{{part}}",
                    "parts": ["a", "b"], "stream": true}}),
                json!({"unpack": {"as": "e", "from": "f", "format": "tar"}}),
                json!({"read": {"as": "r", "from": "e", "format": "json", "columns": [{"name": "x"}]}}),
                json!({"write": {"from": "r", "table": {"name": "t"}}}),
            ];
            if !extra.is_null() {
                s.push(extra);
            }
            serde_json::Value::Array(s)
        };
        let mut d = doc(steps(serde_json::Value::Null));
        assert_eq!(tokens(&d), ["steps.0.fetch.stream invalid"], "large only");
        d.large = true;
        assert_eq!(tokens(&d), Vec::<String>::new());
        // A second reader of the entries: they are never kept.
        let mut d = doc(steps(
            json!({"read": {"as": "r2", "from": "e", "format": "json", "columns": [{"name": "x"}]}}),
        ));
        d.large = true;
        assert_eq!(tokens(&d), ["steps.0.fetch.stream invalid"]);
        let mut d = doc(json!([
            {"fetch": {"as": "f", "url": "https://data.example.io/x", "parts": ["a"]}},
            {"write": {"from": "f", "table": {"name": "t"}}}
        ]));
        d.large = true;
        assert_eq!(tokens(&d), ["steps.0.fetch.parts invalid"]);
    }

    #[test]
    fn post_needs_a_form_and_decimals_a_scale() {
        let d = doc(json!([
            {"fetch": {"as": "b", "url": "https://data.example.io/x", "method": "post"}},
            {"read": {"as": "r", "from": "b", "format": "csv", "columns": [{"name": "p", "type": "decimal"}]}},
            {"write": {"from": "r", "table": {"name": "t"}}}
        ]));
        let t = tokens(&d);
        assert!(
            t.contains(&"steps.0.fetch.form_from post_only".into()),
            "{t:?}"
        );
        assert!(t.contains(&"steps.1.read.columns.0.scale required".into()));
    }

    #[test]
    fn durations_and_hosts_are_read() {
        assert_eq!(duration_ms("250ms"), Some(250));
        assert_eq!(duration_ms("5m"), Some(300_000));
        assert_eq!(duration_ms("5"), None);
        assert_eq!(
            literal_host("https://Data.Example.io/a/{{b}}"),
            Some("data.example.io".into())
        );
        assert_eq!(literal_host("{{url}}"), None);
        assert_eq!(literal_host("https://{{host}}/a"), None);
    }
}
