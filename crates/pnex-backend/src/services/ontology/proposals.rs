//! Proposals (proposals.md D371–D377): what a machine proposes to write,
//! waiting for a human. Producers call [`propose`] (imports, flow nodes,
//! detectors); the inbox lists them; a decision accepts (the write goes
//! through the shared object and link services, provenance
//! `proposal:<id>`) or rejects (the fingerprint is never proposed again).

use pnex_core::err_codes::{FIELD_INVALID, FIELD_MAX_LENGTH, FIELD_REQUIRED};
use pnex_core::ontology::api::{LinkInput, ObjectInput};
use pnex_core::ontology::proposal::{
    fingerprint_text, slot, DecisionKind, ProposalDecision, ProposalInput, ProposalKind,
    ProposalStatus, ProposalView, REASON_MAX,
};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, QueryResult, Statement, TransactionTrait,
};
use serde_json::{Map, Value};
use sha2::Digest;
use uuid::Uuid;

use super::{changes, invalid, links, objects, Actor, OntologyError, Result};

const ORIGIN_MAX: usize = 255;
const VALUES_MAX_BYTES: usize = 64 * 1024;

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

const COLUMNS: &str = "p.id, p.kind, p.status, p.type_key, p.target_id, p.source_id, \
     p.link_target_id, p.title, p.vals, p.base_version, p.evidence, p.refs, p.origin, \
     p.confidence, p.reason, p.applied, p.decided_at, p.created_at, u.full_name AS decider";

/// Objects a proposal is about: the object or link ends it targets.
async fn refs_of<C: ConnectionTrait>(db: &C, org_id: i64, p: &ProposalInput) -> Result<Vec<Uuid>> {
    let uuid = |o: &Option<String>| o.as_deref().and_then(|s| Uuid::parse_str(s.trim()).ok());
    Ok(match p.kind {
        Some(ProposalKind::ObjectUpdate | ProposalKind::ObjectErase) => {
            uuid(&p.target_id).into_iter().collect()
        }
        // The case, then the members the detection is about.
        Some(ProposalKind::CaseEvent) => uuid(&p.target_id)
            .into_iter()
            .chain(
                p.values
                    .get("members")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|m| Uuid::parse_str(m.as_str()?).ok()),
            )
            .collect(),
        Some(ProposalKind::LinkCreate) => [uuid(&p.source_id), uuid(&p.link_target_id)]
            .into_iter()
            .flatten()
            .collect(),
        Some(ProposalKind::LinkUpdate | ProposalKind::LinkClose) => {
            let Some(id) = p.target_id.as_deref().and_then(|s| s.parse::<i64>().ok()) else {
                return Ok(Vec::new());
            };
            db.query_one_raw(stmt(
                "SELECT source_object_id, target_object_id FROM resource_edges \
                 WHERE id = $1 AND org_id = $2",
                vec![id.into(), org_id.into()],
            ))
            .await?
            .map(|r| {
                [
                    r.try_get::<Option<Uuid>>("", "source_object_id")
                        .ok()
                        .flatten(),
                    r.try_get::<Option<Uuid>>("", "target_object_id")
                        .ok()
                        .flatten(),
                ]
                .into_iter()
                .flatten()
                .collect()
            })
            .unwrap_or_default()
        }
        _ => Vec::new(),
    })
}

