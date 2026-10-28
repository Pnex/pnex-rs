//! State and pure reducers of the tour editor — every mutation of the
//! document goes through these pure functions (tested), never inline in
//! the RSX (`flow_editor/state.rs` style).

use pnex_core::{FloorPlan, TourDoc, TourFloor, TourLink};

/// Next free id of a collection: `{prefix}{max(suffixes)+1}` — never
/// a collision even after several additions/deletions (`next_node_id`
/// style). `taken` = existing ids of the collection.
pub fn next_id<'a>(taken: impl Iterator<Item = &'a str>, prefix: &str) -> String {
    let mut max = 0u32;
    for id in taken {
        if let Some(n) = id.strip_prefix(prefix) {
            if let Ok(value) = n.parse::<u32>() {
                max = max.max(value);
            }
        }
    }
    format!("{prefix}{}", max + 1)
}

pub fn next_floor_id(doc: &TourDoc) -> String {
    next_id(doc.floors.iter().map(|f| f.id.as_str()), "f")
}

pub fn next_scene_id(doc: &TourDoc) -> String {
    next_id(doc.scenes.iter().map(|s| s.id.as_str()), "s")
}

pub fn next_link_id(doc: &TourDoc) -> String {
    next_id(doc.links.iter().map(|l| l.id.as_str()), "l")
}

/// Adds a floor (name/level provided by the UI) — returns the created id.
pub fn add_floor(doc: &mut TourDoc, name: String, level: i32) -> String {
    let id = next_floor_id(doc);
    doc.floors.push(TourFloor {
        id: id.clone(),
        name,
        level,
        plan: None,
        north_deg: 0.0,
    });
    id
}

/// Sets/removes the plan of a floor — no-op if the floor is unknown.
pub fn set_floor_plan(doc: &mut TourDoc, floor_id: &str, plan: Option<FloorPlan>) {
    if let Some(floor) = doc.floors.iter_mut().find(|f| f.id == floor_id) {
        floor.plan = plan;
    }
}

/// Patches the plan of a floor (dimensions/scale/removal) — no-op if
/// the floor (or its plan) is unknown.
pub fn patch_plan(doc: &mut TourDoc, floor_id: &str, f: impl FnOnce(&mut FloorPlan)) {
    if let Some(floor) = doc.floors.iter_mut().find(|f| f.id == floor_id) {
        if let Some(plan) = &mut floor.plan {
            f(plan);
        }
    }
}

/// Deletes a floor — **refused** if it carries scenes (the UI asks to
/// delete them first). Returns `true` if deleted.
pub fn remove_floor(doc: &mut TourDoc, floor_id: &str) -> bool {
    let occupied = doc.scenes.iter().any(|s| s.floor_id == floor_id);
    if occupied {
        return false;
    }
    let before = doc.floors.len();
    doc.floors.retain(|f| f.id != floor_id);
    doc.floors.len() != before
}

/// Places a scene (panorama asset + plan position) and **auto-links**: a pair
/// of round-trip links to the closest scene on the same floor, if
/// there is one (connected tour by default — stays editable/deletable
/// like any link). Returns the created id.
pub fn add_scene(
    doc: &mut TourDoc,
    floor_id: &str,
    media_asset_id: String,
    pos: (f64, f64),
) -> String {
    let id = next_scene_id(doc);
    doc.scenes.push(pnex_core::TourScene {
        id: id.clone(),
        floor_id: floor_id.to_string(),
        label: String::new(),
        media_asset_id,
        x: pos.0,
        y: pos.1,
        initial_yaw: 0.0,
        initial_pitch: 0.0,
        initial_fov: 100.0,
    });
    // Nearest neighbour on the SAME floor (distance² on the plan; the order
    // of equal distances is stable = first in the doc).
    let nearest = doc
        .scenes
        .iter()
        .filter(|s| s.floor_id == floor_id && s.id != id)
        .min_by(|a, b| {
            let da = (a.x - pos.0).powi(2) + (a.y - pos.1).powi(2);
            let db = (b.x - pos.0).powi(2) + (b.y - pos.1).powi(2);
            da.total_cmp(&db)
        })
        .map(|s| s.id.clone());
    if let Some(nearest) = nearest {
        connect_scenes(doc, &id, &nearest);
    }
    id
}

