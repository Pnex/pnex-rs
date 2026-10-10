//! `GET /api/v1/search` — global typeahead across the org's objects (D69).
//!
//! Additive endpoint: `pnex_api_contract::CONTRACT` is NOT bumped. Deliberate
//! D14 deviation, documented in `docs/contracts/search.http` and decision
//! D69: typeahead shape `{count, q, groups}` instead of the paginated
//! envelope. Absent/empty `q` returns an empty 200, never a 400.
//!
//! Matching is `Func::lower(col) LIKE pattern ESCAPE '\'` with the pattern
//! lowercased in Rust (shared with the paginated lists).
//!
//! Each searchable entity contributes one org-scoped containment query; the
//! 9 groups (10 SQL queries, edge refs merge two tables) run concurrently
//! (`tokio::join!`, school `controllers/dashboard.rs`). Merging + light
//! ranking happen in Rust: prefix matches beat substring matches, recency
//! order preserved within each class.

use axum::extract::{Query, State};
use axum::routing::get;
use loco_rs::prelude::*;
use sea_orm::sea_query::{Expr, Func, IntoColumnRef, LikeExpr};
use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, DbErr, ExprTrait, QueryOrder, QuerySelect,
};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;

use crate::auth::OrgContext;
use crate::controllers::pagination;
use crate::models::_entities::{
    annotation_layers, dashboards, device_registries, flows, functions, map_pins, media_assets,
    pnex_hosts, tours, wifi_credentials,
};
use crate::services::pois::PIN_MODE_GEO;

/// Per-group cap — typeahead sanity bound (limit silently clamps here).
const MAX_TAKE: i64 = 20;
/// Candidate overshoot factor: prefix matches must survive the SQL LIMIT
/// even when newer substring matches exist (SQL fetches recency-first).
const OVERSHOOT: u64 = 3;
/// LIKE escape character (emitted as an explicit ESCAPE clause).
const LIKE_ESCAPE: char = '\\';

pub fn routes() -> Routes {
    Routes::new().prefix("/api/v1").add("/search", get(search))
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: Option<String>,
    limit: Option<String>,
}

/// One result row, uniform across entity types. `id` is a String on the
/// wire (i64 PKs stringified, UUIDs as-is) — the frontend re-types it at
/// navigation time.
#[derive(Debug, Serialize)]
struct SearchHit {
    id: String,
    title: String,
    subtitle: Option<String>,
    updated_at: String,
}

/// Results grouped by entity type, canonical order, empty groups omitted.
#[derive(Debug, Serialize)]
struct SearchGroup {
    entity_type: &'static str,
    results: Vec<SearchHit>,
}

/// Typeahead response — deliberate D14 deviation, see module header.
#[derive(Debug, Serialize)]
struct SearchResponse {
    count: i64,
    q: String,
    groups: Vec<SearchGroup>,
}

/// Row-level fetch result with its ranking key: `prefix` flags a
/// case-insensitive starts_with on the title column. The recency order is
/// carried by the SQL fetch itself (`updated_at DESC` + stable sort).
struct Candidate {
    prefix: bool,
    hit: SearchHit,
}

/// Escape LIKE wildcards so user `%`, `_` and `\` stay literal. Paired with
/// `LikeExpr::escape('\\')`. The pattern is a bind parameter;
/// this is about wildcard semantics, not injection.
fn escape_like(term: &str) -> String {
    let mut out = String::with_capacity(term.len() + 8);
    for c in term.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push(LIKE_ESCAPE);
        }
        out.push(c);
    }
    out
}

/// Case-insensitive containment on one column:
/// `Func::lower(col) LIKE pattern ESCAPE '\'`.
fn contains_lower(col: impl IntoColumnRef, pattern: &str) -> Expr {
    Expr::expr(Func::lower(Expr::col(col)))
        .like(LikeExpr::new(pattern.to_string()).escape(LIKE_ESCAPE))
}