/// Records a proposal; `None` when the same fingerprint was already
/// proposed (open, accepted or rejected: never proposed twice). A new
/// proposal supersedes the open ones of the same slot. The same values
/// proposed again with evidence, while still open, refresh its evidence
/// and confidence (a model's opinion arriving after the first proposal).
pub async fn propose<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    p: &ProposalInput,
) -> Result<Option<Uuid>> {
    let Some(kind) = p.kind else {
        return Err(invalid("kind", FIELD_REQUIRED));
    };
    let needs = |f: &'static str, v: &Option<String>| match v.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => Ok(()),
        _ => Err(invalid(f, FIELD_REQUIRED)),
    };
    match kind {
        ProposalKind::ObjectCreate => {}
        ProposalKind::ObjectUpdate
        | ProposalKind::LinkUpdate
        | ProposalKind::LinkClose
        | ProposalKind::ObjectErase => needs("target_id", &p.target_id)?,
        ProposalKind::VoiceSpan => return Err(invalid("kind", FIELD_INVALID)),
        ProposalKind::CaseEvent => {
            needs("target_id", &p.target_id)?;
            if p.values
                .get("key")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
            {
                return Err(invalid("values.key", FIELD_REQUIRED));
            }
        }
        ProposalKind::LinkCreate => {
            needs("source_id", &p.source_id)?;
            needs("link_target_id", &p.link_target_id)?;
        }
    }
    if p.type_key.trim().is_empty() {
        return Err(invalid("type_key", FIELD_REQUIRED));
    }
    if p.origin.trim().is_empty() || p.origin.len() > ORIGIN_MAX {
        return Err(invalid("origin", FIELD_INVALID));
    }
    if p.confidence.is_some_and(|c| !(0.0..=100.0).contains(&c)) {
        return Err(invalid("confidence", FIELD_INVALID));
    }
    if Value::Object(p.values.clone()).to_string().len() > VALUES_MAX_BYTES {
        return Err(invalid(
            "values",
            format!("{FIELD_MAX_LENGTH}:{VALUES_MAX_BYTES}"),
        ));
    }
    let (Some(slot), Some(text)) = (slot(p), fingerprint_text(p)) else {
        return Err(invalid("kind", FIELD_INVALID));
    };
    let fingerprint = digest(&text);
    let refs = refs_of(db, org_id, p).await?;
    let id = Uuid::new_v4();
    let inserted = db
        .query_one_raw(stmt(
            "INSERT INTO proposals (id, org_id, kind, type_key, target_id, source_id, \
               link_target_id, title, vals, base_version, evidence, refs, origin, confidence, \
               slot, fingerprint) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16) \
             ON CONFLICT (org_id, fingerprint) DO NOTHING RETURNING id",
            vec![
                id.into(),
                org_id.into(),
                kind.as_str().into(),
                p.type_key.trim().into(),
                p.target_id.clone().into(),
                p.source_id.clone().into(),
                p.link_target_id.clone().into(),
                p.title.clone().into(),
                Value::Object(p.values.clone()).into(),
                p.base_version.into(),
                p.evidence.clone().into(),
                refs.into(),
                p.origin.trim().into(),
                p.confidence.into(),
                slot.clone().into(),
                fingerprint.clone().into(),
            ],
        ))
        .await?;
    if inserted.is_none() {
        if !p.evidence.is_null() {
            db.execute_raw(stmt(
                "UPDATE proposals SET evidence = $3, confidence = coalesce($4, confidence) \
                 WHERE org_id = $1 AND fingerprint = $2 AND status = 'proposed'",
                vec![
                    org_id.into(),
                    fingerprint.into(),
                    p.evidence.clone().into(),
                    p.confidence.into(),
                ],
            ))
            .await?;
        }
        return Ok(None);
    }
    db.execute_raw(stmt(
        "UPDATE proposals SET status = 'superseded' \
         WHERE org_id = $1 AND slot = $2 AND status = 'proposed' AND id <> $3",
        vec![org_id.into(), slot.into(), id.into()],
    ))
    .await?;
    Ok(Some(id))
}

fn digest(text: &str) -> String {
    format!("{:x}", sha2::Sha256::digest(text.as_bytes()))
}

/// Proposes, then accepts under the org's rule `rule` (D377): no user
/// decides, the rule is the reason. A proposal already open with the same
/// values is the one accepted; one decided before keeps its decision. A
/// refused write (cardinality, validation) leaves it open for a human.
pub async fn propose_accepted(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    p: &ProposalInput,
    rule: &str,
) -> Result<Option<Uuid>> {
    let id = match propose(db, org_id, p).await? {
        Some(id) => id,
        None => {
            let Some(text) = fingerprint_text(p) else {
                return Ok(None);
            };
            let open = db
                .query_one_raw(stmt(
                    "SELECT id FROM proposals \
                     WHERE org_id = $1 AND fingerprint = $2 AND status = 'proposed'",
                    vec![org_id.into(), digest(&text).into()],
                ))
                .await?
                .and_then(|r| r.try_get::<Uuid>("", "id").ok());
            match open {
                Some(id) => id,
                None => return Ok(None),
            }
        }
    };
    let system = Actor {
        user_id: None,
        ..actor.clone()
    };
    let accept = ProposalDecision {
        decision: DecisionKind::Accept,
        reason: Some(rule.to_string()),
        values: None,
    };
    if let Err(e) = decide(db, org_id, &system, id, &accept).await {
        tracing::warn!(error = %e, %id, "proposal auto-accept refused, left for a human");
    }
    Ok(Some(id))
}

