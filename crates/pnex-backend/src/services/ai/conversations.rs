//! Assistant conversations (D145, ai-assistant.md §9.4).
//!
//! A conversation belongs to one user **within one org**: every query is
//! filtered on both, and a conversation of anyone else (same org included,
//! org owner and platform admin included) is reported as absent (404, its
//! existence is never revealed). The client only sends the new message;
//! the history is rebuilt here, so a forged history (fake tool results,
//! fake assistant turns) cannot reach the model.
//!
//! Content lives in the relational database (per-record erasure, cascades
//! on user/org deletion). OpenObserve only gets a content-free audit event
//! per turn ([`audit`]).

use std::time::Duration;

use chrono::{DateTime, FixedOffset, Utc};
use loco_rs::app::AppContext;
use pnex_core::{AiConversation, AiConversationDetail, AiMessage, AiToolTrace};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, DatabaseConnection, DbErr, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect, Set, TransactionTrait,
};
use uuid::Uuid;

use super::provider::Msg;
use super::tools::ToolTrace;
use crate::models::_entities::{ai_conversations, ai_messages, system_settings};

/// Title length taken from the first message (no LLM call).
const TITLE_MAX_CHARS: usize = 60;
/// Stored messages replayed to the model (most recent ones).
const HISTORY_MESSAGES: u64 = 24;
/// Tool entries kept per stored turn.
const TRACE_MAX_ENTRIES: usize = 16;
/// Serialized tool arguments kept per entry.
const TRACE_ARGS_MAX_CHARS: usize = 2_000;
/// Summary kept per entry.
const TRACE_SUMMARY_MAX_CHARS: usize = 500;
/// Message length accepted from the user.
pub const CONTENT_MAX_CHARS: usize = 20_000;
/// One-turn lease: longer than the agent's worst case (provider timeout
/// × bounded iterations is cut by the agent before that).
const BUSY_LEASE: Duration = Duration::from_secs(300);

/// `system_settings` key of the platform retention of inactive
/// conversations, in days.
pub const RETENTION_KEY: &str = "ai_conversation_retention_days";
/// Retention when the platform sets nothing.
pub const DEFAULT_RETENTION_DAYS: u32 = 180;
/// Interval of the purge loop.
const PURGE_EVERY: Duration = Duration::from_secs(3600);

/// O2 stream of the content-free audit events.
pub const AUDIT_STREAM: &str = "ai_audit";

fn now() -> DateTime<FixedOffset> {
    Utc::now().fixed_offset()
}

fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((cut, _)) => format!("{}…", &s[..cut]),
        None => s.to_string(),
    }
}

/// Title from the first message: first line, trimmed, bounded.
pub fn title_of(content: &str) -> String {
    let line = content
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let title = truncate_chars(line, TITLE_MAX_CHARS);
    if title.is_empty() {
        "…".to_string()
    } else {
        title
    }
}

pub fn dto(m: &ai_conversations::Model) -> AiConversation {
    AiConversation {
        id: m.id,
        title: m.title.clone(),
        created_at: m.created_at.to_rfc3339(),
        last_message_at: m.last_message_at.to_rfc3339(),
    }
}

fn message_dto(m: ai_messages::Model) -> AiMessage {
    let tool_trace = m
        .tool_trace
        .and_then(|v| serde_json::from_value::<Vec<AiToolTrace>>(v).ok())
        .unwrap_or_default();
    AiMessage {
        seq: m.seq,
        role: m.role,
        content: m.content,
        tool_trace,
        created_at: m.created_at.to_rfc3339(),
    }
}

/// Tool trace → bounded wire/storage form.
pub fn trace_dto(t: &ToolTrace) -> AiToolTrace {
    let args_text = t.arguments.to_string();
    let arguments = if args_text.chars().count() > TRACE_ARGS_MAX_CHARS {
        serde_json::Value::String(truncate_chars(&args_text, TRACE_ARGS_MAX_CHARS))
    } else {
        t.arguments.clone()
    };
    AiToolTrace {
        name: t.name.clone(),
        arguments,
        ok: t.ok,
        summary: truncate_chars(&t.summary, TRACE_SUMMARY_MAX_CHARS),
        flow_id: t.flow_id,
        summary_key: t.summary_key.map(str::to_string),
        code: t.code.map(str::to_string),
        args: t.args.clone(),
    }
}