/// Moves a scene (drag) — no-op if the id is unknown.
pub fn move_scene(doc: &mut TourDoc, id: &str, pos: (f64, f64)) {
    if let Some(scene) = doc.scenes.iter_mut().find(|s| s.id == id) {
        scene.x = pos.0;
        scene.y = pos.1;
    }
}

/// Inspector patches — no-op if the id is unknown.
pub fn set_scene_label(doc: &mut TourDoc, id: &str, label: String) {
    if let Some(scene) = doc.scenes.iter_mut().find(|s| s.id == id) {
        scene.label = label;
    }
}

pub fn set_scene_media(doc: &mut TourDoc, id: &str, media_asset_id: String) {
    if let Some(scene) = doc.scenes.iter_mut().find(|s| s.id == id) {
        scene.media_asset_id = media_asset_id;
    }
}

pub fn set_scene_view(doc: &mut TourDoc, id: &str, yaw: f64, pitch: f64, fov: f64) {
    if let Some(scene) = doc.scenes.iter_mut().find(|s| s.id == id) {
        scene.initial_yaw = yaw;
        scene.initial_pitch = pitch;
        scene.initial_fov = fov;
    }
}

/// Deletes a scene **and** every link touching it.
pub fn remove_scene(doc: &mut TourDoc, id: &str) {
    doc.scenes.retain(|s| s.id != id);
    doc.links.retain(|l| l.from != id && l.to != id);
    if doc.start_scene.as_deref() == Some(id) {
        doc.start_scene = None;
    }
}

/// Link from one scene to another: `kind` **derived** from the floors of the
/// ends (`walk` same floor, `stair` otherwise). Refused (→ `None`):
/// self-link, duplicate (from,to), unknown scene.
pub fn add_link(doc: &mut TourDoc, from: &str, to: &str, yaw: f64, pitch: f64) -> Option<String> {
    if from == to {
        return None;
    }
    if doc.links.iter().any(|l| l.from == from && l.to == to) {
        return None;
    }
    let floor_of = |doc: &TourDoc, scene_id: &str| {
        doc.scenes
            .iter()
            .find(|s| s.id == scene_id)
            .map(|s| s.floor_id.clone())
    };
    let (Some(from_floor), Some(to_floor)) = (floor_of(doc, from), floor_of(doc, to)) else {
        return None;
    };
    let kind = if from_floor == to_floor {
        pnex_core::TOUR_LINK_WALK
    } else {
        pnex_core::TOUR_LINK_STAIR
    };
    let id = next_link_id(doc);
    doc.links.push(TourLink {
        id: id.clone(),
        from: from.to_string(),
        to: to.to_string(),
        yaw,
        pitch,
        kind: kind.to_string(),
        label: None,
    });
    Some(id)
}

pub fn set_link_label(doc: &mut TourDoc, id: &str, label: Option<String>) {
    if let Some(link) = doc.links.iter_mut().find(|l| l.id == id) {
        link.label = label.filter(|l| !l.trim().is_empty());
    }
}

/// Repositions a hotspot (drag of the arrow in preview) — no-op if the
/// link is unknown. Yaw clamped to ±180°, pitch to ±90° (sphere).
pub fn set_link_angles(doc: &mut TourDoc, id: &str, yaw: f64, pitch: f64) {
    if let Some(link) = doc.links.iter_mut().find(|l| l.id == id) {
        link.yaw = yaw.clamp(-180.0, 180.0);
        link.pitch = pitch.clamp(-90.0, 90.0);
    }
}

/// Yaw of the A→B hotspot: direction on the plan in ENU degrees (x = east,
/// plan y downwards → negation, `scene_pointer_down` style).
fn yaw_between(a: (f64, f64), b: (f64, f64)) -> f64 {
    (b.0 - a.0).atan2(-(b.1 - a.1)).to_degrees()
}

/// Hotspot pitch from the levels of the ends: going down = arrow
/// towards the floor (-30°), going up = towards the ceiling (+30°), level = 0°.
const STAIR_PITCH_DEG: f64 = 30.0;
fn pitch_between(from_level: i32, to_level: i32) -> f64 {
    match to_level.cmp(&from_level) {
        std::cmp::Ordering::Less => -STAIR_PITCH_DEG,
        std::cmp::Ordering::Greater => STAIR_PITCH_DEG,
        std::cmp::Ordering::Equal => 0.0,
    }
}

