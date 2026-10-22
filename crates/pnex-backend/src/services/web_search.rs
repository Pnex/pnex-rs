//! Web search providers of an org and the `web_search` node
//! (web-presence.md D360–D361), school of the browser endpoints: key in
//! the vault, one default, a Test button.
//!
//! Every call goes through the egress-guarded client (R8): the provider is
//! chosen by an org, and every member owns one. The node writes nothing.

use std::time::{Duration, Instant};

use loco_rs::app::AppContext;
use pnex_core::err_codes;
use pnex_core::flow::FlowNodeKind;
use pnex_core::web_search::{
    rank, WebSearchConfig, WebSearchHit, WebSearchKind, WebSearchNodeReply, WebSearchNodeRequest,
    WebSearchProvider, WebSearchProviderInput, WebSearchTestResult, NODE_PORT_FAILED, NODE_PORT_OK,
    QUERY_MAX,
};
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, FromQueryResult, Statement,
    TransactionTrait,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::services::ontology::flow_nodes::{deployed, NodeError};
use crate::services::secrets::store::{self, StoreError, Writer};
use crate::services::secrets::{web_search as vault, Keyring};

const NAME_MAX: usize = 255;
const URL_MAX: usize = 2048;
const TIMEOUT: Duration = Duration::from_secs(15);
/// Searches of an org per UTC day, unless `settings.web_search.daily_calls`.
const DAILY_CALLS: u64 = 500;

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

#[derive(Clone, Copy, Debug)]
pub struct Author {
    pub user_id: Option<i64>,
    /// May type a secret value (owner/admin, D117).
    pub can_write_secrets: bool,
}

#[derive(Debug)]
pub enum ProviderError {
    Invalid { field: &'static str, token: String },
    NotFound,
    NameTaken,
    Store(StoreError),
}

impl From<DbErr> for ProviderError {
    fn from(e: DbErr) -> Self {
        Self::Store(StoreError::Db(e))
    }
}

impl From<StoreError> for ProviderError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

fn invalid(field: &'static str, token: impl Into<String>) -> ProviderError {
    ProviderError::Invalid {
        field,
        token: token.into(),
    }
}

#[derive(Debug, Clone, FromQueryResult)]
pub struct Row {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub base_url: String,
    pub secret_id: Option<Uuid>,
    pub is_default: bool,
    pub updated_at: DateTimeWithTimeZone,
}

impl Row {
    fn kind(&self) -> WebSearchKind {
        WebSearchKind::parse(&self.kind).unwrap_or_default()
    }

    /// The stored address, else the kind's default.
    fn base(&self) -> Option<reqwest::Url> {
        let raw = if self.base_url.is_empty() {
            self.kind().default_base_url()?
        } else {
            &self.base_url
        };
        reqwest::Url::parse(raw).ok()
    }
}

const COLUMNS: &str = "id, name, kind, base_url, secret_id, is_default, updated_at";

fn validate(input: &WebSearchProviderInput) -> Result<(String, String), ProviderError> {
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
    let raw = input.base_url.trim().trim_end_matches('/');
    if raw.is_empty() {
        if input.kind.default_base_url().is_none() {
            return Err(invalid("base_url", err_codes::FIELD_REQUIRED));
        }
        return Ok((name.to_string(), String::new()));
    }
    if raw.chars().count() > URL_MAX {
        return Err(invalid(
            "base_url",
            format!("{}:{URL_MAX}", err_codes::FIELD_MAX_LENGTH),
        ));
    }
    let url =
        reqwest::Url::parse(raw).map_err(|_| invalid("base_url", err_codes::FIELD_INVALID))?;
    // URLs are returned to every member: credentials go through the vault (R4, R16).
    if !matches!(url.scheme(), "http" | "https")
        || url.cannot_be_a_base()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(invalid("base_url", err_codes::FIELD_INVALID));
    }
    if let Some(code) = pnex_core::egress::check_url(&url) {
        return Err(invalid("base_url", code));
    }
    Ok((name.to_string(), raw.to_string()))
}

