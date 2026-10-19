use super::*;

// ─────────────────────────────── Kinds ───────────────────────────────

/// `GET /api/v1/notify/kinds`: channel kinds + `field_spec` (source of the
/// frontend's generated form). Static catalogue, nothing org-specific,
/// hence public.
#[utoipa::path(
    get,
    path = "/api/v1/notify/kinds",
    tag = "notify",
    operation_id = "notify_kinds",
    summary = "List the notification channel kinds",
    description = "Paginated catalogue of the channel kinds the server can send to (websocket, webhook, ...) with the `field_spec` describing each kind's configuration form. Static and identical for every organisation (it exposes nothing org-specific), so no authentication is required.",
    params(PageQuery),
    responses((status = 200, body = pnex_core::Paginated<pnex_core::NotifyKindInfo>)),
    security(())
)]
pub(super) async fn kinds(Query(q): Query<PageQuery>) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let all = pnex_notify::kinds();
    let count = all.len() as i64;
    let (skip, take) = page.slice(all.len());
    let results: Vec<_> = all.into_iter().skip(skip).take(take).collect();
    Ok(format::json(pagination::envelope(
        "/api/v1/notify/kinds",
        &[],
        page,
        count,
        results,
    ))
    .into_response())
}

/// `limit`/`offset` of a plain paginated list.
#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct PageQuery {
    #[param(value_type = Option<i64>)]
    pub(super) limit: Option<String>,
    #[param(value_type = Option<i64>)]
    pub(super) offset: Option<String>,
}

// ─────────────────────────────── Canaux ───────────────────────────────

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct ListQuery {
    #[param(value_type = Option<i64>)]
    pub(super) limit: Option<String>,
    #[param(value_type = Option<i64>)]
    pub(super) offset: Option<String>,
    /// Label filter `name` / `name:value` (D218).
    pub(super) label: Option<String>,
    /// Name contains (case-insensitive).
    pub(super) search: Option<String>,
}

