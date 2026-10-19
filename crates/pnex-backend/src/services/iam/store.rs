//! Custom roles, teams and assignments of an org (iam.md D286–D288, D293,
//! lot I3). Shared by the HTTP controller (and so the `pnex` CLI); never
//! reachable from the MCP nor the assistant (iam.md §14).
//!
//! Assignments cover the whole org in I3: the scopes of D288 (types,
//! objects, labels) come with the filtered reads of lot I4.

use std::borrow::Cow;

use chrono::{DateTime, FixedOffset};
use pnex_api_contract::iam::{permission_level, PERMISSIONS};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, FromQueryResult, Statement as Sql,
};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use super::{managed_roles, matches, Effect, Grants, Role, Statement, IAM_SERVICE};
use crate::models::_entities::sea_orm_active_enums::OrgMemberRole;

pub const NAME_MAX: usize = 120;
pub const DESCRIPTION_MAX: usize = 2000;
pub const KEY_MAX: usize = 64;
pub const STATEMENTS_MAX: usize = 50;
pub const PATTERNS_MAX: usize = 200;

#[derive(Debug)]
pub enum IamError {
    /// Field error: `field` + machine token (`required`, `max_length:N`…).
    Invalid {
        field: String,
        token: String,
    },
    RoleNotFound,
    TeamNotFound,
    AssignmentNotFound,
    RoleKeyTaken,
    TeamNameTaken,
    AssignmentDuplicate,
    VersionConflict {
        current: i32,
    },
    ServiceAccountNotFound,
    ServiceAccountNameTaken,
    KeyNotFound,
    /// Two keys are active already: revoke one first.
    KeyLimit,
    /// The caller does not hold `permission`, which the role would grant.
    Escalation {
        permission: String,
    },
    Db(DbErr),
}

impl From<DbErr> for IamError {
    fn from(e: DbErr) -> Self {
        Self::Db(e)
    }
}

pub type Result<T> = std::result::Result<T, IamError>;

fn invalid(field: &str, token: impl Into<String>) -> IamError {
    IamError::Invalid {
        field: field.to_string(),
        token: token.into(),
    }
}

fn sql(text: &str, values: Vec<sea_orm::Value>) -> Sql {
    Sql::from_sql_and_values(DatabaseBackend::Postgres, text, values)
}

fn unique_violation(e: &DbErr) -> bool {
    e.to_string().contains("duplicate key")
}

// ── Grants of a member ──────────────────────────────────────────────

#[derive(FromQueryResult)]
struct GrantRow {
    managed_role: Option<String>,
    key: Option<String>,
    statements: Option<Value>,
}

/// Managed role of the membership (`None` for a service account, which
/// holds nothing but its assignments), then every role assigned to the
/// principal directly or through one of their teams in this org.
// shortcut: one query per org request, no cache; add the compiled cache of
// D292 (Valkey invalidation) when this shows up in latency.
pub async fn grants(
    db: &impl ConnectionTrait,
    org_id: i64,
    user_id: i64,
    role: Option<&OrgMemberRole>,
) -> std::result::Result<Grants, DbErr> {
    let rows = GrantRow::find_by_statement(sql(
        "SELECT a.managed_role, r.key, r.statements FROM iam_assignments a \
         LEFT JOIN iam_roles r ON r.id = a.role_id \
         WHERE a.org_id = $1 AND (a.user_id = $2 OR a.team_id IN \
           (SELECT tm.team_id FROM iam_team_members tm \
            JOIN iam_teams t ON t.id = tm.team_id \
            WHERE tm.user_id = $2 AND t.org_id = $1))",
        vec![org_id.into(), user_id.into()],
    ))
    .all(db)
    .await?;
    let mut grants = role.map_or(Grants(Vec::new()), Grants::managed);
    for row in rows {
        if let Some(managed) = row.managed_role.as_deref().and_then(managed_by_key) {
            grants.0.push(Cow::Borrowed(managed));
        } else if let (Some(key), Some(statements)) = (row.key, row.statements) {
            grants.0.push(Cow::Owned(Role {
                key,
                statements: serde_json::from_value(statements).unwrap_or_default(),
                managed: false,
            }));
        }
    }
    Ok(grants)
}

