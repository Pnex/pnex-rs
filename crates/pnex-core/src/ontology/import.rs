//! Declarative imports into the ontology (partner-watch.md D301): a YAML
//! mapping turns the records of a file or an API into planned objects
//! (upserted on a unique property, D302) and links. No code in a mapping:
//! column paths, constants and a closed list of conversions only.
//!
//! Pure and wasm32-safe: the backend reads the source, plans every row
//! here, then diffs the plan against the database.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::schema::{check_values, valid_key, PropertyDef};
use super::{localized, LinkTypeDef, Names, ObjectTypeDef, PropertyKind};
use crate::err_codes::{FIELD_INVALID, FIELD_MAX_LENGTH, FIELD_REQUIRED};

/// Rows of one run: 50,000 until the bulk write (data-pipelines.md D347),
/// which applies FC networks of ~55k filtered rows in one go. Rows are
/// held in memory, the 200 MB and `PLANNED_MAX` caps bound a run.
pub const ROWS_MAX: usize = 250_000;
/// Objects and links declared by one mapping.
pub const ENTRIES_MAX: usize = 20;
/// Pipeline steps of one import.
pub const STEPS_MAX: usize = 10;
/// Elements expanded by one `each` in one row (an RS sheet lists up to
/// ~4,300 partners).
pub const EACH_MAX: usize = 5_000;
/// Objects planned by one run, every row and `each` together.
pub const PLANNED_MAX: usize = 500_000;
pub const PATH_MAX: usize = 255;
/// YAML text of a mapping.
pub const MAPPING_MAX_BYTES: usize = 64 * 1024;

/// Token of a column absent from the record.
pub const FIELD_MISSING_COLUMN: &str = "missing_column";
/// Token of a lookup key that names no object (`create: false`).
pub const FIELD_UNKNOWN_KEY: &str = "unknown_key";

/// File or response format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum SourceFormat {
    Csv,
    Xlsx,
    Json,
    Xml,
}

impl SourceFormat {
    /// From a file name (`.zip` holds one file of another format).
    pub fn of_name(name: &str) -> Option<Self> {
        let ext = name.rsplit('.').next()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "csv" | "txt" => Self::Csv,
            "xlsx" | "xlsm" | "ods" => Self::Xlsx,
            "json" => Self::Json,
            "xml" => Self::Xml,
            _ => return None,
        })
    }
}

/// A value read from the record: a column path or a constant, then the
/// conversions in order.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FieldSpec {
    /// Column name, or a dot path in a JSON/XML record; `.` = the element
    /// of an `each`, `#` = its position (from 1), `^path` = the record
    /// holding it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Constant used instead of a column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<Object>))]
    pub value: Option<Value>,
    /// Several paths whose non-empty values are joined by `sep`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub concat: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sep: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub convert: Vec<Convert>,
    /// On an object property, `skip`: a value its conversion or the schema
    /// refuses is a warning, not an error. The property is left out of the
    /// row (the object keeps its value); for the object's key, the object
    /// and its links are left out of the row.
    #[serde(default, skip_serializing_if = "Missing::is_error")]
    pub invalid: Missing,
}

/// A field is written either as a bare column path or as a full spec.
impl<'de> Deserialize<'de> for FieldSpec {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Full {
            #[serde(default)]
            from: Option<String>,
            #[serde(default)]
            value: Option<Value>,
            #[serde(default)]
            concat: Vec<String>,
            #[serde(default)]
            sep: Option<String>,
            #[serde(default)]
            convert: Vec<Convert>,
            #[serde(default)]
            invalid: Missing,
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Path(String),
            Full(Full),
        }
        Ok(match Repr::deserialize(d)? {
            Repr::Path(p) => Self {
                from: Some(p),
                value: None,
                concat: Vec::new(),
                sep: None,
                convert: Vec::new(),
                invalid: Missing::Error,
            },
            Repr::Full(f) => Self {
                from: f.from,
                value: f.value,
                concat: f.concat,
                sep: f.sep,
                convert: f.convert,
                invalid: f.invalid,
            },
        })
    }
}

/// The closed list of conversions (D301). Written as a string
/// (`trim`, `digits`, `left:9`, `date:%d/%m/%Y`, `money:EUR`, `split:;`…)
/// or, for `enum`, as `{enum: {source: target}}`.
#[derive(Debug, Clone, PartialEq)]
pub enum Convert {
    Trim,
    Digits,
    Upper,
    Lower,
    /// The first `n` characters.
    Left(usize),
    /// A date in this chrono format → `YYYY-MM-DD`.
    Date(String),
    /// A number → `{"amount", "currency"}`.
    Money(String),
    /// One value per part (only in an `each`).
    Split(String),
    /// One value per paragraph: parts separated by a run of at least `n`
    /// blanks (only in an `each`).
    SplitSpaces(usize),
    /// The text before the first separator (all of it when absent): after
    /// a `concat`, the first non-empty column (a coalesce).
    Before(String),
    /// The text after the last separator (all of it when absent).
    After(String),
    /// Of a list of elements (or a lone one), the one whose field is the
    /// largest (numbers compared as numbers): the latest year.
    MaxBy(String),
    /// A field of an element (after `max_by`).
    Field(String),
    /// A text holding JSON, decoded (then `field:` reads into it): some
    /// APIs ship a structure as a string (BODACC).
    Json,
    /// `scheme://host` of a URL or a bare domain (`https` by default).
    UrlOrigin,
    /// Host of a URL or a bare domain, lower case, without `www.`.
    Domain,
    /// Source value → target value; an unlisted value is an error, unless
    /// the map holds `"*"` (the value for every other one).
    Enum(BTreeMap<String, String>),
    /// Digits passing the Luhn check (SIREN, SIRET), else an error.
    Luhn,
    /// A single e-mail address, lower case, else an error.
    Email,
}

impl Convert {
    /// Yields several values (only in an `each`).
    fn splits(&self) -> bool {
        matches!(self, Self::Split(_) | Self::SplitSpaces(_))
    }

    fn parse(s: &str) -> Option<Self> {
        let (name, arg) = s.split_once(':').unwrap_or((s, ""));
        Some(match (name, arg) {
            ("trim", "") => Self::Trim,
            ("digits", "") => Self::Digits,
            ("upper", "") => Self::Upper,
            ("lower", "") => Self::Lower,
            ("url_origin", "") => Self::UrlOrigin,
            ("luhn", "") => Self::Luhn,
            ("email", "") => Self::Email,
            ("domain", "") => Self::Domain,
            ("json", "") => Self::Json,
            ("left", n) => Self::Left(n.parse().ok().filter(|n| *n > 0)?),
            ("date", f) if !f.is_empty() && f.len() <= 32 => Self::Date(f.into()),
            ("money", c) if c.len() == 3 && c.bytes().all(|b| b.is_ascii_uppercase()) => {
                Self::Money(c.into())
            }
            ("split", sep) if !sep.is_empty() && sep.len() <= 4 => Self::Split(sep.into()),
            ("before", sep) if !sep.is_empty() && sep.len() <= 4 => Self::Before(sep.into()),
            ("after", sep) if !sep.is_empty() && sep.len() <= 4 => Self::After(sep.into()),
            ("max_by", f) if !f.is_empty() && f.len() <= PATH_MAX => Self::MaxBy(f.into()),
            ("field", f) if !f.is_empty() && f.len() <= PATH_MAX => Self::Field(f.into()),
            ("split_spaces", n) => {
                Self::SplitSpaces(n.parse().ok().filter(|n| (2..=16).contains(n))?)
            }
            _ => return None,
        })
    }

    fn wire(&self) -> Value {
        Value::String(match self {
            Self::Trim => "trim".into(),
            Self::Digits => "digits".into(),
            Self::Upper => "upper".into(),
            Self::Lower => "lower".into(),
            Self::UrlOrigin => "url_origin".into(),
            Self::Domain => "domain".into(),
            Self::Json => "json".into(),
            Self::Luhn => "luhn".into(),
            Self::Email => "email".into(),
            Self::Left(n) => format!("left:{n}"),
            Self::Date(f) => format!("date:{f}"),
            Self::Money(c) => format!("money:{c}"),
            Self::Split(s) => format!("split:{s}"),
            Self::Before(s) => format!("before:{s}"),
            Self::After(s) => format!("after:{s}"),
            Self::MaxBy(f) => format!("max_by:{f}"),
            Self::Field(f) => format!("field:{f}"),
            Self::SplitSpaces(n) => format!("split_spaces:{n}"),
            Self::Enum(m) => return serde_json::json!({ "enum": m }),
        })
    }
}

impl Serialize for Convert {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.wire().serialize(s)
    }
}

impl<'de> Deserialize<'de> for Convert {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Name(String),
            Enum {
                #[serde(rename = "enum")]
                map: BTreeMap<String, String>,
            },
        }
        match Repr::deserialize(d)? {
            Repr::Name(s) => Self::parse(&s).ok_or_else(|| D::Error::custom("unknown conversion")),
            Repr::Enum { map } if !map.is_empty() && map.len() <= 200 => Ok(Self::Enum(map)),
            Repr::Enum { .. } => Err(D::Error::custom("empty enum")),
        }
    }
}

#[cfg(feature = "openapi")]
impl utoipa::PartialSchema for Convert {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        utoipa::openapi::schema::ObjectBuilder::new()
            .description(Some(
                "A conversion: `trim`, `digits`, `upper`, `lower`, `left:<n>`, `date:<format>`, `money:<currency>`, `split:<sep>`, `before:<sep>`, `after:<sep>`, `max_by:<field>`, `field:<path>`, `json`, `url_origin`, `domain`, `luhn`, `email`, or `{enum: {source: target, \"*\": other}}`.",
            ))
            .into()
    }
}
#[cfg(feature = "openapi")]
impl utoipa::ToSchema for Convert {}

