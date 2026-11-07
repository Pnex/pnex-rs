//! Connectors (data-pipelines.md D352): saved access to an outside database. Only
//! this server connects, through the egress rule of `pnex_core::connectors`, to the
//! address it checked, so that a name resolving elsewhere later changes nothing.

use std::net::SocketAddr;
use std::time::Duration;

use pnex_core::connectors::{
    egress_policy, Connector, ConnectorHosts, ConnectorInput, ConnectorKind, ConnectorTest,
    RestAuth, RestSettings, WebhookSettings, ALLOWED_HOSTS_MAX, HOST_MAX,
};
use pnex_core::err_codes;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, QueryResult, Statement,
    TransactionTrait,
};
use sqlx::mysql::{MySqlConnectOptions, MySqlConnection, MySqlSslMode};
use sqlx::postgres::{PgConnectOptions, PgSslMode};
use sqlx::{ConnectOptions, Connection, Executor, PgConnection};
use uuid::Uuid;

use crate::services::secrets::store::{self, StoreError, Writer};
use crate::services::secrets::{connector as vault, Keyring};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug)]
pub enum ConnectorError {
    Invalid { field: String, token: String },
    NotFound,
    NameTaken,
    VersionConflict { current: i32 },
    Store(StoreError),
    Db(DbErr),
}

impl From<DbErr> for ConnectorError {
    fn from(e: DbErr) -> Self {
        Self::Db(e)
    }
}

impl From<StoreError> for ConnectorError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

