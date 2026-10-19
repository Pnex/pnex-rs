//! Cross-cutting organisation layer (D42): **kind-agnostic** endpoints.
//!
//! The per-kind rules live in the registry
//! (`services/resources/registry.rs`); the controller never matches on
//! kinds.
//!
//! - errors are `ErrorDetail` (`field-invalid`, `resource-kind-unknown`,
//!   `resource-cycle`, `resource-edge-duplicate`);
//! - cross-org masked as 404 (unknown kind: 400, resource of another org:
//!   404);
//! - writes gated by `resources:write` (owner|admin|member);
//! - static routes before parametric ones: `labels/query`, `edges`,
//!   `folders` before `{kind}/{id}/…`.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::auth::OrgContext;
use crate::services::resources as svc;
use pnex_core::{err_codes, resources::parse_label_filter};

// ─────────────────────────── errors ───────────────────────────

fn field_status(field: &str, msg: &str) -> Response {
    super::field_error(field, msg)
}

/// 400 `resource-kind-unknown` (`args.kind`).
fn kind_unknown(kind: &str) -> Error {
    super::coded_error(
        StatusCode::BAD_REQUEST,
        err_codes::RESOURCE_KIND_UNKNOWN,
        format!("Unknown resource kind: {kind}"),
        Some(serde_json::json!({ "kind": kind })),
    )
}

/// Engine errors to HTTP answers.
fn error_response(e: svc::ResourceError, kind: &str) -> Response {
    use svc::ResourceError as E;
    match e {
        E::KindInvalid => kind_unknown(kind).into_response(),
        E::ResourceUnknown => Error::NotFound.into_response(),
        E::LabelInvalid => field_status("labels", err_codes::FIELD_INVALID),
        E::NameInvalid(token) => field_status("name", &token),
        E::CycleDetected => super::coded_error(
            StatusCode::BAD_REQUEST,
            err_codes::RESOURCE_CYCLE,
            "A resource cannot be its own ancestor",
            None,
        )
        .into_response(),
        E::RelationInvalid => field_status("relation", err_codes::FIELD_INVALID),
        E::TargetInvalid => field_status("target", err_codes::FIELD_INVALID),
        E::Conflict => super::coded_error(
            StatusCode::CONFLICT,
            err_codes::RESOURCE_EDGE_DUPLICATE,
            "An edge already exists for this pair.",
            None,
        )
        .into_response(),
        E::PlacementInvalid => field_status("placement", err_codes::FIELD_INVALID),
        E::Db => Error::InternalServerError.into_response(),
    }
}

