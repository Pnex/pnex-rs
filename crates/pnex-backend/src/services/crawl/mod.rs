//! Site crawls (partner-watch.md D307, D309): `crawl_site` missions walk
//! one declared site and keep its pages in the `site_pages` registry.
//!
//! The server executor (default) reads public pages without running any
//! script: identified client, `robots.txt` read and obeyed, sitemaps first
//! then internal links breadth first, never off the origin (redirects
//! included), one request per host every 2 s at least, capped pages, depth
//! and page size, behind the egress guard (R8). An agent executor reports
//! the pages it rendered through the same [`register`].
//!
//! A page matching the mission's rules is `relevant`: its readable text is
//! kept as a versioned document (D311 diffs it), and when it is new or
//! changed a proof is asked of a browser agent (D308). A page missing from
//! two complete crawls in a row is `gone`. Relevant pages may be written as
//! objects of an org type (`record`), through the ontology services.

pub mod documents;
pub mod html;
pub mod robots;

use std::collections::{HashSet, VecDeque};
use std::time::Duration;

use loco_rs::app::AppContext;
use pnex_core::browser_agent::{
    fold, CrawlSiteParams, MatchKind, Matcher, MissionSpec, PageRecord, SitePage, CRAWL_DELAY_MS,
};
use pnex_core::err_codes;
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, FromQueryResult, Statement};
use uuid::Uuid;

/// Largest page read (bytes).
pub const PAGE_MAX_BYTES: usize = 5 * 1024 * 1024;
/// Pages of the registry per org (D309).
pub const REGISTRY_MAX: i64 = 200_000;
/// Longest crawl delay a `robots.txt` may ask for.
const DELAY_MAX: Duration = Duration::from_secs(30);
/// Paths that are never pages.
const SKIPPED_EXTENSIONS: [&str; 16] = [
    ".pdf", ".jpg", ".jpeg", ".png", ".gif", ".webp", ".svg", ".zip", ".mp4", ".mp3", ".css",
    ".js", ".ico", ".xml", ".doc", ".docx",
];

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

// ───────────────────────────── relevance ─────────────────────────────

/// Compiled relevance rules of a mission.
pub struct Relevance {
    words: Vec<regex::Regex>,
    phrases: Vec<String>,
    regexes: Vec<regex::Regex>,
}

impl Relevance {
    /// `Err(i)`: rule `i` is not a bounded regular expression.
    pub fn compile(matchers: &[Matcher]) -> Result<Self, usize> {
        let bounded = |pattern: &str| {
            regex::RegexBuilder::new(pattern)
                .case_insensitive(true)
                .size_limit(256 * 1024)
                .dfa_size_limit(256 * 1024)
                .build()
        };
        let mut out = Self {
            words: Vec::new(),
            phrases: Vec::new(),
            regexes: Vec::new(),
        };
        for (i, m) in matchers.iter().enumerate() {
            let v = m.value.trim();
            match m.kind {
                MatchKind::Word => out
                    .words
                    .push(bounded(&format!(r"\b{}\b", regex::escape(&fold(v)))).map_err(|_| i)?),
                MatchKind::Phrase => out.phrases.push(fold(v)),
                MatchKind::Regex => out.regexes.push(bounded(v).map_err(|_| i)?),
            }
        }
        Ok(out)
    }

    /// No rule: every page is relevant.
    pub fn matches(&self, text: &str) -> bool {
        if self.words.is_empty() && self.phrases.is_empty() && self.regexes.is_empty() {
            return true;
        }
        let folded = fold(text);
        self.words.iter().any(|r| r.is_match(&folded))
            || self.phrases.iter().any(|p| folded.contains(p.as_str()))
            || self.regexes.iter().any(|r| r.is_match(text))
    }
}

// ───────────────────────────── registry ─────────────────────────────

#[derive(Debug, Clone, FromQueryResult)]
struct PageRow {
    id: Uuid,
    mission_id: Option<Uuid>,
    site: String,
    url: String,
    title: String,
    depth: i32,
    first_seen_at: DateTimeWithTimeZone,
    last_seen_at: DateTimeWithTimeZone,
    text_hash: Option<String>,
    captured_hash: Option<String>,
    state: String,
    state_code: Option<String>,
    misses: i32,
    doc_asset_id: Option<Uuid>,
    last_proof_id: Option<Uuid>,
    object_id: Option<Uuid>,
}

