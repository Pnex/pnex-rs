//! Ontology meta-model (D176–D191, lot L0): object types and link types
//! are **data**, not code.
//!
//! The system types are declared here once; the backend registry derives its
//! containment and relation rules from them (the hand-written `KindSpec` is
//! gone). Org types (lot L1) use the same serde format, loaded from the
//! database and exported as YAML (D186, D190). serde-only, wasm32-safe.

pub mod action;
pub mod api;
pub mod graph;
pub mod layout;
pub mod schema;

use serde::{Deserialize, Serialize};

pub use schema::{PropertyDef, PropertyKind};

use crate::err_codes::{FIELD_INVALID, FIELD_MAX_LENGTH, FIELD_REQUIRED};
use crate::resources::{
    KIND_DASHBOARD, KIND_DEVICE, KIND_DOC, KIND_FLOW, KIND_FOLDER, KIND_MAP_PIN, KIND_MEDIA_ASSET,
    KIND_TOUR, REL_PLACED_ON,
};

/// A set of object type keys: `"*"` (every type, including types created
/// later) or a list of keys, on the wire and in YAML.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeSet {
    Any,
    Only(Vec<String>),
}

impl Serialize for TypeSet {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Any => s.serialize_str("*"),
            Self::Only(keys) => keys.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for TypeSet {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Star(String),
            Keys(Vec<String>),
        }
        match Repr::deserialize(d)? {
            Repr::Star(s) if s == "*" => Ok(Self::Any),
            Repr::Star(s) => Err(serde::de::Error::invalid_value(
                serde::de::Unexpected::Str(&s),
                &"\"*\" or a list",
            )),
            Repr::Keys(keys) => Ok(Self::Only(keys)),
        }
    }
}

impl TypeSet {
    pub fn only(keys: &[&str]) -> Self {
        Self::Only(keys.iter().map(|k| k.to_string()).collect())
    }

    pub fn contains(&self, key: &str) -> bool {
        match self {
            Self::Any => true,
            Self::Only(keys) => keys.iter().any(|k| k == key),
        }
    }
}

/// Minimal org role allowed to write objects of a type (D188).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteRole {
    #[default]
    Member,
    Admin,
    Owner,
}

/// An object type (D176). Containment rules live on the type (D180).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObjectTypeDef {
    /// `[a-z][a-z0-9_]*`, unique per org; system keys are reserved.
    pub key: String,
    /// Display name; empty for system types (the UI translates their key).
    #[serde(default)]
    pub name: String,
    /// Icon name of the UI icon set.
    #[serde(default)]
    pub icon: String,
    /// System types are native tables exposed through an adapter (D177);
    /// they cannot be deleted.
    #[serde(default)]
    pub system: bool,
    #[serde(default)]
    pub properties: Vec<PropertyDef>,
    #[serde(default)]
    pub write_role: WriteRole,
    /// Types this one may contain; `None` = contains nothing.
    #[serde(default)]
    pub may_contain: Option<TypeSet>,
    /// Types this one may be contained in; `None` = never contained.
    #[serde(default)]
    pub may_be_contained_in: Option<TypeSet>,
    /// Operations invocable on the objects of this type (D183).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<ActionDef>,
}

/// An action declared on a type (D183): invoked from the object page,
/// executed by the deployed flows holding a matching `action_trigger` node,
/// journaled. Never executed by the server itself (D123).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionDef {
    /// `[a-z][a-z0-9_]*`, unique within the type.
    pub key: String,
    pub name: String,
    /// Parameters asked at invocation (scalar properties only).
    #[serde(default)]
    pub params: Vec<PropertyDef>,
    /// The UI asks for an explicit confirmation before invoking.
    #[serde(default)]
    pub confirm: bool,
}

