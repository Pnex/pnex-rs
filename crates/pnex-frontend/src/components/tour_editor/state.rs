//! État et réducteurs purs de l'éditeur de tour — toutes les mutations du
//! document passent par ces fonctions pures (testées), jamais in-line dans
//! le RSX (école `flow_editor/state.rs`).

use pnex_core::{FloorPlan, TourDoc, TourFloor, TourLink};

/// Prochain id libre d'une collection : `{prefix}{max(suffixes)+1}` — jamais
/// de collision même après plusieurs ajouts/suppressions (école
/// `next_node_id`). `taken` = ids existants de la collection.
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

/// Ajoute un étage (nom/level fournis par l'UI) — retourne l'id créé.
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

/// Pose/retire le plan d'un étage — no-op si l'étage est inconnu.
pub fn set_floor_plan(doc: &mut TourDoc, floor_id: &str, plan: Option<FloorPlan>) {
    if let Some(floor) = doc.floors.iter_mut().find(|f| f.id == floor_id) {
        floor.plan = plan;
    }
}

/// Patch le plan d'un étage (dimensions/échelle/retrait) — no-op si
/// l'étage (ou son plan) est inconnu.
pub fn patch_plan(doc: &mut TourDoc, floor_id: &str, f: impl FnOnce(&mut FloorPlan)) {
    if let Some(floor) = doc.floors.iter_mut().find(|f| f.id == floor_id) {
        if let Some(plan) = &mut floor.plan {
            f(plan);
        }
    }
}

/// Supprime un étage — **refusé** s'il porte des scènes (l'UI invite à les
/// supprimer d'abord). Retourne `true` si supprimé.
pub fn remove_floor(doc: &mut TourDoc, floor_id: &str) -> bool {
    let occupied = doc.scenes.iter().any(|s| s.floor_id == floor_id);
    if occupied {
        return false;
    }
    let before = doc.floors.len();
    doc.floors.retain(|f| f.id != floor_id);
    doc.floors.len() != before
}

/// Pose une scène (asset panorama + position plan) et **auto-lie** : paire
/// de liens aller-retour vers la scène la plus proche du même étage, s'il
/// y en a une (parcours connecté par défaut — reste éditable/supprimable
/// comme tout lien). Retourne l'id créé.
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
    // Plus proche voisine sur le MÊME étage (distance² au plan ; l'ordre
    // des égales distances est stable = première du doc).
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

/// Déplace une scène (drag) — no-op si l'id est inconnu.
pub fn move_scene(doc: &mut TourDoc, id: &str, pos: (f64, f64)) {
    if let Some(scene) = doc.scenes.iter_mut().find(|s| s.id == id) {
        scene.x = pos.0;
        scene.y = pos.1;
    }
}

/// Patchs de l'inspecteur — no-op si l'id est inconnu.
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

/// Supprime une scène **et** tous les liens qui la touchent.
pub fn remove_scene(doc: &mut TourDoc, id: &str) {
    doc.scenes.retain(|s| s.id != id);
    doc.links.retain(|l| l.from != id && l.to != id);
    if doc.start_scene.as_deref() == Some(id) {
        doc.start_scene = None;
    }
}

/// Lien d'une scène vers une autre : `kind` **dérivé** des étages des
/// extrémités (`walk` même étage, `stair` sinon). Refus (→ `None`) :
/// auto-liaison, doublon (from,to), scène inconnue.
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

/// Repositionne un hotspot (drag de la flèche en preview) — no-op si le
/// lien est inconnu. Yaw borné à ±180°, pitch à ±90° (sphère).
pub fn set_link_angles(doc: &mut TourDoc, id: &str, yaw: f64, pitch: f64) {
    if let Some(link) = doc.links.iter_mut().find(|l| l.id == id) {
        link.yaw = yaw.clamp(-180.0, 180.0);
        link.pitch = pitch.clamp(-90.0, 90.0);
    }
}

/// Yaw du hotspot A→B : direction sur le plan en degrés ENU (x = est,
/// y du plan vers le bas → négation, école `scene_pointer_down`).
fn yaw_between(a: (f64, f64), b: (f64, f64)) -> f64 {
    (b.0 - a.0).atan2(-(b.1 - a.1)).to_degrees()
}

