//! Retention of an archived case (cases.md K16): the persons and
//! identifiers it gathered that no other case and no other open link
//! hold are proposed for purge. Purging deletes them for real (the one
//! exception to "archive, never delete", D179): the object row, every
//! link to or from it, its ranges, labels and containment, and the frozen
//! states that mention it.
//! shortcut: copies already in the change journal (O2 `object_changes`)
//! stay until the org's journal retention expires.

use pnex_core::err_codes::FIELD_INVALID;
use pnex_core::ontology::api::ObjectRef;
use pnex_core::ontology::CASE_STATUS;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement, TransactionTrait};
use serde_json::Value;
use uuid::Uuid;

use super::super::changes::{self, Change};
use super::super::{invalid, objects, role_allows, schema, Actor, OntologyError, Result};
use super::{case_of, T_PERSON};

/// Types holding personal data that a case retention purges.
const PERSONAL: [&str; 2] = [T_PERSON, "identifier"];

/// Personal objects held by this case only. The case holds an object that
/// is (or was) one of its undated pieces or attached to one of its events.
/// Kept out: an object another case holds (even in the past), and an object
/// with an open link to anything this case does not hold (a person and
/// their phone number, both held, go together).
pub async fn candidates(
    db: &DatabaseConnection,
    org_id: i64,
    case_id: Uuid,
) -> Result<Vec<ObjectRef>> {
    case_of(db, org_id, case_id).await?;
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "WITH ev AS (SELECT source_object_id AS id FROM resource_edges \
                         WHERE org_id = $1 AND relation = 'in_case' AND target_object_id = $2), \
                  held AS (SELECT source_object_id AS id FROM resource_edges \
                           WHERE org_id = $1 AND relation = 'in_case' AND target_object_id = $2 \
                           UNION SELECT e.target_object_id FROM resource_edges e JOIN ev ON e.source_object_id = ev.id \
                           WHERE e.org_id = $1 AND e.relation = 'involves') \
             SELECT o.id, o.type_key, o.title, o.native_id FROM objects o \
             WHERE o.org_id = $1 AND o.type_key IN ($3, $4) AND o.id IN (SELECT id FROM held) \
               AND NOT EXISTS (SELECT 1 FROM resource_edges m WHERE m.org_id = $1 AND m.relation = 'in_case' \
                               AND m.source_object_id = o.id AND m.target_object_id <> $2) \
               AND NOT EXISTS (SELECT 1 FROM resource_edges i JOIN resource_edges c \
                               ON c.source_object_id = i.source_object_id AND c.relation = 'in_case' AND c.target_object_id <> $2 \
                               WHERE i.org_id = $1 AND i.relation = 'involves' AND i.target_object_id = o.id) \
               AND NOT EXISTS (SELECT 1 FROM resource_edges e \
                               WHERE e.org_id = $1 AND e.valid_to IS NULL \
                                 AND (e.source_object_id = o.id OR e.target_object_id = o.id) \
                                 AND NOT (e.relation = 'in_case' AND e.target_object_id = $2) \
                                 AND NOT (e.relation = 'involves' AND e.source_object_id IN (SELECT id FROM ev)) \
                                 AND (CASE WHEN e.source_object_id = o.id THEN e.target_object_id \
                                      ELSE e.source_object_id END) NOT IN (SELECT id FROM held)) \
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
        super::snapshots::forget(&txn, org_id, id).await?;
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