pub async fn list<C: ConnectionTrait>(db: &C, org_id: i64) -> Result<Vec<Row>, DbErr> {
    Row::find_by_statement(stmt(
        &format!("SELECT {COLUMNS} FROM web_search_providers WHERE org_id = $1 ORDER BY name"),
        vec![org_id.into()],
    ))
    .all(db)
    .await
}

pub async fn find<C: ConnectionTrait>(db: &C, org_id: i64, id: Uuid) -> Result<Row, ProviderError> {
    Row::find_by_statement(stmt(
        &format!("SELECT {COLUMNS} FROM web_search_providers WHERE org_id = $1 AND id = $2"),
        vec![org_id.into(), id.into()],
    ))
    .one(db)
    .await?
    .ok_or(ProviderError::NotFound)
}

async fn name_taken<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    name: &str,
    except: Option<Uuid>,
) -> Result<bool, DbErr> {
    #[derive(FromQueryResult)]
    struct N {
        n: i64,
    }
    Ok(N::find_by_statement(stmt(
        "SELECT count(*)::bigint AS n FROM web_search_providers WHERE org_id = $1 AND name = $2 \
         AND ($3::uuid IS NULL OR id <> $3)",
        vec![org_id.into(), name.into(), except.into()],
    ))
    .one(db)
    .await?
    .is_some_and(|c| c.n > 0))
}

/// Read models (key = vault reference, never a value).
pub async fn views<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    rows: &[Row],
) -> Result<Vec<WebSearchProvider>, StoreError> {
    let ids: Vec<Uuid> = rows.iter().filter_map(|r| r.secret_id).collect();
    let names = store::names_of(db, Some(org_id), &ids).await?;
    Ok(rows
        .iter()
        .map(|r| WebSearchProvider {
            id: r.id.to_string(),
            name: r.name.clone(),
            kind: r.kind(),
            base_url: r.base_url.clone(),
            api_key: r.secret_id.and_then(|id| {
                Some(pnex_core::SecretFieldView {
                    secret_id: id,
                    name: names.get(&id)?.clone(),
                })
            }),
            is_default: r.is_default,
            updated_at: r.updated_at.to_rfc3339(),
        })
        .collect())
}

fn writer(org_id: i64, by: Author) -> Writer {
    Writer {
        org_id: Some(org_id),
        user_id: by.user_id,
    }
}

/// The first provider of an org is its default; a new default replaces
/// the previous one.
async fn set_default<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    id: Uuid,
    wanted: bool,
) -> Result<(), DbErr> {
    let first = list(db, org_id)
        .await?
        .iter()
        .all(|r| r.id == id || !r.is_default);
    let sql = if wanted || first {
        "UPDATE web_search_providers SET is_default = (id = $2) WHERE org_id = $1"
    } else {
        "UPDATE web_search_providers SET is_default = false WHERE org_id = $1 AND id = $2"
    };
    db.execute_raw(stmt(sql, vec![org_id.into(), id.into()]))
        .await?;
    Ok(())
}

pub async fn create(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
    by: Author,
    input: &WebSearchProviderInput,
) -> Result<Row, ProviderError> {
    let (name, base_url) = validate(input)?;
    if input.kind.key_required() && input.api_key.is_none() {
        return Err(invalid("api_key", err_codes::FIELD_REQUIRED));
    }
    let txn = db.begin().await?;
    if name_taken(&txn, org_id, &name, None).await? {
        return Err(ProviderError::NameTaken);
    }
    let id = Uuid::new_v4();
    let secret = match &input.api_key {
        None => None,
        Some(k) => {
            vault::save(
                &txn,
                ring,
                writer(org_id, by),
                by.can_write_secrets,
                id,
                &name,
                None,
                None,
                Some(k),
            )
            .await?
        }
    };
    txn.execute_raw(stmt(
        "INSERT INTO web_search_providers (id, org_id, name, kind, base_url, secret_id, created_by, updated_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $7)",
        vec![
            id.into(),
            org_id.into(),
            name.into(),
            input.kind.as_str().into(),
            base_url.into(),
            secret.into(),
            by.user_id.into(),
        ],
    ))
    .await?;
    set_default(&txn, org_id, id, input.is_default).await?;
    let row = find(&txn, org_id, id).await?;
    txn.commit().await?;
    Ok(row)
}

