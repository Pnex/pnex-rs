//! Backend clustering (D37): pure grid aggregation and viewport query.

use super::*;

// ── clustering (D37) — pure core, unit-tested ──

/// Grid cell size in degrees: ≈ 64 px on 512 tiles
/// (`360 / (8 × 2^zoom)`).
pub fn cell_size_deg(zoom: i32) -> f64 {
    let z = zoom.clamp(0, ZOOM_MAX) as f64;
    360.0 / (8.0 * 2f64.powf(z))
}

pub struct ClusterPoint {
    pub id: Uuid,
    pub lat: f64,
    pub lon: f64,
    pub label: String,
    pub emoji: String,
}

/// An individual point (`count == 1`, carrying id/label/emoji) or a
/// cluster (count + representative point = centroid + sample).
pub struct ClusterItem {
    pub lat: f64,
    pub lon: f64,
    pub count: u32,
    pub id: Option<Uuid>,
    pub label: String,
    pub emoji: String,
    /// Colour of the pin's coloured status (D339), set after clustering.
    pub color: Option<String>,
}

fn bucket(points: &[ClusterPoint], cell: f64) -> Vec<ClusterItem> {
    // (cell key) → (lat sum, lon sum, n, id of the 1st point, label,
    // emoji) — internal clustering tuple, local to this function.
    #[allow(clippy::type_complexity)]
    let mut buckets: HashMap<(i64, i64), (f64, f64, u32, Option<Uuid>, String, String)> =
        HashMap::new();
    for p in points {
        let key = ((p.lon / cell).floor() as i64, (p.lat / cell).floor() as i64);
        let e = buckets
            .entry(key)
            .or_insert((0.0, 0.0, 0, None, String::new(), String::new()));
        e.0 += p.lat;
        e.1 += p.lon;
        e.2 += 1;
        if e.2 == 1 {
            e.3 = Some(p.id);
            e.4 = p.label.clone();
            e.5 = p.emoji.clone();
        }
    }
    let mut items: Vec<ClusterItem> = buckets
        .into_values()
        .map(
            |(sum_lat, sum_lon, n, first_id, label, emoji)| ClusterItem {
                lat: sum_lat / f64::from(n),
                lon: sum_lon / f64::from(n),
                count: n,
                id: if n == 1 { first_id } else { None },
                label,
                emoji,
                color: None,
            },
        )
        .collect();
    items.sort_by_key(|it| std::cmp::Reverse(it.count));
    items
}

/// Aggregates the points: individual under `CLUSTER_INDIVIDUAL_MAX`, otherwise
/// a `cell` grid doubled until it fits in `CLUSTER_ITEMS_CAP`.
pub fn cluster_points(points: &[ClusterPoint], cell: f64) -> Vec<ClusterItem> {
    if points.len() <= CLUSTER_INDIVIDUAL_MAX {
        return points
            .iter()
            .map(|p| ClusterItem {
                lat: p.lat,
                lon: p.lon,
                count: 1,
                id: Some(p.id),
                label: p.label.clone(),
                emoji: p.emoji.clone(),
                color: None,
            })
            .collect();
    }
    let mut cell = cell.max(f64::MIN_POSITIVE);
    loop {
        let items = bucket(points, cell);
        if items.len() <= CLUSTER_ITEMS_CAP || cell >= 360.0 {
            return items;
        }
        cell *= 2.0;
    }
}

pub struct ClusterQuery {
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
    pub zoom: i32,
}

/// Items (individual pins or clusters) for the viewport's bbox/zoom.
pub async fn cluster_pins(
    db: &DatabaseConnection,
    org_id: i64,
    q: &ClusterQuery,
    f: &PinFilters<'_>,
) -> Result<Vec<ClusterItem>, DbErr> {
    let rows = filtered_pins(db, org_id, Some((q.west, q.south, q.east, q.north)), f).await?;
    let points: Vec<ClusterPoint> = rows
        .into_iter()
        .filter_map(|p| {
            let lat = d2f(p.latitude)?;
            let lon = d2f(p.longitude)?;
            Some(ClusterPoint {
                id: p.id,
                lat,
                lon,
                label: p.label.clone(),
                emoji: p.emoji.unwrap_or_else(|| PIN_EMOJI_DEFAULT.to_string()),
            })
        })
        .collect();
    let mut items = cluster_points(&points, cell_size_deg(q.zoom));
    // Single pins wear the colour of their status (account-mapping.md D339).
    let singles: Vec<String> = items
        .iter()
        .filter_map(|i| i.id)
        .map(|id| id.to_string())
        .collect();
    let colors = crate::services::ontology::objects::colors_of(
        db,
        org_id,
        pnex_core::resources::KIND_MAP_PIN,
        &singles,
    )
    .await
    .map_err(|e| DbErr::Custom(e.to_string()))?;
    for i in &mut items {
        i.color = i.id.and_then(|id| colors.get(&id.to_string()).cloned());
    }
    Ok(items)
}