/// `GET /api/v1/notify/channels` — canaux de l'org (secrets masqués).
#[utoipa::path(
    get,
    path = "/api/v1/notify/channels",
    tag = "notify",
    operation_id = "notify_channels_list",
    summary = "List the notification channels of the organisation",
    description = "Paginated, newest first. Secret configuration fields are never returned: `secrets_set` lists which are set and `secrets` gives their vault reference. `last_status`/`last_delivery_at` come from the delivery journal.",
    params(ListQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::NotifyChannel>),
        (status = 400, description = "`field-invalid`: invalid `label` filter", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn list_channels(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let mut select = NotifyChannels::find().filter(notify_channels::Column::OrgId.eq(org.org.id));
    if let Some(pat) = pagination::sql_search_pattern(q.search.as_deref()) {
        select = select.filter(pagination::sql_contains(
            (notify_channels::Entity, notify_channels::Column::Name),
            &pat,
        ));
    }
    match pagination::label_ids(
        &ctx.db,
        org.org.id,
        q.label.as_deref(),
        pnex_core::resources::KIND_NOTIFY_CHANNEL,
    )
    .await
    {
        Ok(None) => {}
        Ok(Some(ids)) => {
            select = select
                .filter(notify_channels::Column::Id.is_in(pagination::parse_ids::<Uuid>(&ids)))
        }
        Err(resp) => return Ok(resp),
    }
    let rows = select
        .order_by_desc(notify_channels::Column::Id)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let last = notify_journal::last_status_by_channel(&ctx, org.org.id, &ids).await;
    let mut results: Vec<NotifyChannel> = Vec::with_capacity(rows.len());
    for m in &rows {
        results.push(channel_dto(
            m,
            last.get(&m.id),
            secret_views(&ctx.db, m).await?,
        ));
    }
    let count = results.len() as i64;
    let (skip, take) = page.slice(count as usize);
    let results: Vec<NotifyChannel> = results.into_iter().skip(skip).take(take).collect();
    Ok(format::json(pagination::envelope(
        "/api/v1/notify/channels",
        &[],
        page,
        count,
        results,
    ))
    .into_response())
}

/// `GET /api/v1/notify/channels/{id}`.
#[utoipa::path(
    get,
    path = "/api/v1/notify/channels/{id}",
    tag = "notify",
    operation_id = "notify_channels_get",
    summary = "Get a notification channel",
    description = "One channel of the organisation, secret fields masked. 404 when it does not exist or belongs to another organisation.",
    params(("id" = String, Path, format = "uuid", description = "Channel id")),
    responses(
        (status = 200, body = pnex_core::NotifyChannel),
        (status = 404, body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn channel_detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let Some(m) = find_channel(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let last = notify_journal::last_status_by_channel(&ctx, org.org.id, &[m.id]).await;
    let views = secret_views(&ctx.db, &m).await?;
    Ok(format::json(channel_dto(&m, last.get(&m.id), views)).into_response())
}

pub(super) fn validate_channel_input(
    kind: &str,
    name: &str,
    config: &serde_json::Value,
) -> Option<Response> {
    let name = name.trim();
    if name.is_empty() {
        return Some(field_status("name", err_codes::FIELD_REQUIRED));
    }
    if name.chars().count() > 200 {
        return Some(field_status(
            "name",
            &format!("{}:200", err_codes::FIELD_MAX_LENGTH),
        ));
    }
    let Some(ch) = pnex_notify::channel(kind) else {
        return Some(field_status("kind", err_codes::FIELD_INVALID));
    };
    if let Err(detail) = ch.validate(config) {
        return Some(
            crate::controllers::coded_error(
                StatusCode::BAD_REQUEST,
                err_codes::NOTIFY_CHANNEL_CONFIG_INVALID,
                format!("Invalid channel configuration: {detail}"),
                Some(serde_json::json!({ "detail": detail })),
            )
            .into_response(),
        );
    }
    // Every required field of the kind's field_spec is present.
    for f in ch.field_spec() {
        if f.required && config.get(&f.id).is_none_or(|v| v.is_null()) {
            return Some(
                crate::controllers::coded_error(
                    StatusCode::BAD_REQUEST,
                    err_codes::NOTIFY_CHANNEL_FIELD_REQUIRED,
                    format!("The field {} is required for this channel kind", f.id),
                    Some(serde_json::json!({ "field": f.id })),
                )
                .into_response(),
            );
        }
    }
    None
}

/// Vérifie l'unicité (org, name) — **409** (divergence documentée en tête
/// de fichier : clé naturelle, l'UI propose « renommer »).
async fn ensure_name_free(db: &DatabaseConnection, org_id: i64, name: &str) -> Result<()> {
    let clash = NotifyChannels::find()
        .filter(notify_channels::Column::OrgId.eq(org_id))
        .filter(notify_channels::Column::Name.eq(name))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .is_some();
    if clash {
        return Err(conflict(
            "notify-channel-name-conflict",
            "A channel already uses this name.",
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub(super) struct ChannelInput {
    kind: String,
    name: String,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    config: serde_json::Value,
}

/// `POST /api/v1/notify/channels` — création (secrets write-only).
#[utoipa::path(
    post,
    path = "/api/v1/notify/channels",
    tag = "notify",
    operation_id = "notify_channels_create",
    summary = "Create a notification channel",
    description = "Creates a channel (destination of notifications). Secret fields are write-only: a typed value is stored in the vault as the channel's dedicated secret, `{\"secret_id\"}` references an existing org secret (choosing secrets requires the secret-manager role). Requires owner, admin or member role. Channel names are unique per organisation (409).",
    request_body = ChannelInput,
    responses(
        (status = 201, body = pnex_core::NotifyChannel),
        (status = 400, description = "`field-invalid` (`name`: `required`, `max_length:200`; `kind`: `invalid`), `notify-channel-config-invalid` (`args.detail`), `notify-channel-field-required` (`args.field`) or a vault error", body = crate::openapi::ErrorDetail),
        (status = 403, description = "A vault secret the caller may not use", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`notify-channel-name-conflict`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn create_channel(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(params): Json<ChannelInput>,
) -> Result<Response> {
    // D313: recipients from flow messages are an admin decision.
    if crate::services::notify_recipients::dynamic(&params.config) && !org.can("notify:broadcast") {
        return Err(forbidden(
            pnex_core::err_codes::NOTIFY_DYNAMIC_RECIPIENTS_ADMIN,
            "Only owners and admins let a channel send to recipients read from flow messages.",
        ));
    }
    let name = params.name.trim().to_string();
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(ring) => ring,
        Err(e) => return vault_error(e),
    };
    let plan = match secrets::notify::plan(
        &ctx.db,
        &ring,
        org.org.id,
        &params.kind,
        &params.config,
        (!org.can("secrets:write")).then_some(&[][..]),
    )
    .await
    {
        Ok(plan) => plan,
        Err(e) => return vault_error(e),
    };
    if let Some(resp) = validate_channel_input(&params.kind, &name, &plan.sendable) {
        return Ok(resp);
    }
    ensure_name_free(&ctx.db, org.org.id, &name).await?;
    let id = Uuid::new_v4();
    let txn = ctx
        .db
        .begin()
        .await
        .map_err(|_| Error::InternalServerError)?;
    let config = match secrets::notify::commit(
        &txn,
        &ring,
        crate::controllers::secrets::writer(&org),
        org.can("secrets:write"),
        id,
        &name,
        None,
        plan,
    )
    .await
    {
        Ok(config) => config,
        Err(e) => return vault_error(e),
    };
    let m = notify_channels::ActiveModel {
        id: Set(id),
        org_id: Set(org.org.id),
        kind: Set(params.kind),
        name: Set(name),
        config: Set(config),
        enabled: Set(params.enabled),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .map_err(|_| Error::InternalServerError)?;
    txn.commit().await.map_err(|_| Error::InternalServerError)?;
    let views = secret_views(&ctx.db, &m).await?;
    Ok((
        StatusCode::CREATED,
        format::json(channel_dto(&m, None, views)),
    )
        .into_response())
}

/// `PUT /api/v1/notify/channels/{id}` — renommage/toggle/config ; les
/// secrets absents ou `null` sont conservés (merge registre, D54).
/// Secret fields are vault references once stored (lot S4).
#[utoipa::path(
    put,
    path = "/api/v1/notify/channels/{id}",
    tag = "notify",
    operation_id = "notify_channels_update",
    summary = "Replace a notification channel",
    description = "Full update of name, kind, enabled flag and config. Secret fields sent absent or `null` keep their stored value when the kind is unchanged; a changed kind replaces the whole config. Requires owner, admin or member role.",
    params(("id" = String, Path, format = "uuid", description = "Channel id")),
    request_body = ChannelInput,
    responses(
        (status = 200, body = pnex_core::NotifyChannel),
        (status = 400, description = "`field-invalid` (`name`: `required`, `max_length:200`; `kind`: `invalid`), `notify-channel-config-invalid` (`args.detail`), `notify-channel-field-required` (`args.field`) or a vault error", body = crate::openapi::ErrorDetail),
        (status = 403, description = "A vault secret the caller may not use", body = crate::openapi::ErrorDetail),
        (status = 404, body = crate::openapi::ErrorDetail),
        (status = 409, description = "`notify-channel-name-conflict`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn update_channel(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(params): Json<ChannelInput>,
) -> Result<Response> {
    let Some(m) = find_channel(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    // D313: turning recipients from flow messages on is an admin decision.
    use crate::services::notify_recipients::dynamic;
    if dynamic(&params.config) && !dynamic(&m.config) && !org.can("notify:broadcast") {
        return Err(forbidden(
            pnex_core::err_codes::NOTIFY_DYNAMIC_RECIPIENTS_ADMIN,
            "Only owners and admins let a channel send to recipients read from flow messages.",
        ));
    }
    let name = params.name.trim().to_string();
    // Merge des secrets seulement si le kind est stable (sinon remplacement
    // complet : rien à « garder » d'un ancien kind).
    let merged = if params.kind == m.kind {
        pnex_notify::merge_config(&m.kind, &m.config, &params.config)
    } else {
        params.config.clone()
    };
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(ring) => ring,
        Err(e) => return vault_error(e),
    };
    let held = secrets::notify::held_bindings(&m.kind, &m.config);
    let plan = match secrets::notify::plan(
        &ctx.db,
        &ring,
        org.org.id,
        &params.kind,
        &merged,
        (!org.can("secrets:write")).then_some(held.as_slice()),
    )
    .await
    {
        Ok(plan) => plan,
        Err(e) => return vault_error(e),
    };
    if let Some(resp) = validate_channel_input(&params.kind, &name, &plan.sendable) {
        return Ok(resp);
    }
    if name != m.name {
        ensure_name_free(&ctx.db, org.org.id, &name).await?;
    }
    let txn = ctx
        .db
        .begin()
        .await
        .map_err(|_| Error::InternalServerError)?;
    let config = match secrets::notify::commit(
        &txn,
        &ring,
        crate::controllers::secrets::writer(&org),
        org.can("secrets:write"),
        m.id,
        &name,
        Some((&m.kind, &m.name, &m.config)),
        plan,
    )
    .await
    {
        Ok(config) => config,
        Err(e) => return vault_error(e),
    };
    let mut active: notify_channels::ActiveModel = m.into();
    active.kind = Set(params.kind);
    active.name = Set(name);
    active.config = Set(config);
    active.enabled = Set(params.enabled);
    let updated = active.update(&txn).await.map_err(|e| {
        tracing::error!("update_channel DB : {e}");
        Error::InternalServerError
    })?;
    txn.commit().await.map_err(|_| Error::InternalServerError)?;
    let last = notify_journal::last_status_by_channel(&ctx, org.org.id, &[updated.id]).await;
    let views = secret_views(&ctx.db, &updated).await?;
    Ok(format::json(channel_dto(&updated, last.get(&updated.id), views)).into_response())
}

/// `DELETE /api/v1/notify/channels/{id}` — 204 ; the O2 journal entries of
/// the channel stay until O2 retention drops them (D86).
#[utoipa::path(
    delete,
    path = "/api/v1/notify/channels/{id}",
    tag = "notify",
    operation_id = "notify_channels_delete",
    summary = "Delete a notification channel",
    description = "Deletes the channel and releases its vault usages (its dedicated secrets are removed when nothing else uses them). Journal entries stay until retention drops them. Requires owner, admin or member role.",
    params(("id" = String, Path, format = "uuid", description = "Channel id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, body = crate::openapi::ErrorDetail),
        (status = 404, body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn delete_channel(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let Some(m) = find_channel(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let txn = ctx
        .db
        .begin()
        .await
        .map_err(|_| Error::InternalServerError)?;
    // Usages first, then the dedicated secrets left without consumer.
    if let Err(e) = secrets::notify::release(&txn, org.org.id, m.id, &m.name).await {
        return vault_error(e);
    }
    m.into_active_model()
        .delete(&txn)
        .await
        .map_err(|_| Error::InternalServerError)?;
    txn.commit().await.map_err(|_| Error::InternalServerError)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