fn managed_by_key(key: &str) -> Option<&'static Role> {
    managed_roles().iter().find(|r| r.key == key)
}

// ── Validation ──────────────────────────────────────────────────────

fn is_service(service: &str) -> bool {
    PERMISSIONS
        .iter()
        .any(|(p, _)| p.split_once(':').is_some_and(|(s, _)| s == service))
}

/// A pattern names a known permission, a known service (`svc:*`) or `*`;
/// never the access service (D293).
fn check_pattern(pattern: &str) -> std::result::Result<(), String> {
    let service = pattern.split_once(':').map(|(s, _)| s);
    if service == Some(IAM_SERVICE) {
        return Err(format!("iam_permission:{pattern}"));
    }
    let known = pattern == "*"
        || permission_level(pattern).is_some()
        || pattern.strip_suffix(":*").is_some_and(is_service);
    if known {
        Ok(())
    } else {
        Err(format!("unknown_permission:{pattern}"))
    }
}

fn check_statements(statements: &[Statement]) -> Result<()> {
    if statements.len() > STATEMENTS_MAX {
        return Err(invalid("statements", format!("max_items:{STATEMENTS_MAX}")));
    }
    for (i, s) in statements.iter().enumerate() {
        let field = format!("statements.{i}.permissions");
        if s.permissions.is_empty() {
            return Err(invalid(&field, "required"));
        }
        if s.permissions.len() > PATTERNS_MAX {
            return Err(invalid(&field, format!("max_items:{PATTERNS_MAX}")));
        }
        for p in &s.permissions {
            check_pattern(p).map_err(|token| invalid(&field, token))?;
        }
    }
    Ok(())
}

fn check_text(field: &str, value: &str, max: usize, required: bool) -> Result<String> {
    let v = value.trim();
    if required && v.is_empty() {
        return Err(invalid(field, "required"));
    }
    if v.chars().count() > max {
        return Err(invalid(field, format!("max_length:{max}")));
    }
    Ok(v.to_string())
}

fn check_key(key: &str) -> Result<String> {
    let k = check_text("key", key, KEY_MAX, true)?;
    let slug = k
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !k.starts_with('-');
    if !slug || managed_by_key(&k).is_some() {
        return Err(invalid("key", "invalid"));
    }
    Ok(k)
}

/// Anti-escalation (D293): the caller must hold every permission a role
/// would allow (the access service aside, never granted by a custom role).
pub fn check_escalation(caller: &Grants, role: &Role) -> Result<()> {
    for (p, _) in PERMISSIONS {
        if p.starts_with("iam:") {
            continue;
        }
        let grants = role
            .statements
            .iter()
            .any(|s| s.effect == Effect::Allow && s.permissions.iter().any(|pat| matches(pat, p)));
        if grants && !caller.can(p) {
            return Err(IamError::Escalation {
                permission: p.to_string(),
            });
        }
    }
    Ok(())
}

// ── Roles ───────────────────────────────────────────────────────────

/// A custom role of the org.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct CustomRole {
    #[schema(value_type = String, format = "uuid")]
    pub id: Uuid,
    /// Slug, unique in the org (`a-z`, `0-9`, `-`).
    pub key: String,
    pub name: String,
    pub description: String,
    pub statements: Vec<Statement>,
    /// Optimistic concurrency: send it back as `expected_version`.
    pub version: i32,
    #[schema(value_type = String, format = DateTime)]
    pub created_at: DateTime<FixedOffset>,
    #[schema(value_type = String, format = DateTime)]
    pub updated_at: DateTime<FixedOffset>,
}

#[derive(FromQueryResult)]
struct RoleRow {
    id: Uuid,
    key: String,
    name: String,
    description: String,
    statements: Value,
    version: i32,
    created_at: DateTime<FixedOffset>,
    updated_at: DateTime<FixedOffset>,
}

