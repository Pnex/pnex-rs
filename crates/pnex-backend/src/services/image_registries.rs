//! Image registries (agent-environments.md D703). The test is the only thing this
//! server does with one: `GET /v2/`, the challenge it answers, then the right on the
//! repository. Every connection goes to an address checked by the egress rule of
//! `pnex_core::connectors`, without following any redirect.

use std::time::Duration;

use base64::Engine as _;
use pnex_core::connectors::egress_policy;
use pnex_core::err_codes;
use pnex_core::image_registries::{
    bearer_challenge, ImageRegistry, ImageRegistryCheck, ImageRegistryInput, ImageRegistryTest,
};
use reqwest::header::{AUTHORIZATION, LOCATION, WWW_AUTHENTICATE};
use reqwest::{Client, StatusCode, Url};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, QueryResult, Statement,
    TransactionTrait,
};
use uuid::Uuid;

use crate::services::secrets::image_registry::{self as vault, Field};
use crate::services::secrets::store::{self, StoreError, Writer};
use crate::services::secrets::Keyring;

const TIMEOUT: Duration = Duration::from_secs(20);
const TOKEN_ANSWER_MAX: usize = 64 * 1024;

#[derive(Debug)]
pub enum ImageRegistryError {
    Invalid {
        field: String,
        token: String,
    },
    NotFound,
    NameTaken,
    /// An environment still names it.
    InUse,
    VersionConflict {
        current: i32,
    },
    Store(StoreError),
    Db(DbErr),
}

impl From<DbErr> for ImageRegistryError {
    fn from(e: DbErr) -> Self {
        Self::Db(e)
    }
}

impl From<StoreError> for ImageRegistryError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

