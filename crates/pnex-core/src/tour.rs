//! Studio — document de parcours de visite 3D (mode panorama V1).
//!
//! Le document (`tour_versions.doc`, JSONB) porte étages, scènes et liens ;
//! les assets (plans d'étage `floorplan`, panoramas `panorama`) restent des
//! `media_assets` **référencés par UUID, jamais dupliqués** (D21). École
//! `flow.rs` : types partagés backend ↔ front, serde-only (wasm32-safe),
//! validation pure ici — la validation **en base** (existence + kind des
//! assets dans l'org) vit côté service backend.
//!
//! Sémantique de versioning (école flows/D18) : append-only, save = nouvelle
//! version avec concurrence optimiste (`expected_version_number` → 409) ;
//! la version « courante » est la dernière ; la version publique est pointée
//! par `tours.published_version_id` (FK circulaire, école
//! `deployed_version_id`).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Seul mode du document en V1 (la colonne `tours.mode` prépare la tranche
/// maquette three.js — toute autre valeur est rejetée à la validation).
pub const TOUR_MODE_PANORAMA: &str = "panorama";

/// Lien intra-étage (dérivé, non éditable).
pub const TOUR_LINK_WALK: &str = "walk";
/// Lien inter-étages (escalier — dérivé, non éditable).
pub const TOUR_LINK_STAIR: &str = "stair";

/// Violation de validation du document (école `FlowViolation`) — message en
/// français, affichable tel quel au client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TourViolation {
    /// Élément fautif (id de scène/étage/lien), si localisé.
    pub subject: Option<String>,
    /// Code machine (`duplicate_scene_id`, `unknown_link_target`…).
    pub code: String,
    /// Message affichable.
    pub message: String,
}

/// Plan d'étage importé — référence d'un `media_asset` kind `floorplan`.
/// Les ids d'assets sont les UUIDs `media_assets.id` en forme string (le
/// crate reste sans dep uuid — wasm32-safe ; l'existence/kind sont validés
/// côté service backend).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FloorPlan {
    pub media_asset_id: String,
    /// Dimensions natives du plan en px (renseignées par l'éditeur au pick ;
    /// absentes côté natif → repli affichage).
    #[serde(default)]
    pub width: Option<f64>,
    #[serde(default)]
    pub height: Option<f64>,
    /// Échelle mètres par pixel (facultative, saisie éditeur).
    #[serde(default)]
    pub scale_m_per_px: Option<f64>,
}

/// Étage du parcours — positionné dans l'ordre du bâtiment (`level` :
/// 0 = RDC, négatif = sous-sol).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TourFloor {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub level: i32,
    /// `None` = étage sans plan (toléré : le viewer liste les scènes).
    #[serde(default)]
    pub plan: Option<FloorPlan>,
    /// Nord du plan en degrés (facultatif, décoratif V1).
    #[serde(default)]
    pub north_deg: f64,
}

/// Scène du parcours — un panorama 360 (`media_asset` kind `panorama`)
/// posé sur le plan de son étage (`x`/`y` en px natifs du plan).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TourScene {
    pub id: String,
    pub floor_id: String,
    #[serde(default)]
    pub label: String,
    pub media_asset_id: String,
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    /// Vue initiale dans le panorama (yaw/pitch en degrés, fov ∈ [1,179]).
    #[serde(default)]
    pub initial_yaw: f64,
    #[serde(default)]
    pub initial_pitch: f64,
    #[serde(default = "default_initial_fov")]
    pub initial_fov: f64,
}

fn default_initial_fov() -> f64 {
    100.0
}

/// Hotspot de navigation : d'une scène source vers une scène cible, posé sur
/// la sphère (`yaw`/`pitch` en degrés). Le kind `walk|stair` est **dérivé**
/// des étages des extrémités (jamais saisi).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TourLink {
    pub id: String,
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub yaw: f64,
    #[serde(default)]
    pub pitch: f64,
    #[serde(default = "default_link_kind")]
    pub kind: String,
    #[serde(default)]
    pub label: Option<String>,
}

fn default_link_kind() -> String {
    TOUR_LINK_WALK.to_string()
}

/// Document complet d'un parcours (contenu de `tour_versions.doc`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TourDoc {
    pub mode: String,
    /// Scène d'entrée (absent = première scène du document).
    pub start_scene: Option<String>,
    pub floors: Vec<TourFloor>,
    pub scenes: Vec<TourScene>,
    pub links: Vec<TourLink>,
}

