//! Mailboxes of an org (mail.md D453–D463): a source owned by a user (a
//! person or a service account) or shared with teams, its password in the
//! vault, its messages polled by the poller and published to the flows
//! that listen to them. Who manages a mailbox and who reads its messages
//! is decided here (D454), never by the caller.

pub mod imap;
pub mod parse;
pub mod poller;
pub mod replay;
pub mod verify;

use std::collections::HashMap;

use pnex_core::err_codes;
use pnex_core::mail::{
    MailAddress, MailAttachment, MailAuth, MailFolderCount, MailForward, MailMessage, Mailbox,
    MailboxError, MailboxInput, MailboxOwner, MailboxState, MailboxTestResult, ADDRESS_MAX,
    FOLDERS_MAX, HOST_MAX, IMAPS_PORT, NAME_MAX, RETENTION_DAYS_DEFAULT, RETENTION_DAYS_MAX,
    RETENTION_DAYS_MIN, TEAMS_MAX,
};
use pnex_core::SecretFieldInput;
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, FromQueryResult, Statement,
    TransactionTrait,
};
use serde_json::Value;
use uuid::Uuid;

use crate::services::secrets::store::{self, StoreError, Writer};
use crate::services::secrets::{mailbox as vault, Keyring};

/// Read permission of the messages (D454), on top of owning or sharing.
pub const CONTENT_READ: &str = "mail.content:read";
/// Manage shared mailboxes and those of service accounts (D454).
pub const SHARED_WRITE: &str = "mail.shared:write";

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// The caller, as the mailbox rules see it.
#[derive(Clone, Copy, Debug)]
pub struct Caller {
    pub user_id: i64,
    /// `mail.shared:write`: shared and service-account mailboxes.
    pub manages_shared: bool,
    /// `mail.content:read`.
    pub reads_content: bool,
    /// May pick or type any vault secret (owner/admin, D117).
    pub can_write_secrets: bool,
}

#[derive(Debug)]
pub enum MailError {
    Invalid { field: String, token: String },
    NotFound,
    NameTaken,
    Store(StoreError),
}

impl From<DbErr> for MailError {
    fn from(e: DbErr) -> Self {
        Self::Store(StoreError::Db(e))
    }
}

impl From<StoreError> for MailError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

fn invalid(field: impl Into<String>, token: impl Into<String>) -> MailError {
    MailError::Invalid {
        field: field.into(),
        token: token.into(),
    }
}

#[derive(Debug, Clone, FromQueryResult)]
pub struct Row {
    pub id: Uuid,
    pub org_id: i64,
    pub name: String,
    pub address: String,
    pub owner_user_id: Option<i64>,
    pub teams: Vec<Uuid>,
    pub imap_host: String,
    pub imap_port: i32,
    pub imap_user: String,
    pub secret_id: Option<Uuid>,
    pub folders: Vec<String>,
    pub state: String,
    pub last_error_code: Option<String>,
    pub last_error_args: Option<Value>,
    pub last_sync_at: Option<DateTimeWithTimeZone>,
    pub retention_days: i32,
    pub version: i32,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

const COLUMNS: &str = "id, org_id, name, address, owner_user_id, teams, imap_host, imap_port, \
     imap_user, secret_id, folders, state, last_error_code, last_error_args, last_sync_at, \
     retention_days, version, created_at, updated_at";

impl Row {
    pub fn state(&self) -> MailboxState {
        MailboxState::parse(&self.state).unwrap_or_default()
    }
}

/// Owners that are service accounts (agents, applications) of the org.
async fn service_users<C: ConnectionTrait>(db: &C, org_id: i64) -> Result<Vec<i64>, DbErr> {
    let rows = db
        .query_all_raw(stmt(
            "SELECT user_id FROM iam_service_accounts WHERE org_id = $1",
            vec![org_id.into()],
        ))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| r.try_get("", "user_id").ok())
        .collect())
}