const PAGE_COLUMNS: &str = "id, mission_id, site, url, title, depth, first_seen_at, last_seen_at, \
     text_hash, captured_hash, state, state_code, misses, doc_asset_id, last_proof_id, object_id";

impl PageRow {
    fn view(&self) -> SitePage {
        SitePage {
            id: self.id.to_string(),
            mission_id: self.mission_id.map(|m| m.to_string()),
            site: self.site.clone(),
            url: self.url.clone(),
            title: self.title.clone(),
            depth: self.depth.max(0) as u32,
            state: self.state.clone(),
            state_code: self.state_code.clone(),
            first_seen_at: self.first_seen_at.to_rfc3339(),
            last_seen_at: self.last_seen_at.to_rfc3339(),
            text_hash: self.text_hash.clone(),
            misses: self.misses,
            doc_asset_id: self.doc_asset_id.map(|a| a.to_string()),
            last_proof_id: self.last_proof_id.map(|a| a.to_string()),
            object_id: self.object_id.map(|a| a.to_string()),
        }
    }
}

/// Pages of a mission's site, newest seen first (`state` filters).
pub async fn pages(
    db: &DatabaseConnection,
    org_id: i64,
    mission_id: Uuid,
    state: Option<&str>,
) -> Result<Vec<SitePage>, sea_orm::DbErr> {
    let rows = PageRow::find_by_statement(stmt(
        &format!(
            "SELECT {PAGE_COLUMNS} FROM site_pages WHERE org_id = $1 AND mission_id = $2 \
             AND ($3::text IS NULL OR state = $3) ORDER BY last_seen_at DESC, url LIMIT 5000"
        ),
        vec![
            org_id.into(),
            mission_id.into(),
            state.map(str::to_string).into(),
        ],
    ))
    .all(db)
    .await?;
    Ok(rows.iter().map(PageRow::view).collect())
}

/// A page read by a crawl (server or agent).
pub struct Seen {
    pub url: String,
    pub title: String,
    pub text: String,
    pub depth: u32,
}

/// The crawl a page belongs to.
pub struct CrawlRef<'a> {
    pub org_id: i64,
    pub mission_id: Uuid,
    pub site: &'a str,
    pub params: &'a CrawlSiteParams,
    pub relevance: &'a Relevance,
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Records one page in the registry; a relevant page new or changed keeps
/// its text as a document version, asks for a proof and updates its
/// object. `Ok((page, changed))`; `Err(code)` = registry full.
pub async fn register(
    ctx: &AppContext,
    c: &CrawlRef<'_>,
    seen: &Seen,
) -> Result<(SitePage, bool), &'static str> {
    let db = &ctx.db;
    let text_hash = sha256_hex(seen.text.as_bytes());
    let relevant = !seen.text.trim().is_empty() && c.relevance.matches(&seen.text);
    let existing = PageRow::find_by_statement(stmt(
        &format!("SELECT {PAGE_COLUMNS} FROM site_pages WHERE org_id = $1 AND url = $2"),
        vec![c.org_id.into(), seen.url.clone().into()],
    ))
    .one(db)
    .await
    .map_err(|_| err_codes::CRAWL_UNREACHABLE)?;
    if existing.is_none() {
        let n = db
            .query_one_raw(stmt(
                "SELECT count(*)::bigint AS n FROM site_pages WHERE org_id = $1",
                vec![c.org_id.into()],
            ))
            .await
            .ok()
            .flatten()
            .and_then(|r| r.try_get::<i64>("", "n").ok())
            .unwrap_or(0);
        if n >= REGISTRY_MAX {
            return Err(err_codes::EXT_REGISTRY_FULL);
        }
    }
    let changed = existing
        .as_ref()
        .is_none_or(|p| p.text_hash.as_deref() != Some(text_hash.as_str()));
    let captured = existing
        .as_ref()
        .is_some_and(|p| p.captured_hash.as_deref() == Some(text_hash.as_str()));
    let state = match (relevant, captured) {
        (false, _) => "listed",
        (true, true) => "captured",
        (true, false) => "relevant",
    };
    let title: String = seen.title.chars().take(500).collect();
    let row = PageRow::find_by_statement(stmt(
        &format!(
            "INSERT INTO site_pages (id, org_id, mission_id, site, url, title, depth, text_hash, state) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
             ON CONFLICT (org_id, url) DO UPDATE SET mission_id = $3, site = $4, title = $6, \
               depth = least(site_pages.depth, $7), text_hash = $8, state = $9, state_code = NULL, \
               misses = 0, last_seen_at = now() \
             RETURNING {PAGE_COLUMNS}"
        ),
        vec![
            Uuid::new_v4().into(),
            c.org_id.into(),
            c.mission_id.into(),
            c.site.to_string().into(),
            seen.url.clone().into(),
            title.into(),
            (seen.depth as i32).into(),
            text_hash.clone().into(),
            state.into(),
        ],
    ))
    .one(db)
    .await
    .ok()
    .flatten()
    .ok_or(err_codes::CRAWL_UNREACHABLE)?;
    let mut row = row;
    if relevant && changed {
        match keep_text(ctx, c, &row, seen).await {
            Ok(asset) => row.doc_asset_id = Some(asset),
            Err(e) => tracing::warn!(page = %row.id, error = %e, "crawl text not kept"),
        }
        if c.params.proofs {
            if let Err(e) = crate::services::proofs::request(db, c.org_id, &row.view()).await {
                tracing::warn!(page = %row.id, error = %e, "proof not requested");
            }
        }
    }
    if let Some(record) = &c.params.record {
        if relevant || row.object_id.is_some() {
            // A first visit is the baseline, not a change.
            let changed = changed && existing.is_some();
            row.object_id = record_object(db, c, record, &row, relevant, &seen.text, changed)
                .await
                .or(row.object_id);
        }
    }
    Ok((row.view(), changed))
}

