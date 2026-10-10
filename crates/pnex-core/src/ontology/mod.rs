//! Ontology meta-model (D176–D191, lot L0): object types and link types
//! are **data**, not code.
//!
//! The system types are declared here once; the backend registry derives its
//! containment and relation rules from them (the hand-written `KindSpec` is
//! gone). Org types (lot L1) use the same serde format, loaded from the
//! database and exported as YAML (D186, D190). serde-only, wasm32-safe.

pub mod api;
pub mod graph;
pub mod schema;

use serde::{Deserialize, Serialize};

pub use schema::{PropertyDef, PropertyKind};

use crate::err_codes::{FIELD_INVALID, FIELD_MAX_LENGTH, FIELD_REQUIRED};
use crate::resources::{
    KIND_DASHBOARD, KIND_DEVICE, KIND_FLOW, KIND_FOLDER, KIND_MAP_PIN, KIND_MEDIA_ASSET, KIND_TOUR,
    REL_PLACED_ON,
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
        schema::check_defs(&self.properties, "properties", known)
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
    };
    vec![
        leaf(KIND_DEVICE),
        leaf(KIND_MEDIA_ASSET),
        leaf(KIND_DASHBOARD),
        leaf(KIND_TOUR),
        leaf(KIND_FLOW),
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
    ]
}

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
        let mut keys: Vec<String> = system_object_types().into_iter().map(|t| t.key).collect();
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
