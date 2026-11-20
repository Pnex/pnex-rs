//! Connections of a contact on LinkedIn (crm.md D976, D977).
//!
//! A user declares where their own account stands with a person (none,
//! invitation sent, connected). Only for a connected person may they ask
//! the exploration of that person's connections: the default rule plays in
//! their browser agent and session, at a reader's pace. When it ends, the
//! server keeps only the people whose card names an organization of the
//! org's chosen selection (organizations carrying the label set in
//! `prospecting_b2b.connections_keep_label`): each becomes a person, linked
//! by `knows` from the explored contact, and proposed as working at that
//! organization. Everyone else is counted, never written, and the run's raw
//! extracts are erased (D976). Opposition (D341, D610) applies to both ends.

use std::collections::BTreeMap;

use loco_rs::app::AppContext;
use pnex_core::browser_agent::{self as ba, ConnectionState, PersonConnection};
use pnex_core::default_rules;
use pnex_core::ontology::proposal::{ProposalInput, ProposalKind};
use pnex_core::site_recipe::RunExtract;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

use crate::services::extension::ExtensionError;
use crate::services::ontology;
use crate::services::profiles;
use crate::services::site_recipes::purpose::{self, Purpose};

const NETWORK: &str = "linkedin";
/// Setting naming the label of the organizations whose people are kept.
const KEEP_LABEL_SETTING: &str = "prospecting_b2b.connections_keep_label";
/// `source` attribute of the `knows` links written from an exploration.
const KNOWS_SOURCE: &str = "linkedin_connections";
/// People kept from one exploration at most.
const KEPT_MAX: usize = 100;

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
    let runs = purpose::runs_of(db, org_id, "connections_capture", "person_id", &key).await?;
    if let Some((id, _, state, reason, at)) = runs.into_iter().find(|(_, p, state, _, _)| {
        matches!(p, Purpose::ConnectionsCapture { user_id: u, .. } if *u == user_id)
            && matches!(state.as_str(), "queued" | "running")
    }) {
        return Ok(profiles::capture_view(id, &url, &state, reason, at));
    }
    // shortcut: count then insert is racy by a few rows, fine for an abuse cap.
    if purpose::count_today(db, org_id, "connections_capture", "https://").await?
        >= ba::CONNECTIONS_CAPTURES_PER_DAY
    {
        return Err(ExtensionError::ConnectionsDailyCap {
            max: ba::CONNECTIONS_CAPTURES_PER_DAY,
        });
    }
    let path = reqwest::Url::parse(&url)
        .map(|u| u.path().to_string())
        .unwrap_or_default();
    let params = BTreeMap::from([("url".to_string(), url.clone()), ("path".to_string(), path)]);
    let purpose = Purpose::ConnectionsCapture {
        person_id: key,
        user_id,
        url: url.clone(),
    };
    let run = purpose::start(
        ctx,
        org_id,
        Some(user_id),
        &default_rules::connections_capture(),
        params,
        &purpose,
    )
    .await
    .map_err(profiles::start_error)?;
    let id = Uuid::parse_str(&run.id).unwrap_or_default();
    Ok(profiles::capture_view(
        id,
        &url,
        "queued",
        None,
        run.started_at,
    ))
}