impl From<RoleRow> for CustomRole {
    fn from(r: RoleRow) -> Self {
        Self {
            id: r.id,
            key: r.key,
            name: r.name,
            description: r.description,
            statements: serde_json::from_value(r.statements).unwrap_or_default(),
            version: r.version,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

impl CustomRole {
    pub fn as_role(&self) -> Role {
        Role {
            key: self.key.clone(),
            statements: self.statements.clone(),
            managed: false,
        }
    }
}

const ROLE_COLUMNS: &str =
    "id, key, name, description, statements, version, created_at, updated_at";

pub async fn list_roles(db: &DatabaseConnection, org_id: i64) -> Result<Vec<CustomRole>> {
    Ok(RoleRow::find_by_statement(sql(
        &format!("SELECT {ROLE_COLUMNS} FROM iam_roles WHERE org_id = $1 ORDER BY key"),
        vec![org_id.into()],
    ))
    .all(db)
    .await?
    .into_iter()
    .map(Into::into)
    .collect())
}

pub async fn get_role(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<CustomRole> {
    RoleRow::find_by_statement(sql(
        &format!("SELECT {ROLE_COLUMNS} FROM iam_roles WHERE org_id = $1 AND id = $2"),
        vec![org_id.into(), id.into()],
    ))
    .one(db)
    .await?
    .map(Into::into)
    .ok_or(IamError::RoleNotFound)
}

pub struct RoleInput<'a> {
    pub key: &'a str,
    pub name: &'a str,
    pub description: Option<&'a str>,
    pub statements: &'a [Statement],
}

pub async fn create_role(
    db: &DatabaseConnection,
    org_id: i64,
    caller: &Grants,
    input: RoleInput<'_>,
) -> Result<CustomRole> {
    let key = check_key(input.key)?;
    let name = check_text("name", input.name, NAME_MAX, true)?;
    let description = check_text(
        "description",
        input.description.unwrap_or(""),
        DESCRIPTION_MAX,
        false,
    )?;
    check_statements(input.statements)?;
    let role = Role {
        key: key.clone(),
        statements: input.statements.to_vec(),
        managed: false,
    };
    check_escalation(caller, &role)?;
    let statements = serde_json::to_value(input.statements).unwrap_or_default();
    RoleRow::find_by_statement(sql(
        &format!(
            "INSERT INTO iam_roles (id, org_id, key, name, description, statements) \
             VALUES ($1, $2, $3, $4, $5, $6) RETURNING {ROLE_COLUMNS}"
        ),
        vec![
            Uuid::new_v4().into(),
            org_id.into(),
            key.into(),
            name.into(),
            description.into(),
            statements.into(),
        ],
    ))
    .one(db)
    .await
    .map_err(|e| {
        if unique_violation(&e) {
            IamError::RoleKeyTaken
        } else {
            e.into()
        }
    })?
    .map(Into::into)
    .ok_or(IamError::RoleNotFound)
}

pub struct RolePatch<'a> {
    pub name: Option<&'a str>,
    pub description: Option<&'a str>,
    pub statements: Option<&'a [Statement]>,
    pub expected_version: i32,
}

pub async fn update_role(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    caller: &Grants,
    patch: RolePatch<'_>,
) -> Result<CustomRole> {
    let current = get_role(db, org_id, id).await?;
    if current.version != patch.expected_version {
        return Err(IamError::VersionConflict {
            current: current.version,
        });
    }
    let name = match patch.name {
        Some(n) => check_text("name", n, NAME_MAX, true)?,
        None => current.name.clone(),
    };
    let description = match patch.description {
        Some(d) => check_text("description", d, DESCRIPTION_MAX, false)?,
        None => current.description.clone(),
    };
    let statements = match patch.statements {
        Some(s) => {
            check_statements(s)?;
            s.to_vec()
        }
        None => current.statements.clone(),
    };
    check_escalation(
        caller,
        &Role {
            key: current.key.clone(),
            statements: statements.clone(),
            managed: false,
        },
    )?;
    let statements = serde_json::to_value(&statements).unwrap_or_default();
    RoleRow::find_by_statement(sql(
        &format!(
            "UPDATE iam_roles SET name = $3, description = $4, statements = $5, \
             version = version + 1, updated_at = now() \
             WHERE org_id = $1 AND id = $2 AND version = $6 RETURNING {ROLE_COLUMNS}"
        ),
        vec![
            org_id.into(),
            id.into(),
            name.into(),
            description.into(),
            statements.into(),
            patch.expected_version.into(),
        ],
    ))
    .one(db)
    .await?
    .map(Into::into)
    // Lost the race between the read and the write.
    .ok_or(IamError::VersionConflict {
        current: current.version + 1,
    })
}

/// Deletes a custom role and its assignments.
pub async fn delete_role(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<()> {
    let res = db
        .execute_raw(sql(
            "DELETE FROM iam_roles WHERE org_id = $1 AND id = $2",
            vec![org_id.into(), id.into()],
        ))
        .await?;
    if res.rows_affected() == 0 {
        return Err(IamError::RoleNotFound);
    }
    Ok(())
}

// ── Teams ───────────────────────────────────────────────────────────

/// A team of org members.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct Team {
    #[schema(value_type = String, format = "uuid")]
    pub id: Uuid,
    pub name: String,
    pub description: String,
    /// User ids of the members, ascending.
    pub members: Vec<i64>,
    #[schema(value_type = String, format = DateTime)]
    pub created_at: DateTime<FixedOffset>,
}

#[derive(FromQueryResult)]
struct TeamRow {
    id: Uuid,
    name: String,
    description: String,
    members: Value,
    created_at: DateTime<FixedOffset>,
}

impl From<TeamRow> for Team {
    fn from(t: TeamRow) -> Self {
        Self {
            id: t.id,
            name: t.name,
            description: t.description,
            members: serde_json::from_value(t.members).unwrap_or_default(),
            created_at: t.created_at,
        }
    }
}

const TEAM_SELECT: &str = "SELECT t.id, t.name, t.description, t.created_at, \
     coalesce(jsonb_agg(tm.user_id ORDER BY tm.user_id) FILTER (WHERE tm.user_id IS NOT NULL), '[]'::jsonb) AS members \
     FROM iam_teams t LEFT JOIN iam_team_members tm ON tm.team_id = t.id";

pub async fn list_teams(db: &DatabaseConnection, org_id: i64) -> Result<Vec<Team>> {
    Ok(TeamRow::find_by_statement(sql(
        &format!("{TEAM_SELECT} WHERE t.org_id = $1 GROUP BY t.id ORDER BY t.name"),
        vec![org_id.into()],
    ))
    .all(db)
    .await?
    .into_iter()
    .map(Into::into)
    .collect())
}

pub async fn get_team(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<Team> {
    TeamRow::find_by_statement(sql(
        &format!("{TEAM_SELECT} WHERE t.org_id = $1 AND t.id = $2 GROUP BY t.id"),
        vec![org_id.into(), id.into()],
    ))
    .one(db)
    .await?
    .map(Into::into)
    .ok_or(IamError::TeamNotFound)
}

fn team_write_error(e: DbErr) -> IamError {
    if unique_violation(&e) {
        IamError::TeamNameTaken
    } else {
        e.into()
    }
}

pub async fn create_team(
    db: &DatabaseConnection,
    org_id: i64,
    name: &str,
    description: Option<&str>,
) -> Result<Team> {
    let name = check_text("name", name, NAME_MAX, true)?;
    let description = check_text(
        "description",
        description.unwrap_or(""),
        DESCRIPTION_MAX,
        false,
    )?;
    let id = Uuid::new_v4();
    db.execute_raw(sql(
        "INSERT INTO iam_teams (id, org_id, name, description) VALUES ($1, $2, $3, $4)",
        vec![id.into(), org_id.into(), name.into(), description.into()],
    ))
    .await
    .map_err(team_write_error)?;
    get_team(db, org_id, id).await
}

pub async fn update_team(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    name: Option<&str>,
    description: Option<&str>,
) -> Result<Team> {
    let current = get_team(db, org_id, id).await?;
    let name = match name {
        Some(n) => check_text("name", n, NAME_MAX, true)?,
        None => current.name,
    };
    let description = match description {
        Some(d) => check_text("description", d, DESCRIPTION_MAX, false)?,
        None => current.description,
    };
    db.execute_raw(sql(
        "UPDATE iam_teams SET name = $3, description = $4 WHERE org_id = $1 AND id = $2",
        vec![org_id.into(), id.into(), name.into(), description.into()],
    ))
    .await
    .map_err(team_write_error)?;
    get_team(db, org_id, id).await
}

/// Deletes a team, its memberships and its assignments.
pub async fn delete_team(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<()> {
    let res = db
        .execute_raw(sql(
            "DELETE FROM iam_teams WHERE org_id = $1 AND id = $2",
            vec![org_id.into(), id.into()],
        ))
        .await?;
    if res.rows_affected() == 0 {
        return Err(IamError::TeamNotFound);
    }
    Ok(())
}

async fn is_member(db: &DatabaseConnection, org_id: i64, user_id: i64) -> Result<bool> {
    Ok(db
        .query_one_raw(sql(
            "SELECT 1 AS one FROM organization_members WHERE org_id = $1 AND user_id = $2",
            vec![org_id.into(), user_id.into()],
        ))
        .await?
        .is_some())
}

pub async fn add_team_member(
    db: &DatabaseConnection,
    org_id: i64,
    team_id: Uuid,
    user_id: i64,
) -> Result<Team> {
    get_team(db, org_id, team_id).await?;
    if !is_member(db, org_id, user_id).await? {
        return Err(invalid("user_id", "not_a_member"));
    }
    db.execute_raw(sql(
        "INSERT INTO iam_team_members (team_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        vec![team_id.into(), user_id.into()],
    ))
    .await?;
    get_team(db, org_id, team_id).await
}

pub async fn remove_team_member(
    db: &DatabaseConnection,
    org_id: i64,
    team_id: Uuid,
    user_id: i64,
) -> Result<Team> {
    get_team(db, org_id, team_id).await?;
    db.execute_raw(sql(
        "DELETE FROM iam_team_members WHERE team_id = $1 AND user_id = $2",
        vec![team_id.into(), user_id.into()],
    ))
    .await?;
    get_team(db, org_id, team_id).await
}

/// A member left (or was removed from) the org: their teams and direct
/// assignments there go with them, so a later invitation starts clean.
pub async fn forget_member(
    db: &impl ConnectionTrait,
    org_id: i64,
    user_id: i64,
) -> std::result::Result<(), DbErr> {
    db.execute_raw(sql(
        "DELETE FROM iam_assignments WHERE org_id = $1 AND user_id = $2",
        vec![org_id.into(), user_id.into()],
    ))
    .await?;
    db.execute_raw(sql(
        "DELETE FROM iam_team_members WHERE user_id = $2 AND team_id IN \
         (SELECT id FROM iam_teams WHERE org_id = $1)",
        vec![org_id.into(), user_id.into()],
    ))
    .await?;
    Ok(())
}

// ── Assignments ─────────────────────────────────────────────────────

/// A role given to a member or a team, over the whole org (I3).
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct Assignment {
    #[schema(value_type = String, format = "uuid")]
    pub id: Uuid,
    /// Identity that holds it: a member or a service account (exclusive
    /// with `team_id`).
    pub user_id: Option<i64>,
    /// Service account that holds it (its identity is `user_id`).
    #[schema(value_type = Option<String>, format = "uuid")]
    pub service_account_id: Option<Uuid>,
    /// Team that holds it (exclusive with `user_id`).
    #[schema(value_type = Option<String>, format = "uuid")]
    pub team_id: Option<Uuid>,
    /// Managed role given (`viewer`, `member`, `admin`; exclusive with `role_id`).
    pub managed_role: Option<String>,
    /// Custom role given (exclusive with `managed_role`).
    #[schema(value_type = Option<String>, format = "uuid")]
    pub role_id: Option<Uuid>,
    /// Key of the role given, managed or custom.
    pub role_key: String,
    /// Who made the assignment (`null` once that account is gone).
    pub created_by: Option<i64>,
    #[schema(value_type = String, format = DateTime)]
    pub created_at: DateTime<FixedOffset>,
}

#[derive(FromQueryResult)]
struct AssignmentRow {
    id: Uuid,
    user_id: Option<i64>,
    service_account_id: Option<Uuid>,
    team_id: Option<Uuid>,
    managed_role: Option<String>,
    role_id: Option<Uuid>,
    role_key: Option<String>,
    created_by: Option<i64>,
    created_at: DateTime<FixedOffset>,
}

impl From<AssignmentRow> for Assignment {
    fn from(a: AssignmentRow) -> Self {
        Self {
            id: a.id,
            user_id: a.user_id,
            service_account_id: a.service_account_id,
            team_id: a.team_id,
            role_key: a
                .role_key
                .or_else(|| a.managed_role.clone())
                .unwrap_or_default(),
            managed_role: a.managed_role,
            role_id: a.role_id,
            created_by: a.created_by,
            created_at: a.created_at,
        }
    }
}

const ASSIGNMENT_SELECT: &str = "SELECT a.id, a.user_id, sa.id AS service_account_id, a.team_id, \
     a.managed_role, a.role_id, r.key AS role_key, a.created_by, a.created_at \
     FROM iam_assignments a LEFT JOIN iam_roles r ON r.id = a.role_id \
     LEFT JOIN iam_service_accounts sa ON sa.user_id = a.user_id";

/// Filters of the assignment list (all optional, combined with AND).
#[derive(Default)]
pub struct AssignmentFilter {
    pub user_id: Option<i64>,
    pub team_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
}

pub async fn list_assignments(
    db: &DatabaseConnection,
    org_id: i64,
    f: &AssignmentFilter,
) -> Result<Vec<Assignment>> {
    Ok(AssignmentRow::find_by_statement(sql(
        &format!(
            "{ASSIGNMENT_SELECT} WHERE a.org_id = $1 \
             AND ($2::bigint IS NULL OR a.user_id = $2) \
             AND ($3::uuid IS NULL OR a.team_id = $3) \
             AND ($4::uuid IS NULL OR a.role_id = $4) \
             ORDER BY a.created_at, a.id"
        ),
        vec![
            org_id.into(),
            f.user_id.into(),
            f.team_id.into(),
            f.role_id.into(),
        ],
    ))
    .all(db)
    .await?
    .into_iter()
    .map(Into::into)
    .collect())
}

/// Who receives the role.
pub enum Holder {
    User(i64),
    Team(Uuid),
    /// Identity (`users.id`) of a service account of the org.
    Service(i64),
}

/// Which role is given.
pub enum Given {
    Managed(OrgMemberRole),
    Custom(Uuid),
}

pub async fn create_assignment(
    db: &DatabaseConnection,
    org_id: i64,
    caller_id: i64,
    caller: &Grants,
    holder: Holder,
    given: Given,
) -> Result<Assignment> {
    match holder {
        Holder::User(u) if !is_member(db, org_id, u).await? => {
            return Err(invalid("user_id", "not_a_member"));
        }
        Holder::Team(t) => {
            get_team(db, org_id, t).await?;
        }
        Holder::User(_) | Holder::Service(_) => {}
    }
    let (managed, role_id, role) = match given {
        Given::Managed(OrgMemberRole::Owner) => {
            // Owner stays a membership role (last-owner protection).
            return Err(invalid("managed_role", "invalid"));
        }
        // D293: never access management to a service account.
        Given::Managed(OrgMemberRole::Admin) if matches!(holder, Holder::Service(_)) => {
            return Err(invalid("managed_role", "not_for_service_account"));
        }
        Given::Managed(m) => {
            let r = super::managed_role(&m).clone();
            (Some(r.key.clone()), None, r)
        }
        Given::Custom(id) => {
            let r = get_role(db, org_id, id).await.map_err(|e| match e {
                IamError::RoleNotFound => invalid("role_id", "invalid"),
                other => other,
            })?;
            (None, Some(id), r.as_role())
        }
    };
    check_escalation(caller, &role)?;
    let (user_id, team_id) = match holder {
        Holder::User(u) | Holder::Service(u) => (Some(u), None),
        Holder::Team(t) => (None, Some(t)),
    };
    let dup = db
        .query_one_raw(sql(
            "SELECT 1 AS one FROM iam_assignments WHERE org_id = $1 \
             AND user_id IS NOT DISTINCT FROM $2 AND team_id IS NOT DISTINCT FROM $3 \
             AND managed_role IS NOT DISTINCT FROM $4 AND role_id IS NOT DISTINCT FROM $5",
            vec![
                org_id.into(),
                user_id.into(),
                team_id.into(),
                managed.clone().into(),
                role_id.into(),
            ],
        ))
        .await?;
    if dup.is_some() {
        return Err(IamError::AssignmentDuplicate);
    }
    let id = Uuid::new_v4();
    db.execute_raw(sql(
        "INSERT INTO iam_assignments (id, org_id, user_id, team_id, managed_role, role_id, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
        vec![
            id.into(),
            org_id.into(),
            user_id.into(),
            team_id.into(),
            managed.into(),
            role_id.into(),
            caller_id.into(),
        ],
    ))
    .await?;
    AssignmentRow::find_by_statement(sql(
        &format!("{ASSIGNMENT_SELECT} WHERE a.org_id = $1 AND a.id = $2"),
        vec![org_id.into(), id.into()],
    ))
    .one(db)
    .await?
    .map(Into::into)
    .ok_or(IamError::AssignmentNotFound)
}

pub async fn delete_assignment(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<()> {
    let res = db
        .execute_raw(sql(
            "DELETE FROM iam_assignments WHERE org_id = $1 AND id = $2",
            vec![org_id.into(), id.into()],
        ))
        .await?;
    if res.rows_affected() == 0 {
        return Err(IamError::AssignmentNotFound);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_are_known_and_never_iam() {
        assert!(check_pattern("*").is_ok());
        assert!(check_pattern("flows:*").is_ok());
        assert!(check_pattern("flows:deploy").is_ok());
        assert_eq!(
            check_pattern("flows:fly"),
            Err("unknown_permission:flows:fly".into())
        );
        assert_eq!(
            check_pattern("nope:*"),
            Err("unknown_permission:nope:*".into())
        );
        assert_eq!(check_pattern("iam:*"), Err("iam_permission:iam:*".into()));
        assert_eq!(
            check_pattern("iam:read"),
            Err("iam_permission:iam:read".into())
        );
    }

    #[test]
    fn keys_are_slugs_and_never_managed() {
        assert_eq!(check_key(" site-lyon ").unwrap(), "site-lyon");
        for bad in ["", "Site", "a b", "-x", "viewer", "owner"] {
            assert!(check_key(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn nobody_grants_what_they_do_not_hold() {
        let member = Grants::managed(&OrgMemberRole::Member);
        let deploy = Role {
            key: "deployer".into(),
            statements: vec![Statement {
                effect: Effect::Allow,
                permissions: vec!["flows:deploy".into()],
            }],
            managed: false,
        };
        assert!(check_escalation(&member, &deploy).is_ok());
        let secrets = Role {
            key: "vault".into(),
            statements: vec![Statement {
                effect: Effect::Allow,
                permissions: vec!["secrets:*".into()],
            }],
            managed: false,
        };
        assert!(matches!(
            check_escalation(&member, &secrets),
            Err(IamError::Escalation { .. })
        ));
        // A deny never needs the caller to hold anything.
        let deny = Role {
            key: "no-vault".into(),
            statements: vec![Statement {
                effect: Effect::Deny,
                permissions: vec!["*".into()],
            }],
            managed: false,
        };
        assert!(check_escalation(&Grants::managed(&OrgMemberRole::Viewer), &deny).is_ok());
    }
}