/// The readable text of a relevant page as a version of its document
/// (one document per page, created on the first change).
async fn keep_text(
    ctx: &AppContext,
    c: &CrawlRef<'_>,
    row: &PageRow,
    seen: &Seen,
) -> Result<Uuid, String> {
    use crate::models::_entities::media_assets;
    use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
    let asset = match row.doc_asset_id {
        Some(id) => media_assets::Entity::find_by_id(id)
            .filter(media_assets::Column::OrgId.eq(c.org_id))
            .one(&ctx.db)
            .await
            .map_err(|e| e.to_string())?,
        None => None,
    };
    let asset = match asset {
        Some(a) => a,
        None => media_assets::ActiveModel {
            id: Set(Uuid::new_v4()),
            org_id: Set(c.org_id),
            kind: Set("document".into()),
            name: Set(if seen.title.trim().is_empty() {
                seen.url.chars().take(255).collect()
            } else {
                seen.title.chars().take(255).collect()
            }),
            description: Set(None),
            metadata: Set(Some(serde_json::json!({
                "source": "crawler",
                "mission_id": c.mission_id.to_string(),
                "source_url": seen.url,
                "site": c.site,
            }))),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .map_err(|e| e.to_string())?,
    };
    let markdown =
        crate::services::extension::page_markdown(&pnex_core::browser_agent::PageSnapshot {
            mission_id: c.mission_id.to_string(),
            url: seen.url.clone(),
            title: seen.title.clone(),
            text: seen.text.clone(),
        });
    let settings = crate::services::media::MediaSettings::from_config(&ctx.config);
    crate::controllers::media::write_version(
        ctx,
        c.org_id,
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
    .map_err(|e| e.to_string())?;
    ctx.db
        .execute_raw(stmt(
            "UPDATE site_pages SET doc_asset_id = $2 WHERE id = $1",
            vec![row.id.into(), asset.id.into()],
        ))
        .await
        .map_err(|e| e.to_string())?;
    Ok(asset.id)
}

/// Characters of a page's text a record may keep.
const RECORD_TEXT_MAX: usize = 20_000;

/// Writes the page as an object of the record type (upsert on the
/// property `url` maps to), and opens its link once. Fields: `url`,
/// `title`, `first_seen`, `last_seen`, `text_hash`, `relevant`, `state`,
/// and for a watch flow `text` (readable text, clipped), `changed` (its
/// text differs from the last crawl) and `link_to` (the record's target).
async fn record_object(
    db: &DatabaseConnection,
    c: &CrawlRef<'_>,
    record: &PageRecord,
    row: &PageRow,
    relevant: bool,
    text: &str,
    changed: bool,
) -> Option<Uuid> {
    use crate::services::ontology::{links, objects, Actor};
    use pnex_core::ontology::api::{LinkInput, ObjectInput};
    let actor = Actor {
        user_id: None,
        role: crate::models::_entities::sea_orm_active_enums::OrgMemberRole::Member,
        source_ref: format!("crawl:{}", c.mission_id),
    };
    let value = |field: &str| -> Option<serde_json::Value> {
        Some(match field {
            "url" => row.url.clone().into(),
            "title" => row.title.clone().into(),
            "first_seen" => row.first_seen_at.to_rfc3339().into(),
            "last_seen" => row.last_seen_at.to_rfc3339().into(),
            "text_hash" => row.text_hash.clone()?.into(),
            "relevant" => relevant.into(),
            "state" => row.state.clone().into(),
            "text" => text
                .chars()
                .take(RECORD_TEXT_MAX)
                .collect::<String>()
                .into(),
            "link_to" => record.link_to.clone()?.into(),
            _ => return None,
        })
    };
    let mut props = serde_json::Map::new();
    for (field, key) in &record.properties {
        if let Some(v) = value(field) {
            props.insert(key.clone(), v);
        }
    }
    // Set only when the text changed: the watch flow clears it once read.
    if let Some(key) = record.properties.get("changed").filter(|_| changed) {
        props.insert(key.clone(), true.into());
    }
    let url_key = record.properties.get("url")?;
    let title = if row.title.is_empty() {
        row.url.clone()
    } else {
        row.title.clone()
    };
    let found = objects::find_by_key(
        db,
        c.org_id,
        &record.type_key,
        url_key,
        &row.url.clone().into(),
    )
    .await
    .ok()?;
    let written = match found {
        Some(o) => {
            let mut merged = o.properties.as_object().cloned().unwrap_or_default();
            merged.extend(props);
            objects::update(
                db,
                c.org_id,
                &actor,
                o.id,
                &ObjectInput {
                    type_key: o.type_key.clone(),
                    title: title
                        .chars()
                        .take(pnex_core::ontology::api::TITLE_MAX)
                        .collect(),
                    properties: merged,
                    expected_version: Some(o.version),
                },
            )
            .await
        }
        None => {
            objects::create(
                db,
                c.org_id,
                &actor,
                &ObjectInput {
                    type_key: record.type_key.clone(),
                    title: title
                        .chars()
                        .take(pnex_core::ontology::api::TITLE_MAX)
                        .collect(),
                    properties: props,
                    expected_version: None,
                },
            )
            .await
        }
    };
    let object = match written {
        Ok(o) => Uuid::parse_str(&o.id).ok()?,
        Err(e) => {
            tracing::warn!(page = %row.id, error = %e, "crawl page object not written");
            return None;
        }
    };
    if let (Some(link_type), Some(to)) = (&record.link_type, &record.link_to) {
        let open = links::create(
            db,
            c.org_id,
            &actor,
            &LinkInput {
                link_type: link_type.clone(),
                source_id: object.to_string(),
                target_id: to.clone(),
                attributes: Default::default(),
                valid_from: None,
                valid_to: None,
            },
        )
        .await;
        // Already open (409) is the usual case after the first crawl.
        if let Err(e) = open {
            tracing::debug!(page = %row.id, error = %e, "crawl page link kept");
        }
    }
    let _ = db
        .execute_raw(stmt(
            "UPDATE site_pages SET object_id = $2 WHERE id = $1",
            vec![row.id.into(), object.into()],
        ))
        .await;
    Some(object)
}

/// After a complete crawl: pages of the site not seen count one miss more;
/// at two they are `gone` (often the proof of a fix).
async fn count_misses(
    db: &DatabaseConnection,
    org_id: i64,
    site: &str,
    seen: &[String],
) -> Result<(), sea_orm::DbErr> {
    db.execute_raw(stmt(
        "UPDATE site_pages SET misses = misses + 1, \
           state = CASE WHEN misses + 1 >= 2 THEN 'gone' ELSE state END \
         WHERE org_id = $1 AND site = $2 AND state <> 'gone' AND NOT (url = ANY($3))",
        vec![org_id.into(), site.to_string().into(), seen.to_vec().into()],
    ))
    .await?;
    Ok(())
}

// ───────────────────────────── server executor ─────────────────────────────

fn http() -> &'static reqwest::Client {
    static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    HTTP.get_or_init(|| {
        let instance = crate::app::prod_host()
            .map(|h| format!("https://{h}"))
            .unwrap_or_else(|| "https://pnex.io".into());
        pnex_core::egress::guarded(reqwest::Client::builder())
            // Redirects are followed by hand: never off the origin.
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(20))
            .user_agent(format!(
                "PNEX-Crawler/{} (+{instance})",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .expect("reqwest client")
    })
}

pub(crate) struct Fetched {
    pub(crate) url: reqwest::Url,
    pub(crate) status: u16,
    pub(crate) content_type: String,
    pub(crate) body: String,
}

/// GET with same-origin redirects only (5 hops), size capped.
pub(crate) async fn fetch(origin: &str, url: &reqwest::Url) -> Result<Fetched, &'static str> {
    let (url, status, content_type, body, _) = fetch_bytes(origin, url, PAGE_MAX_BYTES).await?;
    Ok(Fetched {
        url,
        status,
        content_type,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

/// `fetch` as bytes, cut at `max`: (final url, status, content type,
/// body, whether it was cut).
pub(crate) async fn fetch_bytes(
    origin: &str,
    url: &reqwest::Url,
    max: usize,
) -> Result<(reqwest::Url, u16, String, Vec<u8>, bool), &'static str> {
    let mut current = url.clone();
    for _ in 0..=5 {
        if pnex_core::egress::check_url(&current).is_some() {
            return Err(err_codes::CRAWL_UNREACHABLE);
        }
        let mut resp = http()
            .get(current.clone())
            .send()
            .await
            .map_err(|_| err_codes::CRAWL_UNREACHABLE)?;
        let status = resp.status();
        if status.is_redirection() {
            let next = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|l| l.to_str().ok())
                .and_then(|l| current.join(l).ok())
                .ok_or(err_codes::CRAWL_UNREACHABLE)?;
            if pnex_core::browser_agent::site_origin(next.as_str()).as_deref() != Some(origin) {
                // Off the site: not followed (D307).
                return Ok((current, status.as_u16(), String::new(), Vec::new(), false));
            }
            current = next;
            continue;
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let mut body = Vec::new();
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|_| err_codes::CRAWL_UNREACHABLE)?
        {
            if body.len() + chunk.len() > max {
                return Ok((current, status.as_u16(), content_type, body, true));
            }
            body.extend_from_slice(&chunk);
        }
        return Ok((current, status.as_u16(), content_type, body, false));
    }
    Err(err_codes::CRAWL_UNREACHABLE)
}

/// May the crawl visit `u`: same origin, allowed, not excluded, a page.
fn visitable(origin: &str, u: &reqwest::Url, robots: &robots::Robots, exclude: &[String]) -> bool {
    let path = u.path().to_ascii_lowercase();
    let with_query = match u.query() {
        Some(q) => format!("{}?{q}", u.path()),
        None => u.path().to_string(),
    };
    pnex_core::browser_agent::site_origin(u.as_str()).as_deref() == Some(origin)
        && robots.allows(&with_query)
        && !exclude.iter().any(|x| u.path().starts_with(x.as_str()))
        && !SKIPPED_EXTENSIONS.iter().any(|e| path.ends_with(e))
        && u.as_str().len() <= pnex_core::browser_agent::URL_MAX
}

/// Outcome of one server crawl.
#[derive(Debug, Default)]
pub struct CrawlOutcome {
    pub pages: usize,
    pub relevant: usize,
    pub truncated: bool,
    pub blocked: Option<&'static str>,
    /// The relevant pages read, text clipped (network-check.md D407).
    pub found: Vec<pnex_core::site_crawl::SiteCrawlPage>,
}

/// Walks the site of a mission once.
pub async fn crawl(
    ctx: &AppContext,
    org_id: i64,
    mission_id: Uuid,
    params: &CrawlSiteParams,
) -> CrawlOutcome {
    let mut out = CrawlOutcome::default();
    let origin = params.url.clone();
    let site = pnex_core::browser_agent::site_key(&origin);
    let Ok(base) = reqwest::Url::parse(&origin) else {
        out.blocked = Some(err_codes::CRAWL_UNREACHABLE);
        return out;
    };
    let relevance = match Relevance::compile(&params.matchers) {
        Ok(r) => r,
        Err(_) => {
            out.blocked = Some(err_codes::CRAWL_UNREACHABLE);
            return out;
        }
    };
    let pace = |robots: &robots::Robots| {
        Duration::from_millis(CRAWL_DELAY_MS)
            .max(Duration::from_secs(robots.crawl_delay_secs.unwrap_or(0)))
            .min(DELAY_MAX)
    };
    // robots.txt: 4xx = no rule; unreachable = no crawl.
    let robots = match fetch(&origin, &base.join("/robots.txt").expect("path")).await {
        Ok(f) if f.status == 200 => robots::Robots::parse(&f.body),
        Ok(f) if (400..500).contains(&f.status) => robots::Robots::default(),
        _ => {
            out.blocked = Some(err_codes::CRAWL_UNREACHABLE);
            return out;
        }
    };
    if !robots.allows("/") && robots.sitemaps.is_empty() {
        out.blocked = Some(err_codes::CRAWL_ROBOTS_DISALLOWED);
        return out;
    }
    let delay = pace(&robots);
    let c = CrawlRef {
        org_id,
        mission_id,
        site: &site,
        params,
        relevance: &relevance,
    };
    let mut queue: VecDeque<(reqwest::Url, u32)> = VecDeque::new();
    let mut known: HashSet<String> = HashSet::new();
    let mut push = |u: reqwest::Url, depth: u32, queue: &mut VecDeque<(reqwest::Url, u32)>| {
        if visitable(&origin, &u, &robots, &params.exclude) && known.insert(u.to_string()) {
            queue.push_back((u, depth));
        }
    };
    // Sitemaps first (an index one level down).
    let mut maps: Vec<String> = robots.sitemaps.clone();
    if maps.is_empty() {
        maps.push(format!("{origin}/sitemap.xml"));
    }
    let mut map_reads = 0;
    while let Some(m) = maps.pop() {
        if map_reads >= 10 {
            break;
        }
        let Ok(u) = reqwest::Url::parse(&m) else {
            continue;
        };
        map_reads += 1;
        tokio::time::sleep(delay).await;
        let Ok(f) = fetch(&origin, &u).await else {
            continue;
        };
        if f.status != 200 {
            continue;
        }
        for loc in html::sitemap_locs(&f.body) {
            if loc.ends_with(".xml") {
                maps.push(loc);
            } else if let Ok(u) = reqwest::Url::parse(&loc) {
                push(u, 0, &mut queue);
            }
        }
    }
    push(base.clone(), 0, &mut queue);
    let mut seen_urls: Vec<String> = Vec::new();
    let mut docs_done: HashSet<String> = HashSet::new();
    let mut first = true;
    while let Some((u, depth)) = queue.pop_front() {
        if out.pages >= params.max_pages as usize {
            out.truncated = true;
            break;
        }
        tokio::time::sleep(delay).await;
        let f = match fetch(&origin, &u).await {
            Ok(f) => f,
            Err(_) => continue,
        };
        // The site refuses robots: stop, never insist (D254, D258).
        if matches!(f.status, 401 | 403 | 429) {
            if first || f.status == 429 {
                out.blocked = Some(err_codes::CRAWL_BLOCKED);
                break;
            }
            continue;
        }
        first = false;
        if f.status != 200 || !f.content_type.contains("html") {
            continue;
        }
        let page = html::read(&f.body, &f.url);
        let mut final_url = f.url.clone();
        final_url.set_fragment(None);
        let seen = Seen {
            url: final_url.to_string(),
            title: page.title,
            text: page.text,
            depth,
        };
        match register(ctx, &c, &seen).await {
            Ok((p, changed)) => {
                out.pages += 1;
                if p.state != "listed" {
                    out.relevant += 1;
                    if out.found.len() < pnex_core::site_crawl::PAGES_OUT_MAX {
                        out.found.push(pnex_core::site_crawl::SiteCrawlPage {
                            url: p.url.clone(),
                            title: p.title.clone(),
                            text: seen
                                .text
                                .chars()
                                .take(pnex_core::site_crawl::TEXT_MAX)
                                .collect(),
                            page_id: p.id.clone(),
                            object_id: p.object_id.clone(),
                            changed,
                        });
                    }
                }
                seen_urls.push(seen.url.clone());
                // D421: the documents a relevant page links, as media.
                let page_object = p.object_id.as_deref().and_then(|o| Uuid::parse_str(o).ok());
                if let (Some(record), Some(object), true) = (
                    params.record.as_ref().filter(|r| r.document_link.is_some()),
                    page_object,
                    p.state != "listed",
                ) {
                    for d in documents::links_of(&origin, &page.links, &robots, &docs_done) {
                        if docs_done.len() >= documents::PER_CRAWL {
                            break;
                        }
                        docs_done.insert(d.to_string());
                        tokio::time::sleep(delay).await;
                        documents::keep(ctx, &c, record, object, &origin, &d).await;
                    }
                }
            }
            Err(code) => {
                out.blocked = Some(code);
                break;
            }
        }
        if depth < params.max_depth {
            for link in page.links {
                if let Ok(l) = reqwest::Url::parse(&link) {
                    push(l, depth + 1, &mut queue);
                }
            }
        }
    }
    // Only a complete walk says a page is missing.
    if !out.truncated && out.blocked.is_none() {
        if let Err(e) = count_misses(&ctx.db, org_id, &site, &seen_urls).await {
            tracing::warn!(mission = %mission_id, error = %e, "crawl misses not counted");
        }
    }
    out
}

// ───────────────────────────── agent executor ─────────────────────────────

/// The agent crawl a report belongs to (its own enabled mission).
async fn agent_crawl(
    ctx: &AppContext,
    agent: &crate::services::extension::AgentAuth,
    mission_id: &str,
) -> Result<(Uuid, CrawlSiteParams), crate::services::extension::ExtensionError> {
    use crate::services::extension::ExtensionError;
    let (id, _, spec) = crate::services::extension::own_mission(&ctx.db, agent, mission_id).await?;
    match spec {
        MissionSpec::CrawlSite(p)
            if p.executor == pnex_core::browser_agent::CrawlExecutor::Agent =>
        {
            Ok((id, p))
        }
        _ => Err(ExtensionError::MissionNotFound),
    }
}

/// A page a browser agent read on its crawl (same registry, same rules).
pub async fn store_report(
    ctx: &AppContext,
    agent: &crate::services::extension::AgentAuth,
    report: &pnex_core::browser_agent::SitePageReport,
) -> Result<SitePage, crate::services::extension::ExtensionError> {
    use crate::services::extension::ExtensionError;
    let (id, params) = agent_crawl(ctx, agent, &report.mission_id).await?;
    let invalid = |f: &str, t: &str| ExtensionError::Invalid {
        field: f.into(),
        token: t.into(),
    };
    let url =
        reqwest::Url::parse(&report.url).map_err(|_| invalid("url", err_codes::FIELD_INVALID))?;
    // Never off the declared site (D307), whatever the agent says.
    if pnex_core::browser_agent::site_origin(url.as_str()).as_deref() != Some(params.url.as_str()) {
        return Err(invalid("url", err_codes::FIELD_INVALID));
    }
    if report.text.len() > pnex_core::browser_agent::PAGE_TEXT_MAX {
        return Err(invalid(
            "text",
            &format!(
                "{}:{}",
                err_codes::FIELD_MAX_LENGTH,
                pnex_core::browser_agent::PAGE_TEXT_MAX
            ),
        ));
    }
    let relevance =
        Relevance::compile(&params.matchers).map_err(|_| ExtensionError::MissionNotFound)?;
    let site = pnex_core::browser_agent::site_key(&params.url);
    let c = CrawlRef {
        org_id: agent.org_id,
        mission_id: id,
        site: &site,
        params: &params,
        relevance: &relevance,
    };
    let mut clean = url.clone();
    clean.set_fragment(None);
    register(
        ctx,
        &c,
        &Seen {
            url: clean.to_string(),
            title: report.title.clone(),
            text: report.text.clone(),
            depth: report.depth.min(pnex_core::browser_agent::CRAWL_DEPTH_MAX),
        },
    )
    .await
    .map(|(page, _)| page)
    .map_err(|code| invalid("url", code))
}

/// End of an agent crawl: a complete one counts the missing pages.
pub async fn agent_done(
    ctx: &AppContext,
    agent: &crate::services::extension::AgentAuth,
    done: &pnex_core::browser_agent::CrawlDone,
) -> Result<(), crate::services::extension::ExtensionError> {
    let (id, params) = agent_crawl(ctx, agent, &done.mission_id).await?;
    let site = pnex_core::browser_agent::site_key(&params.url);
    if done.complete && done.urls.len() <= pnex_core::browser_agent::CRAWL_PAGES_MAX as usize {
        count_misses(&ctx.db, agent.org_id, &site, &done.urls).await?;
    }
    ctx.db
        .execute_raw(stmt(
            "UPDATE extension_missions SET cursor = coalesce(cursor, '{}'::jsonb) \
               || jsonb_build_object('last_crawl_at', now(), 'pages', $2::int) WHERE id = $1",
            vec![id.into(), (done.urls.len() as i32).into()],
        ))
        .await?;
    Ok(())
}

#[derive(Debug, FromQueryResult)]
struct DueRow {
    id: Uuid,
    org_id: i64,
    spec: serde_json::Value,
}

const SWEEP_EVERY: Duration = Duration::from_secs(60);

/// Due server crawls, one after the other on one pod.
// shortcut: crawls run in sequence (a 200-page site takes ~7 min at 2 s
// a page); run them per host in parallel when the backlog grows.
pub fn spawn_scheduler(ctx: &AppContext) {
    let ctx = ctx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(150)).await;
        loop {
            if crate::services::singleton::my_turn(&ctx.db, "site-crawls", SWEEP_EVERY).await {
                if let Err(e) = sweep(&ctx).await {
                    tracing::warn!("site crawl sweep failed: {e}");
                }
            }
            tokio::time::sleep(SWEEP_EVERY).await;
        }
    });
}