/// One API request per live object of `type` whose `property` is set, its
/// value sent as the query parameter `param`, or put in place of `{value}`
/// in the source URL when `param` is empty (a path segment, a SPARQL
/// literal). The objects are taken in turn, `max_pages` requests per run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct PerObject {
    #[serde(rename = "type")]
    pub type_key: String,
    pub property: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub param: String,
    /// Only the objects whose property holds one of these values (the
    /// accounts followed, not every company known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub only: Option<PerObjectOnly>,
    /// Only the objects holding an open link of one of these types, either
    /// way: a certifier's network, not every company the org knows
    /// (network-check.md D403).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub linked: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct PerObjectOnly {
    pub property: String,
    pub values: Vec<String>,
}

/// Latitude and longitude of a `map_pin` created by the import.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct LocationSpec {
    pub lat: FieldSpec,
    pub lon: FieldSpec,
}

/// Objects produced by each record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ObjectMap {
    /// Name of this entry for the links of the mapping.
    #[serde(rename = "as")]
    pub alias: String,
    #[serde(rename = "type")]
    pub type_key: String,
    /// The unique property (D302) matched for the upsert.
    pub key: String,
    /// `false`: the object must exist (a lookup), it is never created.
    #[serde(default = "yes")]
    pub create: bool,
    /// Existing objects get the mapped properties; default: as `create`
    /// (a lookup changes nothing, `create: false, update: true` enriches
    /// only what exists).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<bool>,
    /// One object per element of this array (or split value).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub each: Option<FieldSpec>,
    /// Title; the key value when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<FieldSpec>,
    /// Required to create a `map_pin`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<LocationSpec>,
    /// Property key → value. The key property is mapped here too.
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub properties: BTreeMap<String, FieldSpec>,
    /// Constants written only when the import creates the object, never
    /// over an existing one (a new target enters as `cible`, a client stays
    /// a client).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub defaults: BTreeMap<String, Value>,
    /// Snapshot mode: archive the objects of this import absent from the
    /// run.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub close_missing: bool,
    /// Only the records (or `each` elements) where this path equals the
    /// value produce the object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<When>,
    /// A lookup (`create: false`) whose key names no object: `error` (the
    /// row is refused) or `skip` (the row only enriches what exists).
    #[serde(default, skip_serializing_if = "Missing::is_error")]
    pub missing: Missing,
    /// Entry (an earlier `as`) this object needs: when it resolves to no
    /// object (a skipped lookup), this one is not written either.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct When {
    pub from: String,
    pub equals: String,
    /// `true`: the records where the path does NOT equal the value.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub not: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Missing {
    #[default]
    Error,
    Skip,
}

impl Missing {
    pub fn is_error(&self) -> bool {
        *self == Self::Error
    }
}

/// Links produced by each record, between two entries of the mapping.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct LinkMap {
    #[serde(rename = "type")]
    pub link_type: String,
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<FieldSpec>,
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub attributes: BTreeMap<String, FieldSpec>,
    /// Snapshot mode: close the links of this import absent from the run.
    #[serde(default = "yes")]
    pub close_missing: bool,
    /// `false`: only an open link gets the attributes, none is opened.
    #[serde(default = "yes")]
    pub create: bool,
    /// A new link is a proposal waiting for a human, not a live link
    /// (community data; proposals.md D375).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub propose: bool,
    /// Several records of one pair disagreeing on an attribute: it takes
    /// this value (`perimetre: les_deux` merges preparation and
    /// evaluation). Without it the first record wins, with a warning.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub on_conflict: BTreeMap<String, Value>,
}

fn yes() -> bool {
    true
}

/// A mapping (D301): where the records are, what each one produces.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Mapping {
    pub key: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Names::is_empty")]
    pub names: Names,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<SourceFormat>,
    /// JSON: dot path of the record array (empty = the root); XML: path of
    /// the record elements (`root/item`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rows: String,
    /// CSV separator; `,` or `;` guessed from the header when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delimiter: Option<char>,
    /// Paginated API: query parameter carrying the page number (from 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_param: Option<String>,
    /// Paginated API: dot path of the next-page cursor in the response,
    /// sent back under `cursor_param`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_param: Option<String>,
    /// API: one request per live object instead of pages (a register
    /// queried by identifier).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_object: Option<PerObject>,
    /// Pipeline steps run by the server before the mapping (data-pipelines.md
    /// D344): resolve, download, extract, inspect, filter; the records are
    /// read from the file they leave staged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "openapi", schema(value_type = Vec<Object>))]
    pub steps: Vec<crate::ingest::IngestConfig>,
    #[serde(default)]
    pub objects: Vec<ObjectMap>,
    #[serde(default)]
    pub links: Vec<LinkMap>,
}

impl Mapping {
    pub fn display_name(&self, lang: &str) -> &str {
        localized(&self.name, &self.names, lang)
    }

    /// Every column path the mapping reads (for the schema check, D303).
    pub fn columns(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut push = |f: &FieldSpec| {
            if let Some(p) = f.from.as_deref().filter(|p| *p != "." && *p != "#") {
                if !out.iter().any(|c| c == p) {
                    out.push(p.to_string());
                }
            }
        };
        for o in &self.objects {
            if let Some(e) = &o.each {
                push(e);
            }
            // Paths inside an `each` are relative to its element.
            if o.each.is_none() {
                o.properties.values().for_each(&mut push);
                o.title.iter().for_each(&mut push);
            }
        }
        let each = |alias: &str| {
            self.objects
                .iter()
                .any(|o| o.alias == alias && o.each.is_some())
        };
        for l in &self.links {
            // Read from the element when the link touches an `each`.
            if each(&l.from) || each(&l.to) {
                continue;
            }
            l.attributes.values().for_each(&mut push);
            l.valid_from.iter().for_each(&mut push);
        }
        out
    }
}

/// The types and link types a mapping is checked against.
pub struct Schema<'a> {
    pub types: &'a [ObjectTypeDef],
    pub links: &'a [LinkTypeDef],
}

impl Schema<'_> {
    fn object_type(&self, key: &str) -> Option<&ObjectTypeDef> {
        self.types.iter().find(|t| t.key == key)
    }
}

type FieldError = (String, String);

fn ferr(field: impl Into<String>, token: impl Into<String>) -> FieldError {
    (field.into(), token.into())
}

fn check_field(at: &str, f: &FieldSpec) -> Result<(), FieldError> {
    let path_ok = |p: &String| !p.trim().is_empty() && p.len() <= PATH_MAX;
    match (&f.from, &f.value, f.concat.is_empty()) {
        (Some(p), None, true) if path_ok(p) => {}
        (None, Some(_), true) => {}
        (None, None, false) if f.concat.len() <= 8 && f.concat.iter().all(path_ok) => {}
        _ => return Err(ferr(at, FIELD_INVALID)),
    }
    if f.convert.len() > 8 {
        return Err(ferr(format!("{at}.convert"), FIELD_INVALID));
    }
    Ok(())
}

