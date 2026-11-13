//! The document of each filing (markets-research.md D542): the readable
//! text of its primary document becomes a media of the org, indexed by the
//! document search and linked to the filing. Only the filings of orgs that
//! declared a contact, at the pace the SEC's fair-access rules allow.

use std::time::Duration;

use loco_rs::app::AppContext;
use pnex_core::ontology::api::ObjectInput;
use sea_orm::{ConnectionTrait, DatabaseBackend, FromQueryResult, Statement};
use serde_json::Value;
use uuid::Uuid;

use crate::services::crawl::{self, documents as crawl_docs};
use crate::services::ontology::imports::{source, stop};
use crate::services::ontology::{objects, Actor};

const SWEEP_EVERY: Duration = Duration::from_secs(300);
/// Documents read per sweep, all orgs together.
const PER_SWEEP: i64 = 20;
const PAUSE: Duration = Duration::from_millis(150);
const CONTACT_SETTING: &str = "markets.source_contact";
const LINK_TYPE: &str = "filing_document";
const STATE: &str = "document_state";

/// What became of a filing's document.
#[derive(Debug, PartialEq, Eq)]
enum Fate {
    Text(String),
    /// Not a page this reader understands: left aside for good.
    Unreadable,
}

/// The text kept for a primary document.
fn fate(url: &reqwest::Url, body: &[u8]) -> Fate {
    let path = url.path().to_ascii_lowercase();
    if ![".htm", ".html", ".xhtml"]
        .iter()
        .any(|e| path.ends_with(e))
    {
        return Fate::Unreadable;
    }
    let text = crawl::html::read(&String::from_utf8_lossy(body), url).text;
    if text.is_empty() {
        Fate::Unreadable
    } else {
        Fate::Text(text)
    }
}

#[derive(Debug, FromQueryResult)]
struct Pending {
    id: Uuid,
    org_id: i64,
    title: String,
    version: i32,
    properties: Value,
}

pub fn spawn_scheduler(ctx: &AppContext) {
    super::init(ctx);
    let ctx = ctx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(180)).await;
        loop {
            if !stop::stopping()
                && crate::services::singleton::my_turn(&ctx.db, "market-documents", SWEEP_EVERY)
                    .await
            {
                if let Err(e) = sweep(&ctx).await {
                    tracing::warn!("market documents sweep failed: {e}");
                }
                let today = chrono::Utc::now().date_naive();
                if let Err(e) = super::history::sweep(&ctx, today).await {
                    tracing::warn!(code = e.code(), "price history sweep failed");
                }
                let hosts = super::prediction::Hosts::default();
                if let Err(e) = super::prediction::sweep(&ctx, &hosts, chrono::Utc::now()).await {
                    tracing::warn!("prediction markets sweep failed: {e}");
                }
            }
            tokio::time::sleep(SWEEP_EVERY).await;
        }
    });
}

pub async fn sweep(ctx: &AppContext) -> Result<(), sea_orm::DbErr> {
    // shortcut: random order so a document that keeps failing never blocks
    // the others; it is retried forever at this low rate. Count attempts if
    // the journal shows it matters.
    let due = Pending::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT id, org_id, title, version, properties FROM objects \
         WHERE type_key = 'filing' AND valid_to IS NULL \
           AND (properties ->> 'url' LIKE $3 OR properties ->> 'url' LIKE $4) \
           AND NOT properties ? $1 \
         ORDER BY random() LIMIT $2",
        // Anyone can type a filing by hand: the server only ever fetches the
        // public archives, with the org's declared contact.
        vec![
            STATE.into(),
            PER_SWEEP.into(),
            format!("{}%", pnex_core::markets::SEC_ARCHIVE).into(),
            format!("{}/%", pnex_core::markets::ESEF_BASE).into(),
        ],
    ))
    .all(&ctx.db)
    .await?;
    for filing in due {
        keep(ctx, &filing).await;
        tokio::time::sleep(PAUSE).await;
    }
    exhibits(ctx).await
}