/// Split the search term into AND tokens: whitespace-separated, each token
/// stripped of edge punctuation (a trailing `---` or stray quote is search
/// noise, not a literal — `esp---` must find the esp devices). Internal
/// punctuation stays significant and is escaped for LIKE (`esp32-relay`,
/// `100%d`). If stripping empties every token, the raw lowercased term is
/// kept as a single literal token.
fn search_tokens(term: &str) -> Vec<String> {
    let tokens: Vec<String> = term
        .split_whitespace()
        .map(|t| {
            t.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.is_empty() {
        vec![term.to_lowercase()]
    } else {
        tokens
    }
}

/// AND-of-tokens, each token OR-of-columns (case-insensitive containment).
macro_rules! match_tokens {
    ($patterns:expr, $entity:ident, [$($col:ident),+ $(,)?]) => {{
        let mut all = Condition::all();
        for pat in $patterns {
            let mut any = Condition::any();
            $(any = any.add(contains_lower(($entity::Entity, $entity::Column::$col), pat));)+
            all = all.add(any);
        }
        all
    }};
}

type Fetched = Result<Vec<Candidate>, DbErr>;

/// Devices — matched on `device_id` (registries carry no name column).
async fn fetch_devices(
    db: &DatabaseConnection,
    org_id: i64,
    patterns: &[String],
    prefix_token: &str,
    candidate_limit: u64,
) -> Fetched {
    let rows = device_registries::Entity::find()
        .filter(device_registries::Column::OrgId.eq(org_id))
        .filter(match_tokens!(patterns, device_registries, [DeviceId]))
        .order_by_desc(device_registries::Column::UpdatedAt)
        .limit(candidate_limit)
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| Candidate {
            prefix: r.device_id.to_lowercase().starts_with(prefix_token),
            hit: SearchHit {
                id: r.id.to_string(),
                title: r.device_id,
                subtitle: None,
                updated_at: r.updated_at.to_rfc3339(),
            },
        })
        .collect())
}

/// POIs (map pins) — geo mode + positioned only, school
/// `services/pois.rs::filtered_pins`: search must not return rows the map
/// page cannot render.
async fn fetch_pois(
    db: &DatabaseConnection,
    org_id: i64,
    patterns: &[String],
    prefix_token: &str,
    candidate_limit: u64,
) -> Fetched {
    let rows = map_pins::Entity::find()
        .filter(map_pins::Column::OrgId.eq(org_id))
        .filter(map_pins::Column::Mode.eq(PIN_MODE_GEO))
        .filter(map_pins::Column::Latitude.is_not_null())
        .filter(map_pins::Column::Longitude.is_not_null())
        .filter(match_tokens!(patterns, map_pins, [Label, LocationDetail]))
        .order_by_desc(map_pins::Column::UpdatedAt)
        .limit(candidate_limit)
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| Candidate {
            prefix: r.label.to_lowercase().starts_with(prefix_token),
            hit: SearchHit {
                id: r.id.to_string(),
                title: r.label,
                subtitle: r.location_detail,
                updated_at: r.updated_at.to_rfc3339(),
            },
        })
        .collect())
}

/// Tours — name + description.
async fn fetch_tours(
    db: &DatabaseConnection,
    org_id: i64,
    patterns: &[String],
    prefix_token: &str,
    candidate_limit: u64,
) -> Fetched {
    let rows = tours::Entity::find()
        .filter(tours::Column::OrgId.eq(org_id))
        .filter(match_tokens!(patterns, tours, [Name, Description]))
        .order_by_desc(tours::Column::UpdatedAt)
        .limit(candidate_limit)
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| Candidate {
            prefix: r.name.to_lowercase().starts_with(prefix_token),
            hit: SearchHit {
                id: r.id.to_string(),
                title: r.name,
                subtitle: r.description,
                updated_at: r.updated_at.to_rfc3339(),
            },
        })
        .collect())
}

/// Media assets — name + description.
async fn fetch_media(
    db: &DatabaseConnection,
    org_id: i64,
    patterns: &[String],
    prefix_token: &str,
    candidate_limit: u64,
) -> Fetched {
    let rows = media_assets::Entity::find()
        .filter(media_assets::Column::OrgId.eq(org_id))
        .filter(match_tokens!(patterns, media_assets, [Name, Description]))
        .order_by_desc(media_assets::Column::UpdatedAt)
        .limit(candidate_limit)
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| Candidate {
            prefix: r.name.to_lowercase().starts_with(prefix_token),
            hit: SearchHit {
                id: r.id.to_string(),
                title: r.name,
                subtitle: r.description,
                updated_at: r.updated_at.to_rfc3339(),
            },
        })
        .collect())
}