/// Checks a mapping against the org schema (field paths `objects.<i>.…`).
pub fn check(m: &Mapping, s: &Schema) -> Result<(), FieldError> {
    if !valid_key(&m.key) {
        return Err(ferr("key", FIELD_INVALID));
    }
    if m.name.trim().is_empty() {
        return Err(ferr("name", FIELD_REQUIRED));
    }
    if m.objects.is_empty() {
        return Err(ferr("objects", FIELD_REQUIRED));
    }
    if m.objects.len() + m.links.len() > ENTRIES_MAX {
        return Err(ferr("objects", format!("{FIELD_MAX_LENGTH}:{ENTRIES_MAX}")));
    }
    if m.rows.len() > PATH_MAX {
        return Err(ferr("rows", FIELD_INVALID));
    }
    if m.steps.len() > STEPS_MAX {
        return Err(ferr("steps", format!("{FIELD_MAX_LENGTH}:{STEPS_MAX}")));
    }
    for (i, step) in m.steps.iter().enumerate() {
        use crate::ingest::IngestStep;
        // The import is the end of its pipeline; the staging is cleaned
        // when the run ends.
        let allowed = !matches!(
            step.step,
            IngestStep::OntologyImport | IngestStep::StagingCleanup
        );
        if !allowed || step.check_deployable().is_some() || m.per_object.is_some() {
            return Err(ferr(format!("steps.{i}"), FIELD_INVALID));
        }
    }
    if let Some(p) = &m.per_object {
        let known = s
            .object_type(&p.type_key)
            .is_some_and(|t| t.properties.iter().any(|d| d.key == p.property));
        if !known || p.param.trim() != p.param || p.param.len() > 64 {
            return Err(ferr("per_object", FIELD_INVALID));
        }
        if let Some(o) = &p.only {
            let known = s
                .object_type(&p.type_key)
                .is_some_and(|t| t.properties.iter().any(|d| d.key == o.property));
            if !known || o.values.is_empty() || o.values.len() > 50 {
                return Err(ferr("per_object.only", FIELD_INVALID));
            }
        }
        if p.linked.len() > 8
            || p.linked
                .iter()
                .any(|l| !s.links.iter().any(|d| &d.key == l))
        {
            return Err(ferr("per_object.linked", FIELD_INVALID));
        }
    }
    for (i, o) in m.objects.iter().enumerate() {
        let at = |f: &str| format!("objects.{i}.{f}");
        if !valid_key(&o.alias) || m.objects[..i].iter().any(|p| p.alias == o.alias) {
            return Err(ferr(at("as"), FIELD_INVALID));
        }
        let Some(t) = s.object_type(&o.type_key) else {
            return Err(ferr(at("type"), FIELD_INVALID));
        };
        if !t.properties.iter().any(|p| p.key == o.key && p.unique) {
            return Err(ferr(at("key"), FIELD_INVALID));
        }
        if !o.properties.contains_key(&o.key) {
            return Err(ferr(at("properties"), FIELD_REQUIRED));
        }
        for (k, f) in &o.properties {
            let Some(d) = t.properties.iter().find(|p| &p.key == k) else {
                return Err(ferr(at(&format!("properties.{k}")), FIELD_INVALID));
            };
            if d.kind.is_temporal() || f.convert.iter().any(Convert::splits) {
                return Err(ferr(at(&format!("properties.{k}")), FIELD_INVALID));
            }
            check_field(&at(&format!("properties.{k}")), f)?;
        }
        for k in o.defaults.keys() {
            let known = t
                .properties
                .iter()
                .any(|d| &d.key == k && !d.kind.is_temporal());
            if !known || *k == o.key || !o.create {
                return Err(ferr(at(&format!("defaults.{k}")), FIELD_INVALID));
            }
        }
        for (name, f) in [("each", &o.each), ("title", &o.title)] {
            if let Some(f) = f {
                check_field(&at(name), f)?;
            }
        }
        let pin = o.type_key == crate::resources::KIND_MAP_PIN;
        match (&o.location, pin && o.create) {
            (Some(l), true) => {
                check_field(&at("location.lat"), &l.lat)?;
                check_field(&at("location.lon"), &l.lon)?;
            }
            (None, true) => return Err(ferr(at("location"), FIELD_REQUIRED)),
            (Some(_), false) => return Err(ferr(at("location"), FIELD_INVALID)),
            (None, false) => {}
        }
        if o.needs
            .as_deref()
            .is_some_and(|a| !m.objects[..i].iter().any(|p| p.alias == a))
        {
            return Err(ferr(at("needs"), FIELD_INVALID));
        }
        // Other system objects are created by their own pages.
        if t.native() && !pin && o.create {
            return Err(ferr(at("create"), FIELD_INVALID));
        }
    }
    for (i, l) in m.links.iter().enumerate() {
        let at = |f: &str| format!("links.{i}.{f}");
        let end = |alias: &str| m.objects.iter().find(|o| o.alias == alias);
        let (Some(from), Some(to)) = (end(&l.from), end(&l.to)) else {
            return Err(ferr(at("from"), FIELD_INVALID));
        };
        if from.each.is_some() && to.each.is_some() && from.alias != to.alias {
            return Err(ferr(at("to"), FIELD_INVALID));
        }
        let Some(lt) = s.links.iter().find(|d| d.key == l.link_type) else {
            return Err(ferr(at("type"), FIELD_INVALID));
        };
        if !lt.allows(&l.link_type, &from.type_key, &to.type_key) {
            return Err(ferr(at("type"), FIELD_INVALID));
        }
        for (k, f) in &l.attributes {
            if !lt.attributes.iter().any(|a| &a.key == k) {
                return Err(ferr(at(&format!("attributes.{k}")), FIELD_INVALID));
            }
            check_field(&at(&format!("attributes.{k}")), f)?;
        }
        if let Some(f) = &l.valid_from {
            check_field(&at("valid_from"), f)?;
        }
        for (k, v) in &l.on_conflict {
            let Some(def) = lt.attributes.iter().find(|a| &a.key == k) else {
                return Err(ferr(at(&format!("on_conflict.{k}")), FIELD_INVALID));
            };
            let one = cast(def, v.clone()).map(|v| Map::from_iter([(k.clone(), v)]));
            if !one.is_ok_and(|one| check_values(std::slice::from_ref(def), &one).is_ok()) {
                return Err(ferr(at(&format!("on_conflict.{k}")), FIELD_INVALID));
            }
        }
    }
    Ok(())
}

/// Parses a mapping from YAML-compatible JSON (the backend parses YAML).
pub fn from_value(v: Value) -> Result<Mapping, FieldError> {
    serde_json::from_value(v).map_err(|_| ferr("yaml", FIELD_INVALID))
}

// ───────────────────────────── reading records ─────────────────────────────

/// Parts of `s` separated by a run of at least `n` blanks, trimmed, empty
/// ones dropped.
fn paragraphs(s: &str, n: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut start, mut run, mut run_from) = (0, 0, 0);
    for (i, c) in s.char_indices() {
        if c.is_whitespace() {
            if run == 0 {
                run_from = i;
            }
            run += 1;
            continue;
        }
        if run >= n {
            out.push(&s[start..run_from]);
            start = i;
        }
        run = 0;
    }
    out.push(&s[start..]);
    out.into_iter()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect()
}

/// The value at a dot path (`a.b.0.c`); `.` or empty = the record itself.
pub fn get_path<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    if path.is_empty() || path == "." {
        return Some(v);
    }
    // A flat record (CSV, sheet) may hold dots in its column names.
    if let Some(found) = v.as_object().and_then(|o| o.get(path)) {
        return Some(found);
    }
    path.split('.').try_fold(v, |cur, seg| match cur {
        // `@last`: the greatest key (the latest year of `{"2024": …, "2025": …}`).
        Value::Object(o) if seg == LAST => o.iter().max_by(|a, b| a.0.cmp(b.0)).map(|(_, v)| v),
        Value::Object(o) => o.get(seg),
        Value::Array(a) if seg == LAST => a.last(),
        Value::Array(a) => a.get(seg.parse::<usize>().ok()?),
        _ => None,
    })
}

/// Path segment naming the last entry of an object (by key) or an array.
pub const LAST: &str = "@last";

/// The records of a JSON document at `path` (an array, or a lone object).
pub fn json_rows(doc: &Value, path: &str) -> Option<Vec<Value>> {
    match get_path(doc, path)? {
        Value::Array(a) => Some(a.clone()),
        o @ Value::Object(_) => Some(vec![o.clone()]),
        _ => None,
    }
}

/// RFC 4180 CSV (quoted fields, doubled quotes, newlines in quotes).
pub fn parse_csv(text: &str, delimiter: Option<char>) -> Vec<Vec<String>> {
    let text = text.trim_start_matches('\u{feff}');
    let delim = delimiter.unwrap_or_else(|| {
        let head = text.lines().next().unwrap_or_default();
        if head.matches(';').count() > head.matches(',').count() {
            ';'
        } else {
            ','
        }
    });
    let mut rows = Vec::new();
    let (mut row, mut cell) = (Vec::new(), String::new());
    let (mut quoted, mut chars) = (false, text.chars().peekable());
    while let Some(c) = chars.next() {
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                cell.push('"');
                chars.next();
            }
            ('"', true) => quoted = false,
            ('"', false) if cell.is_empty() => quoted = true,
            (c, false) if c == delim => row.push(std::mem::take(&mut cell)),
            ('\r', false) => {}
            ('\n', false) => {
                row.push(std::mem::take(&mut cell));
                if row.iter().any(|c| !c.is_empty()) {
                    rows.push(std::mem::take(&mut row));
                }
                row.clear();
            }
            (c, _) => cell.push(c),
        }
    }
    row.push(cell);
    if row.iter().any(|c| !c.is_empty()) {
        rows.push(row);
    }
    rows
}

/// Records of a table whose first row holds the headers (trimmed).
pub fn table_rows(table: Vec<Vec<String>>) -> Vec<Value> {
    let mut it = table.into_iter();
    let Some(head) = it.next() else {
        return Vec::new();
    };
    let head: Vec<String> = head.iter().map(|h| h.trim().to_string()).collect();
    it.map(|r| {
        Value::Object(
            head.iter()
                .zip(r.into_iter().chain(std::iter::repeat(String::new())))
                .filter(|(h, _)| !h.is_empty())
                .map(|(h, c)| (h.clone(), Value::String(c)))
                .collect(),
        )
    })
    .collect()
}

// ───────────────────────────── planning rows ─────────────────────────────

/// An error or warning of a record: 1-based row, column (or field path),
/// machine token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RowIssue {
    pub row: usize,
    pub column: String,
    pub token: String,
}

/// An object a record asks for.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedObject {
    pub entry: usize,
    pub type_key: String,
    pub key_prop: String,
    /// Text of the key value, as `properties ->> key` reads it.
    pub key: String,
    pub title: String,
    /// The title comes from the mapping (else it is the key, and an
    /// update leaves the current title alone).
    pub titled: bool,
    pub properties: Map<String, Value>,
    /// Written on creation only, under `properties`.
    pub defaults: Map<String, Value>,
    pub location: Option<(f64, f64)>,
    pub create: bool,
    pub update: bool,
    pub missing: Missing,
    /// Indexes in `RowPlan::objects` of the objects of the entry it needs.
    pub needs: Vec<usize>,
}

