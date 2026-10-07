use super::*;

// ─────────────────────────────── Kinds ───────────────────────────────

/// `GET /api/v1/notify/kinds` — catalogue des canaux + `field_spec`
/// (source du formulaire dynamique front, contrat `notify-kinds.v1`).
pub(super) async fn kinds() -> Result<Response> {
    Ok(format::json(pnex_notify::kinds()).into_response())
}

// ─────────────────────────────── Canaux ───────────────────────────────

#[derive(Debug, Default, Deserialize)]
pub(super) struct ListQuery {
    pub(super) limit: Option<String>,
    pub(super) offset: Option<String>,
}

/// `GET /api/v1/notify/channels` — canaux de l'org (secrets masqués).
pub(super) async fn list_channels(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let rows = NotifyChannels::find()
        .filter(notify_channels::Column::OrgId.eq(org.org.id))
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
        return Some(field_status("kind", "Type de canal inconnu."));
    };
    if let Err(msg) = ch.validate(config) {
        return Some(field_status("config", &msg));
    }
    // Champs requis du field_spec présents ?
    for f in ch.field_spec() {
        if f.required && config.get(&f.id).is_none_or(|v| v.is_null()) {
            return Some(field_status(
                "config",
                &format!("champ requis absent : {}", f.id),
            ));
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

#[derive(Debug, Deserialize)]
pub(super) struct ChannelInput {
    kind: String,
    name: String,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    config: serde_json::Value,
}

/// `POST /api/v1/notify/channels` — création (secrets write-only).
pub(super) async fn create_channel(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(params): Json<ChannelInput>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "notify-write-forbidden",
            "Owner, admin or member role required to manage notifications.",
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
        None,
        &params.config,
        (!org.can_manage_secrets()).then_some(&[][..]),
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
        org.can_manage_secrets(),
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
pub(super) async fn update_channel(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(params): Json<ChannelInput>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "notify-write-forbidden",
            "Owner, admin or member role required to manage notifications.",
        ));
    }
    let Some(m) = find_channel(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
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
        Some(&m.config),
        &merged,
        (!org.can_manage_secrets()).then_some(held.as_slice()),
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
        org.can_manage_secrets(),
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
pub(super) async fn delete_channel(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "notify-write-forbidden",
            "Owner, admin or member role required to manage notifications.",
        ));
    }
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
