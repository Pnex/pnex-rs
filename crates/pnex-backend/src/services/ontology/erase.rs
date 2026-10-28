//! Erasure as a shell (crm.md D446): the personal data of an object goes,
//! the object stays so the links and provenance of other objects hold.
//! Generic: what goes is what the schema classifies `personal` or
//! `sensitive` (D442). The history of an erased object is read redacted.

use loco_rs::app::AppContext;
use pnex_core::ontology::api::ObjectView;
use pnex_core::ontology::proposal::{
    DecisionKind, ProposalDecision, ProposalKind, ProposalStatus, ProposalView, REASON_MAX,
};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, EntityTrait, QueryFilter, Statement,
    TransactionTrait,
};
use serde_json::Value;
use uuid::Uuid;

use super::changes::{self, Change};
use super::objects::{find, require_role, view};
use super::redact::{short, Redaction};
use super::{schema, Actor, OntologyError, OrgSchema, Result};
use crate::models::_entities::objects;

/// Title of an erased object whose title was personal.
pub fn erased_title(id: &str) -> String {
    format!("∅ {}", short(id))
}

/// The properties of `type_key` an erasure removes.
pub fn personal_keys(s: &OrgSchema, type_key: &str) -> Vec<String> {
    s.object_type(type_key)
        .map(|t| {
            t.properties
                .iter()
                .filter(|p| p.classification.is_personal())
                .map(|p| p.key.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Erases object `id`; an object already erased is returned as is.
/// Objects reached by the hooks of the services that own personal
/// material (profile captures) are erased with it.
pub async fn erase(ctx: &AppContext, org_id: i64, actor: &Actor, id: Uuid) -> Result<ObjectView> {
    let s = schema(&ctx.db, org_id).await?;
    let row = find(&ctx.db, org_id, id).await?;
    let Some(def) = s.object_type(&row.type_key) else {
        return Err(OntologyError::NotFound);
    };
    require_role(def, actor)?;
    if row.erased_at.is_some() {
        return Ok(view(&row, &s));
    }
    // What the services owning personal material keep about the object
    // (captured profile texts, identifiers) goes first: they read it.
    let related = crate::services::profiles::before_erase(ctx, org_id, actor, &row)
        .await
        .map_err(|e| OntologyError::Db(sea_orm::DbErr::Custom(e.to_string())))?;
    for other in related {
        Box::pin(erase(ctx, org_id, actor, other)).await?;
    }
    changes::deferred(async {
        let txn = ctx.db.begin().await?;
        let v = erase_in(&txn, &s, org_id, actor, &row).await?;
        txn.commit().await?;
        changes::flush();
        Ok(v)
    })
    .await
}

/// Decides a proposal; an accepted erasure (D445) erases its object, and
/// only a person may accept one, never an org rule.
pub async fn decide(
    ctx: &AppContext,
    org_id: i64,
    actor: &Actor,
    id: Uuid,
    d: &ProposalDecision,
) -> Result<ProposalView> {
    let p = super::proposals::get(&ctx.db, org_id, id).await?;
    if p.kind != ProposalKind::ObjectErase || d.decision == DecisionKind::Reject {
        return super::proposals::decide(&ctx.db, org_id, actor, id, d).await;
    }
    if actor.user_id.is_none() {
        return Err(OntologyError::WriteForbidden);
    }
    match p.status {
        ProposalStatus::Accepted => return Ok(p),
        ProposalStatus::Proposed => {}
        _ => return Err(OntologyError::ProposalDecided),
    }
    let target = p
        .refs
        .first()
        .and_then(|r| Uuid::parse_str(&r.id).ok())
        .ok_or(OntologyError::NotFound)?;
    let by = Actor {
        source_ref: format!("proposal:{id}"),
        ..actor.clone()
    };
    erase(ctx, org_id, &by, target).await?;
    let reason = d.reason.as_deref().map(str::trim).filter(|r| !r.is_empty());
    super::proposals::mark_accepted(
        &ctx.db,
        org_id,
        actor,
        id,
        &target.to_string(),
        reason.map(|r| r.chars().take(REASON_MAX).collect()),
    )
    .await
}

async fn erase_in<C: ConnectionTrait>(
    db: &C,
    s: &OrgSchema,
    org_id: i64,
    actor: &Actor,
    row: &objects::Model,
) -> Result<ObjectView> {
    let def = s
        .object_type(&row.type_key)
        .ok_or(OntologyError::NotFound)?;
    let title_personal = def.title_classification.is_personal() && !def.native();
    // Its accounts keep a fingerprint of the name (D447).
    if title_personal {
        let keys = super::opposition::title_keys(db, s, org_id, row.id, &row.title).await?;
        super::opposition::record(db, org_id, actor.user_id, &keys).await?;
    }
    let mut props = row.properties.as_object().cloned().unwrap_or_default();
    for k in personal_keys(s, &row.type_key) {
        props.remove(&k);
    }
    let title = match title_personal {
        true => erased_title(&row.id.to_string()),
        false => row.title.clone(),
    };
    let now = sea_orm::prelude::DateTimeWithTimeZone::from(chrono::Utc::now());
    objects::Entity::update_many()
        .col_expr(objects::Column::Title, Expr::value(title))
        .col_expr(
            objects::Column::Properties,
            Expr::value(Value::Object(props)),
        )
        .col_expr(objects::Column::Version, Expr::value(row.version + 1))
        .col_expr(objects::Column::UpdatedAt, Expr::value(now))
        .col_expr(objects::Column::ErasedAt, Expr::value(now))
        .col_expr(
            objects::Column::SourceRef,
            Expr::value(actor.source_ref.clone()),
        )
        .filter(objects::Column::Id.eq(row.id))
        .exec(db)
        .await?;
    // Personal attributes of its links, open or closed.
    for (lt, _, _) in &s.links {
        let keys: Vec<String> = lt
            .attributes
            .iter()
            .filter(|a| a.classification.is_personal())
            .map(|a| a.key.clone())
            .collect();
        if keys.is_empty() {
            continue;
        }
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE resource_edges SET attributes = attributes - $4::text[]
             WHERE org_id = $1 AND relation = $2
               AND (source_object_id = $3 OR target_object_id = $3)",
            vec![
                org_id.into(),
                lt.key.clone().into(),
                row.id.into(),
                keys.into(),
            ],
        ))
        .await?;
    }
    let after = view(&find(db, org_id, row.id).await?, s);
    // The journal never receives what was erased.
    let mut before = view(row, s);
    Redaction::ALL.object(s, &mut before);
    changes::submit(Change::object(
        org_id,
        "object.erase",
        &actor.source_ref,
        serde_json::to_value(&before).unwrap_or_default(),
        &after,
    ));
    Ok(after)
}
