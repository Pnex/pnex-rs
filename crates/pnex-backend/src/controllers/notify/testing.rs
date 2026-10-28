use super::*;

/// Answer of a test send: 204 when the message left, else 502
/// `notify-test-failed` with the transport error in `args.detail`.
fn test_answer(outcome: std::result::Result<(), pnex_notify::NotifyError>) -> Result<Response> {
    match outcome {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => {
            let detail = e.to_string();
            Err(crate::controllers::coded_error(
                StatusCode::BAD_GATEWAY,
                err_codes::NOTIFY_TEST_FAILED,
                format!("Test send failed: {detail}"),
                Some(serde_json::json!({ "detail": detail })),
            ))
        }
    }
}

/// Message de test built-in (boutons Test — canal existant ou brouillon).
fn test_message(name: &str) -> pnex_notify::Message {
    pnex_notify::Message {
        subject: Some(format!("PNeX test — {name}")),
        body: format!("Test message sent from PNeX for channel \"{name}\"."),
        meta: serde_json::json!({ "test": true, "ts": chrono::Utc::now().to_rfc3339() }),
    }
}

/// Sends a test + logs it (source `test`) — deliberately bypasses
/// `enabled: false` (that is its purpose, D53). The message is
/// supplied by the caller: built-in (channel) or rendered template (test of a
/// template through a channel).
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
        return Ok(field_status("kind", err_codes::FIELD_INVALID));
    };
    // Websocket channel: injected coordinates (internal loopback), never
    // stored — the test exercises the full endpoint → broker → WS path.
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
    test_answer(outcome)
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub(super) struct TestDraftInput {
    kind: String,
    name: Option<String>,
    #[serde(default)]
    config: serde_json::Value,
    /// Channel being edited: its stored secrets fill the fields left
    /// `null` (unchanged) in the draft.
    #[serde(default)]
    #[schema(value_type = Option<String>)]
    channel_id: Option<Uuid>,
}

/// `POST /api/v1/notify/channels/test-draft` — tests a channel draft
/// **before saving** (no DB row, a log entry on a nil channel_id is
/// impossible → logging without FK: we still log the
/// attempt with a zero channel_id).
#[utoipa::path(
    post,
    path = "/api/v1/notify/channels/test-draft",
    tag = "notify",
    operation_id = "notify_channels_test_draft",
    summary = "Send a test message through an unsaved channel draft",
    description = "Sends a built-in test message using the draft configuration, before saving. Side effect: a real message leaves the server. With `channel_id`, secret fields left `null` take the stored channel's values. Nothing is stored and the attempt is not journaled. Requires owner, admin or member role. A failed send answers 502 `notify-test-failed` with the upstream error in `args.detail`.",
    request_body = TestDraftInput,
    responses(
        (status = 204, description = "Sent"),
        (status = 400, description = "`field-invalid`, `notify-channel-config-invalid`, `notify-channel-field-required` or a vault error", body = crate::openapi::ErrorDetail),
        (status = 403, body = crate::openapi::ErrorDetail),
        (status = 502, description = "`notify-test-failed`: the send failed (`args.detail`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn test_draft(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(params): Json<TestDraftInput>,
) -> Result<Response> {
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
    // R9: a member tests exactly what the stored channel holds (a draft
    // pointing a held secret elsewhere is refused like its save would be).
    let held = existing
        .as_ref()
        .map(|m| secrets::notify::held_bindings(&m.kind, &m.config))
        .unwrap_or_default();
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
    // Draft without an id: the log requires an existing channel (FK) — we
    // treat the test-draft as outside the log (ephemeral result in the
    // toast), persisted channels have their own.
    let Some(ch) = pnex_notify::channel(&params.kind) else {
        return Ok(field_status("kind", err_codes::FIELD_INVALID));
    };
    let settings = NotifySettings::from_config(&ctx.config);
    let effective_config = if params.kind == "websocket" {
        notify::in_app_runtime_config(&settings, org.org.id, Uuid::nil())
    } else {
        plan.sendable
    };
    let msg = test_message(&name);
    test_answer(ch.send(&effective_config, &msg).await)
}

/// Optional body of `POST /channels/{id}/test`: with `template_id` the
/// built-in message is replaced by the rendered template (missing vars take
/// their declared `example`, like the preview).
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub(super) struct TestChannelBody {
    #[schema(value_type = Option<String>)]
    template_id: Option<Uuid>,
    #[serde(default)]
    vars: std::collections::BTreeMap<String, String>,
}

/// `POST /api/v1/notify/channels/{id}/test` — built-in message on the persisted
/// channel (logged with source `test`), bypasses `enabled: false`. With a
/// `template_id`: sends the rendered template instead (the log holds the
/// template_id — the test exercises rendering AND sending). The body is read as
/// `Bytes` and parsed by hand: `Option<Json<T>>` rejects an empty body as soon
/// as a `Content-Type: application/json` is present (default injected by
/// test harnesses), and duplicated `Content-Type` headers make
/// the Json extractor's sniffing fail.
#[utoipa::path(
    post,
    path = "/api/v1/notify/channels/{id}/test",
    tag = "notify",
    operation_id = "notify_channels_test",
    summary = "Send a test message through a saved channel",
    description = "Sends a built-in test message (or, with `template_id`, that template rendered with `vars` falling back to the declared examples) through the channel, even when it is disabled. Side effect: a real message leaves the server and the attempt is journaled with source `test`. The body is optional. Requires owner, admin or member role. A failed send answers 502 `notify-test-failed` (`args.detail`).",
    params(("id" = String, Path, format = "uuid", description = "Channel id")),
    request_body(content = TestChannelBody, description = "Optional; an empty body sends the built-in message"),
    responses(
        (status = 204, description = "Sent"),
        (status = 400, description = "`notify-test-body-invalid`, `notify-template-render` or a vault error", body = crate::openapi::ErrorDetail),
        (status = 403, body = crate::openapi::ErrorDetail),
        (status = 404, description = "Channel or template not found", body = crate::openapi::ErrorDetail),
        (status = 502, description = "`notify-test-failed`: the send failed (`args.detail`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn test_channel(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    raw: axum::body::Bytes,
) -> Result<Response> {
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
    // Same school as the preview: missing vars take their declared example,
    // provided overrides win. An **empty** example is not
    // injected: an empty declared var would shadow the payload key of the same name
    // at the render's root level (vars > payload in the context).
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
