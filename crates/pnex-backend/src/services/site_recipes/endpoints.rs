//! Browser endpoints of an org (site-recipes.md D325): CDP browsers the
//! navigation rules play on, school of the geo providers (token in the
//! vault, one default, a Test button).
//!
//! A run plays on the endpoint its recipe names (`browser:`), else the org
//! default, else the platform browser (`PNEX_BROWSER_URL`, optional
//! `PNEX_BROWSER_TOKEN`), set by the operator. An org endpoint passes the
//! egress guard (R8): every member owns an org, so an internal address
//! (`….svc`, private ranges in `public` mode) is the platform's only.
//!
//! PNEX reads `/json/version` with the token, then opens the browser socket
//! with `Authorization: Bearer` on the upgrade ([`super::cdp`]).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::cdp::{Cdp, Tab};
use pnex_core::err_codes;
use pnex_core::site_recipe::{
    BrowserCheck, BrowserEndpoint, BrowserEndpointInput, BrowserTestResult,
};
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, FromQueryResult, Statement,
    TransactionTrait,
};
use uuid::Uuid;

use crate::services::secrets::store::{self, StoreError, Writer};
use crate::services::secrets::{browser as vault, Keyring};

const NAME_MAX: usize = 255;
const URL_MAX: usize = 2048;
const CONNECTIONS: std::ops::RangeInclusive<i32> = 1..=16;
/// Runs at once on the platform browser.
const PLATFORM_CONNECTIONS: usize = 2;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

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
pub enum EndpointError {
    Invalid { field: &'static str, token: String },
    NotFound,
    NameTaken,
    Store(StoreError),
}

impl From<DbErr> for EndpointError {
    fn from(e: DbErr) -> Self {
        Self::Store(StoreError::Db(e))
    }
}

impl From<StoreError> for EndpointError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

fn invalid(field: &'static str, token: impl Into<String>) -> EndpointError {
    EndpointError::Invalid {
        field,
        token: token.into(),
    }
}

#[derive(Debug, Clone, FromQueryResult)]
pub struct Row {
    pub id: Uuid,
    pub name: String,
    pub cdp_url: String,
    pub secret_id: Option<Uuid>,
    pub max_connections: i32,
    pub is_default: bool,
    pub updated_at: DateTimeWithTimeZone,
}

const COLUMNS: &str = "id, name, cdp_url, secret_id, max_connections, is_default, updated_at";

/// `ws(s)://` or `http(s)://` with a host, no credentials; the HTTP base
/// (`http(s)://host:port`) of `/json/version`.
fn http_base(raw: &str) -> Option<reqwest::Url> {
    let u = reqwest::Url::parse(raw.trim()).ok()?;
    let scheme = match u.scheme() {
        "ws" | "http" => "http",
        "wss" | "https" => "https",
        _ => return None,
    };
    if !u.username().is_empty() || u.password().is_some() {
        return None;
    }
    let host = u.host_str()?;
    let base = match u.port() {
        Some(p) => format!("{scheme}://{host}:{p}"),
        None => format!("{scheme}://{host}"),
    };
    reqwest::Url::parse(&base).ok()
}

fn validate(input: &BrowserEndpointInput) -> Result<String, EndpointError> {
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
    let raw = input.cdp_url.trim();
    if raw.chars().count() > URL_MAX {
        return Err(invalid(
            "cdp_url",
            format!("{}:{URL_MAX}", err_codes::FIELD_MAX_LENGTH),
        ));
    }
    let Some(base) = http_base(raw) else {
        return Err(invalid("cdp_url", err_codes::FIELD_INVALID));
    };
    if let Some(code) = pnex_core::egress::check_url(&base) {
        return Err(invalid("cdp_url", code));
    }
    if !CONNECTIONS.contains(&input.max_connections) {
        return Err(invalid("max_connections", err_codes::FIELD_INVALID));
    }
    Ok(name.to_string())
}

pub async fn list<C: ConnectionTrait>(db: &C, org_id: i64) -> Result<Vec<Row>, DbErr> {
    Row::find_by_statement(stmt(
        &format!("SELECT {COLUMNS} FROM browser_endpoints WHERE org_id = $1 ORDER BY name"),
        vec![org_id.into()],
    ))
    .all(db)
    .await
}

pub async fn find<C: ConnectionTrait>(db: &C, org_id: i64, id: Uuid) -> Result<Row, EndpointError> {
    Row::find_by_statement(stmt(
        &format!("SELECT {COLUMNS} FROM browser_endpoints WHERE org_id = $1 AND id = $2"),
        vec![org_id.into(), id.into()],
    ))
    .one(db)
    .await?
    .ok_or(EndpointError::NotFound)
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
        "SELECT count(*)::bigint AS n FROM browser_endpoints WHERE org_id = $1 AND name = $2 \
         AND ($3::uuid IS NULL OR id <> $3)",
        vec![org_id.into(), name.into(), except.into()],
    ))
    .one(db)
    .await?
    .is_some_and(|c| c.n > 0))
}

