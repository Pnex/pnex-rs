//! Annotations sur médias (D55–D60, doctrine `docs/architecture/annotations.md`)
//! — document JSONB des couches (`annotation_layer_versions.doc`, structure
//! `pnex_core::AnnotationDoc`) : items ancrés sur des `media_assets` par UUID
//! (jamais sur une scène ni un tour — D55), cibles faibles résolues au read
//! (D57). École `tour.rs` : types partagés backend ↔ front, serde-only
//! (wasm32-safe), validation **structurelle** pure ici — l'existence des
//! assets/kind et des devices référencés est validée **en base** côté service
//! backend (`services/annotation_layer.rs`).
//!
//! Sémantique de versioning (école flows/D18 école `tours`) : append-only,
//! save = nouvelle version avec concurrence optimiste
//! (`expected_version_number` → 409) ; la version « courante » est la
//! dernière ; la version publique est pointée par
//! `annotation_layers.published_version_id` (FK circulaire, école
//! `tours.published_version_id`).

use serde::{Deserialize, Serialize};

/// Kinds d'item — string applicatif, cohérence avec `target.type` validée à
/// l'écriture (`kind_target_mismatch`). Le device est identifié par **slug**
/// (`device_registries.device_id`), la pin par **gpio** (identité machine
/// stable, pas un label éditable).
pub const ANNOTATION_KIND_DEVICE: &str = "device";
pub const ANNOTATION_KIND_PIN: &str = "pin";
pub const ANNOTATION_KIND_STATUS: &str = "status";
pub const ANNOTATION_KIND_NOTE: &str = "note";

/// Violation de validation du document (école `TourViolation`) — message en
/// français, affichable tel quel au client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnnotationViolation {
    /// Item fautif (id d'item), si localisé.
    pub subject: Option<String>,
    /// Code machine (`duplicate_item_id`, `invalid_yaw`…).
    pub code: String,
    /// Message affichable.
    pub message: String,
}

/// Géométrie d'un item — sur sphère équirectangulaire (panorama) ou en
/// pourcentage du conteneur d'une image plate (photo/floorplan).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AnnotationGeometry {
    /// Panorama : yaw ∈ [-180, 180], pitch ∈ [-90, 90] (degrés).
    Equirect { yaw: f64, pitch: f64 },
    /// Image plate : x/y ∈ [0, 1] (fraction du conteneur de l'`img`).
    Flat { x: f64, y: f64 },
}

// Manual `Deserialize`: internally-tagged enums buffer fields through
// serde's `Content` deserializer, which is incompatible with serde_json's
// `arbitrary_precision` feature (unified workspace-wide via the starlark
// dependency) — buffered numbers arrive as private magic maps and typed
// `f64` fields fail with "invalid type: map". Parsing through a
// `serde_json::Value` keeps numbers as real Numbers, so the geometry
// survives any feature unification.
impl<'de> Deserialize<'de> for AnnotationGeometry {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v = serde_json::Value::deserialize(d)?;
        let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match kind {
            "equirect" => {
                Ok(Self::Equirect {
                    yaw: v.get("yaw").and_then(|n| n.as_f64()).ok_or_else(|| {
                        serde::de::Error::custom("equirect requires numeric `yaw`")
                    })?,
                    pitch: v.get("pitch").and_then(|n| n.as_f64()).ok_or_else(|| {
                        serde::de::Error::custom("equirect requires numeric `pitch`")
                    })?,
                })
            }
            "flat" => Ok(Self::Flat {
                x: v.get("x")
                    .and_then(|n| n.as_f64())
                    .ok_or_else(|| serde::de::Error::custom("flat requires numeric `x`"))?,
                y: v.get("y")
                    .and_then(|n| n.as_f64())
                    .ok_or_else(|| serde::de::Error::custom("flat requires numeric `y`"))?,
            }),
            other => Err(serde::de::Error::custom(format!(
                "unknown geometry type: {other}"
            ))),
        }
    }
}