/// Who writes: a typed secret needs the right to write secrets.
#[derive(Clone, Copy)]
pub struct Author {
    pub user_id: i64,
    pub can_write_secrets: bool,
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

const SELECT: &str = "SELECT id, name, host, repository, tls, read_username, \
    read_secret_id IS NOT NULL AS has_read_secret, write_username, \
    write_secret_id IS NOT NULL AS has_write_secret, version, \
    to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS created_at, \
    to_char(updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS updated_at \
    FROM image_registries";

fn view(r: &QueryResult) -> Result<ImageRegistry, DbErr> {
    Ok(ImageRegistry {
        id: r.try_get::<Uuid>("", "id")?.to_string(),
        name: r.try_get("", "name")?,
        host: r.try_get("", "host")?,
        repository: r.try_get("", "repository")?,
        tls: r.try_get("", "tls")?,
        read_username: r.try_get("", "read_username")?,
        has_read_secret: r.try_get("", "has_read_secret")?,
        write_username: r.try_get("", "write_username")?,
        has_write_secret: r.try_get("", "has_write_secret")?,
        version: r.try_get("", "version")?,
        created_at: r.try_get("", "created_at")?,
        updated_at: r.try_get("", "updated_at")?,
    })
}

fn invalid(field: &str, token: &str) -> ImageRegistryError {
    ImageRegistryError::Invalid {
        field: field.into(),
        token: token.into(),
    }
}

fn checked(input: &ImageRegistryInput) -> Result<(), ImageRegistryError> {
    match input.violation() {
        Some((field, token)) => Err(invalid(field, &token)),
        None => Ok(()),
    }
}

fn username(u: &Option<String>) -> Option<String> {
    u.as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(str::to_string)
}

/// Both secrets resolved. A user without its secret cannot log in, and one secret in
/// both roles would hand the push right to whoever may only pull (D703).
fn paired(
    input: &ImageRegistryInput,
    read: Option<Uuid>,
    write: Option<Uuid>,
) -> Result<(), ImageRegistryError> {
    if username(&input.read_username).is_some() != read.is_some() {
        return Err(if read.is_some() {
            invalid("read_username", "required")
        } else {
            invalid("read_secret", "required")
        });
    }
    if username(&input.write_username).is_some() != write.is_some() {
        return Err(if write.is_some() {
            invalid("write_username", "required")
        } else {
            invalid("write_secret", "required")
        });
    }
    if read.is_some() && read == write {
        return Err(invalid("write_secret", "invalid"));
    }
    Ok(())
}

pub async fn get(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
) -> Result<ImageRegistry, ImageRegistryError> {
    let sql = format!("{SELECT} WHERE org_id = $1 AND id = $2");
    let row = db
        .query_one_raw(stmt(&sql, vec![org_id.into(), id.into()]))
        .await?
        .ok_or(ImageRegistryError::NotFound)?;
    Ok(view(&row)?)
}

pub async fn list(
    db: &DatabaseConnection,
    org_id: i64,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<ImageRegistry>), ImageRegistryError> {
    let total = db
        .query_one_raw(stmt(
            "SELECT count(*) AS n FROM image_registries WHERE org_id = $1",
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

async fn name_taken<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    name: &str,
    other_than: Uuid,
) -> Result<bool, DbErr> {
    Ok(db
        .query_one_raw(stmt(
            "SELECT 1 AS x FROM image_registries WHERE org_id = $1 AND name = $2 AND id <> $3",
            vec![org_id.into(), name.into(), other_than.into()],
        ))
        .await?
        .is_some())
}

pub async fn create(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
    by: Author,
    input: &ImageRegistryInput,
) -> Result<ImageRegistry, ImageRegistryError> {
    checked(input)?;
    let name = input.name.trim();
    let id = Uuid::new_v4();
    let txn = db.begin().await?;
    if name_taken(&txn, org_id, name, id).await? {
        return Err(ImageRegistryError::NameTaken);
    }
    let w = writer(org_id, by);
    let field = |name, input| Field {
        name,
        current: None,
        input,
    };
    let read = vault::resolve(
        &txn,
        ring,
        w,
        by.can_write_secrets,
        id,
        &field(vault::READ, input.read_secret.as_ref()),
    )
    .await?;
    let write = vault::resolve(
        &txn,
        ring,
        w,
        by.can_write_secrets,
        id,
        &field(vault::WRITE, input.write_secret.as_ref()),
    )
    .await?;
    paired(input, read, write)?;
    vault::commit(&txn, org_id, id, (None, read), (None, write)).await?;
    txn.execute_raw(stmt(
        "INSERT INTO image_registries (id, org_id, name, host, repository, tls, read_username, \
           read_secret_id, write_username, write_secret_id, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        vec![
            id.into(),
            org_id.into(),
            name.into(),
            input.host.trim().into(),
            input.repository.trim().into(),
            input.tls.into(),
            username(&input.read_username).into(),
            read.into(),
            username(&input.write_username).into(),
            write.into(),
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
    input: &ImageRegistryInput,
) -> Result<ImageRegistry, ImageRegistryError> {
    checked(input)?;
    let Some(expected) = input.expected_version else {
        return Err(invalid("expected_version", "required"));
    };
    let name = input.name.trim();
    let txn = db.begin().await?;
    let row = txn
        .query_one_raw(stmt(
            "SELECT host, tls, read_secret_id, write_secret_id, version FROM image_registries \
             WHERE org_id = $1 AND id = $2 FOR UPDATE",
            vec![org_id.into(), id.into()],
        ))
        .await?
        .ok_or(ImageRegistryError::NotFound)?;
    let version: i32 = row.try_get("", "version")?;
    if version != expected {
        return Err(ImageRegistryError::VersionConflict { current: version });
    }
    if name_taken(&txn, org_id, name, id).await? {
        return Err(ImageRegistryError::NameTaken);
    }
    let held_read: Option<Uuid> = row.try_get("", "read_secret_id")?;
    let held_write: Option<Uuid> = row.try_get("", "write_secret_id")?;
    // A stored secret must not reach a host, or cross the network in a way, its author
    // never meant it for.
    let moved = row.try_get::<String>("", "host")? != input.host.trim()
        || row.try_get::<bool>("", "tls")? != input.tls;
    if moved && held_read.is_some() && input.read_secret.is_none() {
        return Err(invalid("read_secret", "required"));
    }
    if moved && held_write.is_some() && input.write_secret.is_none() {
        return Err(invalid("write_secret", "required"));
    }
    let w = writer(org_id, by);
    let read = vault::resolve(
        &txn,
        ring,
        w,
        by.can_write_secrets,
        id,
        &Field {
            name: vault::READ,
            current: held_read,
            input: input.read_secret.as_ref(),
        },
    )
    .await?;
    let write = vault::resolve(
        &txn,
        ring,
        w,
        by.can_write_secrets,
        id,
        &Field {
            name: vault::WRITE,
            current: held_write,
            input: input.write_secret.as_ref(),
        },
    )
    .await?;
    paired(input, read, write)?;
    // The row first: a dedicated secret is pruned only once nothing points at it.
    txn.execute_raw(stmt(
        "UPDATE image_registries SET name = $3, host = $4, repository = $5, tls = $6, \
           read_username = $7, read_secret_id = $8, write_username = $9, write_secret_id = $10, \
           version = version + 1, updated_at = now() WHERE org_id = $1 AND id = $2",
        vec![
            org_id.into(),
            id.into(),
            name.into(),
            input.host.trim().into(),
            input.repository.trim().into(),
            input.tls.into(),
            username(&input.read_username).into(),
            read.into(),
            username(&input.write_username).into(),
            write.into(),
        ],
    ))
    .await?;
    vault::commit(&txn, org_id, id, (held_read, read), (held_write, write)).await?;
    txn.commit().await?;
    get(db, org_id, id).await
}

pub async fn delete(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
) -> Result<(), ImageRegistryError> {
    get(db, org_id, id).await?;
    let txn = db.begin().await?;
    let used = txn
        .query_one_raw(stmt(
            "SELECT 1 AS x FROM environments WHERE org_id = $1 AND registry_id = $2 LIMIT 1",
            vec![org_id.into(), id.into()],
        ))
        .await?
        .is_some();
    if used {
        return Err(ImageRegistryError::InUse);
    }
    txn.execute_raw(stmt(
        "DELETE FROM image_registries WHERE org_id = $1 AND id = $2",
        vec![org_id.into(), id.into()],
    ))
    .await?;
    vault::release(&txn, org_id, id).await?;
    txn.commit().await?;
    Ok(())
}

/// Why a check failed: codes only, the text of an outside server stays in the logs.
type Refusal = &'static str;

/// A client bound to the one address of `host` that the egress rule of the org allows,
/// so that the name resolving elsewhere later changes nothing. TLS still checks the name.
async fn client_for<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    host: &str,
    port: u16,
    tls: bool,
) -> Result<Client, Refusal> {
    let listed = db
        .query_one_raw(stmt(
            "SELECT 1 AS x FROM connector_allowed_hosts WHERE org_id = $1 AND host = $2",
            vec![org_id.into(), host.to_lowercase().into()],
        ))
        .await
        .map_err(|_| err_codes::IMAGE_REGISTRY_UNREACHABLE)?
        .is_some();
    let process = pnex_core::egress::policy();
    let policy = egress_policy(listed, process);
    if pnex_core::egress::check_host(host, policy).is_some() {
        return Err(err_codes::IMAGE_REGISTRY_HOST_REFUSED);
    }
    // Without TLS the secret crosses the network in the clear: a host the platform
    // admin listed only, or a development server.
    if !tls && !listed && process != pnex_core::egress::EgressPolicy::Open {
        return Err(err_codes::IMAGE_REGISTRY_HOST_REFUSED);
    }
    let addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| err_codes::IMAGE_REGISTRY_UNREACHABLE)?;
    let Some(addr) = pnex_core::egress::filter_addrs(addrs, policy)
        .into_iter()
        .next()
    else {
        return Err(err_codes::IMAGE_REGISTRY_HOST_REFUSED);
    };
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(TIMEOUT)
        .resolve(host, addr)
        .build()
        .map_err(|e| {
            tracing::warn!(error = %e, "image registry client not built");
            err_codes::IMAGE_REGISTRY_UNREACHABLE
        })
}

fn unreachable(e: reqwest::Error) -> Refusal {
    // Without the URL: it may hold a realm chosen by the outside server.
    tracing::info!(error = %e.without_url(), "image registry not reached");
    err_codes::IMAGE_REGISTRY_UNREACHABLE
}

/// How to log in: `(user, secret)`.
type Credentials = Option<(String, String)>;

fn basic((user, secret): &(String, String)) -> String {
    let raw = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{secret}"));
    format!("Basic {raw}")
}

/// The token of a bearer challenge. A realm is a URL the outside server chose: it passes
/// the same egress rule, and a registry behind TLS never sends the login to clear text.
async fn bearer_token<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    tls: bool,
    (realm, service): (String, Option<String>),
    scope: &str,
    credentials: &Credentials,
) -> Result<String, Refusal> {
    let mut url = Url::parse(&realm).map_err(|_| err_codes::IMAGE_REGISTRY_UNREACHABLE)?;
    let secure = url.scheme() == "https";
    if !secure && (tls || url.scheme() != "http") {
        return Err(err_codes::IMAGE_REGISTRY_HOST_REFUSED);
    }
    let host = url
        .host_str()
        .ok_or(err_codes::IMAGE_REGISTRY_UNREACHABLE)?
        .to_string();
    let port = url
        .port_or_known_default()
        .ok_or(err_codes::IMAGE_REGISTRY_UNREACHABLE)?;
    {
        let mut query = url.query_pairs_mut();
        if let Some(service) = service {
            query.append_pair("service", &service);
        }
        query.append_pair("scope", scope);
    }
    let client = client_for(db, org_id, &host, port, secure).await?;
    let mut request = client.get(url);
    if let Some(c) = credentials {
        request = request.header(AUTHORIZATION, basic(c));
    }
    let mut response = request.send().await.map_err(unreachable)?;
    match response.status() {
        s if s.is_success() => {}
        // Not only 401: Scaleway answers 404 for an unknown key.
        s if s.is_client_error() => return Err(err_codes::IMAGE_REGISTRY_AUTH_FAILED),
        _ => return Err(err_codes::IMAGE_REGISTRY_UNREACHABLE),
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(unreachable)? {
        body.extend_from_slice(&chunk);
        if body.len() > TOKEN_ANSWER_MAX {
            return Err(err_codes::IMAGE_REGISTRY_UNREACHABLE);
        }
    }
    let answer: serde_json::Value =
        serde_json::from_slice(&body).map_err(|_| err_codes::IMAGE_REGISTRY_UNREACHABLE)?;
    answer["token"]
        .as_str()
        .or_else(|| answer["access_token"].as_str())
        .map(str::to_string)
        .ok_or(err_codes::IMAGE_REGISTRY_UNREACHABLE)
}

struct Probe<'a> {
    org_id: i64,
    /// `name` and `port` apart, as the host of the registry is written `name[:port]`.
    name: &'a str,
    port: u16,
    tls: bool,
    repository: &'a str,
}

impl Probe<'_> {
    /// Logs in as the registry asks, and only then: nothing is sent before its challenge.
    async fn check<C: ConnectionTrait>(
        &self,
        db: &C,
        credentials: &Credentials,
        push: bool,
    ) -> Result<(), Refusal> {
        let client = client_for(db, self.org_id, self.name, self.port, self.tls).await?;
        let scheme = if self.tls { "https" } else { "http" };
        let base = format!("{scheme}://{}:{}/v2/", self.name, self.port);
        let hello = client.get(&base).send().await.map_err(unreachable)?;
        let authorization = match hello.status() {
            s if s.is_success() => None,
            StatusCode::UNAUTHORIZED => {
                let challenge = hello
                    .headers()
                    .get(WWW_AUTHENTICATE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default();
                if let Some(challenge) = bearer_challenge(challenge) {
                    let actions = if push { "pull,push" } else { "pull" };
                    let scope = format!("repository:{}:{actions}", self.repository);
                    let token =
                        bearer_token(db, self.org_id, self.tls, challenge, &scope, credentials)
                            .await?;
                    Some(format!("Bearer {token}"))
                } else if let Some(c) = credentials {
                    Some(basic(c))
                } else {
                    return Err(err_codes::IMAGE_REGISTRY_AUTH_FAILED);
                }
            }
            _ => return Err(err_codes::IMAGE_REGISTRY_UNREACHABLE),
        };
        let authorized = |request: reqwest::RequestBuilder| match &authorization {
            Some(a) => request.header(AUTHORIZATION, a),
            None => request,
        };
        // Basic login is only proved here: `/v2/` answered before it was sent.
        let denied = |status| match status {
            StatusCode::UNAUTHORIZED if matches!(&authorization, Some(a) if a.starts_with("Basic ")) => {
                err_codes::IMAGE_REGISTRY_AUTH_FAILED
            }
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                err_codes::IMAGE_REGISTRY_ACCESS_DENIED
            }
            _ => err_codes::IMAGE_REGISTRY_UNREACHABLE,
        };
        if !push {
            let answer = authorized(client.get(format!("{base}{}/tags/list?n=1", self.repository)))
                .send()
                .await
                .map_err(unreachable)?;
            // 404: allowed, and the repository does not exist until the first push.
            let status = answer.status();
            return if status.is_success() || status == StatusCode::NOT_FOUND {
                Ok(())
            } else {
                Err(denied(status))
            };
        }
        // Opening an upload proves the push right without writing anything.
        let uploads = format!("{base}{}/blobs/uploads/", self.repository);
        let opened = authorized(client.post(&uploads).header("content-length", "0"))
            .send()
            .await
            .map_err(unreachable)?;
        if opened.status() != StatusCode::ACCEPTED {
            return Err(denied(opened.status()));
        }
        // Cancelled when it stays on this registry; otherwise left to its expiry, since
        // the login must not follow a location the server chose.
        let location = opened
            .headers()
            .get(LOCATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|l| Url::parse(&uploads).ok()?.join(l).ok())
            .filter(|l| {
                l.host_str() == Some(self.name) && l.port_or_known_default() == Some(self.port)
            });
        if let Some(location) = location {
            let _ = authorized(client.delete(location)).send().await;
        }
        Ok(())
    }
}

fn outcome(result: Result<(), Refusal>) -> ImageRegistryCheck {
    match result {
        Ok(()) => ImageRegistryCheck {
            ok: true,
            code: None,
        },
        Err(code) => ImageRegistryCheck {
            ok: false,
            code: Some(code.to_string()),
        },
    }
}

async fn credentials<C: ConnectionTrait>(
    db: &C,
    ring: &Keyring,
    org_id: i64,
    user: Option<String>,
    secret: Option<Uuid>,
) -> Result<Credentials, Refusal> {
    let (Some(user), Some(secret)) = (user, secret) else {
        return Ok(None);
    };
    let value = store::reveal(db, ring, Some(org_id), secret)
        .await
        .map_err(|_| err_codes::IMAGE_REGISTRY_AUTH_FAILED)?;
    Ok(Some((user, value)))
}

/// Tries the pull right, and the push right when a write secret is set. No image moves,
/// and no text of the outside server comes back.
pub async fn test(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
    id: Uuid,
) -> Result<ImageRegistryTest, ImageRegistryError> {
    let row = db
        .query_one_raw(stmt(
            "SELECT host, repository, tls, read_username, read_secret_id, write_username, \
               write_secret_id FROM image_registries WHERE org_id = $1 AND id = $2",
            vec![org_id.into(), id.into()],
        ))
        .await?
        .ok_or(ImageRegistryError::NotFound)?;
    let host: String = row.try_get("", "host")?;
    let repository: String = row.try_get("", "repository")?;
    let tls: bool = row.try_get("", "tls")?;
    let (read_user, read_secret): (Option<String>, Option<Uuid>) = (
        row.try_get("", "read_username")?,
        row.try_get("", "read_secret_id")?,
    );
    let (write_user, write_secret): (Option<String>, Option<Uuid>) = (
        row.try_get("", "write_username")?,
        row.try_get("", "write_secret_id")?,
    );
    let default_port = if tls { 443 } else { 80 };
    let (name, port) = match host.rsplit_once(':') {
        Some((name, port)) => (name, port.parse().unwrap_or(default_port)),
        None => (host.as_str(), default_port),
    };
    let probe = Probe {
        org_id,
        name,
        port,
        tls,
        repository: &repository,
    };
    let read = match credentials(db, ring, org_id, read_user, read_secret).await {
        Ok(c) => probe.check(db, &c, false).await,
        Err(code) => Err(code),
    };
    let write = match write_secret {
        None => None,
        Some(_) => Some(
            match credentials(db, ring, org_id, write_user, write_secret).await {
                Ok(c) => probe.check(db, &c, true).await,
                Err(code) => Err(code),
            },
        ),
    };
    Ok(ImageRegistryTest {
        read: outcome(read),
        write: write.map(outcome),
    })
}
