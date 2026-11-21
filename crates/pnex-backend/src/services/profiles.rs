//! Profile captures (crm.md D333) and the prospecting
//! opposition list (D341, minimum). A human asks, from a `person`'s page,
//! one capture of one profile page on a social network; a browser agent of
//! the org opens it once in its signed-in session and uploads the page
//! text. The text becomes a document linked to the person (`published_by`),
//! and the profile address a `uses_identifier` account of the person. The
//! raw text is purged after [`RAW_TEXT_DAYS`].

use loco_rs::app::AppContext;
use pnex_core::browser_agent as ba;
use pnex_core::err_codes;
use pnex_core::ontology::roles;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

use std::collections::BTreeMap;

use crate::services::extension::ExtensionError;
use crate::services::ontology;
use crate::services::site_recipes::purpose;

/// Days a captured profile text is kept (D333).
pub const RAW_TEXT_DAYS: i64 = 30;
const PRUNE_INTERVAL_SECS: u64 = 3600;

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

pub(crate) fn person_key(person_id: Uuid) -> String {
    format!("person:{person_id}")
}

/// The live `person` object of the org.
pub(crate) async fn person(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
) -> Result<(), ExtensionError> {
    let s = ontology::schema(db, org_id)
        .await
        .map_err(|e| ExtensionError::Db(sea_orm::DbErr::Custom(e.to_string())))?;
    match ontology::objects::find(db, org_id, id).await {
        Ok(o) if s.type_role(&o.type_key) == Some(roles::PERSON) && o.valid_to.is_none() => Ok(()),
        Ok(_) | Err(ontology::OntologyError::NotFound) => Err(ExtensionError::PersonNotFound),
        Err(e) => Err(ExtensionError::Db(sea_orm::DbErr::Custom(e.to_string()))),
    }
}

