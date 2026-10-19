//! Recipients taken from flow messages (partner-watch.md D313). A notify
//! node with `to_field` asks here before each e-mail: the call must come
//! from that deployed node, the SMTP channel must be marked
//! `dynamic_recipients` by an org admin, the address must be one valid
//! e-mail, and the daily caps (per channel, per recipient) must allow it.
//! A "yes" counts the send. Addresses are stored hashed only.

use pnex_core::flow::FlowNodeKind;
use pnex_core::ontology::flow::FlowNodeRef;
use pnex_core::ontology::import::valid_email;
use pnex_core::{RecipientCheck, DYNAMIC_PER_CHANNEL_DAY, DYNAMIC_PER_RECIPIENT_DAY};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, QueryFilter,
    Statement, TransactionTrait,
};

use crate::models::_entities::notify_channels;

/// Why a send to a message recipient is refused (runtime diagnostic code).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    NotDeployed,
    ChannelNotDynamic,
    InvalidAddress,
    ChannelCap,
    RecipientCap,
}

impl Refusal {
    pub fn code(self) -> &'static str {
        match self {
            Self::NotDeployed => "flow-node-not-deployed",
            Self::ChannelNotDynamic => "notify-dynamic-recipients-off",
            Self::InvalidAddress => "notify-recipient-invalid",
            Self::ChannelCap => "notify-channel-daily-cap",
            Self::RecipientCap => "notify-recipient-daily-cap",
        }
    }
}

/// Is the SMTP channel config marked for message recipients?
pub fn dynamic(config: &serde_json::Value) -> bool {
    config
        .get("dynamic_recipients")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
}

pub async fn check(db: &DatabaseConnection, req: &RecipientCheck) -> Result<(), Refusal> {
    let node = FlowNodeRef {
        org_id: req.org_id,
        flow_id: req.flow_id,
        version: req.version,
        node_id: req.node_id.clone(),
    };
    let kind = crate::services::ontology::flow_nodes::deployed(db, &node)
        .await
        .map_err(|_| Refusal::NotDeployed)?;
    let FlowNodeKind::PnexNotify { config } = kind else {
        return Err(Refusal::NotDeployed);
    };
    if config.to_field.trim().is_empty() || !config.channel_ids.contains(&req.channel_id) {
        return Err(Refusal::NotDeployed);
    }
    let channel = notify_channels::Entity::find_by_id(req.channel_id)
        .filter(notify_channels::Column::OrgId.eq(req.org_id))
        .filter(notify_channels::Column::Enabled.eq(true))
        .one(db)
        .await
        .ok()
        .flatten()
        .filter(|c| c.kind == "smtp" && dynamic(&c.config))
        .ok_or(Refusal::ChannelNotDynamic)?;
    let mut recipients = vec![req.to.as_str()];
    recipients.extend(req.cc.as_deref());
    if recipients.iter().any(|r| !valid_email(r.trim())) {
        return Err(Refusal::InvalidAddress);
    }
    let hashes: Vec<String> = recipients
        .iter()
        .map(|r| pnex_notify::recipient_hash(r))
        .collect();
    count(db, req.org_id, channel.id, &hashes).await
}

/// Counts one send to `hashes` on the channel today, if the caps allow it.
async fn count(
    db: &DatabaseConnection,
    org_id: i64,
    channel: uuid::Uuid,
    hashes: &[String],
) -> Result<(), Refusal> {
    let run = async {
        let txn = db.begin().await?;
        let stmt = |sql: &str, v: Vec<sea_orm::Value>| {
            Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, v)
        };
        // One decision at a time per channel.
        txn.execute_raw(stmt(
            "SELECT pg_advisory_xact_lock(hashtext($1::text))",
            vec![channel.to_string().into()],
        ))
        .await?;
        let today = chrono::Utc::now().date_naive();
        let total = txn
            .query_one_raw(stmt(
                "SELECT coalesce(sum(count), 0)::bigint AS n FROM notify_dynamic_sends \
                 WHERE channel_id = $1 AND day = $2",
                vec![channel.into(), today.into()],
            ))
            .await?
            .and_then(|r| r.try_get::<i64>("", "n").ok())
            .unwrap_or(0);
        if total >= i64::from(DYNAMIC_PER_CHANNEL_DAY) {
            txn.rollback().await?;
            return Ok::<_, sea_orm::DbErr>(Err(Refusal::ChannelCap));
        }
        for h in hashes {
            let n = txn
                .query_one_raw(stmt(
                    "SELECT count FROM notify_dynamic_sends \
                     WHERE channel_id = $1 AND day = $2 AND recipient_hash = $3",
                    vec![channel.into(), today.into(), h.clone().into()],
                ))
                .await?
                .and_then(|r| r.try_get::<i32>("", "count").ok())
                .unwrap_or(0);
            if n >= DYNAMIC_PER_RECIPIENT_DAY {
                txn.rollback().await?;
                return Ok(Err(Refusal::RecipientCap));
            }
        }
        for h in hashes {
            txn.execute_raw(stmt(
                "INSERT INTO notify_dynamic_sends (org_id, channel_id, day, recipient_hash, count) \
                 VALUES ($1, $2, $3, $4, 1) \
                 ON CONFLICT (channel_id, day, recipient_hash) \
                 DO UPDATE SET count = notify_dynamic_sends.count + 1",
                vec![org_id.into(), channel.into(), today.into(), h.clone().into()],
            ))
            .await?;
        }
        txn.commit().await?;
        Ok(Ok(()))
    };
    match run.await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "dynamic recipient count failed");
            // Fail closed: no count, no send.
            Err(Refusal::ChannelCap)
        }
    }
}
