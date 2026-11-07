//! Profile captures (account-mapping.md D333) and the prospecting
//! opposition list (D341, minimum). A human asks, from a `person`'s page,
//! one capture of one profile page on a social network; a browser agent of
//! the org opens it once in its signed-in session and uploads the page
//! text. The text becomes a document linked to the person (`published_by`),
//! and the profile address a `uses_identifier` account of the person. The
//! raw text is purged after [`RAW_TEXT_DAYS`].

use loco_rs::app::AppContext;
use pnex_core::browser_agent as ba;
use pnex_core::err_codes;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

use std::collections::BTreeMap;

use pnex_core::default_rules;

use crate::services::extension::ExtensionError;
use crate::services::ontology;
use crate::services::site_recipes::purpose::{self, Purpose};
use crate::services::site_recipes::{RecipeError, StartError};

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
fn capture_view(
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

fn start_error(e: StartError) -> ExtensionError {
    match e {
        StartError::NoAgent => ExtensionError::NoAgent,
        StartError::DailyCap { max } => ExtensionError::RunDailyCap { max },
        StartError::Recipe(RecipeError::Db(e)) => ExtensionError::Db(e),
        other => ExtensionError::Db(sea_orm::DbErr::Custom(format!("default rule: {other:?}"))),
    }
}

/// Asks one capture of `raw_url` for the person (D333): the default rule
/// of its network, played in the caller's browser agent (else a common
/// one). An open capture of the same profile is returned as is.
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
    let runs = purpose::runs_of(
        db,
        org_id,
        "profile_capture",
        "person_id",
        &person_id.to_string(),
    )
    .await?;
    if let Some((id, _, state, reason, at)) = runs.into_iter().find(|(_, p, state, _, _)| {
        matches!(p, Purpose::ProfileCapture { url: u, .. } if *u == url)
            && matches!(state.as_str(), "queued" | "running")
    }) {
        return Ok(capture_view(id, &url, &state, reason, at));
    }
    // shortcut: count then insert is racy by a few rows, fine for an abuse cap.
    let site = default_rules::network_site(network).unwrap_or_default();
    if purpose::count_today(db, org_id, "profile_capture", &format!("https://{site}/")).await?
        >= ba::PROFILE_CAPTURES_PER_DAY
    {
        return Err(ExtensionError::ProfileDailyCap {
            max: ba::PROFILE_CAPTURES_PER_DAY,
        });
    }
    let yaml = default_rules::profile_capture(network).ok_or(ExtensionError::NotProfile)?;
    let path = url
        .split_once("://")
        .and_then(|(_, rest)| rest.find('/').map(|i| rest[i..].to_string()))
        .unwrap_or_else(|| "/".into());
    let params = BTreeMap::from([("url".to_string(), url.clone()), ("path".to_string(), path)]);
    let purpose = Purpose::ProfileCapture {
        person_id: person_id.to_string(),
        url: url.clone(),
    };
    let run = purpose::start(ctx, org_id, Some(user_id), &yaml, params, &purpose)
        .await
        .map_err(start_error)?;
    let id = Uuid::parse_str(&run.id).unwrap_or_default();
    Ok(capture_view(id, &url, "queued", None, run.started_at))
}

/// Latest captures asked for the person, newest first.
pub async fn captures(
    db: &DatabaseConnection,
    org_id: i64,
    person_id: Uuid,
) -> Result<Vec<ba::ProfileCapture>, ExtensionError> {
    person(db, org_id, person_id).await?;
    let runs = purpose::runs_of(
        db,
        org_id,
        "profile_capture",
        "person_id",
        &person_id.to_string(),
    )
    .await?;
    Ok(runs
        .into_iter()
        .map(|(id, p, state, reason, at)| {
            let url = match p {
                Purpose::ProfileCapture { url, .. } => url,
                Purpose::PeopleCapture { url, .. } => url,
            };
            capture_view(id, &url, &state, reason, at)
        })
        .collect())
}