/// A link type (D179): directed, from a set of types to a set of types.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkTypeDef {
    pub key: String,
    #[serde(default)]
    pub name: String,
    /// Name read from the target side (« is measured by »).
    #[serde(default)]
    pub inverse_name: String,
    #[serde(default)]
    pub system: bool,
    pub from_types: TypeSet,
    pub to_types: TypeSet,
    /// A source has at most one open link of this type.
    #[serde(default)]
    pub one_target: bool,
    /// A target has at most one open link of this type.
    #[serde(default)]
    pub one_source: bool,
    /// Attributes carried by each link (scalar properties only).
    #[serde(default)]
    pub attributes: Vec<PropertyDef>,
}

impl LinkTypeDef {
    pub fn allows(&self, key: &str, from: &str, to: &str) -> bool {
        self.key == key && self.from_types.contains(from) && self.to_types.contains(to)
    }
}

/// Is `key` taken by a system type or a system link type?
pub fn is_system_key(key: &str) -> bool {
    system_object_types().iter().any(|t| t.key == key)
        || system_link_types().iter().any(|l| l.key == key)
}

fn check_name(field: &str, name: &str) -> Result<(), schema::FieldError> {
    if name.trim().is_empty() {
        return Err((field.into(), FIELD_REQUIRED.into()));
    }
    if name.chars().count() > schema::NAME_MAX {
        return Err((
            field.into(),
            format!("{FIELD_MAX_LENGTH}:{}", schema::NAME_MAX),
        ));
    }
    Ok(())
}

fn check_set(
    field: &str,
    set: &Option<TypeSet>,
    known: &dyn Fn(&str) -> bool,
) -> Result<(), schema::FieldError> {
    match set {
        Some(TypeSet::Only(keys)) if keys.iter().any(|k| !known(k)) => {
            Err((field.into(), FIELD_INVALID.into()))
        }
        _ => Ok(()),
    }
}

impl ObjectTypeDef {
    /// A system type backed by its own table (D42 kinds): its objects are
    /// created and deleted by their pages. A system type without a native
    /// table (`case`) lives entirely in `objects`, written like org types.
    pub fn native(&self) -> bool {
        self.system && crate::resources::KINDS.contains(&self.key.as_str())
    }

    /// Checks an org type definition; `known` answers whether a type key
    /// exists (system, org, or this type itself).
    pub fn check(&self, known: &dyn Fn(&str) -> bool) -> Result<(), schema::FieldError> {
        if !schema::valid_key(&self.key) || is_system_key(&self.key) {
            return Err(("key".into(), FIELD_INVALID.into()));
        }
        check_name("name", &self.name)?;
        if self.icon.len() > schema::KEY_MAX {
            return Err(("icon".into(), FIELD_INVALID.into()));
        }
        if self.system {
            return Err(("system".into(), FIELD_INVALID.into()));
        }
        check_set("may_contain", &self.may_contain, known)?;
        check_set("may_be_contained_in", &self.may_be_contained_in, known)?;
        schema::check_defs(&self.properties, "properties", known)?;
        for (i, a) in self.actions.iter().enumerate() {
            let field = format!("actions.{i}");
            if !schema::valid_key(&a.key) {
                return Err((format!("{field}.key"), FIELD_INVALID.into()));
            }
            if self.actions[..i].iter().any(|b| b.key == a.key) {
                return Err((format!("{field}.key"), "duplicate".into()));
            }
            check_name(&format!("{field}.name"), &a.name)?;
            if a.params.iter().any(|p| p.kind.is_temporal()) {
                return Err((format!("{field}.params"), FIELD_INVALID.into()));
            }
            schema::check_defs(&a.params, &format!("{field}.params"), known)?;
        }
        Ok(())
    }

    pub fn action(&self, key: &str) -> Option<&ActionDef> {
        self.actions.iter().find(|a| a.key == key)
    }
}