/// A link between two planned objects (indexes in `RowPlan::objects`).
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedLink {
    pub entry: usize,
    pub link_type: String,
    pub from: usize,
    pub to: usize,
    pub valid_from: Option<String>,
    pub attributes: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RowPlan {
    pub objects: Vec<PlannedObject>,
    pub links: Vec<PlannedLink>,
    /// Values left out under `invalid: skip`.
    pub warnings: Vec<RowIssue>,
    /// Links of the row whose other end was left out by its key: their
    /// existing links on this end are kept as they are.
    pub held: Vec<HeldEnd>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HeldEnd {
    pub link_type: String,
    /// Index in `objects` of the end the row still knows.
    pub end: usize,
    /// That end is the link's source.
    pub is_source: bool,
}

/// Text form of a scalar read from a record.
fn text_of(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn origin_of(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let with_scheme = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("https://{raw}")
    };
    let u = url::Url::parse(&with_scheme).ok()?;
    if !matches!(u.scheme(), "http" | "https") {
        return None;
    }
    let host = u.host_str()?.to_ascii_lowercase();
    Some(match u.port() {
        Some(p) => format!("{}://{host}:{p}", u.scheme()),
        None => format!("{}://{host}", u.scheme()),
    })
}

/// Applies string conversions; `Ok(None)` = empty value.
pub fn convert(v: Option<Value>, convs: &[Convert]) -> Result<Option<Value>, String> {
    let mut cur = match v {
        None | Some(Value::Null) => return Ok(None),
        Some(v) => v,
    };
    for c in convs {
        // Structure conversions work on elements, not on text.
        match c {
            Convert::MaxBy(f) => {
                let key = |e: &Value| {
                    let t = get_path(e, f).and_then(text_of).unwrap_or_default();
                    (number_of(&t).unwrap_or(f64::NEG_INFINITY), t)
                };
                cur = match cur {
                    Value::Array(a) => a
                        .into_iter()
                        .max_by(|x, y| {
                            key(x)
                                .partial_cmp(&key(y))
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                        .unwrap_or(Value::Null),
                    one => one,
                };
                continue;
            }
            Convert::Field(f) => {
                cur = get_path(&cur, f).cloned().unwrap_or(Value::Null);
                if cur.is_null() {
                    return Ok(None);
                }
                continue;
            }
            Convert::Json => {
                if let Value::String(t) = &cur {
                    // Not JSON: no value rather than a refused record.
                    let Ok(v) = serde_json::from_str::<Value>(t) else {
                        return Ok(None);
                    };
                    cur = v;
                }
                continue;
            }
            _ => {}
        }
        let s = match text_of(&cur) {
            Some(s) => s,
            None if c.splits() => return Ok(Some(cur)),
            None => return Err(FIELD_INVALID.into()),
        };
        cur = match c {
            Convert::MaxBy(_) | Convert::Field(_) | Convert::Json => unreachable!("handled above"),
            Convert::Trim => Value::String(s.trim().to_string()),
            Convert::Digits => Value::String(s.chars().filter(char::is_ascii_digit).collect()),
            Convert::Upper => Value::String(s.to_uppercase()),
            Convert::Lower => Value::String(s.to_lowercase()),
            Convert::Left(n) => Value::String(s.chars().take(*n).collect()),
            Convert::Date(f) => {
                let s = s.trim();
                if s.is_empty() {
                    return Ok(None);
                }
                // A sheet date cell is already ISO (`import_read`).
                let d = chrono::NaiveDate::parse_from_str(s, f)
                    .or_else(|_| chrono::NaiveDateTime::parse_from_str(s, f).map(|t| t.date()))
                    .or_else(|_| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d"))
                    .map_err(|_| FIELD_INVALID.to_string())?;
                Value::String(d.format("%Y-%m-%d").to_string())
            }
            Convert::Money(currency) => {
                let n = number_of(&s).ok_or_else(|| FIELD_INVALID.to_string())?;
                serde_json::json!({ "amount": n, "currency": currency })
            }
            Convert::Before(sep) => Value::String(
                s.split(sep.as_str())
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            ),
            Convert::After(sep) => Value::String(
                s.rsplit(sep.as_str())
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            ),
            Convert::SplitSpaces(n) => Value::Array(
                paragraphs(&s, *n)
                    .into_iter()
                    .map(|p| Value::String(p.into()))
                    .collect(),
            ),
            Convert::Split(sep) => Value::Array(
                s.split(sep.as_str())
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(|p| Value::String(p.into()))
                    .collect(),
            ),
            Convert::UrlOrigin => match s.trim() {
                "" => return Ok(None),
                t => Value::String(origin_of(t).ok_or_else(|| FIELD_INVALID.to_string())?),
            },
            Convert::Domain => match s.trim() {
                "" => return Ok(None),
                t => {
                    let o = origin_of(t).ok_or_else(|| FIELD_INVALID.to_string())?;
                    let host = o.split("://").nth(1).unwrap_or_default();
                    let host = host.split(':').next().unwrap_or_default();
                    Value::String(host.trim_start_matches("www.").to_string())
                }
            },
            Convert::Enum(map) => match s.trim() {
                "" => return Ok(None),
                t => Value::String(
                    map.get(t)
                        .or_else(|| map.get(&t.to_lowercase()))
                        .or_else(|| map.get("*"))
                        .cloned()
                        .ok_or_else(|| FIELD_INVALID.to_string())?,
                ),
            },
            Convert::Luhn => match s.trim() {
                "" => return Ok(None),
                t if luhn(t) => Value::String(t.to_string()),
                _ => return Err(FIELD_INVALID.into()),
            },
            Convert::Email => match s.trim() {
                "" => return Ok(None),
                t if valid_email(t) => Value::String(t.to_lowercase()),
                _ => return Err(FIELD_INVALID.into()),
            },
        };
    }
    Ok(match cur {
        Value::String(s) if s.trim().is_empty() => None,
        v => Some(v),
    })
}

/// Luhn check over a string of digits (SIREN, SIRET).
pub fn luhn(s: &str) -> bool {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let sum: u32 = s
        .bytes()
        .rev()
        .enumerate()
        .map(|(i, b)| {
            let d = u32::from(b - b'0');
            match i % 2 {
                1 if d * 2 > 9 => d * 2 - 9,
                1 => d * 2,
                _ => d,
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

/// One address `local@domain.tld`, 254 characters at most, no spaces nor
/// list separators.
pub fn valid_email(s: &str) -> bool {
    let Some((local, domain)) = s.rsplit_once('@') else {
        return false;
    };
    s.len() <= 254
        && !local.is_empty()
        && local.len() <= 64
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !s
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, ',' | ';' | '<' | '>' | '"' | '(' | ')'))
        && !domain.contains('@')
}

/// A number written the French way or not (`1 234,50`, `1234.5`).
fn number_of(s: &str) -> Option<f64> {
    let t: String = s
        .trim()
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '\u{a0}' && *c != '\u{202f}')
        .collect();
    let t = if t.contains(',') && !t.contains('.') {
        t.replace(',', ".")
    } else {
        t.replace(',', "")
    };
    t.parse::<f64>().ok().filter(|n| n.is_finite())
}

/// Casts a read value to the kind of the property it feeds; the result is
/// validated by `check_values` afterwards.
fn cast(def: &PropertyDef, v: Value) -> Result<Value, String> {
    let invalid = || FIELD_INVALID.to_string();
    Ok(match (&def.kind, v) {
        (PropertyKind::Number { .. }, Value::String(s)) => {
            serde_json::Number::from_f64(number_of(&s).ok_or_else(invalid)?)
                .map(Value::Number)
                .ok_or_else(invalid)?
        }
        (PropertyKind::Bool, Value::String(s)) => match s.trim().to_lowercase().as_str() {
            "true" | "oui" | "yes" | "1" | "o" | "y" | "vrai" => Value::Bool(true),
            "false" | "non" | "no" | "0" | "n" | "faux" => Value::Bool(false),
            _ => return Err(invalid()),
        },
        (PropertyKind::Text { .. }, v @ (Value::Number(_) | Value::Bool(_))) => {
            Value::String(text_of(&v).unwrap_or_default())
        }
        (_, v) => v,
    })
}

/// The element of an `each` and its position (from 1).
type Elem<'a> = Option<(&'a Value, usize)>;

/// A path of a field: `#` = the element's position, `^path` = in the
/// record, else in the element (the record outside an `each`).
fn at_path(row: &Value, elem: Elem, p: &str) -> Option<Value> {
    if p == "#" {
        return elem.map(|(_, n)| Value::from(n));
    }
    match p.strip_prefix('^') {
        Some(p) => get_path(row, p).cloned(),
        None => get_path(elem.map_or(row, |(e, _)| e), p).cloned(),
    }
}

fn read(row: &Value, elem: Elem, f: &FieldSpec) -> Result<Option<Value>, (String, String)> {
    let col = f.from.clone().unwrap_or_else(|| f.concat.join("+"));
    let raw = match (&f.from, &f.value) {
        (_, Some(v)) => Some(v.clone()),
        (Some(p), None) => at_path(row, elem, p),
        (None, None) if !f.concat.is_empty() => {
            let parts: Vec<String> = f
                .concat
                .iter()
                .filter_map(|p| at_path(row, elem, p).as_ref().and_then(text_of))
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect();
            Some(Value::String(parts.join(f.sep.as_deref().unwrap_or(" "))))
        }
        (None, None) => None,
    };
    convert(raw, &f.convert).map_err(|t| (col, t))
}

/// Plans one record: objects (with `each` expanded) and links. Returns
/// the plan, or the issues of the record (it is then skipped whole).
pub fn plan_row(
    m: &Mapping,
    s: &Schema,
    row_no: usize,
    row: &Value,
) -> Result<RowPlan, Vec<RowIssue>> {
    let mut plan = RowPlan::default();
    let mut issues = Vec::new();
    let issue = |(column, token): (String, String)| RowIssue {
        row: row_no,
        column,
        token,
    };
    // Indexes in `plan.objects` of each entry (one, or one per element).
    let mut produced: Vec<Vec<usize>> = Vec::new();
    let mut warnings = Vec::new();
    // Entries left out by an invalid key (`invalid: skip`).
    let mut skipped = vec![false; m.objects.len()];
    // The `each` element of every planned object, with its position.
    let mut elems: Vec<Option<(Value, usize)>> = Vec::new();
    for (i, o) in m.objects.iter().enumerate() {
        let mut mine = Vec::new();
        let def = s.object_type(&o.type_key);
        // An object needing an entry that produced nothing is not planned.
        let needs = match o
            .needs
            .as_deref()
            .and_then(|a| m.objects.iter().position(|x| x.alias == a))
        {
            Some(n) => match produced.get(n) {
                Some(p) if !p.is_empty() => p.clone(),
                _ => {
                    produced.push(mine);
                    continue;
                }
            },
            None => Vec::new(),
        };
        let Some(def) = def else {
            produced.push(mine);
            continue;
        };
        let items: Vec<Option<(Value, usize)>> = match &o.each {
            None => vec![None],
            Some(f) => match read(row, None, f) {
                Ok(Some(Value::Array(a))) => a
                    .into_iter()
                    .take(EACH_MAX)
                    .enumerate()
                    .map(|(n, v)| Some((v, n + 1)))
                    .collect(),
                Ok(Some(v)) => vec![Some((v, 1))],
                Ok(None) => Vec::new(),
                Err(e) => {
                    issues.push(issue(e));
                    Vec::new()
                }
            },
        };
        for item in items {
            let elem = item.as_ref().map(|(v, n)| (v, *n));
            match plan_object(i, o, def, row, elem, &mut warnings, &mut skipped[i]) {
                Ok(Some(mut p)) => {
                    p.needs = needs.clone();
                    mine.push(plan.objects.len());
                    plan.objects.push(p);
                    elems.push(item);
                }
                Ok(None) => {}
                Err(e) => issues.push(issue(e)),
            }
        }
        produced.push(mine);
    }
    for (i, l) in m.links.iter().enumerate() {
        let idx = |alias: &str| m.objects.iter().position(|o| o.alias == alias);
        let (Some(fi), Some(ti)) = (idx(&l.from), idx(&l.to)) else {
            continue;
        };
        let Some(lt) = s.links.iter().find(|d| d.key == l.link_type) else {
            continue;
        };
        let froms = &produced[fi];
        let tos = &produced[ti];
        if skipped[fi] || skipped[ti] {
            // One end left out by its key: the links the other end already
            // has stay as they are.
            if !(skipped[fi] && skipped[ti]) {
                let (known, is_source) = if skipped[fi] {
                    (tos, false)
                } else {
                    (froms, true)
                };
                for &end in known {
                    plan.held.push(HeldEnd {
                        link_type: l.link_type.clone(),
                        end,
                        is_source,
                    });
                }
            }
            continue;
        }
        // One side may be an `each`: one link per element.
        let pairs: Vec<(usize, usize)> = if fi == ti {
            Vec::new()
        } else if froms.len() <= 1 {
            froms
                .iter()
                .flat_map(|f| tos.iter().map(move |t| (*f, *t)))
                .collect()
        } else {
            tos.iter()
                .flat_map(|t| froms.iter().map(move |f| (*f, *t)))
                .collect()
        };
        // Its paths are then read from the element, else once per record.
        let side = match (m.objects[fi].each.is_some(), m.objects[ti].each.is_some()) {
            (true, _) => Some(true),
            (_, true) => Some(false),
            _ => None,
        };
        let shared = match side {
            None => match link_values(l, lt, row, None) {
                Ok(v) => Some(v),
                Err(mut e) => {
                    issues.extend(e.drain(..).map(issue));
                    continue;
                }
            },
            Some(_) => None,
        };
        for (from, to) in pairs {
            let (attributes, valid_from) = match (&shared, side) {
                (Some(v), _) => v.clone(),
                (None, Some(on_from)) => {
                    let at = if on_from { from } else { to };
                    let elem = elems[at].as_ref().map(|(v, n)| (v, *n));
                    match link_values(l, lt, row, elem) {
                        Ok(v) => v,
                        Err(mut e) => {
                            issues.extend(e.drain(..).map(issue));
                            continue;
                        }
                    }
                }
                (None, None) => continue,
            };
            plan.links.push(PlannedLink {
                entry: i,
                link_type: l.link_type.clone(),
                from,
                to,
                valid_from,
                attributes,
            });
        }
    }
    if issues.is_empty() {
        plan.warnings = warnings.into_iter().map(issue).collect();
        Ok(plan)
    } else {
        Err(issues)
    }
}

/// Attributes and start of one link, read from the record (or the `each`
/// element), or the (column, token) issues.
#[allow(clippy::type_complexity)]
fn link_values(
    l: &LinkMap,
    lt: &LinkTypeDef,
    row: &Value,
    elem: Elem,
) -> Result<(Map<String, Value>, Option<String>), Vec<(String, String)>> {
    let mut attrs = Map::new();
    let mut issues = Vec::new();
    for (k, f) in &l.attributes {
        match read(row, elem, f) {
            Ok(Some(v)) => {
                let def = lt.attributes.iter().find(|a| &a.key == k);
                match def.map(|d| cast(d, v)) {
                    Some(Ok(v)) => {
                        attrs.insert(k.clone(), v);
                    }
                    _ => issues.push((
                        f.from.clone().unwrap_or_else(|| k.clone()),
                        FIELD_INVALID.into(),
                    )),
                }
            }
            Ok(None) => {}
            Err(e) => issues.push(e),
        }
    }
    match check_values(&lt.attributes, &attrs) {
        Ok(a) => attrs = a,
        Err((k, token)) => {
            let column = l
                .attributes
                .get(&k)
                .and_then(|f| f.from.clone())
                .unwrap_or(k);
            issues.push((column, token));
        }
    }
    let valid_from = match &l.valid_from {
        None => None,
        Some(f) => match read(row, elem, f) {
            Ok(v) => v.and_then(|v| text_of(&v)).map(|d| match d.len() {
                10 => format!("{d}T00:00:00Z"),
                _ => d,
            }),
            Err(e) => {
                issues.push(e);
                None
            }
        },
    };
    if issues.is_empty() {
        Ok((attrs, valid_from))
    } else {
        Err(issues)
    }
}

/// One object of an entry; `Ok(None)` when its key is empty (nothing to
/// upsert: an optional site column left blank).
/// `warnings` gets the values left out under `invalid: skip`; `skipped` is
/// set when the key itself was.
fn plan_object(
    entry: usize,
    o: &ObjectMap,
    def: &ObjectTypeDef,
    row: &Value,
    elem: Elem,
    warnings: &mut Vec<(String, String)>,
    skipped: &mut bool,
) -> Result<Option<PlannedObject>, (String, String)> {
    if let Some(w) = &o.when {
        let v = at_path(row, elem, &w.from).as_ref().and_then(text_of);
        if (v.as_deref().map(str::trim) == Some(w.equals.as_str())) == w.not {
            return Ok(None);
        }
    }
    let mut props = Map::new();
    for (k, f) in &o.properties {
        let Some(d) = def.properties.iter().find(|p| &p.key == k) else {
            continue;
        };
        let col = f.from.clone().unwrap_or_else(|| k.clone());
        let v = read(row, elem, f)
            .and_then(|v| v.map(|v| cast(d, v).map_err(|t| (col, t))).transpose());
        match v {
            Ok(Some(v)) => {
                props.insert(k.clone(), v);
            }
            Ok(None) => {}
            Err(e) if f.invalid == Missing::Skip => {
                warnings.push(e);
                if *k == o.key {
                    *skipped = true;
                    return Ok(None);
                }
            }
            Err(e) => return Err(e),
        }
    }
    let mut defaults = Map::new();
    for (k, v) in &o.defaults {
        if let Some(d) = def.properties.iter().find(|p| &p.key == k) {
            defaults.insert(k.clone(), cast(d, v.clone()).map_err(|t| (k.clone(), t))?);
        }
    }
    // `required` is checked on creation only, against the merged document
    // (an update may map a few properties).
    let loose: Vec<PropertyDef> = def
        .properties
        .iter()
        .map(|p| PropertyDef {
            required: false,
            ..p.clone()
        })
        .collect();
    let props = loop {
        let (k, t) = match check_values(&loose, &props) {
            Ok(p) => break p,
            Err(e) => e,
        };
        let f = o.properties.get(&k);
        let col = f.and_then(|f| f.from.clone()).unwrap_or_else(|| k.clone());
        // Left out under `invalid: skip`, then the rest is checked again.
        if !f.is_some_and(|f| f.invalid == Missing::Skip) || props.remove(&k).is_none() {
            return Err((col, t));
        }
        warnings.push((col, t));
        if k == o.key {
            *skipped = true;
            return Ok(None);
        }
    };
    let Some(key) = props.get(&o.key).and_then(text_of) else {
        return Ok(None);
    };
    let title = match &o.title {
        Some(f) => read(row, elem, f)?.and_then(|v| text_of(&v)),
        None => None,
    }
    .map(|t| {
        // Trimmed again after the cut: the stored title is trimmed, so a
        // space at the cut would show as a change on every re-import.
        let cut: String = t.trim().chars().take(super::api::TITLE_MAX).collect();
        cut.trim_end().to_string()
    })
    .filter(|t| !t.is_empty());
    let titled = title.is_some();
    let title = title.unwrap_or_else(|| key.clone());
    let location = match &o.location {
        None => None,
        Some(l) => {
            let num = |f: &FieldSpec| -> Result<Option<f64>, (String, String)> {
                let col = f.from.clone().unwrap_or_default();
                match read(row, elem, f)? {
                    None => Ok(None),
                    Some(Value::Number(n)) => Ok(n.as_f64()),
                    Some(v) => text_of(&v)
                        .and_then(|s| number_of(&s))
                        .map(Some)
                        .ok_or((col, FIELD_INVALID.into())),
                }
            };
            match (num(&l.lat)?, num(&l.lon)?) {
                (Some(lat), Some(lon))
                    if (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon) =>
                {
                    Some((lat, lon))
                }
                (None, _) | (_, None) => None,
                _ => return Err((l.lat.from.clone().unwrap_or_default(), FIELD_INVALID.into())),
            }
        }
    };
    Ok(Some(PlannedObject {
        entry,
        type_key: o.type_key.clone(),
        key_prop: o.key.clone(),
        key,
        title,
        titled,
        properties: props,
        defaults,
        location,
        create: o.create,
        update: o.update.unwrap_or(o.create),
        missing: o.missing,
        needs: Vec::new(),
    }))
}

/// Columns the mapping reads that no record of the run holds (D303: an
/// unexpected schema refuses a scheduled run).
pub fn missing_columns(m: &Mapping, rows: &[Value]) -> Vec<String> {
    m.columns()
        .into_iter()
        .filter(|c| !rows.iter().any(|r| get_path(r, c).is_some()))
        .collect()
}

/// D303: a snapshot run that lost more than this share of the rows of the
/// previous one is refused.
pub const SUSPICIOUS_DROP: f64 = 0.30;

pub fn suspicious_drop(previous: usize, now: usize) -> bool {
    previous > 0 && (now as f64) < (previous as f64) * (1.0 - SUSPICIOUS_DROP)
}

// ───────────────────────────── wire (API) ─────────────────────────────

/// Where an import reads its records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ImportSource {
    /// A file sent with each preview.
    #[default]
    File,
    /// A file fetched from an `http(s)` URL (csv, xml, json, maybe zipped).
    Url,
    /// A paginated JSON API (`page_param` or `cursor_path`).
    Api,
}

/// `incremental`: creates and updates. `snapshot`: also closes what this
/// import wrote before and the run no longer holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ImportMode {
    #[default]
    Incremental,
    Snapshot,
}

impl ImportMode {
    pub fn wire(self) -> &'static str {
        match self {
            Self::Incremental => "incremental",
            Self::Snapshot => "snapshot",
        }
    }
}

impl ImportSource {
    pub fn wire(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Url => "url",
            Self::Api => "api",
        }
    }
}

/// Pages fetched from an API by one run, at most.
pub const PAGES_MAX: u32 = 500;

/// Create or update an import (`expected_version` on update, else 409).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ObjectImportInput {
    pub name: String,
    /// The mapping, YAML.
    pub mapping: String,
    #[serde(default)]
    pub source: ImportSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(default)]
    pub mode: ImportMode,
    /// A `recurrence` value (URL and API sources only): scheduled runs
    /// apply without preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<Object>))]
    pub recurrence: Option<Value>,
    /// API pages per run (default 50, at most `PAGES_MAX`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_pages: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i32>,
}

