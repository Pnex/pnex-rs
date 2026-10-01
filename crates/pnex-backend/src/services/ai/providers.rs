//! LLM providers (secrets.md D116, D119): CRUD of the org API
//! (`/api/v1/ai/providers`) and resolution of the provider an org runs on
//! — its own default. The platform provides no LLM: each org brings its own.

use pnex_core::{err_codes, LlmProvider, LlmProviderInput, LlmProviderTest};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, DatabaseConnection, DbErr,
    EntityTrait, QueryFilter, QueryOrder, Set, TransactionTrait,
};
use uuid::Uuid;

use super::config::{ResolvedAiConfig, ANTHROPIC_BASE_URL};
use super::provider::{AiChatRequest, Msg, Provider};
use crate::models::_entities::llm_providers;
use crate::services::secrets::store::{self, StoreError, Writer};
use crate::services::secrets::{llm, Keyring};

const NAME_MAX: usize = 255;
const MODEL_MAX: usize = 200;
const BASE_URL_MAX: usize = 1024;

/// Rows of `org_id` (every provider belongs to an org: users bring
/// their own LLM, the platform provides none).
fn org_filter(org_id: i64) -> Condition {
    Condition::all().add(llm_providers::Column::OrgId.eq(org_id))
}

/// Who writes: the vault writer and whether a value may be typed.
#[derive(Clone, Copy, Debug)]
pub struct Author {
    pub user_id: Option<i64>,
    /// Owner/admin of the org (D117).
    pub can_write_secrets: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("invalid {field}: {token}")]
    Invalid { field: &'static str, token: String },
    #[error("LLM provider not found")]
    NotFound,
    #[error("LLM provider name taken")]
    NameTaken,
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<DbErr> for ProviderError {
    fn from(e: DbErr) -> Self {
        Self::Store(StoreError::Db(e))
    }
}

/// Checked fields of an input.
struct Clean {
    name: String,
    kind: Provider,
    base_url: Option<String>,
    model: String,
}

fn invalid(field: &'static str, token: impl Into<String>) -> ProviderError {
    ProviderError::Invalid {
        field,
        token: token.into(),
    }
}

fn validate(input: &LlmProviderInput) -> Result<Clean, ProviderError> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(invalid("name", err_codes::FIELD_REQUIRED));
    }
    if name.chars().count() > NAME_MAX {
        return Err(invalid(
            "name",
            format!("{}:{NAME_MAX}", err_codes::FIELD_MAX_LENGTH),
        ));
    }
    let kind = Provider::parse(&input.kind);
    if input.kind != kind.as_str() {
        return Err(invalid("kind", err_codes::FIELD_INVALID));
    }
    let model = input.model.trim();
    if model.is_empty() {
        return Err(invalid("model", err_codes::FIELD_REQUIRED));
    }
    if model.chars().count() > MODEL_MAX {
        return Err(invalid(
            "model",
            format!("{}:{MODEL_MAX}", err_codes::FIELD_MAX_LENGTH),
        ));
    }
    let base_url = input
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(str::to_string);
    if let Some(url) = &base_url {
        if url.chars().count() > BASE_URL_MAX {
            return Err(invalid(
                "base_url",
                format!("{}:{BASE_URL_MAX}", err_codes::FIELD_MAX_LENGTH),
            ));
        }
        if !(url.starts_with("http://") || url.starts_with("https://")) || url.ends_with('/') {
            return Err(invalid("base_url", err_codes::FIELD_INVALID));
        }
    } else if kind == Provider::OpenAiCompat {
        // openai_compat: the version root is mandatory (no public default).
        return Err(invalid("base_url", err_codes::FIELD_REQUIRED));
    }
    Ok(Clean {
        name: name.to_string(),
        kind,
        base_url,
        model: model.to_string(),
    })
}

/// Providers of `org_id`, by name.
pub async fn list<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
) -> Result<Vec<llm_providers::Model>, DbErr> {
    llm_providers::Entity::find()
        .filter(org_filter(org_id))
        .order_by_asc(llm_providers::Column::Name)
        .all(db)
        .await
}

pub async fn find<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    id: Uuid,
) -> Result<llm_providers::Model, ProviderError> {
    llm_providers::Entity::find_by_id(id)
        .filter(org_filter(org_id))
        .one(db)
        .await?
        .ok_or(ProviderError::NotFound)
}

async fn default_of<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
) -> Result<Option<llm_providers::Model>, DbErr> {
    llm_providers::Entity::find()
        .filter(org_filter(org_id))
        .filter(llm_providers::Column::IsDefault.eq(true))
        .one(db)
        .await
}