/// What the inbox reads: the proposals, and the voices recognized on media
/// (P3, read only: decided on the media review, where they are heard).
const INBOX: &str = "(SELECT id, org_id, kind, status, type_key, target_id, source_id, \
       link_target_id, title, vals, base_version, evidence, refs, origin, confidence, reason, \
       applied, decided_by, decided_at, created_at FROM proposals \
     UNION ALL \
     SELECT s.id, s.org_id, 'voice_span', \
       CASE s.status WHEN 'confirmed' THEN 'accepted' ELSE s.status END, 'person', \
       s.media_asset_id::text, NULL, NULL, m.name, \
       jsonb_build_object('media_asset_id', s.media_asset_id, 'start_ms', s.start_ms, \
         'end_ms', s.end_ms), NULL, 'null'::jsonb, \
       array_remove(ARRAY[s.object_id, m.object_id], NULL), 'voice', \
       (s.confidence * 100)::float8, NULL, NULL, NULL, NULL, s.created_at \
     FROM speech_attributions s JOIN media_assets m ON m.id = s.media_asset_id \
     WHERE s.origin = 'voice')";

/// Inbox filter.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub status: Option<ProposalStatus>,
    /// Origin prefix (`import:`, `flow:12@`).
    pub origin: Option<String>,
    /// Proposals about this object.
    pub object: Option<Uuid>,
}

async fn views<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    rows: Vec<QueryResult>,
) -> Result<Vec<ProposalView>> {
    let all_refs: Vec<Uuid> = rows
        .iter()
        .flat_map(|r| r.try_get::<Vec<Uuid>>("", "refs").unwrap_or_default())
        .collect();
    let objs = links::refs(db, org_id, all_refs).await?;
    let text = |r: &QueryResult, c: &str| r.try_get::<Option<String>>("", c).ok().flatten();
    Ok(rows
        .iter()
        .filter_map(|r| {
            let refs: Vec<Uuid> = r.try_get("", "refs").unwrap_or_default();
            let at = |c: &str| {
                r.try_get::<Option<sea_orm::prelude::DateTimeWithTimeZone>>("", c)
                    .ok()
                    .flatten()
                    .map(|t| t.to_rfc3339())
            };
            Some(ProposalView {
                id: r.try_get::<Uuid>("", "id").ok()?.to_string(),
                kind: ProposalKind::parse(&text(r, "kind")?)?,
                status: ProposalStatus::parse(&text(r, "status")?)?,
                type_key: text(r, "type_key")?,
                refs: refs
                    .iter()
                    .filter_map(|id| objs.get(id).map(links::object_ref))
                    .collect(),
                title: text(r, "title"),
                values: r
                    .try_get::<Value>("", "vals")
                    .ok()?
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
                evidence: r.try_get("", "evidence").unwrap_or(Value::Null),
                origin: text(r, "origin")?,
                confidence: r.try_get::<Option<f64>>("", "confidence").ok().flatten(),
                decided_by: text(r, "decider"),
                decided_at: at("decided_at"),
                reason: text(r, "reason"),
                applied: text(r, "applied"),
                created_at: at("created_at")?,
            })
        })
        .collect())
}