/// An import shipped by a pack: the mapping inline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct PackImport {
    pub name: String,
    #[serde(default)]
    pub source: ImportSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(default)]
    pub mode: ImportMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<Object>))]
    pub recurrence: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_pages: Option<u32>,
    pub mapping: Mapping,
}

/// Counters of a run's difference.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ImportCounts {
    pub created: usize,
    pub updated: usize,
    pub closed: usize,
    pub links_opened: usize,
    pub links_updated: usize,
    pub errors: usize,
    pub warnings: usize,
}

/// One run: a preview waiting for its application, or a finished run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ImportRunView {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    /// Sequence per import: provenance `import:<import id>@<number>`.
    pub number: i32,
    /// `running` | `preview` | `applied` | `refused` | `failed` | `stale`.
    pub state: String,
    /// Machine code of a refusal or failure (`import-schema-changed`…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_code: Option<String>,
    /// Its arguments (`{rows, max}` for a source too large, the missing
    /// columns…), for the `err-<code>` message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<Object>))]
    pub state_args: Option<Value>,
    /// `preview` | `schedule` | `manual`.
    pub trigger: String,
    /// Import version the run was computed on.
    pub import_version: i32,
    pub rows: usize,
    pub counts: ImportCounts,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ObjectImportView {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub name: String,
    /// The mapping, YAML.
    pub mapping: String,
    /// The pack that shipped the mapping, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack_key: Option<String>,
    pub source: ImportSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    pub mode: ImportMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<Object>))]
    pub recurrence: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub next_run_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_pages: Option<u32>,
    pub version: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<ImportRunView>,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub updated_at: String,
}