/// Teams of the caller in the org.
async fn teams_of<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    user_id: i64,
) -> Result<Vec<Uuid>, DbErr> {
    let rows = db
        .query_all_raw(stmt(
            "SELECT m.team_id FROM iam_team_members m JOIN iam_teams t ON t.id = m.team_id
             WHERE t.org_id = $1 AND m.user_id = $2",
            vec![org_id.into(), user_id.into()],
        ))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| r.try_get("", "team_id").ok())
        .collect())
}

/// What the caller may do with the mailboxes of the org.
pub struct Access {
    caller: Caller,
    teams: Vec<Uuid>,
    services: Vec<i64>,
}

impl Access {
    pub async fn of<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        caller: Caller,
    ) -> Result<Self, DbErr> {
        Ok(Self {
            caller,
            teams: teams_of(db, org_id, caller.user_id).await?,
            services: service_users(db, org_id).await?,
        })
    }

    /// Change, pause or delete the mailbox: its owner, or a manager of
    /// shared mailboxes for a shared or service-account one. A personal
    /// mailbox of another member is out of reach except pause and delete
    /// (`moderate`).
    pub fn manages(&self, r: &Row) -> bool {
        match r.owner_user_id {
            Some(u) if u == self.caller.user_id => true,
            Some(u) if self.services.contains(&u) => self.caller.manages_shared,
            Some(_) => false,
            None => self.caller.manages_shared,
        }
    }

    /// Pause or delete any mailbox of the org (an admin over a member's
    /// personal mailbox: never its content, D454).
    pub fn moderates(&self, r: &Row) -> bool {
        self.manages(r) || self.caller.manages_shared
    }

    /// Read the messages of the mailbox (D454).
    pub fn reads(&self, r: &Row) -> bool {
        if !self.caller.reads_content {
            return false;
        }
        match r.owner_user_id {
            Some(u) if u == self.caller.user_id => true,
            Some(u) if self.services.contains(&u) => self.caller.manages_shared,
            Some(_) => false,
            None => {
                r.teams.iter().any(|t| self.teams.contains(t))
                    || (r.teams.is_empty() && self.caller.manages_shared)
            }
        }
    }
}

pub async fn list<C: ConnectionTrait>(db: &C, org_id: i64) -> Result<Vec<Row>, DbErr> {
    Row::find_by_statement(stmt(
        &format!("SELECT {COLUMNS} FROM mailboxes WHERE org_id = $1 ORDER BY name"),
        vec![org_id.into()],
    ))
    .all(db)
    .await
}

pub async fn find<C: ConnectionTrait>(db: &C, org_id: i64, id: Uuid) -> Result<Row, MailError> {
    Row::find_by_statement(stmt(
        &format!("SELECT {COLUMNS} FROM mailboxes WHERE org_id = $1 AND id = $2"),
        vec![org_id.into(), id.into()],
    ))
    .one(db)
    .await?
    .ok_or(MailError::NotFound)
}

