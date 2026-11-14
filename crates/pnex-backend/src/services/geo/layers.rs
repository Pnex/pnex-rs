//! Geographic layers, phase A (geo-layers.md L4–L6): a layer and its
//! features in PostGIS, MVT tiles encoded by `ST_AsMVT`. No cache yet
//! (phase C): every tile is one indexed query.

use pnex_core::geo::{collection_family, GeoLayer, GeoLayerInput};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbErr, FromQueryResult, Statement, TransactionTrait,
};
use uuid::Uuid;

/// Most features drawn in one tile (L6): beyond it the tile is partial.
const TILE_FEATURE_CAP: i64 = 20_000;
/// MVT tile extent and buffer, in tile units.
const TILE_EXTENT: i32 = 4096;
const TILE_BUFFER: i32 = 64;

#[derive(Debug, thiserror::Error)]
pub enum LayerError {
    #[error("invalid field {field}: {token}")]
    Invalid { field: &'static str, token: String },
    #[error("not found")]
    NotFound,
    #[error("name taken")]
    NameTaken,
    #[error(transparent)]
    Db(#[from] DbErr),
}

fn invalid(field: &'static str, token: &str) -> LayerError {
    LayerError::Invalid {
        field,
        token: token.to_string(),
    }
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres, sql, values)
}

const SELECT: &str = "SELECT id, name, description, geometry_type, generation, zoom_min, \
     zoom_max, style, feature_count, ST_XMin(extent) AS west, ST_YMin(extent) AS south, \
     ST_XMax(extent) AS east, ST_YMax(extent) AS north, updated_at FROM geo_layers";

#[derive(FromQueryResult)]
struct Row {
    id: Uuid,
    name: String,
    description: Option<String>,
    geometry_type: String,
    generation: i64,
    zoom_min: i16,
    zoom_max: i16,
    style: serde_json::Value,
    feature_count: i64,
    west: Option<f64>,
    south: Option<f64>,
    east: Option<f64>,
    north: Option<f64>,
    updated_at: chrono::DateTime<chrono::FixedOffset>,
}

impl From<Row> for GeoLayer {
    fn from(r: Row) -> Self {
        let extent = match (r.west, r.south, r.east, r.north) {
            (Some(w), Some(s), Some(e), Some(n)) => Some([w, s, e, n]),
            _ => None,
        };
        GeoLayer {
            id: r.id,
            name: r.name,
            description: r.description,
            geometry_type: r.geometry_type,
            generation: r.generation,
            zoom_min: r.zoom_min,
            zoom_max: r.zoom_max,
            style: serde_json::from_value(r.style).unwrap_or_default(),
            feature_count: r.feature_count,
            extent,
            updated_at: r.updated_at.to_rfc3339(),
        }
    }
}

pub async fn list(db: &DatabaseConnection, org_id: i64) -> Result<Vec<GeoLayer>, LayerError> {
    Ok(Row::find_by_statement(stmt(
        &format!("{SELECT} WHERE org_id = $1 ORDER BY name"),
        vec![org_id.into()],
    ))
    .all(db)
    .await?
    .into_iter()
    .map(Into::into)
    .collect())
}

pub async fn get(db: &impl ConnectionTrait, org_id: i64, id: Uuid) -> Result<GeoLayer, LayerError> {
    Row::find_by_statement(stmt(
        &format!("{SELECT} WHERE org_id = $1 AND id = $2"),
        vec![org_id.into(), id.into()],
    ))
    .one(db)
    .await?
    .map(Into::into)
    .ok_or(LayerError::NotFound)
}

fn validate(input: &GeoLayerInput) -> Result<(&'static str, i16, i16), LayerError> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(invalid("name", "required"));
    }
    if name.chars().count() > 255 {
        return Err(invalid("name", "max_length:255"));
    }
    if input.style.iter().any(|l| !l.is_object()) {
        return Err(invalid("style", "invalid"));
    }
    let zmin = input.zoom_min.unwrap_or(0);
    let zmax = input.zoom_max.unwrap_or(22);
    if !(0..=24).contains(&zmin) || !(zmin..=24).contains(&zmax) {
        return Err(invalid("zoom_max", "invalid"));
    }
    let family = collection_family(&input.features).map_err(|t| invalid("features", t))?;
    Ok((family, zmin, zmax))
}

/// Creates a layer with its whole content (generation 1).
pub async fn create(
    db: &DatabaseConnection,
    org_id: i64,
    input: &GeoLayerInput,
) -> Result<GeoLayer, LayerError> {
    let (family, zmin, zmax) = validate(input)?;
    let txn = db.begin().await?;
    let id = Uuid::new_v4();
    let style = serde_json::Value::Array(input.style.clone());
    let inserted = txn
        .execute_raw(stmt(
            "INSERT INTO geo_layers (id, org_id, name, description, geometry_type, zoom_min, \
             zoom_max, style) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (org_id, name) DO NOTHING",
            vec![
                id.into(),
                org_id.into(),
                input.name.trim().into(),
                input.description.clone().into(),
                family.into(),
                zmin.into(),
                zmax.into(),
                style.into(),
            ],
        ))
        .await?;
    if inserted.rows_affected() == 0 {
        return Err(LayerError::NameTaken);
    }
    replace_features(&txn, org_id, id, &input.features).await?;
    let layer = get(&txn, org_id, id).await?;
    txn.commit().await?;
    Ok(layer)
}

