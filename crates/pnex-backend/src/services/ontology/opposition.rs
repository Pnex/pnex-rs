//! Objections enforced by the engine (crm.md D447): a person who objected,
//! or whose data was erased, leaves a `title:<name>@<object>` fingerprint
//! for each object it was linked to. A new link between an object whose
//! title is personal and one of those objects is refused when the name
//! matches, so an import or a match never recreates "the same person at the
//! same account". Generic: no type or link key is named here.

use std::collections::HashSet;

use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use uuid::Uuid;

use super::{OntologyError, OrgSchema, Result};
use crate::models::_entities::objects;

/// Keys checked per statement.
const CHUNK: usize = 1_000;

/// Folded words (lower case, Latin accents dropped, letters and digits)
/// separated by one space: « Élodie », « elodie » and « ELODIE » give the
/// same key.
pub fn normalize(title: &str) -> String {
    pnex_core::fold::fold_words(title).join(" ")
}

/// Fingerprint of `title` linked to the object `other`.
pub fn title_key(title: &str, other: Uuid) -> String {
    format!("title:{}@{other}", normalize(title))
}

/// The fingerprint a new link between `a` and `b` would match, when one end
/// carries a personal title (an erased shell never matches: its name is gone).
fn link_keys(s: &OrgSchema, a: &objects::Model, b: &objects::Model) -> Vec<String> {
    let personal = |o: &objects::Model| {
        o.erased_at.is_none()
            && s.object_type(&o.type_key)
                .is_some_and(|t| t.title_classification.is_personal())
    };
    let mut keys = Vec::new();
    for (p, o) in [(a, b), (b, a)] {
        if personal(p) && !normalize(&p.title).is_empty() {
            keys.push(title_key(&p.title, o.id));
        }
    }
    keys
}

/// Refuses the links whose ends match an objection (`prospecting-opposed`).
pub async fn check_links<C: ConnectionTrait>(
    db: &C,
    s: &OrgSchema,
    org_id: i64,
    pairs: &[(&objects::Model, &objects::Model)],
) -> Result<()> {
    match opposed_pairs(db, s, org_id, pairs).await?.is_empty() {
        true => Ok(()),
        false => Err(OntologyError::Opposed),
    }
}

/// The `(a, b)` ids of the pairs matching an objection.
pub async fn opposed_pairs<C: ConnectionTrait>(
    db: &C,
    s: &OrgSchema,
    org_id: i64,
    pairs: &[(&objects::Model, &objects::Model)],
) -> Result<HashSet<(Uuid, Uuid)>> {
    let keyed: Vec<(String, (Uuid, Uuid))> = pairs
        .iter()
        .flat_map(|(a, b)| link_keys(s, a, b).into_iter().map(|k| (k, (a.id, b.id))))
        .collect();
    let keys: Vec<String> = keyed
        .iter()
        .map(|(k, _)| k.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let mut hits = HashSet::new();
    for chunk in keys.chunks(CHUNK) {
        let rows = db
            .query_all_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT fingerprint FROM prospecting_oppositions WHERE org_id = $1 AND fingerprint = ANY($2)",
                vec![org_id.into(), chunk.to_vec().into()],
            ))
            .await?;
        hits.extend(
            rows.iter()
                .filter_map(|r| r.try_get::<String>("", "fingerprint").ok()),
        );
    }
    Ok(keyed
        .into_iter()
        .filter(|(k, _)| hits.contains(k))
        .map(|(_, p)| p)
        .collect())
}

/// The fingerprints of `id` titled `title`: one per object it is linked to
/// by an open link, whose own title is not personal (its accounts, not
/// other people).
pub async fn title_keys<C: ConnectionTrait>(
    db: &C,
    s: &OrgSchema,
    org_id: i64,
    id: Uuid,
    title: &str,
) -> Result<Vec<String>> {
    if normalize(title).is_empty() {
        return Ok(Vec::new());
    }
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT o.id, o.type_key FROM resource_edges l
             JOIN objects o ON o.org_id = l.org_id
              AND o.id = CASE WHEN l.source_object_id = $2 THEN l.target_object_id ELSE l.source_object_id END
             WHERE l.org_id = $1 AND l.valid_to IS NULL
               AND (l.source_object_id = $2 OR l.target_object_id = $2)",
            vec![org_id.into(), id.into()],
        ))
        .await?;
    let mut keys = Vec::new();
    for row in rows {
        let (Ok(other), Ok(type_key)) = (
            row.try_get::<Uuid>("", "id"),
            row.try_get::<String>("", "type_key"),
        ) else {
            continue;
        };
        let personal = s
            .object_type(&type_key)
            .is_some_and(|t| t.title_classification.is_personal());
        if !personal {
            keys.push(title_key(title, other));
        }
    }
    keys.sort();
    keys.dedup();
    Ok(keys)
}

/// Adds fingerprints to the org's list (kept when already there).
pub async fn record<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    user_id: Option<i64>,
    keys: &[String],
) -> Result<()> {
    for key in keys {
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO prospecting_oppositions (org_id, fingerprint, created_by)
             VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
            vec![org_id.into(), key.clone().into(), user_id.into()],
        ))
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_normalize_to_one_key() {
        assert_eq!(normalize("  Ann  SMITH-Jones "), "ann smith jones");
        assert_eq!(normalize("Élodie O'Neil"), "elodie o neil");
        assert_eq!(normalize("ELODIE o’neil"), normalize("Élodie O'Neil"));
        assert_eq!(normalize("--"), "");
        let id = Uuid::nil();
        assert_eq!(title_key("Ann Smith", id), title_key("ann   smith", id));
    }
}
