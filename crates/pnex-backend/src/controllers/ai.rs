//! AI assistant — `/api/v1/ai/*`: state (UI visibility), LLM providers of
//! the org (secrets.md D116, key = vault reference), provider test, chat.
//! The platform provides no LLM (D119): each org brings its own.
//!
//! Garde-fous : kill-switch (`settings.ai.enabled`) coupé → chat 403 sans
//! requête DB ni requête LLM ; les outils d'écriture re-vérifient
//! `can_write` dans `services::ai::tools` ; **aucun** chemin de ce module
//! vers deploy/delete/commandes.

use axum::extract::{Path, State};
use axum::routing::{get, post, put};
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
        .add("/chat", post(chat))
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

/// `POST /api/v1/ai/chat` — tout membre (les outils d'écriture re-vérifient
/// `can_write`). Kill-switch OFF → 403 sans requête LLM.
async fn chat(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Json(body): Json<pnex_core::AiChatRequest>,
) -> Result<Response> {
    // 1. Kill-switch → 403, aucune requête (DB du connecteur ni LLM).
    if !AiSettings::from_config(&ctx.config).enabled {
        return Err(crate::services::ai::AiError::Disabled.into());
    }
    if body.messages.is_empty() {
        return Err(Error::BadRequest("messages must not be empty".into()));
    }
    // 2. Config effective — None → 400 ai_not_configured.
    let Some(cfg) = config::resolve(&ctx.db, &ctx.config, org.org.id).await else {
        return Err(crate::services::ai::AiError::NotConfigured.into());
    };
    // 3. Client O2 (lecture télémétrie) — None si O2 non configuré.
    let o2_client =
        OpenobserveSettings::from_config(&ctx.config).map(|s| openobserve::client::Client::new(&s));
    // 4. Prompt système (bundle org + page + langue).
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
    // 5. Historique texte → messages normalisés (le front n'envoie que le
    // texte des tours précédents ; les tours d'outils restent serveur).
    let history: Vec<Msg> = body
        .messages
        .iter()
        .filter(|m| m.role == "user" || m.role == "assistant")
        .map(|m| {
            if m.role == "user" {
                Msg::User {
                    text: m.content.clone(),
                }
            } else {
                Msg::Assistant {
                    text: Some(m.content.clone()),
                    tool_calls: vec![],
                }
            }
        })
        .collect();
    // 6. Boucle d'agent.
    let deps = tools::ToolDeps {
        db: &ctx.db,
        org_id: org.org.id,
        can_write: org.can_write(),
        author: Some(org.auth.user.email.clone()).filter(|e| !e.trim().is_empty()),
        o2: o2_client.as_ref(),
    };
    let reply = agent::run_turn(&deps, &cfg, cfg.provider, system, history).await?;
    Ok(format::json(pnex_core::AiChatResponse {
        answer: reply.answer,
        tool_trace: reply
            .tool_trace
            .into_iter()
            .map(|t| pnex_core::AiToolTrace {
                name: t.name,
                arguments: t.arguments,
                ok: t.ok,
                summary: t.summary,
                flow_id: t.flow_id,
            })
            .collect(),
    })
    .into_response())
}
