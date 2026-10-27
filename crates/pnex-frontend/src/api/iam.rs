//! Access management API (iam.md §8): `/api/v1/iam`.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::api::client;
use crate::api::error::ApiError;

/// Rights of the caller in the selected org.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MyPermissions {
    /// Managed role of the membership (`viewer`…`owner`).
    pub role: String,
    /// Every permission held, assigned roles included (`service:verb`).
    pub permissions: Vec<String>,
}

/// `GET /api/v1/iam/me/permissions`.
pub async fn my_permissions() -> Result<MyPermissions, ApiError> {
    client::request(reqwest::Method::GET, "/api/v1/iam/me/permissions", None).await
}

/// One statement of a role: `allow` or `deny` permission patterns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Statement {
    pub effect: String,
    pub permissions: Vec<String>,
}

/// A role: managed (`id` null, not editable) or custom.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Role {
    pub id: Option<String>,
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub managed: bool,
    pub statements: Vec<Statement>,
    pub version: Option<i32>,
}

impl Role {
    /// Patterns of the statements with this effect.
    pub fn patterns(&self, effect: &str) -> Vec<String> {
        self.statements
            .iter()
            .filter(|s| s.effect == effect)
            .flat_map(|s| s.permissions.clone())
            .collect()
    }
}

/// `GET /api/v1/iam/roles` — managed roles first, then custom ones.
pub async fn roles() -> Result<Vec<Role>, ApiError> {
    client::request_all("/api/v1/iam/roles").await
}

fn statements(allow: &[String], deny: &[String]) -> Value {
    let mut out = Vec::new();
    if !allow.is_empty() {
        out.push(json!({ "effect": "allow", "permissions": allow }));
    }
    if !deny.is_empty() {
        out.push(json!({ "effect": "deny", "permissions": deny }));
    }
    Value::Array(out)
}

/// `POST /api/v1/iam/roles`.
pub async fn create_role(
    key: &str,
    name: &str,
    description: &str,
    allow: &[String],
    deny: &[String],
) -> Result<Value, ApiError> {
    let body = json!({
        "key": key, "name": name, "description": description,
        "statements": statements(allow, deny),
    });
    client::request(reqwest::Method::POST, "/api/v1/iam/roles", Some(body)).await
}

/// `PATCH /api/v1/iam/roles/{id}` on top of `expected_version`.
pub async fn update_role(
    id: &str,
    name: &str,
    description: &str,
    allow: &[String],
    deny: &[String],
    expected_version: i32,
) -> Result<Value, ApiError> {
    let body = json!({
        "name": name, "description": description,
        "statements": statements(allow, deny),
        "expected_version": expected_version,
    });
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/iam/roles/{id}"),
        Some(body),
    )
    .await
}

async fn delete(path: &str) -> Result<(), ApiError> {
    client::request_opt::<Value>(reqwest::Method::DELETE, path, None)
        .await
        .map(|_| ())
}

/// `DELETE /api/v1/iam/roles/{id}` — its assignments go too.
pub async fn delete_role(id: &str) -> Result<(), ApiError> {
    delete(&format!("/api/v1/iam/roles/{id}")).await
}

/// A team of org members.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Team {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub members: Vec<i64>,
}

/// `GET /api/v1/iam/teams`.
pub async fn teams() -> Result<Vec<Team>, ApiError> {
    client::request_all("/api/v1/iam/teams").await
}

/// `POST /api/v1/iam/teams`.
pub async fn create_team(name: &str, description: &str) -> Result<Team, ApiError> {
    let body = json!({ "name": name, "description": description });
    client::request(reqwest::Method::POST, "/api/v1/iam/teams", Some(body)).await
}

/// `PATCH /api/v1/iam/teams/{id}`.
pub async fn update_team(id: &str, name: &str, description: &str) -> Result<Team, ApiError> {
    let body = json!({ "name": name, "description": description });
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/iam/teams/{id}"),
        Some(body),
    )
    .await
}

/// `DELETE /api/v1/iam/teams/{id}`.
pub async fn delete_team(id: &str) -> Result<(), ApiError> {
    delete(&format!("/api/v1/iam/teams/{id}")).await
}

/// `POST /api/v1/iam/teams/{id}/members`.
pub async fn add_team_member(id: &str, user_id: i64) -> Result<Team, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/iam/teams/{id}/members"),
        Some(json!({ "user_id": user_id })),
    )
    .await
}