async fn sweep(ctx: &AppContext) -> Result<(), sea_orm::DbErr> {
    let due = DueRow::find_by_statement(stmt(
        "SELECT id, org_id, spec FROM extension_missions \
         WHERE enabled AND kind = 'crawl_site' \
           AND coalesce(spec -> 'params' ->> 'executor', 'server') = 'server' \
           AND coalesce((cursor ->> 'last_crawl_at')::timestamptz, 'epoch'::timestamptz) \
               + make_interval(secs => interval_secs) <= now() \
         ORDER BY coalesce((cursor ->> 'last_crawl_at')::timestamptz, 'epoch'::timestamptz) \
         LIMIT 5",
        vec![],
    ))
    .all(&ctx.db)
    .await?;
    for m in due {
        run_server_crawl(ctx, &m).await?;
    }
    Ok(())
}

async fn run_server_crawl(ctx: &AppContext, m: &DueRow) -> Result<(), sea_orm::DbErr> {
    let Ok(MissionSpec::CrawlSite(params)) = serde_json::from_value::<MissionSpec>(m.spec.clone())
    else {
        return Ok(());
    };
    // Marked first: a crash does not loop on the same site.
    ctx.db
        .execute_raw(stmt(
            "UPDATE extension_missions SET state = 'running', state_code = NULL, \
               cursor = coalesce(cursor, '{}'::jsonb) || jsonb_build_object('last_crawl_at', now()) \
             WHERE id = $1",
            vec![m.id.into()],
        ))
        .await?;
    let out = crawl(ctx, m.org_id, m.id, &params).await;
    tracing::info!(mission = %m.id, pages = out.pages, relevant = out.relevant, blocked = ?out.blocked, "site crawled");
    ctx.db
        .execute_raw(stmt(
            "UPDATE extension_missions SET state = $2, state_code = $3, \
               cursor = coalesce(cursor, '{}'::jsonb) || $4::jsonb \
             WHERE id = $1",
            vec![
                m.id.into(),
                if out.blocked.is_some() {
                    "blocked"
                } else {
                    "idle"
                }
                .into(),
                out.blocked.map(str::to_string).into(),
                serde_json::json!({
                    "pages": out.pages, "relevant": out.relevant, "truncated": out.truncated,
                })
                .into(),
            ],
        ))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relevance_reads_words_phrases_and_bounded_regexes() {
        let m = |kind, value: &str| Matcher {
            kind,
            value: value.into(),
        };
        let r = Relevance::compile(&[
            m(MatchKind::Word, "CPF"),
            m(MatchKind::Phrase, "éligible au compte"),
            m(MatchKind::Regex, r"RNCP\s?\d{4,6}"),
        ])
        .unwrap();
        assert!(r.matches("Formation finançable par le cpf."));
        assert!(!r.matches("CPFX n'est pas un mot"));
        assert!(r.matches("Formation ELIGIBLE AU COMPTE personnel"));
        assert!(r.matches("Titre RNCP 34567"));
        assert!(!r.matches("Rien ici"));
        assert!(Relevance::compile(&[]).unwrap().matches("anything"));
        assert_eq!(
            Relevance::compile(&[m(MatchKind::Regex, "(")]).err(),
            Some(0)
        );
    }

    #[test]
    fn only_pages_of_the_origin_are_visited() {
        let robots = robots::Robots::parse("User-agent: *\nDisallow: /admin\n");
        let o = "https://of.example.fr";
        let ok = |s: &str| {
            visitable(
                o,
                &reqwest::Url::parse(s).unwrap(),
                &robots,
                &["/blog/".into()],
            )
        };
        assert!(ok("https://of.example.fr/formations?id=3"));
        assert!(!ok("https://autre.example.fr/formations"));
        assert!(!ok("http://of.example.fr/formations"));
        assert!(!ok("https://of.example.fr/admin/x"));
        assert!(!ok("https://of.example.fr/blog/2024"));
        assert!(!ok("https://of.example.fr/brochure.PDF"));
    }
}
