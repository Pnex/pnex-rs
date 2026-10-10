//! Ontology API (docs/architecture/ontology.md D176–D191) under
//! `/api/v1/ontology`. Org from the principal (R1). Schema writes (types,
//! link types, packs, import) need an org admin; object and link writes
//! need `can_write` and the type's write role (R2, D188). Every member
//! reads.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use pnex_core::err_codes;
use pnex_core::ontology::api::{
    LinkClose, LinkInput, LinkTypeInput, ObjectInput, ObjectTypeInput, OntologyQuery,
};
use pnex_core::ontology::LinkTypeDef;
use serde::Deserialize;
use uuid::Uuid;

use super::coded_error;
use crate::auth::OrgContext;
use crate::services::ontology::{
    self as onto, changes, graph, link_types, links, objects, packs, query, series, types, Actor,
    OntologyError,
};
use crate::services::openobserve::{client::Client, OpenobserveSettings};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/ontology")
        .add("/types", get(list_types).post(create_type))
        .add(
            "/types/{key}",
            get(get_type).put(update_type).delete(delete_type),
        )
        .add("/types/{key}/versions", get(type_versions))
        .add("/link-types", get(list_link_types).post(create_link_type))
        .add(
            "/link-types/{key}",
            axum::routing::put(update_link_type).delete(delete_link_type),
        )
        .add("/objects", post(create_object))
        .add(
            "/objects/{id}",
            get(get_object).put(update_object).delete(archive_object),
        )
        .add("/objects/{id}/links", get(object_links))
        .add("/objects/{id}/graph", get(object_graph))
        .add("/objects/{id}/impact", get(object_impact))
        .add("/objects/{id}/series/{property}", get(object_series))
        .add("/objects/{id}/bindings", get(object_bindings))
        .add("/objects/{id}/changes", get(object_changes))
        .add("/resolve/{kind}/{native_id}", get(resolve))
        .add("/path", get(path))
        .add("/links", post(create_link))
        .add("/links/{id}/close", post(close_link))
        .add("/query", post(run_query))
        .add("/packs", get(list_packs))
        .add("/packs/{key}/install", post(install_pack))
        .add("/export", get(export))
        .add(
            "/import",
            post(import).layer(axum::extract::DefaultBodyLimit::max(
                packs::YAML_MAX_BYTES * 2,
            )),
        )
}

/// Service error → HTTP answer (field errors as `{field: token}`).
pub(crate) fn onto_error(e: OntologyError) -> Result<Response> {
    let coded = |status, code: &str, msg: &str| Err(coded_error(status, code, msg, None));
    match e {
        OntologyError::Invalid { field, token } => Ok((
            StatusCode::BAD_REQUEST,
            format::json(serde_json::json!({ field: token })),
        )
            .into_response()),
        OntologyError::NotFound => coded(
            StatusCode::NOT_FOUND,
            err_codes::ONTOLOGY_NOT_FOUND,
            "Not found in the ontology.",
        ),
        OntologyError::KeyTaken => coded(
            StatusCode::CONFLICT,
            err_codes::ONTOLOGY_ALREADY_EXISTS,
            "It already exists.",
        ),
        OntologyError::VersionConflict { current } => Err(coded_error(
            StatusCode::CONFLICT,
            err_codes::ONTOLOGY_VERSION_CONFLICT,
            "It changed since it was read; reload it.",
            Some(serde_json::json!({ "current": current.to_string() })),
        )),
        OntologyError::TypeInUse => coded(
            StatusCode::CONFLICT,
            err_codes::ONTOLOGY_TYPE_IN_USE,
            "The type is still in use.",
        ),
        OntologyError::WriteForbidden => coded(
            StatusCode::FORBIDDEN,
            err_codes::ONTOLOGY_WRITE_FORBIDDEN,
            "Your role cannot write objects of this type.",
        ),
        OntologyError::SystemReadOnly => coded(
            StatusCode::CONFLICT,
            err_codes::ONTOLOGY_SYSTEM_READ_ONLY,
            "System types and objects are edited from their own pages.",
        ),
        OntologyError::LinkNotAllowed => coded(
            StatusCode::BAD_REQUEST,
            err_codes::ONTOLOGY_LINK_NOT_ALLOWED,
            "This link type does not connect these objects.",
        ),
        OntologyError::LinkCardinality => coded(
            StatusCode::CONFLICT,
            err_codes::ONTOLOGY_LINK_CARDINALITY,
            "An open link already takes this place; close it first.",
        ),
        OntologyError::Db(e) => {
            tracing::error!(error = %e, "ontology database error");
            Err(Error::InternalServerError)
        }
    }
}

