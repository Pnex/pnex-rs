//! POI-first map (D35–D39, PRD `docs/architecture/viz-bases.md`):
//! single write service for POIs (`map_pins`), **backend
//! clustering** (D37) and **device GPS positions** (D38). Modelled on
//! `services/flow.rs`: controllers (and future AI tools) go through here,
//! never through the entities directly.
//!
//! Rules carried here (the DB does not have them all):
//! - org enforced everywhere (D2): device, media_asset, dashboard, diagram are
//!   resolved **within the org**, otherwise `*Unknown` (→ masked 400/404);
//! - POI **geo only** in V1 (`mode="geo"`) — `mode`/`x`/`y` stay in the
//!   database for future floor plans without hierarchy (CHECK `chk_map_pins_mode_coords`);
//! - D43: a device = **a single** placement (`device_placements`
//!   UNIQUE `device_registry_id`) but a POI carries
//!   **several** devices; `location_detail` per placement (in-site);
//!   device resolved in the org's `device_registries`. Amendment
//!   2026-09-13: **detaching** exists (DELETE placement, the device
//!   becomes free again); attaching elsewhere stays possible = moving (PATCH
//!   `pin_id`, 409 confirmation) — uniqueness does not change;
//! - map usage fields **typed and validated** (hardening):
//!   `location_detail` ≤ 255, emoji ≤ 8 chars, bounded lat/lon — the free
//!   JSONB `metadata` stays for the rest (device.metadata pattern);
//! - links (D39 → D42): `placed_on` edges of the cross-cutting organisation
//!   layer (`resource_edges`, `viz_links` absorbed) — targets with
//!   org existence validated; deleting the POI ⇒ **symmetric** purge of the
//!   edges (both ends) via `resources::purge_for`;
//! - D38: GPS convention = telemetry metrics `latitude`/`longitude`
//!   (WGS84 decimal degrees) + optional `gps_*`; **latest** position
//!   per device upserted at the sink tap (`GpsTapSink`); the full
//!   history stays in OpenObserve (metric series per org).

use std::collections::HashMap;
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

/// Default pictogram of a POI (one emoji grapheme, rendered as text).
pub const PIN_EMOJI_DEFAULT: &str = "📍";
pub const PIN_MODE_GEO: &str = "geo";

/// Pinned preview: pinnable kinds (attached objects with a read-only visual —
/// devices have no preview).
pub const PREVIEW_KINDS: [&str; 3] = ["media_asset", "dashboard", "tour"];

/// D37: beyond this number of points in the bbox, we aggregate into clusters.
pub const CLUSTER_INDIVIDUAL_MAX: usize = 200;
/// D37: cap on returned items (cell doubled until it fits in the cap).
pub const CLUSTER_ITEMS_CAP: usize = 500;
pub const ZOOM_MAX: i32 = 22;

mod cluster;
mod links;
mod pins;
mod placements;
/// Position history in O2 (cases-workspace.md K26).
pub mod position_log;
mod positions;
pub mod track_import;
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
        assert_eq!(cell_size_deg(-5), cell_size_deg(0), "zoom clamped low");
        assert_eq!(
            cell_size_deg(99),
            cell_size_deg(ZOOM_MAX),
            "zoom clamped high"
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
        // 250 nearly overlapping points + 1 very far: 2 buckets (zoom 3).
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
        assert_eq!(items.len(), 2, "two occupied cells");
        let big = items.iter().find(|i| i.count > 1).expect("groupe dense");
        assert_eq!(big.count as usize, CLUSTER_INDIVIDUAL_MAX + 50);
        assert!(big.id.is_none(), "cluster without an id");
        // Centroid ≈ barycentre of the jitter.
        assert!((big.lat - 45.0).abs() < 1e-4);
        assert!((big.lon - 4.0).abs() < 1e-4);
        let lone = items.iter().find(|i| i.count == 1).expect("isolated point");
        assert!(lone.id.is_some(), "singleton keeps its id");
        assert!((lone.lat - 10.0).abs() < 1e-9);
    }

    #[test]
    fn le_cap_ditems_doubl_la_cellule() {
        // 300 points evenly spread on a diagonal: at zoom 22 the cell is
        // tiny → many buckets; the cap forces growth.
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
            "cap respected: {} items",
            items.len()
        );
        assert_eq!(
            items.iter().map(|i| i.count as usize).sum::<usize>(),
            points.len(),
            "no point lost"
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
        assert_eq!(counts, sorted, "items sorted by count desc");
    }
}