/// Conversations of `user` in `org`, most recent first.
pub fn owned(user_id: i64, org_id: i64) -> sea_orm::Select<ai_conversations::Entity> {
    ai_conversations::Entity::find()
        .filter(ai_conversations::Column::UserId.eq(user_id))
        .filter(ai_conversations::Column::OrgId.eq(org_id))
        .order_by_desc(ai_conversations::Column::LastMessageAt)
}

/// One conversation of `user` in `org`; `None` for anyone else's.
pub async fn find_owned(
    db: &DatabaseConnection,
    user_id: i64,
    org_id: i64,
    id: Uuid,
) -> Result<Option<ai_conversations::Model>, DbErr> {
    owned(user_id, org_id)
        .filter(ai_conversations::Column::Id.eq(id))
        .one(db)
        .await
}

pub async fn create(
    db: &DatabaseConnection,
    user_id: i64,
    org_id: i64,
    title: Option<&str>,
) -> Result<ai_conversations::Model, DbErr> {
    let at = now();
    ai_conversations::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        user_id: Set(user_id),
        title: Set(title.map(title_of).unwrap_or_default()),
        busy_until: Set(None),
        last_message_at: Set(at),
        created_at: Set(at),
        updated_at: Set(at),
    }
    .insert(db)
    .await
}

pub async fn rename(
    db: &DatabaseConnection,
    conv: ai_conversations::Model,
    title: &str,
) -> Result<ai_conversations::Model, DbErr> {
    let mut active: ai_conversations::ActiveModel = conv.into();
    active.title = Set(title_of(title));
    active.updated_at = Set(now());
    active.update(db).await
}

/// Erases one conversation (its messages cascade).
pub async fn delete(db: &DatabaseConnection, id: Uuid) -> Result<(), DbErr> {
    ai_conversations::Entity::delete_by_id(id).exec(db).await?;
    Ok(())
}

/// Erases every conversation of `user` in `org`; returns how many.
pub async fn delete_all<C: sea_orm::ConnectionTrait>(
    db: &C,
    user_id: i64,
    org_id: i64,
) -> Result<u64, DbErr> {
    let res = ai_conversations::Entity::delete_many()
        .filter(ai_conversations::Column::UserId.eq(user_id))
        .filter(ai_conversations::Column::OrgId.eq(org_id))
        .exec(db)
        .await?;
    Ok(res.rows_affected)
}

pub async fn messages(db: &DatabaseConnection, id: Uuid) -> Result<Vec<AiMessage>, DbErr> {
    Ok(ai_messages::Entity::find()
        .filter(ai_messages::Column::ConversationId.eq(id))
        .order_by_asc(ai_messages::Column::Seq)
        .all(db)
        .await?
        .into_iter()
        .map(message_dto)
        .collect())
}

pub async fn detail(
    db: &DatabaseConnection,
    conv: &ai_conversations::Model,
) -> Result<AiConversationDetail, DbErr> {
    Ok(AiConversationDetail {
        conversation: dto(conv),
        messages: messages(db, conv.id).await?,
    })
}

/// Takes the one-turn lease; `false` when a turn is already running.
/// Atomic on both engines (conditional update), valid across replicas.
pub async fn acquire(db: &DatabaseConnection, id: Uuid) -> Result<bool, DbErr> {
    let at = now();
    let until = at + chrono::Duration::from_std(BUSY_LEASE).expect("lease fits");
    let res = ai_conversations::Entity::update_many()
        .col_expr(
            ai_conversations::Column::BusyUntil,
            Expr::value(Some(until)),
        )
        .filter(ai_conversations::Column::Id.eq(id))
        .filter(
            Condition::any()
                .add(ai_conversations::Column::BusyUntil.is_null())
                .add(ai_conversations::Column::BusyUntil.lt(at)),
        )
        .exec(db)
        .await?;
    Ok(res.rows_affected == 1)
}

/// Releases the lease (best effort: an expired lease frees itself).
pub async fn release(db: &DatabaseConnection, id: Uuid) {
    let res = ai_conversations::Entity::update_many()
        .col_expr(
            ai_conversations::Column::BusyUntil,
            Expr::value(Option::<DateTime<FixedOffset>>::None),
        )
        .filter(ai_conversations::Column::Id.eq(id))
        .exec(db)
        .await;
    if let Err(e) = res {
        tracing::warn!(conversation = %id, "assistant lease release failed: {e}");
    }
}