fn reply<T: serde::Serialize>(r: onto::Result<T>) -> Result<Response> {
    match r {
        Ok(v) => format::json(v),
        Err(e) => onto_error(e),
    }
}

fn created<T: serde::Serialize>(r: onto::Result<T>) -> Result<Response> {
    match r {
        Ok(v) => Ok((StatusCode::CREATED, format::json(v)).into_response()),
        Err(e) => onto_error(e),
    }
}

fn require_admin(org: &OrgContext) -> Result<()> {
    if org.can_administer() {
        return Ok(());
    }
    Err(coded_error(
        StatusCode::FORBIDDEN,
        err_codes::ONTOLOGY_ADMIN_REQUIRED,
        "Only an org owner or admin edits the ontology schema.",
        None,
    ))
}

fn require_write(org: &OrgContext) -> Result<()> {
    if org.can_write() {
        return Ok(());
    }
    Err(coded_error(
        StatusCode::FORBIDDEN,
        err_codes::ONTOLOGY_WRITE_FORBIDDEN,
        "Viewer role: the ontology is read-only.",
        None,
    ))
}

fn actor(org: &OrgContext) -> Actor {
    Actor::manual(org.auth.user.id, org.role.clone())
}

fn o2(ctx: &AppContext) -> Option<Client> {
    OpenobserveSettings::from_config(&ctx.config).map(|s| Client::new(&s))
}

fn parse_uuid(raw: &str) -> Result<Uuid> {
    Uuid::parse_str(raw).map_err(|_| {
        coded_error(
            StatusCode::NOT_FOUND,
            err_codes::ONTOLOGY_NOT_FOUND,
            "Not found in the ontology.",
            None,
        )
    })
}

// ─────────────────────────── types ───────────────────────────

async fn list_types(State(ctx): State<AppContext>, org: OrgContext) -> Result<Response> {
    reply(types::list(&ctx.db, org.org.id).await)
}

async fn get_type(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(key): Path<String>,
) -> Result<Response> {
    reply(types::get(&ctx.db, org.org.id, &key).await)
}

async fn create_type(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<ObjectTypeInput>,
) -> Result<Response> {
    require_admin(&org)?;
    created(
        types::create(
            &ctx.db,
            org.org.id,
            Some(org.auth.user.id),
            &input.def,
            None,
        )
        .await,
    )
}

async fn update_type(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(key): Path<String>,
    Json(input): Json<ObjectTypeInput>,
) -> Result<Response> {
    require_admin(&org)?;
    reply(types::update(&ctx.db, org.org.id, &key, Some(org.auth.user.id), &input).await)
}

async fn delete_type(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(key): Path<String>,
) -> Result<Response> {
    require_admin(&org)?;
    match types::delete(&ctx.db, org.org.id, &key).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => onto_error(e),
    }
}

async fn type_versions(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(key): Path<String>,
) -> Result<Response> {
    reply(types::versions(&ctx.db, org.org.id, &key).await)
}

// ─────────────────────────── link types ───────────────────────────

async fn list_link_types(State(ctx): State<AppContext>, org: OrgContext) -> Result<Response> {
    reply(link_types::list(&ctx.db, org.org.id).await)
}

async fn create_link_type(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(def): Json<LinkTypeDef>,
) -> Result<Response> {
    require_admin(&org)?;
    created(link_types::create(&ctx.db, org.org.id, &def, None).await)
}