impl LinkTypeDef {
    pub fn check(&self, known: &dyn Fn(&str) -> bool) -> Result<(), schema::FieldError> {
        if !schema::valid_key(&self.key) || is_system_key(&self.key) {
            return Err(("key".into(), FIELD_INVALID.into()));
        }
        check_name("name", &self.name)?;
        if self.inverse_name.chars().count() > schema::NAME_MAX {
            return Err((
                "inverse_name".into(),
                format!("{FIELD_MAX_LENGTH}:{}", schema::NAME_MAX),
            ));
        }
        if self.system {
            return Err(("system".into(), FIELD_INVALID.into()));
        }
        check_set("from_types", &Some(self.from_types.clone()), known)?;
        check_set("to_types", &Some(self.to_types.clone()), known)?;
        if self.attributes.iter().any(|a| a.kind.is_temporal()) {
            return Err(("attributes".into(), FIELD_INVALID.into()));
        }
        schema::check_defs(&self.attributes, "attributes", known)
    }
}

/// The system object types, i.e. the D42 kinds as data.
pub fn system_object_types() -> Vec<ObjectTypeDef> {
    // Business objects: stored in a folder, contain nothing.
    let leaf = |key: &str| ObjectTypeDef {
        key: key.into(),
        name: String::new(),
        icon: String::new(),
        system: true,
        properties: Vec::new(),
        write_role: WriteRole::Member,
        may_contain: None,
        may_be_contained_in: Some(TypeSet::only(&[KIND_FOLDER])),
        actions: Vec::new(),
    };
    vec![
        leaf(KIND_DEVICE),
        leaf(KIND_MEDIA_ASSET),
        leaf(KIND_DASHBOARD),
        leaf(KIND_TOUR),
        leaf(KIND_FLOW),
        leaf(KIND_DOC),
        // A POI holds storage folders in its drawer.
        ObjectTypeDef {
            may_contain: Some(TypeSet::only(&[KIND_FOLDER])),
            ..leaf(KIND_MAP_PIN)
        },
        // The pure organisation container holds every type.
        ObjectTypeDef {
            may_contain: Some(TypeSet::Any),
            ..leaf(KIND_FOLDER)
        },
        // Investigation case (cases-workspace.md): its events and its undated
        // pieces are `in_case` links, its other members are what the events
        // involve; sub-cases by containment.
        ObjectTypeDef {
            icon: "book".into(),
            properties: case_properties(),
            may_contain: Some(TypeSet::only(&[KIND_CASE])),
            may_be_contained_in: Some(TypeSet::only(&[KIND_FOLDER, KIND_CASE])),
            ..leaf(KIND_CASE)
        },
        // Dated event, the pivot of a case (cases-workspace.md K20): an
        // instant or a period, to which objects are attached by `involves`.
        ObjectTypeDef {
            icon: "layers".into(),
            properties: event_properties(),
            ..leaf(KIND_EVENT)
        },
        // Minimal analysis note (K34): a hypothesis or remark attached to an
        // event without installing a pack.
        ObjectTypeDef {
            icon: "file-text".into(),
            properties: vec![prop(
                NOTE_BODY,
                PropertyKind::Text {
                    max_len: Some(4000),
                },
            )],
            ..leaf(KIND_NOTE)
        },
    ]
}

fn event_properties() -> Vec<PropertyDef> {
    vec![
        prop(EVENT_OCCURRED, PropertyKind::FuzzyTime),
        prop(
            EVENT_NOTE,
            PropertyKind::Text {
                max_len: Some(4000),
            },
        ),
        prop(
            EVENT_ORIGIN,
            PropertyKind::Enum {
                values: EVENT_ORIGINS.iter().map(|v| v.to_string()).collect(),
            },
        ),
        prop(EVENT_LOCATION, PropertyKind::GeoPoint),
    ]
}

fn prop(key: &str, kind: PropertyKind) -> PropertyDef {
    PropertyDef {
        key: key.into(),
        name: String::new(),
        kind,
        required: false,
        indexed: false,
    }
}

fn threshold(key: &str, unit: &str, min: f64, max: f64) -> PropertyDef {
    prop(
        key,
        PropertyKind::Number {
            unit: Some(unit.into()),
            min: Some(min),
            max: Some(max),
        },
    )
}