/// Read models: never the password, only whether one is stored.
pub async fn views<C: ConnectionTrait>(
    db: &C,
    access: &Access,
    rows: &[Row],
) -> Result<Vec<Mailbox>, DbErr> {
    let owners: Vec<i64> = rows.iter().filter_map(|r| r.owner_user_id).collect();
    let mut names: HashMap<i64, (String, bool)> = HashMap::new();
    if !owners.is_empty() {
        for r in db
            .query_all_raw(stmt(
                "SELECT id, full_name, is_service FROM users WHERE id = ANY($1)",
                vec![owners.into()],
            ))
            .await?
        {
            if let (Ok(id), Ok(name), Ok(service)) = (
                r.try_get::<i64>("", "id"),
                r.try_get::<String>("", "full_name"),
                r.try_get::<bool>("", "is_service"),
            ) {
                names.insert(id, (name, service));
            }
        }
    }
    Ok(rows
        .iter()
        .map(|r| Mailbox {
            id: r.id.to_string(),
            name: r.name.clone(),
            address: r.address.clone(),
            owner: r.owner_user_id.map(|u| {
                let (name, service) = names.get(&u).cloned().unwrap_or_default();
                MailboxOwner {
                    user_id: u,
                    name,
                    service,
                }
            }),
            teams: r.teams.iter().map(Uuid::to_string).collect(),
            imap_host: r.imap_host.clone(),
            imap_port: r.imap_port as u16,
            imap_user: r.imap_user.clone(),
            has_password: r.secret_id.is_some(),
            folders: r.folders.clone(),
            state: r.state(),
            last_error: r.last_error_code.clone().map(|code| MailboxError {
                code,
                args: r.last_error_args.clone().unwrap_or(Value::Null),
            }),
            last_sync_at: r.last_sync_at.map(|t| t.to_rfc3339()),
            retention_days: r.retention_days,
            readable: access.reads(r),
            version: r.version,
            created_at: r.created_at.to_rfc3339(),
            updated_at: r.updated_at.to_rfc3339(),
        })
        .collect())
}

/// Checked fields of an input.
struct Checked {
    name: String,
    address: String,
    host: String,
    port: u16,
    user: String,
    folders: Vec<String>,
    retention: i32,
    teams: Vec<Uuid>,
}

fn too_long(field: &str, max: usize) -> MailError {
    invalid(field, format!("{}:{max}", err_codes::FIELD_MAX_LENGTH))
}

async fn check<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    input: &MailboxInput,
) -> Result<Checked, MailError> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(invalid("name", err_codes::FIELD_REQUIRED));
    }
    if name.chars().count() > NAME_MAX {
        return Err(too_long("name", NAME_MAX));
    }
    let address = pnex_core::mail::parse_address(&input.address)
        .ok_or_else(|| invalid("address", err_codes::FIELD_INVALID))?
        .address;
    let host = input
        .imap_host
        .trim()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host.is_empty() {
        return Err(invalid("imap_host", err_codes::FIELD_REQUIRED));
    }
    if host.len() > HOST_MAX
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
    {
        return Err(invalid("imap_host", err_codes::FIELD_INVALID));
    }
    if let Some(code) = pnex_core::egress::check_host(&host, pnex_core::egress::policy()) {
        return Err(invalid("imap_host", code));
    }
    let port = input.imap_port.unwrap_or(IMAPS_PORT);
    if port == 0 {
        return Err(invalid("imap_port", err_codes::FIELD_INVALID));
    }
    let user = input.imap_user.trim();
    if user.is_empty() {
        return Err(invalid("imap_user", err_codes::FIELD_REQUIRED));
    }
    if user.chars().count() > ADDRESS_MAX || user.chars().any(char::is_control) {
        return Err(invalid("imap_user", err_codes::FIELD_INVALID));
    }
    let mut folders: Vec<String> = input
        .folders
        .iter()
        .map(|f| f.trim().to_string())
        .filter(|f| !f.is_empty())
        .collect();
    folders.dedup();
    if folders.is_empty() {
        folders.push("INBOX".into());
    }
    if folders.len() > FOLDERS_MAX {
        return Err(too_long("folders", FOLDERS_MAX));
    }
    if let Some(i) = folders.iter().position(|f| !imap::folder_ok(f)) {
        return Err(invalid(format!("folders.{i}"), err_codes::FIELD_INVALID));
    }
    let retention = input.retention_days.unwrap_or(RETENTION_DAYS_DEFAULT);
    if !(RETENTION_DAYS_MIN..=RETENTION_DAYS_MAX).contains(&retention) {
        return Err(invalid("retention_days", err_codes::FIELD_INVALID));
    }
    if input.teams.len() > TEAMS_MAX {
        return Err(too_long("teams", TEAMS_MAX));
    }
    let mut teams = Vec::new();
    for (i, t) in input.teams.iter().enumerate() {
        teams.push(
            Uuid::parse_str(t)
                .map_err(|_| invalid(format!("teams.{i}"), err_codes::FIELD_INVALID_UUID))?,
        );
    }
    if !teams.is_empty() {
        let known = db
            .query_all_raw(stmt(
                "SELECT id FROM iam_teams WHERE org_id = $1 AND id = ANY($2)",
                vec![org_id.into(), teams.clone().into()],
            ))
            .await?
            .len();
        if known != teams.len() {
            return Err(invalid("teams", err_codes::FIELD_INVALID));
        }
    }
    Ok(Checked {
        name: name.to_string(),
        address,
        host,
        port,
        user: user.to_string(),
        folders,
        retention,
        teams,
    })
}