/// Replaces every feature of a layer and bumps its generation (L10 full
/// swap; the caller holds the transaction). Pipelines writing a layer
/// (labor-market M5) go through here too.
pub async fn replace_features(
    txn: &impl ConnectionTrait,
    org_id: i64,
    layer_id: Uuid,
    features: &serde_json::Value,
) -> Result<(), LayerError> {
    txn.execute_raw(stmt(
        "DELETE FROM geo_features WHERE org_id = $1 AND layer_id = $2",
        vec![org_id.into(), layer_id.into()],
    ))
    .await?;
    // shortcut: any failure of this insert is reported as invalid geometry
    // (PostGIS raises a generic error on bad GeoJSON); phase B's import job
    // collects errors per feature instead.
    if let Err(e) = txn
        .execute_raw(stmt(
            "INSERT INTO geo_features (org_id, layer_id, fid, geom, props) \
             SELECT $1, $2, f->>'id', \
                    ST_MakeValid(ST_SetSRID(ST_GeomFromGeoJSON(f->'geometry'), 4326)), \
                    CASE WHEN jsonb_typeof(f->'properties') = 'object' \
                         THEN f->'properties' ELSE '{}'::jsonb END \
             FROM jsonb_array_elements($3::jsonb->'features') f",
            vec![org_id.into(), layer_id.into(), features.clone().into()],
        ))
        .await
    {
        tracing::info!(error = %e, "geo layer features refused");
        return Err(invalid("features", "invalid"));
    }
    // A degenerate shape (one-point ring…) parses but repairs into an empty
    // geometry or a lower dimension than its family.
    #[derive(FromQueryResult)]
    struct Bad {
        n: i64,
    }
    let bad = Bad::find_by_statement(stmt(
        "SELECT count(*) AS n FROM geo_features f JOIN geo_layers l ON l.id = f.layer_id \
         WHERE f.layer_id = $1 AND (ST_IsEmpty(f.geom) OR ST_Dimension(f.geom) <> \
           CASE l.geometry_type WHEN 'point' THEN 0 WHEN 'line' THEN 1 ELSE 2 END)",
        vec![layer_id.into()],
    ))
    .one(txn)
    .await?;
    if bad.is_some_and(|b| b.n > 0) {
        return Err(invalid("features", "invalid"));
    }
    txn.execute_raw(stmt(
        "UPDATE geo_layers SET generation = generation + 1, updated_at = now(), \
           feature_count = (SELECT count(*) FROM geo_features WHERE layer_id = $2), \
           extent = (SELECT ST_SetSRID(ST_Envelope(ST_Extent(geom)::geometry), 4326) \
                     FROM geo_features WHERE layer_id = $2) \
         WHERE org_id = $1 AND id = $2",
        vec![org_id.into(), layer_id.into()],
    ))
    .await?;
    Ok(())
}

pub async fn delete(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<(), LayerError> {
    let res = db
        .execute_raw(stmt(
            "DELETE FROM geo_layers WHERE org_id = $1 AND id = $2",
            vec![org_id.into(), id.into()],
        ))
        .await?;
    if res.rows_affected() == 0 {
        return Err(LayerError::NotFound);
    }
    Ok(())
}

#[derive(FromQueryResult)]
struct Tile {
    mvt: Option<Vec<u8>>,
}

/// MVT bytes of one tile (empty outside the layer's zoom range). The
/// caller checked the tile coordinates and the layer's org.
pub async fn tile(
    db: &DatabaseConnection,
    org_id: i64,
    layer: &GeoLayer,
    z: u8,
    x: u32,
    y: u32,
) -> Result<Vec<u8>, LayerError> {
    if i16::from(z) < layer.zoom_min || i16::from(z) > layer.zoom_max {
        return Ok(Vec::new());
    }
    // A point's single vertex sits in one tile; lines and polygons are
    // clipped to the tile plus its buffer by ST_AsMVTGeom.
    let row = Tile::find_by_statement(stmt(
        &format!(
            "WITH b AS (SELECT ST_TileEnvelope($3, $4, $5) AS env), \
             q AS ( \
               SELECT f.id, f.props, \
                      ST_AsMVTGeom(f.geom_3857, b.env, {TILE_EXTENT}, {TILE_BUFFER}, true) AS geom \
               FROM geo_features f, b \
               WHERE f.org_id = $1 AND f.layer_id = $2 AND f.geom_3857 && b.env \
               LIMIT {TILE_FEATURE_CAP}) \
             SELECT ST_AsMVT(q.*, 'features', {TILE_EXTENT}, 'geom', 'id') AS mvt \
             FROM q WHERE q.geom IS NOT NULL"
        ),
        vec![
            org_id.into(),
            layer.id.into(),
            i32::from(z).into(),
            (x as i32).into(),
            (y as i32).into(),
        ],
    ))
    .one(db)
    .await?;
    Ok(row.and_then(|r| r.mvt).unwrap_or_default())
}