/// One page of the inbox, newest first, and the count.
pub async fn list(
    db: &DatabaseConnection,
    org_id: i64,
    f: &Filter,
    limit: u64,
    offset: u64,
) -> Result<(i64, Vec<ProposalView>)> {
    let status = f.status.unwrap_or_default().as_str();
    let origin = f
        .origin
        .as_deref()
        .map(|o| format!("{}%", o.replace('%', "\\%").replace('_', "\\_")));
    let filter = "p.org_id = $1 AND p.status = $2 \
         AND ($3::text IS NULL OR p.origin LIKE $3) \
         AND ($4::uuid IS NULL OR $4 = ANY(p.refs))";
    let args = |extra: Vec<sea_orm::Value>| {
        let mut v: Vec<sea_orm::Value> = vec![
            org_id.into(),
            status.into(),
            origin.clone().into(),
            f.object.into(),
        ];
        v.extend(extra);
        v
    };
    let count = db
        .query_one_raw(stmt(
            &format!("SELECT count(*)::bigint AS n FROM {INBOX} p WHERE {filter}"),
            args(vec![]),
        ))
        .await?
        .and_then(|r| r.try_get::<i64>("", "n").ok())
        .unwrap_or(0);
    let rows = db
        .query_all_raw(stmt(
            &format!(
                "SELECT {COLUMNS} FROM {INBOX} p LEFT JOIN users u ON u.id = p.decided_by \
                 WHERE {filter} ORDER BY p.created_at DESC, p.id LIMIT $5 OFFSET $6"
            ),
            args(vec![(limit as i64).into(), (offset as i64).into()]),
        ))
        .await?;
    Ok((count, views(db, org_id, rows).await?))
}

pub async fn get<C: ConnectionTrait>(db: &C, org_id: i64, id: Uuid) -> Result<ProposalView> {
    let rows = db
        .query_all_raw(stmt(
            &format!(
                "SELECT {COLUMNS} FROM proposals p LEFT JOIN users u ON u.id = p.decided_by \
                 WHERE p.id = $1 AND p.org_id = $2"
            ),
            vec![id.into(), org_id.into()],
        ))
        .await?;
    views(db, org_id, rows)
        .await?
        .pop()
        .ok_or(OntologyError::NotFound)
}

/// Writes what an accepted proposal says, through the shared services.
async fn apply<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    actor: &Actor,
    p: &ProposalView,
    raw: &QueryResult,
    values: &Map<String, Value>,
) -> Result<String> {
    let text = |c: &str| raw.try_get::<Option<String>>("", c).ok().flatten();
    let link_id = || {
        text("target_id")
            .and_then(|t| t.parse::<i64>().ok())
            .ok_or_else(|| invalid("target_id", FIELD_INVALID))
    };
    Ok(match p.kind {
        ProposalKind::ObjectCreate => {
            let input = ObjectInput {
                type_key: p.type_key.clone(),
                title: p.title.clone().unwrap_or_default(),
                properties: values.clone(),
                expected_version: None,
            };
            objects::create(db, org_id, actor, &input).await?.id
        }
        ProposalKind::ObjectUpdate => {
            let id = text("target_id")
                .and_then(|t| Uuid::parse_str(&t).ok())
                .ok_or_else(|| invalid("target_id", FIELD_INVALID))?;
            let row = objects::find(db, org_id, id).await?;
            let mut props = row.properties.as_object().cloned().unwrap_or_default();
            props.extend(values.clone());
            let input = ObjectInput {
                type_key: row.type_key.clone(),
                title: p.title.clone().unwrap_or(row.title.clone()),
                properties: props,
                // The object moved since it was read: refused (409).
                expected_version: raw
                    .try_get::<Option<i32>>("", "base_version")
                    .ok()
                    .flatten(),
            };
            objects::update_in(db, org_id, actor, id, &input)
                .await?
                .0
                .id
        }
        ProposalKind::LinkCreate => {
            let input = LinkInput {
                link_type: p.type_key.clone(),
                source_id: text("source_id").unwrap_or_default(),
                target_id: text("link_target_id").unwrap_or_default(),
                attributes: values.clone(),
                ..Default::default()
            };
            links::create(db, org_id, actor, &input)
                .await?
                .id
                .to_string()
        }
        ProposalKind::LinkUpdate => {
            let id = link_id()?;
            let current = db
                .query_one_raw(stmt(
                    "SELECT attributes FROM resource_edges WHERE id = $1 AND org_id = $2",
                    vec![id.into(), org_id.into()],
                ))
                .await?
                .and_then(|r| r.try_get::<Value>("", "attributes").ok())
                .ok_or(OntologyError::NotFound)?;
            let mut attrs = current.as_object().cloned().unwrap_or_default();
            attrs.extend(values.clone());
            links::update(db, org_id, actor, id, &attrs)
                .await?
                .id
                .to_string()
        }
        ProposalKind::LinkClose => links::close(db, org_id, actor, link_id()?, None)
            .await?
            .id
            .to_string(),
        // Decided by `erase::decide` with a human (D445): an org rule
        // accepting it lands here and leaves it to validate.
        ProposalKind::VoiceSpan | ProposalKind::ObjectErase => {
            return Err(invalid("kind", FIELD_INVALID))
        }
        ProposalKind::CaseEvent => {
            let case_id = text("target_id")
                .and_then(|t| Uuid::parse_str(&t).ok())
                .ok_or_else(|| invalid("target_id", FIELD_INVALID))?;
            // The detection stays as proposed; only its title is chosen.
            let title = values
                .get("title")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .or_else(|| p.title.clone())
                .unwrap_or_else(|| p.type_key.clone());
            super::cases::detect::event_of(db, org_id, actor, case_id, &p.values, &title)
                .await?
                .to_string()
        }
    })
}

