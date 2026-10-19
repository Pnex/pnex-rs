use super::*;

// ─────────────────────────── POI ───────────────────────────

/// `GET /api/v1/pois` — liste globale org, D14.
#[utoipa::path(
    get,
    path = "/api/v1/pois",
    tag = "pois",
    operation_id = "pois_list",
    summary = "List the POIs of the organisation",
    description = "Paginated (`limit`/`offset`). Each POI carries its placed devices and its outgoing links with resolved target names. Filters: `search`, `emoji`, `has_device`, `has_position`, `label`.",
    params(PoiListQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<PoiDto>),
        (status = 400, description = "Invalid `label` filter", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PoiListQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let mut filters = pin_filters(&q.search, &q.emoji, &q.has_device, &q.has_position);
    match pagination::label_ids(
        &ctx.db,
        org.org.id,
        q.label.as_deref(),
        pnex_core::resources::KIND_MAP_PIN,
    )
    .await
    {
        Ok(None) => {}
        Ok(Some(ids)) => filters.ids = Some(pagination::parse_ids::<uuid::Uuid>(&ids)),
        Err(resp) => return Ok(resp),
    }
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
#[utoipa::path(
    post,
    path = "/api/v1/pois",
    tag = "pois",
    operation_id = "pois_create",
    summary = "Create a POI",
    description = "Creates a geographic POI (label and coordinates required). Devices are attached afterwards with `POST /pois/{id}/devices`. Requires owner, admin or member role.",
    request_body = PoiPayload,
    responses(
        (status = 201, body = PoiDto),
        (status = 400, description = "`field-invalid`: `errors.fields` maps `label`, `coordinates`, `latitude`, `longitude`, `emoji`, `location_detail`, `preview_kind` to a token", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(p): Json<PoiPayload>,
) -> Result<Response> {
    let Some(label) = p.label.as_deref() else {
        return Ok(field_status("label", err_codes::FIELD_REQUIRED));
    };
    let (Some(latitude), Some(longitude)) = (p.latitude, p.longitude) else {
        return Ok(field_status("coordinates", err_codes::FIELD_REQUIRED));
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
#[utoipa::path(
    get,
    path = "/api/v1/pois/cluster",
    tag = "pois",
    operation_id = "pois_cluster",
    summary = "Cluster the POIs of a map viewport",
    description = "Returns the POIs inside `bbox` at `zoom`: individual points when few enough, otherwise numbered clusters (centroid + sample). Same filters as the list. Read-only.",
    params(ClusterQuery),
    responses(
        (status = 200, body = ClusterResponse),
        (status = 400, description = "Missing or malformed `bbox` / `zoom`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn cluster(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ClusterQuery>,
) -> Result<Response> {
    let Some(bbox) = q.bbox.as_deref() else {
        return Ok(field_status("bbox", err_codes::FIELD_REQUIRED));
    };
    let Ok(zoom) = q.zoom.as_deref().unwrap_or_default().parse::<i32>() else {
        return Ok(field_status("zoom", err_codes::FIELD_INVALID));
    };
    let parts: Vec<f64> = bbox
        .split(',')
        .filter_map(|p| p.trim().parse::<f64>().ok())
        .collect();
    let [west, south, east, north] = parts[..] else {
        return Ok(field_status("bbox", err_codes::FIELD_INVALID));
    };
    if !(west < east && south < north) {
        return Ok(field_status("bbox", err_codes::FIELD_INVALID));
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

#[utoipa::path(
    get,
    path = "/api/v1/pois/{id}",
    tag = "pois",
    operation_id = "pois_get",
    summary = "Get a POI",
    description = "One POI with its placed devices and resolved links.",
    params(("id" = String, Path, format = "uuid", description = "POI id (UUID)")),
    responses(
        (status = 200, body = PoiDto),
        (status = 404, description = "Unknown POI or POI of another org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
#[utoipa::path(
    patch,
    path = "/api/v1/pois/{id}",
    tag = "pois",
    operation_id = "pois_update",
    summary = "Update a POI",
    description = "Partial update (move, rename, emoji, metadata, pinned preview). For nullable fields: absent = unchanged, `null` = cleared. Requires owner, admin or member role.",
    params(("id" = String, Path, format = "uuid", description = "POI id (UUID)")),
    request_body = PoiPatchPayload,
    responses(
        (status = 200, body = PoiDto),
        (status = 400, description = "`field-invalid`: `errors.fields` maps `label`, `coordinates`, `latitude`, `longitude`, `emoji`, `location_detail`, `preview_kind` to a token", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown POI or POI of another org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(p): Json<PoiPatchPayload>,
) -> Result<Response> {
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

#[utoipa::path(
    delete,
    path = "/api/v1/pois/{id}",
    tag = "pois",
    operation_id = "pois_delete",
    summary = "Delete a POI",
    description = "Deletes the POI; its device placements and links are removed with it (devices become free again). Requires owner, admin or member role.",
    params(("id" = String, Path, format = "uuid", description = "POI id (UUID)")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Unknown POI or POI of another org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn delete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let pin = svc::find_pin(&ctx.db, org.org.id, id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let pin = or_not_found(pin)?;
    svc::delete_pin(&ctx.db, org.org.id, pin)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
