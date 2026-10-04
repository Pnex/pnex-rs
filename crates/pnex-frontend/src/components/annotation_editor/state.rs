//! État du panneau d'édition d'annotations (D59) — `AnnotationEditorCx`
//! (paquet de signaux Copy, école `TourEditorCx`) + réducteurs purs testés.
//! Invariants : `kind` dérivé du type de cible (cohérence kind↔cible
//! garantie par construction) ; ids `a{n}` auto-incrémentés ; géométrie
//! equirect uniquement en V1 (les scènes de tour sont des panoramas — D59).

use dioxus::prelude::*;
use pnex_core::{
    validate_annotation_doc, AnnotationDoc, AnnotationGeometry, AnnotationItem, AnnotationTarget,
    AnnotationViolation, ANNOTATION_KIND_DEVICE, ANNOTATION_KIND_NOTE, ANNOTATION_KIND_PIN,
    ANNOTATION_KIND_STATUS,
};

/// Prochain id d'item libre (a{n} : 1 + max des suffixes numériques du doc).
pub fn next_item_id(doc: &AnnotationDoc) -> String {
    let mut max = 0u32;
    for it in &doc.items {
        if let Some(suffix) = it.id.strip_prefix('a') {
            if let Ok(n) = suffix.parse::<u32>() {
                max = max.max(n);
            }
        }
    }
    format!("a{}", max + 1)
}

/// Ids des items ancrés sur un média (ordre du doc).
#[allow(dead_code)]
pub fn item_ids_for_asset(doc: &AnnotationDoc, media_asset_id: &str) -> Vec<String> {
    doc.items
        .iter()
        .filter(|it| it.media_asset_id == media_asset_id)
        .map(|it| it.id.clone())
        .collect()
}

/// Item résumé pour la liste du panneau.
#[derive(Clone)]
pub struct EditorItemRow {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub yaw: f64,
    pub pitch: f64,
}

pub fn item_rows(doc: &AnnotationDoc, media_asset_id: &str) -> Vec<EditorItemRow> {
    doc.items
        .iter()
        .filter(|it| it.media_asset_id == media_asset_id)
        .filter_map(|it| match &it.geometry {
            AnnotationGeometry::Equirect { yaw, pitch } => Some(EditorItemRow {
                id: it.id.clone(),
                kind: it.kind.clone(),
                label: it.label.clone(),
                yaw: *yaw,
                pitch: *pitch,
            }),
            _ => None,
        })
        .collect()
}

/// Rows d'items plats (x/y ∈ [0,1]) — marqueurs de la page annotations.
pub fn item_rows_flat(doc: &AnnotationDoc, media_asset_id: &str) -> Vec<EditorItemRowFlat> {
    doc.items
        .iter()
        .filter(|it| it.media_asset_id == media_asset_id)
        .filter_map(|it| match &it.geometry {
            AnnotationGeometry::Flat { x, y } => Some(EditorItemRowFlat {
                id: it.id.clone(),
                kind: it.kind.clone(),
                label: it.label.clone(),
                x: *x,
                y: *y,
            }),
            _ => None,
        })
        .collect()
}

/// Item résumé plat pour la liste du panneau.
#[derive(Clone)]
pub struct EditorItemRowFlat {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub x: f64,
    pub y: f64,
}

/// Pose un item au clic pano (défaut : note) — retourne le nouvel id.
pub fn place_item(doc: &mut AnnotationDoc, media_asset_id: &str, yaw: f64, pitch: f64) -> String {
    let id = next_item_id(doc);
    doc.items.push(AnnotationItem {
        id: id.clone(),
        media_asset_id: media_asset_id.to_string(),
        kind: ANNOTATION_KIND_NOTE.into(),
        geometry: AnnotationGeometry::Equirect {
            yaw: yaw.clamp(-180.0, 180.0),
            pitch: pitch.clamp(-90.0, 90.0),
        },
        color: None,
        label: String::new(),
        target: AnnotationTarget::Note {
            text: String::new(),
        },
    });
    id
}

/// Pose un item sur une image PLATE (x/y ∈ [0,1] du conteneur) — pivot
/// UX v3 : la page annotations traite aussi les photos.
pub fn place_item_flat(doc: &mut AnnotationDoc, media_asset_id: &str, x: f64, y: f64) -> String {
    let id = next_item_id(doc);
    doc.items.push(AnnotationItem {
        id: id.clone(),
        media_asset_id: media_asset_id.to_string(),
        kind: ANNOTATION_KIND_NOTE.into(),
        geometry: AnnotationGeometry::Flat {
            x: x.clamp(0.0, 1.0),
            y: y.clamp(0.0, 1.0),
        },
        color: None,
        label: String::new(),
        target: AnnotationTarget::Note {
            text: String::new(),
        },
    });
    id
}