/// Marks an open proposal accepted once its write was made elsewhere
/// (`erase::decide`); refused when decided or superseded meanwhile.
pub async fn mark_accepted(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    id: Uuid,
    applied: &str,
    reason: Option<String>,
) -> Result<ProposalView> {
    let done = db
        .execute_raw(stmt(
            "UPDATE proposals SET status = 'accepted', applied = $1, reason = $2, \
               decided_by = $3, decided_at = now() \
             WHERE id = $4 AND org_id = $5 AND status = 'proposed'",
            vec![
                applied.into(),
                reason.into(),
                actor.user_id.into(),
                id.into(),
                org_id.into(),
            ],
        ))
        .await?;
    let view = get(db, org_id, id).await?;
    if done.rows_affected() == 0 && view.status != ProposalStatus::Accepted {
        return Err(OntologyError::ProposalDecided);
    }
    Ok(view)
}

/// Accepts or rejects. Idempotent for the same decision; the other one on a
/// decided proposal is refused (`proposal-decided`), as is a superseded one.
pub async fn decide(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    id: Uuid,
    d: &ProposalDecision,
) -> Result<ProposalView> {
    let reason: Option<String> = d
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(|r| r.chars().take(REASON_MAX).collect());
    changes::deferred(async {
        let txn = db.begin().await?;
        let raw = txn
            .query_one_raw(stmt(
                "SELECT target_id, source_id, link_target_id, base_version, status \
                 FROM proposals WHERE id = $1 AND org_id = $2 FOR UPDATE",
                vec![id.into(), org_id.into()],
            ))
            .await?
            .ok_or(OntologyError::NotFound)?;
        let p = get(&txn, org_id, id).await?;
        let wanted = match d.decision {
            DecisionKind::Accept => ProposalStatus::Accepted,
            DecisionKind::Reject => ProposalStatus::Rejected,
        };
        if p.status == wanted {
            txn.commit().await?;
            return Ok(p);
        }
        if p.status != ProposalStatus::Proposed {
            return Err(OntologyError::ProposalDecided);
        }
        match d.decision {
            DecisionKind::Accept => {
                let values = d.values.clone().unwrap_or_else(|| p.values.clone());
                let by = Actor {
                    source_ref: format!("proposal:{id}"),
                    ..actor.clone()
                };
                let applied = apply(&txn, org_id, &by, &p, &raw, &values).await?;
                let correction = d.values.clone().map(Value::Object);
                txn.execute_raw(stmt(
                    "UPDATE proposals SET status = 'accepted', applied = $1, correction = $2, \
                       reason = $3, decided_by = $4, decided_at = now() WHERE id = $5",
                    vec![
                        applied.into(),
                        correction.into(),
                        reason.into(),
                        actor.user_id.into(),
                        id.into(),
                    ],
                ))
                .await?;
            }
            DecisionKind::Reject => {
                txn.execute_raw(stmt(
                    "UPDATE proposals SET status = 'rejected', reason = $1, decided_by = $2, \
                       decided_at = now() WHERE id = $3",
                    vec![reason.into(), actor.user_id.into(), id.into()],
                ))
                .await?;
            }
        }
        let view = get(&txn, org_id, id).await?;
        txn.commit().await?;
        changes::flush();
        Ok(view)
    })
    .await
}