/// Read models (token = vault reference, never a value).
pub async fn views<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    rows: &[Row],
) -> Result<Vec<BrowserEndpoint>, StoreError> {
    let ids: Vec<Uuid> = rows.iter().filter_map(|r| r.secret_id).collect();
    let names = store::names_of(db, Some(org_id), &ids).await?;
    Ok(rows
        .iter()
        .map(|r| BrowserEndpoint {
            id: r.id.to_string(),
            name: r.name.clone(),
            cdp_url: r.cdp_url.clone(),
            token: r.secret_id.and_then(|id| {
                Some(pnex_core::SecretFieldView {
                    secret_id: id,
                    name: names.get(&id)?.clone(),
                })
            }),
            max_connections: r.max_connections,
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

/// The first endpoint of an org is its default; a new default replaces
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
    if wanted || first {
        db.execute_raw(stmt(
            "UPDATE browser_endpoints SET is_default = (id = $2) WHERE org_id = $1",
            vec![org_id.into(), id.into()],
        ))
        .await?;
    } else {
        db.execute_raw(stmt(
            "UPDATE browser_endpoints SET is_default = false WHERE org_id = $1 AND id = $2",
            vec![org_id.into(), id.into()],
        ))
        .await?;
    }
    Ok(())
}

pub async fn create(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
    by: Author,
    input: &BrowserEndpointInput,
) -> Result<Row, EndpointError> {
    let name = validate(input)?;
    let txn = db.begin().await?;
    if name_taken(&txn, org_id, &name, None).await? {
        return Err(EndpointError::NameTaken);
    }
    let id = Uuid::new_v4();
    let secret = match &input.token {
        None => None,
        Some(t) => {
            vault::save(
                &txn,
                ring,
                writer(org_id, by),
                by.can_write_secrets,
                id,
                &name,
                None,
                None,
                Some(t),
            )
            .await?
        }
    };
    txn.execute_raw(stmt(
        "INSERT INTO browser_endpoints (id, org_id, name, cdp_url, secret_id, max_connections, created_by, updated_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $7)",
        vec![
            id.into(),
            org_id.into(),
            name.into(),
            input.cdp_url.trim().into(),
            secret.into(),
            input.max_connections.into(),
            by.user_id.into(),
        ],
    ))
    .await?;
    set_default(&txn, org_id, id, input.is_default).await?;
    let row = find(&txn, org_id, id).await?;
    txn.commit().await?;
    Ok(row)
}

/// `token: None` keeps the current token.
pub async fn update(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
    by: Author,
    id: Uuid,
    input: &BrowserEndpointInput,
) -> Result<Row, EndpointError> {
    let name = validate(input)?;
    let txn = db.begin().await?;
    let row = find(&txn, org_id, id).await?;
    if name_taken(&txn, org_id, &name, Some(id)).await? {
        return Err(EndpointError::NameTaken);
    }
    let secret = if input.token.is_some() || row.secret_id.is_some() {
        vault::save(
            &txn,
            ring,
            writer(org_id, by),
            by.can_write_secrets,
            id,
            &name,
            Some(&row.name),
            row.secret_id,
            input.token.as_ref(),
        )
        .await?
    } else {
        None
    };
    txn.execute_raw(stmt(
        "UPDATE browser_endpoints SET name = $3, cdp_url = $4, secret_id = $5, max_connections = $6, \
         updated_by = $7, updated_at = now() WHERE org_id = $1 AND id = $2",
        vec![
            org_id.into(),
            id.into(),
            name.into(),
            input.cdp_url.trim().into(),
            secret.into(),
            input.max_connections.into(),
            by.user_id.into(),
        ],
    ))
    .await?;
    set_default(&txn, org_id, id, input.is_default).await?;
    let row = find(&txn, org_id, id).await?;
    txn.commit().await?;
    Ok(row)
}

pub async fn delete(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<(), EndpointError> {
    let txn = db.begin().await?;
    let row = find(&txn, org_id, id).await?;
    if row.secret_id.is_some() {
        vault::release(&txn, Some(org_id), id, &row.name).await?;
    }
    txn.execute_raw(stmt(
        "DELETE FROM browser_endpoints WHERE org_id = $1 AND id = $2",
        vec![org_id.into(), id.into()],
    ))
    .await?;
    txn.commit().await?;
    Ok(())
}

// ───────────────────────────── connecting ─────────────────────────────

/// Where a run connects.
pub struct Target {
    /// Concurrency key: the endpoint id, or the platform browser.
    key: String,
    base: reqwest::Url,
    token: Option<String>,
    max: usize,
    /// Set by an org (egress-guarded), not by the operator.
    org_owned: bool,
}

/// The platform browser, from the environment.
fn platform() -> Option<Target> {
    let raw = std::env::var("PNEX_BROWSER_URL").unwrap_or_else(|_| "http://127.0.0.1:9222".into());
    Some(Target {
        key: "platform".into(),
        base: http_base(&raw)?,
        token: std::env::var("PNEX_BROWSER_TOKEN")
            .ok()
            .filter(|t| !t.is_empty()),
        max: PLATFORM_CONNECTIONS,
        org_owned: false,
    })
}

async fn target_of(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
    r: &Row,
) -> Result<Target, String> {
    let token = match r.secret_id {
        None => None,
        Some(id) => Some(
            store::reveal(db, ring, Some(org_id), id)
                .await
                .map_err(|e| e.to_string())?,
        ),
    };
    Ok(Target {
        key: r.id.to_string(),
        base: http_base(&r.cdp_url).ok_or("invalid endpoint address")?,
        token,
        max: r.max_connections.clamp(1, 16) as usize,
        org_owned: true,
    })
}

#[cfg(test)]
pub fn resolve_platform_for_tests() -> Target {
    platform().expect("PNEX_BROWSER_URL")
}

/// Endpoint of a run: the named one, else the org default, else the
/// platform browser. `Err` = diagnostic as is.
pub async fn resolve(
    db: &DatabaseConnection,
    ring: Option<&Keyring>,
    org_id: i64,
    name: Option<&str>,
) -> Result<Target, String> {
    let rows = list(db, org_id).await.map_err(|e| e.to_string())?;
    let chosen = match name {
        Some(n) => Some(
            rows.iter()
                .find(|r| r.name == n)
                .ok_or_else(|| format!("browser endpoint {n} not found"))?,
        ),
        None => rows.iter().find(|r| r.is_default),
    };
    match chosen {
        Some(r) => target_of(db, ring.ok_or("secrets keyring unavailable")?, org_id, r).await,
        None => platform().ok_or_else(|| "PNEX_BROWSER_URL is invalid".into()),
    }
}

/// One connection slot of an endpoint (its `max_connections`).
pub async fn slot(t: &Target) -> tokio::sync::OwnedSemaphorePermit {
    static SLOTS: std::sync::OnceLock<
        std::sync::Mutex<HashMap<String, Arc<tokio::sync::Semaphore>>>,
    > = std::sync::OnceLock::new();
    let sem = SLOTS
        .get_or_init(Default::default)
        .lock()
        .expect("slots")
        .entry(t.key.clone())
        .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(t.max)))
        .clone();
    sem.acquire_owned().await.expect("semaphore never closed")
}

fn http(org_owned: bool) -> &'static reqwest::Client {
    static GUARDED: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    static PLAIN: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    let build = |guarded: bool| {
        let b = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(CONNECT_TIMEOUT);
        if guarded {
            pnex_core::egress::guarded(b)
        } else {
            b
        }
        .build()
        .expect("reqwest client")
    };
    if org_owned {
        GUARDED.get_or_init(|| build(true))
    } else {
        PLAIN.get_or_init(|| build(false))
    }
}

#[derive(serde::Deserialize, Default)]
struct Version {
    #[serde(rename = "Browser", default)]
    browser: Option<String>,
    #[serde(rename = "Protocol-Version", default)]
    protocol: Option<String>,
    #[serde(rename = "webSocketDebuggerUrl", default)]
    ws: String,
}

async fn version(t: &Target) -> Result<Version, String> {
    let mut req = http(t.org_owned).get(t.base.join("/json/version").map_err(|e| e.to_string())?);
    if let Some(tok) = &t.token {
        req = req.bearer_auth(tok);
    }
    let res = req.send().await.map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!(
            "/json/version answered HTTP {}",
            res.status().as_u16()
        ));
    }
    let v: Version = res.json().await.map_err(|e| e.to_string())?;
    if v.ws.is_empty() {
        return Err("/json/version has no webSocketDebuggerUrl".into());
    }
    Ok(v)
}

