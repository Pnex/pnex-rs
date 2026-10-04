use super::*;

// ─────────────────────────────── Helpers ───────────────────────────────

pub(super) async fn find_channel(
    db: &DatabaseConnection,
    org: &OrgContext,
    id: Uuid,
) -> Result<Option<notify_channels::Model>> {
    NotifyChannels::find_by_id(id)
        .filter(notify_channels::Column::OrgId.eq(org.org.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)
}

pub(super) async fn find_template(
    db: &DatabaseConnection,
    org: &OrgContext,
    id: Uuid,
) -> Result<Option<notify_templates::Model>> {
    NotifyTemplates::find_by_id(id)
        .filter(notify_templates::Column::OrgId.eq(org.org.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)
}

pub(super) fn channel_dto(
    m: &notify_channels::Model,
    last: Option<&(String, String)>,
    secrets: std::collections::BTreeMap<String, pnex_core::SecretFieldView>,
) -> NotifyChannel {
    NotifyChannel {
        id: m.id,
        org_id: m.org_id,
        kind: m.kind.clone(),
        name: m.name.clone(),
        enabled: m.enabled,
        config: pnex_notify::mask_config(&m.kind, &m.config),
        secrets_set: pnex_notify::secrets_set(&m.kind, &m.config),
        secrets,
        last_status: last.map(|(s, _)| s.clone()),
        last_delivery_at: last.map(|(_, t)| t.clone()),
        created_at: m.created_at.to_rfc3339(),
        updated_at: m.updated_at.to_rfc3339(),
    }
}

/// Vault references of a channel, as shown by its form.
pub(super) async fn secret_views(
    db: &DatabaseConnection,
    m: &notify_channels::Model,
) -> Result<std::collections::BTreeMap<String, pnex_core::SecretFieldView>> {
    secrets::notify::views(db, m.org_id, &m.kind, &m.config)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "notify channel secret names");
            Error::InternalServerError
        })
}

/// Maps a vault error of a channel save to its API response.
pub(super) fn vault_error(e: secrets::store::StoreError) -> Result<Response> {
    crate::controllers::secrets::store_error(e)
}

pub(super) fn template_dto(m: notify_templates::Model) -> NotifyTemplate {
    let vars: Vec<TemplateVar> = serde_json::from_value(m.vars).unwrap_or_default();
    NotifyTemplate {
        id: m.id,
        org_id: m.org_id,
        name: m.name,
        subject: m.subject,
        body: m.body,
        vars,
        created_at: m.created_at.to_rfc3339(),
        updated_at: m.updated_at.to_rfc3339(),
    }
}