/// A property change `from` → `to`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ValueChange {
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub from: Value,
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub to: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DiffCreate {
    pub type_key: String,
    pub key_prop: String,
    pub key: String,
    pub title: String,
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub properties: Map<String, Value>,
    /// `[lat, lon]` of a created `map_pin`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<[f64; 2]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DiffUpdate {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub type_key: String,
    pub key: String,
    pub title: String,
    /// Object version read by the preview: the application writes on it.
    pub version: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_title: Option<String>,
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub changes: BTreeMap<String, ValueChange>,
}

/// An object archived or a link closed by a snapshot run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DiffClose {
    /// `object` | `link`.
    pub kind: String,
    /// Object UUID or link id.
    pub id: String,
    pub type_key: String,
    pub label: String,
}

/// An end of a link: an existing object, or one created by the run
/// (index in `created`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DiffEnd {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<usize>,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DiffLink {
    pub link_type: String,
    pub from: DiffEnd,
    pub to: DiffEnd,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub valid_from: Option<String>,
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub attributes: Map<String, Value>,
    /// Written as a proposal, not as a link (D375).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub propose: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DiffLinkUpdate {
    pub id: i64,
    pub link_type: String,
    pub label: String,
    /// The link's attributes once updated.
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub attributes: Map<String, Value>,
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub changes: BTreeMap<String, ValueChange>,
}

/// What a run writes (D301): computed by the preview, applied as is.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ImportDiff {
    pub rows: usize,
    pub created: Vec<DiffCreate>,
    pub updated: Vec<DiffUpdate>,
    pub closed: Vec<DiffClose>,
    pub links_opened: Vec<DiffLink>,
    pub links_updated: Vec<DiffLinkUpdate>,
    pub errors: Vec<RowIssue>,
    pub warnings: Vec<RowIssue>,
}

impl ImportDiff {
    pub fn counts(&self) -> ImportCounts {
        ImportCounts {
            created: self.created.len(),
            updated: self.updated.len(),
            closed: self.closed.len(),
            links_opened: self.links_opened.len(),
            links_updated: self.links_updated.len(),
            errors: self.errors.len(),
            warnings: self.warnings.len(),
        }
    }

    /// The same difference with each list cut to `n` entries (display).
    pub fn sample(&self, n: usize) -> Self {
        fn cut<T: Clone>(v: &[T], n: usize) -> Vec<T> {
            v.iter().take(n).cloned().collect()
        }
        Self {
            rows: self.rows,
            created: cut(&self.created, n),
            updated: cut(&self.updated, n),
            closed: cut(&self.closed, n),
            links_opened: cut(&self.links_opened, n),
            links_updated: cut(&self.links_updated, n),
            errors: cut(&self.errors, n),
            warnings: cut(&self.warnings, n),
        }
    }
}

/// `POST …/dry-run` answer: the run to apply and its difference (each
/// list cut to 500 entries; `run.counts` has the totals).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ImportPreview {
    pub run: ImportRunView,
    pub diff: ImportDiff,
    /// The run's steps, in order (an import with `steps:`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<crate::ingest::IngestStepView>,
}

/// `POST …/apply`: the preview run, on the import version it was read on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ImportApply {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub run_id: String,
    pub expected_version: i32,
}

#[cfg(test)]
mod tests {
    #[test]
    fn json_text_is_decoded_then_read() {
        use super::{convert, Convert};
        let v = Some(serde_json::json!(
            "{\"descriptif\": \"modification du capital\"}"
        ));
        let c = [Convert::Json, Convert::Field("descriptif".into())];
        assert_eq!(
            convert(v, &c),
            Ok(Some(serde_json::json!("modification du capital")))
        );
        assert_eq!(
            convert(Some(serde_json::json!("pas du json")), &c),
            Ok(None)
        );
    }

    #[test]
    fn last_segment_takes_the_latest_key_or_element() {
        let v =
            serde_json::json!({"finances": {"2024": {"ca": 1}, "2025": {"ca": 2}}, "a": [1, 2, 3]});
        assert_eq!(
            super::get_path(&v, "finances.@last.ca"),
            Some(&serde_json::json!(2))
        );
        assert_eq!(super::get_path(&v, "a.@last"), Some(&serde_json::json!(3)));
        assert_eq!(
            super::get_path(&serde_json::json!({"finances": {}}), "finances.@last.ca"),
            None
        );
    }

    use super::*;
    use serde_json::json;