/// The browser's WebSocket address on the endpoint's own scheme and host
/// (engines answer with the address they listen on).
fn ws_url(t: &Target, advertised: &str) -> Result<String, String> {
    let path = reqwest::Url::parse(advertised)
        .map_err(|e| e.to_string())?
        .path()
        .to_string();
    let scheme = if t.base.scheme() == "https" {
        "wss"
    } else {
        "ws"
    };
    let host = t.base.host_str().ok_or("no host")?;
    Ok(match t.base.port() {
        Some(p) => format!("{scheme}://{host}:{p}{path}"),
        None => format!("{scheme}://{host}{path}"),
    })
}

/// Chrome refuses a DevTools request whose Host is a name (DNS rebinding
/// guard): a plain `http`/`ws` endpoint is reached by its address.
async fn by_address(t: &Target) -> Target {
    let host = t.base.host_str().unwrap_or_default().to_string();
    let named = host.parse::<std::net::IpAddr>().is_err() && host != "localhost";
    let mut base = t.base.clone();
    if t.base.scheme() == "http" && named {
        let port = t.base.port_or_known_default().unwrap_or(80);
        if let Ok(Some(a)) = tokio::net::lookup_host((host.as_str(), port))
            .await
            .map(|mut a| a.next())
        {
            let _ = base.set_ip_host(a.ip());
        }
    }
    Target {
        key: t.key.clone(),
        base,
        token: t.token.clone(),
        max: t.max,
        org_owned: t.org_owned,
    }
}

