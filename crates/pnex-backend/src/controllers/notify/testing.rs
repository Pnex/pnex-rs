use super::*;

/// Message de test built-in (boutons Test — canal existant ou brouillon).
fn test_message(name: &str) -> pnex_notify::Message {
    pnex_notify::Message {
        subject: Some(format!("PNeX test — {name}")),
        body: format!("Test message sent from PNeX for channel \"{name}\"."),
        meta: serde_json::json!({ "test": true, "ts": chrono::Utc::now().to_rfc3339() }),
    }
}

/// Envoi d'un test + journalisation (source `test`) — contourne
/// volontairement `enabled: false` (c'est son but, D53). Le message est
/// fourni par l'appelant : built-in (canal) ou template rendu (test d'un
/// template via un canal).
#[allow(clippy::too_many_arguments)] // private test helper, call sites stay explicit
async fn send_test(
    settings: &NotifySettings,
    org_id: i64,
    channel_id: Uuid,
    template_id: Option<Uuid>,
    kind: &str,
    config: &serde_json::Value,
    msg: pnex_notify::Message,
) -> Result<Response> {
    let Some(ch) = pnex_notify::channel(kind) else {
        return Ok(field_status("kind", "Type de canal inconnu."));
    };
    // Canal websocket : coordonnées injectées (loopback interne), jamais
    // stockées — le test exerce le chemin complet endpoint → broker → WS.
    let effective_config = if kind == "websocket" {
        notify::in_app_runtime_config(settings, org_id, channel_id)
    } else {
        config.clone()
    };
    let outcome = ch.send(&effective_config, &msg).await;
    // The websocket loopback journals its own success (deliver endpoint);
    // everything else — and a loopback that failed — is journaled here.
    if kind != "websocket" || outcome.is_err() {
        let (status, http_status, error) = match &outcome {
            Ok(()) => (pnex_core::DELIVERY_SENT, None, None),
            Err(e) => (
                pnex_core::DELIVERY_FAILED,
                match e {
                    pnex_notify::NotifyError::Http { status } => Some(*status),
                    _ => None,
                },
                Some(e.to_string()),
            ),
        };
        notify_journal::submit(pnex_core::NotifyDeliveryEntry {
            org_id,
            channel_id,
            channel_kind: kind.to_string(),
            template_id,
            source: "test".into(),
            status: status.into(),
            http_status,
            error,
            subject: msg.subject.clone(),
            ..Default::default()
        });
    }
    match outcome {
        Ok(()) => Ok(format::json(serde_json::json!({ "status": "sent" })).into_response()),
        Err(e) => Ok((
            StatusCode::BAD_GATEWAY,
            format::json(serde_json::json!({ "status": "failed", "error": e.to_string() })),
        )
            .into_response()),
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct TestDraftInput {
    kind: String,
    name: Option<String>,
    #[serde(default)]
    config: serde_json::Value,
    /// Channel being edited: its stored secrets fill the fields left
    /// `null` (unchanged) in the draft.
    #[serde(default)]
    channel_id: Option<Uuid>,
}

/// `POST /api/v1/notify/channels/test-draft` — teste un brouillon de canal
/// **avant sauvegarde** (pas de ligne DB, journal sur channel_id nil
/// impossible → journalisation sans FK : on journalise quand même la
/// tentative avec un channel_id zéro).
pub(super) async fn test_draft(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(params): Json<TestDraftInput>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "notify-write-forbidden",
            "Owner, admin or member role required to manage notifications.",
        ));
    }
    let name = params.name.unwrap_or_else(|| "draft".to_string());
    let existing = match params.channel_id {
        Some(id) => find_channel(&ctx.db, &org, id)
            .await?
            .filter(|m| m.kind == params.kind),
        None => None,
    };
    let merged = match &existing {
        Some(m) => pnex_notify::merge_config(&m.kind, &m.config, &params.config),
        None => params.config.clone(),
    };
    // Picked secrets and typed values → plaintext, memory only (nothing
    // is written to the vault by a test).
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(ring) => ring,
        Err(e) => return vault_error(e),
    };
    let plan = match secrets::notify::plan(
        &ctx.db,
        &ring,
        org.org.id,
        &params.kind,
        existing.as_ref().map(|m| &m.config),
        &merged,
    )
    .await
    {
        Ok(plan) => plan,
        Err(e) => return vault_error(e),
    };
    if let Some(resp) = validate_channel_input(&params.kind, &name, &plan.sendable) {
        return Ok(resp);
    }
    // Brouillon sans id : le journal exige un canal existant (FK) — on
    // considère le test-draft comme hors journal (résultat éphémère au
    // toast), les canaux persistés ont le leur.
    let Some(ch) = pnex_notify::channel(&params.kind) else {
        return Ok(field_status("kind", "Type de canal inconnu."));
    };
    let settings = NotifySettings::from_config(&ctx.config);
    let effective_config = if params.kind == "websocket" {
        notify::in_app_runtime_config(&settings, org.org.id, Uuid::nil())
    } else {
        plan.sendable
    };
    let msg = test_message(&name);
    match ch.send(&effective_config, &msg).await {
        Ok(()) => Ok(format::json(serde_json::json!({ "status": "sent" })).into_response()),
        Err(e) => Ok((
            StatusCode::BAD_GATEWAY,
            format::json(serde_json::json!({ "status": "failed", "error": e.to_string() })),
        )
            .into_response()),
    }
}