impl Default for TourDoc {
    fn default() -> Self {
        Self {
            mode: TOUR_MODE_PANORAMA.to_string(),
            start_scene: None,
            floors: Vec::new(),
            scenes: Vec::new(),
            links: Vec::new(),
        }
    }
}

impl TourDoc {
    /// Document minimal d'un tour fraîchement créé (un étage, aucune scène).
    pub fn minimal() -> Self {
        Self {
            floors: vec![TourFloor {
                id: "f1".to_string(),
                name: "Ground floor".to_string(),
                level: 0,
                plan: None,
                north_deg: 0.0,
            }],
            ..Default::default()
        }
    }
}

fn violation(subject: Option<&str>, code: &str, message: String) -> TourViolation {
    TourViolation {
        subject: subject.map(str::to_string),
        code: code.to_string(),
        message,
    }
}

/// Validation **structurelle** pure du document (école `validate_graph`) —
/// nulle accès base : l'existence et le kind des assets référencés sont
/// vérifiés côté service (`validate_doc_assets`).
pub fn validate_tour_doc(doc: &TourDoc) -> Vec<TourViolation> {
    let mut v = Vec::new();
    if doc.mode != TOUR_MODE_PANORAMA {
        v.push(violation(
            None,
            "unknown_mode",
            format!(
                "mode de tour inconnu : {} (attendu {TOUR_MODE_PANORAMA})",
                doc.mode
            ),
        ));
    }

    // Ids uniques par collection + non vides.
    let mut floor_ids = std::collections::HashSet::new();
    for f in &doc.floors {
        if f.id.is_empty() {
            v.push(violation(None, "empty_id", "id d'étage vide".to_string()));
        } else if !floor_ids.insert(&f.id) {
            v.push(violation(
                Some(&f.id),
                "duplicate_floor_id",
                format!("id d'étage dupliqué : {}", f.id),
            ));
        }
    }
    let mut scene_ids = std::collections::HashSet::new();
    for s in &doc.scenes {
        if s.id.is_empty() {
            v.push(violation(None, "empty_id", "id de scène vide".to_string()));
        } else if !scene_ids.insert(&s.id) {
            v.push(violation(
                Some(&s.id),
                "duplicate_scene_id",
                format!("id de scène dupliqué : {}", s.id),
            ));
        }
    }
    let mut link_ids = std::collections::HashSet::new();
    let mut link_pairs = std::collections::HashSet::new();
    for l in &doc.links {
        if l.id.is_empty() {
            v.push(violation(None, "empty_id", "id de lien vide".to_string()));
        } else if !link_ids.insert(&l.id) {
            v.push(violation(
                Some(&l.id),
                "duplicate_link_id",
                format!("id de lien dupliqué : {}", l.id),
            ));
        }
        if l.from == l.to {
            v.push(violation(
                Some(&l.id),
                "self_link",
                format!("le lien {} boucle sur sa scène source", l.id),
            ));
        } else if !link_pairs.insert((l.from.clone(), l.to.clone())) {
            v.push(violation(
                Some(&l.id),
                "duplicate_link",
                format!("lien déjà existant : {} → {}", l.from, l.to),
            ));
        }
        if !scene_ids.contains(&l.from) {
            v.push(violation(
                Some(&l.id),
                "unknown_link_target",
                format!("lien {} : scène source inconnue {}", l.id, l.from),
            ));
        }
        if !scene_ids.contains(&l.to) {
            v.push(violation(
                Some(&l.id),
                "unknown_link_target",
                format!("lien {} : scène cible inconnue {}", l.id, l.to),
            ));
        }
    }

    // Scènes : étage connu + champs numériques cohérents.
    for s in &doc.scenes {
        if !floor_ids.contains(&s.floor_id) {
            v.push(violation(
                Some(&s.id),
                "unknown_floor",
                format!("scène {} : étage inconnu {}", s.id, s.floor_id),
            ));
        }
        if !(s.x.is_finite() && s.y.is_finite()) {
            v.push(violation(
                Some(&s.id),
                "invalid_position",
                format!("scène {} : position non finie", s.id),
            ));
        }
        if !(s.initial_fov.is_finite() && (1.0..=179.0).contains(&s.initial_fov)) {
            v.push(violation(
                Some(&s.id),
                "invalid_fov",
                format!("scène {} : fov initial hors [1, 179]", s.id),
            ));
        }
        if !(s.initial_pitch.is_finite() && (-90.0..=90.0).contains(&s.initial_pitch)) {
            v.push(violation(
                Some(&s.id),
                "invalid_pitch",
                format!("scène {} : pitch initial hors [-90, 90]", s.id),
            ));
        }
        if !s.initial_yaw.is_finite() {
            v.push(violation(
                Some(&s.id),
                "invalid_yaw",
                format!("scène {} : yaw initial non fini", s.id),
            ));
        }
    }

    // Plans : dimensions et échelle strictement positives quand présentes.
    for f in &doc.floors {
        if let Some(plan) = &f.plan {
            if plan.width.is_some_and(|w| !w.is_finite() || w <= 0.0)
                || plan.height.is_some_and(|h| !h.is_finite() || h <= 0.0)
            {
                v.push(violation(
                    Some(&f.id),
                    "invalid_plan_dimensions",
                    format!("étage {} : dimensions de plan invalides", f.id),
                ));
            }
            if plan
                .scale_m_per_px
                .is_some_and(|s| !s.is_finite() || s <= 0.0)
            {
                v.push(violation(
                    Some(&f.id),
                    "invalid_plan_scale",
                    format!("étage {} : échelle du plan invalide", f.id),
                ));
            }
        }
        if !f.north_deg.is_finite() {
            v.push(violation(
                Some(&f.id),
                "invalid_north",
                format!("étage {} : nord non fini", f.id),
            ));
        }
    }

    // Scène de départ connue.
    if let Some(start) = &doc.start_scene {
        if !scene_ids.contains(start) {
            v.push(violation(
                Some(start),
                "unknown_start_scene",
                format!("scène de départ inconnue : {start}"),
            ));
        }
    }

    v
}

