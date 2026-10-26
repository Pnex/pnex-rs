//! Replay of the stored mails to a `mail_received` node (mail.md D464). The
//! bus is a publication: a mail published while the flow is stopped, being
//! redeployed or cut from Valkey never reaches the node. On each start the
//! node asks here for the mails after its cursor, then acknowledges every
//! mail it emits. Its first start only sets the cursor: no history.
//!
//! The node is named by its deployed reference: the mailboxes and filters
//! are read from the deployed version (the deploy checked the deployer
//! reads them, D454), never from the request.

use chrono::Utc;
use pnex_core::flow::FlowNodeKind;
use pnex_core::mail::{
    clean_text, MailAckRequest, MailMessage, MailReceivedConfig, MailReplayRequest, EVENT_TEXT_MAX,
    REPLAY_DAYS, REPLAY_MAX,
};
use pnex_core::ontology::flow::FlowNodeRef;
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{ConnectionTrait, DatabaseConnection, FromQueryResult};
use uuid::Uuid;

use super::{stmt, MessageRow, MESSAGE_COLUMNS};
use crate::services::ontology::flow_nodes::{deployed, NodeError};
use crate::services::ontology::OntologyError;

async fn config(
    db: &DatabaseConnection,
    node: &FlowNodeRef,
) -> Result<MailReceivedConfig, NodeError> {
    match deployed(db, node).await? {
        FlowNodeKind::MailReceived { config } => Ok(config),
        _ => Err(NodeError::NotDeployed),
    }
}

fn db(e: sea_orm::DbErr) -> NodeError {
    NodeError::Onto(OntologyError::Db(e))
}

/// The mails the node missed, oldest first (at most [`REPLAY_MAX`], received
/// in the last [`REPLAY_DAYS`] days).
pub async fn replay(
    db_: &DatabaseConnection,
    req: &MailReplayRequest,
) -> Result<Vec<MailMessage>, NodeError> {
    let cfg = config(db_, &req.node).await?;
    let n = &req.node;
    let cursor = db_
        .query_one_raw(stmt(
            "SELECT last_received_at, last_id FROM mail_flow_cursors WHERE flow_id = $1 AND node_id = $2",
            vec![n.flow_id.into(), n.node_id.clone().into()],
        ))
        .await
        .map_err(db)?;
    let Some(row) = cursor else {
        db_.execute_raw(stmt(
            "INSERT INTO mail_flow_cursors (flow_id, node_id, org_id, last_received_at) \
             VALUES ($1, $2, $3, now()) ON CONFLICT DO NOTHING",
            vec![n.flow_id.into(), n.node_id.clone().into(), n.org_id.into()],
        ))
        .await
        .map_err(db)?;
        return Ok(Vec::new());
    };
    let at: DateTimeWithTimeZone = row.try_get("", "last_received_at").map_err(db)?;
    let last: Option<Uuid> = row.try_get("", "last_id").map_err(db)?;
    let boxes: Vec<Uuid> = cfg
        .mailboxes
        .iter()
        .filter_map(|m| Uuid::parse_str(m).ok())
        .collect();
    let since: DateTimeWithTimeZone = (Utc::now() - chrono::Duration::days(REPLAY_DAYS)).into();
    // The node's filters run in SQL, so the limit counts only its mails.
    let mut sql = format!(
        "SELECT {MESSAGE_COLUMNS} FROM mail_messages WHERE org_id = $1 AND mailbox_id = ANY($2) \
         AND (received_at, id) > ($3, $4) AND received_at > $5"
    );
    let mut values: Vec<sea_orm::Value> = vec![
        n.org_id.into(),
        boxes.into(),
        at.into(),
        last.unwrap_or(Uuid::nil()).into(),
        since.into(),
    ];
    let mut bind = |sql: &mut String, cond: &str, v: sea_orm::Value| {
        values.push(v);
        sql.push_str(&cond.replace("$?", &format!("${}", values.len())));
    };
    if cfg.skip_automatic {
        sql.push_str(" AND NOT automatic AND NOT from_pnex");
    }
    if !cfg.folder.is_empty() {
        bind(
            &mut sql,
            " AND lower(folder) = lower($?)",
            cfg.folder.clone().into(),
        );
    }
    let like = |s: &str| {
        format!(
            "%{}%",
            pnex_core::fold::fold(s)
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        )
    };
    if !cfg.from_contains.is_empty() {
        let col = pnex_core::fold::sql("(from_name || ' ' || from_address)");
        bind(
            &mut sql,
            &format!(" AND {col} LIKE $?"),
            like(&cfg.from_contains).into(),
        );
    }
    if !cfg.subject_contains.is_empty() {
        let col = pnex_core::fold::sql("subject");
        bind(
            &mut sql,
            &format!(" AND {col} LIKE $?"),
            like(&cfg.subject_contains).into(),
        );
    }
    sql.push_str(&format!(" ORDER BY received_at, id LIMIT {REPLAY_MAX}"));
    let rows = MessageRow::find_by_statement(stmt(&sql, values))
        .all(db_)
        .await
        .map_err(db)?;
    Ok(rows
        .into_iter()
        .map(MessageRow::view)
        .filter(|m| cfg.accepts(m))
        .map(|mut m| {
            m.text = clean_text(&m.text, EVENT_TEXT_MAX).0;
            m
        })
        .collect())
}

/// Moves the node's cursor to a mail it emitted (never backwards).
pub async fn ack(db_: &DatabaseConnection, req: &MailAckRequest) -> Result<(), NodeError> {
    config(db_, &req.node).await?;
    let Ok(id) = Uuid::parse_str(&req.id) else {
        return Err(NodeError::NotDeployed);
    };
    let n = &req.node;
    db_.execute_raw(stmt(
        "INSERT INTO mail_flow_cursors (flow_id, node_id, org_id, last_received_at, last_id) \
         SELECT $1, $2, $3, received_at, id FROM mail_messages \
           WHERE org_id = $3 AND id = $4 \
         ON CONFLICT (flow_id, node_id) DO UPDATE SET last_received_at = EXCLUDED.last_received_at, \
           last_id = EXCLUDED.last_id, updated_at = now() \
         WHERE (mail_flow_cursors.last_received_at, COALESCE(mail_flow_cursors.last_id, \
           '00000000-0000-0000-0000-000000000000'::uuid)) < (EXCLUDED.last_received_at, EXCLUDED.last_id)",
        vec![
            n.flow_id.into(),
            n.node_id.clone().into(),
            n.org_id.into(),
            id.into(),
        ],
    ))
    .await
    .map_err(db)?;
    Ok(())
}