/// Who writes: a typed password needs the right to write secrets.
#[derive(Clone, Copy)]
pub struct Author {
    pub user_id: i64,
    pub can_write_secrets: bool,
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

const SELECT: &str = "SELECT id, name, kind, host, port, database, username, tls, \
    settings, secret_id IS NOT NULL AS has_password, version, \
    to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS created_at, \
    to_char(updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS updated_at \
    FROM connectors";

fn kind_of(r: &QueryResult) -> Result<ConnectorKind, DbErr> {
    let wire: String = r.try_get("", "kind")?;
    ConnectorKind::parse(&wire)
        .ok_or_else(|| DbErr::Custom(format!("unknown connector kind {wire}")))
}

/// The settings of a REST connector; a stored value that no longer parses reads as
/// the defaults rather than failing the whole list.
fn rest_of(r: &QueryResult, kind: ConnectorKind) -> Result<Option<RestSettings>, DbErr> {
    if kind != ConnectorKind::Rest {
        return Ok(None);
    }
    let v: serde_json::Value = r.try_get("", "settings")?;
    Ok(Some(serde_json::from_value(v).unwrap_or_default()))
}

fn webhook_of(r: &QueryResult, kind: ConnectorKind) -> Result<Option<WebhookSettings>, DbErr> {
    if kind != ConnectorKind::Webhook {
        return Ok(None);
    }
    let v: serde_json::Value = r.try_get("", "settings")?;
    Ok(Some(serde_json::from_value(v).unwrap_or_default()))
}

/// What goes into the `settings` column.
fn settings_json(input: &ConnectorInput) -> String {
    let json = match input.kind {
        ConnectorKind::Rest => input.rest.as_ref().map(serde_json::to_string),
        ConnectorKind::Webhook => Some(serde_json::to_string(
            &input.webhook.clone().unwrap_or_default(),
        )),
        ConnectorKind::Postgres | ConnectorKind::Mysql => None,
    };
    json.and_then(Result::ok).unwrap_or_else(|| "{}".into())
}

fn view(r: &QueryResult) -> Result<Connector, DbErr> {
    let kind = kind_of(r)?;
    Ok(Connector {
        id: r.try_get::<Uuid>("", "id")?.to_string(),
        name: r.try_get("", "name")?,
        kind,
        host: r.try_get("", "host")?,
        port: r.try_get("", "port")?,
        database: r.try_get("", "database")?,
        username: r.try_get("", "username")?,
        tls: r.try_get("", "tls")?,
        rest: rest_of(r, kind)?,
        webhook: webhook_of(r, kind)?,
        has_password: r.try_get("", "has_password")?,
        version: r.try_get("", "version")?,
        created_at: r.try_get("", "created_at")?,
        updated_at: r.try_get("", "updated_at")?,
    })
}

fn checked(input: &ConnectorInput) -> Result<(), ConnectorError> {
    match input.violation() {
        Some((field, token)) => Err(ConnectorError::Invalid {
            field: field.into(),
            token,
        }),
        None => Ok(()),
    }
}

pub async fn get(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
) -> Result<Connector, ConnectorError> {
    let sql = format!("{SELECT} WHERE org_id = $1 AND id = $2");
    let row = db
        .query_one_raw(stmt(&sql, vec![org_id.into(), id.into()]))
        .await?
        .ok_or(ConnectorError::NotFound)?;
    Ok(view(&row)?)
}

pub async fn list(
    db: &DatabaseConnection,
    org_id: i64,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<Connector>), ConnectorError> {
    let total = db
        .query_one_raw(stmt(
            "SELECT count(*) AS n FROM connectors WHERE org_id = $1",
            vec![org_id.into()],
        ))
        .await?
        .map_or(Ok(0), |r| r.try_get("", "n"))?;
    let sql = format!("{SELECT} WHERE org_id = $1 ORDER BY name LIMIT $2 OFFSET $3");
    let rows = db
        .query_all_raw(stmt(&sql, vec![org_id.into(), limit.into(), offset.into()]))
        .await?;
    Ok((total, rows.iter().map(view).collect::<Result<_, _>>()?))
}

fn writer(org_id: i64, by: Author) -> Writer {
    Writer {
        org_id: Some(org_id),
        user_id: Some(by.user_id),
    }
}

pub async fn create(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
    by: Author,
    input: &ConnectorInput,
) -> Result<Connector, ConnectorError> {
    checked(input)?;
    let name = input.name.trim();
    let id = Uuid::new_v4();
    let txn = db.begin().await?;
    let taken = txn
        .query_one_raw(stmt(
            "SELECT 1 AS x FROM connectors WHERE org_id = $1 AND name = $2",
            vec![org_id.into(), name.into()],
        ))
        .await?
        .is_some();
    if taken {
        return Err(ConnectorError::NameTaken);
    }
    let secret = vault::save(
        &txn,
        ring,
        writer(org_id, by),
        by.can_write_secrets,
        id,
        name,
        None,
        None,
        input.password.as_ref(),
    )
    .await?;
    txn.execute_raw(stmt(
        "INSERT INTO connectors (id, org_id, name, kind, host, port, database, username, tls, \
           secret_id, created_by, settings) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12::jsonb)",
        vec![
            id.into(),
            org_id.into(),
            name.into(),
            input.kind.wire().into(),
            input.host.trim().into(),
            input.port.into(),
            input.database.trim().into(),
            input.username.trim().into(),
            input.tls.into(),
            secret.into(),
            by.user_id.into(),
            settings_json(input).into(),
        ],
    ))
    .await?;
    txn.commit().await?;
    get(db, org_id, id).await
}

pub async fn update(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
    by: Author,
    id: Uuid,
    input: &ConnectorInput,
) -> Result<Connector, ConnectorError> {
    checked(input)?;
    let Some(expected) = input.expected_version else {
        return Err(ConnectorError::Invalid {
            field: "expected_version".into(),
            token: "required".into(),
        });
    };
    let current = get(db, org_id, id).await?;
    let name = input.name.trim();
    let txn = db.begin().await?;
    let row = txn
        .query_one_raw(stmt(
            "SELECT secret_id FROM connectors WHERE org_id = $1 AND id = $2 FOR UPDATE",
            vec![org_id.into(), id.into()],
        ))
        .await?
        .ok_or(ConnectorError::NotFound)?;
    let held: Option<Uuid> = row.try_get("", "secret_id")?;
    let taken = txn
        .query_one_raw(stmt(
            "SELECT 1 AS x FROM connectors WHERE org_id = $1 AND name = $2 AND id <> $3",
            vec![org_id.into(), name.into(), id.into()],
        ))
        .await?
        .is_some();
    if taken {
        return Err(ConnectorError::NameTaken);
    }
    let secret = vault::save(
        &txn,
        ring,
        writer(org_id, by),
        by.can_write_secrets,
        id,
        name,
        Some(&current.name),
        held,
        input.password.as_ref(),
    )
    .await?;
    // A connector pointed elsewhere keeps no password unless one is given again: the
    // stored password must not be sent to a host its author never meant it for.
    let token_url = |r: Option<&RestSettings>| r.map(|r| r.token_url.trim().to_string());
    let moved = current.host != input.host.trim()
        || current.port != input.port
        || current.kind != input.kind
        || token_url(current.rest.as_ref()) != token_url(input.rest.as_ref());
    if moved && input.password.is_none() && held.is_some() {
        return Err(ConnectorError::Invalid {
            field: "password".into(),
            token: "required".into(),
        });
    }
    let done = txn
        .execute_raw(stmt(
            "UPDATE connectors SET name = $3, kind = $4, host = $5, port = $6, database = $7, \
               username = $8, tls = $9, secret_id = $10, settings = $12::jsonb, \
               version = version + 1, updated_at = now() \
             WHERE org_id = $1 AND id = $2 AND version = $11",
            vec![
                org_id.into(),
                id.into(),
                name.into(),
                input.kind.wire().into(),
                input.host.trim().into(),
                input.port.into(),
                input.database.trim().into(),
                input.username.trim().into(),
                input.tls.into(),
                secret.into(),
                expected.into(),
                settings_json(input).into(),
            ],
        ))
        .await?;
    if done.rows_affected() != 1 {
        return Err(ConnectorError::VersionConflict {
            current: current.version,
        });
    }
    txn.commit().await?;
    get(db, org_id, id).await
}

pub async fn delete(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<(), ConnectorError> {
    let current = get(db, org_id, id).await?;
    let txn = db.begin().await?;
    vault::release(&txn, Some(org_id), id, &current.name).await?;
    txn.execute_raw(stmt(
        "DELETE FROM connectors WHERE org_id = $1 AND id = $2",
        vec![org_id.into(), id.into()],
    ))
    .await?;
    txn.commit().await?;
    Ok(())
}

pub async fn allowed_hosts(db: &DatabaseConnection, org_id: i64) -> Result<ConnectorHosts, DbErr> {
    let rows = db
        .query_all_raw(stmt(
            "SELECT host FROM connector_allowed_hosts WHERE org_id = $1 ORDER BY host",
            vec![org_id.into()],
        ))
        .await?;
    let hosts = rows
        .iter()
        .map(|r| r.try_get("", "host"))
        .collect::<Result<_, _>>()?;
    Ok(ConnectorHosts { hosts })
}

/// Replaces the list. Platform admin only (Q-P2): every member owns a personal org,
/// so an org admin must not open the network of the deployment.
pub async fn set_allowed_hosts(
    db: &DatabaseConnection,
    org_id: i64,
    hosts: &[String],
    by: i64,
) -> Result<ConnectorHosts, ConnectorError> {
    let invalid = |token: String| ConnectorError::Invalid {
        field: "hosts".into(),
        token,
    };
    if hosts.len() > ALLOWED_HOSTS_MAX {
        return Err(invalid(format!("max_items:{ALLOWED_HOSTS_MAX}")));
    }
    let clean: Vec<String> = hosts.iter().map(|h| h.trim().to_lowercase()).collect();
    if clean.iter().any(|h| h.is_empty() || h.len() > HOST_MAX) {
        return Err(invalid("invalid".into()));
    }
    let txn = db.begin().await?;
    txn.execute_raw(stmt(
        "DELETE FROM connector_allowed_hosts WHERE org_id = $1",
        vec![org_id.into()],
    ))
    .await?;
    for host in &clean {
        txn.execute_raw(stmt(
            "INSERT INTO connector_allowed_hosts (org_id, host, updated_by) VALUES ($1, $2, $3) \
             ON CONFLICT DO NOTHING",
            vec![org_id.into(), host.clone().into(), by.into()],
        ))
        .await?;
    }
    txn.commit().await?;
    Ok(allowed_hosts(db, org_id).await?)
}

/// A checked way to reach a connector. Holds its secret as long as the run needs it.
pub(crate) enum Target {
    Pg(Box<PgTarget>),
    My(Box<MyTarget>),
    Rest(Box<RestTarget>),
    /// Reached by nobody: an import reads its inbox (D795).
    Webhook {
        id: Uuid,
        has_secret: bool,
    },
}

pub(crate) struct PgTarget {
    options: PgConnectOptions,
}

/// Why no connection: codes only, the text of an outside server stays in the logs.
pub(crate) type Refusal = &'static str;

/// Resolves the connector of the org with its secret and the address it may reach.
pub(crate) async fn target<C: ConnectionTrait>(
    db: &C,
    ring: &Keyring,
    org_id: i64,
    name: Option<&str>,
    id: Option<Uuid>,
) -> Result<Target, Refusal> {
    let by_db = |_e: DbErr| err_codes::CONNECTOR_UNREACHABLE;
    const COLUMNS: &str =
        "SELECT id, kind, host, port, database, username, tls, settings, secret_id FROM connectors";
    let row = match (name, id) {
        (Some(name), _) => {
            db.query_one_raw(stmt(
                &format!("{COLUMNS} WHERE org_id = $1 AND name = $2"),
                vec![org_id.into(), name.into()],
            ))
            .await
        }
        (_, Some(id)) => {
            db.query_one_raw(stmt(
                &format!("{COLUMNS} WHERE org_id = $1 AND id = $2"),
                vec![org_id.into(), id.into()],
            ))
            .await
        }
        _ => Ok(None),
    }
    .map_err(by_db)?
    .ok_or(err_codes::CONNECTOR_NOT_FOUND)?;
    let kind = kind_of(&row).map_err(by_db)?;
    if kind == ConnectorKind::Webhook {
        let secret: Option<Uuid> = row.try_get("", "secret_id").map_err(by_db)?;
        return Ok(Target::Webhook {
            id: row.try_get("", "id").map_err(by_db)?,
            has_secret: secret.is_some(),
        });
    }
    let host: String = row.try_get("", "host").map_err(by_db)?;
    let port: i32 = row.try_get("", "port").map_err(by_db)?;
    let tls: bool = row.try_get("", "tls").map_err(by_db)?;
    let username: String = row.try_get("", "username").map_err(by_db)?;
    let secret: Option<Uuid> = row.try_get("", "secret_id").map_err(by_db)?;

    let listed = db
        .query_one_raw(stmt(
            "SELECT 1 AS x FROM connector_allowed_hosts WHERE org_id = $1 AND host = $2",
            vec![org_id.into(), host.to_lowercase().into()],
        ))
        .await
        .map_err(by_db)?
        .is_some();
    let process = pnex_core::egress::policy();
    let policy = egress_policy(listed, process);
    if pnex_core::egress::check_host(&host, policy).is_some() {
        return Err(err_codes::CONNECTOR_HOST_REFUSED);
    }
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port as u16))
        .await
        .map_err(|_| err_codes::CONNECTOR_UNREACHABLE)?
        .collect();
    let Some(addr) = pnex_core::egress::filter_addrs(addrs, policy)
        .into_iter()
        .next()
    else {
        return Err(err_codes::CONNECTOR_HOST_REFUSED);
    };
    // Without TLS the secret crosses the network in the clear: a host the platform
    // admin listed only, or a development server.
    if !tls && !listed && process != pnex_core::egress::EgressPolicy::Open {
        return Err(err_codes::CONNECTOR_HOST_REFUSED);
    }
    let secret = match secret {
        Some(secret) => Some(
            store::reveal(db, ring, Some(org_id), secret)
                .await
                .map_err(|_| err_codes::CONNECTOR_UNREACHABLE)?,
        ),
        None => None,
    };
    if kind == ConnectorKind::Rest {
        let settings = rest_of(&row, kind).map_err(by_db)?.unwrap_or_default();
        return RestTarget::new(host, addr, tls, settings, username, secret)
            .map(|t| Target::Rest(Box::new(t)));
    }
    if kind == ConnectorKind::Mysql {
        // TLS is required by the input check; a row older than it is refused here
        // too, so that the password never goes out RSA-encrypted (D792).
        if !tls {
            return Err(err_codes::CONNECTOR_HOST_REFUSED);
        }
        let mut options = MySqlConnectOptions::new()
            .host(&addr.ip().to_string())
            .port(addr.port())
            .database(
                row.try_get::<String>("", "database")
                    .map_err(by_db)?
                    .as_str(),
            )
            .username(&username)
            // shortcut: TLS without checking the certificate, as for PostgreSQL above.
            .ssl_mode(MySqlSslMode::Required);
        if let Some(password) = &secret {
            options = options.password(password);
        }
        return Ok(Target::My(Box::new(MyTarget { options })));
    }
    let mut options = PgConnectOptions::new()
        .host(&addr.ip().to_string())
        .port(addr.port())
        .database(
            row.try_get::<String>("", "database")
                .map_err(by_db)?
                .as_str(),
        )
        .username(&username)
        // shortcut: TLS without checking the certificate, since the connection goes to
        // the checked address and not the name; pass the name to verify it when a
        // customer asks for it.
        .ssl_mode(if tls {
            PgSslMode::Require
        } else {
            PgSslMode::Disable
        })
        .application_name("pnex-connector")
        // Set at startup: a query cannot undo it with a COMMIT.
        .options([("default_transaction_read_only", "on")]);
    if let Some(password) = secret {
        options = options.password(&password);
    }
    Ok(Target::Pg(Box::new(PgTarget { options })))
}