/// Annotation layers — name + description.
async fn fetch_layers(
    db: &DatabaseConnection,
    org_id: i64,
    patterns: &[String],
    prefix_token: &str,
    candidate_limit: u64,
) -> Fetched {
    let rows = annotation_layers::Entity::find()
        .filter(annotation_layers::Column::OrgId.eq(org_id))
        .filter(match_tokens!(
            patterns,
            annotation_layers,
            [Name, Description]
        ))
        .order_by_desc(annotation_layers::Column::UpdatedAt)
        .limit(candidate_limit)
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| Candidate {
            prefix: r.name.to_lowercase().starts_with(prefix_token),
            hit: SearchHit {
                id: r.id.to_string(),
                title: r.name,
                subtitle: r.description,
                updated_at: r.updated_at.to_rfc3339(),
            },
        })
        .collect())
}

/// Functions — name + description, language as subtitle.
async fn fetch_functions(
    db: &DatabaseConnection,
    org_id: i64,
    patterns: &[String],
    prefix_token: &str,
    candidate_limit: u64,
) -> Fetched {
    let rows = functions::Entity::find()
        .filter(functions::Column::OrgId.eq(org_id))
        .filter(match_tokens!(patterns, functions, [Name, Description]))
        .order_by_desc(functions::Column::UpdatedAt)
        .limit(candidate_limit)
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| Candidate {
            prefix: r.name.to_lowercase().starts_with(prefix_token),
            hit: SearchHit {
                id: r.id.to_string(),
                title: r.name,
                subtitle: Some(r.language),
                updated_at: r.updated_at.to_rfc3339(),
            },
        })
        .collect())
}

/// Flows — matched on name only, status as subtitle.
async fn fetch_flows(
    db: &DatabaseConnection,
    org_id: i64,
    patterns: &[String],
    prefix_token: &str,
    candidate_limit: u64,
) -> Fetched {
    let rows = flows::Entity::find()
        .filter(flows::Column::OrgId.eq(org_id))
        .filter(match_tokens!(patterns, flows, [Name]))
        .order_by_desc(flows::Column::UpdatedAt)
        .limit(candidate_limit)
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| Candidate {
            prefix: r.name.to_lowercase().starts_with(prefix_token),
            hit: SearchHit {
                id: r.id.to_string(),
                title: r.name,
                subtitle: Some(r.status),
                updated_at: r.updated_at.to_rfc3339(),
            },
        })
        .collect())
}

/// Dashboards — name + description.
async fn fetch_dashboards(
    db: &DatabaseConnection,
    org_id: i64,
    patterns: &[String],
    prefix_token: &str,
    candidate_limit: u64,
) -> Fetched {
    let rows = dashboards::Entity::find()
        .filter(dashboards::Column::OrgId.eq(org_id))
        .filter(match_tokens!(patterns, dashboards, [Name, Description]))
        .order_by_desc(dashboards::Column::UpdatedAt)
        .limit(candidate_limit)
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| Candidate {
            prefix: r.name.to_lowercase().starts_with(prefix_token),
            hit: SearchHit {
                id: r.id.to_string(),
                title: r.name,
                subtitle: r.description,
                updated_at: r.updated_at.to_rfc3339(),
            },
        })
        .collect())
}