fn case_properties() -> Vec<PropertyDef> {
    vec![
        prop(
            CASE_QUESTION,
            PropertyKind::Text {
                max_len: Some(1000),
            },
        ),
        PropertyDef {
            required: true,
            indexed: true,
            ..prop(
                CASE_STATUS,
                PropertyKind::Enum {
                    values: CASE_STATUSES.iter().map(|v| v.to_string()).collect(),
                },
            )
        },
        prop(CASE_LEGAL_BASIS, PropertyKind::Text { max_len: Some(255) }),
        threshold(CASE_COPRESENCE_DISTANCE_M, "m", 1.0, 1_000.0),
        threshold(CASE_COPRESENCE_MIN_S, "s", 10.0, 86_400.0),
        threshold(CASE_STOP_DIAMETER_M, "m", 1.0, 1_000.0),
        threshold(CASE_STOP_MIN_S, "s", 60.0, 86_400.0),
        threshold(CASE_SIGNAL_GAP_S, "s", 10.0, 86_400.0),
        prop(
            CASE_CONCLUSION_PAGE,
            PropertyKind::Ref {
                to_type: crate::resources::KIND_DOC.into(),
            },
        ),
    ]
}

/// Investigation case (cases.md K1): system type without a native table.
pub const KIND_CASE: &str = "case";
pub const CASE_QUESTION: &str = "question";
pub const CASE_STATUS: &str = "status";
pub const CASE_STATUSES: [&str; 3] = ["open", CASE_CONCLUDED, "archived"];
pub const CASE_CONCLUDED: &str = "concluded";
/// Declared processing basis of the personal data in the case: required
/// before a person is attached (cases-workspace.md K38).
pub const CASE_LEGAL_BASIS: &str = "legal_basis";
/// Detector thresholds of the case (K30); unset = the detector default.
pub const CASE_COPRESENCE_DISTANCE_M: &str = "copresence_distance_m";
pub const CASE_COPRESENCE_MIN_S: &str = "copresence_min_s";
pub const CASE_STOP_DIAMETER_M: &str = "stop_diameter_m";
pub const CASE_STOP_MIN_S: &str = "stop_min_s";
pub const CASE_SIGNAL_GAP_S: &str = "signal_gap_s";
/// The page holding the conclusion (K8), versioned when the case concludes.
pub const CASE_CONCLUSION_PAGE: &str = "conclusion_page";
/// An event of the case, or an undated piece of it (K22): any object →
/// case; removing it closes the link. Roles live on `involves`.
pub const REL_IN_CASE: &str = "in_case";

/// Dated event (cases-workspace.md K20): system type without a native table.
pub const KIND_EVENT: &str = "event";
/// When it happened, at the precision known (instant or `range` period).
pub const EVENT_OCCURRED: &str = "occurred";
/// Analysis note: what is established, what is not, and from where.
pub const EVENT_NOTE: &str = "note";
/// How the event entered the case.
pub const EVENT_ORIGIN: &str = "origin";
pub const EVENT_ORIGINS: [&str; 4] = ["manual", "detection", "signal", "import"];
/// Place set by hand when no attached object locates the event.
pub const EVENT_LOCATION: &str = "location";
/// Analysis note (K34): system type without a native table.
pub const KIND_NOTE: &str = "note";
pub const NOTE_BODY: &str = "body";
/// Attachment of an object to an event (K21), role chosen at attach time.
pub const REL_INVOLVES: &str = "involves";
pub const INVOLVES_ROLE: &str = "role";
pub const INVOLVES_ROLES: [&str; 5] = ["evidence", "actor", "context", "hypothesis", "source"];
/// Free qualifier of the attachment ("witness", "operator on duty").
pub const INVOLVES_DETAIL: &str = "detail";
/// A tracker gives the position of an object while the link is open (K25).
pub const REL_TRACKS: &str = "tracks";