/// `api_key: None` keeps the current key.
pub async fn update(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
    by: Author,
    id: Uuid,
    input: &WebSearchProviderInput,
) -> Result<Row, ProviderError> {
    let (name, base_url) = validate(input)?;
    let txn = db.begin().await?;
    let row = find(&txn, org_id, id).await?;
    if input.kind.key_required() && input.api_key.is_none() && row.secret_id.is_none() {
        return Err(invalid("api_key", err_codes::FIELD_REQUIRED));
    }
    if name_taken(&txn, org_id, &name, Some(id)).await? {
        return Err(ProviderError::NameTaken);
    }
    let secret = if input.api_key.is_some() || row.secret_id.is_some() {
        vault::save(
            &txn,
            ring,
            writer(org_id, by),
            by.can_write_secrets,
            id,
            &name,
            Some(&row.name),
            row.secret_id,
            input.api_key.as_ref(),
        )
        .await?
    } else {
        None
    };
    txn.execute_raw(stmt(
        "UPDATE web_search_providers SET name = $3, kind = $4, base_url = $5, secret_id = $6, \
         updated_by = $7, updated_at = now() WHERE org_id = $1 AND id = $2",
        vec![
            org_id.into(),
            id.into(),
            name.into(),
            input.kind.as_str().into(),
            base_url.into(),
            secret.into(),
            by.user_id.into(),
        ],
    ))
    .await?;
    set_default(&txn, org_id, id, input.is_default).await?;
    let row = find(&txn, org_id, id).await?;
    txn.commit().await?;
    Ok(row)
}

pub async fn delete(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<(), ProviderError> {
    let txn = db.begin().await?;
    let row = find(&txn, org_id, id).await?;
    if row.secret_id.is_some() {
        vault::release(&txn, Some(org_id), id, &row.name).await?;
    }
    txn.execute_raw(stmt(
        "DELETE FROM web_search_providers WHERE org_id = $1 AND id = $2",
        vec![org_id.into(), id.into()],
    ))
    .await?;
    txn.commit().await?;
    Ok(())
}

// ───────────────────────────── searching ─────────────────────────────

/// Why a search gave nothing (machine code + args for the node).
#[derive(Debug)]
pub enum SearchError {
    NoProvider(Option<String>),
    /// HTTP status when the provider answered, then the diagnostic.
    Unavailable(Option<u16>, String),
}

impl SearchError {
    fn code_args(&self) -> (&'static str, Value) {
        match self {
            Self::NoProvider(name) => (
                err_codes::WEB_SEARCH_NO_PROVIDER,
                name.as_ref()
                    .map_or(Value::Null, |n| json!({ "provider": n })),
            ),
            Self::Unavailable(status, _) => (
                err_codes::WEB_SEARCH_UNAVAILABLE,
                status.map_or(Value::Null, |s| json!({ "status": s })),
            ),
        }
    }
}

fn http() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        pnex_core::egress::guarded(
            reqwest::Client::builder()
                .redirect(pnex_core::egress::redirect_policy(3))
                .timeout(TIMEOUT)
                .user_agent(concat!("PNEX/", env!("CARGO_PKG_VERSION"))),
        )
        .build()
        .expect("reqwest client")
    })
}

#[derive(Deserialize)]
struct SerperAnswer {
    #[serde(default)]
    organic: Vec<SerperHit>,
}

#[derive(Deserialize)]
struct SerperHit {
    #[serde(default)]
    link: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    snippet: String,
}