/// The owner a creation asks for: the caller, a service account of the org
/// (shared managers only), or none (shared, shared managers only).
async fn owner_of<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    caller: Caller,
    input: &MailboxInput,
) -> Result<Option<i64>, MailError> {
    if input.shared {
        return match caller.manages_shared {
            true => Ok(None),
            false => Err(invalid("shared", err_codes::FIELD_INVALID)),
        };
    }
    match input.owner_user_id {
        None => Ok(Some(caller.user_id)),
        Some(u) if u == caller.user_id => Ok(Some(u)),
        Some(u) if caller.manages_shared && service_users(db, org_id).await?.contains(&u) => {
            Ok(Some(u))
        }
        Some(_) => Err(invalid("owner_user_id", err_codes::FIELD_INVALID)),
    }
}

/// The password rule (D454, R9): a member types the password of their own
/// mailbox into its dedicated secret, never picks a vault secret; owners
/// and admins may pick. A typed value is accepted for one's own mailbox.
fn password_allowed(
    caller: Caller,
    owner: Option<i64>,
    input: Option<&SecretFieldInput>,
) -> Result<bool, MailError> {
    match input {
        None => Ok(caller.can_write_secrets),
        Some(SecretFieldInput::Pick { .. }) if !caller.can_write_secrets => {
            Err(invalid("password", err_codes::FIELD_INVALID))
        }
        Some(SecretFieldInput::Pick { .. }) => Ok(true),
        Some(SecretFieldInput::Value { .. }) => {
            Ok(caller.can_write_secrets || owner == Some(caller.user_id))
        }
    }
}

fn writer(org_id: i64, caller: Caller) -> Writer {
    Writer {
        org_id: Some(org_id),
        user_id: Some(caller.user_id),
    }
}

async fn name_taken<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    name: &str,
    except: Option<Uuid>,
) -> Result<bool, DbErr> {
    Ok(db
        .query_one_raw(stmt(
            "SELECT 1 AS x FROM mailboxes WHERE org_id = $1 AND name = $2 AND ($3::uuid IS NULL OR id <> $3)",
            vec![org_id.into(), name.into(), except.into()],
        ))
        .await?
        .is_some())
}

pub async fn create(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
    caller: Caller,
    input: &MailboxInput,
) -> Result<Row, MailError> {
    let c = check(db, org_id, input).await?;
    let owner = owner_of(db, org_id, caller, input).await?;
    if owner.is_some() && !c.teams.is_empty() {
        return Err(invalid("teams", err_codes::FIELD_INVALID));
    }
    let Some(password) = input.password.as_ref() else {
        return Err(invalid("password", err_codes::FIELD_REQUIRED));
    };
    let may_write = password_allowed(caller, owner, Some(password))?;
    let txn = db.begin().await?;
    if name_taken(&txn, org_id, &c.name, None).await? {
        return Err(MailError::NameTaken);
    }
    let id = Uuid::new_v4();
    let secret = vault::save(
        &txn,
        ring,
        writer(org_id, caller),
        may_write,
        id,
        None,
        Some(password),
    )
    .await?;
    txn.execute_raw(stmt(
        "INSERT INTO mailboxes (id, org_id, name, address, owner_user_id, teams, imap_host, imap_port, \
           imap_user, secret_id, folders, state, retention_days, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
        vec![
            id.into(),
            org_id.into(),
            c.name.into(),
            c.address.into(),
            owner.into(),
            c.teams.into(),
            c.host.into(),
            (c.port as i32).into(),
            c.user.into(),
            secret.into(),
            c.folders.into(),
            if input.paused { "paused" } else { "active" }.into(),
            c.retention.into(),
            caller.user_id.into(),
        ],
    ))
    .await?;
    let row = find(&txn, org_id, id).await?;
    txn.commit().await?;
    Ok(row)
}