/// The system link types.
pub fn system_link_types() -> Vec<LinkTypeDef> {
    let text = |key: &str| PropertyDef {
        key: key.into(),
        name: String::new(),
        kind: PropertyKind::Text { max_len: Some(255) },
        required: true,
        indexed: false,
    };
    let link = |key: &str, from: TypeSet, to: TypeSet| LinkTypeDef {
        key: key.into(),
        name: String::new(),
        inverse_name: String::new(),
        system: true,
        from_types: from,
        to_types: to,
        one_target: false,
        one_source: false,
        attributes: Vec::new(),
    };
    vec![
        // Media, dashboard or tour placed on a POI (D39 → D42).
        link(
            REL_PLACED_ON,
            TypeSet::only(&[KIND_MAP_PIN]),
            TypeSet::only(&[KIND_MEDIA_ASSET, KIND_DASHBOARD, KIND_TOUR]),
        ),
        // A device installed at a POI (D43 → D191), one POI at a time.
        LinkTypeDef {
            one_target: true,
            ..link(
                REL_PLACED_AT,
                TypeSet::only(&[KIND_DEVICE]),
                TypeSet::only(&[KIND_MAP_PIN]),
            )
        },
        // A device metric feeds a `series` property of an object (D181):
        // replacing the sensor closes the link and opens another one.
        LinkTypeDef {
            attributes: vec![text(MEASURES_METRIC), text(MEASURES_PROPERTY)],
            ..link(REL_MEASURES, TypeSet::only(&[KIND_DEVICE]), TypeSet::Any)
        },
        // Event or undated piece of a case (K22): removing it closes the link.
        link(REL_IN_CASE, TypeSet::Any, TypeSet::only(&[KIND_CASE])),
        // Object attached to an event (K21): detaching closes the link.
        LinkTypeDef {
            attributes: vec![
                PropertyDef {
                    required: true,
                    ..prop(
                        INVOLVES_ROLE,
                        PropertyKind::Enum {
                            values: INVOLVES_ROLES.iter().map(|v| v.to_string()).collect(),
                        },
                    )
                },
                prop(INVOLVES_DETAIL, PropertyKind::Text { max_len: Some(255) }),
            ],
            ..link(REL_INVOLVES, TypeSet::only(&[KIND_EVENT]), TypeSet::Any)
        },
        // Tracker → tracked object (K25): the object's track is recomposed
        // from successive links, like `measures` for series (D181).
        link(REL_TRACKS, TypeSet::only(&[KIND_DEVICE]), TypeSet::Any),
    ]
}

/// Device installed at a POI (system link, D191).
pub const REL_PLACED_AT: &str = "placed_at";
/// Device metric bound to an object `series` property (system link, D181).
pub const REL_MEASURES: &str = "measures";
/// `measures` attribute: the device metric name.
pub const MEASURES_METRIC: &str = "metric";
/// `measures` attribute: the object property key.
pub const MEASURES_PROPERTY: &str = "property";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::KINDS;

    #[test]
    fn system_types_are_the_live_kinds() {
        let mut keys: Vec<String> = system_object_types()
            .into_iter()
            .filter(ObjectTypeDef::native)
            .map(|t| t.key)
            .collect();
        keys.sort();
        let mut kinds: Vec<String> = KINDS.iter().map(|k| k.to_string()).collect();
        kinds.sort();
        assert_eq!(keys, kinds);
    }

    #[test]
    fn every_referenced_type_exists() {
        let types = system_object_types();
        let links = system_link_types();
        let known = |k: &String| types.iter().any(|t| &t.key == k);
        let sets = types
            .iter()
            .flat_map(|t| [&t.may_contain, &t.may_be_contained_in])
            .flatten()
            .chain(links.iter().map(|l| &l.to_types));
        for set in sets {
            if let TypeSet::Only(keys) = set {
                assert!(keys.iter().all(known), "{keys:?}");
            }
        }
        for l in &links {
            if let TypeSet::Only(keys) = &l.from_types {
                assert!(keys.iter().all(known), "{keys:?}");
            }
        }
    }

    #[test]
    fn serde_roundtrip() {
        let types = system_object_types();
        let json = serde_json::to_string(&types).unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<ObjectTypeDef>>(&json).unwrap(),
            types
        );
        let links = system_link_types();
        let json = serde_json::to_string(&links).unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<LinkTypeDef>>(&json).unwrap(),
            links
        );
    }
}