/// Pitch du hotspot selon les niveaux des extrémités : descendre = flèche
/// vers le sol (-30°), monter = vers le plafond (+30°), à plat = 0°.
const STAIR_PITCH_DEG: f64 = 30.0;
fn pitch_between(from_level: i32, to_level: i32) -> f64 {
    match to_level.cmp(&from_level) {
        std::cmp::Ordering::Less => -STAIR_PITCH_DEG,
        std::cmp::Ordering::Greater => STAIR_PITCH_DEG,
        std::cmp::Ordering::Equal => 0.0,
    }
}

/// Connecte deux scènes : **paire aller-retour** de liens (un lien = un
/// hotspot directionnel — le retour est nécessaire pour revenir), yaw et
/// pitch **automatiques** — yaw = direction sur le plan, pitch = ±30° si
/// les niveaux diffèrent (stair : descendre/monter), plat sinon (walk).
/// Retourne le nombre de liens créés (0 = auto-liaison, scènes inconnues,
/// ou les deux directions existaient déjà — les doublons sont conservés,
/// jamais dupliqués).
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

/// Supprime un lien (ou la demi-lien d'un câble supprimé côté UI).
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
    fn ids_sans_collision_apres_suppression() {
        let mut doc = doc_with_floor();
        let s1 = add_scene(&mut doc, "f1", "a".into(), (0.0, 0.0));
        let s2 = add_scene(&mut doc, "f1", "b".into(), (10.0, 0.0));
        assert_eq!((s1.as_str(), s2.as_str()), ("s1", "s2"));
        remove_scene(&mut doc, "s2");
        // s2 supprimé : réutilisable (école next_node_id).
        assert_eq!(add_scene(&mut doc, "f1", "c".into(), (0.0, 0.0)), "s2");
    }

    #[test]
    fn ajout_etage_et_suppression_refusee_si_scenes() {
        let mut doc = doc_with_floor();
        assert_eq!(add_floor(&mut doc, "1er".into(), 1), "f2");
        assert_eq!(doc.floors.len(), 2);

        add_scene(&mut doc, "f2", "a".into(), (0.0, 0.0));
        assert!(!remove_floor(&mut doc, "f2"), "étage occupé : refusé");
        assert_eq!(doc.floors.len(), 2);
        remove_scene(&mut doc, "s1");
        assert!(remove_floor(&mut doc, "f2"), "étage vide : supprimé");
    }

    #[test]
    fn suppression_scene_purge_les_liens_et_le_depart() {
        let mut doc = doc_with_floor();
        add_scene(&mut doc, "f1", "a".into(), (0.0, 0.0));
        add_scene(&mut doc, "f1", "b".into(), (10.0, 0.0));
        // Auto-link : la 2e scène est reliée d'office à la 1re (aller-retour).
        assert_eq!(doc.links.len(), 2);
        set_start_scene(&mut doc, Some("s1".into()));
        remove_scene(&mut doc, "s1");
        assert!(doc.links.is_empty(), "liens touchés purgés");
        assert_eq!(doc.start_scene, None, "départ tombé : reset");
        assert_eq!(doc.scenes.len(), 1);
    }

    #[test]
    fn liens_refus_et_kind_derive() {
        let mut doc = doc_with_floor();
        assert_eq!(add_floor(&mut doc, "1er".into(), 1), "f2");
        add_scene(&mut doc, "f1", "a".into(), (0.0, 0.0)); // s1 @ f1
        add_scene(&mut doc, "f1", "b".into(), (10.0, 0.0)); // s2 @ f1 — auto-liée à s1
        add_scene(&mut doc, "f2", "c".into(), (0.0, 0.0)); // s3 @ f2 — isolée (aucune scène sur f2)

        assert_eq!(doc.links.len(), 2, "auto-lien s1↔s2 seulement");
        assert!(doc
            .links
            .iter()
            .all(|l| l.kind == pnex_core::TOUR_LINK_WALK));

        // Kind dérivé : étages différents → stair (ajout manuel).
        assert!(add_link(&mut doc, "s2", "s3", 0.0, 0.0).is_some());
        assert_eq!(doc.links[2].kind, pnex_core::TOUR_LINK_STAIR);

        // Refus : auto-liaison, doublon (déjà auto-lié), scène inconnue.
        assert_eq!(add_link(&mut doc, "s1", "s1", 0.0, 0.0), None);
        assert_eq!(add_link(&mut doc, "s1", "s2", 0.0, 0.0), None);
        assert_eq!(add_link(&mut doc, "s1", "zz", 0.0, 0.0), None);
        assert_eq!(doc.links.len(), 3, "aucun lien parasite");
        assert_eq!(next_link_id(&doc), "l4");
    }

    #[test]
    fn auto_lien_plus_proche_meme_etage_seulement() {
        let mut doc = doc_with_floor();
        assert_eq!(add_floor(&mut doc, "1er".into(), 1), "f2");
        add_scene(&mut doc, "f1", "a".into(), (0.0, 0.0));
        assert!(doc.links.is_empty(), "1re scène : aucun lien");
        add_scene(&mut doc, "f2", "b".into(), (5.0, 5.0));
        assert!(doc.links.is_empty(), "auto-lien = même étage seulement");
        add_scene(&mut doc, "f1", "c".into(), (11.0, 0.0));
        // s3 @(11,0) : plus proche de s2 @(10,0)… qui est sur l'AUTRE étage —
        // la plus proche du même étage est s1 @(0,0) → paire s1↔s3.
        assert_eq!(doc.links.len(), 2);
        assert!(doc.links.iter().all(|l| l.from != "s2" && l.to != "s2"));
    }

    #[test]
    fn connect_scenes_paire_et_angles_auto() {
        let mut doc = doc_with_floor();
        assert_eq!(add_floor(&mut doc, "1er".into(), 1), "f2");
        add_scene(&mut doc, "f1", "a".into(), (10.0, 0.0)); // s1 @ f1 (level 0)
        add_scene(&mut doc, "f2", "b".into(), (10.0, 10.0)); // s2 @ f2 — pas d'auto-lien (étages)
        assert!(doc.links.is_empty());

        assert_eq!(connect_scenes(&mut doc, "s1", "s2"), 2, "paire créée");
        let ab = doc.links.iter().find(|l| l.from == "s1").unwrap();
        let ba = doc.links.iter().find(|l| l.from == "s2").unwrap();
        assert_eq!(ab.kind, pnex_core::TOUR_LINK_STAIR);
        assert!((ab.pitch - STAIR_PITCH_DEG).abs() < 1e-9, "monter : +30°");
        assert!(
            (ba.pitch + STAIR_PITCH_DEG).abs() < 1e-9,
            "descendre : -30°"
        );
        // Yaw : azimut ENU de la direction plan (dx=0, dy=+10 = sud = 180°).
        assert!((ab.yaw - 180.0).abs() < 1e-9);
        assert!((ba.yaw - 0.0).abs() < 1e-9);

        // Déjà connecté : plus rien à créer.
        assert_eq!(connect_scenes(&mut doc, "s1", "s2"), 0, "doublons refusés");
        assert_eq!(connect_scenes(&mut doc, "s1", "s1"), 0, "auto-liaison");
        assert_eq!(connect_scenes(&mut doc, "s1", "zz"), 0, "scène inconnue");
        assert_eq!(doc.links.len(), 2);
    }

    #[test]
    fn patchs_noop_sur_id_inconnu() {
        let mut doc = doc_with_floor();
        move_scene(&mut doc, "zz", (1.0, 1.0));
        set_scene_label(&mut doc, "zz", "x".into());
        set_scene_media(&mut doc, "zz", "u".into());
        set_scene_view(&mut doc, "zz", 1.0, 1.0, 1.0);
        set_link_label(&mut doc, "zz", Some("x".into()));
        set_link_angles(&mut doc, "zz", 1.0, 1.0);
        remove_link(&mut doc, "zz");
        set_floor_plan(&mut doc, "zz", None);
        assert_eq!(doc, doc_with_floor(), "tous les patchs : no-op");
    }
}
