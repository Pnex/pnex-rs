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
        authorization: None,
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
