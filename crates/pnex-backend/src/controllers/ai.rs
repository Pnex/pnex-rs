//! AI assistant — `/api/v1/ai/*`: state (UI visibility), LLM providers of
//! the org (secrets.md D116, key = vault reference), provider test, and
//! the user's private conversations (D145).
//! The platform provides no LLM (D119): each org brings its own.
//!
//! Garde-fous : kill-switch (`settings.ai.enabled`) coupé → chat 403 sans
//! requête DB ni requête LLM ; every tool needs its IAM permission
//! (`services::ai::tools::execute`) ; **aucun** chemin de ce module
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
        .add("/retention", get(retention_get).put(retention_put))
        .add("/retention/default", put(retention_default_put))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(
        status,
        org_list,
        org_create,
        org_update,
        org_delete,
        org_test,
        conversation_list,
        conversation_create,
        conversation_delete_all,
        conversation_export,
        conversation_detail,
        conversation_rename,
        conversation_delete,
        conversation_send,
        retention_get,
        retention_put,
        retention_default_put
    ))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

/// `GET /api/v1/ai/status` — every member. Kill-switch off: answered
/// without a database query. Never returns a key.
#[utoipa::path(
    get,
    path = "/api/v1/ai/status",
    tag = "ai",
    operation_id = "ai_status",
    summary = "Get the assistant status",
    description = "Whether the assistant is enabled on this server and which LLM provider/model the org uses for it. Never returns a key. Any member.",
    responses(
        (status = 200, body = pnex_core::AiStatus)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn status(org: OrgContext, State(ctx): State<AppContext>) -> Result<Response> {
    if !AiSettings::from_config(&ctx.config).enabled {
        return format::json(pnex_core::AiStatus::default());
    }
    let resolved = config::resolve(
        &ctx.db,
        &ctx.config,
        org.org.id,
        pnex_core::LlmUsage::Assistant,
    )
    .await;
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
        ProviderError::Invalid { field, token } => Ok(super::field_error(field, &token)),
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

/// Providers of the org, one page (D240).
async fn list_response(ctx: &AppContext, org_id: i64, q: &ListQuery) -> Result<Response> {
    let rows = providers::list(&ctx.db, org_id).await.map_err(db_error)?;
    match providers::views(&ctx.db, org_id, &rows).await {
        Ok(views) => {
            let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
            let count = views.len() as i64;
            let (skip, take) = page.slice(views.len());
            let views: Vec<_> = views.into_iter().skip(skip).take(take).collect();
            format::json(pagination::envelope(
                "/api/v1/ai/providers",
                &[],
                page,
                count,
                views,
            ))
        }
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
        can_write_secrets: org.can("secrets:write"),
    }
}

/// `GET /api/v1/ai/providers` — every member.
#[utoipa::path(
    get,
    path = "/api/v1/ai/providers",
    tag = "ai",
    operation_id = "ai_providers_list",
    summary = "List the LLM providers of the organisation",
    description = "LLM providers the org brings (the platform provides none), with default flag and routed usages, paginated (`limit`/`offset`). The API key is only a vault reference. Any member.",
    params(ListQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::LlmProvider>)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn org_list(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    list_response(&ctx, org.org.id, &q).await
}

/// `POST /api/v1/ai/providers` — owner/admin.
#[utoipa::path(
    post,
    path = "/api/v1/ai/providers",
    tag = "ai",
    operation_id = "ai_providers_create",
    summary = "Create an LLM provider",
    description = "Registers an LLM provider (`anthropic` or `openai_compat`). A typed `api_key` value is stored in the org vault. `is_default` and `usages` are taken away from other providers. Owner or admin only.",
    request_body = pnex_core::LlmProviderInput,
    responses(
        (status = 201, body = pnex_core::LlmProvider),
        (status = 400, description = "`field-invalid` (`errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`llm-provider-name-taken`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn org_create(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Json(input): Json<pnex_core::LlmProviderInput>,
) -> Result<Response> {
    create_response(&ctx, org.org.id, org_author(&org), input).await
}

/// `PUT /api/v1/ai/providers/{id}` — owner/admin.
#[utoipa::path(
    put,
    path = "/api/v1/ai/providers/{id}",
    tag = "ai",
    operation_id = "ai_providers_update",
    summary = "Replace an LLM provider",
    description = "Full replacement of the provider settings; an absent `api_key` keeps the current key. Owner or admin only.",
    params(("id" = String, Path, format = "uuid", description = "LLM provider id")),
    request_body = pnex_core::LlmProviderInput,
    responses(
        (status = 200, body = pnex_core::LlmProvider),
        (status = 400, description = "`field-invalid` (`errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`llm-provider-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`llm-provider-name-taken`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn org_update(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Path(id): Path<Uuid>,
    Json(input): Json<pnex_core::LlmProviderInput>,
) -> Result<Response> {
    update_response(&ctx, org.org.id, org_author(&org), id, input).await
}

/// `DELETE /api/v1/ai/providers/{id}` — owner/admin.
#[utoipa::path(
    delete,
    path = "/api/v1/ai/providers/{id}",
    tag = "ai",
    operation_id = "ai_providers_delete",
    summary = "Delete an LLM provider",
    description = "Deletes the provider. Owner or admin only.",
    params(("id" = String, Path, format = "uuid", description = "LLM provider id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "`llm-provider-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn org_delete(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    delete_response(&ctx, org.org.id, id).await
}

/// `POST /api/v1/ai/providers/{id}/test` — owner/admin.
#[utoipa::path(
    post,
    path = "/api/v1/ai/providers/{id}/test",
    tag = "ai",
    operation_id = "ai_providers_test",
    summary = "Test an LLM provider",
    description = "Sends one real request to the LLM (works even with the assistant switched off) and reports success, latency or the coded failure in the body (always 200 once the provider is found). Owner or admin only.",
    params(("id" = String, Path, format = "uuid", description = "LLM provider id")),
    responses(
        (status = 200, body = pnex_core::LlmProviderTest),
        (status = 404, description = "`llm-provider-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn org_test(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    test_response(&ctx, org.org.id, id).await
}

// ─────────────────────── Conversations (D145) ───────────────────────

/// Pagination query of the provider and conversation lists.
#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ListQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
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
        .ok_or_else(|| {
            super::coded_error(
                axum::http::StatusCode::NOT_FOUND,
                err_codes::AI_CONVERSATION_NOT_FOUND,
                "Conversation not found.",
                None,
            )
        })
}

/// `GET /api/v1/ai/conversations` — my conversations in this org (D14).
/// Rows carry no message nor tool trace. Allowed with the kill-switch off
/// (a user can always read and erase what is stored about them).
#[utoipa::path(
    get,
    path = "/api/v1/ai/conversations",
    tag = "ai",
    operation_id = "ai_conversations_list",
    summary = "List my assistant conversations",
    description = "The caller's own conversations in this org, newest first, paginated (`limit`/`offset`); rows carry no messages. Works with the assistant switched off. Any member.",
    params(ListQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::AiConversation>)
    ),
    security(("bearer" = [], "org" = []))
)]
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
#[utoipa::path(
    post,
    path = "/api/v1/ai/conversations",
    tag = "ai",
    operation_id = "ai_conversations_create",
    summary = "Start an assistant conversation",
    description = "Creates an empty private conversation of the caller (title optional). Any member.",
    request_body = pnex_core::AiConversationWrite,
    responses(
        (status = 201, body = pnex_core::AiConversation)
    ),
    security(("bearer" = [], "org" = []))
)]
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
#[utoipa::path(
    get,
    path = "/api/v1/ai/conversations/{id}",
    tag = "ai",
    operation_id = "ai_conversations_get",
    summary = "Get an assistant conversation",
    description = "Every stored message of one of the caller's conversations, with tool traces. Any member.",
    params(("id" = String, Path, format = "uuid", description = "Conversation id")),
    responses(
        (status = 200, body = pnex_core::AiConversationDetail),
        (status = 404, description = "`ai-conversation-not-found`: not one of the caller's conversations in this org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
#[utoipa::path(
    patch,
    path = "/api/v1/ai/conversations/{id}",
    tag = "ai",
    operation_id = "ai_conversations_rename",
    summary = "Rename an assistant conversation",
    description = "Sets the title of one of the caller's conversations. Any member.",
    params(("id" = String, Path, format = "uuid", description = "Conversation id")),
    request_body = pnex_core::AiConversationWrite,
    responses(
        (status = 200, body = pnex_core::AiConversation),
        (status = 400, description = "`field-invalid` (`title`: `required`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ai-conversation-not-found`: not one of the caller's conversations in this org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn conversation_rename(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Path(id): Path<Uuid>,
    Json(body): Json<pnex_core::AiConversationWrite>,
) -> Result<Response> {
    let Some(title) = body.title.as_deref().filter(|t| !t.trim().is_empty()) else {
        return Ok(super::field_error("title", err_codes::FIELD_REQUIRED));
    };
    let conv = owned_or_404(&ctx, &org, id).await?;
    let conv = conversations::rename(&ctx.db, conv, title)
        .await
        .map_err(db_error)?;
    format::json(conversations::dto(&conv))
}

/// `DELETE /api/v1/ai/conversations/{id}` — permanent erasure.
#[utoipa::path(
    delete,
    path = "/api/v1/ai/conversations/{id}",
    tag = "ai",
    operation_id = "ai_conversations_delete",
    summary = "Delete an assistant conversation",
    description = "Permanently erases one of the caller's conversations and its messages. Any member.",
    params(("id" = String, Path, format = "uuid", description = "Conversation id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "`ai-conversation-not-found`: not one of the caller's conversations in this org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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

/// Answer of the erase-all endpoint.
#[derive(serde::Serialize, utoipa::ToSchema)]
struct AiDeletedCount {
    deleted: u64,
}

/// Effective platform retention (days) after an update.
#[derive(serde::Serialize, utoipa::ToSchema)]
struct AiRetentionDays {
    days: u32,
}

/// `DELETE /api/v1/ai/conversations` — erase all my conversations of
/// this org.
#[utoipa::path(
    delete,
    path = "/api/v1/ai/conversations",
    tag = "ai",
    operation_id = "ai_conversations_delete_all",
    summary = "Delete all my assistant conversations",
    description = "Permanently erases every conversation of the caller in this org and returns how many were deleted. Any member.",
    responses(
        (status = 200, body = AiDeletedCount)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn conversation_delete_all(
    org: OrgContext,
    State(ctx): State<AppContext>,
) -> Result<Response> {
    let deleted = conversations::delete_all(&ctx.db, org.auth.user.id, org.org.id)
        .await
        .map_err(db_error)?;
    format::json(AiDeletedCount { deleted })
}

/// `GET /api/v1/ai/conversations/export` — portability export (JSON).
#[utoipa::path(
    get,
    path = "/api/v1/ai/conversations/export",
    tag = "ai",
    operation_id = "ai_conversations_export",
    summary = "Export my assistant conversations",
    description = "Portability export (JSON) of every conversation of the caller in this org with all messages. Any member.",
    responses(
        (status = 200, body = pnex_core::AiConversationsExport)
    ),
    security(("bearer" = [], "org" = []))
)]
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
/// member may converse (viewers included); every tool needs its IAM
/// permission. Kill-switch off → 403 without any LLM request. One turn
/// at a time per conversation → 409 `ai-conversation-busy`.
#[utoipa::path(
    post,
    path = "/api/v1/ai/conversations/{id}/messages",
    tag = "ai",
    operation_id = "ai_conversations_send",
    summary = "Send a message to the assistant",
    description = "Runs one agent turn: the server rebuilds the history, calls the org LLM, may run assistant tools (write tools re-check the caller's write role and never deploy, stop or delete) and stores the turn. One turn at a time per conversation. Any member.",
    params(("id" = String, Path, format = "uuid", description = "Conversation id")),
    request_body = pnex_core::AiSendMessage,
    responses(
        (status = 200, body = pnex_core::AiTurnResponse),
        (status = 400, description = "`field-invalid` on `content` (`required`, `max_length:<n>`), or `ai-not-configured`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`ai-disabled`: assistant switched off on this server", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ai-conversation-not-found`: not one of the caller's conversations in this org", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`ai-conversation-busy`", body = crate::openapi::ErrorDetail),
        (status = 429, description = "`ai-rate-limited` by the LLM provider", body = crate::openapi::ErrorDetail),
        (status = 502, description = "LLM failure (`ai-upstream`, `ai-timeout`, `ai-auth-rejected`, ...)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
        return Ok(super::field_error("content", err_codes::FIELD_REQUIRED));
    }
    if content.chars().count() > conversations::CONTENT_MAX_CHARS {
        let token = format!("max_length:{}", conversations::CONTENT_MAX_CHARS);
        return Ok(super::field_error("content", &token));
    }
    let conv = owned_or_404(&ctx, &org, id).await?;
    let Some(cfg) = config::resolve(
        &ctx.db,
        &ctx.config,
        org.org.id,
        pnex_core::LlmUsage::Assistant,
    )
    .await
    else {
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
        grants: org.grants.clone(),
        author: Some(org.auth.user.email.clone()).filter(|e| !e.trim().is_empty()),
        o2: o2_client.as_ref(),
        config: Some(&ctx.config),
        actor: Some(crate::services::ontology::Actor {
            user_id: Some(org.auth.user.id),
            role: org.role.clone(),
            source_ref: format!("assistant:{}", org.auth.user.id),
        }),
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

// ─────────────────────── Retention (D145) ───────────────────────

async fn retention_dto(ctx: &AppContext, org: &OrgContext) -> Result<pnex_core::AiRetention> {
    let platform = conversations::retention_days(&ctx.db).await;
    Ok(pnex_core::AiRetention {
        days: conversations::effective_retention(platform, org.org.ai_retention_days),
        platform_days: platform,
        org_days: org
            .org
            .ai_retention_days
            .and_then(|d| u32::try_from(d).ok()),
        editable: org.can("ai.settings:write"),
        platform_editable: org.auth.user.platform_admin,
    })
}

/// `GET /api/v1/ai/retention` — every member.
#[utoipa::path(
    get,
    path = "/api/v1/ai/retention",
    tag = "ai",
    operation_id = "ai_retention_get",
    summary = "Get the conversation retention",
    description = "How long inactive assistant conversations are kept in this org (effective, platform and org values) and whether the caller may change them. Any member.",
    responses(
        (status = 200, body = pnex_core::AiRetention)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn retention_get(org: OrgContext, State(ctx): State<AppContext>) -> Result<Response> {
    format::json(retention_dto(&ctx, &org).await?)
}

/// `PUT /api/v1/ai/retention` — owner/admin; the org may only SHORTEN the
/// platform value (`null` = follow the platform).
#[utoipa::path(
    put,
    path = "/api/v1/ai/retention",
    tag = "ai",
    operation_id = "ai_retention_update",
    summary = "Set the org conversation retention",
    description = "Shortens the retention of the org (1 to the platform value, in days); `null` follows the platform value. Owner or admin only.",
    request_body = pnex_core::RetentionUpdate,
    responses(
        (status = 200, body = pnex_core::AiRetention),
        (status = 422, description = "`ai-retention-above-platform` (args.max)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn retention_put(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Json(body): Json<pnex_core::RetentionUpdate>,
) -> Result<Response> {
    let platform = conversations::retention_days(&ctx.db).await;
    if let Some(days) = body.days {
        if days < 1 || days > platform {
            return Err(Error::CustomError(
                axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                loco_rs::controller::ErrorDetail {
                    error: Some(err_codes::AI_RETENTION_ABOVE_PLATFORM.to_string()),
                    description: Some(
                        "The organization may only keep conversations for less time than the platform"
                            .to_string(),
                    ),
                    errors: Some(serde_json::json!({ "args": { "max": platform.to_string() } })),
                },
            ));
        }
    }
    let mut active: crate::models::_entities::organizations::ActiveModel = org.org.clone().into();
    active.ai_retention_days = sea_orm::Set(body.days.map(|d| d as i32));
    let updated = active.update(&ctx.db).await.map_err(db_error)?;
    tracing::info!(org_id = updated.id, days = ?body.days, "assistant conversation retention updated");
    let mut org = org;
    org.org = updated;
    format::json(retention_dto(&ctx, &org).await?)
}

/// `PUT /api/v1/ai/retention/default` — platform admin (1..=3650 days,
/// `null` = built-in default).
#[utoipa::path(
    put,
    path = "/api/v1/ai/retention/default",
    tag = "ai",
    operation_id = "ai_retention_default_update",
    summary = "Set the platform conversation retention",
    description = "Platform admin only. Sets the platform retention of assistant conversations (1..=3650 days, `null` = built-in default) and returns the effective platform value.",
    request_body = pnex_core::RetentionUpdate,
    responses(
        (status = 200, body = AiRetentionDays),
        (status = 403, description = "Not a platform admin", body = crate::openapi::ErrorDetail),
        (status = 422, description = "`retention-out-of-range`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = []))
)]
async fn retention_default_put(
    admin: crate::auth::PlatformAdmin,
    State(ctx): State<AppContext>,
    Json(body): Json<pnex_core::RetentionUpdate>,
) -> Result<Response> {
    if let Some(days) = body.days {
        if !(1..=3650).contains(&days) {
            return Err(Error::CustomError(
                axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                loco_rs::controller::ErrorDetail::new(
                    err_codes::RETENTION_OUT_OF_RANGE,
                    "Retention must be between 1 and 3650 days.".to_string(),
                ),
            ));
        }
    }
    conversations::set_platform_retention(&ctx.db, body.days, admin.0.user.id)
        .await
        .map_err(db_error)?;
    tracing::info!(days = ?body.days, by = admin.0.user.id, "platform conversation retention updated");
    format::json(AiRetentionDays {
        days: conversations::retention_days(&ctx.db).await,
    })
}
