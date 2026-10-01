//! Backend clustering (D37): pure grid aggregation and viewport query.

use super::*;

// ── clustering (D37) — cœur pur, testé unitairement ──

/// Taille de cellule de la grille en degrés : ≈ 64 px sur tuiles 512
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

/// Un point individuel (`count == 1`, porteur d'id/label/emoji) ou un
/// cluster (numéro + point représentatif = centroïde + échantillon).
pub struct ClusterItem {
    pub lat: f64,
    pub lon: f64,
    pub count: u32,
    pub id: Option<Uuid>,
    pub label: String,
    pub emoji: String,
}

fn bucket(points: &[ClusterPoint], cell: f64) -> Vec<ClusterItem> {
    // (clé de cellule) → (somme lat, somme lon, n, id du 1er point, label,
    // emoji) — tuple interne du clustering, local à cette fonction.
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
            },
        )
        .collect();
    items.sort_by_key(|it| std::cmp::Reverse(it.count));
    items
}

/// Agrège les points : individuels sous `CLUSTER_INDIVIDUAL_MAX`, sinon
/// grille `cell` doublée jusqu'à tenir dans `CLUSTER_ITEMS_CAP`.
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

/// Items (pins individuels ou clusters) pour la bbox/zoom du viewport.
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
    Ok(cluster_points(&points, cell_size_deg(q.zoom)))
}
