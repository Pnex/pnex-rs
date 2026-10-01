use super::*;

// ─────────────────────────── POI ───────────────────────────

/// `GET /api/v1/pois` — liste globale org, D14.
pub(super) async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PoiListQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let filters = pin_filters(&q.search, &q.emoji, &q.has_device, &q.has_position);
    let (count, page_pins) = svc::list_pins(
        &ctx.db,
        org.org.id,
        &filters,
        page.offset as u64,
        page.limit as u64,
    )
    .await
    .map_err(|_| Error::InternalServerError)?;

    // Liens de la page en UNE requête (pas de N+1), noms de cibles en un
    // lot par kind (échec d'hydratation = lignes grises, jamais 500).
    // Placements devices (D43) batchés pareillement pour la page.
    let pin_ids: Vec<Uuid> = page_pins.iter().map(|p| p.id).collect();
    let links = svc::links_for_pins(&ctx.db, org.org.id, &pin_ids)
        .await
        .unwrap_or_default();
    let labels = svc::link_target_labels(&ctx.db, org.org.id, &links)
        .await
        .ok();
    let placements = svc::placements_for_pins(&ctx.db, org.org.id, &pin_ids)
        .await
        .unwrap_or_default();
    let results: Vec<PoiDto> = page_pins
        .iter()
        .map(|p| poi_dto(p, &placements, &links, labels.as_ref()))
        .collect();
    format::json(pagination::envelope(
        "/api/v1/pois",
        &[],
        page,
        count,
        results,
    ))
}

/// `POST /api/v1/pois` — création (géo uniquement en V1).
pub(super) async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(p): Json<PoiPayload>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "poi-write-forbidden",
            "Owner, admin or member role required to manage POIs",
        ));
    }
    let Some(label) = p.label.as_deref() else {
        return Ok(field_status("label", err_codes::FIELD_REQUIRED));
    };
    let (Some(latitude), Some(longitude)) = (p.latitude, p.longitude) else {
        return Ok(field_status(
            "coordinates",
            "Coordonnées requises pour ce mode.",
        ));
    };
    match svc::create_pin(
        &ctx.db,
        org.org.id,
        svc::NewPin {
            label,
            emoji: p.emoji.as_deref(),
            location_detail: p.location_detail.as_deref(),
            latitude,
            longitude,
            metadata: p.metadata,
        },
    )
    .await
    {
        Ok(pin) => {
            // Un POI tout neuf n'a pas de device (D43 : l'attache se fait
            // ensuite, délibérément, via POST /pois/{id}/devices).
            let dto = poi_dto(&pin, &[], &[], None);
            Ok((StatusCode::CREATED, format::json(dto)).into_response())
        }
        Err(e) => Ok(write_error_response(e)),
    }
}

/// `GET /api/v1/pois/cluster` — items pour le viewport (D37).
pub(super) async fn cluster(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ClusterQuery>,
) -> Result<Response> {
    let Some(bbox) = q.bbox.as_deref() else {
        return Ok(field_status("bbox", err_codes::FIELD_REQUIRED));
    };
    let Ok(zoom) = q.zoom.as_deref().unwrap_or_default().parse::<i32>() else {
        return Ok(field_status("zoom", "Entier attendu (0..22)."));
    };
    let parts: Vec<f64> = bbox
        .split(',')
        .filter_map(|p| p.trim().parse::<f64>().ok())
        .collect();
    let [west, south, east, north] = parts[..] else {
        return Ok(field_status(
            "bbox",
            "Format attendu : west,south,east,north.",
        ));
    };
    if !(west < east && south < north) {
        return Ok(field_status(
            "bbox",
            "bbox invalide (west<east et south<north).",
        ));
    }
    let query = svc::ClusterQuery {
        west,
        south,
        east,
        north,
        zoom,
    };
    let filters = pin_filters(&q.search, &q.emoji, &q.has_device, &q.has_position);
    let items = svc::cluster_pins(&ctx.db, org.org.id, &query, &filters)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let total: usize = items.iter().map(|i| i.count as usize).sum();
    format::json(ClusterResponse {
        zoom,
        total,
        items: items
            .into_iter()
            .map(|i| ClusterItemDto {
                lat: i.lat,
                lon: i.lon,
                count: i.count,
                id: i.id,
                label: i.label,
                emoji: i.emoji,
            })
            .collect(),
    })
}

pub(super) async fn detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let pin = svc::find_pin(&ctx.db, org.org.id, id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let pin = or_not_found(pin)?;
    let links = svc::links_for_pins(&ctx.db, org.org.id, &[pin.id])
        .await
        .unwrap_or_default();
    let labels = svc::link_target_labels(&ctx.db, org.org.id, &links)
        .await
        .ok();
    let placements = svc::placements_for_pins(&ctx.db, org.org.id, &[pin.id])
        .await
        .unwrap_or_default();
    format::json(poi_dto(&pin, &placements, &links, labels.as_ref()))
}

/// `PATCH /api/v1/pois/{id}` — déplacement/édition (1 PATCH par pointer-up).
pub(super) async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(p): Json<PoiPatchPayload>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "poi-write-forbidden",
            "Owner, admin or member role required to manage POIs",
        ));
    }
    let pin = svc::find_pin(&ctx.db, org.org.id, id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let pin = or_not_found(pin)?;
    match svc::update_pin(
        &ctx.db,
        pin,
        svc::UpdatePin {
            label: p.label.as_deref(),
            emoji: p.emoji.as_ref().map(|o| o.as_deref()),
            location_detail: p.location_detail.as_ref().map(|o| o.as_deref()),
            latitude: p.latitude,
            longitude: p.longitude,
            metadata: p.metadata,
            preview_kind: p.preview_kind.as_ref().map(|o| o.as_deref()),
            preview_id: p.preview_id.as_ref().map(|o| o.as_deref()),
        },
    )
    .await
    {
        Ok(pin) => {
            let links = svc::links_for_pins(&ctx.db, org.org.id, &[pin.id])
                .await
                .unwrap_or_default();
            let labels = svc::link_target_labels(&ctx.db, org.org.id, &links)
                .await
                .ok();
            let placements = svc::placements_for_pins(&ctx.db, org.org.id, &[pin.id])
                .await
                .unwrap_or_default();
            format::json(poi_dto(&pin, &placements, &links, labels.as_ref()))
        }
        Err(e) => Ok(write_error_response(e)),
    }
}

pub(super) async fn delete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "poi-write-forbidden",
            "Owner, admin or member role required to manage POIs",
        ));
    }
    let pin = svc::find_pin(&ctx.db, org.org.id, id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let pin = or_not_found(pin)?;
    svc::delete_pin(&ctx.db, org.org.id, pin)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
