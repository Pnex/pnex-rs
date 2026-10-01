use super::*;

// ─────────────────────────── viz_links (D39) ───────────────────────────

/// `GET /api/v1/viz/links?source_kind=map_pin&source_id=…` — thin-wrapper
/// D42 (les nouvelles intégrations passent par `/api/v1/resources/edges`).
pub(super) async fn list_links(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<LinkListQuery>,
) -> Result<Response> {
    let Some(source_id) = q.source_id else {
        return Ok(field_status("source_id", err_codes::FIELD_REQUIRED));
    };
    // D42 : source restreinte au POI (spec du kind `map_pin`, registre).
    if q.source_kind.as_deref().is_some_and(|k| k != "map_pin") {
        return Ok(field_status("source_kind", "Source de lien invalide."));
    }
    let links = svc::links_for_pins(&ctx.db, org.org.id, &[source_id])
        .await
        .map_err(|_| Error::InternalServerError)?;
    let labels = svc::link_target_labels(&ctx.db, org.org.id, &links)
        .await
        .ok();
    format::json(serde_json::json!({
        "results": links.iter().map(|l| link_dto(l, labels.as_ref())).collect::<Vec<_>>()
    }))
}

/// `POST /api/v1/viz/links` — attache une cible (existence org validée
/// **via le registre**, D42). Thin-wrapper : nouvelles intégrations →
/// `/api/v1/resources/edges`.
pub(super) async fn create_link(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(p): Json<LinkPayload>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "poi-link-write-forbidden",
            "Owner, admin or member role required to manage links",
        ));
    }
    let (Some(_source_kind), Some(source_id), Some(target_kind), Some(target_id)) = (
        p.source_kind.as_deref(),
        p.source_id,
        p.target_kind.as_deref(),
        p.target_id,
    ) else {
        return Ok(field_status(
            "source",
            "source_kind, source_id, target_kind et target_id sont requis.",
        ));
    };
    match svc::create_link(
        &ctx.db,
        org.org.id,
        "map_pin",
        source_id,
        target_kind,
        target_id,
        p.label.as_deref(),
    )
    .await
    {
        Ok(link) => {
            // La cible vient d'être validée par le registre : une seule
            // entrée à résoudre (échec = ligne grise, la réponse 201 reste).
            let labels = svc::link_target_labels(&ctx.db, org.org.id, std::slice::from_ref(&link))
                .await
                .ok();
            let dto = link_dto(&link, labels.as_ref());
            Ok((StatusCode::CREATED, format::json(dto)).into_response())
        }
        Err(e) => Ok(write_error_response(e)),
    }
}

/// `DELETE /api/v1/viz/links/{id}` — 204 ; 404 masqué cross-org.
pub(super) async fn delete_link(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "poi-link-write-forbidden",
            "Owner, admin or member role required to manage links",
        ));
    }
    match svc::delete_link(&ctx.db, org.org.id, id)
        .await
        .map_err(|_| Error::InternalServerError)?
    {
        true => Ok(StatusCode::NO_CONTENT.into_response()),
        false => Err(Error::NotFound),
    }
}