/// Repositionne un item équirect (drag d'ajustement pano).
pub fn move_item(doc: &mut AnnotationDoc, item_id: &str, yaw: f64, pitch: f64) {
    for it in doc.items.iter_mut().filter(|it| it.id == item_id) {
        if let AnnotationGeometry::Equirect { yaw: gy, pitch: gp } = &mut it.geometry {
            *gy = yaw.clamp(-180.0, 180.0);
            *gp = pitch.clamp(-90.0, 90.0);
        }
    }
}

/// Repositionne un item plat (drag sur l'image, x/y ∈ [0,1]) — consommé
/// par le drag des marqueurs plats (page annotations).
pub fn move_item_flat(doc: &mut AnnotationDoc, item_id: &str, x: f64, y: f64) {
    for it in doc.items.iter_mut().filter(|it| it.id == item_id) {
        if let AnnotationGeometry::Flat { x: gx, y: gy } = &mut it.geometry {
            *gx = x.clamp(0.0, 1.0);
            *gy = y.clamp(0.0, 1.0);
        }
    }
}

/// Change la cible d'un item — le `kind` SUIT le type de cible
/// (cohérence kind↔cible garantie par construction, D57).
pub fn set_item_target(doc: &mut AnnotationDoc, item_id: &str, target: AnnotationTarget) {
    let kind = target_kind(&target);
    for it in doc.items.iter_mut().filter(|it| it.id == item_id) {
        it.target = target.clone();
        it.kind = kind.to_string();
    }
}

pub fn set_item_label(doc: &mut AnnotationDoc, item_id: &str, label: String) {
    if let Some(it) = doc.items.iter_mut().find(|it| it.id == item_id) {
        it.label = label;
    }
}

pub fn set_item_color(doc: &mut AnnotationDoc, item_id: &str, color: Option<String>) {
    if let Some(it) = doc.items.iter_mut().find(|it| it.id == item_id) {
        it.color = color;
    }
}

/// Type de cible → kind (string applicatif).
pub fn target_kind(target: &AnnotationTarget) -> &'static str {
    match target {
        AnnotationTarget::Device { .. } => ANNOTATION_KIND_DEVICE,
        AnnotationTarget::Pin { .. } => ANNOTATION_KIND_PIN,
        AnnotationTarget::Status { .. } => ANNOTATION_KIND_STATUS,
        AnnotationTarget::Note { .. } => ANNOTATION_KIND_NOTE,
        AnnotationTarget::Control { .. } => pnex_core::ANNOTATION_KIND_CONTROL,
        AnnotationTarget::Reading { .. } => pnex_core::ANNOTATION_KIND_READING,
    }
}

/// `true` si l'item existe.
pub fn has_item(doc: &AnnotationDoc, item_id: &str) -> bool {
    doc.items.iter().any(|it| it.id == item_id)
}

/// Enlève un item par id.
pub fn remove_item(doc: &mut AnnotationDoc, item_id: &str) {
    doc.items.retain(|it| it.id != item_id)
}

/// Paquet de signaux du panneau d'édition (école TourEditorCx — tous
/// Copy, passables aux sous-composants).
#[derive(Clone, Copy, PartialEq)]
pub struct AnnotationEditorCx {
    /// Couche en cours d'édition (None = aucune).
    pub layer_id: Signal<Option<String>>,
    pub doc: Signal<AnnotationDoc>,
    pub saved_doc: Signal<AnnotationDoc>,
    pub saved_version: Signal<i64>,
    /// Item sélectionné (id) pour l'inspecteur.
    pub selected: Signal<Option<String>>,
    /// Mode pose actif (clic pano → place_item).
    pub placing: Signal<bool>,
    pub violations: Signal<Vec<AnnotationViolation>>,
    pub saving: Signal<bool>,
    /// Conflit 409 : description serveur (modal recharger/écraser).
    pub conflict: Signal<Option<String>>,
}