/// `DELETE /api/v1/iam/teams/{id}/members/{user_id}`.
pub async fn remove_team_member(id: &str, user_id: i64) -> Result<Team, ApiError> {
    client::request(
        reqwest::Method::DELETE,
        &format!("/api/v1/iam/teams/{id}/members/{user_id}"),
        None,
    )
    .await
}

/// A role given to a member, a team or a service account.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Assignment {
    pub id: String,
    pub user_id: Option<i64>,
    pub service_account_id: Option<String>,
    pub team_id: Option<String>,
    pub role_key: String,
    pub created_at: String,
}

/// `GET /api/v1/iam/assignments`.
pub async fn assignments() -> Result<Vec<Assignment>, ApiError> {
    client::request_all("/api/v1/iam/assignments").await
}

/// Who receives a role.
#[derive(Debug, Clone, PartialEq)]
pub enum Holder {
    User(i64),
    Team(String),
    Service(String),
}

/// `POST /api/v1/iam/assignments`: `role` is a managed key (`viewer`,
/// `member`, `admin`) or a custom role id.
pub async fn create_assignment(holder: &Holder, role: &Role) -> Result<Assignment, ApiError> {
    let mut body = match holder {
        Holder::User(u) => json!({ "user_id": u }),
        Holder::Team(t) => json!({ "team_id": t }),
        Holder::Service(s) => json!({ "service_account_id": s }),
    };
    match &role.id {
        Some(id) => body["role_id"] = json!(id),
        None => body["managed_role"] = json!(role.key),
    }
    client::request(reqwest::Method::POST, "/api/v1/iam/assignments", Some(body)).await
}

/// `DELETE /api/v1/iam/assignments/{id}`.
pub async fn delete_assignment(id: &str) -> Result<(), ApiError> {
    delete(&format!("/api/v1/iam/assignments/{id}")).await
}

/// A service account of the org.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ServiceAccount {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Identity of the account (owner of its mailboxes).
    pub user_id: i64,
    pub active_keys: i64,
    pub created_at: String,
}

/// `GET /api/v1/iam/service-accounts`.
pub async fn service_accounts() -> Result<Vec<ServiceAccount>, ApiError> {
    client::request_all("/api/v1/iam/service-accounts").await
}

/// `POST /api/v1/iam/service-accounts`.
pub async fn create_service_account(
    name: &str,
    description: &str,
) -> Result<ServiceAccount, ApiError> {
    let body = json!({ "name": name, "description": description });
    client::request(
        reqwest::Method::POST,
        "/api/v1/iam/service-accounts",
        Some(body),
    )
    .await
}

/// `PATCH /api/v1/iam/service-accounts/{id}`.
pub async fn update_service_account(
    id: &str,
    name: &str,
    description: &str,
) -> Result<ServiceAccount, ApiError> {
    let body = json!({ "name": name, "description": description });
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/iam/service-accounts/{id}"),
        Some(body),
    )
    .await
}

/// `DELETE /api/v1/iam/service-accounts/{id}` — its keys and roles go too.
pub async fn delete_service_account(id: &str) -> Result<(), ApiError> {
    delete(&format!("/api/v1/iam/service-accounts/{id}")).await
}

/// An access key, without its secret.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AccessKey {
    pub id: String,
    pub prefix: String,
    pub expires_at: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked_at: Option<String>,
}

/// A key just created: the only time its secret is shown.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CreatedKey {
    pub key: AccessKey,
    pub secret: String,
}

/// `GET /api/v1/iam/service-accounts/{id}/keys`.
pub async fn access_keys(account: &str) -> Result<Vec<AccessKey>, ApiError> {
    client::request_all(&format!("/api/v1/iam/service-accounts/{account}/keys")).await
}

/// `POST /api/v1/iam/service-accounts/{id}/keys`, `expires_at` RFC 3339.
pub async fn create_access_key(account: &str, expires_at: &str) -> Result<CreatedKey, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/iam/service-accounts/{account}/keys"),
        Some(json!({ "expires_at": expires_at })),
    )
    .await
}

/// `DELETE /api/v1/iam/service-accounts/{id}/keys/{key_id}` — revocation.
pub async fn revoke_access_key(account: &str, key: &str) -> Result<(), ApiError> {
    delete(&format!(
        "/api/v1/iam/service-accounts/{account}/keys/{key}"
    ))
    .await
}
