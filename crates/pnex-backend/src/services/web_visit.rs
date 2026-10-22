//! The `web_visit` node (web-presence.md D364): one look at a candidate
//! website to identify it. At most three requests on its origin
//! (`robots.txt`, the page, its legal notice) through the crawler's client:
//! identified, egress-guarded (R8), same-origin redirects only, robots
//! obeyed, 2 s at least between two requests to a host. Writes nothing; the
//! options come from the deployed node, never from the runtime (R1).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use loco_rs::app::AppContext;
use pnex_core::err_codes;
use pnex_core::flow::FlowNodeKind;
use pnex_core::web_visit::{
    clip, legal_link, WebVisitNodeReply, WebVisitNodeRequest, NODE_PORT_FAILED, NODE_PORT_OK,
    URL_MAX,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::services::crawl::{fetch, html, robots::Robots};
use crate::services::ontology::flow_nodes::{deployed, NodeError};

/// Visits of an org per UTC day, unless `settings.web_visit.daily_calls`.
const DAILY_CALLS: u64 = 1000;
/// Floor between two requests to one host (D307).
const HOST_FLOOR: Duration = Duration::from_secs(2);

/// Last request time per host, for this process.
static LAST: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

/// Waits until `host` may be asked again, then books the slot.
async fn pace(host: &str) {
    let wait = {
        let mut guard = LAST.lock().unwrap_or_else(|e| e.into_inner());
        let map = guard.get_or_insert_with(HashMap::new);
        let now = Instant::now();
        let at = map
            .get(host)
            .map(|t| (*t + HOST_FLOOR).max(now))
            .unwrap_or(now);
        map.insert(host.to_string(), at);
        at - now
    };
    tokio::time::sleep(wait).await;
}

#[derive(Deserialize)]
struct CapPartial {
    daily_calls: Option<u64>,
}

fn daily_cap(ctx: &AppContext) -> u64 {
    ctx.config
        .settings
        .as_ref()
        .and_then(|s| s.get("web_visit"))
        .and_then(|v| serde_json::from_value::<CapPartial>(v.clone()).ok())
        .and_then(|p| p.daily_calls)
        .unwrap_or(DAILY_CALLS)
}

fn failed(code: &str, args: Value) -> WebVisitNodeReply {
    WebVisitNodeReply {
        port: NODE_PORT_FAILED,
        code: Some(code.to_string()),
        args,
        ..Default::default()
    }
}

/// An HTML page read, or why not.
async fn page(origin: &str, host: &str, u: &reqwest::Url) -> Result<(u16, String, html::Page), ()> {
    pace(host).await;
    let f = fetch(origin, u).await.map_err(|_| ())?;
    let is_html = f.content_type.is_empty() || f.content_type.contains("html");
    if !(200..300).contains(&f.status) || !is_html {
        return Err(());
    }
    let p = html::read(&f.body, &f.url);
    Ok((f.status, f.url.to_string(), p))
}

/// Reads `raw` and, when asked, its legal notice.
pub async fn visit(raw: &str, legal_page: bool) -> WebVisitNodeReply {
    let raw = raw.trim();
    let parsed = reqwest::Url::parse(raw).ok().filter(|u| {
        raw.len() <= URL_MAX && pnex_core::egress::check_url(u).is_none() && u.host_str().is_some()
    });
    let (Some(u), Some(origin)) = (parsed, pnex_core::browser_agent::site_origin(raw)) else {
        return failed(err_codes::WEB_VISIT_URL_INVALID, Value::Null);
    };
    let host = u.host_str().unwrap_or_default().to_ascii_lowercase();
    // robots.txt: missing or unreadable means everything is allowed.
    let robots = match reqwest::Url::parse(&format!("{origin}/robots.txt")) {
        Ok(r) => {
            pace(&host).await;
            match fetch(&origin, &r).await {
                Ok(f) if f.status == 200 => Robots::parse(&f.body),
                _ => Robots::default(),
            }
        }
        Err(_) => Robots::default(),
    };
    let allowed = |u: &reqwest::Url| {
        let path = match u.query() {
            Some(q) => format!("{}?{q}", u.path()),
            None => u.path().to_string(),
        };
        robots.allows(&path)
    };
    if !allowed(&u) {
        return failed(err_codes::WEB_VISIT_BLOCKED, Value::Null);
    }
    let Ok((status, final_url, home)) = page(&origin, &host, &u).await else {
        return failed(err_codes::WEB_VISIT_UNREACHABLE, Value::Null);
    };
    let mut reply = WebVisitNodeReply {
        port: NODE_PORT_OK,
        final_url,
        status,
        title: home.title,
        text: clip(&home.text),
        ..Default::default()
    };
    let legal = legal_page
        .then(|| legal_link(&origin, &home.links))
        .flatten()
        .and_then(|l| reqwest::Url::parse(l).ok())
        .filter(|l| allowed(l));
    if let Some(l) = legal {
        // A legal notice that fails leaves the visit as it is.
        if let Ok((_, at, p)) = page(&origin, &host, &l).await {
            reply.legal_url = at;
            reply.legal_text = clip(&p.text);
        }
    }
    reply
}

/// One visit of the deployed node `req.node`.
pub async fn run(
    ctx: &AppContext,
    req: &WebVisitNodeRequest,
) -> Result<WebVisitNodeReply, NodeError> {
    let FlowNodeKind::WebVisit { config } = deployed(&ctx.db, &req.node).await? else {
        return Err(NodeError::NotDeployed);
    };
    let org_id = req.node.org_id;
    if req.url.trim().is_empty() || req.url.len() > URL_MAX {
        return Ok(failed(err_codes::WEB_VISIT_URL_INVALID, Value::Null));
    }
    let cap = daily_cap(ctx);
    match crate::services::flow_ai::count_daily(ctx, "web-visit", org_id, cap).await {
        Ok(()) => {}
        Err(crate::services::flow_ai::FlowAiError::Quota(cap)) => {
            return Ok(failed(err_codes::WEB_VISIT_QUOTA, json!({ "cap": cap })))
        }
        Err(e) => {
            tracing::warn!(org_id, error = ?e, "web visit quota counter failed");
            return Ok(failed(err_codes::WEB_VISIT_UNREACHABLE, Value::Null));
        }
    }
    let mut reply = visit(&req.url, config.legal_page).await;
    if config.screenshot && reply.port == NODE_PORT_OK {
        // A capture that fails leaves the visit as it is.
        match capture(ctx, org_id, &reply.final_url).await {
            Ok(id) => reply.screenshot_asset_id = id.to_string(),
            Err(e) => tracing::warn!(org_id, error = %e, "web visit capture skipped"),
        }
    }
    Ok(reply)
}

/// The page captured by the org's browser, as an image of its media
/// library (quota of the org's storage).
async fn capture(ctx: &AppContext, org_id: i64, url: &str) -> Result<uuid::Uuid, String> {
    use crate::models::_entities::media_assets;
    use sea_orm::{ActiveModelTrait, Set};
    let ring = crate::services::secrets::Keyring::from_config(&ctx.config).ok();
    let shot = tokio::time::timeout(
        Duration::from_secs(60),
        crate::services::site_recipes::endpoints::capture(&ctx.db, ring.as_ref(), org_id, url),
    )
    .await
    .map_err(|_| "timed out".to_string())??;
    let asset = media_assets::ActiveModel {
        id: Set(uuid::Uuid::new_v4()),
        org_id: Set(org_id),
        kind: Set("image".into()),
        name: Set(url.chars().take(255).collect()),
        description: Set(None),
        metadata: Set(Some(json!({ "source": "web_visit", "source_url": url }))),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .map_err(|e| e.to_string())?;
    let settings = crate::services::media::MediaSettings::from_config(&ctx.config);
    crate::controllers::media::write_version(
        ctx,
        org_id,
        &asset,
        crate::controllers::media::IncomingVersion {
            filename: "capture.jpg".into(),
            content_type: "image/jpeg".into(),
            note: None,
            bytes: shot.into(),
        },
        &settings,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(asset.id)
}