/// The subsidiaries of each annual report (F6): read once per 10-K, the
/// filing then carries `subsidiaries_state`.
async fn exhibits(ctx: &AppContext) -> Result<(), sea_orm::DbErr> {
    let mut prefixes = vec![format!("{}%", pnex_core::markets::SEC_ARCHIVE)];
    prefixes.extend(super::test_hosts().into_iter().map(|h| format!("{h}%")));
    let due = Pending::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT id, org_id, title, version, properties FROM objects \
         WHERE type_key = 'filing' AND valid_to IS NULL \
           AND properties ->> 'form' IN ('10-K', '10-K405', '10-KT') \
           AND properties ->> 'url' LIKE ANY($3) \
           AND NOT properties ? $1 \
         ORDER BY properties ->> 'filed_on' NULLS FIRST, id LIMIT $2",
        vec![EXHIBIT_STATE.into(), PER_SWEEP.into(), prefixes.into()],
    ))
    .all(&ctx.db)
    .await?;
    for filing in due {
        subsidiaries(ctx, &filing).await;
        tokio::time::sleep(PAUSE).await;
    }
    Ok(())
}

const EXHIBIT_STATE: &str = "subsidiaries_state";

async fn subsidiaries(ctx: &AppContext, filing: &Pending) {
    use pnex_core::markets::exhibit21;
    let Some(contact) = contact(&ctx.db, filing.org_id).await else {
        return;
    };
    let text = |k: &str| {
        filing
            .properties
            .get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let (url, accession) = (text("url"), text("accession"));
    let Some(dir) = url.rfind('/').map(|i| &url[..i]) else {
        return;
    };
    let by = source::Declared {
        contact: Some(contact),
        ..Default::default()
    };
    let read = |u: String| {
        let by = &by;
        async move {
            let u = source::check_url(&u).map_err(|_| "invalid")?;
            let body = source::get_as(&u, by).await?;
            Ok::<_, &'static str>((u, String::from_utf8_lossy(&body).into_owned()))
        }
    };
    let state = match read(format!("{dir}/{accession}-index.htm")).await {
        Err(code) => {
            tracing::info!(filing = %filing.id, code, "filing index not read");
            return;
        }
        Ok((index_url, index)) => match exhibit21::exhibit_href(&index)
            .and_then(|h| index_url.join(&h).ok())
            // The exhibit lives in the same filing, on the same host.
            .filter(|u| u.host_str() == index_url.host_str())
        {
            None => ("none", 0),
            Some(ex) => match read(ex.to_string()).await {
                Err(code) => {
                    tracing::info!(filing = %filing.id, code, "exhibit 21 not read");
                    return;
                }
                Ok((ex_url, doc)) => {
                    let found = exhibit21::subsidiaries(&doc);
                    if found.is_empty() {
                        ("unreadable", 0)
                    } else {
                        match write_subsidiaries(ctx, filing, ex_url.as_str(), &found).await {
                            Ok(n) => ("read", n),
                            Err(e) => {
                                tracing::warn!(filing = %filing.id, error = %e, "subsidiaries not written");
                                return;
                            }
                        }
                    }
                }
            },
        },
    };
    tracing::info!(filing = %filing.id, state = state.0, subsidiaries = state.1, "exhibit 21");
    // Read again: the document pass may have changed the filing meanwhile.
    let Ok(fresh) = objects::find(&ctx.db, filing.org_id, filing.id).await else {
        return;
    };
    let mut properties = fresh.properties.as_object().cloned().unwrap_or_default();
    properties.insert(EXHIBIT_STATE.into(), state.0.into());
    let input = ObjectInput {
        type_key: "filing".into(),
        title: fresh.title.clone(),
        properties,
        expected_version: Some(fresh.version),
    };
    if let Err(e) = objects::update(
        &ctx.db,
        filing.org_id,
        &actor(filing.org_id),
        filing.id,
        &input,
    )
    .await
    {
        tracing::warn!(filing = %filing.id, error = %e, "exhibit state not recorded");
    }
}

/// Each declared subsidiary as an organization found again by its country
/// and name, linked to the filer by `subsidiary_of` (nature `declared`, the
/// exhibit as source), from the filing date. A declared link whose
/// subsidiary is missing from the filer's latest report is closed at that
/// report's date. Returns how many links were opened.
// shortcut: reports are swept oldest first, so a report read after a newer
// one (an older filing imported later) neither closes nor back-dates links;
// rebuild the intervals from every report if that case shows up.
async fn write_subsidiaries(
    ctx: &AppContext,
    filing: &Pending,
    exhibit: &str,
    found: &[pnex_core::markets::exhibit21::Declared],
) -> Result<usize, crate::services::ontology::OntologyError> {
    use crate::services::ontology::{changes, links, schema};
    use pnex_core::ontology::api::LinkInput;
    use sea_orm::TransactionTrait;
    use serde_json::{json, Map};
    let org = filing.org_id;
    let Some(parent) = ctx
        .db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT target_object_id AS p FROM resource_edges WHERE org_id = $1 \
               AND relation = 'filed_by' AND source_object_id = $2 AND valid_to IS NULL LIMIT 1",
            vec![org.into(), filing.id.into()],
        ))
        .await?
        .map(|r| r.try_get::<Uuid>("", "p"))
        .transpose()?
    else {
        return Ok(0);
    };
    let actor = actor(org);
    let s = schema(&ctx.db, org).await?;
    let filed_on = filing
        .properties
        .get("filed_on")
        .and_then(Value::as_str)
        .filter(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok())
        .map(str::to_string);
    let since = filed_on.as_deref().map(|d| format!("{d}T00:00:00Z"));
    changes::deferred(async {
        let txn = ctx.db.begin().await?;
        let mut opened = 0;
        let mut listed = Vec::with_capacity(found.len());
        for d in found {
            let key = d.key();
            let sub = match objects::find_by_key(&txn, org, "organization", "declared_key", &json!(key)).await? {
                Some(o) => o.id,
                None => {
                    let mut props = Map::new();
                    props.insert("declared_key".into(), json!(key));
                    props.insert("kind".into(), json!("company"));
                    props.insert("incorporated_in".into(), json!(d.jurisdiction));
                    if let Some(c) = d.country {
                        props.insert("country".into(), json!(c));
                    }
                    let input = ObjectInput {
                        type_key: "organization".into(),
                        title: d.name.clone(),
                        properties: props,
                        expected_version: None,
                    };
                    let v = objects::create_with(&txn, &s, org, &actor, &input).await?;
                    Uuid::parse_str(&v.id).map_err(|_| crate::services::ontology::OntologyError::NotFound)?
                }
            };
            if sub == parent {
                continue;
            }
            listed.push(sub);
            let exists = txn
                .query_one_raw(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    "SELECT 1 AS x FROM resource_edges WHERE org_id = $1 AND relation = 'subsidiary_of' \
                       AND source_object_id = $2 AND target_object_id = $3 AND valid_to IS NULL",
                    vec![org.into(), sub.into(), parent.into()],
                ))
                .await?
                .is_some();
            if exists {
                continue;
            }
            let mut attrs = Map::new();
            attrs.insert("nature".into(), json!("declared"));
            attrs.insert("source".into(), json!(exhibit));
            links::create_with(
                &txn,
                &s,
                org,
                &actor,
                &LinkInput {
                    link_type: "subsidiary_of".into(),
                    source_id: sub.to_string(),
                    target_id: parent.to_string(),
                    attributes: attrs,
                    valid_from: since.clone(),
                    valid_to: None,
                },
            )
            .await?;
            opened += 1;
        }
        if let Some(day) = &filed_on {
            close_dropped(&txn, org, &actor, parent, &listed, day, since.as_deref()).await?;
        }
        txn.commit().await?;
        changes::flush();
        Ok(opened)
    })
    .await
}

