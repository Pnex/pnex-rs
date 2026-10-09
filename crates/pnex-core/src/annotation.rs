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
/// Operates an org control (D128: an annotation never references a pin to
/// write; the flows listening to the control act).
pub const ANNOTATION_KIND_CONTROL: &str = "control";
/// Live reading of a telemetry series or an org memory value, optionally
/// with a sparkline (D129).
pub const ANNOTATION_KIND_READING: &str = "reading";

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
    /// Gaussian splat: a point in the scene's world coordinates (any finite
    /// value — the scale is the capture's own).
    Splat { x: f64, y: f64, z: f64 },
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
            "splat" => {
                let axis = |name: &str| {
                    v.get(name).and_then(|n| n.as_f64()).ok_or_else(|| {
                        serde::de::Error::custom(format!("splat requires numeric `{name}`"))
                    })
                };
                Ok(Self::Splat {
                    x: axis("x")?,
                    y: axis("y")?,
                    z: axis("z")?,
                })
            }
            other => Err(serde::de::Error::custom(format!(
                "unknown geometry type: {other}"
            ))),
        }
    }
}

/// Cible faible d'un item (D57) — résolue **au read, en batch** (école
/// `link_target_labels`) ; référence morte tolérée (`dead: true`, jamais 500).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AnnotationTarget {
    Device {
        device_id: String,
    },
    Pin {
        device_id: String,
        pin_gpio: u32,
    },
    Status {
        device_id: String,
    },
    Note {
        text: String,
    },
    /// Org control operated from the annotation (D125, D128). A nil
    /// `control_id` declares the item's own control: the server provisions
    /// one of `kind` when the layer is saved (D131).
    Control {
        control_id: uuid::Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<crate::ui_control::ControlKind>,
    },
    /// Same read binding as the dashboard widgets (D129).
    Reading {
        source: crate::viz::SourceRef,
        /// Mini chart drawn for the value: `stat`, `line`, `gauge` or
        /// `indicator` (`READING_DISPLAYS`). Absent = `stat`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        /// Gauge range (dashboard default 0..100 when absent).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<f64>,
    },
}

/// Mini charts a reading item can draw (dashboard widget types).
pub const READING_DISPLAYS: [&str; 4] = ["stat", "line", "gauge", "indicator"];

/// Effective mini chart of a reading: the explicit `display`, else a
/// plain value (`stat`).
pub fn reading_display(display: Option<&str>) -> &str {
    display.unwrap_or("stat")
}

// Manual `Deserialize` (same reason as `AnnotationGeometry`): the derived
// internally-tagged form buffers through serde's `Content`, which breaks
// numeric fields (`pin_gpio`, `min`, `max`) under `arbitrary_precision`.
// The tag is matched by hand and each variant is read from a plain struct.
impl<'de> Deserialize<'de> for AnnotationTarget {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct DeviceRepr {
            device_id: String,
        }
        #[derive(Deserialize)]
        struct PinRepr {
            device_id: String,
            pin_gpio: u32,
        }
        #[derive(Deserialize)]
        struct NoteRepr {
            text: String,
        }
        #[derive(Deserialize)]
        struct ControlRepr {
            control_id: uuid::Uuid,
            #[serde(default)]
            kind: Option<crate::ui_control::ControlKind>,
        }
        #[derive(Deserialize)]
        struct ReadingRepr {
            source: crate::viz::SourceRef,
            #[serde(default)]
            display: Option<String>,
            #[serde(default)]
            min: Option<f64>,
            #[serde(default)]
            max: Option<f64>,
        }
        fn part<T: serde::de::DeserializeOwned, E: serde::de::Error>(
            v: serde_json::Value,
        ) -> Result<T, E> {
            serde_json::from_value(v).map_err(E::custom)
        }