/// Full replacement; `password` absent keeps the current one, unless the
/// server or the user name changes and the caller may not move a secret
/// (R9): then the password must be typed again.
pub async fn update(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
    caller: Caller,
    id: Uuid,
    input: &MailboxInput,
) -> Result<Row, MailError> {
    let c = check(db, org_id, input).await?;
    let access = Access::of(db, org_id, caller).await?;
    let txn = db.begin().await?;
    let row = find(&txn, org_id, id).await?;
    if !access.manages(&row) {
        return Err(MailError::NotFound);
    }
    if let Some(v) = input.expected_version {
        if v != row.version {
            return Err(invalid("expected_version", err_codes::FIELD_INVALID));
        }
    }
    if row.owner_user_id.is_some() && !c.teams.is_empty() {
        return Err(invalid("teams", err_codes::FIELD_INVALID));
    }
    if name_taken(&txn, org_id, &c.name, Some(id)).await? {
        return Err(MailError::NameTaken);
    }
    let moved =
        row.imap_host != c.host || row.imap_port != c.port as i32 || row.imap_user != c.user;
    let may_write = password_allowed(caller, row.owner_user_id, input.password.as_ref())?;
    if moved && input.password.is_none() && !caller.can_write_secrets {
        return Err(invalid("password", err_codes::FIELD_REQUIRED));
    }
    let secret = match &input.password {
        None => row.secret_id,
        Some(p) => {
            vault::save(
                &txn,
                ring,
                writer(org_id, caller),
                may_write,
                id,
                row.secret_id,
                Some(p),
            )
            .await?
        }
    };
    // A changed mailbox is tried again: its error was perhaps the cause.
    let state = if input.paused { "paused" } else { "active" };
    txn.execute_raw(stmt(
        "UPDATE mailboxes SET name = $3, address = $4, teams = $5, imap_host = $6, imap_port = $7, \
           imap_user = $8, secret_id = $9, folders = $10, retention_days = $11, state = $12, \
           last_error_code = CASE WHEN $12 = 'active' THEN NULL ELSE last_error_code END, \
           last_error_args = CASE WHEN $12 = 'active' THEN NULL ELSE last_error_args END, \
           version = version + 1, updated_at = now() \
         WHERE org_id = $1 AND id = $2",
        vec![
            org_id.into(),
            id.into(),
            c.name.into(),
            c.address.into(),
            c.teams.into(),
            c.host.into(),
            (c.port as i32).into(),
            c.user.into(),
            secret.into(),
            c.folders.into(),
            c.retention.into(),
            state.into(),
        ],
    ))
    .await?;
    if moved {
        txn.execute_raw(stmt(
            "DELETE FROM mail_cursors WHERE mailbox_id = $1",
            vec![id.into()],
        ))
        .await?;
    }
    let row = find(&txn, org_id, id).await?;
    txn.commit().await?;
    Ok(row)
}