/// Body optionnel de `POST /channels/{id}/test` : avec `template_id` le
/// message built-in est remplacé par le template rendu (les vars manquantes
/// prennent leur `example` déclaré, école preview).
#[derive(Debug, Default, Deserialize)]
pub(super) struct TestChannelBody {
    template_id: Option<Uuid>,
    #[serde(default)]
    vars: std::collections::BTreeMap<String, String>,
}

/// `POST /api/v1/notify/channels/{id}/test` — message built-in sur le canal
/// persisté (journalisé source `test`), contourne `enabled: false`. Avec un
/// `template_id` : envoie le template rendu à la place (journal tient le
/// template_id — le test exerce le rendu ET l'envoi). Le body est lu en
/// `Bytes` et parsé à la main : `Option<Json<T>>` rejette un body vide dès
/// qu'un `Content-Type: application/json` est présent (défaut injecté par
/// les harnais de test), et les headers `Content-Type` dupliqués font
/// échouer le sniffing du Json extractor.
pub(super) async fn test_channel(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    raw: axum::body::Bytes,
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
    let body: TestChannelBody = if raw.is_empty() {
        TestChannelBody::default()
    } else {
        serde_json::from_slice(&raw).map_err(|e| {
            Error::CustomError(
                StatusCode::BAD_REQUEST,
                loco_rs::controller::ErrorDetail::new(
                    err_codes::NOTIFY_TEST_BODY_INVALID,
                    format!("Invalid JSON body: {e}"),
                ),
            )
        })?
    };
    let TestChannelBody { template_id, vars } = body;
    let settings = NotifySettings::from_config(&ctx.config);
    // Vault references → values, in memory only (lot S4).
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(ring) => ring,
        Err(e) => return vault_error(e),
    };
    let config =
        match secrets::notify::sendable(&ctx.db, &ring, org.org.id, &m.kind, &m.config).await {
            Ok(config) => config,
            Err(e) => return vault_error(e),
        };
    let Some(template_id) = template_id else {
        return send_test(
            &settings,
            org.org.id,
            m.id,
            None,
            &m.kind,
            &config,
            test_message(&m.name),
        )
        .await;
    };
    let Some(tpl) = find_template(&ctx.db, &org, template_id).await? else {
        return Err(Error::NotFound);
    };
    // Même école que le preview : les vars manquantes prennent leur example
    // déclaré, les overrides fournis gagnent. Un example **vide** n'est pas
    // injecté : une var déclarée vide masquerait la clé payload de même nom
    // au niveau racine du rendu (vars > payload dans le contexte).
    let mut vars = vars;
    let declared: Vec<TemplateVar> = serde_json::from_value(tpl.vars.clone()).unwrap_or_default();
    for v in declared {
        if !v.example.is_empty() {
            vars.entry(v.name).or_insert(v.example);
        }
    }
    let rendered = pnex_notify::render(
        tpl.subject.as_deref(),
        &tpl.body,
        &vars,
        serde_json::json!({}),
        serde_json::json!({ "test": true, "ts": chrono::Utc::now().to_rfc3339() }),
    )
    .map_err(|e| template_render_error(&e))?;
    send_test(
        &settings,
        org.org.id,
        m.id,
        Some(tpl.id),
        &m.kind,
        &config,
        rendered,
    )
    .await
}
