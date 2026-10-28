//! The `org_resolve` node (account-mapping.md D332): a company name (and a
//! place) → scored SIREN candidates from the public company register
//! (the French company search API). Writes nothing; the thresholds are read
//! from the deployed node, never from the runtime (R1).

use std::time::Duration;

use loco_rs::app::AppContext;
use pnex_core::err_codes;
use pnex_core::flow::FlowNodeKind;
use pnex_core::org_resolve::{
    port_of, score, OrgCandidate, OrgQuery, OrgResolveNodeReply, OrgResolveNodeRequest,
    RegisterEntry, NAME_MAX, NODE_PORT_FAILED, NODE_PORT_MATCHED,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::services::ontology::flow_nodes::{deployed, NodeError};

const TIMEOUT: Duration = Duration::from_secs(15);
/// Lookups of an org per UTC day, unless `settings.org_resolve.daily_calls`.
const DAILY_CALLS: u64 = 2000;
/// Entries read from the register per lookup, before scoring.
const PER_PAGE: &str = "10";
const REGISTER_URL: &str = "https://recherche-entreprises.api.gouv.fr/search";

/// The register, overridable for tests (`PNEX_COMPANY_SEARCH_URL`).
fn register_url() -> String {
    std::env::var("PNEX_COMPANY_SEARCH_URL").unwrap_or_else(|_| REGISTER_URL.to_string())
}

fn http() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        pnex_core::egress::guarded(
            reqwest::Client::builder()
                .redirect(pnex_core::egress::redirect_policy(3))
                .timeout(TIMEOUT)
                .user_agent(concat!("PNEX/", env!("CARGO_PKG_VERSION"))),
        )
        .build()
        .expect("reqwest client")
    })
}

#[derive(Deserialize)]
struct CapPartial {
    daily_calls: Option<u64>,
}

fn daily_cap(ctx: &AppContext) -> u64 {
    ctx.config
        .settings
        .as_ref()
        .and_then(|s| s.get("org_resolve"))
        .and_then(|v| serde_json::from_value::<CapPartial>(v.clone()).ok())
        .and_then(|p| p.daily_calls)
        .unwrap_or(DAILY_CALLS)
}

fn failed(code: &str, args: Value) -> OrgResolveNodeReply {
    OrgResolveNodeReply {
        port: NODE_PORT_FAILED,
        code: Some(code.to_string()),
        args,
        ..Default::default()
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Place {
    code_postal: Option<String>,
    libelle_commune: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Hit {
    siren: String,
    nom_complet: Option<String>,
    nom_raison_sociale: Option<String>,
    sigle: Option<String>,
    etat_administratif: Option<String>,
    activite_principale: Option<String>,
    tranche_effectif_salarie: Option<String>,
    siege: Place,
    matching_etablissements: Vec<Place>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Page {
    results: Vec<Hit>,
}

/// One page of the register for `name`, narrowed to `postcode` if given.
async fn lookup(name: &str, postcode: &str) -> Result<Vec<Hit>, String> {
    let mut url = reqwest::Url::parse(&register_url()).map_err(|e| e.to_string())?;
    url.query_pairs_mut()
        .append_pair("q", name)
        .append_pair("per_page", PER_PAGE);
    if !postcode.is_empty() {
        url.query_pairs_mut().append_pair("code_postal", postcode);
    }
    let res = http().get(url).send().await.map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!("status {}", res.status()));
    }
    let page: Page = res.json().await.map_err(|e| e.to_string())?;
    Ok(page.results)
}

/// Scored candidates, best first.
fn candidates(q: &OrgQuery, hits: Vec<Hit>) -> Vec<OrgCandidate> {
    let text = |s: &Option<String>| s.clone().unwrap_or_default();
    let mut out: Vec<OrgCandidate> = hits
        .into_iter()
        .filter(|h| !h.siren.is_empty())
        .map(|h| {
            let place = |p: &Place| (text(&p.code_postal), text(&p.libelle_commune));
            let entry = RegisterEntry {
                siren: h.siren.clone(),
                names: [&h.nom_complet, &h.nom_raison_sociale, &h.sigle]
                    .into_iter()
                    .filter_map(|n| n.clone())
                    .collect(),
                places: std::iter::once(&h.siege)
                    .chain(&h.matching_etablissements)
                    .map(place)
                    .collect(),
                active: h.etat_administratif.as_deref() == Some("A"),
            };
            OrgCandidate {
                score: score(q, &entry),
                siren: h.siren,
                name: text(&h.nom_complet),
                city: text(&h.siege.libelle_commune),
                postcode: text(&h.siege.code_postal),
                naf: text(&h.activite_principale),
                headcount_band: text(&h.tranche_effectif_salarie),
                active: entry.active,
            }
        })
        .collect();
    out.sort_by_key(|a| std::cmp::Reverse(a.score));
    out
}

/// One lookup of the deployed node `req.node`.
pub async fn run(
    ctx: &AppContext,
    req: &OrgResolveNodeRequest,
) -> Result<OrgResolveNodeReply, NodeError> {
    let FlowNodeKind::OrgResolve { config } = deployed(&ctx.db, &req.node).await? else {
        return Err(NodeError::NotDeployed);
    };
    let org_id = req.node.org_id;
    let q = OrgQuery {
        name: req.query.name.trim().to_string(),
        city: req.query.city.trim().to_string(),
        postcode: req.query.postcode.trim().to_string(),
    };
    let short = |s: &str| s.chars().count() <= NAME_MAX;
    if q.name.is_empty() || !short(&q.name) || !short(&q.city) || !short(&q.postcode) {
        return Ok(failed(err_codes::ORG_RESOLVE_QUERY_INVALID, Value::Null));
    }
    let cap = daily_cap(ctx);
    match crate::services::flow_ai::count_daily(ctx, "org-resolve", org_id, cap).await {
        Ok(()) => {}
        Err(crate::services::flow_ai::FlowAiError::Quota(cap)) => {
            return Ok(failed(err_codes::ORG_RESOLVE_QUOTA, json!({ "cap": cap })))
        }
        Err(e) => {
            tracing::warn!(org_id, error = ?e, "org resolve quota counter failed");
            return Ok(failed(err_codes::ORG_RESOLVE_UNAVAILABLE, Value::Null));
        }
    }
    // A wrong postcode must not hide the company: retried without it.
    let mut hits = lookup(&q.name, &q.postcode).await;
    if !q.postcode.is_empty() && matches!(&hits, Ok(h) if h.is_empty()) {
        hits = lookup(&q.name, "").await;
    }
    let hits = match hits {
        Ok(h) => h,
        Err(detail) => {
            tracing::warn!(org_id, %detail, "company register lookup failed");
            return Ok(failed(err_codes::ORG_RESOLVE_UNAVAILABLE, Value::Null));
        }
    };
    let mut found = candidates(&q, hits);
    let port = port_of(&config, &found);
    found.truncate(config.limit as usize);
    Ok(OrgResolveNodeReply {
        port,
        siren: (port == NODE_PORT_MATCHED).then(|| found[0].siren.clone()),
        candidates: found,
        ..Default::default()
    })
}