async fn update_link_type(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(key): Path<String>,
    Json(input): Json<LinkTypeInput>,
) -> Result<Response> {
    require_admin(&org)?;
    reply(link_types::update(&ctx.db, org.org.id, &key, &input).await)
}

async fn delete_link_type(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(key): Path<String>,
) -> Result<Response> {
    require_admin(&org)?;
    match link_types::delete(&ctx.db, org.org.id, &key).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => onto_error(e),
    }
}

// ─────────────────────────── objects ───────────────────────────

async fn create_object(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<ObjectInput>,
) -> Result<Response> {
    require_write(&org)?;
    created(objects::create(&ctx.db, org.org.id, &actor(&org), &input).await)
}

async fn get_object(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
) -> Result<Response> {
    reply(objects::get(&ctx.db, org.org.id, parse_uuid(&id)?).await)
}

async fn update_object(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(input): Json<ObjectInput>,
) -> Result<Response> {
    require_write(&org)?;
    reply(objects::update(&ctx.db, org.org.id, &actor(&org), parse_uuid(&id)?, &input).await)
}

async fn archive_object(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
) -> Result<Response> {
    require_write(&org)?;
    match objects::archive(&ctx.db, org.org.id, &actor(&org), parse_uuid(&id)?).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => onto_error(e),
    }
}

async fn resolve(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((kind, native_id)): Path<(String, String)>,
) -> Result<Response> {
    let s = onto::schema(&ctx.db, org.org.id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    match objects::find_by_native(&ctx.db, org.org.id, &kind, &native_id).await {
        Ok(o) => format::json(objects::view(&o, &s)),
        Err(e) => onto_error(e),
    }
}

#[derive(Debug, Deserialize)]
struct LinksQuery {
    as_of: Option<String>,
    history: Option<bool>,
}

async fn object_links(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Query(q): Query<LinksQuery>,
) -> Result<Response> {
    let as_of = match onto::parse_instant("as_of", q.as_of.as_deref()) {
        Ok(t) => t,
        Err(e) => return onto_error(e),
    };
    reply(
        links::for_object(
            &ctx.db,
            org.org.id,
            parse_uuid(&id)?,
            as_of,
            q.history.unwrap_or(false),
        )
        .await,
    )
}

#[derive(Debug, Deserialize)]
struct GraphQuery {
    depth: Option<u32>,
    upstream: Option<bool>,
}

async fn object_graph(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Query(q): Query<GraphQuery>,
) -> Result<Response> {
    reply(graph::neighborhood(&ctx.db, org.org.id, parse_uuid(&id)?, q.depth.unwrap_or(1)).await)
}

async fn object_impact(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Query(q): Query<GraphQuery>,
) -> Result<Response> {
    let depth = q
        .depth
        .unwrap_or(4)
        .clamp(1, pnex_core::ontology::api::MAX_HOPS as u32);
    reply(
        graph::impact(
            &ctx.db,
            org.org.id,
            parse_uuid(&id)?,
            q.upstream.unwrap_or(false),
            depth,
        )
        .await,
    )
}

#[derive(Debug, Deserialize)]
struct PathQuery {
    from: String,
    to: String,
}

async fn path(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PathQuery>,
) -> Result<Response> {
    let depth = pnex_core::ontology::api::MAX_HOPS as u32;
    reply(
        graph::path(
            &ctx.db,
            org.org.id,
            parse_uuid(&q.from)?,
            parse_uuid(&q.to)?,
            depth,
        )
        .await,
    )
}

#[derive(Debug, Deserialize)]
struct SeriesQuery {
    window: Option<String>,
}

async fn object_series(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, property)): Path<(String, String)>,
    Query(q): Query<SeriesQuery>,
) -> Result<Response> {
    let id = parse_uuid(&id)?;
    let s = match onto::schema(&ctx.db, org.org.id).await {
        Ok(s) => s,
        Err(e) => return onto_error(e.into()),
    };
    let object = match objects::find(&ctx.db, org.org.id, id).await {
        Ok(o) => o,
        Err(e) => return onto_error(e),
    };
    let client = o2(&ctx);
    reply(
        series::read(
            &ctx.db,
            client.as_ref(),
            org.org.id,
            &s,
            &object,
            &property,
            q.window.as_deref().unwrap_or("24h"),
        )
        .await,
    )
}

