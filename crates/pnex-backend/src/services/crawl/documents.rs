//! Documents linked from a relevant page (network-check.md D421): each PDF
//! or DOCX of the same site becomes a media of the org, one version per
//! SHA-256 (D311), linked from the page object by the record's
//! `document_link` with its url and dates. Fetched by the guarded client of
//! the crawl: same origin, robots.txt, the crawl's pace, 20 MB at most.

use loco_rs::prelude::*;
use pnex_core::browser_agent::PageRecord;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use super::{fetch_bytes, robots, sha256_hex, CrawlRef};
use crate::models::_entities::{media_assets, media_versions, resource_edges};

pub const DOC_MAX_BYTES: usize = 20 * 1024 * 1024;
/// Documents read per page and per crawl.
pub const PER_PAGE: usize = 5;
pub const PER_CRAWL: usize = 50;
const EXTENSIONS: [(&str, &str); 2] = [
    (".pdf", "application/pdf"),
    (
        ".docx",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    ),
];

/// The document links of a page worth reading: same site, allowed, a
/// known extension; each once.
pub fn links_of(
    origin: &str,
    links: &[String],
    robots: &robots::Robots,
    done: &std::collections::HashSet<String>,
) -> Vec<reqwest::Url> {
    let mut out: Vec<reqwest::Url> = Vec::new();
    for l in links {
        let Ok(u) = reqwest::Url::parse(l) else {
            continue;
        };
        let path = u.path().to_ascii_lowercase();
        let known = EXTENSIONS.iter().any(|(e, _)| path.ends_with(e));
        let same = pnex_core::browser_agent::site_origin(u.as_str()).as_deref() == Some(origin);
        if known
            && same
            && robots.allows(u.path())
            && !done.contains(u.as_str())
            && !out.iter().any(|x| x == &u)
        {
            out.push(u);
        }
        if out.len() >= PER_PAGE {
            break;
        }
    }
    out
}

fn today() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

/// Reads one document and keeps it; a failure is logged, never fatal to
/// the crawl.
pub async fn keep(
    ctx: &AppContext,
    c: &CrawlRef<'_>,
    record: &PageRecord,
    page_object: Uuid,
    origin: &str,
    url: &reqwest::Url,
) {
    let Some(link_type) = record.document_link.as_deref() else {
        return;
    };
    let fetched = fetch_bytes(origin, url, DOC_MAX_BYTES).await;
    let (status, body, cut) = match fetched {
        Ok((_, status, _, body, cut)) => (status, body, cut),
        Err(_) => return,
    };
    if status != 200 || cut || body.is_empty() {
        return;
    }
    let path = url.path().to_ascii_lowercase();
    let Some((ext, content_type)) = EXTENSIONS.iter().find(|(e, _)| path.ends_with(e)) else {
        return;
    };
    let filename: String = url
        .path_segments()
        .and_then(|mut s| s.next_back())
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("document{ext}"))
        .chars()
        .take(200)
        .collect();
    let sha = sha256_hex(&body);
    match store(
        ctx,
        c.org_id,
        url.as_str(),
        &filename,
        content_type,
        &sha,
        body,
    )
    .await
    {
        Ok(asset) => {
            if let Err(e) = link(
                ctx,
                c.org_id,
                link_type,
                page_object,
                asset,
                url.as_str(),
                &sha,
            )
            .await
            {
                tracing::warn!(url = %url, error = %e, "crawl document not linked");
            }
        }
        Err(e) => tracing::warn!(url = %url, error = %e, "crawl document not kept"),
    }
}

