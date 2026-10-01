//! Carte POI-first (D35–D39, PRD `docs/architecture/viz-bases.md`) :
//! service unique d'écriture des POI (`map_pins`), du **clustering
//! backend** (D37) et des **positions GPS devices** (D38). École
//! `services/flow.rs` : contrôleurs (et futurs outils IA) passent par ici,
//! jamais par les entités directement.
//!
//! Règles portées ici (la DB ne les a pas toutes) :
//! - org imposé partout (D2) : device, media_asset, dashboard, diagram sont
//!   résolus **dans l'org**, sinon `*Unknown` (→ 400/404 masqué) ;
//! - POI **géo uniquement** en V1 (`mode="geo"`) — `mode`/`x`/`y` restent en
//!   base pour les plans futurs sans hiérarchie (CHECK `chk_map_pins_mode_coords`) ;
//! - D43 : un device = **un seul** placement (`device_placements`
//!   UNIQUE `device_registry_id`, PG **et** sqlite) mais un POI porte
//!   **plusieurs** devices ; `location_detail` par placement (in-site) ;
//!   device résolu dans `device_registries` de l'org. Amendement
//!   2026-09-13 : le **détachement** existe (DELETE placement, le device
//!   redevient libre) ; attacher ailleurs reste possible = déplacer (PATCH
//!   `pin_id`, confirmation 409) — l'unicité ne bouge pas ;
//! - champs d'usage carte **typés et validés** (hardening) :
//!   `location_detail` ≤ 255, emoji ≤ 8 chars, lat/lon bornés — le
//!   `metadata` JSONB libre reste pour le reste (école device.metadata) ;
//! - liens (D39 → D42) : arêtes `placed_on` de la couche d'organisation
//!   transverse (`resource_edges`, `viz_links` absorbé) — cibles à
//!   existence org validée ; suppression du POI ⇒ purge **symétrique** des
//!   arêtes (2 bouts) via `resources::purge_for` ;
//! - D38 : convention GPS = métriques télémétrie `latitude`/`longitude`
//!   (WGS84 degrés décimaux) + optionnels `gps_*` ; **dernière** position
//!   par device upsertée au tap du sink (`GpsTapSink`) ; l'historique
//!   complet reste dans OpenObserve (séries métriques par org).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use sea_orm::entity::prelude::{DateTimeWithTimeZone, Decimal};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set,
};
use uuid::Uuid;

use crate::models::_entities::{
    device_placements, device_positions, device_registries, map_pins, resource_edges,
};
use crate::services::telemetry::{TelemetryPoint, TelemetrySink};

/// Pictogramme par défaut d'un POI (un graphème emoji, rendu comme du texte).
pub const PIN_EMOJI_DEFAULT: &str = "📍";
pub const PIN_MODE_GEO: &str = "geo";

/// Aperçu épinglé : kinds épinglables (objets attachés à visuel read-only —
/// les devices n'ont pas d'aperçu).
pub const PREVIEW_KINDS: [&str; 3] = ["media_asset", "dashboard", "tour"];

/// D37 : au-delà de ce nombre de points dans la bbox, on agrège en clusters.
pub const CLUSTER_INDIVIDUAL_MAX: usize = 200;
/// D37 : cap d'items renvoyés (cellule doublée jusqu'à tenir dans le cap).
pub const CLUSTER_ITEMS_CAP: usize = 500;
pub const ZOOM_MAX: i32 = 22;

mod cluster;
mod links;
mod pins;
mod placements;
mod positions;
mod validation;

pub use cluster::*;
pub use links::*;
pub use pins::*;
pub use placements::*;
pub use positions::*;
pub use validation::*;

