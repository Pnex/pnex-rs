//! `GET /api/v1/search` — global typeahead across the org's objects (D69).
//!
//! Typeahead shape `{count, q, groups}`: a computed result (each group
//! capped by `limit`), not a paginated collection (D69, D240). Absent/empty
//! `q` returns an empty 200, never a 400.
//!
//! Matching is `Func::lower(col) LIKE pattern ESCAPE '\'` with the pattern
//! lowercased in Rust (shared with the paginated lists).
//!
//! Each searchable entity contributes one org-scoped containment query; the
//! 10 groups (11 SQL queries, edge refs merge two tables) run concurrently
//! (`tokio::join!`, school `controllers/dashboard.rs`). Merging + light
//! ranking happen in Rust: prefix matches beat substring matches, recency
//! order preserved within each class.

use axum::extract::{Query, State};
use axum::routing::get;
use loco_rs::prelude::*;
use pnex_core::fold::fold;
use sea_orm::sea_query::{Alias, Expr, Func, IntoColumnRef, LikeExpr, SimpleExpr};
use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, DbErr, ExprTrait, QueryOrder, QuerySelect,
};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;

use crate::auth::OrgContext;
use crate::controllers::pagination;
use crate::models::_entities::{
    annotation_layers, dashboards, device_registries, flows, functions, map_pins, media_assets,
    objects, pnex_hosts, tours, wifi_credentials,
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

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct SearchQuery {
    /// Search term (whitespace-separated tokens, all must match). Absent or empty = empty answer.
    q: Option<String>,
    /// Max hits per group, clamped to 1..=20.
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
}

/// One result row, uniform across entity types. `id` is a String on the
/// wire (i64 PKs stringified, UUIDs as-is) — the frontend re-types it at
/// navigation time.
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct SearchHit {
    id: String,
    title: String,
    subtitle: Option<String>,
    /// RFC 3339; absent for label and document hits.
    #[schema(format = DateTime)]
    updated_at: Option<String>,
    /// Kind of the hit when the group mixes kinds (labels, D218).
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
}

/// Entity type of a result group.
#[derive(Debug, Clone, Copy, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
enum SearchEntityType {
    Label,
    Device,
    Poi,
    Tour,
    Media,
    Document,
    Layer,
    Function,
    Flow,
    Dashboard,
    EdgeRef,
    Object,
}

/// Results grouped by entity type, canonical order, empty groups omitted.
#[derive(Debug, Serialize, utoipa::ToSchema)]
struct SearchGroup {
    entity_type: SearchEntityType,
    results: Vec<SearchHit>,
}

/// Typeahead response: a computed result (groups capped per type), not a
/// paginated collection.
#[derive(Debug, Serialize, utoipa::ToSchema)]
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

/// Case- and accent-insensitive containment on one column (« elodie »
/// finds « Élodie », crm.md D447): the column folded by the shared table,
/// `LIKE pattern ESCAPE '\'`, the pattern folded by [`search_tokens`].
fn contains_lower(col: impl IntoColumnRef, pattern: &str) -> Expr {
    let f = pnex_core::fold::sql_fold();
    let mut e: SimpleExpr = Func::lower(Expr::col(col)).into();
    for (v, ascii) in f.ligatures {
        e = Func::cust(Alias::new("replace"))
            .arg(e)
            .arg(v)
            .arg(ascii)
            .into();
    }
    Expr::expr(
        Func::cust(Alias::new("translate"))
            .arg(e)
            .arg(f.from)
            .arg(f.to),
    )
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
        .map(|t| fold(t.trim_matches(|c: char| !c.is_alphanumeric())))
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.is_empty() {
        vec![fold(term)]
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
            prefix: fold(&r.device_id).starts_with(prefix_token),
            hit: SearchHit {
                id: r.id.to_string(),
                title: r.device_id,
                subtitle: None,
                updated_at: Some(r.updated_at.to_rfc3339()),
                kind: None,
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
            prefix: fold(&r.label).starts_with(prefix_token),
            hit: SearchHit {
                id: r.id.to_string(),
                title: r.label,
                subtitle: r.location_detail,
                updated_at: Some(r.updated_at.to_rfc3339()),
                kind: None,
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
            prefix: fold(&r.name).starts_with(prefix_token),
            hit: SearchHit {
                id: r.id.to_string(),
                title: r.name,
                subtitle: r.description,
                updated_at: Some(r.updated_at.to_rfc3339()),
                kind: None,
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
            prefix: fold(&r.name).starts_with(prefix_token),
            hit: SearchHit {
                id: r.id.to_string(),
                title: r.name,
                subtitle: r.description,
                updated_at: Some(r.updated_at.to_rfc3339()),
                kind: None,
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
            prefix: fold(&r.name).starts_with(prefix_token),
            hit: SearchHit {
                id: r.id.to_string(),
                title: r.name,
                subtitle: r.description,
                updated_at: Some(r.updated_at.to_rfc3339()),
                kind: None,
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
            prefix: fold(&r.name).starts_with(prefix_token),
            hit: SearchHit {
                id: r.id.to_string(),
                title: r.name,
                subtitle: Some(r.language),
                updated_at: Some(r.updated_at.to_rfc3339()),
                kind: None,
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
            prefix: fold(&r.name).starts_with(prefix_token),
            hit: SearchHit {
                id: r.id.to_string(),
                title: r.name,
                subtitle: Some(r.status),
                updated_at: Some(r.updated_at.to_rfc3339()),
                kind: None,
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
            prefix: fold(&r.name).starts_with(prefix_token),
            hit: SearchHit {
                id: r.id.to_string(),
                title: r.name,
                subtitle: r.description,
                updated_at: Some(r.updated_at.to_rfc3339()),
                kind: None,
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
        prefix: fold(&r.ssid).starts_with(prefix_token),
        hit: SearchHit {
            id: r.id.to_string(),
            title: r.ssid,
            subtitle: Some("wifi".to_string()),
            updated_at: Some(r.updated_at.to_rfc3339()),
            kind: None,
        },
    }));
    out.extend(hosts.into_iter().map(|r| Candidate {
        prefix: fold(&r.host).starts_with(prefix_token),
        hit: SearchHit {
            id: r.id.to_string(),
            title: r.host,
            subtitle: Some("host".to_string()),
            updated_at: Some(r.updated_at.to_rfc3339()),
            kind: None,
        },
    }));
    Ok(out)
}

/// Ontology objects of the org's own types (D185 → D69): system objects
/// are already found through their native group. Subtitle = type key.
async fn fetch_objects(
    db: &DatabaseConnection,
    org_id: i64,
    patterns: &[String],
    prefix_token: &str,
    candidate_limit: u64,
) -> Fetched {
    let system: Vec<String> = pnex_core::ontology::system_object_types()
        .into_iter()
        .filter(|t| t.native())
        .map(|t| t.key)
        .collect();
    let rows = objects::Entity::find()
        .filter(objects::Column::OrgId.eq(org_id))
        .filter(objects::Column::ValidTo.is_null())
        .filter(objects::Column::TypeKey.is_not_in(system))
        .filter(match_tokens!(patterns, objects, [Title]))
        .order_by_desc(objects::Column::UpdatedAt)
        .limit(candidate_limit)
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| Candidate {
            prefix: fold(&r.title).starts_with(prefix_token),
            hit: SearchHit {
                id: r.id.to_string(),
                title: r.title,
                subtitle: Some(r.type_key),
                updated_at: Some(r.updated_at.to_rfc3339()),
                kind: None,
            },
        })
        .collect())
}

/// Document contents (O54): full-text hits of the org's indexed documents,
/// one row per asset (best chunk first). Subtitle = the excerpt, match
/// markers stripped; the hit opens the media asset.
async fn fetch_documents(
    db: &DatabaseConnection,
    org_id: i64,
    term: &str,
    candidate_limit: u64,
) -> Fetched {
    let hits = crate::services::doc_search::search(db, org_id, term, None, candidate_limit).await?;
    let mut seen = std::collections::HashSet::new();
    Ok(hits
        .into_iter()
        .filter(|h| seen.insert(h.asset_id))
        .map(|h| Candidate {
            prefix: false,
            hit: SearchHit {
                id: h.asset_id.to_string(),
                title: h.asset_name,
                subtitle: Some(h.snippet.replace(['⟦', '⟧'], "")),
                updated_at: None,
                kind: None,
            },
        })
        .collect())
}

/// Labelled objects of any kind (D218): `#value` / `#name:value` matches a
/// whole label, plain text a contained one. Subtitle = the labels.
async fn fetch_labels(
    db: &DatabaseConnection,
    org_id: i64,
    term: &str,
    candidate_limit: u64,
) -> Fetched {
    use crate::services::resources::{labels, registry};
    let (exact, needle) = match term.strip_prefix('#') {
        Some(rest) => (true, rest.trim()),
        None => (false, term),
    };
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let hits = labels::search_labeled(db, org_id, needle, exact, candidate_limit).await?;
    if hits.is_empty() {
        return Ok(Vec::new());
    }
    let reg = registry::for_org(db, org_id).await?;
    let mut out = Vec::new();
    for (kind, id, set) in hits {
        let name = match labels::label_only_name(db, org_id, &kind, &id).await? {
            Some(n) => Some(n),
            None => match reg.entry(&kind) {
                Some(e) => e.resolver.display_name(db, org_id, &id).await?,
                None => None,
            },
        };
        // A row deleted since (or of an unknown kind) is not shown.
        let Some(title) = name else { continue };
        let subtitle = set
            .iter()
            .map(|(k, v)| match v {
                Some(v) => format!("{k}:{v}"),
                None => k.clone(),
            })
            .collect::<Vec<_>>()
            .join(" · ");
        out.push(Candidate {
            prefix: exact,
            hit: SearchHit {
                id,
                title,
                subtitle: Some(subtitle),
                updated_at: None,
                kind: Some(kind),
            },
        });
    }
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
/// the 11 groups run concurrently and reassemble in canonical order.
#[utoipa::path(
    get,
    path = "/api/v1/search",
    tag = "search",
    operation_id = "search_global",
    summary = "Global typeahead search",
    description = "Searches the org objects (labels, devices, POIs, tours, media, documents, annotation layers, functions, flows, dashboards, edge references, ontology objects) by name. Results are grouped by entity type in a fixed order, empty groups omitted; within a group prefix matches come first, then most recently updated. Not paginated: `limit` caps each group (max 20). An empty `q` returns an empty 200. Read-only, any org member.",
    params(SearchQuery),
    responses(
        (status = 200, body = SearchResponse)
    ),
    security(("bearer" = [], "org" = []))
)]
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

    let (labels, dev, poi, tour, media, docs, layer, func, flow, dash, refs, objs) = tokio::join!(
        fetch_labels(&ctx.db, org_id, term, candidate_limit),
        fetch_devices(&ctx.db, org_id, &patterns, prefix_token, candidate_limit),
        fetch_pois(&ctx.db, org_id, &patterns, prefix_token, candidate_limit),
        fetch_tours(&ctx.db, org_id, &patterns, prefix_token, candidate_limit),
        fetch_media(&ctx.db, org_id, &patterns, prefix_token, candidate_limit),
        fetch_documents(&ctx.db, org_id, term, candidate_limit),
        fetch_layers(&ctx.db, org_id, &patterns, prefix_token, candidate_limit),
        fetch_functions(&ctx.db, org_id, &patterns, prefix_token, candidate_limit),
        fetch_flows(&ctx.db, org_id, &patterns, prefix_token, candidate_limit),
        fetch_dashboards(&ctx.db, org_id, &patterns, prefix_token, candidate_limit),
        fetch_edge_refs(&ctx.db, org_id, &patterns, prefix_token, candidate_limit),
        fetch_objects(&ctx.db, org_id, &patterns, prefix_token, candidate_limit),
    );

    let mut groups = Vec::new();
    for (entity_type, fetched) in [
        (SearchEntityType::Label, labels),
        (SearchEntityType::Device, dev),
        (SearchEntityType::Poi, poi),
        (SearchEntityType::Tour, tour),
        (SearchEntityType::Media, media),
        (SearchEntityType::Document, docs),
        (SearchEntityType::Layer, layer),
        (SearchEntityType::Function, func),
        (SearchEntityType::Flow, flow),
        (SearchEntityType::Dashboard, dash),
        (SearchEntityType::EdgeRef, refs),
        (SearchEntityType::Object, objs),
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

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(search))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