/// Cible faible d'un item (D57) — résolue **au read, en batch** (école
/// `link_target_labels`) ; référence morte tolérée (`dead: true`, jamais 500).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AnnotationTarget {
    Device { device_id: String },
    Pin { device_id: String, pin_gpio: u32 },
    Status { device_id: String },
    Note { text: String },
}

/// Item d'annotation posé sur un média.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnnotationItem {
    pub id: String,
    /// `media_assets.id` — l'ancre unique du système (D55).
    pub media_asset_id: String,
    /// `device|pin|status|note` — string applicatif.
    pub kind: String,
    pub geometry: AnnotationGeometry,
    /// Couleur du marqueur (`#rrggbb`), défaut serveur sinon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default)]
    pub label: String,
    pub target: AnnotationTarget,
}

/// Document complet d'une couche (contenu de
/// `annotation_layer_versions.doc`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AnnotationDoc {
    pub items: Vec<AnnotationItem>,
}

fn violation(subject: Option<&str>, code: &str, message: String) -> AnnotationViolation {
    AnnotationViolation {
        subject: subject.map(str::to_string),
        code: code.to_string(),
        message,
    }
}

/// Cible compatible avec le kind ? (le kind est un string applicatif, la
/// cohérence kind ↔ target.type est la seule règle).
fn kind_matches_target(kind: &str, target: &AnnotationTarget) -> bool {
    match target {
        AnnotationTarget::Device { .. } => kind == ANNOTATION_KIND_DEVICE,
        AnnotationTarget::Pin { .. } => kind == ANNOTATION_KIND_PIN,
        AnnotationTarget::Status { .. } => kind == ANNOTATION_KIND_STATUS,
        AnnotationTarget::Note { .. } => kind == ANNOTATION_KIND_NOTE,
    }
}

/// Validation **structurelle** pure du document (école `validate_tour_doc`) —
/// nulle accès base : l'existence/kind des assets et l'existence des devices
/// référencés sont vérifiés côté service (batch, école `validate_doc_assets`).
pub fn validate_annotation_doc(doc: &AnnotationDoc) -> Vec<AnnotationViolation> {
    let mut v = Vec::new();
    let mut ids = std::collections::HashSet::new();
    for item in &doc.items {
        if item.id.is_empty() {
            v.push(violation(
                None,
                "empty_item_id",
                "id d'item vide".to_string(),
            ));
        } else if !ids.insert(&item.id) {
            v.push(violation(
                Some(&item.id),
                "duplicate_item_id",
                format!("id d'item dupliqué : {}", item.id),
            ));
        }
        if item.media_asset_id.is_empty() {
            v.push(violation(
                Some(&item.id),
                "empty_media_asset_id",
                format!("item {} : media_asset_id vide", item.id),
            ));
        }
        if !kind_matches_target(&item.kind, &item.target) {
            v.push(violation(
                Some(&item.id),
                "kind_target_mismatch",
                format!(
                    "item {} : kind `{}` incompatible avec la cible",
                    item.id, item.kind
                ),
            ));
        }
        match &item.geometry {
            AnnotationGeometry::Equirect { yaw, pitch } => {
                if !yaw.is_finite() || !(-180.0..=180.0).contains(yaw) {
                    v.push(violation(
                        Some(&item.id),
                        "invalid_yaw",
                        format!("item {} : yaw hors [-180, 180]", item.id),
                    ));
                }
                if !pitch.is_finite() || !(-90.0..=90.0).contains(pitch) {
                    v.push(violation(
                        Some(&item.id),
                        "invalid_pitch",
                        format!("item {} : pitch hors [-90, 90]", item.id),
                    ));
                }
            }
            AnnotationGeometry::Flat { x, y } => {
                if !x.is_finite()
                    || !(0.0..=1.0).contains(x)
                    || !y.is_finite()
                    || !(0.0..=1.0).contains(y)
                {
                    v.push(violation(
                        Some(&item.id),
                        "invalid_position",
                        format!("item {} : position hors [0, 1]", item.id),
                    ));
                }
            }
        }
        // Couleur `#rrggbb` stricte (D55 §4 : `^#[0-9a-fA-F]{6}$`).
        if let Some(color) = &item.color {
            let hex = color.strip_prefix('#').unwrap_or("");
            if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                v.push(violation(
                    Some(&item.id),
                    "invalid_color",
                    format!("item {} : couleur invalide (attendu #rrggbb)", item.id),
                ));
            }
        }
    }
    v
}