/// Model history rebuilt from storage: the last stored messages, as plain
/// user/assistant text. Older tool calls are not replayed: an assistant
/// turn carries the one-line summaries of the tools it ran, which keeps
/// the prompt bounded and provider-neutral.
pub async fn history(db: &DatabaseConnection, id: Uuid) -> Result<Vec<Msg>, DbErr> {
    let mut rows = ai_messages::Entity::find()
        .filter(ai_messages::Column::ConversationId.eq(id))
        .order_by_desc(ai_messages::Column::Seq)
        .limit(HISTORY_MESSAGES)
        .all(db)
        .await?;
    rows.reverse();
    // Providers require the history to open on a user message.
    let start = rows
        .iter()
        .position(|m| m.role == "user")
        .unwrap_or(rows.len());
    Ok(rows
        .into_iter()
        .skip(start)
        .filter_map(|m| match m.role.as_str() {
            "user" => Some(Msg::User { text: m.content }),
            "assistant" => {
                let traces = m
                    .tool_trace
                    .and_then(|v| serde_json::from_value::<Vec<AiToolTrace>>(v).ok())
                    .unwrap_or_default();
                let text = if traces.is_empty() {
                    m.content
                } else {
                    let tools: Vec<String> = traces
                        .iter()
                        .map(|t| {
                            let state = if t.ok { "ok" } else { "error" };
                            format!("{} {state}: {}", t.name, t.summary)
                        })
                        .collect();
                    format!(
                        "{}\n\n[tools run in that turn: {}]",
                        m.content,
                        tools.join("; ")
                    )
                };
                Some(Msg::Assistant {
                    text: Some(text),
                    tool_calls: vec![],
                })
            }
            _ => None,
        })
        .collect())
}

/// What one finished turn stores.
pub struct TurnRecord<'a> {
    pub user_text: &'a str,
    pub page: Option<&'a str>,
    pub answer: &'a str,
    pub trace: &'a [AiToolTrace],
    pub tokens_in: u64,
    pub tokens_out: u64,
}

/// Stores the user message and the answer of one turn; the first message
/// titles an untitled conversation.
pub async fn append_turn(
    db: &DatabaseConnection,
    conv: &ai_conversations::Model,
    turn: TurnRecord<'_>,
) -> Result<ai_conversations::Model, DbErr> {
    let txn = db.begin().await?;
    let last_seq = ai_messages::Entity::find()
        .filter(ai_messages::Column::ConversationId.eq(conv.id))
        .order_by_desc(ai_messages::Column::Seq)
        .one(&txn)
        .await?
        .map_or(0, |m| m.seq);
    let at = now();
    let page = turn.page.map(|p| truncate_chars(p, 200));
    let trace: Vec<AiToolTrace> = turn.trace.iter().take(TRACE_MAX_ENTRIES).cloned().collect();
    ai_messages::ActiveModel {
        id: Set(Uuid::new_v4()),
        conversation_id: Set(conv.id),
        seq: Set(last_seq + 1),
        role: Set("user".into()),
        content: Set(turn.user_text.to_string()),
        tool_trace: Set(None),
        page: Set(page.clone()),
        tokens_in: Set(None),
        tokens_out: Set(None),
        created_at: Set(at),
    }
    .insert(&txn)
    .await?;
    ai_messages::ActiveModel {
        id: Set(Uuid::new_v4()),
        conversation_id: Set(conv.id),
        seq: Set(last_seq + 2),
        role: Set("assistant".into()),
        content: Set(turn.answer.to_string()),
        tool_trace: Set((!trace.is_empty()).then(|| serde_json::json!(trace))),
        page: Set(page),
        tokens_in: Set(Some(i32::try_from(turn.tokens_in).unwrap_or(i32::MAX))),
        tokens_out: Set(Some(i32::try_from(turn.tokens_out).unwrap_or(i32::MAX))),
        created_at: Set(at),
    }
    .insert(&txn)
    .await?;
    let mut active: ai_conversations::ActiveModel = conv.clone().into();
    if conv.title.trim().is_empty() {
        active.title = Set(title_of(turn.user_text));
    }
    active.last_message_at = Set(at);
    active.updated_at = Set(at);
    let updated = active.update(&txn).await?;
    txn.commit().await?;
    Ok(updated)
}