impl PgTarget {
    pub(crate) async fn connect(
        self,
        statement_timeout: Duration,
    ) -> Result<PgConnection, Refusal> {
        let timeout = format!("{}", statement_timeout.as_millis());
        let options = self
            .options
            .options([("statement_timeout", timeout.as_str())])
            .disable_statement_logging();
        match tokio::time::timeout(CONNECT_TIMEOUT, options.connect()).await {
            Ok(Ok(conn)) => Ok(conn),
            Ok(Err(e)) => {
                tracing::info!(error = %e, "connector not connected");
                Err(err_codes::CONNECTOR_UNREACHABLE)
            }
            Err(_) => Err(err_codes::CONNECTOR_UNREACHABLE),
        }
    }
}

pub(crate) struct MyTarget {
    options: MySqlConnectOptions,
}

impl MyTarget {
    /// A session that reads only, with a time limit per statement.
    pub(crate) async fn connect(self, timeout: Duration) -> Result<MySqlConnection, Refusal> {
        let options = self.options.disable_statement_logging();
        let mut conn = match tokio::time::timeout(CONNECT_TIMEOUT, options.connect()).await {
            Ok(Ok(conn)) => conn,
            Ok(Err(e)) => {
                tracing::info!(error = %e, "connector not connected");
                return Err(err_codes::CONNECTOR_UNREACHABLE);
            }
            Err(_) => return Err(err_codes::CONNECTOR_UNREACHABLE),
        };
        if (&mut conn)
            .execute("SET SESSION TRANSACTION READ ONLY")
            .await
            .is_err()
        {
            return Err(err_codes::CONNECTOR_UNREACHABLE);
        }
        // MySQL only; MariaDB has no such variable and keeps the run's own limit.
        let limit = format!("SET SESSION MAX_EXECUTION_TIME = {}", timeout.as_millis());
        let _ = (&mut conn).execute(sqlx::AssertSqlSafe(limit)).await;
        Ok(conn)
    }
}