#[derive(Deserialize)]
struct SearxAnswer {
    #[serde(default)]
    results: Vec<SearxHit>,
}

#[derive(Deserialize)]
struct SearxHit {
    #[serde(default)]
    url: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    content: String,
}

/// Raw `(url, title, snippet)` of one query, provider order.
async fn query_raw(
    row: &Row,
    key: Option<&str>,
    query: &str,
    config: &WebSearchConfig,
) -> Result<Vec<(String, String, String)>, SearchError> {
    let base = row
        .base()
        .ok_or_else(|| SearchError::Unavailable(None, "invalid provider address".into()))?;
    let url = |path: &str| {
        let mut u = base.clone();
        u.set_path(&format!("{}/{path}", base.path().trim_end_matches('/')));
        u
    };
    // Ask for more than the limit: exclusions drop some.
    let wanted = (config.limit * 2).min(pnex_core::web_search::LIMIT_MAX * 2);
    let req = match row.kind() {
        WebSearchKind::Serper => {
            let mut body = json!({ "q": query, "num": wanted });
            if !config.country.is_empty() {
                body["gl"] = config.country.to_ascii_lowercase().into();
            }
            if !config.lang.is_empty() {
                body["hl"] = config.lang.to_ascii_lowercase().into();
            }
            http()
                .post(url("search"))
                .header("X-API-KEY", key.unwrap_or_default())
                .json(&body)
        }
        WebSearchKind::Searxng => {
            let mut params = vec![("q", query.to_string()), ("format", "json".into())];
            if !config.lang.is_empty() {
                let lang = match config.country.is_empty() {
                    true => config.lang.to_ascii_lowercase(),
                    false => format!(
                        "{}-{}",
                        config.lang.to_ascii_lowercase(),
                        config.country.to_ascii_uppercase()
                    ),
                };
                params.push(("language", lang));
            }
            let r = http().get(url("search")).query(&params);
            match key {
                Some(k) => r.bearer_auth(k),
                None => r,
            }
        }
    };
    let res = req
        .send()
        .await
        .map_err(|e| SearchError::Unavailable(None, e.to_string()))?;
    let status = res.status();
    if !status.is_success() {
        return Err(SearchError::Unavailable(
            Some(status.as_u16()),
            format!("provider answered {status}"),
        ));
    }
    let unreadable =
        |e: reqwest::Error| SearchError::Unavailable(None, format!("unreadable answer: {e}"));
    Ok(match row.kind() {
        WebSearchKind::Serper => res
            .json::<SerperAnswer>()
            .await
            .map_err(unreadable)?
            .organic
            .into_iter()
            .map(|h| (h.link, h.title, h.snippet))
            .collect(),
        WebSearchKind::Searxng => res
            .json::<SearxAnswer>()
            .await
            .map_err(unreadable)?
            .results
            .into_iter()
            .map(|h| (h.url, h.title, h.content))
            .collect(),
    })
}

async fn key_of(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
    row: &Row,
) -> Result<Option<String>, SearchError> {
    match row.secret_id {
        None => Ok(None),
        Some(id) => store::reveal(db, ring, Some(org_id), id)
            .await
            .map(Some)
            .map_err(|e| SearchError::Unavailable(None, format!("key unreadable: {e}"))),
    }
}

/// One search on the named provider (else the org default): its name and
/// the ranked candidates.
pub async fn search(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
    query: &str,
    config: &WebSearchConfig,
) -> Result<(String, Vec<WebSearchHit>), SearchError> {
    let rows = list(db, org_id)
        .await
        .map_err(|e| SearchError::Unavailable(None, e.to_string()))?;
    let named = (!config.provider.is_empty()).then(|| config.provider.clone());
    let row = match &named {
        Some(n) => rows.iter().find(|r| &r.name == n),
        None => rows.iter().find(|r| r.is_default),
    }
    .ok_or(SearchError::NoProvider(named))?;
    let key = key_of(db, ring, org_id, row).await?;
    let raw = query_raw(row, key.as_deref(), query, config).await?;
    Ok((row.name.clone(), rank(raw, config)))
}