/// Common preamble of the `{kind}/{id}/…` routes: unknown kind 400,
/// missing / cross-org resource 404 (resolved through the registry).
async fn require_resource(ctx: &AppContext, org: &OrgContext, kind: &str, id: &str) -> Result<()> {
    let reg = svc::registry::for_org(&ctx.db, org.org.id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let Some(entry) = reg.entry(kind) else {
        return Err(kind_unknown(kind));
    };
    let ok = entry
        .resolver
        .resolve(&ctx.db, org.org.id, id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if !ok {
        return Err(Error::NotFound);
    }
    Ok(())
}

// ─────────────────────────── labels ───────────────────────────

/// A label target (D218): an ontology resource, or a row of a label-only
/// kind (function, control, stream…), checked in the org (R1).
async fn require_label_target(
    ctx: &AppContext,
    org: &OrgContext,
    kind: &str,
    id: &str,
) -> Result<()> {
    match svc::labels::label_only_exists(&ctx.db, org.org.id, kind, id)
        .await
        .map_err(|_| Error::InternalServerError)?
    {
        Some(true) => Ok(()),
        Some(false) => Err(Error::NotFound),
        None => require_resource(ctx, org, kind, id).await,
    }
}

/// Whether `kind` can carry labels in this org.
fn labelable(reg: &svc::registry::Registry, kind: &str) -> bool {
    reg.entry(kind).is_some() || pnex_core::resources::LABEL_ONLY_KINDS.contains(&kind)
}

#[derive(Serialize, utoipa::ToSchema)]
struct LabelsDto {
    kind: String,
    id: String,
    /// Own labels: name -> value (`null` = bare tag).
    #[schema(value_type = BTreeMap<String, Option<String>>)]
    labels: pnex_core::resources::LabelSet,
}

fn labels_dto(kind: &str, id: &str, labels: &pnex_core::resources::LabelSet) -> LabelsDto {
    LabelsDto {
        kind: kind.to_string(),
        id: id.to_string(),
        labels: labels.clone(),
    }
}

/// `GET /api/v1/resources/{kind}/{id}/labels`.
#[utoipa::path(
    get,
    path = "/api/v1/resources/{kind}/{id}/labels",
    tag = "resources",
    operation_id = "resources_labels_get",
    summary = "Get the labels of a resource",
    description = "Own labels of one resource (inherited labels are not included).",
    params(("kind" = String, Path, description = "Resource kind (e.g. `device`, `map_pin`, `media_asset`, `folder`)"), ("id" = String, Path, description = "Resource id within its kind")),
    responses(
        (status = 200, body = LabelsDto),
        (status = 400, description = "Unknown kind", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown resource or resource of another org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn get_labels(
    ctx: State<AppContext>,
    org: OrgContext,
    Path((kind, id)): Path<(String, String)>,
) -> Result<Response> {
    require_label_target(&ctx, &org, &kind, &id).await?;
    let labels = svc::labels::get_labels(&ctx.db, org.org.id, &kind, &id)
        .await
        .map_err(|_| Error::InternalServerError)?
        .unwrap_or_default();
    format::json(labels_dto(&kind, &id, &labels))
}

/// `PUT /api/v1/resources/{kind}/{id}/labels`: replaces the whole set.
#[utoipa::path(
    put,
    path = "/api/v1/resources/{kind}/{id}/labels",
    tag = "resources",
    operation_id = "resources_labels_set",
    summary = "Replace the labels of a resource",
    description = "Replaces the whole label set of the resource with `{\"labels\": {name: value|null}}` (`null` = bare tag). Requires owner, admin or member role.",
    params(("kind" = String, Path, description = "Resource kind (e.g. `device`, `map_pin`, `media_asset`, `folder`)"), ("id" = String, Path, description = "Resource id within its kind")),
    request_body = serde_json::Value,
    responses(
        (status = 200, body = LabelsDto),
        (status = 400, description = "`field-invalid` on `labels` (`required`, `invalid`: not an object, bad name or value, too many) or `resource-kind-unknown`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown resource or resource of another org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn put_labels(
    ctx: State<AppContext>,
    org: OrgContext,
    Path((kind, id)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Result<Response> {
    require_label_target(&ctx, &org, &kind, &id).await?;
    let Some(doc) = body.get("labels") else {
        return Ok(field_status("labels", err_codes::FIELD_REQUIRED));
    };
    let Some(doc) = doc.as_object() else {
        return Ok(field_status("labels", err_codes::FIELD_INVALID));
    };
    let mut labels = pnex_core::resources::LabelSet::new();
    for (name, value) in doc {
        let value = match value {
            Value::Null => None,
            Value::String(s) => Some(s.clone()),
            _ => {
                return Ok(field_status("labels", err_codes::FIELD_INVALID));
            }
        };
        labels.insert(name.clone(), value);
    }
    match svc::labels::put_labels(
        &ctx.db,
        org.org.id,
        &kind,
        &id,
        &labels,
        Some(org.auth.user.id),
    )
    .await
    {
        Ok(put) => format::json(labels_dto(&kind, &id, &put)),
        Err(e) => Ok(error_response(e, &kind)),
    }
}

// ─────────────────────────── labels query ───────────────────────────

/// Most ids per query (a list page is 20–100 rows).
const LABELS_QUERY_MAX: usize = 200;

#[derive(Deserialize, utoipa::ToSchema)]
struct LabelsQueryBody {
    kind: String,
    ids: Vec<String>,
}

/// `POST /resources/labels/query`: own labels of a page of resources of
/// one kind, in one query (list rows). A read with a large body (D242).
/// Org-scoped: ids of another org simply come back without labels (R1).
/// Read-only, any member.
#[utoipa::path(
    post,
    path = "/api/v1/resources/labels/query",
    tag = "resources",
    operation_id = "resources_labels_query",
    summary = "Get the labels of many resources",
    description = "Own labels of up to 200 resources of one kind in one call (list rows). Ids of another org come back without labels. Read-only, any org member.",
    request_body = LabelsQueryBody,
    responses(
        (status = 200, body = LabelsBatch),
        (status = 400, description = "`field-invalid`: `kind` (`invalid`) or `ids` above 200 (`max_length:200`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn labels_query(
    ctx: State<AppContext>,
    org: OrgContext,
    Json(body): Json<LabelsQueryBody>,
) -> Result<Response> {
    let reg = svc::registry::for_org(&ctx.db, org.org.id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if !labelable(&reg, &body.kind) {
        return Ok(field_status("kind", err_codes::FIELD_INVALID));
    }
    if body.ids.len() > LABELS_QUERY_MAX {
        return Ok(field_status(
            "ids",
            &format!("{}:{LABELS_QUERY_MAX}", err_codes::FIELD_MAX_LENGTH),
        ));
    }
    let labels = svc::labels::labels_for_many(&ctx.db, org.org.id, &body.kind, &body.ids)
        .await
        .map_err(|_| Error::InternalServerError)?;
    format::json(LabelsBatch { labels })
}

/// Own labels of each requested id (ids without labels are absent).
#[derive(Serialize, utoipa::ToSchema)]
struct LabelsBatch {
    /// id -> (label name -> value, `null` = bare tag).
    #[schema(value_type = HashMap<String, BTreeMap<String, Option<String>>>)]
    labels: std::collections::HashMap<String, pnex_core::resources::LabelSet>,
}

/// One label name of the org with its known values (picker suggestions).
#[derive(Serialize, utoipa::ToSchema)]
struct LabelCatalogEntry {
    name: String,
    values: Vec<String>,
    /// Resources carrying this name.
    count: i64,
}

/// `GET /resources/labels/catalog`: label names and values already used in
/// the org, for the label picker (read-only, any member, org from the
/// principal, R1).
#[utoipa::path(
    get,
    path = "/api/v1/resources/labels/catalog",
    tag = "resources",
    operation_id = "resources_labels_catalog",
    summary = "List the labels used in the organisation",
    description = "Paginated (`limit`/`offset`): every label name of the org with its known values and the number of resources carrying it (label picker suggestions). Read-only.",
    params(PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<LabelCatalogEntry>)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn labels_catalog(
    ctx: State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let entries = svc::labels::catalog(&ctx.db, org.org.id)
        .await
        .map_err(|_| Error::InternalServerError)?
        .into_iter()
        .map(|(name, values, count)| LabelCatalogEntry {
            name,
            values,
            count,
        })
        .collect::<Vec<_>>();
    page_of("/api/v1/resources/labels/catalog", &[], &q, entries)
}

/// `limit`/`offset` of a plain paginated list.
#[derive(Deserialize, Default, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct PageQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// One page of an in-memory list (small org-wide lists).
fn page_of<T: Serialize>(
    path: &str,
    filters: &[(String, String)],
    q: &PageQuery,
    all: Vec<T>,
) -> Result<Response> {
    let page = super::pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let count = all.len() as i64;
    let (skip, take) = page.slice(all.len());
    let results: Vec<T> = all.into_iter().skip(skip).take(take).collect();
    format::json(super::pagination::envelope(
        path, filters, page, count, results,
    ))
}

// ─────────────────────────── location ───────────────────────────

/// `GET /resources/{kind}/{id}/location` — site breadcrumb(s) of a
/// resource (read-only, any member; resource checked in the org, R1).
#[utoipa::path(
    get,
    path = "/api/v1/resources/{kind}/{id}/location",
    tag = "resources",
    operation_id = "resources_location_get",
    summary = "Get the site location of a resource",
    description = "Site breadcrumb(s) of the resource: its site then the folders down to it, one entry per site for objects attached to several sites. Read-only.",
    params(("kind" = String, Path, description = "Resource kind (e.g. `device`, `map_pin`, `media_asset`, `folder`)"), ("id" = String, Path, description = "Resource id within its kind")),
    responses(
        (status = 200, body = pnex_core::resources::ResourceLocations),
        (status = 400, description = "Unknown kind", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown resource or resource of another org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn get_location(
    ctx: State<AppContext>,
    org: OrgContext,
    Path((kind, id)): Path<(String, String)>,
) -> Result<Response> {
    require_resource(&ctx, &org, &kind, &id).await?;
    let locations = svc::location::locations_of(&ctx.db, org.org.id, &kind, &id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    format::json(pnex_core::resources::ResourceLocations { locations })
}

// ─────────────────────────── containment ───────────────────────────

#[derive(Serialize, utoipa::ToSchema)]
struct ContainmentDto {
    kind: String,
    id: String,
    /// Parent (`null` = root).
    parent: Option<RefDto>,
    /// Breadcrumb, leaf to root (parent excluded).
    path: Vec<RefDto>,
    /// Direct children (`sort_key` order).
    children: Vec<RefDto>,
}

#[derive(Serialize, utoipa::ToSchema)]
struct RefDto {
    kind: String,
    id: String,
    /// Display name, when it could be resolved.
    name: Option<String>,
    emoji: Option<String>,
    sort_key: Option<String>,
}

fn ref_dto(
    kind: &str,
    id: &str,
    name: Option<String>,
    emoji: Option<String>,
    sort_key: Option<String>,
) -> RefDto {
    RefDto {
        kind: kind.to_string(),
        id: id.to_string(),
        name,
        emoji,
        sort_key,
    }
}

#[derive(Deserialize, utoipa::ToSchema)]
struct ContainmentPut {
    /// `{"kind": "folder", "id": "3"}`, or `null` to detach.
    parent: Option<ParentRef>,
    /// Order within the parent (optional).
    sort_key: Option<String>,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct ParentRef {
    kind: String,
    id: String,
}

/// `GET /api/v1/resources/{kind}/{id}/containment`: parent + path +
/// children.
#[utoipa::path(
    get,
    path = "/api/v1/resources/{kind}/{id}/containment",
    tag = "resources",
    operation_id = "resources_containment_get",
    summary = "Get the containment of a resource",
    description = "Parent, path to the root and direct children of the resource in the organisation tree, with display names.",
    params(("kind" = String, Path, description = "Resource kind (e.g. `device`, `map_pin`, `media_asset`, `folder`)"), ("id" = String, Path, description = "Resource id within its kind")),
    responses(
        (status = 200, body = ContainmentDto),
        (status = 400, description = "Unknown kind", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown resource or resource of another org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn get_containment(
    ctx: State<AppContext>,
    org: OrgContext,
    Path((kind, id)): Path<(String, String)>,
) -> Result<Response> {
    require_resource(&ctx, &org, &kind, &id).await?;
    let parent = svc::containment::parent_of(&ctx.db, org.org.id, &kind, &id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let path = svc::containment::path_of(&ctx.db, org.org.id, &kind, &id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    // Children + display names: folders in one batched query
    // (`folder_names`), other kinds through the registry resolver (one call
    // per ref, tree lists are short).
    let children = svc::containment::children_of(&ctx.db, org.org.id, &kind, &id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let folder_ids: Vec<i64> = children
        .iter()
        .filter(|(k, _, _)| k == pnex_core::resources::KIND_FOLDER)
        .filter_map(|(_, i, _)| i.parse::<i64>().ok())
        .collect();
    let names = svc::containment::folder_names(&ctx.db, org.org.id, &folder_ids)
        .await
        .map_err(|_| Error::InternalServerError)?;
    // Non-folder refs to name: parent + path + children (deduplicated).
    let mut display = std::collections::HashMap::<String, String>::new();
    let mut refs: Vec<(&str, &str)> = Vec::new();
    if let Some((pk, pi)) = &parent {
        if pk != pnex_core::resources::KIND_FOLDER {
            refs.push((pk.as_str(), pi.as_str()));
        }
    }
    for (k, i) in &path {
        if k != pnex_core::resources::KIND_FOLDER {
            refs.push((k.as_str(), i.as_str()));
        }
    }
    for (k, i, _) in &children {
        if k != pnex_core::resources::KIND_FOLDER {
            refs.push((k.as_str(), i.as_str()));
        }
    }
    let reg = svc::registry::for_org(&ctx.db, org.org.id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    for (k, i) in refs {
        let key = format!("{k}:{i}");
        if display.contains_key(&key) {
            continue;
        }
        if let Some(entry) = reg.entry(k) {
            if let Ok(Some(name)) = entry.resolver.display_name(&ctx.db, org.org.id, i).await {
                display.insert(key, name);
            }
        }
    }
    let parent_name: Option<(String, Option<String>)> = parent.as_ref().and_then(|(pk, pi)| {
        if pk == pnex_core::resources::KIND_FOLDER {
            pi.parse::<i64>()
                .ok()
                .and_then(|i| names.get(&i))
                .map(|(n, e)| (n.clone(), e.clone()))
        } else {
            display
                .get(&format!("{pk}:{pi}"))
                .cloned()
                .map(|n| (n, None))
        }
    });
    format::json(ContainmentDto {
        kind: kind.clone(),
        id: id.clone(),
        parent: parent.map(|(pk, pi)| {
            let name = parent_name.clone().map(|(n, _)| n);
            let emoji = parent_name.and_then(|(_, e)| e);
            ref_dto(&pk, &pi, name, emoji, None)
        }),
        path: path
            .iter()
            .map(|(pk, pi)| {
                let (name, emoji) = ref_name(&display, &names, pk, pi);
                ref_dto(pk, pi, name, emoji, None)
            })
            .collect(),
        children: children
            .iter()
            .map(|(k, i, sk)| {
                let (name, emoji) = ref_name(&display, &names, k, i);
                ref_dto(k, i, name, emoji, sk.clone())
            })
            .collect(),
    })
}

/// (name, emoji) of a ref: folders through `folder_names`, other kinds
/// through the names resolved by the registry.
fn ref_name(
    display: &std::collections::HashMap<String, String>,
    names: &std::collections::HashMap<i64, (String, Option<String>)>,
    kind: &str,
    id: &str,
) -> (Option<String>, Option<String>) {
    if kind == pnex_core::resources::KIND_FOLDER {
        return id
            .parse::<i64>()
            .ok()
            .and_then(|i| names.get(&i))
            .map(|(n, e)| (Some(n.clone()), e.clone()))
            .unwrap_or((None, None));
    }
    (display.get(&format!("{kind}:{id}")).cloned(), None)
}

/// `PUT /api/v1/resources/{kind}/{id}/containment`: (re)parents; a cycle is
/// 400; `parent: null` detaches (the subtree follows, nothing is deleted).
#[utoipa::path(
    put,
    path = "/api/v1/resources/{kind}/{id}/containment",
    tag = "resources",
    operation_id = "resources_containment_set",
    summary = "Move a resource in the organisation tree",
    description = "Sets the parent (and optional `sort_key`) of the resource; `parent: null` detaches it. Its subtree follows, nothing is deleted. A cycle is refused. Requires owner, admin or member role.",
    params(("kind" = String, Path, description = "Resource kind (e.g. `device`, `map_pin`, `media_asset`, `folder`)"), ("id" = String, Path, description = "Resource id within its kind")),
    request_body = ContainmentPut,
    responses(
        (status = 200, body = ContainmentDto),
        (status = 400, description = "`resource-kind-unknown`, or `resource-cycle` (the move would make the resource its own ancestor)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown resource or resource of another org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn put_containment(
    ctx: State<AppContext>,
    org: OrgContext,
    Path((kind, id)): Path<(String, String)>,
    Json(p): Json<ContainmentPut>,
) -> Result<Response> {
    require_resource(&ctx, &org, &kind, &id).await?;
    let parent = p.parent.map(|r| (r.kind, r.id));
    let parent = parent.as_ref().map(|(k, i)| (k.as_str(), i.as_str()));
    match svc::containment::set_parent(
        &ctx.db,
        org.org.id,
        &kind,
        &id,
        parent,
        p.sort_key.as_deref(),
    )
    .await
    {
        Ok(()) => get_containment(ctx, org, Path((kind, id))).await,
        Err(e) => Ok(error_response(e, &kind)),
    }
}

/// `DELETE /api/v1/resources/{kind}/{id}/containment`: detaches.
#[utoipa::path(
    delete,
    path = "/api/v1/resources/{kind}/{id}/containment",
    tag = "resources",
    operation_id = "resources_containment_delete",
    summary = "Detach a resource from its parent",
    description = "Moves the resource to the root of the organisation tree; nothing is deleted. Requires owner, admin or member role.",
    params(("kind" = String, Path, description = "Resource kind (e.g. `device`, `map_pin`, `media_asset`, `folder`)"), ("id" = String, Path, description = "Resource id within its kind")),
    responses(
        (status = 204, description = "Detached"),
        (status = 400, description = "Unknown kind", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown resource, resource of another org, or resource without parent", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn delete_containment(
    ctx: State<AppContext>,
    org: OrgContext,
    Path((kind, id)): Path<(String, String)>,
) -> Result<Response> {
    require_resource(&ctx, &org, &kind, &id).await?;
    let detached = svc::containment::detach(&ctx.db, org.org.id, &kind, &id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if !detached {
        return Err(Error::NotFound);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ─────────────────────────── edges ───────────────────────────

#[derive(Serialize, utoipa::ToSchema)]
struct EdgeDto {
    id: i64,
    relation: String,
    source_kind: String,
    source_id: String,
    target_kind: String,
    target_id: String,
    placement: Option<Value>,
    #[schema(format = DateTime)]
    created_at: String,
}

fn edge_dto(e: &crate::models::_entities::resource_edges::Model) -> EdgeDto {
    EdgeDto {
        id: e.id,
        relation: e.relation.clone(),
        source_kind: e.source_kind.clone(),
        source_id: e.source_id.clone(),
        target_kind: e.target_kind.clone(),
        target_id: e.target_id.clone(),
        placement: e.placement.clone(),
        created_at: e.created_at.to_rfc3339(),
    }
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct EdgeListQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
    relation: Option<String>,
    source_kind: Option<String>,
    source_id: Option<String>,
    target_kind: Option<String>,
    target_id: Option<String>,
}

/// `GET /api/v1/resources/edges`: filtered edges.
#[utoipa::path(
    get,
    path = "/api/v1/resources/edges",
    tag = "resources",
    operation_id = "resources_edges_list",
    summary = "List resource edges",
    description = "Typed edges between resources of the org, paginated (`limit`/`offset`), filtered by `relation` and/or source (`source_kind` + `source_id`) and/or target (`target_kind` + `target_id`).",
    params(EdgeListQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<EdgeDto>)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list_edges(
    ctx: State<AppContext>,
    org: OrgContext,
    Query(q): Query<EdgeListQuery>,
) -> Result<Response> {
    let rows = svc::edges::list_edges(
        &ctx.db,
        org.org.id,
        q.relation.as_deref(),
        match (&q.source_kind, &q.source_id) {
            (Some(k), Some(i)) => Some((k.as_str(), i.as_str())),
            _ => None,
        },
        match (&q.target_kind, &q.target_id) {
            (Some(k), Some(i)) => Some((k.as_str(), i.as_str())),
            _ => None,
        },
    )
    .await
    .map_err(|_| Error::InternalServerError)?;
    let filters: Vec<(String, String)> = [
        ("relation", &q.relation),
        ("source_kind", &q.source_kind),
        ("source_id", &q.source_id),
        ("target_kind", &q.target_kind),
        ("target_id", &q.target_id),
    ]
    .into_iter()
    .filter_map(|(k, v)| v.clone().map(|v| (k.to_string(), v)))
    .collect();
    let page = PageQuery {
        limit: q.limit.clone(),
        offset: q.offset.clone(),
    };
    page_of(
        "/api/v1/resources/edges",
        &filters,
        &page,
        rows.iter().map(edge_dto).collect(),
    )
}

#[derive(Deserialize, utoipa::ToSchema)]
struct EdgeCreate {
    relation: String,
    source_kind: String,
    source_id: String,
    target_kind: String,
    target_id: String,
    placement: Option<Value>,
}

/// `POST /api/v1/resources/edges`: validated per kind (spec), duplicate 409.
#[utoipa::path(
    post,
    path = "/api/v1/resources/edges",
    tag = "resources",
    operation_id = "resources_edges_create",
    summary = "Create a resource edge",
    description = "Creates a typed edge between two resources of the org; the relation and the target kind must be allowed by the source kind. Requires owner, admin or member role.",
    request_body = EdgeCreate,
    responses(
        (status = 201, body = EdgeDto),
        (status = 400, description = "`resource-kind-unknown`, or `field-invalid` on `relation`, `target` or `placement` (`invalid`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown source resource", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`resource-edge-duplicate`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create_edge(
    ctx: State<AppContext>,
    org: OrgContext,
    Json(p): Json<EdgeCreate>,
) -> Result<Response> {
    match svc::edges::create_edge(
        &ctx.db,
        org.org.id,
        &p.relation,
        &p.source_kind,
        &p.source_id,
        &p.target_kind,
        &p.target_id,
        p.placement.as_ref(),
    )
    .await
    {
        Ok(edge) => {
            let dto = edge_dto(&edge);
            Ok((StatusCode::CREATED, format::json(dto)).into_response())
        }
        Err(e) => Ok(error_response(e, &p.source_kind)),
    }
}

#[derive(Deserialize, utoipa::ToSchema)]
struct EdgePlacementPatch {
    placement: Value,
}

/// `PATCH /api/v1/resources/edges/{id}`: placement (hotspot move,
/// overrides).
#[utoipa::path(
    patch,
    path = "/api/v1/resources/edges/{id}",
    tag = "resources",
    operation_id = "resources_edges_update",
    summary = "Update the placement of an edge",
    description = "Replaces the edge `placement` metadata (hotspot position, overrides). Requires owner, admin or member role.",
    params(("id" = i64, Path, description = "Edge id")),
    request_body = EdgePlacementPatch,
    responses(
        (status = 200, body = EdgeDto),
        (status = 400, description = "`field-invalid`: `placement` (`invalid`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown edge or edge of another org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn patch_edge(
    ctx: State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Json(p): Json<EdgePlacementPatch>,
) -> Result<Response> {
    match svc::edges::update_placement(&ctx.db, org.org.id, id, &p.placement).await {
        Ok(Some(edge)) => format::json(edge_dto(&edge)),
        Ok(None) => Err(Error::NotFound),
        Err(e) => Ok(error_response(e, "")),
    }
}

/// `DELETE /api/v1/resources/edges/{id}`: 204; cross-org masked as 404.
#[utoipa::path(
    delete,
    path = "/api/v1/resources/edges/{id}",
    tag = "resources",
    operation_id = "resources_edges_delete",
    summary = "Delete a resource edge",
    description = "Deletes the edge; both resources are kept. Requires owner, admin or member role.",
    params(("id" = i64, Path, description = "Edge id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Unknown edge or edge of another org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn delete_edge(
    ctx: State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    let deleted = svc::edges::delete_edge(&ctx.db, org.org.id, id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if !deleted {
        return Err(Error::NotFound);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ─────────────────────────── folders ───────────────────────────

#[derive(Serialize, utoipa::ToSchema)]
struct FolderDto {
    id: i64,
    name: String,
    emoji: Option<String>,
    #[schema(format = DateTime)]
    created_at: String,
    #[schema(format = DateTime)]
    updated_at: String,
}

fn folder_dto(f: &crate::models::_entities::resource_folders::Model) -> FolderDto {
    FolderDto {
        id: f.id,
        name: f.name.clone(),
        emoji: f.emoji.clone(),
        created_at: f.created_at.to_rfc3339(),
        updated_at: f.updated_at.to_rfc3339(),
    }
}

#[derive(Deserialize, utoipa::ToSchema)]
struct FolderCreate {
    name: String,
    emoji: Option<String>,
}

/// `POST /api/v1/resources/folders`.
#[utoipa::path(
    post,
    path = "/api/v1/resources/folders",
    tag = "resources",
    operation_id = "resources_folders_create",
    summary = "Create a folder",
    description = "Creates a folder (organisation layer) at the root; place it with `PUT /resources/folder/{id}/containment`. Requires owner, admin or member role.",
    request_body = FolderCreate,
    responses(
        (status = 201, body = FolderDto),
        (status = 400, description = "`field-invalid`: `name` (`required`, `max_length:255`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create_folder(
    ctx: State<AppContext>,
    org: OrgContext,
    Json(p): Json<FolderCreate>,
) -> Result<Response> {
    match svc::folders::create_folder(
        &ctx.db,
        org.org.id,
        &p.name,
        p.emoji.as_deref(),
        Some(org.auth.user.id),
    )
    .await
    {
        Ok(f) => {
            let dto = folder_dto(&f);
            Ok((StatusCode::CREATED, format::json(dto)).into_response())
        }
        Err(e) => Ok(error_response(e, pnex_core::resources::KIND_FOLDER)),
    }
}

/// `GET /api/v1/resources/folders`: the folders of the org.
#[utoipa::path(
    get,
    path = "/api/v1/resources/folders",
    tag = "resources",
    operation_id = "resources_folders_list",
    summary = "List the folders",
    description = "Folders of the org, paginated (`limit`/`offset`).",
    params(PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<FolderDto>)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list_folders(
    ctx: State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let rows = svc::folders::list_folders(&ctx.db, org.org.id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    page_of(
        "/api/v1/resources/folders",
        &[],
        &q,
        rows.iter().map(folder_dto).collect(),
    )
}

#[derive(Deserialize, utoipa::ToSchema)]
struct FolderPatch {
    name: Option<String>,
    /// Absent = unchanged, `null` = cleared.
    #[serde(default, deserialize_with = "deserialize_some")]
    #[schema(value_type = Option<String>)]
    emoji: Option<Option<String>>,
}

/// `Option<Option<T>>` idiom: absent vs `null`.
fn deserialize_some<'de, T, D>(d: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    serde::Deserialize::deserialize(d).map(Some)
}

/// `PATCH /api/v1/resources/folders/{id}`.
#[utoipa::path(
    patch,
    path = "/api/v1/resources/folders/{id}",
    tag = "resources",
    operation_id = "resources_folders_update",
    summary = "Rename a folder",
    description = "Updates the folder name and/or emoji (`null` clears the emoji). Requires owner, admin or member role.",
    params(("id" = i64, Path, description = "Folder id")),
    request_body = FolderPatch,
    responses(
        (status = 200, body = FolderDto),
        (status = 400, description = "`field-invalid`: `name` (`required`, `max_length:255`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown folder or folder of another org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn patch_folder(
    ctx: State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Json(p): Json<FolderPatch>,
) -> Result<Response> {
    match svc::folders::update_folder(
        &ctx.db,
        org.org.id,
        id,
        svc::folders::FolderPatch {
            name: p.name.as_deref(),
            emoji: p.emoji.as_ref().map(|o| o.as_deref()),
        },
    )
    .await
    {
        Ok(Some(f)) => format::json(folder_dto(&f)),
        Ok(None) => Err(Error::NotFound),
        Err(e) => Ok(error_response(e, pnex_core::resources::KIND_FOLDER)),
    }
}

/// `DELETE /api/v1/resources/folders/{id}`: organisation purge (subtree
/// re-rooted), entity row deleted.
#[utoipa::path(
    delete,
    path = "/api/v1/resources/folders/{id}",
    tag = "resources",
    operation_id = "resources_folders_delete",
    summary = "Delete a folder",
    description = "Deletes the folder; its content is moved up (re-rooted), nothing else is deleted. Requires owner, admin or member role.",
    params(("id" = i64, Path, description = "Folder id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Unknown folder or folder of another org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn delete_folder(
    ctx: State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    let deleted = svc::folders::delete_folder(&ctx.db, org.org.id, id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if !deleted {
        return Err(Error::NotFound);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ─────────────────────────── cross-kind search ───────────────────────────

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct SearchQuery {
    /// Label filter `name` or `name:value` (own labels, D218).
    label: String,
    /// Comma-separated kinds to search (absent = every labelable kind).
    kinds: Option<String>,
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
struct SearchResultItem {
    kind: String,
    id: String,
}

/// `GET /api/v1/resources/search?label=&kinds=`: resources of the org
/// carrying the label (own labels).
#[utoipa::path(
    get,
    path = "/api/v1/resources/search",
    tag = "resources",
    operation_id = "resources_search",
    summary = "Find resources by label",
    description = "Resources of the org carrying the label filter (`name` or `name:value`), optionally restricted to some kinds (`kinds`, comma-separated), paginated (`limit`/`offset`). Read-only.",
    params(SearchQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<SearchResultItem>),
        (status = 400, description = "`field-invalid`: `label` (`required`, `invalid`) or `kinds` (`invalid`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn search(
    ctx: State<AppContext>,
    org: OrgContext,
    Query(q): Query<SearchQuery>,
) -> Result<Response> {
    let (filter, err) = parse_label_filter(&q.label);
    let Some(filter) = filter else {
        return Ok(field_status(
            "label",
            err.as_deref().unwrap_or(err_codes::FIELD_INVALID),
        ));
    };
    let reg = svc::registry::for_org(&ctx.db, org.org.id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let kinds: Vec<String> = q
        .kinds
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_string)
        .collect();
    if kinds.iter().any(|k| !labelable(&reg, k)) {
        return Ok(field_status("kinds", err_codes::FIELD_INVALID));
    }
    let results: Vec<SearchResultItem> =
        svc::labels::ids_with_label(&ctx.db, org.org.id, &filter, None)
            .await
            .map_err(|_| Error::InternalServerError)?
            .into_iter()
            .filter(|(k, _)| kinds.is_empty() || kinds.contains(k))
            .map(|(kind, id)| SearchResultItem { kind, id })
            .collect();
    let mut filters = vec![("label".to_string(), q.label.clone())];
    if let Some(k) = q.kinds.clone().filter(|k| !k.is_empty()) {
        filters.push(("kinds".to_string(), k));
    }
    let page = PageQuery {
        limit: q.limit.clone(),
        offset: q.offset.clone(),
    };
    page_of("/api/v1/resources/search", &filters, &page, results)
}

// ─────────────────────────── routes ───────────────────────────

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/resources")
        // Static routes before parametric ones (matchit).
        .add("/labels/query", post(labels_query))
        .add("/labels/catalog", get(labels_catalog))
        .add("/edges", get(list_edges).post(create_edge))
        .add(
            "/edges/{id}",
            axum::routing::patch(patch_edge).delete(delete_edge),
        )
        .add("/folders", get(list_folders).post(create_folder))
        .add(
            "/folders/{id}",
            axum::routing::patch(patch_folder).delete(delete_folder),
        )
        .add("/search", get(search))
        .add("/{kind}/{id}/labels", get(get_labels).put(put_labels))
        .add("/{kind}/{id}/location", get(get_location))
        .add(
            "/{kind}/{id}/containment",
            get(get_containment)
                .put(put_containment)
                .delete(delete_containment),
        )
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(
        labels_query,
        labels_catalog,
        list_edges,
        create_edge,
        patch_edge,
        delete_edge,
        list_folders,
        create_folder,
        patch_folder,
        delete_folder,
        search,
        get_labels,
        put_labels,
        get_location,
        get_containment,
        put_containment,
        delete_containment
    ))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
