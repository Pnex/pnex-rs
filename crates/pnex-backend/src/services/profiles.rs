//! Profile captures (account-mapping.md D333) and the prospecting
//! opposition list (D341, minimum). A human asks, from a `person`'s page,
//! one capture of one profile page on a social network; a browser agent of
//! the org opens it once in its signed-in session and uploads the page
//! text. The text becomes a document linked to the person (`published_by`),
//! and the profile address a `uses_identifier` account of the person. The
//! raw text is purged after [`RAW_TEXT_DAYS`].

use loco_rs::app::AppContext;
use pnex_core::browser_agent::{self as ba, Mission, MissionSpec};
use pnex_core::err_codes;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

use crate::services::extension::{AgentAuth, ExtensionError};
use crate::services::ontology;

/// Days a captured profile text is kept (D333).
pub const RAW_TEXT_DAYS: i64 = 30;
const PRUNE_INTERVAL_SECS: u64 = 3600;

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

fn person_key(person_id: Uuid) -> String {
    format!("person:{person_id}")
}

/// The live `person` object of the org.
async fn person(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<(), ExtensionError> {
    match ontology::objects::find(db, org_id, id).await {
        Ok(o) if o.type_key == "person" && o.valid_to.is_none() => Ok(()),
        Ok(_) | Err(ontology::OntologyError::NotFound) => Err(ExtensionError::PersonNotFound),
        Err(e) => Err(ExtensionError::Db(sea_orm::DbErr::Custom(e.to_string()))),
    }
}

/// Whether the person, or this profile address, is on the opposition list.
async fn opposed_any(
    db: &impl ConnectionTrait,
    org_id: i64,
    keys: &[String],
) -> Result<bool, ExtensionError> {
    Ok(db
        .query_one_raw(stmt(
            "SELECT 1 AS x FROM prospection_oppositions WHERE org_id = $1 AND fingerprint = ANY($2)",
            vec![org_id.into(), keys.to_vec().into()],
        ))
        .await?
        .is_some())
}

/// Whether the person objected to prospecting.
pub async fn opposed(
    db: &DatabaseConnection,
    org_id: i64,
    person_id: Uuid,
) -> Result<bool, ExtensionError> {
    person(db, org_id, person_id).await?;
    opposed_any(db, org_id, &[person_key(person_id)]).await
}

/// Records the person's objection (D341): the person and every profile
/// address it uses enter the list, which outlives the person, with a
/// fingerprint of its name at each of its accounts (crm.md D447); the
/// date lands on the person (`opposition_le`) for the prospecting flows.
pub async fn oppose(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: i64,
    person_id: Uuid,
) -> Result<(), ExtensionError> {
    person(db, org_id, person_id).await?;
    oppose_by(db, org_id, Some(user_id), person_id).await
}

/// Property of `prospection_b2b` holding the objection date.
const OPPOSED_ON: &str = "opposition_le";

async fn oppose_by(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: Option<i64>,
    person_id: Uuid,
) -> Result<(), ExtensionError> {
    let onto_err =
        |e: ontology::OntologyError| ExtensionError::Db(sea_orm::DbErr::Custom(e.to_string()));
    let s = ontology::schema(db, org_id)
        .await
        .map_err(|e| onto_err(e.into()))?;
    let row = ontology::objects::find(db, org_id, person_id)
        .await
        .map_err(onto_err)?;
    let names = ontology::opposition::title_keys(db, &s, org_id, person_id, &row.title)
        .await
        .map_err(onto_err)?;
    ontology::opposition::record(db, org_id, user_id, &names)
        .await
        .map_err(onto_err)?;
    let has_date = s
        .object_type("person")
        .is_some_and(|t| t.properties.iter().any(|p| p.key == OPPOSED_ON));
    if has_date && row.properties.get(OPPOSED_ON).is_none() {
        // Written in place: a date stamp, not an edit of the person.
        db.execute_raw(stmt(
            "UPDATE objects SET properties = properties || jsonb_build_object($3::text, to_char(now(), 'YYYY-MM-DD'))
             WHERE org_id = $1 AND id = $2",
            vec![org_id.into(), person_id.into(), OPPOSED_ON.into()],
        ))
        .await?;
    }
    let accounts = db
        .query_all_raw(stmt(
            "SELECT i.properties ->> 'value' AS value FROM resource_edges l
             JOIN objects i ON i.id = l.target_object_id AND i.org_id = l.org_id
             WHERE l.org_id = $1 AND l.source_object_id = $2 AND l.relation = 'uses_identifier'
               AND i.properties ->> 'kind' = 'account'",
            vec![org_id.into(), person_id.into()],
        ))
        .await?;
    let mut keys = vec![person_key(person_id)];
    for row in accounts {
        if let Ok(Some(v)) = row.try_get::<Option<String>>("", "value") {
            keys.push(ba::profile_of(&v).map(|(_, c)| c).unwrap_or(v));
        }
    }
    for key in keys {
        db.execute_raw(stmt(
            "INSERT INTO prospection_oppositions (org_id, fingerprint, created_by)
             VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
            vec![org_id.into(), key.into(), user_id.into()],
        ))
        .await?;
    }
    // Pending captures of the person stop at once.
    db.execute_raw(stmt(
        "UPDATE extension_missions SET enabled = false
         WHERE org_id = $1 AND kind = 'capture_profile' AND spec -> 'params' ->> 'person_id' = $2",
        vec![org_id.into(), person_id.to_string().into()],
    ))
    .await?;
    Ok(())
}

/// Before a person is erased (crm.md D446): it enters the opposition
/// list, its captured profile texts are deleted, and the identifiers it
/// uses are returned to be erased with it. Other objects: nothing.
pub async fn before_erase(
    ctx: &AppContext,
    org_id: i64,
    actor: &ontology::Actor,
    row: &crate::models::_entities::objects::Model,
) -> Result<Vec<Uuid>, ExtensionError> {
    if row.type_key != "person" {
        return Ok(Vec::new());
    }
    oppose_by(&ctx.db, org_id, actor.user_id, row.id).await?;
    let texts = ctx
        .db
        .query_all_raw(stmt(
            "SELECT id FROM media_assets WHERE org_id = $1 AND metadata ->> 'profile_person_id' = $2",
            vec![org_id.into(), row.id.to_string().into()],
        ))
        .await?;
    let texts: Vec<Uuid> = texts
        .iter()
        .filter_map(|r| r.try_get("", "id").ok())
        .collect();
    if !texts.is_empty() {
        crate::services::voice_id::samples::delete_assets(ctx, org_id, &texts)
            .await
            .map_err(|e| ExtensionError::Db(sea_orm::DbErr::Custom(e.to_string())))?;
    }
    // The mails the person sent go with them (mail.md D461): those whose
    // correspondent they are (the original of a trusted forward, else the
    // sender), never a member's forward of someone else's mail.
    ctx.db
        .execute_raw(stmt(
            "DELETE FROM mail_messages WHERE org_id = $1 AND \
               CASE WHEN COALESCE((forwarded ->> 'trusted')::boolean, false) \
                 THEN lower(forwarded -> 'from' ->> 'address') ELSE from_address END IN (
               SELECT lower(i.properties ->> 'value') FROM resource_edges l
               JOIN objects i ON i.id = l.target_object_id AND i.org_id = l.org_id
               WHERE l.org_id = $1 AND l.source_object_id = $2 AND l.relation = 'uses_identifier'
                 AND i.properties ->> 'kind' = 'email')",
            vec![org_id.into(), row.id.into()],
        ))
        .await?;
    let ids = ctx
        .db
        .query_all_raw(stmt(
            "SELECT target_object_id AS id FROM resource_edges
             WHERE org_id = $1 AND source_object_id = $2 AND relation = 'uses_identifier'",
            vec![org_id.into(), row.id.into()],
        ))
        .await?;
    Ok(ids
        .iter()
        .filter_map(|r| r.try_get("", "id").ok())
        .collect())
}

