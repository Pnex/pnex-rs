//! Retention of an archived case (cases.md K16): the persons and
//! identifiers it gathered that no other case and no other open link
//! hold are proposed for purge. Purging deletes them for real (the one
//! exception to "archive, never delete", D179): the object row, every
//! link to or from it, its ranges, labels and containment.
//! shortcut: copies already in the change journal (O2 `object_changes`)
//! stay until the org's journal retention expires.

use pnex_core::err_codes::FIELD_INVALID;
use pnex_core::ontology::api::ObjectRef;
use pnex_core::ontology::{CASE_STATUS, KIND_CASE};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement, TransactionTrait};
use serde_json::Value;
use uuid::Uuid;

use super::changes::{self, Change};
use super::{invalid, objects, role_allows, schema, Actor, OntologyError, Result};

/// Types holding personal data that a case retention purges.
const PERSONAL: [&str; 2] = ["person", "identifier"];

/// Personal objects held by this case only: a membership of the case
/// (open or closed), no membership of any other case (even closed), no
/// open link but to another member of the case (a person and their phone
/// number, both members, go together).
pub async fn candidates(
    db: &DatabaseConnection,
    org_id: i64,
    case_id: Uuid,
) -> Result<Vec<ObjectRef>> {
    let case = objects::find(db, org_id, case_id).await?;
    if case.type_key != KIND_CASE {
        return Err(OntologyError::NotFound);
    }
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT o.id, o.type_key, o.title, o.native_id FROM objects o \
             WHERE o.org_id = $1 AND o.type_key IN ($3, $4) \
               AND EXISTS (SELECT 1 FROM resource_edges m WHERE m.source_object_id = o.id \
                           AND m.relation = 'in_case' AND m.target_object_id = $2) \
               AND NOT EXISTS (SELECT 1 FROM resource_edges e \
                           WHERE (e.source_object_id = o.id OR e.target_object_id = o.id) \
                             AND NOT (e.relation = 'in_case' AND e.target_object_id = $2) \
                             AND (e.valid_to IS NULL OR e.relation = 'in_case') \
                             AND NOT EXISTS (SELECT 1 FROM resource_edges m2 \
                                   WHERE m2.relation = 'in_case' AND m2.target_object_id = $2 \
                                     AND m2.source_object_id = CASE WHEN e.source_object_id = o.id \
                                         THEN e.target_object_id ELSE e.source_object_id END)) \
             ORDER BY o.title",
            [
                org_id.into(),
                case_id.into(),
                PERSONAL[0].into(),
                PERSONAL[1].into(),
            ],
        ))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            Some(ObjectRef {
                id: r.try_get::<Uuid>("", "id").ok()?.to_string(),
                type_key: r.try_get("", "type_key").ok()?,
                title: r.try_get("", "title").ok()?,
                native_id: r.try_get("", "native_id").ok()?,
            })
        })
        .collect())
}

/// Purges `ids` among the candidates of an archived case; returns how
/// many were purged. Ids that are not (or no longer) candidates are
/// skipped: the list is re-computed here, never trusted from the client.
pub async fn purge(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    case_id: Uuid,
    ids: &[String],
) -> Result<usize> {
    let case = objects::find(db, org_id, case_id).await?;
    if case.properties.get(CASE_STATUS).and_then(Value::as_str) != Some("archived") {
        return Err(invalid(CASE_STATUS, FIELD_INVALID));
    }
    let s = schema(db, org_id).await?;
    let picked: Vec<ObjectRef> = candidates(db, org_id, case_id)
        .await?
        .into_iter()
        .filter(|c| ids.contains(&c.id))
        .collect();
    for c in &picked {
        let allowed = s
            .object_type(&c.type_key)
            .is_some_and(|d| role_allows(&actor.role, d.write_role));
        if !allowed {
            return Err(OntologyError::WriteForbidden);
        }
    }
    for c in &picked {
        let id = Uuid::parse_str(&c.id).map_err(|_| OntologyError::NotFound)?;
        let txn = db.begin().await?;
        for sql in [
            "DELETE FROM resource_edges WHERE org_id = $1 AND (source_object_id = $2 OR target_object_id = $2)",
            "DELETE FROM time_ranges WHERE org_id = $1 AND scope_kind = 'object' AND scope_id = $2::text",
            "DELETE FROM objects WHERE org_id = $1 AND id = $2",
        ] {
            txn.execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                sql,
                [org_id.into(), id.into()],
            ))
            .await?;
        }
        txn.commit().await?;
        crate::services::resources::purge_for(db, org_id, &c.type_key, &c.native_id).await?;
        // The journal records the purge, never the purged data again.
        changes::submit(Change {
            org_id,
            action: "object.purge".into(),
            object_id: c.id.clone(),
            type_key: c.type_key.clone(),
            source_ref: actor.source_ref.clone(),
            before: Value::Null,
            after: Value::Null,
        });
    }
    Ok(picked.len())
}
