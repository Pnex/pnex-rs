//! Erasure as a shell (crm.md D446): the personal data of an object goes,
//! the object stays so the links and provenance of other objects hold.
//! Generic: what goes is what the schema classifies `personal` or
//! `sensitive` (D442). The history of an erased object is read redacted.

use loco_rs::app::AppContext;
use pnex_core::ontology::api::ObjectView;
use pnex_core::ontology::proposal::{
    DecisionKind, ProposalDecision, ProposalInput, ProposalKind, ProposalStatus, ProposalView,
    REASON_MAX,
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
    // Before the object itself: a refusal here leaves it erasable again.
    if def.title_classification.is_personal() {
        for other in sources_to_erase(&ctx.db, &s, org_id, id).await? {
            Box::pin(erase(ctx, org_id, actor, other)).await?;
        }
    }
    let v = changes::deferred(async {
        let txn = ctx.db.begin().await?;
        let v = erase_in(&txn, &s, org_id, actor, &row).await?;
        txn.commit().await?;
        changes::flush();
        Ok::<_, OntologyError>(v)
    })
    .await?;
    propose_other_holders(&ctx.db, &s, org_id, &row).await;
    if def.title_classification.is_personal() {
        propose_shared_targets(&ctx.db, &s, org_id, row.id).await;
    }
    Ok(v)
}

/// The sources of the `erase_sources` links pointing at `id`, open or closed.
async fn sources_to_erase<C: ConnectionTrait>(
    db: &C,
    s: &OrgSchema,
    org_id: i64,
    id: Uuid,
) -> Result<Vec<Uuid>> {
    let relations: Vec<String> = s
        .links
        .iter()
        .filter(|(l, _, _)| l.erase_sources)
        .map(|(l, _, _)| l.key.clone())
        .collect();
    if relations.is_empty() {
        return Ok(Vec::new());
    }
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT DISTINCT source_object_id AS id FROM resource_edges
             WHERE org_id = $1 AND target_object_id = $2 AND relation = ANY($3)
               AND source_object_id IS NOT NULL",
            vec![org_id.into(), id.into(), relations.into()],
        ))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| r.try_get("", "id").ok())
        .collect())
}

/// Other objects holding a contact point of the erased `row` become erasure
/// proposals (D610). Shan's call, 2026-11-04: proposed, not erased at once, because an
/// address may be shared (contact@acme.test) and an erasure cannot be undone;
/// until a person accepts, those objects keep the value. Best effort: the
/// erasure itself is done and must not fail on this.
async fn propose_other_holders<C: ConnectionTrait>(
    db: &C,
    s: &OrgSchema,
    org_id: i64,
    row: &objects::Model,
) {
    let Some(def) = s.object_type(&row.type_key) else {
        return;
    };
    let values = super::opposition::contact_values(def, &row.properties);
    if values.is_empty() {
        return;
    }
    for t in s.types.iter().map(|(t, _, _)| t) {
        for p in t.properties.iter().filter(|p| p.contact_point) {
            let found = db
                .query_all_raw(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    "SELECT id FROM objects
                     WHERE org_id = $1 AND type_key = $2 AND valid_to IS NULL AND erased_at IS NULL
                       AND id <> $3 AND lower(properties ->> $4) = ANY($5)",
                    vec![
                        org_id.into(),
                        t.key.clone().into(),
                        row.id.into(),
                        p.key.clone().into(),
                        values.clone().into(),
                    ],
                ))
                .await;
            let found = match found {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!(error = %e, "other holders not looked up");
                    continue;
                }
            };
            for other in found
                .iter()
                .filter_map(|r| r.try_get::<Uuid>("", "id").ok())
            {
                propose_erasure(db, org_id, &t.key, other, "same_contact_as", row.id).await;
            }
        }
    }
}