/// Closes the declared links of `parent` whose subsidiary this report no
/// longer lists, when no newer report of the filer was read already.
async fn close_dropped<C: ConnectionTrait>(
    db: &C,
    org: i64,
    actor: &Actor,
    parent: Uuid,
    listed: &[Uuid],
    day: &str,
    at: Option<&str>,
) -> Result<(), crate::services::ontology::OntologyError> {
    use crate::services::ontology::links;
    let newer = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT 1 AS x FROM resource_edges e JOIN objects o ON o.id = e.source_object_id \
             WHERE e.org_id = $1 AND e.relation = 'filed_by' AND e.target_object_id = $2 \
               AND e.valid_to IS NULL AND o.valid_to IS NULL AND o.type_key = 'filing' \
               AND o.properties ->> $3 = 'read' AND o.properties ->> 'filed_on' > $4 LIMIT 1",
            vec![org.into(), parent.into(), EXHIBIT_STATE.into(), day.into()],
        ))
        .await?;
    if newer.is_some() {
        return Ok(());
    }
    let dropped = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT id FROM resource_edges WHERE org_id = $1 AND relation = 'subsidiary_of' \
               AND target_object_id = $2 AND valid_to IS NULL AND attributes ->> 'nature' = 'declared' \
               AND NOT (source_object_id = ANY($3))",
            vec![org.into(), parent.into(), listed.to_vec().into()],
        ))
        .await?;
    for row in dropped {
        let id: i64 = row.try_get("", "id")?;
        // A link opened before links were dated starts after the report: left open.
        if let Err(e) = links::close(db, org, actor, id, at).await {
            tracing::info!(link = id, error = %e, "declared subsidiary link not closed");
        }
    }
    Ok(())
}