/// Whether the person, or this profile address, is on the opposition list.
pub(crate) async fn opposed_any(
    db: &impl ConnectionTrait,
    org_id: i64,
    keys: &[String],
) -> Result<bool, ExtensionError> {
    Ok(db
        .query_one_raw(stmt(
            "SELECT 1 AS x FROM prospecting_oppositions WHERE org_id = $1 AND fingerprint = ANY($2)",
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
/// date lands on the person (`opposed_on`) for the prospecting flows.
pub async fn oppose(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: i64,
    person_id: Uuid,
) -> Result<(), ExtensionError> {
    person(db, org_id, person_id).await?;
    oppose_by(db, org_id, Some(user_id), person_id).await
}

/// Property of `prospecting_b2b` holding the objection date.
const OPPOSED_ON: &str = "opposed_on";

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
        .type_key(roles::PERSON)
        .and_then(|k| s.object_type(k))
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
    // Its addresses and numbers stop flows creating anything from them (D610),
    // and a draft nobody sent yet is discarded.
    let contacts = db
        .query_all_raw(stmt(
            "SELECT lower(i.properties ->> 'value') AS value FROM resource_edges l
             JOIN objects i ON i.id = l.target_object_id AND i.org_id = l.org_id
             WHERE l.org_id = $1 AND l.source_object_id = $2 AND l.relation = 'uses_identifier'
               AND i.properties ->> 'kind' IN ('email', 'phone')",
            vec![org_id.into(), person_id.into()],
        ))
        .await?;
    for row in contacts {
        let Ok(Some(v)) = row.try_get::<Option<String>>("", "value") else {
            continue;
        };
        keys.push(ontology::opposition::contact_key(org_id, &v));
        db.execute_raw(stmt(
            "UPDATE mail_outbox SET state = 'discarded'
             WHERE org_id = $1 AND state = 'draft'
               AND EXISTS (SELECT 1 FROM unnest(to_addrs) a WHERE lower(a) = $2)",
            vec![org_id.into(), v.into()],
        ))
        .await?;
    }
    for key in keys {
        db.execute_raw(stmt(
            "INSERT INTO prospecting_oppositions (org_id, fingerprint, created_by)
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

/// The mails of an erased address (mail.md D461, crm.md D610): received ones
/// whose correspondent it is (the original of a trusted forward, else the
/// sender), never a member's forward of someone else's mail; outgoing ones,
/// except one being handed to the server right now.
/// A sent mail goes too, so PneX keeps no proof of it (Shan, 2026-11-04). An
/// outgoing mail to several people only loses this recipient and keeps its body.
async fn forget_mail(
    db: &impl ConnectionTrait,
    org_id: i64,
    addr: &str,
) -> Result<(), ExtensionError> {
    db.execute_raw(stmt(
        "DELETE FROM mail_messages WHERE org_id = $1 AND \
           CASE WHEN COALESCE((forwarded ->> 'trusted')::boolean, false) \
             THEN lower(forwarded -> 'from' ->> 'address') ELSE from_address END = $2",
        vec![org_id.into(), addr.into()],
    ))
    .await?;
    db.execute_raw(stmt(
        "DELETE FROM mail_outbox WHERE org_id = $1 AND state <> 'sending'
           AND EXISTS (SELECT 1 FROM unnest(to_addrs) a WHERE lower(a) = $2)
           AND NOT EXISTS (SELECT 1 FROM unnest(to_addrs) a WHERE lower(a) <> $2)",
        vec![org_id.into(), addr.into()],
    ))
    .await?;
    db.execute_raw(stmt(
        "UPDATE mail_outbox SET to_addrs = ARRAY(SELECT a FROM unnest(to_addrs) a WHERE lower(a) <> $2)
         WHERE org_id = $1 AND state <> 'sending'
           AND EXISTS (SELECT 1 FROM unnest(to_addrs) a WHERE lower(a) = $2)",
        vec![org_id.into(), addr.into()],
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
    // Whatever holds an address: an identifier, a lead from a mailbox (D610).
    let s = ontology::schema(&ctx.db, org_id)
        .await
        .map_err(|e| ExtensionError::Db(sea_orm::DbErr::Custom(e.to_string())))?;
    if let Some(def) = s.object_type(&row.type_key) {
        for addr in ontology::opposition::contact_values(def, &row.properties)
            .iter()
            .filter(|v| v.contains('@'))
        {
            forget_mail(&ctx.db, org_id, addr).await?;
        }
    }
    if s.type_role(&row.type_key) != Some(roles::PERSON) {
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
    // Its identifiers are returned and erased next: their addresses take the
    // mails with them (`forget_mail`).
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

/// A capture as listed on the person's or the company's page, read from
/// its rule run (D780).
pub(crate) fn capture_view(
    id: Uuid,
    url: &str,
    state: &str,
    reason: Option<String>,
    created_at: String,
) -> ba::ProfileCapture {
    let (mission_state, open) = match state {
        "queued" => (ba::MissionState::Idle, true),
        "running" => (ba::MissionState::Running, true),
        "succeeded" => (ba::MissionState::Idle, false),
        _ => (ba::MissionState::Blocked, false),
    };
    ba::ProfileCapture {
        id: id.to_string(),
        url: url.to_string(),
        state: mission_state,
        state_code: reason,
        open,
        captured: state == "succeeded",
        created_at,
    }
}

/// Pipelines of the prospecting pack that capture a profile, one per network
/// (`profile_capture_<network>`, D996).
const PROFILE_PIPELINES: &str = "prospecting_b2b/profile_capture_";

/// The view of the navigation a pipeline row started, else of the refusal.
pub(crate) fn asked_view(
    out: &crate::services::pipeline_engine::RunOutcome,
    url: &str,
) -> Result<(), ExtensionError> {
    let failure = out
        .failure
        .as_ref()
        .or_else(|| out.items.first().and_then(|(_, f, _)| f.as_ref()))
        .filter(|f| f.code != crate::services::pipeline_engine::WAITING);
    match failure.map(|f| (f.code, f.args.clone())) {
        None => Ok(()),
        Some(("no_agent", _)) => Err(ExtensionError::NoAgent),
        Some(("daily_cap", args)) => Err(ExtensionError::RunDailyCap {
            max: args["max"].as_i64().unwrap_or_default(),
        }),
        Some((code, _)) => Err(ExtensionError::Db(sea_orm::DbErr::Custom(format!(
            "capture of {url} not started: {code}"
        )))),
    }
}

/// Asks one capture of `raw_url` for the person (D333): the pipeline of its
/// network, played in the caller's browser agent (else a common one). An
/// open capture of the same profile is returned as is.
pub async fn request(
    ctx: &AppContext,
    org_id: i64,
    user_id: i64,
    person_id: Uuid,
    raw_url: &str,
) -> Result<ba::ProfileCapture, ExtensionError> {
    let db = &ctx.db;
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
    let person_key = person_id.to_string();
    let open = |runs: Vec<(
        Uuid,
        BTreeMap<String, String>,
        String,
        Option<String>,
        String,
    )>| {
        runs.into_iter().find(|(_, p, state, _, _)| {
            p.get("url") == Some(&url) && matches!(state.as_str(), "queued" | "running")
        })
    };
    let runs =
        purpose::pipeline_runs(db, org_id, PROFILE_PIPELINES, "person_id", &person_key).await?;
    if let Some((id, _, state, reason, at)) = open(runs) {
        return Ok(capture_view(id, &url, &state, reason, at));
    }
    let pipeline = format!("{PROFILE_PIPELINES}{network}");
    let site = url
        .split_once("://")
        .map(|(scheme, rest)| format!("{scheme}://{}/", rest.split('/').next().unwrap_or_default()))
        .unwrap_or_default();
    // shortcut: count then insert is racy by a few rows, fine for an abuse cap.
    if purpose::pipeline_runs_today(db, org_id, PROFILE_PIPELINES, &site).await?
        >= ba::PROFILE_CAPTURES_PER_DAY
    {
        return Err(ExtensionError::ProfileDailyCap {
            max: ba::PROFILE_CAPTURES_PER_DAY,
        });
    }
    let path = url
        .split_once("://")
        .and_then(|(_, rest)| rest.find('/').map(|i| rest[i..].to_string()))
        .unwrap_or_else(|| "/".into());
    let row = serde_json::Map::from_iter([
        ("url".to_string(), url.clone().into()),
        ("person_id".to_string(), person_key.clone().into()),
        ("path".to_string(), path.into()),
    ]);
    let out = crate::services::pipeline_engine::store::run_asked(
        ctx,
        org_id,
        Some(user_id),
        &pipeline,
        vec![row],
    )
    .await
    .map_err(|e| {
        ExtensionError::Db(sea_orm::DbErr::Custom(format!(
            "profile pipeline: {}",
            e.code()
        )))
    })?;
    asked_view(&out, &url)?;
    let runs =
        purpose::pipeline_runs(db, org_id, PROFILE_PIPELINES, "person_id", &person_key).await?;
    let (id, _, state, reason, at) = runs
        .into_iter()
        .find(|(_, p, _, _, _)| p.get("url") == Some(&url))
        .ok_or_else(|| ExtensionError::Db(sea_orm::DbErr::Custom("capture not recorded".into())))?;
    Ok(capture_view(id, &url, &state, reason, at))
}

/// Latest captures asked for the person, newest first.
pub async fn captures(
    db: &DatabaseConnection,
    org_id: i64,
    person_id: Uuid,
) -> Result<Vec<ba::ProfileCapture>, ExtensionError> {
    person(db, org_id, person_id).await?;
    let runs = purpose::pipeline_runs(
        db,
        org_id,
        PROFILE_PIPELINES,
        "person_id",
        &person_id.to_string(),
    )
    .await?;
    Ok(runs
        .into_iter()
        .map(|(id, p, state, reason, at)| {
            let url = p.get("url").cloned().unwrap_or_default();
            capture_view(id, &url, &state, reason, at)
        })
        .collect())
}

/// A live object of the org whose type declares [`PEOPLE_TO_READ`]: the
/// pack that reads employee lists says which types have one (D441). Its
/// type.
async fn organization(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
) -> Result<String, ExtensionError> {
    let o = match ontology::objects::find(db, org_id, id).await {
        Ok(o) if o.valid_to.is_none() => o,
        Ok(_) | Err(ontology::OntologyError::NotFound) => {
            return Err(ExtensionError::OrganizationNotFound)
        }
        Err(e) => return Err(ExtensionError::Db(sea_orm::DbErr::Custom(e.to_string()))),
    };
    let declared = ontology::schema(db, org_id)
        .await
        .ok()
        .and_then(|s| {
            s.object_type(&o.type_key)
                .map(|t| t.properties.iter().any(|p| p.key == PEOPLE_TO_READ))
        })
        .unwrap_or(false);
    if !declared {
        return Err(ExtensionError::OrganizationNotFound);
    }
    Ok(o.type_key)
}

/// Pipeline of the prospecting pack that captures an employee list (D996).
const PEOPLE_PIPELINE: &str = "prospecting_b2b/people_capture_linkedin";

/// Asks one capture of a company's employee list on LinkedIn for the
/// organization (D779): the pipeline of the prospecting pack, played in the
/// caller's browser agent. An open capture of the same list is returned as
/// is. The people it names become list contacts to validate, read by a flow
/// of the prospecting pack; nobody objecting at that company comes back.
pub async fn request_people(
    ctx: &AppContext,
    org_id: i64,
    user_id: i64,
    organization_id: Uuid,
    raw_url: &str,
) -> Result<ba::ProfileCapture, ExtensionError> {
    let db = &ctx.db;
    let type_key = organization(db, org_id, organization_id).await?;
    let url = ba::people_page_of(raw_url).map_err(|r| match r {
        ba::ProfileRefusal::NotNetwork => ExtensionError::Invalid {
            field: "url".into(),
            token: err_codes::FIELD_INVALID.into(),
        },
        ba::ProfileRefusal::NotProfile => ExtensionError::NotPeoplePage,
    })?;
    let org_key = organization_id.to_string();
    let runs =
        purpose::pipeline_runs(db, org_id, PEOPLE_PIPELINE, "organization_id", &org_key).await?;
    if let Some((id, _, state, reason, at)) = runs.into_iter().find(|(_, p, state, _, _)| {
        p.get("url") == Some(&url) && matches!(state.as_str(), "queued" | "running")
    }) {
        return Ok(capture_view(id, &url, &state, reason, at));
    }
    // shortcut: count then insert is racy by a few rows, fine for an abuse cap.
    if purpose::pipeline_runs_today(db, org_id, PEOPLE_PIPELINE, "https://").await?
        >= ba::PEOPLE_CAPTURES_PER_DAY
    {
        return Err(ExtensionError::PeopleDailyCap {
            max: ba::PEOPLE_CAPTURES_PER_DAY,
        });
    }
    let row = serde_json::Map::from_iter([
        ("url".to_string(), url.clone().into()),
        ("organization_id".to_string(), org_key.clone().into()),
        ("type_key".to_string(), type_key.into()),
    ]);
    let out = crate::services::pipeline_engine::store::run_asked(
        ctx,
        org_id,
        Some(user_id),
        PEOPLE_PIPELINE,
        vec![row],
    )
    .await
    .map_err(|e| {
        ExtensionError::Db(sea_orm::DbErr::Custom(format!(
            "people pipeline: {}",
            e.code()
        )))
    })?;
    asked_view(&out, &url)?;
    let runs =
        purpose::pipeline_runs(db, org_id, PEOPLE_PIPELINE, "organization_id", &org_key).await?;
    let (id, _, state, reason, at) = runs
        .into_iter()
        .find(|(_, p, _, _, _)| p.get("url") == Some(&url))
        .ok_or_else(|| ExtensionError::Db(sea_orm::DbErr::Custom("capture not recorded".into())))?;
    Ok(capture_view(id, &url, &state, reason, at))
}

/// Latest employee list captures asked for the organization, newest first.
pub async fn people_captures(
    db: &DatabaseConnection,
    org_id: i64,
    organization_id: Uuid,
) -> Result<Vec<ba::ProfileCapture>, ExtensionError> {
    organization(db, org_id, organization_id).await?;
    let runs = purpose::pipeline_runs(
        db,
        org_id,
        PEOPLE_PIPELINE,
        "organization_id",
        &organization_id.to_string(),
    )
    .await?;
    Ok(runs
        .into_iter()
        .map(|(id, p, state, reason, at)| {
            let url = p.get("url").cloned().unwrap_or_default();
            capture_view(id, &url, &state, reason, at)
        })
        .collect())
}

/// Property of `prospecting_b2b` naming the captured employee list a flow
/// has yet to read (D779).
const PEOPLE_TO_READ: &str = "people_to_read";

/// Deletes captured profile texts and employee lists older than
/// [`RAW_TEXT_DAYS`] (D333, D779).
pub async fn prune_expired(ctx: &AppContext) -> Result<u64, ExtensionError> {
    let rows = ctx
        .db
        .query_all_raw(stmt(
            "SELECT id, org_id FROM media_assets
             WHERE ((metadata ->> 'profile_person_id' IS NOT NULL
                     OR metadata ->> 'people_org_id' IS NOT NULL)
                    AND created_at < now() - make_interval(days => $1::int))
                -- Kept by a pipeline for a while (`write: media`, `keep_days`).
                OR (metadata ->> 'purge_after')::date < current_date
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
const RETENTION_SETTING: &str = "prospecting_b2b.retention_months";
/// Default period when the setting cannot be read.
const RETENTION_MONTHS: i64 = 36;
/// People proposed per org and per run.
const RETENTION_BATCH: i64 = 200;
/// Types the queue proposes besides the person role: what names one before
/// they are in the graph, a lead (crm.md D514) or a contact from a list (D610).
/// A type missing from the org matches nothing.
const RETENTION_TYPES: [&str; 2] = ["lead", "list_contact"];

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
    let s = ontology::schema(db, org_id)
        .await
        .map_err(|e| ExtensionError::Db(sea_orm::DbErr::Custom(e.to_string())))?;
    let mut types: Vec<String> = RETENTION_TYPES.map(String::from).to_vec();
    types.extend(s.type_key(roles::PERSON).map(str::to_string));
    let rows = db
        .query_all_raw(stmt(
            "WITH activity AS (
               SELECT o.id, o.type_key, greatest(
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
              WHERE o.org_id = $1 AND o.type_key = ANY($4) AND o.valid_to IS NULL
                AND o.erased_at IS NULL
                AND o.updated_at < now() - make_interval(months => $2::int)
                AND NOT EXISTS (SELECT 1 FROM proposals p
                   WHERE p.org_id = o.org_id AND p.kind = 'object_erase'
                     AND p.target_id = o.id::text AND p.status = 'proposed')
             )
             SELECT id, type_key, to_char(last, 'YYYY-MM-DD') AS last FROM activity
              WHERE last < now() - make_interval(months => $2::int)
              ORDER BY last LIMIT $3",
            vec![
                org_id.into(),
                (months as i32).into(),
                RETENTION_BATCH.into(),
                types.into(),
            ],
        ))
        .await?;
    let mut n = 0;
    for row in rows {
        let (Ok(id), Ok(type_key), Ok(last)) = (
            row.try_get::<Uuid>("", "id"),
            row.try_get::<String>("", "type_key"),
            row.try_get::<String>("", "last"),
        ) else {
            continue;
        };
        let mut values = serde_json::Map::new();
        values.insert("last_activity".into(), last.clone().into());
        let p = pnex_core::ontology::proposal::ProposalInput {
            kind: Some(pnex_core::ontology::proposal::ProposalKind::ObjectErase),
            type_key,
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
            // analysis.md D826: queries and their frozen objects kept a day.
            if let Ok(store) = crate::services::datasets::store_of(&ctx.config) {
                if let Err(e) = crate::services::analysis::purge(&ctx.db, &store).await {
                    tracing::warn!(error = %e, "analysis purge failed");
                }
            }
            // governance.md D804: personal data in new table versions, proposed only.
            if let Ok(store) = crate::services::datasets::store_of(&ctx.config) {
                match crate::services::governance::detect_pass(&ctx.db, &store).await {
                    Ok(n) if n > 0 => tracing::info!(proposed = n, "personal data detection"),
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "personal data detection failed"),
                }
            }
        }
    });
}