async fn name_taken<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    name: &str,
    except: Option<Uuid>,
) -> Result<bool, DbErr> {
    let mut q = llm_providers::Entity::find()
        .filter(org_filter(org_id))
        .filter(llm_providers::Column::Name.eq(name));
    if let Some(id) = except {
        q = q.filter(llm_providers::Column::Id.ne(id));
    }
    Ok(q.one(db).await?.is_some())
}

/// Clears the default flag of every other provider of `org_id` (one
/// default per org, enforced by a partial unique index).
async fn clear_default<C: ConnectionTrait>(db: &C, org_id: i64, except: Uuid) -> Result<(), DbErr> {
    llm_providers::Entity::update_many()
        .col_expr(llm_providers::Column::IsDefault, Expr::value(false))
        .filter(org_filter(org_id))
        .filter(llm_providers::Column::IsDefault.eq(true))
        .filter(llm_providers::Column::Id.ne(except))
        .exec(db)
        .await?;
    Ok(())
}

fn writer(org_id: i64, by: Author) -> Writer {
    Writer {
        org_id: Some(org_id),
        user_id: by.user_id,
    }
}

/// Creates a provider. The key is required; the first provider of an
/// org becomes its default.
pub async fn create(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
    by: Author,
    input: &LlmProviderInput,
) -> Result<llm_providers::Model, ProviderError> {
    let clean = validate(input)?;
    let Some(key) = input.api_key.as_ref() else {
        return Err(invalid("api_key", err_codes::FIELD_REQUIRED));
    };
    let txn = db.begin().await?;
    if name_taken(&txn, org_id, &clean.name, None).await? {
        return Err(ProviderError::NameTaken);
    }
    let id = Uuid::new_v4();
    let is_default = input.is_default || default_of(&txn, org_id).await?.is_none();
    if is_default {
        clear_default(&txn, org_id, id).await?;
    }
    let secret = llm::save(
        &txn,
        ring,
        writer(org_id, by),
        by.can_write_secrets,
        id,
        &clean.name,
        None,
        None,
        Some(key),
    )
    .await?;
    let now: sea_orm::prelude::DateTimeWithTimeZone = chrono::Utc::now().into();
    let row = llm_providers::ActiveModel {
        id: Set(id),
        org_id: Set(org_id),
        name: Set(clean.name),
        kind: Set(clean.kind.as_str().to_string()),
        base_url: Set(clean.base_url),
        model: Set(clean.model),
        secret_id: Set(secret),
        is_default: Set(is_default),
        created_by: Set(by.user_id),
        updated_by: Set(by.user_id),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&txn)
    .await?;
    txn.commit().await?;
    Ok(row)
}

/// Updates a provider; `api_key: None` keeps the current key.
pub async fn update(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
    by: Author,
    id: Uuid,
    input: &LlmProviderInput,
) -> Result<llm_providers::Model, ProviderError> {
    let clean = validate(input)?;
    let txn = db.begin().await?;
    let row = find(&txn, org_id, id).await?;
    if name_taken(&txn, org_id, &clean.name, Some(id)).await? {
        return Err(ProviderError::NameTaken);
    }
    if input.is_default && !row.is_default {
        clear_default(&txn, org_id, id).await?;
    }
    let secret = llm::save(
        &txn,
        ring,
        writer(org_id, by),
        by.can_write_secrets,
        id,
        &clean.name,
        Some(&row.name),
        row.secret_id,
        input.api_key.as_ref(),
    )
    .await?;
    let mut am: llm_providers::ActiveModel = row.into();
    am.name = Set(clean.name);
    am.kind = Set(clean.kind.as_str().to_string());
    am.base_url = Set(clean.base_url);
    am.model = Set(clean.model);
    am.secret_id = Set(secret);
    am.is_default = Set(input.is_default);
    am.updated_by = Set(by.user_id);
    let row = am.update(&txn).await?;
    txn.commit().await?;
    Ok(row)
}

/// Deletes a provider and its dedicated key.
pub async fn delete(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<(), ProviderError> {
    let txn = db.begin().await?;
    let row = find(&txn, org_id, id).await?;
    llm::release(&txn, Some(org_id), id, &row.name).await?;
    llm_providers::Entity::delete_by_id(id).exec(&txn).await?;
    txn.commit().await?;
    Ok(())
}

/// Read models of `org_id`'s providers (key = reference, never a value).
pub async fn views<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    rows: &[llm_providers::Model],
) -> Result<Vec<LlmProvider>, StoreError> {
    let ids: Vec<Uuid> = rows.iter().filter_map(|r| r.secret_id).collect();
    let names = store::names_of(db, Some(org_id), &ids).await?;
    Ok(rows
        .iter()
        .map(|r| {
            let api_key = r.secret_id.and_then(|id| {
                Some(pnex_core::SecretFieldView {
                    secret_id: id,
                    name: names.get(&id)?.clone(),
                })
            });
            view(r, api_key)
        })
        .collect())
}