/// Pauses or resumes; any moderator (an admin over a member's mailbox).
pub async fn set_paused(
    db: &DatabaseConnection,
    org_id: i64,
    caller: Caller,
    id: Uuid,
    paused: bool,
) -> Result<Row, MailError> {
    let access = Access::of(db, org_id, caller).await?;
    let row = find(db, org_id, id).await?;
    if !access.moderates(&row) {
        return Err(MailError::NotFound);
    }
    db.execute_raw(stmt(
        "UPDATE mailboxes SET state = $3, last_error_code = NULL, last_error_args = NULL, \
           version = version + 1, updated_at = now() WHERE org_id = $1 AND id = $2",
        vec![
            org_id.into(),
            id.into(),
            if paused { "paused" } else { "active" }.into(),
        ],
    ))
    .await?;
    find(db, org_id, id).await
}

/// Deletes the mailbox, its messages and its dedicated password.
pub async fn delete(
    db: &DatabaseConnection,
    org_id: i64,
    caller: Caller,
    id: Uuid,
) -> Result<(), MailError> {
    let access = Access::of(db, org_id, caller).await?;
    let row = find(db, org_id, id).await?;
    if !access.moderates(&row) {
        return Err(MailError::NotFound);
    }
    let txn = db.begin().await?;
    vault::release(&txn, Some(org_id), id).await?;
    txn.execute_raw(stmt(
        "DELETE FROM mailboxes WHERE org_id = $1 AND id = $2",
        vec![org_id.into(), id.into()],
    ))
    .await?;
    txn.commit().await?;
    Ok(())
}

/// The decrypted password of a mailbox, for one connection.
pub async fn password<C: ConnectionTrait>(
    db: &C,
    ring: &Keyring,
    row: &Row,
) -> Result<String, MailboxError> {
    let Some(secret) = row.secret_id else {
        return Err(imap::error(err_codes::MAIL_NO_PASSWORD, Value::Null));
    };
    store::reveal(db, ring, Some(row.org_id), secret)
        .await
        .map_err(|_| imap::error(err_codes::MAIL_NO_PASSWORD, Value::Null))
}

/// Logs in and counts the messages of each folder; nothing is fetched.
pub async fn test(db: &DatabaseConnection, ring: &Keyring, row: &Row) -> MailboxTestResult {
    let started = std::time::Instant::now();
    let result = async {
        let pw = password(db, ring, row).await?;
        let mut s =
            imap::connect(&row.imap_host, row.imap_port as u16, &row.imap_user, &pw).await?;
        let mut folders = Vec::new();
        for f in &row.folders {
            match imap::examine(&mut s, f).await {
                Ok((_, n)) => folders.push(MailFolderCount {
                    folder: f.clone(),
                    messages: n,
                }),
                Err(e) => {
                    imap::logout(s).await;
                    return Err(e);
                }
            }
        }
        imap::logout(s).await;
        Ok(folders)
    }
    .await;
    let latency_ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(folders) => MailboxTestResult {
            ok: true,
            error: None,
            folders,
            latency_ms,
        },
        Err(e) => MailboxTestResult {
            ok: false,
            error: Some(e),
            folders: Vec::new(),
            latency_ms,
        },
    }
}

// ───────────────────────────── messages ─────────────────────────────

#[derive(Debug, Clone, FromQueryResult)]
struct MessageRow {
    id: Uuid,
    mailbox_id: Uuid,
    folder: String,
    message_id: String,
    in_reply_to: Option<String>,
    refs: Vec<String>,
    from_name: String,
    from_address: String,
    to_addrs: Value,
    cc_addrs: Value,
    subject: String,
    sent_at: Option<DateTimeWithTimeZone>,
    body: String,
    truncated: bool,
    attachments: Value,
    auth: Value,
    automatic: bool,
    from_pnex: bool,
    forwarded: Option<Value>,
    received_at: DateTimeWithTimeZone,
}

const MESSAGE_COLUMNS: &str = "id, mailbox_id, folder, message_id, in_reply_to, refs, from_name, \
     from_address, to_addrs, cc_addrs, subject, sent_at, body, truncated, attachments, auth, \
     automatic, from_pnex, forwarded, received_at";