/// `true` si la géométrie exige un média kind `panorama` (sinon image plate :
/// `photo`/`floorplan` — même liste que les plans d'étage des tours).
pub fn geometry_requires_panorama(g: &AnnotationGeometry) -> bool {
    matches!(g, AnnotationGeometry::Equirect { .. })
}

// ─────────────────────────── DTOs API ───────────────────────────

/// Couche en liste (école `TourSummary`) — `media_asset_id` : le média
/// associé de l'ensemble (pivot UX §9). `PartialEq` requis par les props
/// des composants du socle CRUD (macro `#[component]` dioxus 0.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnnotationLayerSummary {
    pub id: String,
    pub org_id: i64,
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub media_asset_id: Option<String>,
    /// Tour associé (000028) — XOR media_asset_id à la création.
    #[serde(default)]
    pub tour_id: Option<String>,
    pub latest_version_number: i64,
    /// Version publiée (numéro résolu depuis `published_version_id`).
    pub published_version_number: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

/// Couche en détail : + document de la version consultée.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnotationLayerDetail {
    pub id: String,
    pub org_id: i64,
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub media_asset_id: Option<String>,
    /// Tour associé (000028) — XOR media_asset_id à la création.
    #[serde(default)]
    pub tour_id: Option<String>,
    pub doc: AnnotationDoc,
    /// Version du document renvoyé (latest, ou celle demandée par `?version=`).
    pub doc_version_number: i64,
    pub latest_version_number: i64,
    pub published_version_number: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

/// Entrée d'historique (école `TourVersionSummary`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnotationLayerVersionSummary {
    pub id: String,
    pub version_number: i64,
    pub author: Option<String>,
    pub note: Option<String>,
    pub published: bool,
    pub created_at: String,
}

/// Détail d'une version (audit / rechargement éditeur).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnotationLayerVersionDetail {
    pub id: String,
    pub version_number: i64,
    pub author: Option<String>,
    pub note: Option<String>,
    pub published: bool,
    pub created_at: String,
    pub doc: AnnotationDoc,
}

/// `POST /api/v1/annotation-layers` — pivot UX 2026-09-16 : l'ensemble
/// **déclare son média** (`media_asset_id`, pivot UX §9 — « un nom global,
/// un média associé et les annotations ») ; tous les items du doc devront
/// y être ancrés.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAnnotationLayer {
    pub name: String,
    /// Média associé (UUID en string) — l'ancre unique de l'ensemble.
    #[serde(default)]
    pub media_asset_id: Option<String>,
    /// Tour associé (UUID en string, 000028) — XOR media_asset_id.
    #[serde(default)]
    pub tour_id: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// `PATCH /api/v1/annotation-layers/{id}` — save append-only, 409 optimiste.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateAnnotationLayer {
    pub expected_version_number: i64,
    pub doc: AnnotationDoc,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// `POST /{id}/publish` — version absente = dernière (pointeur).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PublishAnnotationLayer {
    #[serde(default)]
    pub version_number: Option<i64>,
}

// ─────────────────────────── Read model viewers ───────────────────────────

/// Réponse du read model `GET /api/v1/media/{asset_id}/annotations` — items
/// **fusionnés des couches publiées** ancrées sur l'asset (D55 : le lien
/// tour↔couche est implicite, la composition se fait au rendu).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaAnnotations {
    pub media_asset_id: String,
    pub items: Vec<ResolvedAnnotationItem>,
}

/// Item read-model : doc + couche d'origine + résolution de cible (D57).
/// `PartialEq` : props dioxus (composants front, école `TourDetail`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedAnnotationItem {
    // Document.
    pub id: String,
    pub media_asset_id: String,
    pub kind: String,
    pub geometry: AnnotationGeometry,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default)]
    pub label: String,
    pub target: AnnotationTarget,
    // Couche d'origine.
    pub layer_id: String,
    pub layer_name: String,
    /// `None` pour les items `note` (pas de cible device).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved: Option<ResolvedTargetInfo>,
}