/// The caller's latest explorations of the person, newest first.
pub async fn captures(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: i64,
    person_id: Uuid,
) -> Result<Vec<ba::ProfileCapture>, ExtensionError> {
    profiles::person(db, org_id, person_id).await?;
    let runs = purpose::runs_of(
        db,
        org_id,
        "connections_capture",
        "person_id",
        &person_id.to_string(),
    )
    .await?;
    Ok(runs
        .into_iter()
        .filter_map(|(id, p, state, reason, at)| match p {
            Purpose::ConnectionsCapture {
                user_id: u, url, ..
            } if u == user_id => Some(profiles::capture_view(id, &url, &state, reason, at)),
            _ => None,
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

/// The end of an exploration: a hidden list is reported as such; a read one
/// keeps the people at the chosen organizations, then its raw extracts are
/// erased whatever happens.
pub async fn captured(
    ctx: &AppContext,
    org_id: i64,
    run_id: Uuid,
    person_id: &str,
    user_id: i64,
    ok: bool,
    extracts: &[RunExtract],
) -> Result<(), ExtensionError> {
    let db = &ctx.db;
    let outcome = if ok {
        keep(ctx, org_id, run_id, person_id, user_id, extracts).await
    } else {
        hidden(db, run_id).await
    };
    // The cards of everyone met were in the run: they never stay (D976).
    db.execute_raw(stmt(
        "UPDATE agent_runs SET extracts = '[]' WHERE id = $1 AND org_id = $2",
        vec![run_id.into(), org_id.into()],
    ))
    .await?;
    outcome
}

/// A run stopped at the step that needs the connections link: the list is
/// hidden (or the person not a connection), said with its own code.
async fn hidden(db: &DatabaseConnection, run_id: Uuid) -> Result<(), ExtensionError> {
    db.execute_raw(stmt(
        "UPDATE agent_runs SET reason = $2 WHERE id = $1 AND failed_step = $3",
        vec![
            run_id.into(),
            pnex_core::err_codes::EXT_CONNECTIONS_HIDDEN.into(),
            (default_rules::CONNECTIONS_HIDDEN_STEP as i32).into(),
        ],
    ))
    .await?;
    Ok(())
}

async fn keep(
    ctx: &AppContext,
    org_id: i64,
    run_id: Uuid,
    person_id: &str,
    user_id: i64,
    extracts: &[RunExtract],
) -> Result<(), ExtensionError> {
    let db = &ctx.db;
    let Ok(explored) = Uuid::parse_str(person_id) else {
        return Ok(());
    };
    // The contact may have objected since the exploration was asked.
    if profiles::opposed_any(db, org_id, &[profiles::person_key(explored)]).await? {
        return Ok(());
    }
    let cards: Vec<BTreeMap<String, String>> = extracts
        .iter()
        .filter(|e| e.name.starts_with("connections."))
        .flat_map(|e| e.cards.iter().cloned())
        .collect();
    let orgs = keep_set(db, org_id).await?;
    let titles: Vec<String> = orgs.iter().map(|(_, t)| t.clone()).collect();
    let (kept, dropped) = ba::keep_connections(&cards, &titles);
    let source = format!("rule_run:{run_id}");
    let actor = profiles::actor_of(&source);
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let mut written = 0;
    for k in kept.iter().take(KEPT_MAX) {
        if profiles::opposed_any(db, org_id, std::slice::from_ref(&k.url)).await? {
            continue;
        }
        let Some(found) = person_of(db, org_id, &actor, k).await else {
            continue;
        };
        if found == explored
            || profiles::opposed_any(db, org_id, &[profiles::person_key(found)]).await?
        {
            continue;
        }
        let mut attributes = serde_json::Map::new();
        attributes.insert("source".into(), KNOWS_SOURCE.into());
        attributes.insert("retrieved_on".into(), today.clone().into());
        attributes.insert("explored_by".into(), user_id.into());
        let link = pnex_core::ontology::api::LinkInput {
            link_type: "knows".into(),
            source_id: explored.to_string(),
            target_id: found.to_string(),
            attributes,
            valid_from: None,
            valid_to: None,
        };
        // An open `knows` of the pair already exists: kept as it is.
        if let Err(e) = ontology::links::create(db, org_id, &actor, &link).await {
            tracing::debug!(error = %e, "knows link not written");
        }
        let (org, _) = &orgs[k.organization];
        propose_employs(db, org_id, *org, found, k).await;
        written += 1;
    }
    db.execute_raw(stmt(
        "UPDATE network_connections SET explored_at = now()
         WHERE org_id = $1 AND user_id = $2 AND person_id = $3 AND network = $4",
        vec![
            org_id.into(),
            user_id.into(),
            explored.into(),
            NETWORK.into(),
        ],
    ))
    .await?;
    tracing::info!(run = %run_id, kept = written, dropped, "connections explored");
    Ok(())
}

/// The person of a kept card: found by profile address, else created.
async fn person_of(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &ontology::Actor,
    k: &ba::KeptConnection,
) -> Option<Uuid> {
    let url = serde_json::Value::String(k.url.clone());
    match ontology::objects::find_by_key(db, org_id, "person", "profile_url", &url).await {
        Ok(Some(o)) => return Some(o.id),
        Ok(None) => {}
        Err(e) => {
            tracing::warn!(error = %e, "person lookup failed");
            return None;
        }
    }
    let mut properties = serde_json::Map::new();
    properties.insert("profile_url".into(), url);
    let input = pnex_core::ontology::api::ObjectInput {
        type_key: "person".into(),
        title: k.name.clone(),
        properties,
        expected_version: None,
    };
    match ontology::objects::create(db, org_id, actor, &input).await {
        Ok(o) => Uuid::parse_str(&o.id).ok(),
        Err(e) => {
            tracing::warn!(error = %e, "person not written");
            None
        }
    }
}

/// `employs` organization → person, to validate, unless already open.
async fn propose_employs(
    db: &DatabaseConnection,
    org_id: i64,
    organization: Uuid,
    person: Uuid,
    k: &ba::KeptConnection,
) {
    let open = db
        .query_one_raw(stmt(
            "SELECT 1 AS x FROM resource_edges WHERE org_id = $1 AND relation = 'employs'
               AND valid_to IS NULL AND source_object_id = $2 AND target_object_id = $3",
            vec![org_id.into(), organization.into(), person.into()],
        ))
        .await;
    if matches!(open, Ok(Some(_))) {
        return;
    }
    let mut values = serde_json::Map::new();
    values.insert("position".into(), k.headline.clone().into());
    let input = ProposalInput {
        kind: Some(ProposalKind::LinkCreate),
        type_key: "employs".into(),
        target_id: None,
        source_id: Some(organization.to_string()),
        link_target_id: Some(person.to_string()),
        link_type: None,
        title: Some(k.name.clone()),
        values,
        base_version: None,
        evidence: serde_json::json!({ "url": k.url, "headline": k.headline }),
        origin: KNOWS_SOURCE.into(),
        confidence: None,
    };
    if let Err(e) = ontology::proposals::propose(db, org_id, &input).await {
        tracing::warn!(error = %e, "employs proposal not written");
    }
}