// ─────────────────────────── DTOs API ───────────────────────────

/// Tour en liste (école `FlowSummary`).
/// `PartialEq` requis par les props des composants du socle CRUD (macro
/// `#[component]` dioxus 0.7 — impl généré par champ).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TourSummary {
    pub id: String,
    pub org_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub mode: String,
    pub latest_version_number: i64,
    /// Version publique (numéro résolu depuis `published_version_id`).
    pub published_version_number: Option<i64>,
    pub share_enabled: bool,
    /// Secret de publication — peuplé **uniquement pour un writer**.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_token: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Tour en détail : + document de la version consultée.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TourDetail {
    pub id: String,
    pub org_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub mode: String,
    pub doc: TourDoc,
    /// Version du document renvoyé (latest, ou celle demandée par `?version=`).
    pub doc_version_number: i64,
    pub latest_version_number: i64,
    pub published_version_number: Option<i64>,
    pub share_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_token: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Entrée d'historique (école `FlowVersionSummary`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TourVersionSummary {
    pub id: String,
    pub version_number: i64,
    pub author: Option<String>,
    pub note: Option<String>,
    pub published: bool,
    pub created_at: String,
}

/// Détail d'une version (audit / rechargement éditeur).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TourVersionDetail {
    pub id: String,
    pub version_number: i64,
    pub author: Option<String>,
    pub note: Option<String>,
    pub published: bool,
    pub created_at: String,
    pub doc: TourDoc,
}

/// `POST /api/v1/tours`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTour {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// `PATCH /api/v1/tours/{id}` — save append-only avec concurrence optimiste.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateTour {
    pub expected_version_number: i64,
    pub doc: TourDoc,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// `POST /api/v1/tours/{id}/publish` — version absente = dernière.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PublishTour {
    #[serde(default)]
    pub version_number: Option<i64>,
}

/// Résolution d'un asset référencé par le doc publié (endpoint public).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicAssetRef {
    pub version_number: i64,
    pub content_type: String,
    pub filename: String,
    pub size_bytes: i64,
}