// ─────────────────────────── Retention ───────────────────────────

/// Platform retention of inactive conversations (days, ≥ 1).
pub async fn retention_days(db: &DatabaseConnection) -> u32 {
    platform_retention(db)
        .await
        .unwrap_or(DEFAULT_RETENTION_DAYS)
        .max(1)
}

/// Platform value stored in `system_settings` (`None` = default).
pub async fn platform_retention(db: &DatabaseConnection) -> Option<u32> {
    system_settings::Entity::find()
        .filter(system_settings::Column::Key.eq(RETENTION_KEY))
        .one(db)
        .await
        .ok()
        .flatten()
        .and_then(|r| r.value.trim().parse::<u32>().ok())
}

/// Sets (`Some`) or clears (`None`) the platform value.
pub async fn set_platform_retention(
    db: &DatabaseConnection,
    days: Option<u32>,
    user_id: i64,
) -> Result<(), DbErr> {
    let existing = system_settings::Entity::find()
        .filter(system_settings::Column::Key.eq(RETENTION_KEY))
        .one(db)
        .await?;
    match (existing, days) {
        (Some(row), None) => {
            system_settings::Entity::delete_by_id(row.id)
                .exec(db)
                .await?;
        }
        (Some(row), Some(days)) => {
            let mut active: system_settings::ActiveModel = row.into();
            active.value = Set(days.to_string());
            active.updated_by = Set(Some(user_id));
            active.update(db).await?;
        }
        (None, Some(days)) => {
            system_settings::ActiveModel {
                key: Set(RETENTION_KEY.to_string()),
                value: Set(days.to_string()),
                updated_by: Set(Some(user_id)),
                ..Default::default()
            }
            .insert(db)
            .await?;
        }
        (None, None) => {}
    }
    Ok(())
}

/// Effective retention of an org: its own value may only shorten the
/// platform one.
pub fn effective_retention(platform_days: u32, org_days: Option<i32>) -> u32 {
    let org = org_days
        .and_then(|d| u32::try_from(d).ok())
        .filter(|d| *d >= 1);
    org.map_or(platform_days, |d| d.min(platform_days)).max(1)
}

/// Erases conversations without a message for `days`, in one org or in
/// every org (`None`); returns how many.
pub async fn purge_inactive_in(
    db: &DatabaseConnection,
    org_id: Option<i64>,
    days: u32,
) -> Result<u64, DbErr> {
    let cutoff = now() - chrono::Duration::days(i64::from(days));
    let mut q = ai_conversations::Entity::delete_many()
        .filter(ai_conversations::Column::LastMessageAt.lt(cutoff));
    if let Some(org) = org_id {
        q = q.filter(ai_conversations::Column::OrgId.eq(org));
    }
    Ok(q.exec(db).await?.rows_affected)
}

/// Erases conversations without a message for `days` (every org).
pub async fn purge_inactive(db: &DatabaseConnection, days: u32) -> Result<u64, DbErr> {
    purge_inactive_in(db, None, days).await
}

/// One purge pass: the platform retention everywhere, then the shorter
/// retention of the orgs that set one.
pub async fn purge_pass(db: &DatabaseConnection) -> Result<u64, DbErr> {
    use crate::models::_entities::organizations;
    let platform = retention_days(db).await;
    let mut erased = purge_inactive_in(db, None, platform).await?;
    let shorter = organizations::Entity::find()
        .filter(organizations::Column::AiRetentionDays.is_not_null())
        .all(db)
        .await?;
    for org in shorter {
        let days = effective_retention(platform, org.ai_retention_days);
        if days < platform {
            erased += purge_inactive_in(db, Some(org.id), days).await?;
        }
    }
    Ok(erased)
}

/// Hourly purge loop, one pod at a time (D106).
pub fn spawn_purger(ctx: &AppContext) {
    let ctx = ctx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(60)).await;
        loop {
            if crate::services::singleton::my_turn(&ctx.db, "ai-conversation-purge", PURGE_EVERY)
                .await
            {
                match purge_pass(&ctx.db).await {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(erased = n, "inactive assistant conversations erased"),
                    Err(e) => tracing::warn!("assistant conversation purge failed: {e}"),
                }
            }
            tokio::time::sleep(PURGE_EVERY).await;
        }
    });
}