fn view(r: &llm_providers::Model, api_key: Option<pnex_core::SecretFieldView>) -> LlmProvider {
    LlmProvider {
        id: r.id,
        name: r.name.clone(),
        kind: r.kind.clone(),
        base_url: r.base_url.clone(),
        model: r.model.clone(),
        api_key,
        api_key_set: r.secret_id.is_some(),
        is_default: r.is_default,
        updated_at: r.updated_at.to_rfc3339(),
    }
}

/// The provider `org_id` runs on: its default (no platform fallback).
pub async fn effective<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
) -> Result<Option<llm_providers::Model>, DbErr> {
    default_of(db, org_id).await
}

/// Ready-to-call configuration of a provider: the key is decrypted in
/// memory. `None` when it has no key.
pub async fn resolved<C: ConnectionTrait>(
    db: &C,
    ring: &Keyring,
    row: &llm_providers::Model,
) -> Result<Option<ResolvedAiConfig>, StoreError> {
    let Some(secret) = row.secret_id else {
        return Ok(None);
    };
    let api_key = store::reveal(db, ring, Some(row.org_id), secret).await?;
    let provider = Provider::parse(&row.kind);
    let base_url = match (&row.base_url, provider) {
        (Some(url), _) => url.clone(),
        (None, Provider::Anthropic) => ANTHROPIC_BASE_URL.into(),
        (None, Provider::OpenAiCompat) => return Ok(None),
    };
    Ok(Some(ResolvedAiConfig {
        provider,
        name: row.name.clone(),
        base_url,
        api_key,
        model: row.model.clone(),
    }))
}

/// One-shot ping of a provider (never persists anything; a real LLM
/// request).
pub async fn test(
    db: &DatabaseConnection,
    ring: &Keyring,
    row: &llm_providers::Model,
) -> LlmProviderTest {
    let cfg = match resolved(db, ring, row).await {
        Ok(Some(cfg)) => cfg,
        Ok(None) => {
            return LlmProviderTest {
                ok: false,
                provider: Some(row.kind.clone()),
                model: Some(row.model.clone()),
                latency_ms: None,
                error: Some("No API key set for this provider.".into()),
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "LLM provider key could not be read");
            return LlmProviderTest {
                ok: false,
                provider: Some(row.kind.clone()),
                model: Some(row.model.clone()),
                latency_ms: None,
                error: Some("The API key could not be read from the vault.".into()),
            };
        }
    };
    let started = std::time::Instant::now();
    let http = super::agent::shared_probe_http();
    let messages = [Msg::User {
        text: "ping".into(),
    }];
    let req = AiChatRequest {
        system: "Connectivity test: answer \"pong\".".into(),
        messages: &messages,
        tools: &[],
        max_tokens: 16,
    };
    let result = cfg.provider.chat(http, &cfg, &req).await;
    LlmProviderTest {
        ok: result.is_ok(),
        provider: Some(cfg.provider.as_str().to_string()),
        model: Some(cfg.model.clone()),
        latency_ms: Some(started.elapsed().as_millis() as u64),
        error: result.err().map(|e| e.user_message()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(kind: &str, base_url: Option<&str>, model: &str) -> LlmProviderInput {
        LlmProviderInput {
            name: "main".into(),
            kind: kind.into(),
            base_url: base_url.map(str::to_string),
            model: model.into(),
            api_key: None,
            is_default: false,
        }
    }

    #[test]
    fn validation_rejects_invalid_fields() {
        assert!(
            validate(&input("mistral", None, "m")).is_err(),
            "unknown kind"
        );
        assert!(
            validate(&input("anthropic", None, "  ")).is_err(),
            "empty model"
        );
        assert!(
            validate(&input("openai_compat", None, "llama3")).is_err(),
            "openai_compat needs a base url"
        );
        assert!(
            validate(&input(
                "openai_compat",
                Some("http://localhost:11434/v1/"),
                "llama3"
            ))
            .is_err(),
            "trailing slash"
        );
        assert!(
            validate(&input(
                "openai_compat",
                Some("localhost:11434/v1"),
                "llama3"
            ))
            .is_err(),
            "scheme required"
        );
        assert!(
            validate(&input("anthropic", None, "claude-sonnet-4-5")).is_ok(),
            "anthropic without base url"
        );
        assert!(validate(&input(
            "openai_compat",
            Some("http://localhost:11434/v1"),
            "llama3"
        ))
        .is_ok());
    }
}