/// Stores a captured text as a document of the org, purged with the other
/// captured texts by `metadata` (D333, D779).
async fn store_text(
    ctx: &AppContext,
    org_id: i64,
    name: &str,
    url: &str,
    text: &str,
    metadata: serde_json::Value,
) -> Result<Uuid, ExtensionError> {
    use crate::models::_entities::media_assets;
    use sea_orm::{ActiveModelTrait, Set};
    let mut meta = serde_json::json!({ "source": "browser_agent", "source_url": url });
    if let (Some(m), Some(extra)) = (meta.as_object_mut(), metadata.as_object()) {
        m.extend(extra.clone());
    }
    let name: String = name.chars().take(ba::NAME_MAX).collect();
    let asset = media_assets::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        kind: Set("document".into()),
        name: Set(name.clone()),
        description: Set(None),
        metadata: Set(Some(meta)),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await?;
    let markdown = format!("# {name}\n\n<{url}>\n\n{}\n", text.trim());
    let settings = crate::services::media::MediaSettings::from_config(&ctx.config);
    crate::controllers::media::write_version(
        ctx,
        org_id,
        &asset,
        crate::controllers::media::IncomingVersion {
            filename: "page.md".into(),
            content_type: "text/markdown".into(),
            note: None,
            bytes: markdown.into_bytes().into(),
        },
        &settings,
    )
    .await
    .map_err(|e| ExtensionError::Db(sea_orm::DbErr::Custom(format!("captured text: {e}"))))?;
    Ok(asset.id)
}

/// Property of `prospecting_b2b` naming the captured text a flow has yet to
/// read (account-mapping.md D538).
const PROFILE_TO_READ: &str = "profile_to_read";

/// Written in place when the org has the property: a stamp, not an edit of
/// the person.
async fn stamp_to_read(
    ctx: &AppContext,
    org_id: i64,
    person_id: &str,
    asset_id: Uuid,
) -> Result<(), ExtensionError> {
    let Ok(person) = Uuid::parse_str(person_id) else {
        return Ok(());
    };
    let known = ontology::schema(&ctx.db, org_id)
        .await
        .ok()
        .and_then(|s| {
            s.object_type("person")
                .map(|t| t.properties.iter().any(|p| p.key == PROFILE_TO_READ))
        })
        .unwrap_or(false);
    if known {
        ctx.db
            .execute_raw(stmt(
                "UPDATE objects SET properties = properties || jsonb_build_object($3::text, $4::text)
                 WHERE org_id = $1 AND id = $2",
                vec![
                    org_id.into(),
                    person.into(),
                    PROFILE_TO_READ.into(),
                    asset_id.to_string().into(),
                ],
            ))
            .await?;
    }
    Ok(())
}

/// A profile captured by its rule run: unless the person or the address
/// objected since, its text becomes a document of the person, stamped to be
/// read, and the address an account identifier of theirs.
pub async fn profile_captured(
    ctx: &AppContext,
    org_id: i64,
    source_ref: &str,
    person_id: &str,
    url: &str,
    text: &str,
) -> Result<(), ExtensionError> {
    let keys = [format!("person:{person_id}"), url.to_string()];
    if opposed_any(&ctx.db, org_id, &keys).await? {
        return Ok(());
    }
    let meta = serde_json::json!({ "profile_person_id": person_id });
    let asset = store_text(ctx, org_id, &format!("Profile · {url}"), url, text, meta).await?;
    attach_profile(ctx, org_id, source_ref, person_id, url, asset).await
}