/// Tries per request on a 429, a 5xx or a network failure.
const REST_TRIES: u32 = 5;
const REST_TIMEOUT: Duration = Duration::from_secs(120);
const TOKEN_MAX_BYTES: usize = 64 * 1024;

/// A REST connector ready to send GETs (connectors.md D793). The client reaches the
/// connector's host at the address checked above only, follows no redirect, and every
/// other host (the OAuth2 token endpoint) goes through the guarded resolver.
pub(crate) struct RestTarget {
    client: reqwest::Client,
    base: reqwest::Url,
    settings: RestSettings,
    username: String,
    secret: Option<String>,
    token: Option<String>,
}

impl RestTarget {
    fn new(
        host: String,
        addr: SocketAddr,
        tls: bool,
        settings: RestSettings,
        username: String,
        secret: Option<String>,
    ) -> Result<Self, Refusal> {
        let scheme = if tls { "https" } else { "http" };
        // An IPv6 literal needs brackets inside a URL.
        let shown = if host.contains(':') {
            format!("[{host}]")
        } else {
            host.clone()
        };
        let base = reqwest::Url::parse(&format!(
            "{scheme}://{shown}:{}{}",
            addr.port(),
            settings.base_path
        ))
        .map_err(|_| err_codes::CONNECTOR_UNREACHABLE)?;
        let client = pnex_core::egress::guarded(reqwest::Client::builder())
            .resolve(&host, addr)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REST_TIMEOUT)
            .user_agent("pnex-connector")
            .build()
            .map_err(|_| err_codes::CONNECTOR_UNREACHABLE)?;
        Ok(Self {
            client,
            base,
            settings,
            username,
            secret,
            token: None,
        })
    }

    /// The base URL followed by `path` (already checked by `RestQuery::check`).
    pub(crate) fn url(&self, path: &str) -> reqwest::Url {
        let mut url = self.base.clone();
        if !path.is_empty() {
            let joined = format!("{}{path}", url.path().trim_end_matches('/'));
            url.set_path(&joined);
        }
        url
    }

    /// Whether `url` stays on the connector's scheme, host and port.
    pub(crate) fn same_origin(&self, url: &reqwest::Url) -> bool {
        url.origin() == self.base.origin()
    }

    async fn bearer(&mut self) -> Result<Option<String>, Refusal> {
        if self.settings.auth != RestAuth::Oauth2ClientCredentials {
            return Ok(None);
        }
        if let Some(t) = &self.token {
            return Ok(Some(t.clone()));
        }
        let mut form = vec![("grant_type", "client_credentials".to_string())];
        if !self.settings.scope.trim().is_empty() {
            form.push(("scope", self.settings.scope.trim().to_string()));
        }
        let res = self
            .client
            .post(&self.settings.token_url)
            .basic_auth(&self.username, self.secret.as_deref())
            .form(&form)
            .send()
            .await
            .map_err(|e| {
                tracing::info!(error = %e, "connector token request failed");
                err_codes::CONNECTOR_UNREACHABLE
            })?;
        if !res.status().is_success() {
            tracing::info!(status = %res.status(), "connector token refused");
            return Err(err_codes::CONNECTOR_REST_DENIED);
        }
        let mut res = res;
        let mut body = Vec::new();
        while let Some(chunk) = res
            .chunk()
            .await
            .map_err(|_| err_codes::CONNECTOR_UNREACHABLE)?
        {
            body.extend_from_slice(&chunk);
            if body.len() > TOKEN_MAX_BYTES {
                return Err(err_codes::CONNECTOR_REST_NOT_JSON);
            }
        }
        let token = serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("access_token")?.as_str().map(str::to_string))
            .ok_or(err_codes::CONNECTOR_REST_NOT_JSON)?;
        // shortcut: the token is kept for the whole run, not refreshed at expiry; a run
        // longer than the token's life fails with rest-denied, refresh when one does.
        self.token = Some(token.clone());
        Ok(Some(token))
    }

    /// One GET with the connector's credentials, retried on 429, 5xx and network
    /// failures. A refused or unexpected status is a code, never the server's text.
    pub(crate) async fn get(
        &mut self,
        url: reqwest::Url,
        params: &[(String, String)],
    ) -> Result<reqwest::Response, Refusal> {
        let bearer = self.bearer().await?;
        let mut wait = Duration::from_secs(1);
        for attempt in 1..=REST_TRIES {
            let mut req = self.client.get(url.clone()).query(params);
            req = match (self.settings.auth, &bearer, &self.secret) {
                (RestAuth::Oauth2ClientCredentials, Some(t), _) => req.bearer_auth(t),
                (RestAuth::Bearer, _, Some(s)) => req.bearer_auth(s),
                (RestAuth::Header, _, Some(s)) => req.header(self.settings.header_name.as_str(), s),
                (RestAuth::Basic, _, s) => req.basic_auth(&self.username, s.as_deref()),
                _ => req,
            };
            let last = attempt == REST_TRIES;
            match req.send().await {
                Ok(res) if res.status().is_success() => return Ok(res),
                Ok(res) => {
                    let status = res.status();
                    let again = status.as_u16() == 429 || status.is_server_error();
                    if !again || last {
                        tracing::info!(%status, "connector request refused");
                        return Err(match status.as_u16() {
                            401 | 403 => err_codes::CONNECTOR_REST_DENIED,
                            _ => err_codes::CONNECTOR_REST_STATUS,
                        });
                    }
                    let asked = res
                        .headers()
                        .get(reqwest::header::RETRY_AFTER)
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.trim().parse::<u64>().ok());
                    if let Some(secs) = asked {
                        wait = Duration::from_secs(secs.min(60));
                    }
                }
                Err(e) if last => {
                    tracing::info!(error = %e, "connector request failed");
                    return Err(err_codes::CONNECTOR_UNREACHABLE);
                }
                Err(_) => {}
            }
            tokio::time::sleep(wait).await;
            wait = (wait * 2).min(Duration::from_secs(60));
        }
        Err(err_codes::CONNECTOR_UNREACHABLE)
    }
}