    fn types() -> Vec<ObjectTypeDef> {
        serde_json::from_value(json!([
            {"key": "organization", "name": "Org", "properties": [
                {"key": "siren", "kind": "text", "max_len": 9, "unique": true},
                {"key": "nda", "kind": "text"},
                {"key": "qualiopi", "kind": "bool"}
            ]},
            {"key": "certification", "name": "Cert", "properties": [
                {"key": "code", "kind": "text", "required": true, "unique": true},
                {"key": "intitule", "kind": "text"}
            ]},
            {"key": "site_web", "name": "Site", "properties": [
                {"key": "url_racine", "kind": "url", "required": true},
                {"key": "domaine", "kind": "text", "unique": true}
            ]},
            {"key": "offre", "name": "Offre", "properties": [
                {"key": "id", "kind": "text", "unique": true},
                {"key": "prix", "kind": "money"},
                {"key": "heures", "kind": "number"}
            ]}
        ]))
        .unwrap()
    }

    fn links() -> Vec<LinkTypeDef> {
        serde_json::from_value(json!([
            {"key": "habilite_pour", "name": "h", "from_types": ["organization"], "to_types": ["certification"],
             "attributes": [{"key": "perimetre", "kind": "enum", "values": ["preparation", "evaluation", "les_deux"]},
                            {"key": "declaree_fc", "kind": "bool"}]},
            {"key": "a_pour_site", "name": "s", "from_types": ["organization"], "to_types": ["site_web"]}
        ]))
        .unwrap()
    }

    fn mapping() -> Mapping {
        let yaml = json!({
            "key": "reseau", "name": "Réseau",
            "objects": [
                {"as": "org", "type": "organization", "key": "siren",
                 "title": "raison_sociale",
                 "properties": {"siren": {"from": "siret", "convert": ["digits", "left:9"]}, "nda": "nda"}},
                {"as": "cert", "type": "certification", "key": "code", "create": false,
                 "properties": {"code": {"from": "code_certification", "convert": ["trim", "upper"]}}},
                {"as": "site", "type": "site_web", "key": "domaine",
                 "each": {"from": "site_web", "convert": ["split:;"]},
                 "properties": {"url_racine": {"from": ".", "convert": ["url_origin"]},
                                "domaine": {"from": ".", "convert": ["domain"]}}}
            ],
            "links": [
                {"type": "habilite_pour", "from": "org", "to": "cert",
                 "valid_from": {"from": "date_debut", "convert": ["date:%d/%m/%Y"]},
                 "attributes": {"perimetre": "perimetre",
                                "declaree_fc": {"from": "declaree", "convert": [{"enum": {"oui": "true", "non": "false"}}]}}},
                {"type": "a_pour_site", "from": "org", "to": "site"}
            ]
        });
        from_value(yaml).unwrap()
    }

    #[test]
    fn mapping_is_checked() {
        let (t, l) = (types(), links());
        let s = Schema {
            types: &t,
            links: &l,
        };
        assert_eq!(check(&mapping(), &s), Ok(()));
        let mut m = mapping();
        m.objects[0].key = "nda".into();
        assert_eq!(check(&m, &s).unwrap_err().0, "objects.0.key");
        let mut m = mapping();
        m.links[0].to = "site".into();
        assert_eq!(check(&m, &s).unwrap_err().0, "links.0.type");
        let mut m = mapping();
        m.objects[0]
            .properties
            .insert("nope".into(), serde_json::from_value(json!("x")).unwrap());
        assert_eq!(check(&m, &s).unwrap_err().0, "objects.0.properties.nope");
        // A merge value names a declared attribute and fits its kind.
        let mut m = mapping();
        m.links[0]
            .on_conflict
            .insert("perimetre".into(), json!("les_deux"));
        assert_eq!(check(&m, &s), Ok(()));
        m.links[0]
            .on_conflict
            .insert("perimetre".into(), json!("tout"));
        assert_eq!(
            check(&m, &s).unwrap_err().0,
            "links.0.on_conflict.perimetre"
        );
        let mut m = mapping();
        m.links[0].on_conflict.insert("nope".into(), json!("x"));
        assert_eq!(check(&m, &s).unwrap_err().0, "links.0.on_conflict.nope");
        assert!(
            from_value(json!({"key": "a", "name": "a", "objects": [], "script": "x"})).is_err()
        );
        assert!(serde_json::from_value::<Convert>(json!("eval:1")).is_err());
    }

    #[test]
    fn a_row_plans_objects_and_links() {
        let (t, l) = (types(), links());
        let s = Schema {
            types: &t,
            links: &l,
        };
        let row = json!({
            "siret": "123 456 789 00012", "raison_sociale": " ACME ", "nda": "",
            "code_certification": " rncp12345", "site_web": "www.Acme.fr/formations; https://b.acme.fr",
            "date_debut": "01/02/2024", "perimetre": "les_deux", "declaree": "oui"
        });
        let p = plan_row(&mapping(), &s, 2, &row).unwrap();
        assert_eq!(p.objects.len(), 4);
        assert_eq!(p.objects[0].key, "123456789");
        assert_eq!(p.objects[0].title, "ACME");
        let long =
            json!({"siret": "123456789", "raison_sociale": format!("{} x", "a".repeat(254))});
        let t = &plan_row(&mapping(), &s, 3, &long).unwrap().objects[0].title;
        assert_eq!(t.chars().count(), 254, "a space at the cut is trimmed");
        assert!(!p.objects[0].properties.contains_key("nda"));
        assert_eq!(p.objects[1].key, "RNCP12345");
        assert!(!p.objects[1].create);
        assert_eq!(
            p.objects[2].properties["url_racine"],
            json!("https://www.acme.fr")
        );
        assert_eq!(p.objects[2].key, "acme.fr");
        assert_eq!(p.objects[3].key, "b.acme.fr");
        assert_eq!(p.links.len(), 3);
        assert_eq!(
            p.links[0].valid_from.as_deref(),
            Some("2024-02-01T00:00:00Z")
        );
        assert_eq!(p.links[0].attributes["declaree_fc"], json!(true));
        assert_eq!((p.links[2].from, p.links[2].to), (0, 3));
    }

    #[test]
    fn bad_cells_are_row_issues() {
        let (t, l) = (types(), links());
        let s = Schema {
            types: &t,
            links: &l,
        };
        let row = json!({"siret": "123456789", "code_certification": "RS1",
                         "date_debut": "2024-13-45", "perimetre": "tout", "declaree": "peut-être"});
        let issues = plan_row(&mapping(), &s, 7, &row).unwrap_err();
        let cols: Vec<&str> = issues.iter().map(|i| i.column.as_str()).collect();
        assert!(cols.contains(&"declaree"), "{issues:?}");
        assert!(cols.contains(&"date_debut"), "{issues:?}");
        assert!(issues.iter().all(|i| i.row == 7));
    }

    #[test]
    fn numbers_and_money_read_the_french_way() {
        let (t, l) = (types(), links());
        let s = Schema {
            types: &t,
            links: &l,
        };
        let m = from_value(json!({"key": "o", "name": "o", "objects": [
            {"as": "o", "type": "offre", "key": "id", "properties": {
                "id": "id", "prix": {"from": "prix", "convert": ["money:EUR"]}, "heures": "h"}}]}))
        .unwrap();
        let p = plan_row(&m, &s, 1, &json!({"id": 42, "prix": "1 250,50", "h": "35"})).unwrap();
        assert_eq!(p.objects[0].key, "42");
        assert_eq!(
            p.objects[0].properties["prix"],
            json!({"amount": 1250.5, "currency": "EUR"})
        );
        assert_eq!(p.objects[0].properties["heures"], json!(35.0));
    }

    #[test]
    fn concat_then_before_is_a_coalesce() {
        let f: FieldSpec = serde_json::from_value(json!({
            "concat": ["Date_Effet", "Date_Decision"], "sep": "|",
            "convert": ["before:|", "date:%d/%m/%Y"]
        }))
        .unwrap();
        let both = json!({"Date_Effet": "01/02/2024", "Date_Decision": "15/01/2024"});
        let rs = json!({"Date_Effet": "", "Date_Decision": "15/01/2024"});
        assert_eq!(read(&both, None, &f).unwrap(), Some(json!("2024-02-01")));
        assert_eq!(read(&rs, None, &f).unwrap(), Some(json!("2024-01-15")));
        assert_eq!(read(&json!({}), None, &f).unwrap(), None);
        // The repertoire of a sheet number: two letters, then an enum.
        let r: FieldSpec = serde_json::from_value(json!({
            "from": "n", "convert": ["trim", "upper", "left:2", {"enum": {"RN": "rncp", "RS": "rs"}}]
        }))
        .unwrap();
        assert_eq!(
            read(&json!({"n": "rs6765"}), None, &r).unwrap(),
            Some(json!("rs"))
        );
        assert_eq!(
            read(&json!({"n": "RNCP1"}), None, &r).unwrap(),
            Some(json!("rncp"))
        );
    }

    #[test]
    fn csv_parses_quotes_and_guesses_the_separator() {
        let t = parse_csv(
            "\u{feff}a;b;c\r\n1;\"x;y\";\"he said \"\"hi\"\"\"\n\n2;\"multi\nline\";\n",
            None,
        );
        assert_eq!(t[0], vec!["a", "b", "c"]);
        assert_eq!(t[1], vec!["1", "x;y", "he said \"hi\""]);
        assert_eq!(t[2], vec!["2", "multi\nline", ""]);
        let rows = table_rows(t);
        assert_eq!(rows[1]["b"], json!("multi\nline"));
        assert_eq!(parse_csv("a,b\n1,2", None)[1], vec!["1", "2"]);
    }

    #[test]
    fn paths_reach_nested_records() {
        let doc = json!({"results": [{"siege": {"siret": "1"}, "dirigeants": [{"nom": "A"}]}]});
        let rows = json_rows(&doc, "results").unwrap();
        assert_eq!(get_path(&rows[0], "siege.siret"), Some(&json!("1")));
        assert_eq!(get_path(&rows[0], "dirigeants.0.nom"), Some(&json!("A")));
        assert_eq!(get_path(&json!({"a.b": 1}), "a.b"), Some(&json!(1)));
        let m = mapping();
        assert_eq!(missing_columns(&m, &[json!({"siret": "1"})]).len(), 7);
    }