/// The captured profile `url` becomes the person's: its document linked to
/// them, stamped to be read, the address an account identifier of theirs.
/// A second attach of the same profile changes nothing.
pub(crate) async fn attach_profile(
    ctx: &AppContext,
    org_id: i64,
    source_ref: &str,
    person_id: &str,
    url: &str,
    asset_id: Uuid,
) -> Result<(), ExtensionError> {
    crate::services::extension::link_publisher_as(ctx, org_id, source_ref, asset_id, person_id)
        .await;
    stamp_to_read(ctx, org_id, person_id, asset_id).await?;
    let actor = actor_of(source_ref);
    if let Some(identifier) = identifier_of(&ctx.db, org_id, source_ref, url).await {
        let input = pnex_core::ontology::api::LinkInput {
            link_type: "uses_identifier".into(),
            source_id: person_id.to_string(),
            target_id: identifier,
            attributes: serde_json::Map::new(),
            valid_from: None,
            valid_to: None,
        };
        // An existing link (second capture of the profile) is refused: fine.
        let _ = ontology::links::create(&ctx.db, org_id, &actor, &input).await;
    }
    Ok(())
}

fn actor_of(source_ref: &str) -> ontology::Actor {
    ontology::Actor {
        user_id: None,
        role: crate::models::_entities::sea_orm_active_enums::OrgMemberRole::Member,
        source_ref: source_ref.to_string(),
    }
}

/// The `identifier` object of a profile address, created when missing;
/// `None` in an org without the investigation pack.
pub(crate) async fn identifier_of(
    db: &DatabaseConnection,
    org_id: i64,
    source_ref: &str,
    url: &str,
) -> Option<String> {
    let value = serde_json::Value::String(url.to_string());
    match ontology::objects::find_by_key(db, org_id, "identifier", "value", &value).await {
        Ok(Some(o)) => Some(o.id.to_string()),
        Ok(None) => {
            let mut properties = serde_json::Map::new();
            properties.insert("kind".into(), "account".into());
            properties.insert("value".into(), value);
            let input = pnex_core::ontology::api::ObjectInput {
                type_key: "identifier".into(),
                title: url.to_string(),
                properties,
                expected_version: None,
            };
            ontology::objects::create(db, org_id, &actor_of(source_ref), &input)
                .await
                .map(|o| o.id.to_string())
                .ok()
        }
        Err(_) => None,
    }
}

/// A live object of the org whose type declares [`PEOPLE_TO_READ`]: the
/// pack that reads employee lists says which types have one (D441).
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
    Ok(o.title)
}

/// Asks one capture of a company's employee list on LinkedIn for the
/// organization (D779): the default rule, played in the caller's browser
/// agent. An open capture of the same list is returned as is. The people it
/// names become list contacts to validate, read by a flow of the
/// prospecting pack; nobody objecting at that company comes back.
pub async fn request_people(
    ctx: &AppContext,
    org_id: i64,
    user_id: i64,
    organization_id: Uuid,
    raw_url: &str,
) -> Result<ba::ProfileCapture, ExtensionError> {
    let db = &ctx.db;
    organization(db, org_id, organization_id).await?;
    let url = ba::people_page_of(raw_url).map_err(|r| match r {
        ba::ProfileRefusal::NotNetwork => ExtensionError::Invalid {
            field: "url".into(),
            token: err_codes::FIELD_INVALID.into(),
        },
        ba::ProfileRefusal::NotProfile => ExtensionError::NotPeoplePage,
    })?;
    let org_key = organization_id.to_string();
    let runs = purpose::runs_of(db, org_id, "people_capture", "organization_id", &org_key).await?;
    if let Some((id, _, state, reason, at)) = runs.into_iter().find(|(_, p, state, _, _)| {
        matches!(p, Purpose::PeopleCapture { url: u, .. } if *u == url)
            && matches!(state.as_str(), "queued" | "running")
    }) {
        return Ok(capture_view(id, &url, &state, reason, at));
    }
    // shortcut: count then insert is racy by a few rows, fine for an abuse cap.
    if purpose::count_today(db, org_id, "people_capture", "https://").await?
        >= ba::PEOPLE_CAPTURES_PER_DAY
    {
        return Err(ExtensionError::PeopleDailyCap {
            max: ba::PEOPLE_CAPTURES_PER_DAY,
        });
    }
    let params = BTreeMap::from([("url".to_string(), url.clone())]);
    let purpose = Purpose::PeopleCapture {
        organization_id: org_key,
        url: url.clone(),
    };
    let run = purpose::start(
        ctx,
        org_id,
        Some(user_id),
        &default_rules::people_capture(),
        params,
        &purpose,
    )
    .await
    .map_err(start_error)?;
    let id = Uuid::parse_str(&run.id).unwrap_or_default();
    Ok(capture_view(id, &url, "queued", None, run.started_at))
}