// ─────────────────────────── tests unitaires purs ───────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn point(id: &str, lat: f64, lon: f64) -> ClusterPoint {
        ClusterPoint {
            id: Uuid::parse_str(id).unwrap(),
            lat,
            lon,
            label: format!("poi-{id}"),
            emoji: "📍".into(),
        }
    }

    #[test]
    fn cellule_diminue_avec_le_zoom() {
        assert!((cell_size_deg(0) - 45.0).abs() < 1e-9, "360 / (8 × 2⁰)");
        assert!((cell_size_deg(1) - 22.5).abs() < 1e-9);
        assert!(cell_size_deg(10) < cell_size_deg(5));
        assert_eq!(cell_size_deg(-5), cell_size_deg(0), "zoom borné bas");
        assert_eq!(
            cell_size_deg(99),
            cell_size_deg(ZOOM_MAX),
            "zoom borné haut"
        );
    }

    #[test]
    fn sous_le_seuil_points_individuels_porteurs_did() {
        let points: Vec<ClusterPoint> = (0..10)
            .map(|i| {
                point(
                    &format!("00000000-0000-0000-0000-{i:012}"),
                    45.0 + f64::from(i as u32) * 0.001,
                    4.0,
                )
            })
            .collect();
        let items = cluster_points(&points, cell_size_deg(3));
        assert_eq!(items.len(), 10, "≤ CLUSTER_INDIVIDUAL_MAX : individuels");
        assert!(items.iter().all(|i| i.count == 1 && i.id.is_some()));
        assert!(items.iter().all(|i| i.label.starts_with("poi-")));
    }

    #[test]
    fn au_dessu_du_seuil_agregation_en_centroides() {
        // 250 points quasi confondus + 1 très loin : 2 buckets (zoom 3).
        let mut points: Vec<ClusterPoint> = (0..CLUSTER_INDIVIDUAL_MAX + 50)
            .map(|i| {
                point(
                    &format!("00000000-0000-0000-0000-{i:012}"),
                    45.0 + f64::from((i % 7) as u32) * 1e-6,
                    4.0 + f64::from((i % 5) as u32) * 1e-6,
                )
            })
            .collect();
        points.push(point("ffffffff-ffff-ffff-ffff-ffffffffff01", 10.0, 20.0));
        let items = cluster_points(&points, cell_size_deg(3));
        assert_eq!(items.len(), 2, "deux cellules occupées");
        let big = items.iter().find(|i| i.count > 1).expect("groupe dense");
        assert_eq!(big.count as usize, CLUSTER_INDIVIDUAL_MAX + 50);
        assert!(big.id.is_none(), "cluster sans id");
        // Centroïde ≈ barycentre des jitter.
        assert!((big.lat - 45.0).abs() < 1e-4);
        assert!((big.lon - 4.0).abs() < 1e-4);
        let lone = items.iter().find(|i| i.count == 1).expect("point isolé");
        assert!(lone.id.is_some(), "singleton garde son id");
        assert!((lone.lat - 10.0).abs() < 1e-9);
    }

    #[test]
    fn le_cap_ditems_doubl_la_cellule() {
        // 300 points équirépartis en diagonale : à zoom 22 la cellule est
        // minuscule → beaucoup de buckets ; le cap force la croissance.
        let points: Vec<ClusterPoint> = (0..CLUSTER_INDIVIDUAL_MAX + 100)
            .map(|i| {
                point(
                    &format!("00000000-0000-0000-0000-{i:012}"),
                    40.0 + f64::from(i as u32) * 0.01,
                    0.0 + f64::from(i as u32) * 0.013,
                )
            })
            .collect();
        let items = cluster_points(&points, cell_size_deg(22));
        assert!(
            items.len() <= CLUSTER_ITEMS_CAP,
            "cap respecté : {} items",
            items.len()
        );
        assert_eq!(
            items.iter().map(|i| i.count as usize).sum::<usize>(),
            points.len(),
            "aucun point perdu"
        );
    }

    #[test]
    fn tri_decroissant_par_densite() {
        let mut points: Vec<ClusterPoint> = (0..CLUSTER_INDIVIDUAL_MAX + 30)
            .map(|i| {
                point(
                    &format!("00000000-0000-0000-0000-{i:012}"),
                    if i < 200 { 45.0 } else { 10.0 },
                    if i < 200 { 4.0 } else { 20.0 },
                )
            })
            .collect();
        points.push(point("ffffffff-ffff-ffff-ffff-ffffffffff01", 0.0, 0.0));
        let items = cluster_points(&points, cell_size_deg(5));
        let counts: Vec<u32> = items.iter().map(|i| i.count).collect();
        let mut sorted = counts.clone();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(counts, sorted, "items triés par count desc");
    }
}