impl AnnotationEditorCx {
    pub fn new() -> Self {
        Self {
            layer_id: use_signal(|| None),
            doc: use_signal(AnnotationDoc::default),
            saved_doc: use_signal(AnnotationDoc::default),
            saved_version: use_signal(|| 0),
            selected: use_signal(|| None),
            placing: use_signal(|| false),
            violations: use_signal(Vec::new),
            saving: use_signal(|| false),
            conflict: use_signal(|| None),
        }
    }

    /// Mutation du doc + revalidation structurelle immédiate (la méthode
    /// prend `mut self` : Signal::set exige &mut — école TourEditorCx).
    pub fn update_doc(mut self, f: impl FnOnce(&mut AnnotationDoc)) {
        let mut doc = self.doc.cloned();
        f(&mut doc);
        self.violations.set(validate_annotation_doc(&doc));
        self.doc.set(doc);
    }

    pub fn is_dirty(&self) -> bool {
        self.doc.cloned() != self.saved_doc.cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_with_one() -> AnnotationDoc {
        let mut doc = AnnotationDoc::default();
        place_item(&mut doc, "asset-1", 10.0, -5.0);
        doc
    }

    #[test]
    fn ids_auto_incrementes_et_uniques() {
        let mut doc = doc_with_one();
        let id1 = place_item(&mut doc, "asset-1", 20.0, 0.0);
        let id2 = place_item(&mut doc, "asset-1", 30.0, 5.0);
        assert_ne!(id1, id2);
        assert_eq!(next_item_id(&doc), format!("a{}", 4));
        // Un id exotique ne casse pas l'incrément.
        doc.items[0].id = "zz".into();
        assert_eq!(next_item_id(&doc), "a4");
    }

    #[test]
    fn pose_et_repositionne_clampes() {
        let mut doc = doc_with_one();
        let id = place_item(&mut doc, "asset-1", 999.0, -400.0);
        let it = doc.items.iter().find(|it| it.id == id).unwrap();
        assert!(matches!(
            it.geometry,
            AnnotationGeometry::Equirect {
                yaw: 180.0,
                pitch: -90.0
            }
        ));
        move_item(&mut doc, &id, -999.0, 500.0);
        let it = doc.items.iter().find(|it| it.id == id).unwrap();
        assert!(matches!(
            it.geometry,
            AnnotationGeometry::Equirect {
                yaw: -180.0,
                pitch: 90.0
            }
        ));
        move_item(&mut doc, &id, 45.0, -10.0);
        let it = doc.items.iter().find(|it| it.id == id).unwrap();
        assert!(matches!(
            it.geometry,
            AnnotationGeometry::Equirect {
                yaw: 45.0,
                pitch: -10.0
            }
        ));
    }

    #[test]
    fn cible_note_vers_device() {
        let mut doc = doc_with_one();
        let id = doc.items[0].id.clone();
        set_item_target(
            &mut doc,
            &id,
            AnnotationTarget::Device {
                device_id: "pac-01".into(),
            },
        );
        let it = doc.items.iter().find(|it| it.id == id).unwrap();
        assert_eq!(it.kind, "device");
        assert!(validate_annotation_doc(&doc).is_empty());
    }

    #[test]
    fn items_plats_place_et_deplace_clampes() {
        let mut doc = AnnotationDoc::default();
        let id = place_item_flat(&mut doc, "asset-p", 1.5, -0.2);
        let it = doc.items.iter().find(|it| it.id == id).unwrap();
        assert!(matches!(
            it.geometry,
            AnnotationGeometry::Flat { x: 1.0, y: 0.0 }
        ));
        assert_eq!(it.kind, "note");
        move_item_flat(&mut doc, &id, 0.25, 0.75);
        let it = doc.items.iter().find(|it| it.id == id).unwrap();
        assert!(matches!(
            it.geometry,
            AnnotationGeometry::Flat { x: 0.25, y: 0.75 }
        ));
        let rows = item_rows_flat(&doc, "asset-p");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].x, 0.25);
    }

    #[test]
    fn suppression_et_rows_filtrees_par_media() {
        let mut doc = doc_with_one();
        place_item(&mut doc, "asset-2", 0.0, 0.0);
        let first = doc.items[0].id.clone();
        remove_item(&mut doc, &first);
        assert!(doc.items.iter().all(|it| it.media_asset_id != "asset-1"));
        assert_eq!(item_rows(&doc, "asset-2").len(), 1);
        assert!(item_ids_for_asset(&doc, "asset-1").is_empty());
        assert!(!has_item(&doc, "a9"));
        assert!(has_item(&doc, &doc.items[0].id));
    }
}