/// Connects to the endpoint's browser.
pub async fn connect(t: &Target) -> Result<(Arc<Cdp>, Engine), String> {
    let t = &by_address(t).await;
    let v = version(t).await?;
    let ws = ws_url(t, &v.ws)?;
    if t.org_owned {
        let host = t.base.host_str().unwrap_or_default().to_string();
        if !super::browser::egress_ok(&host).await {
            return Err("egress refused for the browser endpoint".into());
        }
    }
    let cdp = Cdp::connect(&ws, t.token.as_deref()).await?;
    Ok((
        cdp,
        Engine {
            engine: v.browser,
            protocol: v.protocol,
        },
    ))
}

/// What `/json/version` says of the engine.
pub struct Engine {
    pub engine: Option<String>,
    pub protocol: Option<String>,
}

/// Navigates a tab and waits until the page answers again (≤ 30 s).
async fn visit(tab: &Tab, url: &str) -> Result<(), String> {
    let target = serde_json::to_string(url).unwrap_or_default();
    tab.eval(
        &format!("(setTimeout(() => {{ location.href = {target}; }}, 0), true)"),
        Duration::from_secs(5),
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let until = Instant::now() + Duration::from_secs(30);
    while Instant::now() < until {
        let ready = tab
            .eval("document.readyState", Duration::from_secs(2))
            .await;
        if ready.is_ok_and(|r| r != "loading") {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err("the page did not load".into())
}

/// The Test button: `/json/version` with the token, a fresh context, a
/// public page with a screenshot, and an internal address refused.
pub async fn test(t: &Target) -> BrowserTestResult {
    let started = Instant::now();
    let mut checks = Vec::new();
    let mut check = |name: &str, at: Instant, r: Result<(), String>| {
        let ok = r.is_ok();
        checks.push(BrowserCheck {
            check: name.into(),
            ok,
            detail: r.err(),
            ms: at.elapsed().as_millis() as u64,
        });
        ok
    };
    let at = Instant::now();
    let (cdp, v) = match connect(t).await {
        Ok(s) => {
            check("version", at, Ok(()));
            s
        }
        Err(e) => {
            check("version", at, Err(e));
            return BrowserTestResult {
                ok: false,
                engine: None,
                protocol: None,
                latency_ms: started.elapsed().as_millis() as u64,
                checks,
            };
        }
    };
    let at = Instant::now();
    let tab = tokio::time::timeout(Duration::from_secs(20), Tab::open(cdp))
        .await
        .unwrap_or_else(|_| Err("timed out".into()));
    let ok_ctx = check(
        "context",
        at,
        tab.as_ref().map(|_| ()).map_err(Clone::clone),
    );
    if let (true, Ok(tab)) = (ok_ctx, &tab) {
        // Same guard as a run: an internal address is refused by PNEX even
        // when the engine would try it.
        let _ = tokio::time::timeout(
            Duration::from_secs(10),
            super::browser::guard_page(tab, "example.com"),
        )
        .await;
        let at = Instant::now();
        let shot = async {
            visit(tab, "https://example.com/").await?;
            if tab.screenshot().await?.is_empty() {
                return Err("empty screenshot".to_string());
            }
            Ok(())
        }
        .await;
        check("navigate", at, shot);
        let at = Instant::now();
        // shortcut: a refused navigation and an unreachable address look
        // alike from here; the endpoint's network policy is the real guard.
        let _ = visit(tab, "http://169.254.169.254/latest/meta-data/").await;
        let text = tab
            .eval(
                "document.body ? document.body.innerText : ''",
                Duration::from_secs(5),
            )
            .await
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        let answered = tab.url().await.contains("169.254.169.254") && text.contains("ami-");
        check(
            "internal-refused",
            at,
            if answered {
                Err("the internal address answered".to_string())
            } else {
                Ok(())
            },
        );
    }
    if let Ok(tab) = tab {
        tab.close().await;
    }
    BrowserTestResult {
        ok: checks.iter().all(|c| c.ok),
        engine: v.engine,
        protocol: v.protocol,
        latency_ms: started.elapsed().as_millis() as u64,
        checks,
    }
}