/// Latest employee list captures asked for the organization, newest first.
pub async fn people_captures(
    db: &DatabaseConnection,
    org_id: i64,
    organization_id: Uuid,
) -> Result<Vec<ba::ProfileCapture>, ExtensionError> {
    organization(db, org_id, organization_id).await?;
    let runs = purpose::runs_of(
        db,
        org_id,
        "people_capture",
        "organization_id",
        &organization_id.to_string(),
    )
    .await?;
    Ok(runs
        .into_iter()
        .map(|(id, p, state, reason, at)| {
            let url = match p {
                Purpose::PeopleCapture { url, .. } | Purpose::ProfileCapture { url, .. } => url,
            };
            capture_view(id, &url, &state, reason, at)
        })
        .collect())
}

/// Property of `prospecting_b2b` naming the captured employee list a flow
/// has yet to read (D779).
const PEOPLE_TO_READ: &str = "people_to_read";

/// An employee list captured by its rule run: a document of the
/// organization, marked to read when the org has the prospecting pack.
pub async fn people_captured(
    ctx: &AppContext,
    org_id: i64,
    source_ref: &str,
    organization_id: &str,
    url: &str,
    text: &str,
) -> Result<(), ExtensionError> {
    let Ok(id) = Uuid::parse_str(organization_id) else {
        return Ok(());
    };
    // The type may have lost the property since the capture was asked.
    if organization(&ctx.db, org_id, id).await.is_err() {
        return Ok(());
    }
    let meta = serde_json::json!({ "people_org_id": organization_id });
    let asset = store_text(ctx, org_id, &format!("Employees · {url}"), url, text, meta).await?;
    crate::services::extension::link_publisher_as(ctx, org_id, source_ref, asset, organization_id)
        .await;
    // Written in place: a stamp, not an edit of the organization.
    ctx.db
        .execute_raw(stmt(
            "UPDATE objects SET properties = properties || jsonb_build_object($3::text, $4::text)
             WHERE org_id = $1 AND id = $2 AND valid_to IS NULL",
            vec![
                org_id.into(),
                id.into(),
                PEOPLE_TO_READ.into(),
                asset.to_string().into(),
            ],
        ))
        .await?;
    Ok(())
}

/// Deletes captured profile texts and employee lists older than
/// [`RAW_TEXT_DAYS`] (D333, D779).
pub async fn prune_expired(ctx: &AppContext) -> Result<u64, ExtensionError> {
    let rows = ctx
        .db
        .query_all_raw(stmt(
            "SELECT id, org_id FROM media_assets
             WHERE (metadata ->> 'profile_person_id' IS NOT NULL
                    OR metadata ->> 'people_org_id' IS NOT NULL)
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
const RETENTION_SETTING: &str = "prospecting_b2b.retention_months";
/// Default period when the setting cannot be read.
const RETENTION_MONTHS: i64 = 36;
/// People proposed per org and per run.
const RETENTION_BATCH: i64 = 200;
/// Types the queue proposes: a person, and what names one before they are in
/// the graph, a lead (crm.md D514) or a contact from a list (D610). A type
/// missing from the org matches nothing.
const RETENTION_TYPES: [&str; 3] = ["person", "lead", "list_contact"];

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
                RETENTION_TYPES.map(String::from).to_vec().into(),
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
        }
    });
}