/// The live targets of the erased `id` through `propose_erase_targets` links
/// become erasure proposals. Best effort, like the other holders.
async fn propose_shared_targets<C: ConnectionTrait>(db: &C, s: &OrgSchema, org_id: i64, id: Uuid) {
    let relations: Vec<String> = s
        .links
        .iter()
        .filter(|(l, _, _)| l.propose_erase_targets)
        .map(|(l, _, _)| l.key.clone())
        .collect();
    if relations.is_empty() {
        return;
    }
    let found = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT DISTINCT o.id, o.type_key FROM resource_edges e
             JOIN objects o ON o.org_id = e.org_id AND o.id = e.target_object_id
             WHERE e.org_id = $1 AND e.source_object_id = $2 AND e.relation = ANY($3)
               AND o.valid_to IS NULL AND o.erased_at IS NULL",
            vec![org_id.into(), id.into(), relations.into()],
        ))
        .await;
    let found = match found {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(error = %e, "shared targets not looked up");
            return;
        }
    };
    for r in &found {
        let (Ok(target), Ok(type_key)) = (
            r.try_get::<Uuid>("", "id"),
            r.try_get::<String>("", "type_key"),
        ) else {
            continue;
        };
        propose_erasure(db, org_id, &type_key, target, "shared_with", id).await;
    }
}

/// Proposes erasing `target`; the proposal holds only ids, nothing erased.
async fn propose_erasure<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    type_key: &str,
    target: Uuid,
    reason: &str,
    erased: Uuid,
) {
    let mut values = serde_json::Map::new();
    values.insert(reason.into(), erased.to_string().into());
    let input = ProposalInput {
        kind: Some(ProposalKind::ObjectErase),
        type_key: type_key.to_string(),
        target_id: Some(target.to_string()),
        evidence: Value::Object(values.clone()),
        values,
        origin: format!("erasure:{erased}"),
        ..Default::default()
    };
    if let Err(e) = super::proposals::propose(db, org_id, &input).await {
        tracing::warn!(error = %e, %target, "erasure not proposed");
    }
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
    crate::services::datasets::erasure::enqueue(
        db,
        org_id,
        &row.type_key,
        &row.title,
        &row.properties,
    )
    .await?;
    // Its accounts keep a fingerprint of the name (D447).
    if title_personal {
        let keys = super::opposition::title_keys(db, s, org_id, row.id, &row.title).await?;
        super::opposition::record(db, org_id, actor.user_id, &keys).await?;
    }
    // Its contact points stay refused once they are gone (D610).
    let contacts: Vec<String> = super::opposition::contact_values(def, &row.properties)
        .iter()
        .map(|v| super::opposition::contact_key(org_id, v))
        .collect();
    super::opposition::record(db, org_id, actor.user_id, &contacts).await?;
    // A diff kept for review holds titles and properties as they were: the preview
    // is computed again if someone still wants it.
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE object_import_runs SET diff = NULL, \
           state = CASE WHEN state = 'preview' THEN 'stale' ELSE state END \
         WHERE org_id = $1 AND diff IS NOT NULL AND state IN ('preview', 'refused')",
        vec![org_id.into()],
    ))
    .await?;
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
                keys.clone().into(),
            ],
        ))
        .await?;
        // Their recorded past states too (D913): a read at an earlier
        // instant must not bring the erased values back.
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE link_versions v SET attributes = v.attributes - $4::text[]
             FROM resource_edges e
             WHERE v.edge_id = e.id AND e.org_id = $1 AND e.relation = $2
               AND (e.source_object_id = $3 OR e.target_object_id = $3)",
            vec![
                org_id.into(),
                lt.key.clone().into(),
                row.id.into(),
                keys.into(),
            ],
        ))
        .await?;
    }
    // Same for the object's own recorded states (D900).
    let personal = personal_keys(s, &row.type_key);
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE object_versions SET properties = properties - $2::text[],
           title = CASE WHEN $3 THEN $4 ELSE title END
         WHERE object_id = $1",
        vec![
            row.id.into(),
            personal.into(),
            title_personal.into(),
            erased_title(&row.id.to_string()).into(),
        ],
    ))
    .await?;
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