/// One real query from the server; nothing stored.
pub async fn test(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
    row: &Row,
) -> WebSearchTestResult {
    let start = Instant::now();
    let outcome = match key_of(db, ring, org_id, row).await {
        Ok(key) => {
            let config = WebSearchConfig {
                limit: 5,
                exclude_domains: Vec::new(),
                ..Default::default()
            };
            query_raw(row, key.as_deref(), "PNEX", &config).await
        }
        Err(e) => Err(e),
    };
    let latency_ms = start.elapsed().as_millis() as u64;
    match outcome {
        Ok(raw) => WebSearchTestResult {
            ok: true,
            latency_ms,
            results: raw.len() as u32,
            detail: None,
        },
        Err(e) => WebSearchTestResult {
            ok: false,
            latency_ms,
            results: 0,
            detail: Some(match e {
                SearchError::Unavailable(_, detail) => detail,
                SearchError::NoProvider(_) => "no provider".into(),
            }),
        },
    }
}

// ───────────────────────────── flow node ─────────────────────────────

#[derive(Default, Deserialize)]
struct CapPartial {
    daily_calls: Option<u64>,
}

/// `settings.web_search.daily_calls`, default [`DAILY_CALLS`].
fn daily_cap(ctx: &AppContext) -> u64 {
    ctx.config
        .settings
        .as_ref()
        .and_then(|s| s.get("web_search"))
        .and_then(|v| serde_json::from_value::<CapPartial>(v.clone()).ok())
        .and_then(|p| p.daily_calls)
        .unwrap_or(DAILY_CALLS)
}

fn failed(code: &str, args: Value) -> WebSearchNodeReply {
    WebSearchNodeReply {
        port: NODE_PORT_FAILED,
        code: Some(code.to_string()),
        args,
        ..Default::default()
    }
}

/// One query of the deployed node `req.node`, its options read from the
/// deployed version (the runtime is not trusted, R1).
pub async fn run(
    ctx: &AppContext,
    req: &WebSearchNodeRequest,
) -> Result<WebSearchNodeReply, NodeError> {
    let FlowNodeKind::WebSearch { config } = deployed(&ctx.db, &req.node).await? else {
        return Err(NodeError::NotDeployed);
    };
    let org_id = req.node.org_id;
    let query = req.query.trim();
    if query.is_empty() || query.chars().count() > QUERY_MAX {
        return Ok(failed(err_codes::WEB_SEARCH_QUERY_INVALID, Value::Null));
    }
    let cap = daily_cap(ctx);
    match crate::services::flow_ai::count_daily(ctx, "web-search", org_id, cap).await {
        Ok(()) => {}
        Err(crate::services::flow_ai::FlowAiError::Quota(cap)) => {
            return Ok(failed(err_codes::WEB_SEARCH_QUOTA, json!({ "cap": cap })))
        }
        Err(e) => {
            tracing::warn!(org_id, error = ?e, "web search quota counter failed");
            return Ok(failed(err_codes::WEB_SEARCH_UNAVAILABLE, Value::Null));
        }
    }
    let Ok(ring) = Keyring::from_config(&ctx.config) else {
        return Ok(failed(err_codes::WEB_SEARCH_UNAVAILABLE, Value::Null));
    };
    Ok(match search(&ctx.db, &ring, org_id, query, &config).await {
        Ok((provider, results)) => WebSearchNodeReply {
            port: NODE_PORT_OK,
            results,
            provider,
            ..Default::default()
        },
        Err(e) => {
            if let SearchError::Unavailable(_, detail) = &e {
                tracing::warn!(org_id, %detail, "web search failed");
            }
            let (code, args) = e.code_args();
            failed(code, args)
        }
    })
}
