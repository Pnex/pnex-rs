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
    LinkClose, LinkInput, LinkTypeInput, LinkUpdate, ObjectInput, ObjectTypeInput, OntologyQuery,
};
use pnex_core::ontology::LinkTypeDef;
use serde::Deserialize;
use uuid::Uuid;

use super::coded_error;
use super::pagination::{envelope, PageParams};
use crate::auth::OrgContext;
use crate::services::ontology::redact::Redaction;
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
        .add("/objects/{id}/erase", post(erase_object))
        .add("/objects/{id}/links", get(object_links))
        .add("/objects/{id}/pages", post(object_page))
        .add("/objects/{id}/graph", get(object_graph))
        .add("/objects/{id}/hierarchy", get(object_hierarchy))
        .add("/objects/{id}/impact", get(object_impact))
        .add("/objects/{id}/series/{property}", get(object_series))
        .add("/objects/{id}/bindings", get(object_bindings))
        .add("/objects/{id}/changes", get(object_changes))
        .add("/objects/{id}/evidence", get(object_evidence))
        .add("/objects/{id}/actions/{key}", post(invoke_action))
        .add("/resolve/{kind}/{native_key}", get(resolve))
        .add("/path", get(path))
        .add("/links", post(create_link))
        .add("/links/{id}", axum::routing::put(update_link))
        .add("/links/{id}/close", post(close_link))
        .add("/query", post(run_query))
        .add("/packs", get(list_packs))
        .add("/packs/{key}/install", post(install_pack))
        .add("/packs/{key}/take", post(take_pack_version))
        .add(
            "/packs/{key}/settings",
            axum::routing::patch(update_pack_settings),
        )
        .add("/export", get(export))
        .add(
            "/import",
            post(import).layer(axum::extract::DefaultBodyLimit::max(
                packs::YAML_MAX_BYTES * 2,
            )),
        )
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(
        list_types,
        get_type,
        create_type,
        update_type,
        delete_type,
        type_versions,
        list_link_types,
        create_link_type,
        update_link_type,
        delete_link_type,
        create_object,
        object_page,
        invoke_action,
        get_object,
        update_object,
        archive_object,
        erase_object,
        resolve,
        object_links,
        object_graph,
        object_hierarchy,
        object_impact,
        path,
        object_series,
        object_bindings,
        object_changes,
        object_evidence,
        create_link,
        update_link,
        close_link,
        run_query,
        list_packs,
        install_pack,
        take_pack_version,
        update_pack_settings,
        export,
        import
    ))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

/// Service error → HTTP answer (field errors as `field-invalid`).
pub(crate) fn onto_error(e: OntologyError) -> Result<Response> {
    let coded = |status, code: &str, msg: &str| Err(coded_error(status, code, msg, None));
    match e {
        OntologyError::Invalid { field, token } => Ok(super::field_error(&field, &token)),
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
        OntologyError::Erased => coded(
            StatusCode::CONFLICT,
            err_codes::ONTOLOGY_ERASED,
            "This object was erased: it is never edited nor linked again.",
        ),
        OntologyError::Opposed => coded(
            StatusCode::CONFLICT,
            err_codes::PROSPECTION_OPPOSED,
            "This person objected to the processing of their data: the link is refused.",
        ),
        OntologyError::ClaimUnsourced => coded(
            StatusCode::CONFLICT,
            err_codes::ONTOLOGY_CLAIM_UNSOURCED,
            "A claim is corroborated only with two pieces of distinct origins.",
        ),
        OntologyError::CaseLegalBasisRequired => coded(
            StatusCode::CONFLICT,
            err_codes::CASE_LEGAL_BASIS_REQUIRED,
            "Declare the processing basis of personal data before attaching a person.",
        ),
        OntologyError::CaseProposalStale => coded(
            StatusCode::CONFLICT,
            err_codes::CASE_PROPOSAL_STALE,
            "This proposal was already reviewed the other way.",
        ),
        OntologyError::ProposalDecided => coded(
            StatusCode::CONFLICT,
            err_codes::PROPOSAL_DECIDED,
            "This proposal was already decided, or a newer one replaced it.",
        ),
        OntologyError::TrackImport(code) => coded(
            StatusCode::UNPROCESSABLE_ENTITY,
            code,
            match code {
                err_codes::TRACK_IMPORT_TIMEZONE_REQUIRED => {
                    "Times carry no offset: give the time zone of the recording."
                }
                err_codes::TRACK_IMPORT_EMPTY => "The file holds no usable point.",
                _ => "Not a GPX file, or a CSV without time, latitude and longitude columns.",
            },
        ),
        OntologyError::ActionNoFlow => coded(
            StatusCode::CONFLICT,
            err_codes::ONTOLOGY_ACTION_NO_FLOW,
            "No deployed flow handles this action.",
        ),
        OntologyError::ActionUnavailable => coded(
            StatusCode::SERVICE_UNAVAILABLE,
            err_codes::ONTOLOGY_ACTION_UNAVAILABLE,
            "The action bus is unavailable: the action was not sent.",
        ),
        OntologyError::UniqueDuplicates { property, values } => Err(coded_error(
            StatusCode::CONFLICT,
            err_codes::ONTOLOGY_UNIQUE_DUPLICATES,
            "Live objects already share values of this property.",
            Some(serde_json::json!({ "property": property, "values": values.join(", ") })),
        )),
        OntologyError::Import { code, args } => Err(coded_error(
            match code {
                err_codes::IMPORT_SOURCE_UNREADABLE
                | err_codes::IMPORT_SOURCE_TOO_LARGE
                | err_codes::IMPORT_SOURCE_UNREACHABLE => StatusCode::UNPROCESSABLE_ENTITY,
                err_codes::IMPORT_SERVER_STOPPING => StatusCode::SERVICE_UNAVAILABLE,
                _ => StatusCode::CONFLICT,
            },
            code,
            "The import was refused.",
            args,
        )),
        OntologyError::Db(e) => {
            tracing::error!(error = %e, "ontology database error");
            Err(crate::services::compute_limits::db_busy(&e).unwrap_or(Error::InternalServerError))
        }
    }
}