/// Connects two scenes: **round-trip pair** of links (a link = a
/// directional hotspot — the return is needed to come back), **automatic**
/// yaw and pitch — yaw = direction on the plan, pitch = ±30° if
/// the levels differ (stair: down/up), level otherwise (walk).
/// Returns the number of links created (0 = self-link, unknown scenes,
/// or both directions already existed — existing ones are kept,
/// never duplicated).
pub fn connect_scenes(doc: &mut TourDoc, a: &str, b: &str) -> usize {
    let pos_of =
        |doc: &TourDoc, id: &str| doc.scenes.iter().find(|s| s.id == id).map(|s| (s.x, s.y));
    let level_of = |doc: &TourDoc, id: &str| {
        let floor_id = doc.scenes.iter().find(|s| s.id == id)?.floor_id.clone();
        doc.floors
            .iter()
            .find(|f| f.id == floor_id)
            .map(|f| f.level)
    };
    let (Some(pa), Some(pb)) = (pos_of(doc, a), pos_of(doc, b)) else {
        return 0;
    };
    let (Some(la), Some(lb)) = (level_of(doc, a), level_of(doc, b)) else {
        return 0;
    };
    let mut created = 0;
    if add_link(doc, a, b, yaw_between(pa, pb), pitch_between(la, lb)).is_some() {
        created += 1;
    }
    if add_link(doc, b, a, yaw_between(pb, pa), pitch_between(lb, la)).is_some() {
        created += 1;
    }
    created
}

/// Deletes a link (or the half-link of a wire deleted in the UI).
pub fn remove_link(doc: &mut TourDoc, id: &str) {
    doc.links.retain(|l| l.id != id);
}