async fn contact<C: ConnectionTrait>(db: &C, org_id: i64) -> Option<String> {
    crate::services::ontology::packs::setting_value(db, org_id, CONTACT_SETTING)
        .await
        .ok()
        .flatten()
        .and_then(|v| v.as_str().map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
}

async fn keep(ctx: &AppContext, filing: &Pending) {
    let Some(contact) = contact(&ctx.db, filing.org_id).await else {
        return;
    };
    let Some(url) = filing
        .properties
        .get("url")
        .and_then(Value::as_str)
        .and_then(|u| source::check_url(u).ok())
    else {
        return;
    };
    let by = source::Declared {
        contact: Some(contact),
        ..Default::default()
    };
    let body = match source::get_as(&url, &by).await {
        Ok(b) => b,
        Err(code) => {
            tracing::info!(url = %url, code, "filing document not read");
            return;
        }
    };
    let bytes = body.len();
    let state = match fate(&url, &body) {
        Fate::Unreadable => "unreadable",
        Fate::Text(text) => {
            drop(body);
            match store(ctx, filing, &url, text).await {
                Ok(()) => "kept",
                Err(e) => {
                    tracing::warn!(url = %url, bytes, error = %e, "filing document not kept");
                    return;
                }
            }
        }
    };
    let mut properties = filing.properties.as_object().cloned().unwrap_or_default();
    properties.insert(STATE.into(), state.into());
    let input = ObjectInput {
        type_key: "filing".into(),
        title: filing.title.clone(),
        properties,
        expected_version: Some(filing.version),
    };
    if let Err(e) = objects::update(
        &ctx.db,
        filing.org_id,
        &actor(filing.org_id),
        filing.id,
        &input,
    )
    .await
    {
        tracing::warn!(filing = %filing.id, error = %e, "filing state not recorded");
    }
}

fn actor(org_id: i64) -> Actor {
    Actor {
        user_id: None,
        role: crate::models::_entities::sea_orm_active_enums::OrgMemberRole::Member,
        source_ref: format!("markets-documents:{org_id}"),
    }
}

async fn store(
    ctx: &AppContext,
    filing: &Pending,
    url: &reqwest::Url,
    text: String,
) -> Result<(), String> {
    let accession = filing
        .properties
        .get("accession")
        .and_then(Value::as_str)
        .unwrap_or("filing");
    let body = text.into_bytes();
    let sha = crawl::sha256_hex(&body);
    let asset = crawl_docs::store(
        ctx,
        filing.org_id,
        "sec_edgar",
        url.as_str(),
        &format!("{accession}.txt"),
        "text/plain",
        &sha,
        body,
    )
    .await?;
    crawl_docs::link(
        ctx,
        filing.org_id,
        LINK_TYPE,
        filing.id,
        asset,
        url.as_str(),
        &sha,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_filing_page_loses_its_hidden_facts_and_only_pages_are_read() {
        let url = reqwest::Url::parse("https://sec.example/a/report.htm").unwrap();
        let html = b"<html><body><div style='display:none'><ix:header>0001 hidden</ix:header></div><p>Revenue grew</p></body></html>";
        assert_eq!(fate(&url, html), Fate::Text("Revenue grew".into()));
        let xml = reqwest::Url::parse("https://sec.example/a/report.xml").unwrap();
        assert_eq!(fate(&xml, html), Fate::Unreadable);
    }
}