/// Tries the connection, without returning any data.
pub async fn test(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
    id: Uuid,
) -> Result<ConnectorTest, ConnectorError> {
    get(db, org_id, id).await?;
    let outcome = async {
        match target(db, ring, org_id, None, Some(id)).await? {
            Target::Pg(target) => {
                let mut conn = target.connect(Duration::from_secs(10)).await?;
                conn.ping().await.map_err(|e| {
                    tracing::info!(error = %e, "connector ping failed");
                    err_codes::CONNECTOR_UNREACHABLE
                })?;
                let _ = conn.close().await;
            }
            Target::My(target) => {
                let mut conn = target.connect(Duration::from_secs(10)).await?;
                conn.ping().await.map_err(|e| {
                    tracing::info!(error = %e, "connector ping failed");
                    err_codes::CONNECTOR_UNREACHABLE
                })?;
                let _ = conn.close().await;
            }
            Target::Rest(mut target) => {
                let base = target.url("");
                target.get(base, &[]).await?;
            }
            Target::Webhook { has_secret, .. } => {
                if !has_secret {
                    return Err(err_codes::HOOK_SECRET_MISSING);
                }
            }
        }
        Ok::<(), Refusal>(())
    }
    .await;
    Ok(match outcome {
        Ok(()) => ConnectorTest {
            ok: true,
            code: None,
        },
        Err(code) => ConnectorTest {
            ok: false,
            code: Some(code.to_string()),
        },
    })
}