/// The media of `url` (created once), with a new version when its bytes
/// changed.
async fn store(
    ctx: &AppContext,
    org_id: i64,
    url: &str,
    filename: &str,
    content_type: &str,
    sha: &str,
    body: Vec<u8>,
) -> std::result::Result<Uuid, String> {
    use sea_orm::{ActiveModelTrait, Set};
    let db = &ctx.db;
    let found = media_assets::Entity::find()
        .filter(media_assets::Column::OrgId.eq(org_id))
        .filter(sea_orm::sea_query::Expr::cust_with_values(
            "metadata->>'crawl_url' = $1",
            [url.to_string()],
        ))
        .one(db)
        .await
        .map_err(|e| e.to_string())?;
    let asset = match found {
        Some(a) => {
            let current = media_versions::Entity::find()
                .filter(media_versions::Column::AssetId.eq(a.id))
                .order_by_desc(media_versions::Column::VersionNumber)
                .one(db)
                .await
                .map_err(|e| e.to_string())?;
            if current.and_then(|v| v.sha256).as_deref() == Some(sha) {
                return Ok(a.id);
            }
            a
        }
        None => media_assets::ActiveModel {
            id: Set(Uuid::new_v4()),
            org_id: Set(org_id),
            kind: Set("document".into()),
            name: Set(filename.to_string()),
            description: Set(None),
            // D281 provenance.
            metadata: Set(Some(json!({ "source": "site_crawl", "crawl_url": url }))),
            ..Default::default()
        }
        .insert(db)
        .await
        .map_err(|e| e.to_string())?,
    };
    let settings = crate::services::media::MediaSettings::from_config(&ctx.config);
    crate::controllers::media::write_version(
        ctx,
        org_id,
        &asset,
        crate::controllers::media::IncomingVersion {
            filename: filename.to_string(),
            content_type: content_type.to_string(),
            note: None,
            bytes: axum::body::Bytes::from(body),
        },
        &settings,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(asset.id)
}

/// Opens the link page → media once; an open link gets `seen_on` and the
/// current `sha256`.
async fn link(
    ctx: &AppContext,
    org_id: i64,
    link_type: &str,
    page_object: Uuid,
    asset: Uuid,
    url: &str,
    sha: &str,
) -> std::result::Result<(), String> {
    use crate::services::ontology::{links, objects, schema, Actor};
    let db = &ctx.db;
    let s = schema(db, org_id).await.map_err(|e| format!("{e:?}"))?;
    let Some(lt) = s.link_type(link_type) else {
        return Ok(());
    };
    let declared = |k: &str| lt.attributes.iter().any(|a| a.key == k);
    let media = objects::find_by_native(
        db,
        org_id,
        pnex_core::resources::KIND_MEDIA_ASSET,
        &asset.to_string(),
    )
    .await
    .map_err(|e| format!("{e:?}"))?;
    let actor = Actor {
        user_id: None,
        role: crate::models::_entities::sea_orm_active_enums::OrgMemberRole::Member,
        source_ref: format!("crawl:{org_id}"),
    };
    let mut attrs = Map::new();
    for (k, v) in [
        ("url", Value::from(url)),
        ("seen_on", Value::from(today())),
        ("sha256", Value::from(sha)),
    ] {
        if declared(k) {
            attrs.insert(k.into(), v);
        }
    }
    let open = resource_edges::Entity::find()
        .filter(resource_edges::Column::OrgId.eq(org_id))
        .filter(resource_edges::Column::Relation.eq(link_type))
        .filter(resource_edges::Column::SourceObjectId.eq(page_object))
        .filter(resource_edges::Column::TargetObjectId.eq(media.id))
        .filter(resource_edges::Column::ValidTo.is_null())
        .one(db)
        .await
        .map_err(|e| e.to_string())?;
    match open {
        Some(e) => {
            let mut all = e.attributes.as_object().cloned().unwrap_or_default();
            if attrs.iter().all(|(k, v)| all.get(k) == Some(v)) {
                return Ok(());
            }
            all.extend(attrs);
            links::update(db, org_id, &actor, e.id, &all)
                .await
                .map_err(|e| format!("{e:?}"))?;
        }
        None => {
            if declared("first_seen") {
                attrs.insert("first_seen".into(), Value::from(today()));
            }
            links::create(
                db,
                org_id,
                &actor,
                &pnex_core::ontology::api::LinkInput {
                    link_type: link_type.to_string(),
                    source_id: page_object.to_string(),
                    target_id: media.id.to_string(),
                    attributes: attrs,
                    valid_from: None,
                    valid_to: None,
                },
            )
            .await
            .map_err(|e| format!("{e:?}"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_documents_of_the_site_are_read() {
        let links: Vec<String> = [
            "https://of.example/docs/programme.pdf",
            "https://of.example/docs/programme.pdf",
            "https://of.example/docs/fiche.DOCX",
            "https://of.example/page.html",
            "https://other.example/x.pdf",
            "https://of.example/admin/secret.pdf",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let robots = robots::Robots::parse("User-agent: *\nDisallow: /admin\n");
        let got = links_of("https://of.example", &links, &robots, &Default::default());
        let got: Vec<&str> = got.iter().map(|u| u.as_str()).collect();
        assert_eq!(
            got,
            vec![
                "https://of.example/docs/programme.pdf",
                "https://of.example/docs/fiche.DOCX"
            ]
        );
    }
}
