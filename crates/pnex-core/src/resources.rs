//! Couche d'organisation transverse (D42) — vocabulaire partagé
//! backend ↔ front, serde-only (wasm32-safe).
//!
//! Trois mécanismes séparés : **labels** (classification requêtable GIN),
//! **containment** (arbre, un seul parent), **edges** (liens + placement).
//! Labels effectifs résolus au read (propres + ancêtres). Validité
//! kind×relation×kind déclarée par kind (registre backend) — le moteur ne
//! `match` jamais sur les kinds. Découplage device : organiser n'écrit jamais
//! dans `device_registries` ni le firmware.

// ─────────────────────────── ResourceRef ───────────────────────────

/// Identité polymorphe — (org, kind, id), `id` = PK stringifiée.
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

/// Concepts enregistrés (additif : futur concept = 1 constante + 1 entrée de
/// registre backend, rien d'autre).
pub const KIND_DEVICE: &str = "device";
pub const KIND_MEDIA_ASSET: &str = "media_asset";
pub const KIND_DASHBOARD: &str = "dashboard";
pub const KIND_TOUR: &str = "tour";
pub const KIND_MAP_PIN: &str = "map_pin";
pub const KIND_FLOW: &str = "flow";
/// Conteneur d'organisation pur (nom + emoji), contient tous les kinds.
pub const KIND_FOLDER: &str = "folder";

/// Kinds vivants, pour `valid_kind` et l'enumération du registre.
pub const KINDS: [&str; 7] = [
    KIND_DEVICE,
    KIND_MEDIA_ASSET,
    KIND_DASHBOARD,
    KIND_TOUR,
    KIND_MAP_PIN,
    KIND_FLOW,
    KIND_FOLDER,
];

pub fn valid_kind(kind: &str) -> bool {
    KINDS.contains(&kind)
}

// ─────────────────────────── relations ───────────────────────────

/// Pano/média posé sur un POI (absorbe viz_links, D39 → D42).
pub const REL_PLACED_ON: &str = "placed_on";

// ─────────────────────────── labels ───────────────────────────

/// `{"site": "serre", "critique": null}` — tag nu = `None`.
pub type LabelSet = std::collections::BTreeMap<String, Option<String>>;

/// Cap de classification (labels ≠ vrac : le libre reste dans `metadata`).
pub const LABELS_MAX: usize = 32;
/// Nom : `[a-z0-9_-]`, 1..=64.
pub const LABEL_NAME_MAX: usize = 64;
/// Valeur : trimmée non-vide, ≤ 255 chars (ou tag nu = `None`).
pub const LABEL_VALUE_MAX: usize = 255;

/// Nom de label valide — charset fermé, école `naming.rs`.
pub fn valid_label_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= LABEL_NAME_MAX
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Valeur de label valide après trim (tag nu autorise `None`).
pub fn valid_label_value(value: &str) -> bool {
    !value.is_empty() && value.chars().count() <= LABEL_VALUE_MAX
}

/// Filtre de liste `"site:serre"` / `"critique"` (tag nu).
/// `(None, Some(msg))` = invalide.
pub fn parse_label_filter(raw: &str) -> (Option<(String, Option<String>)>, Option<String>) {
    let raw = raw.trim();
    if raw.is_empty() {
        return (None, Some("filtre label vide".into()));
    }
    match raw.split_once(':') {
        Some((name, value)) => {
            let value = value.trim();
            if !valid_label_name(name) {
                return (None, Some(format!("nom de label invalide : {name}")));
            }
            if !valid_label_value(value) {
                return (None, Some(format!("valeur de label invalide : {value}")));
            }
            (Some((name.to_string(), Some(value.to_string()))), None)
        }
        None => {
            if !valid_label_name(raw) {
                return (None, Some(format!("nom de label invalide : {raw}")));
            }
            (Some((raw.to_string(), None)), None)
        }
    }
}

// ─────────────────────────── specs déclaratives ───────────────────────────

/// Règles de containment d'un kind — déclarées **par le kind**.
/// `None` = interdit ; `Some(&[])` = sans restriction (wildcard) ;
/// `Some(&[kinds])` = liste close.
#[derive(Debug, Clone, Copy)]
pub struct ContainmentRules {
    /// Ce kind peut-il *contenir* d'autres ressources ?
    pub may_contain: Option<&'static [&'static str]>,
    /// Ce kind peut-il *être contenu* ?
    pub may_be_contained_in: Option<&'static [&'static str]>,
}

/// Une relation déclarée par un kind source : `relation` vers `target_kinds`.
#[derive(Debug, Clone, Copy)]
pub struct RelationSpec {
    pub relation: &'static str,
    pub source_kind: &'static str,
    /// Cibles admises (`&[]` = wildcard).
    pub target_kinds: &'static [&'static str],
}

/// Ce que le moteur lit pour décider — jamais de `match` sur les kinds.
#[derive(Debug, Clone)]
pub struct KindSpec {
    pub kind: &'static str,
    pub containment: ContainmentRules,
    pub relations: Vec<RelationSpec>,
}

// ─────────────────────────── tests ───────────────────────────

/// One step of a location breadcrumb: a site (`map_pin`) or a folder.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
pub struct ResourceLocation {
    pub site: LocationNode,
    #[serde(default)]
    pub folders: Vec<LocationNode>,
}

/// `GET /api/v1/resources/{kind}/{id}/location`.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
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
        assert!(!valid_label_name("Site")); // casse interdite (normalisé bas)
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
        assert!(f.is_none() && err.is_some(), "nom non normalisé rejeté");
    }
}