/// Gives the connector a new secret PneX generated, returned this once (D795). The
/// sender must be given it again: the previous one stops working at once.
pub async fn regenerate_secret(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
    by: Author,
    id: Uuid,
) -> Result<String, ConnectorError> {
    let current = get(db, org_id, id).await?;
    let mut bytes = [0u8; 32];
    rand::RngExt::fill(&mut rand::rng(), &mut bytes);
    let secret: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let txn = db.begin().await?;
    let row = txn
        .query_one_raw(stmt(
            "SELECT secret_id FROM connectors WHERE org_id = $1 AND id = $2 FOR UPDATE",
            vec![org_id.into(), id.into()],
        ))
        .await?
        .ok_or(ConnectorError::NotFound)?;
    let held: Option<Uuid> = row.try_get("", "secret_id")?;
    let saved = vault::save(
        &txn,
        ring,
        writer(org_id, by),
        by.can_write_secrets,
        id,
        &current.name,
        Some(&current.name),
        held,
        Some(&pnex_core::SecretFieldInput::Value {
            value: secret.clone(),
        }),
    )
    .await?;
    txn.execute_raw(stmt(
        "UPDATE connectors SET secret_id = $3, version = version + 1, updated_at = now() \
         WHERE org_id = $1 AND id = $2",
        vec![org_id.into(), id.into(), saved.into()],
    ))
    .await?;
    txn.commit().await?;
    Ok(secret)
}
