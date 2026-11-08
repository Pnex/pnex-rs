//! A company created from its SEC number found again by its LEI (D812): the
//! SEC name is looked up in GLEIF, and a single candidate with the same
//! legal name becomes a proposal, never a write. If the org already knows
//! that LEI on another organization, the proposal is a `same_company_as`
//! link between the two; otherwise it is the LEI on the SEC company.

use loco_rs::app::AppContext;
use pnex_core::markets::same_company;
use pnex_core::ontology::proposal::{ProposalInput, ProposalKind};
use sea_orm::{ConnectionTrait, DatabaseBackend, FromQueryResult, Statement};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::services::ontology::imports::source;
use crate::services::ontology::{objects, Actor};
use pnex_core::ontology::api::ObjectInput;

pub const GLEIF_BASE: &str = "https://api.gleif.org";
/// Companies looked up per sweep, all orgs together.
const PER_SWEEP: i64 = 10;
const STATE: &str = "lei_state";
const ORIGIN: &str = "markets.gleif";
const LINK: &str = "same_company_as";

#[derive(Debug, FromQueryResult)]
struct Pending {
    id: Uuid,
    org_id: i64,
    title: String,
    version: i32,
    properties: Value,
}

/// One pass over the SEC companies that have no LEI and were never looked
/// up. A failed request leaves the company for a later pass.
pub async fn sweep(ctx: &AppContext, base: &str) -> Result<(), sea_orm::DbErr> {
    let due = Pending::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT id, org_id, title, version, properties FROM objects \
         WHERE type_key = 'organization' AND valid_to IS NULL \
           AND properties ? 'cik' AND NOT properties ? 'lei' AND NOT properties ? $1 \
         ORDER BY org_id, id LIMIT $2",
        vec![STATE.into(), PER_SWEEP.into()],
    ))
    .all(&ctx.db)
    .await?;
    for company in due {
        look_up(&ctx.db, base, &company).await;
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    Ok(())
}

async fn look_up(db: &sea_orm::DatabaseConnection, base: &str, company: &Pending) {
    let Ok(mut url) = source::check_url(&format!("{base}/api/v1/lei-records")) else {
        return;
    };
    url.query_pairs_mut()
        .append_pair("filter[fulltext]", &company.title)
        .append_pair("filter[entity.legalAddress.country]", "US")
        .append_pair("page[size]", "10");
    let body = match source::get_as(&url, &source::Declared::default()).await {
        Ok(b) => b,
        Err(code) => {
            tracing::info!(company = %company.id, code, "gleif not read");
            return;
        }
    };
    let found = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|d| same_company::candidates(&d));
    let Some(found) = found else {
        tracing::info!(company = %company.id, "gleif answer unreadable");
        return;
    };
    let hit = same_company::unique_match(&company.title, &found);
    let state = match hit {
        None if found.is_empty() => "none",
        None => "ambiguous",
        Some(_) => "proposed",
    };
    let mut properties = company.properties.as_object().cloned().unwrap_or_default();
    properties.insert(STATE.into(), state.into());
    let input = ObjectInput {
        type_key: "organization".into(),
        title: company.title.clone(),
        properties,
        expected_version: Some(company.version),
    };
    let actor = Actor {
        user_id: None,
        role: crate::models::_entities::sea_orm_active_enums::OrgMemberRole::Member,
        source_ref: format!("{ORIGIN}:{}", company.org_id),
    };
    // The state first: the proposal is made on the version it leaves, the
    // one an acceptance checks. A member editing meanwhile: the next pass.
    let version = match objects::update(db, company.org_id, &actor, company.id, &input).await {
        Ok(v) => v.version,
        Err(e) => {
            tracing::info!(company = %company.id, error = %e, "lei state not recorded");
            return;
        }
    };
    if let Some(c) = hit {
        if let Err(e) = propose(db, company, version, c, url.as_str()).await {
            tracing::warn!(company = %company.id, error = %e, "lei proposal not written");
        }
    }
}

async fn propose(
    db: &sea_orm::DatabaseConnection,
    company: &Pending,
    version: i32,
    c: &same_company::Candidate,
    asked: &str,
) -> Result<(), crate::services::ontology::OntologyError> {
    let holder = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT id FROM objects WHERE org_id = $1 AND type_key = 'organization' \
               AND valid_to IS NULL AND properties ->> 'lei' = $2 AND id <> $3 LIMIT 1",
            vec![
                company.org_id.into(),
                c.lei.clone().into(),
                company.id.into(),
            ],
        ))
        .await?
        .map(|r| r.try_get::<Uuid>("", "id"))
        .transpose()?;
    let evidence = json!({ "lei": c.lei, "legal_name": c.legal_name, "source": asked });
    let input = match holder {
        Some(other) => ProposalInput {
            kind: Some(ProposalKind::LinkCreate),
            type_key: LINK.into(),
            target_id: None,
            source_id: Some(company.id.to_string()),
            link_target_id: Some(other.to_string()),
            title: Some(format!("{} = {}", company.title, c.legal_name)),
            values: Map::from_iter([("lei".to_string(), json!(c.lei))]),
            base_version: None,
            evidence,
            origin: ORIGIN.into(),
            confidence: Some(90.0),
        },
        None => ProposalInput {
            kind: Some(ProposalKind::ObjectUpdate),
            type_key: "organization".into(),
            target_id: Some(company.id.to_string()),
            source_id: None,
            link_target_id: None,
            title: Some(company.title.clone()),
            values: Map::from_iter([("lei".to_string(), json!(c.lei))]),
            base_version: Some(version),
            evidence,
            origin: ORIGIN.into(),
            confidence: Some(90.0),
        },
    };
    crate::services::ontology::proposals::propose(db, company.org_id, &input).await?;
    Ok(())
}