/// Résolution de cible (batch au read, école `link_target_labels`) —
/// `dead: true` = le device a disparu : référence morte **tolérée**, le
/// viewer grise (jamais 500, jamais panic, école S7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedTargetInfo {
    /// PK `device_registries.id` — pour le poll live front
    /// (`/devices/{id}/pins`) — `None` si la cible est morte.
    pub device_pk: Option<i64>,
    /// Slug (`device_registries.device_id`).
    pub device_label: String,
    pub dead: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset() -> String {
        "00000000-0000-0000-0000-000000000000".to_string()
    }

    fn item(
        id: &str,
        kind: &str,
        target: AnnotationTarget,
        geometry: AnnotationGeometry,
    ) -> AnnotationItem {
        AnnotationItem {
            id: id.to_string(),
            media_asset_id: asset(),
            kind: kind.to_string(),
            geometry,
            color: None,
            label: format!("Item {id}"),
            target,
        }
    }

    #[test]
    fn doc_serde_roundtrip() {
        let doc = AnnotationDoc {
            items: vec![
                item(
                    "a1",
                    ANNOTATION_KIND_DEVICE,
                    AnnotationTarget::Device {
                        device_id: "pac-01".into(),
                    },
                    AnnotationGeometry::Equirect {
                        yaw: 45.0,
                        pitch: -10.0,
                    },
                ),
                item(
                    "a2",
                    ANNOTATION_KIND_PIN,
                    AnnotationTarget::Pin {
                        device_id: "pac-01".into(),
                        pin_gpio: 4,
                    },
                    AnnotationGeometry::Flat { x: 0.32, y: 0.48 },
                ),
                item(
                    "a3",
                    ANNOTATION_KIND_NOTE,
                    AnnotationTarget::Note {
                        text: "Vanne CAU fermée l'été".into(),
                    },
                    AnnotationGeometry::Equirect {
                        yaw: 0.0,
                        pitch: 0.0,
                    },
                ),
            ],
        };
        let json = serde_json::to_string(&doc).unwrap();
        let back: AnnotationDoc = serde_json::from_str(&json).unwrap();
        assert_eq!(back, doc);
        // Forme wire conforme à la doctrine §4.
        assert!(json.contains(r#""type":"equirect""#));
        assert!(json.contains(r#""type":"flat""#));
        assert!(json.contains(r#""pin_gpio":4"#));
    }

    #[test]
    fn doc_tolerant_aux_champs_manquants() {
        // Doc ancien sans color/label : défauts posés, doc accepté.
        let doc: AnnotationDoc = serde_json::from_str(
            r#"{"items":[{"id":"a1","media_asset_id":"00000000-0000-0000-0000-000000000000",
                "kind":"device","geometry":{"type":"equirect","yaw":1.0,"pitch":-2.0},
                "target":{"type":"device","device_id":"pac-01"}}]}"#,
        )
        .unwrap();
        assert_eq!(doc.items[0].label, "");
        assert_eq!(doc.items[0].color, None);
    }

    #[test]
    fn doc_vide_valide() {
        assert!(validate_annotation_doc(&AnnotationDoc::default()).is_empty());
    }

    #[test]
    fn validation_structurelle() {
        let mut doc = AnnotationDoc {
            items: vec![item(
                "a1",
                ANNOTATION_KIND_STATUS,
                AnnotationTarget::Status {
                    device_id: "pac-01".into(),
                },
                AnnotationGeometry::Equirect {
                    yaw: 0.0,
                    pitch: 0.0,
                },
            )],
        };
        assert!(
            validate_annotation_doc(&doc).is_empty(),
            "doc valide sans violation"
        );

        // Doublon + id vide + asset vide + kind/cible incohérents.
        doc.items.push(item(
            "a1",
            ANNOTATION_KIND_DEVICE,
            AnnotationTarget::Device {
                device_id: "pac-01".into(),
            },
            AnnotationGeometry::Equirect {
                yaw: 0.0,
                pitch: 0.0,
            },
        ));
        doc.items.push(item(
            "",
            ANNOTATION_KIND_DEVICE,
            AnnotationTarget::Device {
                device_id: "pac-01".into(),
            },
            AnnotationGeometry::Equirect {
                yaw: 0.0,
                pitch: 0.0,
            },
        ));
        let mut orphan = item(
            "a2",
            ANNOTATION_KIND_DEVICE,
            AnnotationTarget::Device {
                device_id: "pac-01".into(),
            },
            AnnotationGeometry::Equirect {
                yaw: 0.0,
                pitch: 0.0,
            },
        );
        orphan.media_asset_id = String::new();
        doc.items.push(orphan);
        let mut mismatch = item(
            "a3",
            ANNOTATION_KIND_PIN,
            AnnotationTarget::Device {
                device_id: "pac-01".into(),
            },
            AnnotationGeometry::Flat { x: 0.5, y: 0.5 },
        );
        mismatch.target = AnnotationTarget::Device {
            device_id: "pac-01".into(),
        };
        doc.items.push(mismatch);
        let codes: Vec<String> = validate_annotation_doc(&doc)
            .into_iter()
            .map(|v| v.code)
            .collect();
        assert!(codes.iter().any(|c| c == "duplicate_item_id"), "{codes:?}");
        assert!(codes.iter().any(|c| c == "empty_item_id"), "{codes:?}");
        assert!(
            codes.iter().any(|c| c == "empty_media_asset_id"),
            "{codes:?}"
        );
        assert!(
            codes.iter().any(|c| c == "kind_target_mismatch"),
            "{codes:?}"
        );

        // Bornes géométrie : yaw/pitch équirect, x/y flat, couleur.
        let mut doc = AnnotationDoc::default();
        let mut bad = item(
            "a1",
            ANNOTATION_KIND_DEVICE,
            AnnotationTarget::Device {
                device_id: "pac-01".into(),
            },
            AnnotationGeometry::Equirect {
                yaw: 181.0,
                pitch: -91.0,
            },
        );
        bad.color = Some("#zzzzzz".into());
        doc.items.push(bad);
        doc.items.push(item(
            "a2",
            ANNOTATION_KIND_NOTE,
            AnnotationTarget::Note { text: "x".into() },
            AnnotationGeometry::Flat { x: 1.5, y: -0.1 },
        ));
        let codes: Vec<String> = validate_annotation_doc(&doc)
            .into_iter()
            .map(|v| v.code)
            .collect();
        assert!(codes.iter().any(|c| c == "invalid_yaw"), "{codes:?}");
        assert!(codes.iter().any(|c| c == "invalid_pitch"), "{codes:?}");
        assert!(codes.iter().any(|c| c == "invalid_position"), "{codes:?}");
        assert!(codes.iter().any(|c| c == "invalid_color"), "{codes:?}");

        // NaN rejeté (non fini) — pas seulement hors bornes.
        let nan = f64::NAN;
        let mut doc = AnnotationDoc::default();
        doc.items.push(item(
            "a1",
            ANNOTATION_KIND_DEVICE,
            AnnotationTarget::Device {
                device_id: "pac-01".into(),
            },
            AnnotationGeometry::Equirect {
                yaw: nan,
                pitch: 0.0,
            },
        ));
        let codes: Vec<String> = validate_annotation_doc(&doc)
            .into_iter()
            .map(|v| v.code)
            .collect();
        assert!(codes.iter().any(|c| c == "invalid_yaw"));
    }

    #[test]
    fn kind_pour_geometrie() {
        assert!(geometry_requires_panorama(&AnnotationGeometry::Equirect {
            yaw: 0.0,
            pitch: 0.0
        }));
        assert!(!geometry_requires_panorama(&AnnotationGeometry::Flat {
            x: 0.0,
            y: 0.0
        }));
    }
}