/// Asks one capture of `raw_url` for the person (D333). An open capture of
/// the same profile is returned as is.
pub async fn request(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: i64,
    person_id: Uuid,
    raw_url: &str,
) -> Result<Mission, ExtensionError> {
    person(db, org_id, person_id).await?;
    let (network, url) = ba::profile_of(raw_url).map_err(|r| match r {
        ba::ProfileRefusal::NotNetwork => ExtensionError::Invalid {
            field: "url".into(),
            token: err_codes::FIELD_INVALID.into(),
        },
        ba::ProfileRefusal::NotProfile => ExtensionError::NotProfile,
    })?;
    if opposed_any(db, org_id, &[person_key(person_id), url.clone()]).await? {
        return Err(ExtensionError::Opposed);
    }
    let target = ba::profile_target(network, &url);
    let open = db
        .query_one_raw(stmt(
            "SELECT id FROM extension_missions WHERE org_id = $1 AND kind = 'capture_profile'
               AND enabled AND target = $2 AND spec -> 'params' ->> 'person_id' = $3",
            vec![
                org_id.into(),
                target.clone().into(),
                person_id.to_string().into(),
            ],
        ))
        .await?
        .and_then(|r| r.try_get::<Uuid>("", "id").ok());
    if let Some(id) = open {
        return crate::services::extension::get_mission(db, org_id, id).await;
    }
    // shortcut: the cap counts per org and network, not per agent (the agent
    // is only known when it runs the capture); count then insert is racy by a
    // few rows, fine for an abuse cap.
    let today = db
        .query_one_raw(stmt(
            "SELECT count(*) AS n FROM extension_missions WHERE org_id = $1
               AND kind = 'capture_profile' AND target LIKE $2
               AND created_at > now() - interval '24 hours'",
            vec![org_id.into(), format!("{network}:profile:%").into()],
        ))
        .await?
        .and_then(|r| r.try_get::<i64>("", "n").ok())
        .unwrap_or(0);
    if today >= ba::PROFILE_CAPTURES_PER_DAY {
        return Err(ExtensionError::ProfileDailyCap {
            max: ba::PROFILE_CAPTURES_PER_DAY,
        });
    }
    let spec = MissionSpec::CaptureProfile(ba::CaptureProfileParams {
        url: url.clone(),
        person_id: person_id.to_string(),
    });
    let name: String = format!("Profile · {url}")
        .chars()
        .take(ba::NAME_MAX)
        .collect();
    let waiting = !crate::services::extension::any_online(db, org_id).await;
    let id = Uuid::new_v4();
    db.execute_raw(stmt(
        "INSERT INTO extension_missions (id, org_id, name, kind, spec, target, interval_secs,
            enabled, state_code, created_by)
         VALUES ($1, $2, $3, 'capture_profile', $4, $5, $6, true, $7, $8)",
        vec![
            id.into(),
            org_id.into(),
            name.into(),
            serde_json::to_value(&spec).unwrap_or_default().into(),
            target.into(),
            ba::MAX_INTERVAL_SECS.into(),
            waiting.then(|| err_codes::EXT_NO_AGENT.to_string()).into(),
            user_id.into(),
        ],
    ))
    .await?;
    crate::services::extension::get_mission(db, org_id, id).await
}