// ─────────────────────────── Audit ───────────────────────────

/// Content-free audit event of one turn: who, which provider, how many
/// tokens, which tools with which outcome, which flows were written.
/// Never the messages nor the tool arguments/results.
pub struct AuditEvent {
    pub org_id: i64,
    pub user_id: i64,
    pub conversation_id: Uuid,
    pub provider: &'static str,
    pub model: String,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub tools: Vec<(String, bool)>,
    pub written_flow_ids: Vec<i64>,
    pub latency_ms: u64,
    /// `ok` or the machine error of a failed turn.
    pub outcome: String,
}

pub fn audit_document(e: &AuditEvent, ts_us: i64) -> serde_json::Value {
    let tools: Vec<String> = e
        .tools
        .iter()
        .map(|(name, ok)| format!("{name}:{}", if *ok { "ok" } else { "error" }))
        .collect();
    serde_json::json!({
        "_timestamp": ts_us,
        "org_id": e.org_id,
        "user_id": e.user_id,
        "conversation_id": e.conversation_id.to_string(),
        "provider": e.provider,
        "model": e.model,
        "tokens_in": e.tokens_in,
        "tokens_out": e.tokens_out,
        "tools": tools.join(","),
        "tool_count": e.tools.len(),
        "written_flow_ids": e
            .written_flow_ids
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(","),
        "latency_ms": e.latency_ms,
        "outcome": e.outcome,
    })
}

/// Writes the audit event in the background (best effort, never blocks
/// nor fails the turn; no-op without OpenObserve).
pub fn audit(ctx: &AppContext, event: AuditEvent) {
    let Some(settings) =
        crate::services::openobserve::OpenobserveSettings::from_config(&ctx.config)
    else {
        return;
    };
    let db = ctx.db.clone();
    tokio::spawn(async move {
        let client = crate::services::openobserve::Client::new(&settings);
        let result = async {
            let creds =
                crate::services::openobserve::ensure_org_credentials(&db, &client, event.org_id)
                    .await?;
            let doc = audit_document(&event, Utc::now().timestamp_micros());
            client
                .ingest_json(&creds.o2_org, AUDIT_STREAM, &[doc], &creds.email_passcode)
                .await
        }
        .await;
        if let Err(e) = result {
            tracing::warn!(org = event.org_id, "assistant audit write failed: {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_is_the_first_non_empty_line_bounded() {
        assert_eq!(title_of("\n  hello there \nsecond"), "hello there");
        let long = "x".repeat(100);
        assert_eq!(title_of(&long).chars().count(), TITLE_MAX_CHARS + 1);
        assert_eq!(title_of("   "), "…");
    }

    #[test]
    fn org_retention_only_shortens_the_platform_one() {
        assert_eq!(effective_retention(180, None), 180);
        assert_eq!(effective_retention(180, Some(30)), 30);
        assert_eq!(effective_retention(180, Some(400)), 180);
        assert_eq!(effective_retention(180, Some(0)), 180);
    }

    #[test]
    fn audit_document_carries_no_content() {
        let doc = audit_document(
            &AuditEvent {
                org_id: 1,
                user_id: 2,
                conversation_id: Uuid::nil(),
                provider: "anthropic",
                model: "m".into(),
                tokens_in: 10,
                tokens_out: 5,
                tools: vec![("update_flow".into(), false)],
                written_flow_ids: vec![7],
                latency_ms: 12,
                outcome: "ok".into(),
            },
            1,
        );
        let keys: Vec<&str> = doc
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        for forbidden in ["content", "message", "arguments", "answer", "summary"] {
            assert!(!keys.contains(&forbidden), "audit carries {forbidden}");
        }
        assert_eq!(doc["tools"], "update_flow:error");
        assert_eq!(doc["written_flow_ids"], "7");
    }

    #[test]
    fn long_tool_arguments_are_truncated() {
        let t = ToolTrace {
            name: "update_flow".into(),
            arguments: serde_json::json!({"graph": "y".repeat(5_000)}),
            ok: true,
            summary: "s".repeat(1_000),
            summary_key: None,
            flow_id: Some(1),
            code: None,
            args: None,
        };
        let d = trace_dto(&t);
        assert!(d.arguments.is_string());
        assert!(d.summary.chars().count() <= TRACE_SUMMARY_MAX_CHARS + 1);
    }
}
