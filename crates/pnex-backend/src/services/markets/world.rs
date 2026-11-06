//! Confirmation of world event leads (markets-research.md D741): a GDELT
//! lead still unconfirmed is confirmed by a verified event (UCDP, ACLED) of
//! the same country, at most a day apart, of a compatible kind. The change
//! is an ordinary object version, by the import that brought the evidence:
//! traced in the object's history. A status a member set is never touched.

use chrono::NaiveDate;
use pnex_core::conflict_events::{confirms, Dated};
use pnex_core::ontology::api::ObjectInput;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::Value;
use uuid::Uuid;

use crate::services::ontology::{objects, Actor, OntologyError};

/// Days of leads and verified events compared at each pass.
const WINDOW_DAYS: i64 = 120;

fn dated(p: &Value) -> Option<Dated> {
    let s = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or_default();
    Some(Dated {
        country: s("country").to_string(),
        day: NaiveDate::parse_from_str(s("event_at"), "%Y-%m-%d").ok()?,
        kind: s("kind").to_string(),
    })
}

/// Confirms the leads a verified event matches; returns how many.
pub async fn confirm_leads(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
) -> Result<usize, OntologyError> {
    let since = (chrono::Utc::now().date_naive() - chrono::Duration::days(WINDOW_DAYS)).to_string();
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT id, properties FROM objects WHERE org_id = $1 AND type_key = 'geo_event' \
               AND valid_to IS NULL AND properties ->> 'event_at' >= $2 \
               AND (properties ->> 'source_kind' IN ('ucdp', 'acled') \
                 OR (properties ->> 'source_kind' = 'gdelt' \
                     AND coalesce(properties ->> 'status', 'unconfirmed') = 'unconfirmed')) \
             LIMIT 50000",
            vec![org_id.into(), since.into()],
        ))
        .await?;
    let (mut verified, mut leads) = (Vec::new(), Vec::new());
    for r in rows {
        let id: Uuid = r.try_get("", "id")?;
        let p: Value = r.try_get("", "properties")?;
        let Some(d) = dated(&p) else { continue };
        let reference = p
            .get("ref")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        match p.get("source_kind").and_then(Value::as_str) {
            Some("gdelt") => leads.push((id, d)),
            _ => verified.push((reference, d)),
        }
    }
    let mut confirmed = 0;
    for (id, lead) in leads {
        let Some((by, _)) = verified.iter().find(|(_, v)| confirms(v, &lead)) else {
            continue;
        };
        let o = objects::find(db, org_id, id).await?;
        let mut props = o.properties.as_object().cloned().unwrap_or_default();
        props.insert("status".into(), "confirmed".into());
        props.insert("confirmed_with".into(), by.clone().into());
        let input = ObjectInput {
            type_key: o.type_key.clone(),
            title: o.title.clone(),
            properties: props,
            expected_version: Some(o.version),
        };
        match objects::update(db, org_id, actor, id, &input).await {
            Ok(_) => confirmed += 1,
            // Changed meanwhile (a member): theirs wins, the next pass sees it.
            Err(OntologyError::VersionConflict { .. }) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(confirmed)
}
