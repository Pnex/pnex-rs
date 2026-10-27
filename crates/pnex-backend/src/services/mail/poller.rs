//! The poller of the mailboxes (mail.md D460): every two minutes, on one
//! instance, each active mailbox is polled folder by folder after its UID
//! cursor; each new message is stored once (one per `Message-ID`), then
//! published to the flows of its org. The cursor moves only once the
//! message is written. A refusal of the server stops the mailbox until it
//! is changed; a network error is retried at the next tick.

use std::time::Duration;

use loco_rs::app::AppContext;
use pnex_core::err_codes;
use pnex_core::mail::{mail_channel, MailEvent, MailMessage, MailboxError};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::{json, Value};
use uuid::Uuid;

use super::{imap, parse, Row};
use crate::services::secrets::Keyring;

const INTERVAL_SECS: u64 = 120;
/// First poll of a folder: the messages of the last days only.
const FIRST_DAYS: i64 = 7;
/// Messages accepted per mailbox and per hour (D457.4).
const HOURLY_MAX: i64 = 500;
const VALKEY_TIMEOUT: Duration = Duration::from_secs(2);

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// Started at boot, skipped in tests.
pub fn spawn(ctx: &AppContext) {
    use loco_rs::config::WorkerMode;
    if matches!(ctx.config.workers.mode, WorkerMode::ForegroundBlocking) {
        return;
    }
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(INTERVAL_SECS));
        loop {
            tick.tick().await;
            let every = Duration::from_secs(INTERVAL_SECS);
            if !crate::services::singleton::my_turn(&ctx.db, "mail-poller", every).await {
                continue;
            }
            if let Err(e) = run_once(&ctx).await {
                tracing::warn!(error = %e, "mail poller failed");
            }
        }
    });
}

/// One pass over every active mailbox.
pub async fn run_once(ctx: &AppContext) -> Result<usize, String> {
    let ring = Keyring::from_config(&ctx.config).map_err(|e| e.to_string())?;
    let valkey = crate::services::shared_valkey::conn(&ctx.config).await;
    let rows = super::Row::find_active(&ctx.db)
        .await
        .map_err(|e| e.to_string())?;
    let mut n = 0;
    for row in rows {
        n += poll(&ctx.db, &ring, valkey.clone(), &row).await;
    }
    Ok(n)
}

impl Row {
    async fn find_active(db: &DatabaseConnection) -> Result<Vec<Row>, sea_orm::DbErr> {
        use sea_orm::FromQueryResult;
        Row::find_by_statement(stmt(
            &format!(
                "SELECT {} FROM mailboxes WHERE state = 'active' ORDER BY last_sync_at NULLS FIRST",
                super::COLUMNS
            ),
            vec![],
        ))
        .all(db)
        .await
    }
}

/// Polls one mailbox and records the outcome on it; returns the messages
/// stored.
pub async fn poll(
    db: &DatabaseConnection,
    ring: &Keyring,
    valkey: Option<redis::aio::ConnectionManager>,
    row: &Row,
) -> usize {
    let outcome = poll_inner(db, ring, valkey, row).await;
    let (state, error) = match &outcome {
        Ok(_) => ("active", None),
        Err(e) if imap::is_permanent(e) || e.code == err_codes::MAIL_RATE_EXCEEDED => (
            if e.code == err_codes::MAIL_RATE_EXCEEDED {
                "paused"
            } else {
                "error"
            },
            Some(e),
        ),
        // A network error stays active: the next tick tries again.
        Err(e) => ("active", Some(e)),
    };
    let _ = db
        .execute_raw(stmt(
            "UPDATE mailboxes SET state = $2, last_error_code = $3, last_error_args = $4, \
               last_sync_at = CASE WHEN $3 IS NULL THEN now() ELSE last_sync_at END \
             WHERE id = $1 AND state = 'active'",
            vec![
                row.id.into(),
                state.into(),
                error.map(|e| e.code.clone()).into(),
                error.map(|e| e.args.clone()).into(),
            ],
        ))
        .await;
    if let Err(e) = &outcome {
        tracing::info!(mailbox = %row.id, code = %e.code, "mailbox not polled");
    }
    let _ = db
        .execute_raw(stmt(
            "DELETE FROM mail_messages WHERE mailbox_id = $1 \
             AND received_at < now() - make_interval(days => $2::int)",
            vec![row.id.into(), row.retention_days.into()],
        ))
        .await;
    outcome.unwrap_or(0)
}