/// The caller's masking by classification (crm.md D443) with the schema it
/// reads; `None` when the role reads everything.
pub(crate) async fn masking(ctx: &AppContext, org: &OrgContext) -> onto::Result<Option<Masking>> {
    let r = Redaction::of(|p| org.can(p));
    if r.is_none() {
        return Ok(None);
    }
    Ok(Some(Masking(r, onto::schema(&ctx.db, org.org.id).await?)))
}

pub(crate) struct Masking(pub Redaction, pub onto::OrgSchema);

/// `r` with the caller's masking applied.
async fn masked<T>(
    ctx: &AppContext,
    org: &OrgContext,
    r: onto::Result<T>,
    apply: impl Fn(&Masking, &mut T),
) -> onto::Result<T> {
    let mut v = r?;
    if let Some(m) = masking(ctx, org).await? {
        apply(&m, &mut v);
    }
    Ok(v)
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

/// `limit`/`offset` of a computed sub-collection (D240).
#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct PageQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

impl PageQuery {
    pub(crate) fn page(&self) -> PageParams {
        PageParams::from(self.limit.as_deref(), self.offset.as_deref())
    }
}

/// Pages an already computed list into the D14 envelope; `filters` are kept
/// in the `next`/`previous` links.
pub(crate) fn paged<T: serde::Serialize>(
    path: &str,
    filters: &[(String, String)],
    q: &PageQuery,
    r: onto::Result<Vec<T>>,
) -> Result<Response> {
    let rows = match r {
        Ok(rows) => rows,
        Err(e) => return onto_error(e),
    };
    let page = q.page();
    let count = rows.len() as i64;
    let (skip, take) = page.slice(rows.len());
    let rows: Vec<T> = rows.into_iter().skip(skip).take(take).collect();
    format::json(envelope(path, filters, page, count, rows))
}

pub(crate) fn actor(org: &OrgContext) -> Actor {
    Actor::manual(org.auth.user.id, org.role.clone())
}

pub(crate) fn o2(ctx: &AppContext) -> Option<Client> {
    OpenobserveSettings::from_config(&ctx.config).map(|s| Client::new(&s))
}

pub(crate) fn parse_uuid(raw: &str) -> Result<Uuid> {
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

#[utoipa::path(
    get,
    path = "/api/v1/ontology/types",
    tag = "ontology",
    operation_id = "ontology_types_list",
    summary = "List the object types",
    description = "Every object type visible to the organisation: the system types (version 0, native tables) and the org's own types, each with its live object count. Paginated (`limit`/`offset`). Any org member.",
    params(PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::ontology::api::ObjectTypeView>)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list_types(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let rows = types::list(&ctx.db, org.org.id).await;
    paged("/api/v1/ontology/types", &[], &q, rows)
}

#[utoipa::path(
    get,
    path = "/api/v1/ontology/types/{key}",
    tag = "ontology",
    operation_id = "ontology_types_get",
    summary = "Get an object type",
    description = "One object type (system or org) by its key, at its current version. Any org member.",
    params(("key" = String, Path, description = "Type key (`[a-z][a-z0-9_]*`)")),
    responses(
        (status = 200, body = pnex_core::ontology::api::ObjectTypeView),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn get_type(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(key): Path<String>,
) -> Result<Response> {
    reply(types::get(&ctx.db, org.org.id, &key).await)
}

#[utoipa::path(
    post,
    path = "/api/v1/ontology/types",
    tag = "ontology",
    operation_id = "ontology_types_create",
    summary = "Create an object type",
    description = "Creates an org object type (version 1) from `def`; `expected_version` is ignored. Org owner or admin only. Field errors come back as `field-invalid` (`errors.fields`).",
    request_body = pnex_core::ontology::api::ObjectTypeInput,
    responses(
        (status = 201, body = pnex_core::ontology::api::ObjectTypeView),
        (status = 400, description = "Invalid definition (field → token)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`ontology-already-exists` (key taken, system keys included)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create_type(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<ObjectTypeInput>,
) -> Result<Response> {
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

#[utoipa::path(
    put,
    path = "/api/v1/ontology/types/{key}",
    tag = "ontology",
    operation_id = "ontology_types_update",
    summary = "Publish a new version of an object type",
    description = "Replaces the definition of an org type with `def`, creating a new type version; existing objects keep their data. `expected_version` (optimistic concurrency) must match the current version when set. System types are read-only. Org owner or admin only.",
    params(("key" = String, Path, description = "Type key")),
    request_body = pnex_core::ontology::api::ObjectTypeInput,
    responses(
        (status = 200, body = pnex_core::ontology::api::ObjectTypeView),
        (status = 400, description = "Invalid definition (field → token)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`ontology-version-conflict` (args.current = current version), `ontology-system-read-only`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update_type(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(key): Path<String>,
    Json(input): Json<ObjectTypeInput>,
) -> Result<Response> {
    reply(types::update(&ctx.db, org.org.id, &key, Some(org.auth.user.id), &input).await)
}

#[utoipa::path(
    delete,
    path = "/api/v1/ontology/types/{key}",
    tag = "ontology",
    operation_id = "ontology_types_delete",
    summary = "Delete an object type",
    description = "Deletes an org type that no live object or link type uses any more. System types cannot be deleted. Org owner or admin only. 204 with no body.",
    params(("key" = String, Path, description = "Type key")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`ontology-type-in-use`, `ontology-system-read-only`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn delete_type(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(key): Path<String>,
) -> Result<Response> {
    match types::delete(&ctx.db, org.org.id, &key).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => onto_error(e),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/ontology/types/{key}/versions",
    tag = "ontology",
    operation_id = "ontology_types_versions",
    summary = "List the versions of an object type",
    description = "Every published definition of an org type with its creation date. Paginated (`limit`/`offset`). Any org member.",
    params(("key" = String, Path, description = "Type key"), PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::ontology::api::ObjectTypeVersionView>),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn type_versions(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(key): Path<String>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let rows = types::versions(&ctx.db, org.org.id, &key).await;
    paged(
        &format!("/api/v1/ontology/types/{key}/versions"),
        &[],
        &q,
        rows,
    )
}

// ─────────────────────────── link types ───────────────────────────

#[utoipa::path(
    get,
    path = "/api/v1/ontology/link-types",
    tag = "ontology",
    operation_id = "ontology_link_types_list",
    summary = "List the link types",
    description = "Every link type visible to the organisation (system and org), with the types each one connects. Paginated (`limit`/`offset`). Any org member.",
    params(PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::ontology::api::LinkTypeView>)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list_link_types(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let rows = link_types::list(&ctx.db, org.org.id).await;
    paged("/api/v1/ontology/link-types", &[], &q, rows)
}

#[utoipa::path(
    post,
    path = "/api/v1/ontology/link-types",
    tag = "ontology",
    operation_id = "ontology_link_types_create",
    summary = "Create a link type",
    description = "Creates an org link type (version 1). The body is the bare definition, not wrapped in `def`. Org owner or admin only.",
    request_body = pnex_core::ontology::LinkTypeDef,
    responses(
        (status = 201, body = pnex_core::ontology::api::LinkTypeView),
        (status = 400, description = "Invalid definition (field → token)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`ontology-already-exists`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create_link_type(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(def): Json<LinkTypeDef>,
) -> Result<Response> {
    created(link_types::create(&ctx.db, org.org.id, &def, None).await)
}

#[utoipa::path(
    put,
    path = "/api/v1/ontology/link-types/{key}",
    tag = "ontology",
    operation_id = "ontology_link_types_update",
    summary = "Publish a new version of a link type",
    description = "Replaces the definition of an org link type; `expected_version` must match the current version when set. System link types are read-only. Org owner or admin only.",
    params(("key" = String, Path, description = "Link type key")),
    request_body = pnex_core::ontology::api::LinkTypeInput,
    responses(
        (status = 200, body = pnex_core::ontology::api::LinkTypeView),
        (status = 400, description = "Invalid definition (field → token)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`ontology-version-conflict` (args.current), `ontology-system-read-only`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update_link_type(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(key): Path<String>,
    Json(input): Json<LinkTypeInput>,
) -> Result<Response> {
    reply(link_types::update(&ctx.db, org.org.id, &key, &input).await)
}

#[utoipa::path(
    delete,
    path = "/api/v1/ontology/link-types/{key}",
    tag = "ontology",
    operation_id = "ontology_link_types_delete",
    summary = "Delete a link type",
    description = "Deletes an org link type no link uses any more. System link types cannot be deleted. Org owner or admin only. 204 with no body.",
    params(("key" = String, Path, description = "Link type key")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`ontology-type-in-use`, `ontology-system-read-only`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn delete_link_type(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(key): Path<String>,
) -> Result<Response> {
    match link_types::delete(&ctx.db, org.org.id, &key).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => onto_error(e),
    }
}

// ─────────────────────────── objects ───────────────────────────

#[utoipa::path(
    post,
    path = "/api/v1/ontology/objects",
    tag = "ontology",
    operation_id = "ontology_objects_create",
    summary = "Create an object",
    description = "Creates an object of an org type with a title and properties validated against the type; the change is journaled. Needs a writer role and the type's write role. System objects are created from their own pages, not here.",
    request_body = pnex_core::ontology::api::ObjectInput,
    responses(
        (status = 201, body = pnex_core::ontology::api::ObjectView),
        (status = 400, description = "Invalid field (field → token)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`ontology-write-forbidden`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found` (unknown type)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`ontology-system-read-only`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create_object(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<ObjectInput>,
) -> Result<Response> {
    let r: onto::Result<_> = changes::deferred(async {
        let txn = ctx.db.begin().await?;
        let v = objects::create(&txn, org.org.id, &actor(&org), &input).await?;
        txn.commit().await?;
        changes::flush();
        Ok(v)
    })
    .await;
    created(r)
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
struct InvokeBody {
    /// Action parameters by key, validated against the action's `params`.
    #[serde(default)]
    #[schema(value_type = Object)]
    params: serde_json::Map<String, serde_json::Value>,
}

/// Invokes an action of the object's type (D183): 202 with the flows it
/// was dispatched to, 409 `ontology-action-no-flow` when none listens.
#[utoipa::path(
    post,
    path = "/api/v1/ontology/objects/{id}/actions/{key}",
    tag = "ontology",
    operation_id = "ontology_objects_invoke_action",
    summary = "Invoke an action on an object",
    description = "Publishes an invocation of an action declared on the object's type to the deployed flows that hold a matching action trigger; the server itself executes nothing. Journaled in the object's history. Needs a writer role and the type's write role. Answers 202 with the flows it was dispatched to.",
    params(
        ("id" = String, Path, format = "uuid", description = "Object id (UUID)"),
        ("key" = String, Path, description = "Action key declared on the type")
    ),
    request_body = InvokeBody,
    responses(
        (status = 202, body = pnex_core::ontology::action::ActionInvocation),
        (status = 400, description = "Invalid parameter (field → token)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`ontology-write-forbidden`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found` (object or action)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`ontology-action-no-flow`: no deployed flow handles the action", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`ontology-action-unavailable`: the action bus is down, nothing was sent", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn invoke_action(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, key)): Path<(String, String)>,
    Json(body): Json<InvokeBody>,
) -> Result<Response> {
    let user = &org.auth.user;
    let by = user
        .full_name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| user.email.clone());
    let valkey = crate::services::shared_valkey::conn(&ctx.config).await;
    match onto::actions::invoke(
        &ctx.db,
        valkey,
        org.org.id,
        &actor(&org),
        &by,
        parse_uuid(&id)?,
        &key,
        &body.params,
    )
    .await
    {
        Ok(v) => Ok((StatusCode::ACCEPTED, format::json(v)).into_response()),
        Err(e) => onto_error(e),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/ontology/objects/{id}",
    tag = "ontology",
    operation_id = "ontology_objects_get",
    summary = "Get an object",
    description = "One object of the organisation (system or org type) with its properties and version. Any org member.",
    params(("id" = String, Path, format = "uuid", description = "Object id (UUID); a malformed id answers 404")),
    responses(
        (status = 200, body = pnex_core::ontology::api::ObjectView),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn get_object(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
) -> Result<Response> {
    let r = objects::get(&ctx.db, org.org.id, parse_uuid(&id)?).await;
    reply(masked(&ctx, &org, r, |Masking(m, s), v| m.object(s, v)).await)
}

#[utoipa::path(
    put,
    path = "/api/v1/ontology/objects/{id}",
    tag = "ontology",
    operation_id = "ontology_objects_update",
    summary = "Update an object",
    description = "Replaces the title and properties of an object (for a system object, only the org's extra properties are writable). `expected_version` must match the current version when set. Journaled. Needs a writer role and the type's write role.",
    params(("id" = String, Path, format = "uuid", description = "Object id (UUID)")),
    request_body = pnex_core::ontology::api::ObjectInput,
    responses(
        (status = 200, body = pnex_core::ontology::api::ObjectView),
        (status = 400, description = "Invalid field (field → token)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`ontology-write-forbidden`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`ontology-version-conflict` (args.current), `ontology-system-read-only`, `ontology-erased` (an erased object is a read-only shell)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update_object(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(input): Json<ObjectInput>,
) -> Result<Response> {
    reply(objects::update(&ctx.db, org.org.id, &actor(&org), parse_uuid(&id)?, &input).await)
}

#[utoipa::path(
    delete,
    path = "/api/v1/ontology/objects/{id}",
    tag = "ontology",
    operation_id = "ontology_objects_archive",
    summary = "Archive an object",
    description = "Archives an org object: sets its `valid_to` (history kept) and drops its labels and containment like any deleted resource. System objects are archived from their own pages. Journaled. Needs a writer role and the type's write role. 204 with no body.",
    params(("id" = String, Path, format = "uuid", description = "Object id (UUID)")),
    responses(
        (status = 204, description = "Archived"),
        (status = 403, description = "`ontology-write-forbidden`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`ontology-system-read-only`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn archive_object(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
) -> Result<Response> {
    let id = parse_uuid(&id)?;
    let r: onto::Result<()> = changes::deferred(async {
        let txn = ctx.db.begin().await?;
        objects::archive(&txn, org.org.id, &actor(&org), id).await?;
        txn.commit().await?;
        changes::flush();
        Ok(())
    })
    .await;
    match r {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => onto_error(e),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/ontology/resolve/{kind}/{native_key}",
    tag = "ontology",
    operation_id = "ontology_resolve",
    summary = "Resolve a native row to its object",
    description = "Finds the ontology object that mirrors a native row of a system type (e.g. a device registry id), to reach its links and history. Any org member.",
    params(
        ("kind" = String, Path, description = "System type key (e.g. `device`)"),
        ("native_key" = String, Path, description = "Key of the native row (device id, media id…)")
    ),
    responses(
        (status = 200, body = pnex_core::ontology::api::ObjectView),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn resolve(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((kind, native_id)): Path<(String, String)>,
) -> Result<Response> {
    let s = onto::schema(&ctx.db, org.org.id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let r = objects::find_by_native(&ctx.db, org.org.id, &kind, &native_id)
        .await
        .map(|o| objects::view(&o, &s));
    reply(masked(&ctx, &org, r, |Masking(m, s), v| m.object(s, v)).await)
}

#[utoipa::path(
    post,
    path = "/api/v1/ontology/objects/{id}/erase",
    tag = "ontology",
    operation_id = "ontology_objects_erase",
    summary = "Erase the personal data of an object",
    description = "Removes the properties, link attributes and title the schema classifies personal or sensitive, and keeps the object as an empty shell so the links and provenance of other objects hold (crm.md D446). A person also enters the org's opposition list (its name at each of its accounts, its profile addresses), its captured profile texts are deleted and the identifiers it uses are erased with it. Its history is then read redacted. Irreversible; an object already erased is returned as is. Needs the delete permission and the type's write role.",
    params(("id" = String, Path, format = "uuid", description = "Object id (UUID)")),
    responses(
        (status = 200, body = pnex_core::ontology::api::ObjectView),
        (status = 403, description = "`ontology-write-forbidden`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn erase_object(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
) -> Result<Response> {
    let id = parse_uuid(&id)?;
    reply(onto::erase::erase(&ctx, org.org.id, &actor(&org), id).await)
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct LinksQuery {
    /// RFC 3339: links valid at that instant (default now).
    as_of: Option<String>,
    /// true: closed links too (the whole history).
    history: Option<bool>,
}

#[utoipa::path(
    get,
    path = "/api/v1/ontology/objects/{id}/links",
    tag = "ontology",
    operation_id = "ontology_objects_links",
    summary = "List the links of an object",
    description = "Links where the object is source or target, open at `as_of` (default now), or every link ever with `history=true`. Paginated (`limit`/`offset`). Any org member.",
    params(("id" = String, Path, format = "uuid", description = "Object id (UUID)"), LinksQuery, PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::ontology::api::LinkView>),
        (status = 400, description = "Invalid `as_of` (field → token)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn object_links(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Query(q): Query<LinksQuery>,
    Query(page): Query<PageQuery>,
) -> Result<Response> {
    let as_of = match onto::parse_instant("as_of", q.as_of.as_deref()) {
        Ok(t) => t,
        Err(e) => return onto_error(e),
    };
    let history = q.history.unwrap_or(false);
    let rows = links::for_object(&ctx.db, org.org.id, parse_uuid(&id)?, as_of, history).await;
    let rows = masked(&ctx, &org, rows, |Masking(m, s), rows| {
        rows.iter_mut().for_each(|l| m.link(s, l))
    })
    .await;
    let mut filters = vec![("history".to_string(), history.to_string())];
    if let Some(t) = q.as_of {
        filters.push(("as_of".to_string(), t));
    }
    paged(
        &format!("/api/v1/ontology/objects/{id}/links"),
        &filters,
        &page,
        rows,
    )
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct GraphQuery {
    /// Hops from the object (graph: 1–2, default 1; impact: 1–4, default 4).
    depth: Option<u32>,
    /// Impact only: walk the links backwards (what this object depends on).
    upstream: Option<bool>,
}

#[utoipa::path(
    get,
    path = "/api/v1/ontology/objects/{id}/graph",
    tag = "ontology",
    operation_id = "ontology_objects_graph",
    summary = "Get the neighbourhood graph of an object",
    description = "Objects within `depth` hops (clamped to 1–2) through open links, and the links among them. `upstream` is ignored here. Any org member.",
    params(("id" = String, Path, format = "uuid", description = "Object id (UUID)"), GraphQuery),
    responses(
        (status = 200, body = pnex_core::ontology::api::Neighborhood),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn object_graph(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Query(q): Query<GraphQuery>,
) -> Result<Response> {
    let r = graph::neighborhood(&ctx.db, org.org.id, parse_uuid(&id)?, q.depth.unwrap_or(1)).await;
    reply(masked(&ctx, &org, r, |Masking(m, s), n| m.neighborhood(s, n)).await)
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct HierarchyQuery {
    /// Hierarchy key (default: the first one of the installed packs).
    key: Option<String>,
    /// Links valid at this instant (RFC 3339, default now).
    at: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/ontology/objects/{id}/hierarchy",
    tag = "ontology",
    operation_id = "ontology_objects_hierarchy",
    summary = "Get the hierarchy tree under an object",
    description = "Tree computed on the links a hierarchy of an installed pack declares (depth ≤ 6, 500 nodes), links valid at `at`. A node with several parents appears under each, with the others in `also_under`. Lists the installed hierarchies too. Any org member.",
    params(("id" = String, Path, format = "uuid", description = "Object id (UUID)"), HierarchyQuery),
    responses(
        (status = 200, body = pnex_core::ontology::api::HierarchyView),
        (status = 400, description = "Invalid `at` (field → token)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn object_hierarchy(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Query(q): Query<HierarchyQuery>,
) -> Result<Response> {
    let at = match onto::parse_instant("at", q.at.as_deref()) {
        Ok(t) => t,
        Err(e) => return onto_error(e),
    };
    let hide = onto::redact::Redaction::of(|p| org.can(p));
    let r = onto::hierarchy::tree(
        &ctx.db,
        org.org.id,
        parse_uuid(&id)?,
        q.key.as_deref(),
        at,
        hide,
    )
    .await;
    reply(masked(&ctx, &org, r, |Masking(m, s), h| m.hierarchy(s, h)).await)
}

#[utoipa::path(
    get,
    path = "/api/v1/ontology/objects/{id}/impact",
    tag = "ontology",
    operation_id = "ontology_objects_impact",
    summary = "List the objects impacted by an object",
    description = "Objects reachable from the object through open links within `depth` hops (1–4, default 4): downstream by default, upstream with `upstream=true`. Any org member.",
    params(("id" = String, Path, format = "uuid", description = "Object id (UUID)"), GraphQuery),
    responses(
        (status = 200, body = pnex_core::ontology::api::ObjectImpact),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
    let r = graph::impact(
        &ctx.db,
        org.org.id,
        parse_uuid(&id)?,
        q.upstream.unwrap_or(false),
        depth,
    )
    .await
    .map(|objects| pnex_core::ontology::api::ObjectImpact { objects });
    reply(masked(&ctx, &org, r, |Masking(m, s), i| m.refs(s, &mut i.objects)).await)
}

#[utoipa::path(
    post,
    path = "/api/v1/ontology/objects/{id}/pages",
    tag = "ontology",
    operation_id = "ontology_objects_pages_create",
    summary = "Create a page from a template, filled from an object",
    description = "Creates a page from an installed pack's template whose subject is the object's type (an account plan, a competitor sheet, a meeting preparation): its placeholders are filled with the object, its open links and its hierarchies as of now, then the page is linked to the object. The page is a snapshot, edited freely afterwards. Needs the right to write pages, and to write the object's type for the link.",
    params(("id" = String, Path, format = "uuid", description = "Object id")),
    request_body = pnex_core::ontology::api::ObjectPageInput,
    responses(
        (status = 201, body = pnex_core::pages::DocSummary),
        (status = 400, description = "Unknown template for this type, or title too long (`field-invalid`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`, or `ontology-write-forbidden` by the type's write role", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn object_page(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Json(input): Json<pnex_core::ontology::api::ObjectPageInput>,
) -> Result<Response> {
    let id = parse_uuid(&id)?;
    created(onto::cases::pages::from_object(&ctx.db, org.org.id, &actor(&org), id, &input).await)
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct PathQuery {
    /// Start object id (UUID).
    from: Option<String>,
    /// End object id (UUID).
    to: Option<String>,
    /// Link type keys the path may follow, comma-separated (at most 20);
    /// all when absent. An introduction path follows the links between
    /// people: `knows,recommande,employs,dirige,member_of`.
    links: Option<String>,
    /// Instead of `from`: every object at the source of an open link of
    /// these types, comma-separated (`suit_le_compte` = the team that
    /// follows accounts); the shortest path from any of them wins (D338).
    from_links: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/ontology/path",
    tag = "ontology",
    operation_id = "ontology_path",
    summary = "Find a path between two objects",
    description = "Shortest chain of open links, followed in either direction, within 4 hops, from `from` or from the nearest object at the source of an open link of a `from_links` type (several starts), along the given link types only when `links` is set: the objects along it, ends included. 404 when there is no such path. Any org member.",
    params(PathQuery),
    responses(
        (status = 200, body = pnex_core::ontology::api::ObjectPath),
        (status = 400, description = "Missing `from` (or `from_links`) or `to`, or more than 20 `links` (`field-invalid`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found` (malformed or unknown id, or no path)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn path(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PathQuery>,
) -> Result<Response> {
    let depth = pnex_core::ontology::api::MAX_HOPS as u32;
    let keys = |raw: &Option<String>| -> Vec<String> {
        raw.as_deref()
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect()
    };
    let (links, from_links) = (keys(&q.links), keys(&q.from_links));
    for (field, list) in [("links", &links), ("from_links", &from_links)] {
        if list.len() > 20 || list.iter().any(|l| l.len() > 64) {
            return onto_error(onto::invalid(field, err_codes::FIELD_INVALID));
        }
    }
    let Some(to) = q.to else {
        return onto_error(onto::invalid("to", err_codes::FIELD_REQUIRED));
    };
    let to = parse_uuid(&to)?;
    let found = match (q.from, from_links.is_empty()) {
        (Some(from), _) => {
            graph::path(&ctx.db, org.org.id, parse_uuid(&from)?, to, depth, &links).await
        }
        (None, false) => {
            graph::path_from_any(&ctx.db, org.org.id, &from_links, to, depth, &links).await
        }
        (None, true) => return onto_error(onto::invalid("from", err_codes::FIELD_REQUIRED)),
    };
    let r = found.map(|path| pnex_core::ontology::api::ObjectPath { path });
    reply(masked(&ctx, &org, r, |Masking(m, s), p| m.refs(s, &mut p.path)).await)
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct SeriesQuery {
    /// `5m` | `15m` | `30m` | `1h` | `6h` | `24h` (default) | `7d` | `30d`.
    window: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/ontology/objects/{id}/series/{property}",
    tag = "ontology",
    operation_id = "ontology_objects_series",
    summary = "Read the time series of an object property",
    description = "Recomposes a `series` property over the window from the telemetry of every device that measured it (`measures` links), segment by segment, so a sensor replacement does not break the curve. Empty points when telemetry is unavailable. Any org member.",
    params(
        ("id" = String, Path, format = "uuid", description = "Object id (UUID)"),
        ("property" = String, Path, description = "Key of a `series` property of the object's type"),
        SeriesQuery
    ),
    responses(
        (status = 200, body = pnex_core::ontology::api::SeriesView),
        (status = 400, description = "Invalid `window` or property (field → token)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
#[utoipa::path(
    get,
    path = "/api/v1/ontology/objects/{id}/bindings",
    tag = "ontology",
    operation_id = "ontology_objects_bindings",
    summary = "List the current sensors of an object",
    description = "For each `series` property bound by an open `measures` link: the telemetry `device_id` label and metric that feed it now (what a type dashboard reads). Any org member.",
    params(("id" = String, Path, format = "uuid", description = "Object id (UUID)"), PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::SeriesBinding>),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn object_bindings(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let id = parse_uuid(&id)?;
    if let Err(e) = objects::find(&ctx.db, org.org.id, id).await {
        return onto_error(e);
    }
    let rows = series::current_bindings(&ctx.db, org.org.id, id).await;
    paged(
        &format!("/api/v1/ontology/objects/{id}/bindings"),
        &[],
        &q,
        rows,
    )
}

#[utoipa::path(
    get,
    path = "/api/v1/ontology/objects/{id}/changes",
    tag = "ontology",
    operation_id = "ontology_objects_changes",
    summary = "Get the change history of an object",
    description = "The last 200 changes of the object (create, update, archive, link open/close, action invocations) from the change journal, newest first, with before/after values. Paginated (`limit`/`offset`). 503 `ontology-changes-unavailable` when the journal cannot be read. Any org member.",
    params(("id" = String, Path, format = "uuid", description = "Object id (UUID)"), PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::ontology::api::ChangeView>),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`ontology-changes-unavailable`: the change journal cannot be read", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn object_changes(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let id = parse_uuid(&id)?;
    let erased = match objects::find(&ctx.db, org.org.id, id).await {
        Ok(o) => o.erased_at.is_some(),
        Err(e) => return onto_error(e),
    };
    let rows = match changes::history(&ctx, org.org.id, &id.to_string(), 200).await {
        Ok(Some(rows)) => rows,
        Ok(None) => return Err(changes_unavailable()),
        Err(e) => {
            tracing::warn!("object changes read failed: {e}");
            return Err(changes_unavailable());
        }
    };
    // The history of an erased object never shows what was erased (D446).
    let rows = match erased {
        true => onto::schema(&ctx.db, org.org.id)
            .await
            .map_err(Into::into)
            .map(|s| {
                let mut rows = rows;
                rows.iter_mut().for_each(|c| Redaction::ALL.change(&s, c));
                rows
            }),
        false => {
            masked(&ctx, &org, Ok(rows), |Masking(m, s), rows| {
                rows.iter_mut().for_each(|c| m.change(s, c))
            })
            .await
        }
    };
    paged(
        &format!("/api/v1/ontology/objects/{id}/changes"),
        &[],
        &q,
        rows,
    )
}

#[utoipa::path(
    get,
    path = "/api/v1/ontology/objects/{id}/evidence",
    tag = "ontology",
    operation_id = "ontology_objects_evidence",
    summary = "List the evidence of an object",
    description = "The media proving the object (external-apps.md D437), reached through link types declared `evidence`, two hops at most (a finding, the page it was seen in, the parts of that page's web proof), open links only. Each item gives the media id to download (`GET /media/{id}/content`, `media:read`), its current version and SHA-256, the link types followed, and for a web proof part the proof's seal (sequence, manifest SHA-256, previous seal). Paginated (`limit`/`offset`). Any org member.",
    params(("id" = String, Path, format = "uuid", description = "Object id (UUID)"), PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::ontology::api::EvidenceItem>),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn object_evidence(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let id = parse_uuid(&id)?;
    paged(
        &format!("/api/v1/ontology/objects/{id}/evidence"),
        &[],
        &q,
        crate::services::ontology::evidence::of(&ctx.db, org.org.id, id).await,
    )
}

fn changes_unavailable() -> Error {
    coded_error(
        StatusCode::SERVICE_UNAVAILABLE,
        err_codes::ONTOLOGY_CHANGES_UNAVAILABLE,
        "The change journal cannot be read.",
        None,
    )
}

// ─────────────────────────── links ───────────────────────────

#[utoipa::path(
    post,
    path = "/api/v1/ontology/links",
    tag = "ontology",
    operation_id = "ontology_links_create",
    summary = "Create a link between two objects",
    description = "Opens a link of `link_type` from `source_id` to `target_id`, starting at `valid_from` (default now; may be in the past). With `valid_to` (after `valid_from`, not in the future) the link is written closed, as history, even while the same pair has an open link; cardinalities bind open links only. Checks the link type allows these object types and its cardinality. Journaled on both ends. Needs a writer role.",
    request_body = pnex_core::ontology::api::LinkInput,
    responses(
        (status = 201, body = pnex_core::ontology::api::LinkView),
        (status = 400, description = "Invalid field (field → token), or `ontology-link-not-allowed`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`ontology-write-forbidden`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found` (link type or object)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`ontology-link-cardinality` (an open link already takes this place), `ontology-already-exists`, `ontology-erased` (an end was erased), `prospection-opposed`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create_link(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<LinkInput>,
) -> Result<Response> {
    let r: onto::Result<_> = changes::deferred(async {
        let txn = ctx.db.begin().await?;
        let v = links::create(&txn, org.org.id, &actor(&org), &input).await?;
        txn.commit().await?;
        changes::flush();
        Ok(v)
    })
    .await;
    created(r)
}

#[utoipa::path(
    put,
    path = "/api/v1/ontology/links/{id}",
    tag = "ontology",
    operation_id = "ontology_links_update",
    summary = "Update a link's attributes",
    description = "Replaces the attributes of a link, open or closed, validated against its link type (a source added, a note corrected). Ends, type and validity do not change: close the link and create another. Not for `measures` or `depicts` bindings. Journaled as `link.update` with the previous version. Needs a writer role.",
    params(("id" = i64, Path, description = "Link id")),
    request_body = pnex_core::ontology::api::LinkUpdate,
    responses(
        (status = 200, body = pnex_core::ontology::api::LinkView),
        (status = 400, description = "Invalid attribute (field → token)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`ontology-write-forbidden`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update_link(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Json(input): Json<LinkUpdate>,
) -> Result<Response> {
    let r: onto::Result<_> = changes::deferred(async {
        let txn = ctx.db.begin().await?;
        let v = links::update(&txn, org.org.id, &actor(&org), id, &input.attributes).await?;
        txn.commit().await?;
        changes::flush();
        Ok(v)
    })
    .await;
    reply(r)
}

#[utoipa::path(
    post,
    path = "/api/v1/ontology/links/{id}/close",
    tag = "ontology",
    operation_id = "ontology_links_close",
    summary = "Close a link",
    description = "Ends an open link at `valid_to` (default now); the link stays in the history. The body is optional (empty = now). Journaled. Needs a writer role.",
    params(("id" = i64, Path, description = "Link id")),
    request_body(content = Option<pnex_core::ontology::api::LinkClose>, description = "Optional; empty body = close now"),
    responses(
        (status = 200, body = pnex_core::ontology::api::LinkView),
        (status = 400, description = "Invalid `valid_to` (field → token)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`ontology-write-forbidden`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn close_link(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    raw: axum::body::Bytes,
) -> Result<Response> {
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
    let r: onto::Result<_> = changes::deferred(async {
        let txn = ctx.db.begin().await?;
        let v = links::close(&txn, org.org.id, &actor(&org), id, valid_to.as_deref()).await?;
        txn.commit().await?;
        changes::flush();
        Ok(v)
    })
    .await;
    reply(r)
}

// ─────────────────────────── query ───────────────────────────

#[utoipa::path(
    post,
    path = "/api/v1/ontology/query",
    tag = "ontology",
    operation_id = "ontology_query",
    summary = "Run an ontology query",
    description = "Read-only declarative query: selects objects (by type, ids, title text, property filters, label, containment), traverses up to 4 link hops, optionally joins the last value of `series` properties, then pages (`limit` up to 500, `offset`). `count` is the total before paging. Any org member.",
    request_body = pnex_core::ontology::api::OntologyQuery,
    responses(
        (status = 200, body = pnex_core::ontology::api::QueryResult),
        (status = 400, description = "Invalid query (field → token)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn run_query(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(q): Json<OntologyQuery>,
) -> Result<Response> {
    let client = o2(&ctx);
    let hide = onto::redact::Redaction::of(|p| org.can(p));
    let r = query::run(&ctx.db, client.as_ref(), org.org.id, &q, hide).await;
    reply(masked(&ctx, &org, r, |Masking(m, s), r| m.query(s, r)).await)
}

// ─────────────────────────── packs ───────────────────────────

#[utoipa::path(
    get,
    path = "/api/v1/ontology/packs",
    tag = "ontology",
    operation_id = "ontology_packs_list",
    summary = "List the ontology packs",
    description = "The packs shipped with the server (ready-made types, link types and page templates) and the version the org installed, if any. Any org member.",
    params(PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::ontology::api::PackView>)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list_packs(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let rows = packs::list(&ctx.db, org.org.id).await;
    paged("/api/v1/ontology/packs", &[], &q, rows)
}

#[utoipa::path(
    post,
    path = "/api/v1/ontology/packs/{key}/install",
    tag = "ontology",
    operation_id = "ontology_packs_install",
    summary = "Install a shipped pack",
    description = "Installs (or upgrades) a shipped pack into the org: creates or versions its object types, link types and page templates. No body. Org owner or admin only.",
    params(("key" = String, Path, description = "Pack key")),
    responses(
        (status = 200, body = pnex_core::ontology::api::PackView),
        (status = 400, description = "Invalid pack content (field → token)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found` (unknown pack)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`ontology-already-exists`, `ontology-system-read-only`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn install_pack(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(key): Path<String>,
) -> Result<Response> {
    let Some(pack) = packs::shipped().into_iter().find(|p| p.key == key) else {
        return onto_error(OntologyError::NotFound);
    };
    reply(packs::install(&ctx.db, org.org.id, Some(org.auth.user.id), &pack).await)
}

#[utoipa::path(
    post,
    path = "/api/v1/ontology/packs/{key}/take",
    tag = "ontology",
    operation_id = "ontology_packs_take",
    summary = "Take the pack's version of edited functions and flows",
    description = "Writes the installed pack's version of functions and flows the org edited (names listed in the pack's `kept`) as a new version over the org's, which stays in their history. A flow runs it at its next deploy. Org owner or admin only.",
    params(("key" = String, Path, description = "Pack key")),
    request_body = pnex_core::ontology::api::PackTakeInput,
    responses(
        (status = 200, body = pnex_core::ontology::api::PackView),
        (status = 400, description = "A name the org did not edit, or the pack is not at its shipped version (`field-invalid`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found` (unknown or not installed pack)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn take_pack_version(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(key): Path<String>,
    Json(input): Json<pnex_core::ontology::api::PackTakeInput>,
) -> Result<Response> {
    reply(packs::take(&ctx.db, org.org.id, Some(org.auth.user.id), &key, &input).await)
}

#[utoipa::path(
    patch,
    path = "/api/v1/ontology/packs/{key}/settings",
    tag = "ontology",
    operation_id = "ontology_packs_settings_update",
    summary = "Set the org's values of a pack's settings",
    description = "Overrides settings an installed pack declares (thresholds, weights) in the org's own layer, kept across pack upgrades; `null` returns a setting to the pack's default. Pack functions read the values in force at the next deploy of their flow. Org owner or admin only.",
    params(("key" = String, Path, description = "Pack key")),
    request_body = pnex_core::ontology::api::PackSettingsInput,
    responses(
        (status = 200, body = pnex_core::ontology::api::PackView),
        (status = 400, description = "A key the pack does not declare, or a value of the wrong kind or out of bounds (`field-invalid`, `field-max-length`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found` (unknown or not installed pack)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update_pack_settings(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(key): Path<String>,
    Json(input): Json<pnex_core::ontology::api::PackSettingsInput>,
) -> Result<Response> {
    reply(packs::set_settings(&ctx.db, org.org.id, &key, &input).await)
}

#[utoipa::path(
    get,
    path = "/api/v1/ontology/export",
    tag = "ontology",
    operation_id = "ontology_export",
    summary = "Export the org schema as a pack",
    description = "The org's own object types and link types as pack YAML (the format `POST /api/v1/ontology/import` reads). Any org member.",
    responses(
        (status = 200, content_type = "application/yaml", body = String)
    ),
    security(("bearer" = [], "org" = []))
)]
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
#[utoipa::path(
    post,
    path = "/api/v1/ontology/import",
    tag = "ontology",
    operation_id = "ontology_import",
    summary = "Import a pack from YAML",
    description = "Installs a pack given as YAML text (schema as code): creates or versions its object types, link types and page templates in the org. Body up to 512 KiB. Org owner or admin only.",
    request_body(content = String, content_type = "application/yaml", description = "Pack YAML"),
    responses(
        (status = 200, body = pnex_core::ontology::api::PackView),
        (status = 400, description = "Not UTF-8, unreadable YAML or invalid content (field → token)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`ontology-already-exists`, `ontology-system-read-only`", body = crate::openapi::ErrorDetail),
        (status = 413, description = "Body over the limit")
    ),
    security(("bearer" = [], "org" = []))
)]
async fn import(
    State(ctx): State<AppContext>,
    org: OrgContext,
    raw: axum::body::Bytes,
) -> Result<Response> {
    let Ok(body) = std::str::from_utf8(&raw) else {
        return onto_error(onto::invalid("yaml", err_codes::FIELD_INVALID));
    };
    let pack = match packs::parse(body) {
        Ok(p) => p,
        Err(e) => return onto_error(e),
    };
    reply(packs::install(&ctx.db, org.org.id, Some(org.auth.user.id), &pack).await)
}
