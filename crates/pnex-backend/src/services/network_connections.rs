//! Connections of a contact on LinkedIn (crm.md D976, D977).
//!
//! A user declares where their own account stands with a person (none,
//! invitation sent, connected). Only for a connected person may they ask
//! the exploration of that person's connections: the pipeline
//! `prospecting_b2b/connections_capture_linkedin` plays in their browser
//! agent and session, at a reader's pace (pipelines.md D996, PE5.4). It keeps
//! only the people whose card names an organization of the org's chosen
//! selection (organizations carrying the label set in
//! `prospecting_b2b.connections_keep_label`): each becomes a person, linked
//! by `knows` from the explored contact, and proposed as working at that
//! organization. Everyone else is never written, and the run's raw extracts
//! are erased (D976). Opposition (D341, D610) applies to both ends.

use loco_rs::app::AppContext;
use pnex_core::browser_agent::{self as ba, ConnectionState, PersonConnection};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

use crate::services::extension::ExtensionError;
use crate::services::ontology;
use crate::services::profiles;
use crate::services::site_recipes::purpose;

const NETWORK: &str = "linkedin";
/// Setting naming the label of the organizations whose people are kept.
const KEEP_LABEL_SETTING: &str = "prospecting_b2b.connections_keep_label";
/// Pipeline of the prospecting pack that reads a contact's connections.
const PIPELINE: &str = "prospecting_b2b/connections_capture_linkedin";
/// Index of the browse step that fails when the connections list is not
/// offered (hidden by the contact, or not a 1st-degree connection).
pub(crate) const HIDDEN_STEP: i32 = 5;

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// The caller's relationship with the person (`none` when never declared).
pub async fn state(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: i64,
    person_id: Uuid,
) -> Result<PersonConnection, ExtensionError> {
    profiles::person(db, org_id, person_id).await?;
    let row = db
        .query_one_raw(stmt(
            "SELECT state, since, explored_at FROM network_connections
             WHERE org_id = $1 AND user_id = $2 AND person_id = $3 AND network = $4",
            vec![
                org_id.into(),
                user_id.into(),
                person_id.into(),
                NETWORK.into(),
            ],
        ))
        .await?;
    let Some(row) = row else {
        return Ok(PersonConnection::default());
    };
    let at = |c: &str| {
        row.try_get::<Option<chrono::DateTime<chrono::FixedOffset>>>("", c)
            .ok()
            .flatten()
            .map(|d| d.to_rfc3339())
    };
    Ok(PersonConnection {
        state: ConnectionState::parse(&row.try_get::<String>("", "state").unwrap_or_default()),
        since: at("since"),
        explored_at: at("explored_at"),
    })
}

/// Declares the caller's relationship with the person.
pub async fn set_state(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: i64,
    person_id: Uuid,
    new: ConnectionState,
) -> Result<PersonConnection, ExtensionError> {
    profiles::person(db, org_id, person_id).await?;
    db.execute_raw(stmt(
        "INSERT INTO network_connections (org_id, user_id, person_id, network, state)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (org_id, user_id, person_id, network) DO UPDATE
           SET state = EXCLUDED.state,
               since = CASE WHEN network_connections.state = EXCLUDED.state
                            THEN network_connections.since ELSE now() END",
        vec![
            org_id.into(),
            user_id.into(),
            person_id.into(),
            NETWORK.into(),
            new.as_str().into(),
        ],
    ))
    .await?;
    state(db, org_id, user_id, person_id).await
}

/// Canonical LinkedIn profile of the person (`profile_url`), if any.
async fn profile_url(
    db: &DatabaseConnection,
    org_id: i64,
    person_id: Uuid,
) -> Result<Option<String>, ExtensionError> {
    let o = ontology::objects::find(db, org_id, person_id)
        .await
        .map_err(|_| ExtensionError::PersonNotFound)?;
    Ok(o.properties
        .get("profile_url")
        .and_then(|v| v.as_str())
        .and_then(|u| ba::profile_of(u).ok())
        .filter(|(n, _)| *n == NETWORK)
        .map(|(_, u)| u))
}