/// Refuses a capture whose person objected since it was asked; the mission
/// stops.
pub async fn check_still_allowed(
    db: &DatabaseConnection,
    org_id: i64,
    mission_id: Uuid,
    params: &ba::CaptureProfileParams,
) -> Result<(), ExtensionError> {
    let keys = [format!("person:{}", params.person_id), params.url.clone()];
    if opposed_any(db, org_id, &keys).await? {
        disable(db, mission_id).await?;
        return Err(ExtensionError::MissionNotFound);
    }
    Ok(())
}

async fn disable(db: &DatabaseConnection, mission_id: Uuid) -> Result<(), ExtensionError> {
    db.execute_raw(stmt(
        "UPDATE extension_missions SET enabled = false, state = 'idle' WHERE id = $1",
        vec![mission_id.into()],
    ))
    .await?;
    Ok(())
}

/// After the page is stored: the document is linked to the person, the
/// profile address becomes an account identifier of the person, and the
/// one-shot mission ends. An org without the investigation pack keeps the
/// document only.
pub async fn captured(
    ctx: &AppContext,
    agent: &AgentAuth,
    mission_id: Uuid,
    params: &ba::CaptureProfileParams,
    asset_id: Uuid,
) -> Result<(), ExtensionError> {
    crate::services::extension::link_publisher(ctx, agent, asset_id, &params.person_id).await;
    let actor = ontology::Actor {
        user_id: None,
        role: crate::models::_entities::sea_orm_active_enums::OrgMemberRole::Member,
        source_ref: format!("agent:{}", agent.device_id),
    };
    let value = serde_json::Value::String(params.url.clone());
    let identifier =
        match ontology::objects::find_by_key(&ctx.db, agent.org_id, "identifier", "value", &value)
            .await
        {
            Ok(Some(o)) => Some(o.id.to_string()),
            Ok(None) => {
                let mut properties = serde_json::Map::new();
                properties.insert("kind".into(), "account".into());
                properties.insert("value".into(), value);
                let input = pnex_core::ontology::api::ObjectInput {
                    type_key: "identifier".into(),
                    title: params.url.clone(),
                    properties,
                    expected_version: None,
                };
                ontology::objects::create(&ctx.db, agent.org_id, &actor, &input)
                    .await
                    .map(|o| o.id)
                    .ok()
            }
            Err(_) => None,
        };
    if let Some(identifier) = identifier {
        let input = pnex_core::ontology::api::LinkInput {
            link_type: "uses_identifier".into(),
            source_id: params.person_id.clone(),
            target_id: identifier,
            attributes: serde_json::Map::new(),
            valid_from: None,
            valid_to: None,
        };
        // An existing link (second capture of the profile) is refused: fine.
        let _ = ontology::links::create(&ctx.db, agent.org_id, &actor, &input).await;
    }
    disable(&ctx.db, mission_id).await
}