impl MessageRow {
    fn view(self) -> MailMessage {
        let list = |v: Value| -> Vec<MailAddress> { serde_json::from_value(v).unwrap_or_default() };
        MailMessage {
            id: self.id.to_string(),
            mailbox_id: self.mailbox_id.to_string(),
            folder: self.folder,
            message_id: self.message_id,
            in_reply_to: self.in_reply_to,
            references: self.refs,
            from: MailAddress {
                name: self.from_name,
                address: self.from_address,
            },
            to: list(self.to_addrs),
            cc: list(self.cc_addrs),
            subject: self.subject,
            date: self.sent_at.map(|t| t.to_rfc3339()),
            text: self.body,
            truncated: self.truncated,
            attachments: serde_json::from_value::<Vec<MailAttachment>>(self.attachments)
                .unwrap_or_default(),
            auth: serde_json::from_value::<MailAuth>(self.auth).unwrap_or_default(),
            automatic: self.automatic,
            from_pnex: self.from_pnex,
            forwarded: self
                .forwarded
                .and_then(|v| serde_json::from_value::<MailForward>(v).ok()),
            received_at: self.received_at.to_rfc3339(),
        }
    }
}

/// Messages of a mailbox the caller reads, newest first, and their count.
pub async fn messages(
    db: &DatabaseConnection,
    org_id: i64,
    caller: Caller,
    id: Uuid,
    limit: u64,
    offset: u64,
) -> Result<(i64, Vec<MailMessage>), MailError> {
    let access = Access::of(db, org_id, caller).await?;
    let row = find(db, org_id, id).await?;
    if !access.reads(&row) {
        return Err(MailError::NotFound);
    }
    let count = db
        .query_one_raw(stmt(
            "SELECT count(*)::bigint AS n FROM mail_messages WHERE org_id = $1 AND mailbox_id = $2",
            vec![org_id.into(), id.into()],
        ))
        .await?
        .and_then(|r| r.try_get::<i64>("", "n").ok())
        .unwrap_or(0);
    let rows = MessageRow::find_by_statement(stmt(
        &format!(
            "SELECT {MESSAGE_COLUMNS} FROM mail_messages WHERE org_id = $1 AND mailbox_id = $2 \
             ORDER BY received_at DESC, id LIMIT $3 OFFSET $4"
        ),
        vec![
            org_id.into(),
            id.into(),
            (limit as i64).into(),
            (offset as i64).into(),
        ],
    ))
    .all(db)
    .await?;
    Ok((count, rows.into_iter().map(MessageRow::view).collect()))
}

/// The mailboxes a deployer may make a flow listen to (D454): `Ok(None)`
/// when all are readable, else the first one that is not (or unknown).
pub async fn unreadable_for(
    db: &DatabaseConnection,
    org_id: i64,
    caller: Caller,
    ids: &[String],
) -> Result<Option<(String, &'static str)>, DbErr> {
    let access = Access::of(db, org_id, caller).await?;
    for raw in ids {
        let Ok(id) = Uuid::parse_str(raw) else {
            return Ok(Some((raw.clone(), err_codes::MAIL_MAILBOX_UNKNOWN)));
        };
        match find(db, org_id, id).await {
            Ok(r) if access.reads(&r) => {}
            Ok(_) => return Ok(Some((raw.clone(), err_codes::MAIL_MAILBOX_FORBIDDEN))),
            Err(MailError::NotFound) => {
                return Ok(Some((raw.clone(), err_codes::MAIL_MAILBOX_UNKNOWN)))
            }
            Err(MailError::Store(StoreError::Db(e))) => return Err(e),
            Err(_) => return Ok(Some((raw.clone(), err_codes::MAIL_MAILBOX_UNKNOWN))),
        }
    }
    Ok(None)
}
