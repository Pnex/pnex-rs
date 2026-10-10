//! Ontology meta-model (D176–D191, lot L0): object types and link types
//! are **data**, not code.
//!
//! The system types are declared here once; the backend registry derives its
//! containment and relation rules from them (the hand-written `KindSpec` is
//! gone). Org types (lot L1) use the same serde format, loaded from the
//! database and exported as YAML (D186, D190). serde-only, wasm32-safe.

use serde::{Deserialize, Serialize};

use crate::resources::{
    KIND_DASHBOARD, KIND_DEVICE, KIND_FLOW, KIND_FOLDER, KIND_MAP_PIN, KIND_MEDIA_ASSET, KIND_TOUR,
    REL_PLACED_ON,
};

/// A set of object type keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypeSet {
    /// Every type, including types created later.
    Any,
    Only(Vec<String>),
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

/// An object type (D176). Containment rules live on the type (D180).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectTypeDef {
    /// `[a-z0-9_]`, unique per org (system keys are global).
    pub key: String,
    /// System types are native tables exposed through an adapter (D177);
    /// they cannot be deleted.
    #[serde(default)]
    pub system: bool,
    /// Types this one may contain; `None` = contains nothing.
    #[serde(default)]
    pub may_contain: Option<TypeSet>,
    /// Types this one may be contained in; `None` = never contained.
    #[serde(default)]
    pub may_be_contained_in: Option<TypeSet>,
}

/// A link type (D179): directed, from one type to a set of types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkTypeDef {
    pub key: String,
    #[serde(default)]
    pub system: bool,
    pub from_type: String,
    pub to_types: TypeSet,
}

impl LinkTypeDef {
    pub fn allows(&self, key: &str, from: &str, to: &str) -> bool {
        self.key == key && self.from_type == from && self.to_types.contains(to)
    }
}

/// The system object types, i.e. the D42 kinds as data.
pub fn system_object_types() -> Vec<ObjectTypeDef> {
    // Business objects: stored in a folder, contain nothing.
    let leaf = |key: &str| ObjectTypeDef {
        key: key.into(),
        system: true,
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
    vec![LinkTypeDef {
        // Media, dashboard or tour placed on a POI (D39 → D42).
        key: REL_PLACED_ON.into(),
        system: true,
        from_type: KIND_MAP_PIN.into(),
        to_types: TypeSet::only(&[KIND_MEDIA_ASSET, KIND_DASHBOARD, KIND_TOUR]),
    }]
}

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
        assert!(links.iter().all(|l| known(&l.from_type)));
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