/// Deletes captured profile texts older than [`RAW_TEXT_DAYS`] (D333).
pub async fn prune_expired(ctx: &AppContext) -> Result<u64, ExtensionError> {
    let rows = ctx
        .db
        .query_all_raw(stmt(
            "SELECT id, org_id FROM media_assets
             WHERE metadata ->> 'profile_person_id' IS NOT NULL
               AND created_at < now() - make_interval(days => $1::int)
             LIMIT 500",
            vec![(RAW_TEXT_DAYS as i32).into()],
        ))
        .await?;
    let mut n = 0;
    for row in rows {
        let (Ok(id), Ok(org_id)) = (
            row.try_get::<Uuid>("", "id"),
            row.try_get::<i64>("", "org_id"),
        ) else {
            continue;
        };
        crate::services::voice_id::samples::delete_assets(ctx, org_id, &[id])
            .await
            .map_err(|e| ExtensionError::Db(sea_orm::DbErr::Custom(e.to_string())))?;
        n += 1;
    }
    Ok(n)
}

/// Pack and setting of the retention period (crm.md D444).
const RETENTION_SETTING: &str = "prospection_b2b.retention_mois";
/// Default period when the setting cannot be read.
const RETENTION_MONTHS: i64 = 36;
/// People proposed per org and per run.
const RETENTION_BATCH: i64 = 200;

/// The retention queue (crm.md D445): in each org where the prospecting
/// pack is installed, a live person with no change, no link opened or
/// closed and no profile capture for the org's period becomes an erasure
/// proposal, its last activity as evidence. A refusal counts as an
/// activity; one open proposal per person (its slot).
pub async fn propose_erasures(db: &DatabaseConnection) -> Result<u64, ExtensionError> {
    let pack = RETENTION_SETTING.split('.').next().unwrap_or_default();
    let orgs = db
        .query_all_raw(stmt(
            "SELECT org_id FROM packs WHERE key = $1",
            vec![pack.into()],
        ))
        .await?;
    let mut n = 0;
    for row in orgs {
        let Ok(org_id) = row.try_get::<i64>("", "org_id") else {
            continue;
        };
        let months = ontology::packs::setting_value(db, org_id, RETENTION_SETTING)
            .await?
            .and_then(|v| v.as_f64())
            .map_or(RETENTION_MONTHS, |m| m as i64);
        n += propose_erasures_in(db, org_id, months).await?;
    }
    Ok(n)
}