/// Edge referentials — WiFi credentials + PNeX hosts merged into one
/// group; the subtitle carries the kind so the two stay distinguishable.
async fn fetch_edge_refs(
    db: &DatabaseConnection,
    org_id: i64,
    patterns: &[String],
    prefix_token: &str,
    candidate_limit: u64,
) -> Fetched {
    let wifi = wifi_credentials::Entity::find()
        .filter(wifi_credentials::Column::OrgId.eq(org_id))
        .filter(match_tokens!(patterns, wifi_credentials, [Ssid]))
        .order_by_desc(wifi_credentials::Column::UpdatedAt)
        .limit(candidate_limit)
        .all(db)
        .await?;
    let hosts = pnex_hosts::Entity::find()
        .filter(pnex_hosts::Column::OrgId.eq(org_id))
        .filter(match_tokens!(patterns, pnex_hosts, [Host]))
        .order_by_desc(pnex_hosts::Column::UpdatedAt)
        .limit(candidate_limit)
        .all(db)
        .await?;
    let mut out = Vec::with_capacity(wifi.len() + hosts.len());
    out.extend(wifi.into_iter().map(|r| Candidate {
        prefix: r.ssid.to_lowercase().starts_with(prefix_token),
        hit: SearchHit {
            id: r.id.to_string(),
            title: r.ssid,
            subtitle: Some("wifi".to_string()),
            updated_at: r.updated_at.to_rfc3339(),
        },
    }));
    out.extend(hosts.into_iter().map(|r| Candidate {
        prefix: r.host.to_lowercase().starts_with(prefix_token),
        hit: SearchHit {
            id: r.id.to_string(),
            title: r.host,
            subtitle: Some("host".to_string()),
            updated_at: r.updated_at.to_rfc3339(),
        },
    }));
    Ok(out)
}

/// Stable ranking: prefix matches first, recency order preserved within
/// each class; then truncate to the per-group cap.
fn rank_and_cap(mut candidates: Vec<Candidate>, take: i64) -> Vec<SearchHit> {
    candidates.sort_by_key(|c| Reverse(c.prefix));
    candidates.truncate(take as usize);
    candidates.into_iter().map(|c| c.hit).collect()
}

/// Handler: empty `q` → empty 200 (silent default, never 400); otherwise
/// the 9 groups run concurrently and reassemble in canonical order.
async fn search(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Query(q): Query<SearchQuery>,
) -> Result<Response> {
    let term = q.q.as_deref().map(str::trim).unwrap_or("");
    if term.is_empty() {
        return format::json(SearchResponse {
            count: 0,
            q: String::new(),
            groups: vec![],
        });
    }

    let page = pagination::PageParams::from(q.limit.as_deref(), None);
    let take = page.limit.clamp(1, MAX_TAKE);
    let tokens = search_tokens(term);
    let prefix_token = tokens.first().map(String::as_str).unwrap_or("");
    let patterns: Vec<String> = tokens
        .iter()
        .map(|t| format!("%{}%", escape_like(t)))
        .collect();
    let candidate_limit = (take as u64 * OVERSHOOT).clamp(6, 30);
    let org_id = org.org.id;

    let (dev, poi, tour, media, layer, func, flow, dash, refs) = tokio::join!(
        fetch_devices(&ctx.db, org_id, &patterns, prefix_token, candidate_limit),
        fetch_pois(&ctx.db, org_id, &patterns, prefix_token, candidate_limit),
        fetch_tours(&ctx.db, org_id, &patterns, prefix_token, candidate_limit),
        fetch_media(&ctx.db, org_id, &patterns, prefix_token, candidate_limit),
        fetch_layers(&ctx.db, org_id, &patterns, prefix_token, candidate_limit),
        fetch_functions(&ctx.db, org_id, &patterns, prefix_token, candidate_limit),
        fetch_flows(&ctx.db, org_id, &patterns, prefix_token, candidate_limit),
        fetch_dashboards(&ctx.db, org_id, &patterns, prefix_token, candidate_limit),
        fetch_edge_refs(&ctx.db, org_id, &patterns, prefix_token, candidate_limit),
    );

    let mut groups = Vec::new();
    for (entity_type, fetched) in [
        ("device", dev),
        ("poi", poi),
        ("tour", tour),
        ("media", media),
        ("layer", layer),
        ("function", func),
        ("flow", flow),
        ("dashboard", dash),
        ("edge_ref", refs),
    ] {
        let hits = rank_and_cap(fetched.map_err(|_| Error::InternalServerError)?, take);
        if !hits.is_empty() {
            groups.push(SearchGroup {
                entity_type,
                results: hits,
            });
        }
    }
    let count: i64 = groups.iter().map(|g| g.results.len() as i64).sum();
    format::json(SearchResponse {
        count,
        q: term.to_string(),
        groups,
    })
}