/// Réponse du endpoint public (sans auth) — version publiée + carte des
/// assets référencés, pour booter le viewer en un seul aller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicTour {
    pub name: String,
    pub description: Option<String>,
    pub mode: String,
    pub published_version_number: i64,
    pub published_at: String,
    pub doc: TourDoc,
    pub assets: HashMap<String, PublicAssetRef>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_doc() -> TourDoc {
        let pano = "00000000-0000-0000-0000-000000000000".to_string();
        let plan = "00000000-0000-0000-0000-000000000001".to_string();
        TourDoc {
            mode: TOUR_MODE_PANORAMA.into(),
            start_scene: Some("s1".into()),
            floors: vec![TourFloor {
                id: "f1".into(),
                name: "RDC".into(),
                level: 0,
                plan: Some(FloorPlan {
                    media_asset_id: plan,
                    width: Some(2048.0),
                    height: Some(1024.0),
                    scale_m_per_px: Some(0.05),
                }),
                north_deg: 0.0,
            }],
            scenes: vec![
                TourScene {
                    id: "s1".into(),
                    floor_id: "f1".into(),
                    label: "Salon".into(),
                    media_asset_id: pano.clone(),
                    x: 512.0,
                    y: 300.0,
                    initial_yaw: 0.0,
                    initial_pitch: 0.0,
                    initial_fov: 100.0,
                },
                TourScene {
                    id: "s2".into(),
                    floor_id: "f1".into(),
                    label: "Cuisine".into(),
                    media_asset_id: pano.clone(),
                    x: 900.0,
                    y: 300.0,
                    initial_yaw: 90.0,
                    initial_pitch: 0.0,
                    initial_fov: 100.0,
                },
            ],
            links: vec![TourLink {
                id: "l1".into(),
                from: "s1".into(),
                to: "s2".into(),
                yaw: 90.0,
                pitch: 0.0,
                kind: TOUR_LINK_WALK.into(),
                label: Some("Vers cuisine".into()),
            }],
        }
    }

    #[test]
    fn doc_serde_roundtrip() {
        let doc = sample_doc();
        let json = serde_json::to_string(&doc).unwrap();
        let back: TourDoc = serde_json::from_str(&json).unwrap();
        assert_eq!(back, doc);
    }

    #[test]
    fn doc_tolerant_aux_champs_manquants() {
        // Un doc ancien sans start_scene/links ni options du plan se charge.
        let doc: TourDoc = serde_json::from_str(
            r#"{"mode":"panorama","floors":[{"id":"f1","name":"RDC","plan":{"media_asset_id":"00000000-0000-0000-0000-000000000001"}}],
                "scenes":[{"id":"s1","floor_id":"f1","media_asset_id":"00000000-0000-0000-0000-000000000000"}]}"#,
        )
        .unwrap();
        assert_eq!(doc.start_scene, None);
        assert!(doc.links.is_empty());
        assert_eq!(doc.scenes[0].initial_fov, 100.0, "défaut fov");
    }

    #[test]
    fn doc_minimal_valide() {
        assert!(validate_tour_doc(&TourDoc::minimal()).is_empty());
        assert!(validate_tour_doc(&TourDoc::default()).is_empty());
    }

    #[test]
    fn validation_structurelle() {
        let mut doc = sample_doc();
        assert!(
            validate_tour_doc(&doc).is_empty(),
            "doc valide sans violation"
        );

        // Lien orphelin + boucle sur soi + doublon.
        doc.links.push(TourLink {
            id: "l2".into(),
            from: "s1".into(),
            to: "zz".into(),
            yaw: 0.0,
            pitch: 0.0,
            kind: TOUR_LINK_WALK.into(),
            label: None,
        });
        doc.links.push(TourLink {
            id: "l3".into(),
            from: "s1".into(),
            to: "s1".into(),
            yaw: 0.0,
            pitch: 0.0,
            kind: TOUR_LINK_WALK.into(),
            label: None,
        });
        doc.links.push(TourLink {
            id: "l4".into(),
            from: "s1".into(),
            to: "s2".into(),
            yaw: 0.0,
            pitch: 0.0,
            kind: TOUR_LINK_WALK.into(),
            label: None,
        });
        let violations = validate_tour_doc(&doc);
        let codes: Vec<&str> = violations.iter().map(|v| v.code.as_str()).collect();
        assert!(codes.contains(&"unknown_link_target"));
        assert!(codes.contains(&"self_link"));
        assert!(codes.contains(&"duplicate_link"));

        // Scène hors étage + fov hors bornes + départ inconnu.
        let mut doc = sample_doc();
        doc.scenes[0].floor_id = "zz".into();
        doc.scenes[0].initial_fov = 200.0;
        doc.start_scene = Some("nope".into());
        let violations = validate_tour_doc(&doc);
        let codes: Vec<&str> = violations.iter().map(|v| v.code.as_str()).collect();
        assert!(codes.contains(&"unknown_floor"));
        assert!(codes.contains(&"invalid_fov"));
        assert!(codes.contains(&"unknown_start_scene"));

        // Mode inconnu.
        let mut doc = sample_doc();
        doc.mode = "hologramme".into();
        let violations = validate_tour_doc(&doc);
        assert_eq!(violations[0].code, "unknown_mode");
    }
}