/// `GET /api/v1/ontology/objects/{id}/bindings` — current sensor of each
/// bound series property (type dashboards, D187).
async fn object_bindings(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
) -> Result<Response> {
    let id = parse_uuid(&id)?;
    if let Err(e) = objects::find(&ctx.db, org.org.id, id).await {
        return onto_error(e);
    }
    reply(series::current_bindings(&ctx.db, org.org.id, id).await)
}

async fn object_changes(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
) -> Result<Response> {
    let id = parse_uuid(&id)?;
    if let Err(e) = objects::find(&ctx.db, org.org.id, id).await {
        return onto_error(e);
    }
    match changes::history(&ctx, org.org.id, &id.to_string(), 200).await {
        Ok(rows) => format::json(
            serde_json::json!({ "available": rows.is_some(), "changes": rows.unwrap_or_default() }),
        ),
        Err(e) => {
            tracing::warn!("object changes read failed: {e}");
            format::json(serde_json::json!({ "available": false, "changes": [] }))
        }
    }
}

// ─────────────────────────── links ───────────────────────────

async fn create_link(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<LinkInput>,
) -> Result<Response> {
    require_write(&org)?;
    created(links::create(&ctx.db, org.org.id, &actor(&org), &input).await)
}

async fn close_link(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    raw: axum::body::Bytes,
) -> Result<Response> {
    require_write(&org)?;
    // Optional body: read raw (an empty body may still carry a JSON type).
    let body: LinkClose = if raw.is_empty() {
        LinkClose::default()
    } else {
        match serde_json::from_slice(&raw) {
            Ok(b) => b,
            Err(_) => return onto_error(onto::invalid("valid_to", err_codes::FIELD_INVALID)),
        }
    };
    let valid_to = body.valid_to;
    reply(links::close(&ctx.db, org.org.id, &actor(&org), id, valid_to.as_deref()).await)
}

// ─────────────────────────── query ───────────────────────────

async fn run_query(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(q): Json<OntologyQuery>,
) -> Result<Response> {
    let client = o2(&ctx);
    reply(query::run(&ctx.db, client.as_ref(), org.org.id, &q).await)
}

// ─────────────────────────── packs ───────────────────────────

async fn list_packs(State(ctx): State<AppContext>, org: OrgContext) -> Result<Response> {
    reply(packs::list(&ctx.db, org.org.id).await)
}

async fn install_pack(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(key): Path<String>,
) -> Result<Response> {
    require_admin(&org)?;
    let Some(pack) = packs::shipped().into_iter().find(|p| p.key == key) else {
        return onto_error(OntologyError::NotFound);
    };
    reply(packs::install(&ctx.db, org.org.id, Some(org.auth.user.id), &pack).await)
}

async fn export(State(ctx): State<AppContext>, org: OrgContext) -> Result<Response> {
    match packs::export(&ctx.db, org.org.id).await {
        Ok(yaml) => Ok((
            [(axum::http::header::CONTENT_TYPE, "application/yaml")],
            yaml,
        )
            .into_response()),
        Err(e) => onto_error(e),
    }
}

/// `POST /api/v1/ontology/import` — the YAML of a pack (D186 as-code).
async fn import(
    State(ctx): State<AppContext>,
    org: OrgContext,
    raw: axum::body::Bytes,
) -> Result<Response> {
    require_admin(&org)?;
    let Ok(body) = std::str::from_utf8(&raw) else {
        return onto_error(onto::invalid("yaml", err_codes::FIELD_INVALID));
    };
    let pack = match packs::parse(body) {
        Ok(p) => p,
        Err(e) => return onto_error(e),
    };
    reply(packs::install(&ctx.db, org.org.id, Some(org.auth.user.id), &pack).await)
}
