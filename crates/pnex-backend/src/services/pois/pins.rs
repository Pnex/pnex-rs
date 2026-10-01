//! POI CRUD and filtered listings (map_pins writes).

use super::*;

// ─────────────────────────── POI ───────────────────────────

pub struct NewPin<'a> {
    pub label: &'a str,
    pub emoji: Option<&'a str>,
    pub location_detail: Option<&'a str>,
    pub latitude: f64,
    pub longitude: f64,
    pub metadata: Option<serde_json::Value>,
}

pub async fn create_pin(
    db: &DatabaseConnection,
    org_id: i64,
    p: NewPin<'_>,
) -> Result<map_pins::Model, VizWriteError> {
    let label = validate_label(Some(p.label))?;
    let emoji = validate_emoji(p.emoji)?.unwrap_or_else(|| PIN_EMOJI_DEFAULT.to_string());
    let location_detail = validate_location_detail(p.location_detail)?;
    let latitude = validate_lat(p.latitude)?;
    let longitude = validate_lon(p.longitude)?;

    let pin = map_pins::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        mode: Set(PIN_MODE_GEO.to_string()),
        latitude: Set(Some(latitude)),
        longitude: Set(Some(longitude)),
        x: Set(None),
        y: Set(None),
        label: Set(label),
        emoji: Set(Some(emoji)),
        location_detail: Set(location_detail),
        metadata: Set(p.metadata),
        ..Default::default()
    };
    pin.insert(db).await.map_err(|_| VizWriteError::Db)
}

/// PATCH partiel : `Option<Option<T>>` = absent / null / valeur (école sites).
pub struct UpdatePin<'a> {
    pub label: Option<&'a str>,
    pub emoji: Option<Option<&'a str>>,
    pub location_detail: Option<Option<&'a str>>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub metadata: Option<Option<serde_json::Value>>,
    /// Aperçu épinglé : posé (kind+id) ou retiré (null) **ensemble**.
    pub preview_kind: Option<Option<&'a str>>,
    pub preview_id: Option<Option<&'a str>>,
}

/// Kind d'aperçu valide, trimé (`None` si vide) — `Err` si hors liste.
fn validate_preview_kind(kind: &str) -> Result<Option<&str>, VizWriteError> {
    let kind = kind.trim();
    if kind.is_empty() {
        return Ok(None);
    }
    if PREVIEW_KINDS.contains(&kind) {
        Ok(Some(kind))
    } else {
        Err(VizWriteError::PreviewKindInvalid)
    }
}

pub async fn update_pin(
    db: &DatabaseConnection,
    pin: map_pins::Model,
    p: UpdatePin<'_>,
) -> Result<map_pins::Model, VizWriteError> {
    let mut am: map_pins::ActiveModel = pin.into();

    if let Some(label) = p.label {
        am.label = Set(validate_label(Some(label))?);
    }
    match p.emoji {
        // Some(None) = reset au pictogramme par défaut.
        Some(None) => am.emoji = Set(Some(PIN_EMOJI_DEFAULT.to_string())),
        Some(Some(e)) => am.emoji = Set(validate_emoji(Some(e))?),
        None => {}
    }
    match p.location_detail {
        Some(None) => am.location_detail = Set(None),
        Some(Some(d)) => am.location_detail = Set(validate_location_detail(Some(d))?),
        None => {}
    }
    match p.metadata {
        Some(None) => am.metadata = Set(None),
        Some(Some(m)) => am.metadata = Set(Some(m)),
        None => {}
    }
    if let Some(lat) = p.latitude {
        am.latitude = Set(Some(validate_lat(lat)?));
    }
    if let Some(lon) = p.longitude {
        am.longitude = Set(Some(validate_lon(lon)?));
    }
    // Aperçu épinglé : le couple (kind, id) vit ensemble — `null` sur l'un
    // des deux débriefe les deux (désépinglage) ; une pose exige les deux.
    match (p.preview_kind, p.preview_id) {
        (Some(None), _) | (_, Some(None)) => {
            am.preview_kind = Set(None);
            am.preview_id = Set(None);
        }
        (Some(Some(k)), Some(Some(id))) => {
            let kind = validate_preview_kind(k)?;
            let id = id.trim();
            if kind.is_none() || id.is_empty() || id.chars().count() > 255 {
                return Err(VizWriteError::PreviewKindInvalid);
            }
            am.preview_kind = Set(kind.map(str::to_string));
            am.preview_id = Set(Some(id.to_string()));
        }
        (Some(Some(_)), None) | (None, Some(Some(_))) => {
            return Err(VizWriteError::PreviewKindInvalid);
        }
        (None, None) => {}
    }

    am.update(db).await.map_err(|_| VizWriteError::Db)
}

pub async fn delete_pin(
    db: &DatabaseConnection,
    org_id: i64,
    pin: map_pins::Model,
) -> Result<(), DbErr> {
    // D42 : purge **symétrique** de la couche d'organisation (labels +
    // containment + arêtes des DEUX bouts) — remplace delete_links_for.
    crate::services::resources::purge_for(db, org_id, "map_pin", &pin.id.to_string()).await?;
    map_pins::Entity::delete_by_id(pin.id)
        .exec(db)
        .await
        .map(|_| ())
}

// ─────────────────────────── listes + clustering ───────────────────────────