        let v = serde_json::Value::deserialize(d)?;
        let tag = v
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        match tag.as_str() {
            "device" => part::<DeviceRepr, D::Error>(v).map(|r| Self::Device {
                device_id: r.device_id,
            }),
            "pin" => part::<PinRepr, D::Error>(v).map(|r| Self::Pin {
                device_id: r.device_id,
                pin_gpio: r.pin_gpio,
            }),
            "status" => part::<DeviceRepr, D::Error>(v).map(|r| Self::Status {
                device_id: r.device_id,
            }),
            "note" => part::<NoteRepr, D::Error>(v).map(|r| Self::Note { text: r.text }),
            "control" => part::<ControlRepr, D::Error>(v).map(|r| Self::Control {
                control_id: r.control_id,
                kind: r.kind,
            }),
            "reading" => part::<ReadingRepr, D::Error>(v).map(|r| Self::Reading {
                source: r.source,
                display: r.display,
                min: r.min,
                max: r.max,
            }),
            other => Err(serde::de::Error::custom(format!(
                "unknown target type: {other}"
            ))),
        }
    }
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
        AnnotationTarget::Control { .. } => kind == ANNOTATION_KIND_CONTROL,
        AnnotationTarget::Reading { .. } => kind == ANNOTATION_KIND_READING,
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
            AnnotationGeometry::Splat { x, y, z } => {
                if !x.is_finite() || !y.is_finite() || !z.is_finite() {
                    v.push(violation(
                        Some(&item.id),
                        "invalid_position",
                        format!("item {} : position must be finite", item.id),
                    ));
                }
            }
        }
        match &item.target {
            AnnotationTarget::Control { control_id, kind } => {
                if control_id.is_nil() && kind.is_none() {
                    v.push(violation(
                        Some(&item.id),
                        "control_unset",
                        format!("item {} : pick a control or a control kind", item.id),
                    ));
                }
                if item.id.chars().count() > crate::ui_control::CONTROL_ORIGIN_ITEM_MAX_LEN {
                    v.push(violation(
                        Some(&item.id),
                        "item_id_too_long",
                        format!(
                            "item id longer than {} characters",
                            crate::ui_control::CONTROL_ORIGIN_ITEM_MAX_LEN
                        ),
                    ));
                }
            }
            AnnotationTarget::Reading {
                source,
                display,
                min,
                max,
            } => {
                let shape = reading_display(display.as_deref());
                if !READING_DISPLAYS.contains(&shape) {
                    v.push(violation(
                        Some(&item.id),
                        "invalid_display",
                        format!("item {} : unknown mini chart `{shape}`", item.id),
                    ));
                }
                if let (Some(lo), Some(hi)) = (min, max) {
                    if !lo.is_finite() || !hi.is_finite() || lo >= hi {
                        v.push(violation(
                            Some(&item.id),
                            "invalid_range",
                            format!("item {} : gauge min must be below max", item.id),
                        ));
                    }
                }
                crate::viz::check_sources(shape, std::slice::from_ref(source), &mut |code, msg| {
                    v.push(violation(
                        Some(&item.id),
                        code,
                        format!("item {} : {msg}", item.id),
                    ));
                });
            }
            _ => {}
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

/// Media kinds an item of this geometry can anchor on: sphere → `panorama`,
/// flat image → `photo`/`floorplan` (same list as the tour floor plans),
/// splat point → `splat`.
pub fn geometry_media_kinds(g: &AnnotationGeometry) -> &'static [&'static str] {
    match g {
        AnnotationGeometry::Equirect { .. } => &["panorama"],
        AnnotationGeometry::Flat { .. } => &["photo", "floorplan"],
        AnnotationGeometry::Splat { .. } => &["splat"],
    }
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
    fn control_and_reading_items_validate_and_round_trip() {
        let geo = || AnnotationGeometry::Equirect {
            yaw: 10.0,
            pitch: -5.0,
        };
        let reading = |metric: &str, line: bool| AnnotationTarget::Reading {
            source: crate::viz::SourceRef {
                role: "primary".into(),
                metric: metric.into(),
                device_id: "proud-ibex".into(),
                window: "1h".into(),
                memory: None,
            },
            display: line.then(|| "line".to_string()),
            min: None,
            max: None,
        };
        let doc = AnnotationDoc {
            items: vec![
                item(
                    "c1",
                    ANNOTATION_KIND_CONTROL,
                    AnnotationTarget::Control {
                        control_id: uuid::Uuid::from_u128(7),
                        kind: None,
                    },
                    geo(),
                ),
                // Own control declared by kind, provisioned at save (D131).
                item(
                    "c0",
                    ANNOTATION_KIND_CONTROL,
                    AnnotationTarget::Control {
                        control_id: uuid::Uuid::nil(),
                        kind: Some(crate::ui_control::ControlKind::Slider),
                    },
                    geo(),
                ),
                item(
                    "r1",
                    ANNOTATION_KIND_READING,
                    reading("temperature", true),
                    geo(),
                ),
            ],
        };
        assert!(validate_annotation_doc(&doc).is_empty());
        let json = serde_json::to_value(&doc).unwrap();
        assert_eq!(json["items"][2]["target"]["type"], "reading");
        assert_eq!(json["items"][1]["target"]["kind"], "slider");
        assert!(json["items"][0]["target"].get("kind").is_none());
        let back: AnnotationDoc = serde_json::from_value(json).unwrap();
        assert_eq!(back, doc);

        let bad = AnnotationDoc {
            items: vec![
                item(
                    "c2",
                    ANNOTATION_KIND_CONTROL,
                    AnnotationTarget::Control {
                        control_id: uuid::Uuid::nil(),
                        kind: None,
                    },
                    geo(),
                ),
                item(
                    "r2",
                    ANNOTATION_KIND_READING,
                    reading("bad metric!", false),
                    geo(),
                ),
                item(
                    "r3",
                    ANNOTATION_KIND_CONTROL,
                    reading("temperature", false),
                    geo(),
                ),
            ],
        };
        let codes: Vec<String> = validate_annotation_doc(&bad)
            .into_iter()
            .map(|v| v.code)
            .collect();
        assert!(codes.contains(&"control_unset".to_string()), "{codes:?}");
        assert!(codes.contains(&"bad_metric".to_string()), "{codes:?}");
        assert!(
            codes.contains(&"kind_target_mismatch".to_string()),
            "{codes:?}"
        );
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

    #[test]
    fn media_kinds_per_geometry() {
        let pano = AnnotationGeometry::Equirect {
            yaw: 0.0,
            pitch: 0.0,
        };
        let flat = AnnotationGeometry::Flat { x: 0.5, y: 0.5 };
        let splat = AnnotationGeometry::Splat {
            x: 1.0,
            y: -2.0,
            z: 3.5,
        };
        assert_eq!(geometry_media_kinds(&pano), &["panorama"]);
        assert_eq!(geometry_media_kinds(&flat), &["photo", "floorplan"]);
        assert_eq!(geometry_media_kinds(&splat), &["splat"]);
    }

    #[test]
    fn splat_geometry_round_trips_and_rejects_non_finite() {
        let json = serde_json::json!({
            "items": [{
                "id": "s1", "media_asset_id": "m", "kind": "note",
                "geometry": {"type": "splat", "x": 0.25, "y": -1.5, "z": 12.0},
                "target": {"type": "note", "text": "valve"}
            }]
        });
        let doc: AnnotationDoc = serde_json::from_value(json).unwrap();
        assert_eq!(
            doc.items[0].geometry,
            AnnotationGeometry::Splat {
                x: 0.25,
                y: -1.5,
                z: 12.0
            }
        );
        assert!(validate_annotation_doc(&doc).is_empty());
        let back: AnnotationDoc =
            serde_json::from_value(serde_json::to_value(&doc).unwrap()).unwrap();
        assert_eq!(back, doc);

        let missing_z = serde_json::json!({"type": "splat", "x": 1.0, "y": 2.0});
        assert!(serde_json::from_value::<AnnotationGeometry>(missing_z).is_err());

        let mut bad = doc.clone();
        bad.items[0].geometry = AnnotationGeometry::Splat {
            x: f64::NAN,
            y: 0.0,
            z: 0.0,
        };
        let codes: Vec<String> = validate_annotation_doc(&bad)
            .into_iter()
            .map(|v| v.code)
            .collect();
        assert!(codes.contains(&"invalid_position".to_string()));
    }

    #[test]
    fn reading_display_rules() {
        assert_eq!(reading_display(None), "stat");
        assert_eq!(reading_display(Some("gauge")), "gauge");

        // Unset optional fields are omitted.
        let plain = serde_json::json!({
            "type": "reading",
            "source": {"metric": "temp", "device_id": "d1", "window": "1h"}
        });
        let t: AnnotationTarget = serde_json::from_value(plain).unwrap();
        let out = serde_json::to_value(&t).unwrap();
        assert!(out.get("display").is_none() && out.get("min").is_none());

        let gauge = |display: &str, min: Option<f64>, max: Option<f64>| AnnotationDoc {
            items: vec![item(
                "g1",
                ANNOTATION_KIND_READING,
                AnnotationTarget::Reading {
                    source: crate::viz::SourceRef {
                        role: "primary".into(),
                        metric: "temperature".into(),
                        device_id: "proud-ibex".into(),
                        window: "1h".into(),
                        memory: None,
                    },
                    display: Some(display.into()),
                    min,
                    max,
                },
                AnnotationGeometry::Flat { x: 0.1, y: 0.2 },
            )],
        };
        let ok = gauge("gauge", Some(0.0), Some(120.0));
        assert!(validate_annotation_doc(&ok).is_empty());
        let back: AnnotationDoc =
            serde_json::from_value(serde_json::to_value(&ok).unwrap()).unwrap();
        assert_eq!(back, ok);

        let codes = |d: &AnnotationDoc| -> Vec<String> {
            validate_annotation_doc(d)
                .into_iter()
                .map(|v| v.code)
                .collect()
        };
        assert!(codes(&gauge("pie", None, None)).contains(&"invalid_display".to_string()));
        assert!(codes(&gauge("gauge", Some(5.0), Some(5.0))).contains(&"invalid_range".to_string()));
    }
}
