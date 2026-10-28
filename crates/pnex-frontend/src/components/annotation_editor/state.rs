//! State of the annotation editing panel (D59) — `AnnotationEditorCx`
//! (bundle of Copy signals, `TourEditorCx` approach) + tested pure reducers.
//! Invariants: `kind` derived from the target type (kind↔target consistency
//! guaranteed by construction); auto-incremented `a{n}` ids; equirect
//! geometry only in V1 (tour scenes are panoramas — D59).

use dioxus::prelude::*;
use pnex_core::{
    validate_annotation_doc, AnnotationDoc, AnnotationGeometry, AnnotationItem, AnnotationTarget,
    AnnotationViolation, ANNOTATION_KIND_DEVICE, ANNOTATION_KIND_NOTE, ANNOTATION_KIND_PIN,
    ANNOTATION_KIND_STATUS,
};

/// Next free item id (a{n}: 1 + max of the doc's numeric suffixes).
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

/// Summary item for the panel list.
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

/// Flat item rows (x/y ∈ [0,1]) — markers of the annotations page.
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

/// Flat summary item for the panel list.
#[derive(Clone)]
pub struct EditorItemRowFlat {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub x: f64,
    pub y: f64,
}

/// Places an item on a pano click (default: note) — returns the new id.
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

/// Places an item on a FLAT image (x/y ∈ [0,1] of the container) — UX
/// pivot v3: the annotations page also handles photos.
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

/// Repositions an equirect item (pano adjustment drag).
pub fn move_item(doc: &mut AnnotationDoc, item_id: &str, yaw: f64, pitch: f64) {
    for it in doc.items.iter_mut().filter(|it| it.id == item_id) {
        if let AnnotationGeometry::Equirect { yaw: gy, pitch: gp } = &mut it.geometry {
            *gy = yaw.clamp(-180.0, 180.0);
            *gp = pitch.clamp(-90.0, 90.0);
        }
    }
}

/// Repositions a flat item (drag on the image, x/y ∈ [0,1]) — used
/// by the drag of flat markers (annotations page).
pub fn move_item_flat(doc: &mut AnnotationDoc, item_id: &str, x: f64, y: f64) {
    for it in doc.items.iter_mut().filter(|it| it.id == item_id) {
        if let AnnotationGeometry::Flat { x: gx, y: gy } = &mut it.geometry {
            *gx = x.clamp(0.0, 1.0);
            *gy = y.clamp(0.0, 1.0);
        }
    }
}

/// Splat item row (world point) — markers of a splat media.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct EditorItemRowSplat {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// Rows of the splat items anchored on a media (document order).
pub fn item_rows_splat(doc: &AnnotationDoc, media_asset_id: &str) -> Vec<EditorItemRowSplat> {
    doc.items
        .iter()
        .filter(|it| it.media_asset_id == media_asset_id)
        .filter_map(|it| match &it.geometry {
            AnnotationGeometry::Splat { x, y, z } => Some(EditorItemRowSplat {
                id: it.id.clone(),
                kind: it.kind.clone(),
                label: it.label.clone(),
                x: *x,
                y: *y,
                z: *z,
            }),
            _ => None,
        })
        .collect()
}

/// Places a note on a splat at a picked world point — returns the new id.
pub fn place_item_splat(
    doc: &mut AnnotationDoc,
    media_asset_id: &str,
    x: f64,
    y: f64,
    z: f64,
) -> String {
    let id = next_item_id(doc);
    doc.items.push(AnnotationItem {
        id: id.clone(),
        media_asset_id: media_asset_id.to_string(),
        kind: ANNOTATION_KIND_NOTE.into(),
        geometry: AnnotationGeometry::Splat { x, y, z },
        color: None,
        label: String::new(),
        target: AnnotationTarget::Note {
            text: String::new(),
        },
    });
    id
}

/// Moves a splat item to a re-picked world point (marker drag).
pub fn move_item_splat(doc: &mut AnnotationDoc, item_id: &str, x: f64, y: f64, z: f64) {
    for it in doc.items.iter_mut().filter(|it| it.id == item_id) {
        if let AnnotationGeometry::Splat {
            x: gx,
            y: gy,
            z: gz,
        } = &mut it.geometry
        {
            *gx = x;
            *gy = y;
            *gz = z;
        }
    }
}

/// Changes the target of an item — the `kind` FOLLOWS the target type
/// (kind↔target consistency guaranteed by construction, D57).
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

/// Target type → kind (application string).
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

/// `true` if the item exists.
pub fn has_item(doc: &AnnotationDoc, item_id: &str) -> bool {
    doc.items.iter().any(|it| it.id == item_id)
}

/// Removes an item by id.
pub fn remove_item(doc: &mut AnnotationDoc, item_id: &str) {
    doc.items.retain(|it| it.id != item_id)
}

