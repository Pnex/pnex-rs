//! AI assistant — `/api/v1/ai/*`: state (UI visibility), LLM providers of
//! the org (secrets.md D116, key = vault reference), provider test, and
//! the user's private conversations (D145).
//! The platform provides no LLM (D119): each org brings its own.
//!
//! Garde-fous : kill-switch (`settings.ai.enabled`) coupé → chat 403 sans
//! requête DB ni requête LLM ; les outils d'écriture re-vérifient
//! `can_write` dans `services::ai::tools` ; **aucun** chemin de ce module
//! vers deploy/delete/commandes.

use axum::extract::{Path, Query, State};
use axum::routing::{get, post, put};

use crate::controllers::pagination;
use crate::services::ai::conversations;
use loco_rs::prelude::*;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::services::ai::config::{self, AiSettings};
use crate::services::ai::provider::Msg;
use crate::services::ai::providers::{self, Author, ProviderError};
use crate::services::ai::{agent, tools};
use crate::services::openobserve::{self, OpenobserveSettings};
use pnex_core::err_codes;

/// Forbidden with the machine code + canonical English description
/// (frontend resolves `err-<code>` at render time, verbatim fallback).
fn forbidden(code: &str, msg: &str) -> Error {
    Error::CustomError(
        axum::http::StatusCode::FORBIDDEN,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/ai")
        .add("/status", get(status))
        .add("/providers", get(org_list).post(org_create))
        .add("/providers/{id}", put(org_update).delete(org_delete))
        .add("/providers/{id}/test", post(org_test))
        .add(
            "/conversations",
            get(conversation_list)
                .post(conversation_create)
                .delete(conversation_delete_all),
        )
        .add("/conversations/export", get(conversation_export))
        .add(
            "/conversations/{id}",
            get(conversation_detail)
                .patch(conversation_rename)
                .delete(conversation_delete),
        )
        .add("/conversations/{id}/messages", post(conversation_send))
}

/// `GET /api/v1/ai/status` — every member. Kill-switch off: answered
/// without a database query. Never returns a key.
async fn status(org: OrgContext, State(ctx): State<AppContext>) -> Result<Response> {
    if !AiSettings::from_config(&ctx.config).enabled {
        return format::json(pnex_core::AiStatus::default());
    }
    let resolved = config::resolve(&ctx.db, &ctx.config, org.org.id).await;
    format::json(match resolved {
        Some(r) => pnex_core::AiStatus {
            enabled: true,
            configured: true,
            provider_name: Some(r.name),
            provider: Some(r.provider.as_str().to_string()),
            model: Some(r.model),
        },
        None => pnex_core::AiStatus {
            enabled: true,
            ..Default::default()
        },
    })
}

/// HTTP mapping of provider errors.
fn provider_error(e: ProviderError) -> Result<Response> {
    match e {
        ProviderError::Invalid { field, token } => Ok((
            axum::http::StatusCode::BAD_REQUEST,
            format::json(serde_json::json!({ field: token })),
        )
            .into_response()),
        ProviderError::NotFound => Err(Error::CustomError(
            axum::http::StatusCode::NOT_FOUND,
            loco_rs::controller::ErrorDetail::new(
                err_codes::LLM_PROVIDER_NOT_FOUND,
                "LLM provider not found.".to_string(),
            ),
        )),
        ProviderError::NameTaken => Err(Error::CustomError(
            axum::http::StatusCode::CONFLICT,
            loco_rs::controller::ErrorDetail::new(
                err_codes::LLM_PROVIDER_NAME_TAKEN,
                "An LLM provider with this name already exists.".to_string(),
            ),
        )),
        ProviderError::Store(e) => crate::controllers::secrets::store_error(e),
    }
}

fn db_error(e: sea_orm::DbErr) -> Error {
    tracing::error!(error = %e, "LLM providers database error");
    Error::InternalServerError
}

fn require_manage(org: &OrgContext) -> Result<()> {
    if org.can_administer() {
        Ok(())
    } else {
        Err(forbidden(
            err_codes::LLM_PROVIDER_FORBIDDEN,
            "Owner or admin role required to manage LLM providers.",
        ))
    }
}

/// Providers of the org.
async fn list_response(ctx: &AppContext, org_id: i64) -> Result<Response> {
    let rows = providers::list(&ctx.db, org_id).await.map_err(db_error)?;
    match providers::views(&ctx.db, org_id, &rows).await {
        Ok(views) => format::json(views),
        Err(e) => crate::controllers::secrets::store_error(e),
    }
}

async fn one_response(
    ctx: &AppContext,
    org_id: i64,
    row: &crate::models::_entities::llm_providers::Model,
    status: axum::http::StatusCode,
) -> Result<Response> {
    match providers::views(&ctx.db, org_id, std::slice::from_ref(row)).await {
        Ok(mut views) => Ok((status, format::json(views.remove(0))).into_response()),
        Err(e) => crate::controllers::secrets::store_error(e),
    }
}

async fn create_response(
    ctx: &AppContext,
    org_id: i64,
    by: Author,
    input: pnex_core::LlmProviderInput,
) -> Result<Response> {
    let ring = match crate::controllers::secrets::keyring(ctx) {
        Ok(ring) => ring,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    match providers::create(&ctx.db, &ring, org_id, by, &input).await {
        Ok(row) => one_response(ctx, org_id, &row, axum::http::StatusCode::CREATED).await,
        Err(e) => provider_error(e),
    }
}

async fn update_response(
    ctx: &AppContext,
    org_id: i64,
    by: Author,
    id: Uuid,
    input: pnex_core::LlmProviderInput,
) -> Result<Response> {
    let ring = match crate::controllers::secrets::keyring(ctx) {
        Ok(ring) => ring,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    match providers::update(&ctx.db, &ring, org_id, by, id, &input).await {
        Ok(row) => one_response(ctx, org_id, &row, axum::http::StatusCode::OK).await,
        Err(e) => provider_error(e),
    }
}

async fn delete_response(ctx: &AppContext, org_id: i64, id: Uuid) -> Result<Response> {
    match providers::delete(&ctx.db, org_id, id).await {
        Ok(()) => Ok(axum::http::StatusCode::NO_CONTENT.into_response()),
        Err(e) => provider_error(e),
    }
}

/// One real LLM request; does not depend on the kill-switch (testing is
/// not using).
async fn test_response(ctx: &AppContext, org_id: i64, id: Uuid) -> Result<Response> {
    let row = match providers::find(&ctx.db, org_id, id).await {
        Ok(row) => row,
        Err(e) => return provider_error(e),
    };
    let ring = match crate::controllers::secrets::keyring(ctx) {
        Ok(ring) => ring,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    format::json(providers::test(&ctx.db, &ring, &row).await)
}

fn org_author(org: &OrgContext) -> Author {
    Author {
        user_id: Some(org.auth.user.id),
        can_write_secrets: org.can_manage_secrets(),
    }
}

/// `GET /api/v1/ai/providers` — every member.
async fn org_list(org: OrgContext, State(ctx): State<AppContext>) -> Result<Response> {
    list_response(&ctx, org.org.id).await
}

/// `POST /api/v1/ai/providers` — owner/admin.
async fn org_create(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Json(input): Json<pnex_core::LlmProviderInput>,
) -> Result<Response> {
    require_manage(&org)?;
    create_response(&ctx, org.org.id, org_author(&org), input).await
}

/// `PUT /api/v1/ai/providers/{id}` — owner/admin.
async fn org_update(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Path(id): Path<Uuid>,
    Json(input): Json<pnex_core::LlmProviderInput>,
) -> Result<Response> {
    require_manage(&org)?;
    update_response(&ctx, org.org.id, org_author(&org), id, input).await
}

/// `DELETE /api/v1/ai/providers/{id}` — owner/admin.
async fn org_delete(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    require_manage(&org)?;
    delete_response(&ctx, org.org.id, id).await
}

/// `POST /api/v1/ai/providers/{id}/test` — owner/admin.
async fn org_test(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    require_manage(&org)?;
    test_response(&ctx, org.org.id, id).await
}

// ─────────────────────── Conversations (D145) ───────────────────────

/// Pagination query of the conversation list.
#[derive(Debug, serde::Deserialize)]
struct ListQuery {
    limit: Option<String>,
    offset: Option<String>,
}

/// Conversation of the caller in the current org, else 404 (the existence
/// of anyone else's conversation is never revealed).
async fn owned_or_404(
    ctx: &AppContext,
    org: &OrgContext,
    id: Uuid,
) -> Result<crate::models::_entities::ai_conversations::Model> {
    conversations::find_owned(&ctx.db, org.auth.user.id, org.org.id, id)
        .await
        .map_err(db_error)?
        .ok_or(Error::NotFound)
}

/// `GET /api/v1/ai/conversations` — my conversations in this org (D14).
/// Rows carry no message nor tool trace. Allowed with the kill-switch off
/// (a user can always read and erase what is stored about them).
async fn conversation_list(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let (count, rows) = pagination::sql_page(
        &ctx.db,
        conversations::owned(org.auth.user.id, org.org.id),
        page,
    )
    .await
    .map_err(|_| Error::InternalServerError)?;
    let results: Vec<pnex_core::AiConversation> = rows.iter().map(conversations::dto).collect();
    Ok(format::json(pagination::envelope(
        "/api/v1/ai/conversations",
        &[],
        page,
        count,
        results,
    ))
    .into_response())
}

/// `POST /api/v1/ai/conversations` — new empty conversation.
async fn conversation_create(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Json(body): Json<pnex_core::AiConversationWrite>,
) -> Result<Response> {
    let title = body.title.as_deref().filter(|t| !t.trim().is_empty());
    let conv = conversations::create(&ctx.db, org.auth.user.id, org.org.id, title)
        .await
        .map_err(db_error)?;
    Ok((
        axum::http::StatusCode::CREATED,
        format::json(conversations::dto(&conv)),
    )
        .into_response())
}

/// `GET /api/v1/ai/conversations/{id}` — resume: every stored message.
async fn conversation_detail(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let conv = owned_or_404(&ctx, &org, id).await?;
    format::json(
        conversations::detail(&ctx.db, &conv)
            .await
            .map_err(db_error)?,
    )
}

/// `PATCH /api/v1/ai/conversations/{id}` — rename.
async fn conversation_rename(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Path(id): Path<Uuid>,
    Json(body): Json<pnex_core::AiConversationWrite>,
) -> Result<Response> {
    let Some(title) = body.title.as_deref().filter(|t| !t.trim().is_empty()) else {
        return Ok((
            axum::http::StatusCode::BAD_REQUEST,
            format::json(serde_json::json!({ "title": "required" })),
        )
            .into_response());
    };
    let conv = owned_or_404(&ctx, &org, id).await?;
    let conv = conversations::rename(&ctx.db, conv, title)
        .await
        .map_err(db_error)?;
    format::json(conversations::dto(&conv))
}

/// `DELETE /api/v1/ai/conversations/{id}` — permanent erasure.
async fn conversation_delete(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let conv = owned_or_404(&ctx, &org, id).await?;
    conversations::delete(&ctx.db, conv.id)
        .await
        .map_err(db_error)?;
    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}

/// `DELETE /api/v1/ai/conversations` — erase all my conversations of
/// this org.
async fn conversation_delete_all(
    org: OrgContext,
    State(ctx): State<AppContext>,
) -> Result<Response> {
    let deleted = conversations::delete_all(&ctx.db, org.auth.user.id, org.org.id)
        .await
        .map_err(db_error)?;
    format::json(serde_json::json!({ "deleted": deleted }))
}

/// `GET /api/v1/ai/conversations/export` — portability export (JSON).
async fn conversation_export(org: OrgContext, State(ctx): State<AppContext>) -> Result<Response> {
    let rows = conversations::owned(org.auth.user.id, org.org.id)
        .all(&ctx.db)
        .await
        .map_err(db_error)?;
    let mut out = Vec::with_capacity(rows.len());
    for conv in &rows {
        out.push(
            conversations::detail(&ctx.db, conv)
                .await
                .map_err(db_error)?,
        );
    }
    format::json(pnex_core::AiConversationsExport {
        exported_at: chrono::Utc::now().to_rfc3339(),
        conversations: out,
    })
}

/// `POST /api/v1/ai/conversations/{id}/messages` — one agent turn. Every
/// member may converse (viewers included); write tools re-check
/// `can_write`. Kill-switch off → 403 without any LLM request. One turn
/// at a time per conversation → 409 `ai-conversation-busy`.
async fn conversation_send(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Path(id): Path<Uuid>,
    Json(body): Json<pnex_core::AiSendMessage>,
) -> Result<Response> {
    if !AiSettings::from_config(&ctx.config).enabled {
        return Err(crate::services::ai::AiError::Disabled.into());
    }
    let content = body.content.trim();
    if content.is_empty() {
        return Ok((
            axum::http::StatusCode::BAD_REQUEST,
            format::json(serde_json::json!({ "content": "required" })),
        )
            .into_response());
    }
    if content.chars().count() > conversations::CONTENT_MAX_CHARS {
        return Ok((
            axum::http::StatusCode::BAD_REQUEST,
            format::json(serde_json::json!({
                "content": format!("max_length:{}", conversations::CONTENT_MAX_CHARS)
            })),
        )
            .into_response());
    }
    let conv = owned_or_404(&ctx, &org, id).await?;
    let Some(cfg) = config::resolve(&ctx.db, &ctx.config, org.org.id).await else {
        return Err(crate::services::ai::AiError::NotConfigured.into());
    };
    if !conversations::acquire(&ctx.db, conv.id)
        .await
        .map_err(db_error)?
    {
        return Err(Error::CustomError(
            axum::http::StatusCode::CONFLICT,
            loco_rs::controller::ErrorDetail::new(
                err_codes::AI_CONVERSATION_BUSY,
                "A reply is already being written in this conversation".to_string(),
            ),
        ));
    }
    let started = std::time::Instant::now();
    let result = run_conversation_turn(&ctx, &org, &cfg, &conv, content, &body).await;
    conversations::release(&ctx.db, conv.id).await;
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let mut event = conversations::AuditEvent {
        org_id: org.org.id,
        user_id: org.auth.user.id,
        conversation_id: conv.id,
        provider: cfg.provider.as_str(),
        model: cfg.model.clone(),
        tokens_in: 0,
        tokens_out: 0,
        tools: Vec::new(),
        written_flow_ids: Vec::new(),
        latency_ms,
        outcome: "ok".into(),
    };
    match result {
        Ok((reply, trace, updated)) => {
            event.tokens_in = reply.usage.input_tokens;
            event.tokens_out = reply.usage.output_tokens;
            event.tools = trace.iter().map(|t| (t.name.clone(), t.ok)).collect();
            event.written_flow_ids = trace
                .iter()
                .filter(|t| t.ok)
                .filter_map(|t| t.flow_id)
                .collect();
            conversations::audit(&ctx, event);
            format::json(pnex_core::AiTurnResponse {
                answer: reply.answer,
                tool_trace: trace,
                conversation: conversations::dto(&updated),
            })
        }
        Err(e) => {
            event.outcome = "error".into();
            conversations::audit(&ctx, event);
            Err(e)
        }
    }
}

/// Body of one turn: prompt, server-side history, agent loop, storage.
async fn run_conversation_turn(
    ctx: &AppContext,
    org: &OrgContext,
    cfg: &crate::services::ai::ResolvedAiConfig,
    conv: &crate::models::_entities::ai_conversations::Model,
    content: &str,
    body: &pnex_core::AiSendMessage,
) -> Result<(
    agent::AgentReply,
    Vec<pnex_core::AiToolTrace>,
    crate::models::_entities::ai_conversations::Model,
)> {
    let o2_client =
        OpenobserveSettings::from_config(&ctx.config).map(|s| openobserve::client::Client::new(&s));
    let page = body
        .page
        .as_ref()
        .map(|p| crate::services::ai::context::PageContext {
            page: p.page.clone(),
            flow_id: p.flow_id,
            device_id: p.device_id,
        });
    let system = crate::services::ai::context::build_system_prompt(
        &ctx.db,
        o2_client.as_ref(),
        org.org.id,
        &org.org.name,
        body.language.as_deref().unwrap_or("fr"),
        page.as_ref(),
    )
    .await;
    // History rebuilt from storage: the client only sends the new message.
    let mut history: Vec<Msg> = conversations::history(&ctx.db, conv.id)
        .await
        .map_err(db_error)?;
    history.push(Msg::User {
        text: content.to_string(),
    });
    let deps = tools::ToolDeps {
        db: &ctx.db,
        org_id: org.org.id,
        can_write: org.can_write(),
        author: Some(org.auth.user.email.clone()).filter(|e| !e.trim().is_empty()),
        o2: o2_client.as_ref(),
        config: Some(&ctx.config),
    };
    let reply = agent::run_turn(&deps, cfg, cfg.provider, system, history).await?;
    let trace: Vec<pnex_core::AiToolTrace> = reply
        .tool_trace
        .iter()
        .map(conversations::trace_dto)
        .collect();
    let updated = conversations::append_turn(
        &ctx.db,
        conv,
        conversations::TurnRecord {
            user_text: content,
            page: body.page.as_ref().and_then(|p| p.page.as_deref()),
            answer: &reply.answer,
            trace: &trace,
            tokens_in: reply.usage.input_tokens,
            tokens_out: reply.usage.output_tokens,
        },
    )
    .await
    .map_err(db_error)?;
    Ok((reply, trace, updated))
}
