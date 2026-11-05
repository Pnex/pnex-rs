//! Connectors (data-pipelines.md D352): saved access to an outside database. Only
//! this server connects, through the egress rule of `pnex_core::connectors`, to the
//! address it checked, so that a name resolving elsewhere later changes nothing.

use std::net::SocketAddr;
use std::time::Duration;

use pnex_core::connectors::{
    egress_policy, Connector, ConnectorHosts, ConnectorInput, ConnectorKind, ConnectorTest,
    ALLOWED_HOSTS_MAX, HOST_MAX,
};
use pnex_core::err_codes;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, QueryResult, Statement,
    TransactionTrait,
};
use sqlx::postgres::{PgConnectOptions, PgSslMode};
use sqlx::{ConnectOptions, Connection, PgConnection};
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
    secret_id IS NOT NULL AS has_password, version, \
    to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS created_at, \
    to_char(updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS updated_at \
    FROM connectors";

fn view(r: &QueryResult) -> Result<Connector, DbErr> {
    Ok(Connector {
        id: r.try_get::<Uuid>("", "id")?.to_string(),
        name: r.try_get("", "name")?,
        kind: ConnectorKind::Postgres,
        host: r.try_get("", "host")?,
        port: r.try_get("", "port")?,
        database: r.try_get("", "database")?,
        username: r.try_get("", "username")?,
        tls: r.try_get("", "tls")?,
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
           secret_id, created_by) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
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
    let moved = current.host != input.host.trim() || current.port != input.port;
    if moved && input.password.is_none() && held.is_some() {
        return Err(ConnectorError::Invalid {
            field: "password".into(),
            token: "required".into(),
        });
    }
    let done = txn
        .execute_raw(stmt(
            "UPDATE connectors SET name = $3, kind = $4, host = $5, port = $6, database = $7, \
               username = $8, tls = $9, secret_id = $10, version = version + 1, updated_at = now() \
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

/// A checked way to open a connection. Holds the password as long as the run needs it.
pub(crate) struct Target {
    options: PgConnectOptions,
}

/// Why no connection: codes only, the text of an outside server stays in the logs.
pub(crate) type Refusal = &'static str;

/// Resolves the connector of the org with its password and the address it may reach.
pub(crate) async fn target<C: ConnectionTrait>(
    db: &C,
    ring: &Keyring,
    org_id: i64,
    name: Option<&str>,
    id: Option<Uuid>,
) -> Result<Target, Refusal> {
    let by_db = |_e: DbErr| err_codes::CONNECTOR_UNREACHABLE;
    let row = match (name, id) {
        (Some(name), _) => {
            db.query_one_raw(stmt(
                "SELECT host, port, database, username, tls, secret_id FROM connectors \
                 WHERE org_id = $1 AND name = $2",
                vec![org_id.into(), name.into()],
            ))
            .await
        }
        (_, Some(id)) => {
            db.query_one_raw(stmt(
                "SELECT host, port, database, username, tls, secret_id FROM connectors \
                 WHERE org_id = $1 AND id = $2",
                vec![org_id.into(), id.into()],
            ))
            .await
        }
        _ => Ok(None),
    }
    .map_err(by_db)?
    .ok_or(err_codes::CONNECTOR_NOT_FOUND)?;
    let host: String = row.try_get("", "host").map_err(by_db)?;
    let port: i32 = row.try_get("", "port").map_err(by_db)?;
    let tls: bool = row.try_get("", "tls").map_err(by_db)?;
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
    // Without TLS the password crosses the network in the clear: a host the platform
    // admin listed only, or a development server.
    if !tls && !listed && process != pnex_core::egress::EgressPolicy::Open {
        return Err(err_codes::CONNECTOR_HOST_REFUSED);
    }
    let mut options = PgConnectOptions::new()
        .host(&addr.ip().to_string())
        .port(addr.port())
        .database(
            row.try_get::<String>("", "database")
                .map_err(by_db)?
                .as_str(),
        )
        .username(
            row.try_get::<String>("", "username")
                .map_err(by_db)?
                .as_str(),
        )
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
    if let Some(secret) = secret {
        let password = store::reveal(db, ring, Some(org_id), secret)
            .await
            .map_err(|_| err_codes::CONNECTOR_UNREACHABLE)?;
        options = options.password(&password);
    }
    Ok(Target { options })
}

impl Target {
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

/// Tries the connection, without returning any data.
pub async fn test(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
    id: Uuid,
) -> Result<ConnectorTest, ConnectorError> {
    get(db, org_id, id).await?;
    let outcome = async {
        let target = target(db, ring, org_id, None, Some(id)).await?;
        let mut conn = target.connect(Duration::from_secs(10)).await?;
        conn.ping().await.map_err(|e| {
            tracing::info!(error = %e, "connector ping failed");
            err_codes::CONNECTOR_UNREACHABLE
        })?;
        let _ = conn.close().await;
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