/// Signal bundle of the editing panel (TourEditorCx approach — all
/// Copy, passable to subcomponents).
#[derive(Clone, Copy, PartialEq)]
pub struct AnnotationEditorCx {
    /// Layer being edited (None = none).
    pub layer_id: Signal<Option<String>>,
    pub doc: Signal<AnnotationDoc>,
    pub saved_doc: Signal<AnnotationDoc>,
    pub saved_version: Signal<i64>,
    /// Selected item (id) for the inspector.
    pub selected: Signal<Option<String>>,
    /// Placement mode active (pano click → place_item).
    pub placing: Signal<bool>,
    pub violations: Signal<Vec<AnnotationViolation>>,
    pub saving: Signal<bool>,
    /// 409 conflict: server description (reload/overwrite modal).
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

    /// Doc mutation + immediate structural revalidation (the method
    /// takes `mut self`: Signal::set requires &mut — TourEditorCx approach).
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

/// D131: copies onto `local` the control ids the server provisioned for
/// the `control` items declaring their own control (nil id), matched by
/// item id. Every other local edit is kept.
pub fn adopt_bound_controls(local: &AnnotationDoc, server: &AnnotationDoc) -> AnnotationDoc {
    let mut out = local.clone();
    for item in &mut out.items {
        let AnnotationTarget::Control { control_id, .. } = &mut item.target else {
            continue;
        };
        if !control_id.is_nil() {
            continue;
        }
        let bound = server
            .items
            .iter()
            .find(|s| s.id == item.id)
            .and_then(|s| match &s.target {
                AnnotationTarget::Control { control_id, .. } if !control_id.is_nil() => {
                    Some(*control_id)
                }
                _ => None,
            });
        if let Some(id) = bound {
            *control_id = id;
        }
    }
    out
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
    fn save_adopts_provisioned_controls_only_for_own_items() {
        let own = |id: uuid::Uuid| AnnotationTarget::Control {
            control_id: id,
            kind: Some(pnex_core::ui_control::ControlKind::Switch),
        };
        let mut local = doc_with_one();
        local.items[0].target = own(uuid::Uuid::nil());
        let mut server = local.clone();
        server.items[0].target = own(uuid::Uuid::from_u128(3));
        local.items[0].label = "Edited while saving".into();
        let out = adopt_bound_controls(&local, &server);
        assert_eq!(out.items[0].target, own(uuid::Uuid::from_u128(3)));
        assert_eq!(out.items[0].label, "Edited while saving");
        // A linked control is never replaced.
        let mut linked = local.clone();
        linked.items[0].target = own(uuid::Uuid::from_u128(9));
        assert_eq!(adopt_bound_controls(&linked, &server), linked);
    }

    #[test]
    fn ids_auto_incremented_and_unique() {
        let mut doc = doc_with_one();
        let id1 = place_item(&mut doc, "asset-1", 20.0, 0.0);
        let id2 = place_item(&mut doc, "asset-1", 30.0, 5.0);
        assert_ne!(id1, id2);
        assert_eq!(next_item_id(&doc), format!("a{}", 4));
        // An exotic id does not break the increment.
        doc.items[0].id = "zz".into();
        assert_eq!(next_item_id(&doc), "a4");
    }

    #[test]
    fn place_and_reposition_clamped() {
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
    fn note_target_to_device() {
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
    fn flat_items_place_and_move_clamped() {
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
    fn delete_and_rows_filtered_by_media() {
        let mut doc = doc_with_one();
        place_item(&mut doc, "asset-2", 0.0, 0.0);
        let first = doc.items[0].id.clone();
        remove_item(&mut doc, &first);
        assert!(doc.items.iter().all(|it| it.media_asset_id != "asset-1"));
        assert_eq!(item_rows(&doc, "asset-2").len(), 1);
        assert!(!has_item(&doc, "a9"));
        assert!(has_item(&doc, &doc.items[0].id));
    }

    #[test]
    fn splat_items_place_move_and_list() {
        let mut doc = AnnotationDoc::default();
        let id = place_item_splat(&mut doc, "m1", 1.0, -2.0, 3.0);
        place_item(&mut doc, "m1", 10.0, 5.0);
        let rows = item_rows_splat(&doc, "m1");
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].x, rows[0].y, rows[0].z), (1.0, -2.0, 3.0));
        move_item_splat(&mut doc, &id, 0.5, 0.5, 0.5);
        assert_eq!(item_rows_splat(&doc, "m1")[0].z, 0.5);
        // The other geometries ignore a splat move.
        move_item_splat(&mut doc, "a2", 9.0, 9.0, 9.0);
        assert_eq!(item_rows(&doc, "m1")[0].yaw, 10.0);
        assert!(validate_annotation_doc(&doc).is_empty());
    }
}