/// The retention queue of one org, `months` its period.
pub async fn propose_erasures_in(
    db: &DatabaseConnection,
    org_id: i64,
    months: i64,
) -> Result<u64, ExtensionError> {
    let rows = db
        .query_all_raw(stmt(
            "WITH activity AS (
               SELECT o.id, greatest(
                 o.updated_at,
                 (SELECT max(greatest(l.created_at, l.updated_at, coalesce(l.valid_to, l.valid_from)))
                    FROM resource_edges l
                   WHERE l.org_id = o.org_id AND (l.source_object_id = o.id OR l.target_object_id = o.id)),
                 (SELECT max(m.created_at) FROM media_assets m
                   WHERE m.org_id = o.org_id AND m.metadata ->> 'profile_person_id' = o.id::text),
                 (SELECT max(p.decided_at) FROM proposals p
                   WHERE p.org_id = o.org_id AND p.kind = 'object_erase' AND p.target_id = o.id::text
                     AND p.status = 'rejected')
               ) AS last
               FROM objects o
              WHERE o.org_id = $1 AND o.type_key = 'person' AND o.valid_to IS NULL
                AND o.erased_at IS NULL
                AND o.updated_at < now() - make_interval(months => $2::int)
                AND NOT EXISTS (SELECT 1 FROM proposals p
                   WHERE p.org_id = o.org_id AND p.kind = 'object_erase'
                     AND p.target_id = o.id::text AND p.status = 'proposed')
             )
             SELECT id, to_char(last, 'YYYY-MM-DD') AS last FROM activity
              WHERE last < now() - make_interval(months => $2::int)
              ORDER BY last LIMIT $3",
            vec![org_id.into(), (months as i32).into(), RETENTION_BATCH.into()],
        ))
        .await?;
    let mut n = 0;
    for row in rows {
        let (Ok(id), Ok(last)) = (
            row.try_get::<Uuid>("", "id"),
            row.try_get::<String>("", "last"),
        ) else {
            continue;
        };
        let mut values = serde_json::Map::new();
        values.insert("last_activity".into(), last.clone().into());
        let p = pnex_core::ontology::proposal::ProposalInput {
            kind: Some(pnex_core::ontology::proposal::ProposalKind::ObjectErase),
            type_key: "person".into(),
            target_id: Some(id.to_string()),
            values,
            evidence: serde_json::json!({"last_activity": last, "retention_months": months}),
            origin: format!("retention:{RETENTION_SETTING}"),
            ..Default::default()
        };
        match ontology::proposals::propose(db, org_id, &p).await {
            Ok(Some(_)) => n += 1,
            Ok(None) => {}
            Err(e) => tracing::warn!(error = %e, %id, "erasure not proposed"),
        }
    }
    Ok(n)
}

/// Background pruner, started at boot — skipped in tests.
pub fn spawn_pruner(ctx: &AppContext) {
    use loco_rs::config::WorkerMode;
    if matches!(ctx.config.workers.mode, WorkerMode::ForegroundBlocking) {
        return;
    }
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(PRUNE_INTERVAL_SECS));
        loop {
            tick.tick().await;
            if !crate::services::singleton::my_turn(
                &ctx.db,
                "profile-text-pruner",
                std::time::Duration::from_secs(PRUNE_INTERVAL_SECS),
            )
            .await
            {
                continue;
            }
            match prune_expired(&ctx).await {
                Ok(n) if n > 0 => tracing::info!(pruned = n, "profile text pruner"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "profile text pruner failed"),
            }
            match propose_erasures(&ctx.db).await {
                Ok(n) if n > 0 => tracing::info!(proposed = n, "retention queue"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "retention queue failed"),
            }
        }
    });
}