    #[test]
    fn checks_and_joins() {
        assert!(luhn("732829320") && luhn("73282932000074") && !luhn("732829321"));
        assert!(valid_email("a.b@ex.fr") && !valid_email("a@b") && !valid_email("a@b.fr;c@d.fr"));
        let row =
            json!({"nom": "MARTIN", "prenoms": "Jeanne", "type": "pp", "siret": "73282932000074"});
        let f: FieldSpec =
            serde_json::from_value(json!({"concat": ["nom", "prenoms", "x"], "sep": "|"})).unwrap();
        assert_eq!(read(&row, None, &f).unwrap(), Some(json!("MARTIN|Jeanne")));
        let f: FieldSpec =
            serde_json::from_value(json!({"from": "siret", "convert": ["luhn"]})).unwrap();
        assert!(read(&row, None, &f).is_ok());
        let f: FieldSpec = serde_json::from_value(
            json!({"from": "nom", "convert": [{"enum": {"X": "x", "*": "autre"}}]}),
        )
        .unwrap();
        assert_eq!(read(&row, None, &f).unwrap(), Some(json!("autre")));
        let (t, l) = (types(), links());
        let s = Schema {
            types: &t,
            links: &l,
        };
        let m = from_value(json!({"key": "o", "name": "o", "objects": [
            {"as": "o", "type": "organization", "key": "siren", "when": {"from": "type", "equals": "pm"},
             "properties": {"siren": {"from": "siret", "convert": ["left:9"]}}}]}))
        .unwrap();
        assert!(plan_row(&m, &s, 1, &row).unwrap().objects.is_empty());
    }

    #[test]
    fn invalid_skip_leaves_values_out() {
        let (t, l) = (types(), links());
        let s = Schema {
            types: &t,
            links: &l,
        };
        let mapping = |invalid: &str| {
            from_value(json!({"key": "p", "name": "p", "objects": [
                {"as": "org", "type": "organization", "key": "siren", "properties": {
                    "siren": {"from": "siret", "convert": ["digits", "left:9", "luhn"], "invalid": invalid},
                    "qualiopi": {"from": "q", "invalid": invalid}}},
                {"as": "cert", "type": "certification", "key": "code", "properties": {"code": "code"}}],
                "links": [{"type": "habilite_pour", "from": "org", "to": "cert"}]}))
            .unwrap()
        };
        // The key fails: the object and its link are left out, the other end
        // keeps what it has.
        let row = json!({"siret": "30540504800785", "code": "RS1", "q": "oui"});
        let p = plan_row(&mapping("skip"), &s, 4, &row).unwrap();
        assert_eq!(p.objects.len(), 1);
        assert!(p.links.is_empty());
        assert_eq!(
            (
                p.warnings.len(),
                p.warnings[0].row,
                p.warnings[0].column.as_str()
            ),
            (1, 4, "siret")
        );
        assert_eq!(
            p.held,
            [HeldEnd {
                link_type: "habilite_pour".into(),
                end: 0,
                is_source: false
            }]
        );
        assert!(plan_row(&mapping("error"), &s, 4, &row).is_err());
        // Another property fails: only it is left out.
        let row = json!({"siret": "73282932000074", "code": "RS1", "q": "peut-être"});
        let p = plan_row(&mapping("skip"), &s, 5, &row).unwrap();
        assert_eq!((p.objects.len(), p.links.len()), (2, 1));
        assert!(!p.objects[0].properties.contains_key("qualiopi"));
        assert_eq!(p.warnings[0].column, "q");
        assert!(p.held.is_empty());
        assert!(plan_row(&mapping("error"), &s, 5, &row).is_err());
    }

    #[test]
    fn drops_are_flagged() {
        assert!(!suspicious_drop(0, 0));
        assert!(!suspicious_drop(100, 75));
        assert!(suspicious_drop(100, 69));
    }

    #[test]
    fn paragraphs_after_and_latest() {
        let v = convert(
            Some(json!(
                "Cadrer   Auditer le site.  Deux espaces\n\n\nOptimiser"
            )),
            &[Convert::SplitSpaces(3)],
        )
        .unwrap();
        assert_eq!(
            v,
            Some(json!([
                "Cadrer",
                "Auditer le site.  Deux espaces",
                "Optimiser"
            ]))
        );
        assert_eq!(Convert::parse("split_spaces:1"), None);
        let v = convert(
            Some(json!("RNCP183BC02")),
            &[Convert::parse("after:BC").unwrap()],
        )
        .unwrap();
        assert_eq!(v, Some(json!("02")));
        // The latest year, in any order; an XML lone child is one element.
        let latest = [
            Convert::parse("max_by:ANNEE").unwrap(),
            Convert::parse("field:N").unwrap(),
        ];
        let many = json!([{"ANNEE": "2023", "N": "4"}, {"ANNEE": "2024", "N": "64"}, {"ANNEE": "2022", "N": "1"}]);
        assert_eq!(convert(Some(many), &latest).unwrap(), Some(json!("64")));
        assert_eq!(
            convert(Some(json!({"ANNEE": "2024", "N": "12"})), &latest).unwrap(),
            Some(json!("12"))
        );
        assert_eq!(
            convert(Some(json!({"ANNEE": "2024"})), &latest).unwrap(),
            None
        );
    }

    #[test]
    fn each_positions_element_links_and_needs() {
        let t: Vec<ObjectTypeDef> = serde_json::from_value(json!([
            {"key": "certification", "name": "C", "properties": [{"key": "code", "kind": "text", "unique": true}]},
            {"key": "competence", "name": "K", "properties": [
                {"key": "cle", "kind": "text", "unique": true}, {"key": "rang", "kind": "number"},
                {"key": "libelle", "kind": "text"}]},
            {"key": "organization", "name": "O", "properties": [{"key": "siren", "kind": "text", "unique": true}]}
        ]))
        .unwrap();
        let l: Vec<LinkTypeDef> = serde_json::from_value(json!([
            {"key": "competence_de", "name": "d", "from_types": ["competence"], "to_types": ["certification"]},
            {"key": "habilite_pour", "name": "h", "from_types": ["organization"], "to_types": ["certification"],
             "attributes": [{"key": "etat_fc", "kind": "enum", "values": ["actif", "inactif"]}]}
        ]))
        .unwrap();
        let s = Schema {
            types: &t,
            links: &l,
        };
        let m = from_value(json!({
            "key": "rs", "name": "RS", "format": "xml", "rows": "FICHES/FICHE",
            "objects": [
                {"as": "cert", "type": "certification", "key": "code", "create": false, "missing": "skip",
                 "properties": {"code": "NUMERO_FICHE"}},
                {"as": "comp", "type": "competence", "key": "cle", "needs": "cert",
                 "each": {"from": "CAPACITES_ATTESTEES", "convert": ["split_spaces:3"]},
                 "properties": {"cle": {"concat": ["^NUMERO_FICHE", "#"], "sep": "-C"},
                                "rang": "#", "libelle": "."}},
                {"as": "org", "type": "organization", "key": "siren", "create": false, "missing": "skip",
                 "each": "PARTENAIRES.PARTENAIRE",
                 "properties": {"siren": {"from": "SIRET_PARTENAIRE", "convert": ["left:9"]}}}
            ],
            "links": [
                {"type": "competence_de", "from": "comp", "to": "cert"},
                {"type": "habilite_pour", "from": "org", "to": "cert", "create": false,
                 "attributes": {"etat_fc": {"from": "ETAT_HABILITATION", "convert": [{"enum": {"Actif": "actif", "Inactif": "inactif"}}]}}}
            ]
        }))
        .unwrap();
        assert_eq!(check(&m, &s), Ok(()));
        // Element-relative link paths are not record columns (D303).
        assert_eq!(
            m.columns(),
            [
                "NUMERO_FICHE",
                "CAPACITES_ATTESTEES",
                "PARTENAIRES.PARTENAIRE"
            ]
        );
        let row = json!({
            "NUMERO_FICHE": "RS7511",
            "CAPACITES_ATTESTEES": "Cadrer le projet    Auditer les intentions",
            "PARTENAIRES": {"PARTENAIRE": [
                {"SIRET_PARTENAIRE": "91338332900012", "ETAT_HABILITATION": "Actif"},
                {"SIRET_PARTENAIRE": "53098720500016", "ETAT_HABILITATION": "Inactif"}
            ]}
        });
        let p = plan_row(&m, &s, 1, &row).unwrap();
        let comps: Vec<_> = p
            .objects
            .iter()
            .filter(|o| o.type_key == "competence")
            .collect();
        assert_eq!(comps.len(), 2);
        assert_eq!(
            (comps[1].key.as_str(), &comps[1].properties["rang"]),
            ("RS7511-C2", &json!(2))
        );
        assert_eq!(comps[0].needs, [0]);
        let etats: Vec<_> = p
            .links
            .iter()
            .filter(|l| l.link_type == "habilite_pour")
            .map(|l| l.attributes["etat_fc"].clone())
            .collect();
        assert_eq!(etats, [json!("actif"), json!("inactif")]);
        // A `needs` naming a later entry is refused.
        let mut bad = m.clone();
        bad.objects[1].needs = Some("org".into());
        assert_eq!(check(&bad, &s).unwrap_err().0, "objects.1.needs");
    }
}