/// Addresses of the org's mailboxes (our own mails, D457.2) and of its
/// members (whose forwards are trusted, D463).
async fn addresses(
    db: &DatabaseConnection,
    org_id: i64,
) -> Result<(Vec<String>, Vec<String>), sea_orm::DbErr> {
    let own = db
        .query_all_raw(stmt(
            "SELECT address FROM mailboxes WHERE org_id = $1",
            vec![org_id.into()],
        ))
        .await?
        .iter()
        .filter_map(|r| r.try_get::<String>("", "address").ok())
        .collect();
    let members = db
        .query_all_raw(stmt(
            "SELECT lower(u.email) AS email FROM organization_members m JOIN users u ON u.id = m.user_id \
             WHERE m.org_id = $1 AND NOT u.is_service",
            vec![org_id.into()],
        ))
        .await?
        .iter()
        .filter_map(|r| r.try_get::<String>("", "email").ok())
        .collect();
    Ok((own, members))
}

fn db_error(e: sea_orm::DbErr) -> MailboxError {
    tracing::warn!(error = %e, "mail poller database error");
    imap::error(err_codes::MAIL_PROTOCOL, Value::Null)
}

async fn poll_inner(
    db: &DatabaseConnection,
    ring: &Keyring,
    mut valkey: Option<redis::aio::ConnectionManager>,
    row: &Row,
) -> Result<usize, MailboxError> {
    let recent = db
        .query_one_raw(stmt(
            "SELECT count(*)::bigint AS n FROM mail_messages WHERE mailbox_id = $1 \
             AND received_at > now() - interval '1 hour'",
            vec![row.id.into()],
        ))
        .await
        .map_err(db_error)?
        .and_then(|r| r.try_get::<i64>("", "n").ok())
        .unwrap_or(0);
    if recent >= HOURLY_MAX {
        return Err(imap::error(
            err_codes::MAIL_RATE_EXCEEDED,
            json!({"max": HOURLY_MAX}),
        ));
    }
    let password = super::password(db, ring, row).await?;
    let (own, members) = addresses(db, row.org_id).await.map_err(db_error)?;
    let mut session = imap::connect(
        &row.imap_host,
        row.imap_port as u16,
        &row.imap_user,
        &password,
    )
    .await?;
    drop(password);
    let mut stored = 0;
    let mut budget = (HOURLY_MAX - recent).max(0) as usize;
    let mut result = Ok(());
    for folder in &row.folders {
        if budget == 0 {
            break;
        }
        match poll_folder(
            db,
            &mut session,
            row,
            folder,
            &own,
            &members,
            &mut valkey,
            &mut budget,
        )
        .await
        {
            Ok(n) => stored += n,
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }
    imap::logout(session).await;
    result.map(|_| stored)
}

#[allow(clippy::too_many_arguments)] // one folder of one mailbox, all named
async fn poll_folder(
    db: &DatabaseConnection,
    s: &mut imap::Session,
    row: &Row,
    folder: &str,
    own: &[String],
    members: &[String],
    valkey: &mut Option<redis::aio::ConnectionManager>,
    budget: &mut usize,
) -> Result<usize, MailboxError> {
    let (validity, _) = imap::examine(s, folder).await?;
    let cursor = db
        .query_one_raw(stmt(
            "SELECT uid_validity, last_uid FROM mail_cursors WHERE mailbox_id = $1 AND folder = $2",
            vec![row.id.into(), folder.into()],
        ))
        .await
        .map_err(db_error)?
        .and_then(|r| {
            Some((
                r.try_get::<i64>("", "uid_validity").ok()?,
                r.try_get::<i64>("", "last_uid").ok()?,
            ))
        });
    // A new UIDVALIDITY renumbers the folder: read the last days again,
    // the Message-ID keeps each message once.
    let after = match cursor {
        Some((v, last)) if v == validity as i64 => Some(last as u32),
        _ => None,
    };
    let mut uids = imap::new_uids(s, after, FIRST_DAYS).await?;
    uids.truncate(*budget);
    let fetched = imap::fetch(s, &uids).await?;
    let mut stored = 0;
    let mut last = after.unwrap_or(0);
    for f in fetched {
        let fallback = format!("<{}.{}.{}@pnex.mailbox>", f.uid, validity, row.id);
        if let Some(mut p) = parse::parse(&f.raw, f.truncated, own, &fallback) {
            if !f.truncated {
                p.auth = super::verify::auth(&f.raw).await;
            }
            if let Some(m) = store(db, row, folder, f.uid, p, members)
                .await
                .map_err(db_error)?
            {
                stored += 1;
                *budget = budget.saturating_sub(1);
                publish(valkey, row.org_id, m).await;
            }
        }
        last = last.max(f.uid);
        db.execute_raw(stmt(
            "INSERT INTO mail_cursors (mailbox_id, folder, uid_validity, last_uid) VALUES ($1, $2, $3, $4) \
             ON CONFLICT (mailbox_id, folder) DO UPDATE SET uid_validity = $3, last_uid = $4",
            vec![row.id.into(), folder.into(), (validity as i64).into(), (last as i64).into()],
        ))
        .await
        .map_err(db_error)?;
    }
    // A first poll with nothing new still records where the folder is.
    if cursor.is_none() && uids.is_empty() {
        db.execute_raw(stmt(
            "INSERT INTO mail_cursors (mailbox_id, folder, uid_validity, last_uid) VALUES ($1, $2, $3, 0) \
             ON CONFLICT (mailbox_id, folder) DO NOTHING",
            vec![row.id.into(), folder.into(), (validity as i64).into()],
        ))
        .await
        .map_err(db_error)?;
    }
    Ok(stored)
}

/// Writes a parsed message; `None` when the mailbox already holds it.
pub async fn store<C: ConnectionTrait>(
    db: &C,
    row: &Row,
    folder: &str,
    uid: u32,
    p: parse::Parsed,
    members: &[String],
) -> Result<Option<MailMessage>, sea_orm::DbErr> {
    // The first hop from outside the org; believed only when a member
    // forwarded it, authenticated (D463).
    let forwarded = pnex_core::mail::forward_origin(p.forwards, members).map(|mut f| {
        f.trusted = members.iter().any(|m| m == &p.from.address) && p.auth.trusted();
        f
    });
    let sent_at = p
        .date
        .as_deref()
        .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok());
    let id = Uuid::new_v4();
    let inserted = db
        .query_one_raw(stmt(
            "INSERT INTO mail_messages (id, org_id, mailbox_id, folder, uid, message_id, in_reply_to, refs, \
               from_name, from_address, to_addrs, cc_addrs, subject, sent_at, body, truncated, attachments, \
               auth, automatic, from_pnex, forwarded, invitation) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22) \
             ON CONFLICT (mailbox_id, message_id) DO NOTHING RETURNING received_at",
            vec![
                id.into(),
                row.org_id.into(),
                row.id.into(),
                folder.into(),
                (uid as i64).into(),
                p.message_id.clone().into(),
                p.in_reply_to.clone().into(),
                p.references.clone().into(),
                p.from.name.clone().into(),
                p.from.address.clone().into(),
                serde_json::to_value(&p.to).unwrap_or_default().into(),
                serde_json::to_value(&p.cc).unwrap_or_default().into(),
                p.subject.clone().into(),
                sent_at.into(),
                p.text.clone().into(),
                p.truncated.into(),
                serde_json::to_value(&p.attachments).unwrap_or_default().into(),
                serde_json::to_value(p.auth).unwrap_or_default().into(),
                p.automatic.into(),
                p.from_pnex.into(),
                forwarded.as_ref().map(|f| serde_json::to_value(f).unwrap_or_default()).into(),
                p.invitation.as_ref().map(|i| serde_json::to_value(i).unwrap_or_default()).into(),
            ],
        ))
        .await?;
    let Some(r) = inserted else {
        return Ok(None);
    };
    let received_at = r
        .try_get::<sea_orm::prelude::DateTimeWithTimeZone>("", "received_at")
        .map(|t| t.to_rfc3339())
        .unwrap_or_default();
    Ok(Some(MailMessage {
        id: id.to_string(),
        mailbox_id: row.id.to_string(),
        folder: folder.to_string(),
        message_id: p.message_id,
        in_reply_to: p.in_reply_to,
        references: p.references,
        from: p.from,
        to: p.to,
        cc: p.cc,
        subject: p.subject,
        date: p.date,
        text: p.text,
        truncated: p.truncated,
        attachments: p.attachments,
        auth: p.auth,
        automatic: p.automatic,
        from_pnex: p.from_pnex,
        forwarded,
        invitation: p.invitation,
        received_at,
    }))
}

/// Hands a new message to the flows of its org; best effort: the message
/// is stored either way.
async fn publish(
    valkey: &mut Option<redis::aio::ConnectionManager>,
    org_id: i64,
    mut m: MailMessage,
) {
    let Some(conn) = valkey.as_mut() else {
        return;
    };
    let (text, _) = pnex_core::mail::clean_text(&m.text, pnex_core::mail::EVENT_TEXT_MAX);
    m.text = text;
    let Ok(frame) = serde_json::to_string(&MailEvent { message: m }) else {
        return;
    };
    let op = redis::AsyncCommands::publish::<_, _, i64>(conn, mail_channel(org_id), frame);
    if !matches!(tokio::time::timeout(VALKEY_TIMEOUT, op).await, Ok(Ok(_))) {
        tracing::info!(org_id, "mail event not published");
    }
}