/// Filtres communs liste / cluster.
#[derive(Default)]
pub struct PinFilters<'a> {
    pub search: Option<&'a str>,
    pub emoji: Option<&'a str>,
    pub has_device: bool,
    pub has_position: bool,
}

/// SQL select of the org's geo POIs with every filter pushed down: bbox,
/// emoji, `has_device` (a placement exists), `has_position` (a placed
/// device has a position) and the multi-field search (POI label/location,
/// placed device slugs and their location) — no whole-org placement or
/// position load, the map cluster is polled.
fn filtered_pins_select(
    org_id: i64,
    bbox: Option<(f64, f64, f64, f64)>,
    f: &PinFilters<'_>,
) -> sea_orm::Select<map_pins::Entity> {
    use sea_orm::sea_query::{Expr, ExprTrait, Func, LikeExpr, Query};
    use sea_orm::Condition;
    let mut q = map_pins::Entity::find()
        .filter(map_pins::Column::OrgId.eq(org_id))
        .filter(map_pins::Column::Mode.eq(PIN_MODE_GEO))
        .filter(map_pins::Column::Latitude.is_not_null())
        .filter(map_pins::Column::Longitude.is_not_null())
        .order_by_desc(map_pins::Column::CreatedAt);
    if let Some((west, south, east, north)) = bbox {
        q = q
            .filter(map_pins::Column::Longitude.gte(west))
            .filter(map_pins::Column::Longitude.lte(east))
            .filter(map_pins::Column::Latitude.gte(south))
            .filter(map_pins::Column::Latitude.lte(north));
    }
    if let Some(emoji) = f.emoji {
        q = q.filter(map_pins::Column::Emoji.eq(emoji));
    }
    // POIs holding a placement of the org matching `cond`.
    let pins_with_placement = |cond: Option<sea_orm::sea_query::SimpleExpr>| {
        let mut sub = Query::select();
        sub.column((device_placements::Entity, device_placements::Column::PinId))
            .from(device_placements::Entity)
            .and_where(
                Expr::col((device_placements::Entity, device_placements::Column::OrgId)).eq(org_id),
            );
        if let Some(c) = cond {
            sub.and_where(c);
        }
        sub.to_owned()
    };
    if f.has_device {
        q = q.filter(map_pins::Column::Id.in_subquery(pins_with_placement(None)));
    }
    if f.has_position {
        let positioned = Query::select()
            .column((device_positions::Entity, device_positions::Column::DeviceId))
            .from(device_positions::Entity)
            .and_where(
                Expr::col((device_positions::Entity, device_positions::Column::OrgId)).eq(org_id),
            )
            .to_owned();
        q = q.filter(
            map_pins::Column::Id.in_subquery(pins_with_placement(Some(
                Expr::col((
                    device_placements::Entity,
                    device_placements::Column::DeviceId,
                ))
                .in_subquery(positioned),
            ))),
        );
    }
    if let Some(term) = f.search.map(str::trim).filter(|t| !t.is_empty()) {
        let mut pat = String::from("%");
        for c in term.to_lowercase().chars() {
            if matches!(c, '\\' | '%' | '_') {
                pat.push('\\');
            }
            pat.push(c);
        }
        pat.push('%');
        let like = |col: sea_orm::sea_query::ColumnRef| {
            Expr::expr(Func::lower(Expr::col(col))).like(LikeExpr::new(pat.clone()).escape('\\'))
        };
        use sea_orm::sea_query::IntoColumnRef;
        q = q.filter(
            Condition::any()
                .add(like(
                    (map_pins::Entity, map_pins::Column::Label).into_column_ref(),
                ))
                .add(like(
                    (map_pins::Entity, map_pins::Column::LocationDetail).into_column_ref(),
                ))
                .add(
                    map_pins::Column::Id.in_subquery(pins_with_placement(Some(
                        Condition::any()
                            .add(like(
                                (
                                    device_placements::Entity,
                                    device_placements::Column::DeviceId,
                                )
                                    .into_column_ref(),
                            ))
                            .add(like(
                                (
                                    device_placements::Entity,
                                    device_placements::Column::LocationDetail,
                                )
                                    .into_column_ref(),
                            ))
                            .into(),
                    ))),
                ),
        );
    }
    q
}

/// POI géo de l'org, filtrés bbox optionnelle (all filters in SQL).
pub(super) async fn filtered_pins(
    db: &DatabaseConnection,
    org_id: i64,
    bbox: Option<(f64, f64, f64, f64)>,
    f: &PinFilters<'_>,
) -> Result<Vec<map_pins::Model>, DbErr> {
    filtered_pins_select(org_id, bbox, f).all(db).await
}

/// One page of the filtered POIs + the filtered total (D14): COUNT and
/// OFFSET/LIMIT run in SQL.
pub async fn list_pins(
    db: &DatabaseConnection,
    org_id: i64,
    f: &PinFilters<'_>,
    offset: u64,
    limit: u64,
) -> Result<(i64, Vec<map_pins::Model>), DbErr> {
    use sea_orm::{PaginatorTrait, QuerySelect};
    let select = filtered_pins_select(org_id, None, f);
    let count = select.clone().count(db).await? as i64;
    if offset as i64 >= count {
        return Ok((count, Vec::new()));
    }
    let rows = select.offset(offset).limit(limit).all(db).await?;
    Ok((count, rows))
}