/// Asks one exploration of the person's connections in the caller's
/// browser agent. Refused unless the caller declared the person connected,
/// for someone who objected, or past the daily cap. An open exploration of
/// the same person by the caller is returned as is.
pub async fn request(
    ctx: &AppContext,
    org_id: i64,
    user_id: i64,
    person_id: Uuid,
) -> Result<ba::ProfileCapture, ExtensionError> {
    let db = &ctx.db;
    if state(db, org_id, user_id, person_id).await?.state != ConnectionState::Connected {
        return Err(ExtensionError::NotConnected);
    }
    let url = profile_url(db, org_id, person_id)
        .await?
        .ok_or(ExtensionError::PersonWithoutProfile)?;
    if profiles::opposed_any(db, org_id, &[profiles::person_key(person_id), url.clone()]).await? {
        return Err(ExtensionError::Opposed);
    }
    let key = person_id.to_string();
    let runs = purpose::pipeline_runs(db, org_id, PIPELINE, "person_id", &key).await?;
    let mine = user_id.to_string();
    if let Some((id, _, state, reason, at)) = runs.into_iter().find(|(_, p, state, _, _)| {
        p.get("explored_by") == Some(&mine) && matches!(state.as_str(), "queued" | "running")
    }) {
        return Ok(profiles::capture_view(id, &url, &state, reason, at));
    }
    // shortcut: count then insert is racy by a few rows, fine for an abuse cap.
    if purpose::pipeline_runs_today(db, org_id, PIPELINE, "https://").await?
        >= ba::CONNECTIONS_CAPTURES_PER_DAY
    {
        return Err(ExtensionError::ConnectionsDailyCap {
            max: ba::CONNECTIONS_CAPTURES_PER_DAY,
        });
    }
    let path = reqwest::Url::parse(&url)
        .map(|u| u.path().to_string())
        .unwrap_or_default();
    let keep: Vec<serde_json::Value> = keep_set(db, org_id)
        .await?
        .into_iter()
        .map(|(id, title)| serde_json::json!({ "id": id, "title": title }))
        .collect();
    let row = serde_json::Map::from_iter([
        ("url".to_string(), url.clone().into()),
        ("person_id".to_string(), key.clone().into()),
        ("path".to_string(), path.into()),
        (
            "keep".to_string(),
            serde_json::Value::Array(keep).to_string().into(),
        ),
        ("explored_by".to_string(), mine.clone().into()),
    ]);
    let out = crate::services::pipeline_engine::store::run_asked(
        ctx,
        org_id,
        Some(user_id),
        PIPELINE,
        vec![row],
    )
    .await
    .map_err(|e| {
        ExtensionError::Db(sea_orm::DbErr::Custom(format!(
            "connections pipeline: {}",
            e.code()
        )))
    })?;
    profiles::asked_view(&out, &url)?;
    db.execute_raw(stmt(
        "UPDATE network_connections SET explored_at = now()
         WHERE org_id = $1 AND user_id = $2 AND person_id = $3 AND network = $4",
        vec![
            org_id.into(),
            user_id.into(),
            person_id.into(),
            NETWORK.into(),
        ],
    ))
    .await?;
    captures(db, org_id, user_id, person_id)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| ExtensionError::Db(sea_orm::DbErr::Custom("capture not recorded".into())))
}

/// The caller's latest explorations of the person, newest first.
pub async fn captures(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: i64,
    person_id: Uuid,
) -> Result<Vec<ba::ProfileCapture>, ExtensionError> {
    profiles::person(db, org_id, person_id).await?;
    let runs =
        purpose::pipeline_runs(db, org_id, PIPELINE, "person_id", &person_id.to_string()).await?;
    let mine = user_id.to_string();
    let runs: Vec<_> = runs
        .into_iter()
        .filter(|(_, p, _, _, _)| p.get("explored_by") == Some(&mine))
        .collect();
    // A run stopped at the connections link: the list is hidden.
    let ids: Vec<Uuid> = runs.iter().map(|(id, ..)| *id).collect();
    let hidden: Vec<Uuid> = db
        .query_all_raw(stmt(
            "SELECT id FROM agent_runs WHERE org_id = $1 AND id = ANY($2) AND failed_step = $3",
            vec![org_id.into(), ids.into(), HIDDEN_STEP.into()],
        ))
        .await?
        .iter()
        .filter_map(|r| r.try_get("", "id").ok())
        .collect();
    Ok(runs
        .into_iter()
        .map(|(id, p, state, reason, at)| {
            let reason = if hidden.contains(&id) {
                Some(pnex_core::err_codes::EXT_CONNECTIONS_HIDDEN.to_string())
            } else {
                reason
            };
            let url = p.get("url").cloned().unwrap_or_default();
            profiles::capture_view(id, &url, &state, reason, at)
        })
        .collect())
}

/// Titles of the organizations of the org's chosen selection: those
/// carrying the label of the setting (`name` or `name:value`). No setting,
/// no selection: nobody is kept.
async fn keep_set(
    db: &DatabaseConnection,
    org_id: i64,
) -> Result<Vec<(Uuid, String)>, ExtensionError> {
    let label = ontology::packs::setting_value(db, org_id, KEEP_LABEL_SETTING)
        .await?
        .and_then(|v| {
            v.as_str()
                .map(|s| s.trim().trim_start_matches('#').to_string())
        })
        .unwrap_or_default();
    if label.is_empty() {
        return Ok(Vec::new());
    }
    let filter = match label.split_once(':') {
        Some((n, v)) => (n.trim().to_string(), Some(v.trim().to_string())),
        None => (label, None),
    };
    let ids: Vec<Uuid> = crate::services::resources::labels::ids_with_label(
        db,
        org_id,
        &[filter],
        Some("organization"),
    )
    .await?
    .into_iter()
    .filter_map(|(_, id)| Uuid::parse_str(&id).ok())
    .collect();
    let rows = db
        .query_all_raw(stmt(
            "SELECT id, title FROM objects
             WHERE org_id = $1 AND id = ANY($2) AND valid_to IS NULL AND erased_at IS NULL",
            vec![org_id.into(), ids.into()],
        ))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| Some((r.try_get("", "id").ok()?, r.try_get("", "title").ok()?)))
        .collect())
}