pub fn set_start_scene(doc: &mut TourDoc, id: Option<String>) {
    doc.start_scene = id;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_with_floor() -> TourDoc {
        TourDoc {
            floors: vec![TourFloor {
                id: "f1".into(),
                name: "RDC".into(),
                level: 0,
                plan: None,
                north_deg: 0.0,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn ids_without_collision_after_delete() {
        let mut doc = doc_with_floor();
        let s1 = add_scene(&mut doc, "f1", "a".into(), (0.0, 0.0));
        let s2 = add_scene(&mut doc, "f1", "b".into(), (10.0, 0.0));
        assert_eq!((s1.as_str(), s2.as_str()), ("s1", "s2"));
        remove_scene(&mut doc, "s2");
        // s2 deleted: reusable (next_node_id style).
        assert_eq!(add_scene(&mut doc, "f1", "c".into(), (0.0, 0.0)), "s2");
    }

    #[test]
    fn add_floor_and_delete_refused_with_scenes() {
        let mut doc = doc_with_floor();
        assert_eq!(add_floor(&mut doc, "1er".into(), 1), "f2");
        assert_eq!(doc.floors.len(), 2);

        add_scene(&mut doc, "f2", "a".into(), (0.0, 0.0));
        assert!(!remove_floor(&mut doc, "f2"), "occupied floor: refused");
        assert_eq!(doc.floors.len(), 2);
        remove_scene(&mut doc, "s1");
        assert!(remove_floor(&mut doc, "f2"), "empty floor: deleted");
    }

    #[test]
    fn scene_delete_purges_links_and_start() {
        let mut doc = doc_with_floor();
        add_scene(&mut doc, "f1", "a".into(), (0.0, 0.0));
        add_scene(&mut doc, "f1", "b".into(), (10.0, 0.0));
        // Auto-link: the 2nd scene is linked automatically to the 1st (round trip).
        assert_eq!(doc.links.len(), 2);
        set_start_scene(&mut doc, Some("s1".into()));
        remove_scene(&mut doc, "s1");
        assert!(doc.links.is_empty(), "touched links purged");
        assert_eq!(doc.start_scene, None, "start dropped: reset");
        assert_eq!(doc.scenes.len(), 1);
    }

    #[test]
    fn links_refusal_and_derived_kind() {
        let mut doc = doc_with_floor();
        assert_eq!(add_floor(&mut doc, "1er".into(), 1), "f2");
        add_scene(&mut doc, "f1", "a".into(), (0.0, 0.0)); // s1 @ f1
        add_scene(&mut doc, "f1", "b".into(), (10.0, 0.0)); // s2 @ f1 — auto-linked to s1
        add_scene(&mut doc, "f2", "c".into(), (0.0, 0.0)); // s3 @ f2 — isolated (no scene on f2)

        assert_eq!(doc.links.len(), 2, "auto-link s1↔s2 only");
        assert!(doc
            .links
            .iter()
            .all(|l| l.kind == pnex_core::TOUR_LINK_WALK));

        // Derived kind: different floors → stair (manual add).
        assert!(add_link(&mut doc, "s2", "s3", 0.0, 0.0).is_some());
        assert_eq!(doc.links[2].kind, pnex_core::TOUR_LINK_STAIR);

        // Refused: self-link, duplicate (already auto-linked), unknown scene.
        assert_eq!(add_link(&mut doc, "s1", "s1", 0.0, 0.0), None);
        assert_eq!(add_link(&mut doc, "s1", "s2", 0.0, 0.0), None);
        assert_eq!(add_link(&mut doc, "s1", "zz", 0.0, 0.0), None);
        assert_eq!(doc.links.len(), 3, "no stray link");
        assert_eq!(next_link_id(&doc), "l4");
    }

    #[test]
    fn auto_link_nearest_same_floor_only() {
        let mut doc = doc_with_floor();
        assert_eq!(add_floor(&mut doc, "1er".into(), 1), "f2");
        add_scene(&mut doc, "f1", "a".into(), (0.0, 0.0));
        assert!(doc.links.is_empty(), "1st scene: no link");
        add_scene(&mut doc, "f2", "b".into(), (5.0, 5.0));
        assert!(doc.links.is_empty(), "auto-link = same floor only");
        add_scene(&mut doc, "f1", "c".into(), (11.0, 0.0));
        // s3 @(11,0): closest to s2 @(10,0)… which is on the OTHER floor —
        // the closest on the same floor is s1 @(0,0) → pair s1↔s3.
        assert_eq!(doc.links.len(), 2);
        assert!(doc.links.iter().all(|l| l.from != "s2" && l.to != "s2"));
    }

    #[test]
    fn connect_scenes_pair_and_auto_angles() {
        let mut doc = doc_with_floor();
        assert_eq!(add_floor(&mut doc, "1er".into(), 1), "f2");
        add_scene(&mut doc, "f1", "a".into(), (10.0, 0.0)); // s1 @ f1 (level 0)
        add_scene(&mut doc, "f2", "b".into(), (10.0, 10.0)); // s2 @ f2 — no auto-link (floors)
        assert!(doc.links.is_empty());

        assert_eq!(connect_scenes(&mut doc, "s1", "s2"), 2, "pair created");
        let ab = doc.links.iter().find(|l| l.from == "s1").unwrap();
        let ba = doc.links.iter().find(|l| l.from == "s2").unwrap();
        assert_eq!(ab.kind, pnex_core::TOUR_LINK_STAIR);
        assert!((ab.pitch - STAIR_PITCH_DEG).abs() < 1e-9, "up: +30°");
        assert!((ba.pitch + STAIR_PITCH_DEG).abs() < 1e-9, "down: -30°");
        // Yaw: ENU azimuth of the plan direction (dx=0, dy=+10 = south = 180°).
        assert!((ab.yaw - 180.0).abs() < 1e-9);
        assert!((ba.yaw - 0.0).abs() < 1e-9);

        // Already connected: nothing left to create.
        assert_eq!(
            connect_scenes(&mut doc, "s1", "s2"),
            0,
            "duplicates refused"
        );
        assert_eq!(connect_scenes(&mut doc, "s1", "s1"), 0, "auto-liaison");
        assert_eq!(connect_scenes(&mut doc, "s1", "zz"), 0, "unknown scene");
        assert_eq!(doc.links.len(), 2);
    }

    #[test]
    fn patches_noop_on_unknown_id() {
        let mut doc = doc_with_floor();
        move_scene(&mut doc, "zz", (1.0, 1.0));
        set_scene_label(&mut doc, "zz", "x".into());
        set_scene_media(&mut doc, "zz", "u".into());
        set_scene_view(&mut doc, "zz", 1.0, 1.0, 1.0);
        set_link_label(&mut doc, "zz", Some("x".into()));
        set_link_angles(&mut doc, "zz", 1.0, 1.0);
        remove_link(&mut doc, "zz");
        set_floor_plan(&mut doc, "zz", None);
        assert_eq!(doc, doc_with_floor(), "all patches: no-op");
    }
}
