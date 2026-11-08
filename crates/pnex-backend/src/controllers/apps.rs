//! Applications API (external-apps.md D433) under `/api/v1/apps`. Org
//! from the principal (R1). Reading needs `apps:read`; declaring, changing
//! and deleting need `apps:write` / `apps:delete` (Admin). `GET /apps/me`
//! answers the application a service account signs in for. Its screens
//! (app-builder.md D506): a reader runs its object sets with its own rights,
//! a builder changes them with `apps:build`.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use pnex_core::apps::{AppBuildInput, AppInput, AppPublishInput, AppScreenView, AppView};
use pnex_core::err_codes;
use std::collections::HashMap;
use uuid::Uuid;

use super::coded_error;
use super::ontology::{paged, PageQuery};
use crate::auth::OrgContext;
use crate::services::apps::{self, AppError};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/apps")
        .add("/", get(list).post(create))
        .add("/me", get(me))
        .add("/me/object-sets/{name}", get(me_object_set))
        .add("/{id}", get(one).put(update).delete(remove))
        .add("/{id}/object-sets/{name}", get(object_set))
        .add(
            "/{id}/object-sets/{name}/aggregate/query",
            post(object_set_aggregate),
        )
        .add("/{id}/screens/{key}", get(screen))
        .add("/{id}/build", axum::routing::put(build))
        .add("/{id}/publish", post(publish))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(
        list,
        me,
        me_object_set,
        one,
        object_set,
        object_set_aggregate,
        screen,
        create,
        update,
        build,
        publish,
        remove
    ))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

pub(crate) fn app_error(e: AppError) -> Result<Response> {
    match e {
        AppError::Fields(fields) => Ok(super::fields_error(
            StatusCode::BAD_REQUEST,
            fields
                .into_iter()
                .map(|(k, v)| (k, serde_json::Value::String(v)))
                .collect(),
        )),
        AppError::NotFound => Err(coded_error(
            StatusCode::NOT_FOUND,
            err_codes::APP_NOT_FOUND,
            "Application not found.",
            None,
        )),
        AppError::VersionConflict { current } => Err(coded_error(
            StatusCode::CONFLICT,
            err_codes::APP_VERSION_CONFLICT,
            "The application changed since it was read; reload it.",
            Some(serde_json::json!({ "current": current.to_string() })),
        )),
        AppError::Violations(v) => Ok(super::violations_error(err_codes::DASHBOARD_INVALID, v)),
        AppError::OutOfScope => Err(coded_error(
            StatusCode::FORBIDDEN,
            err_codes::APP_OUT_OF_SCOPE,
            "This application does not permit this write.",
            None,
        )),
        AppError::Query(e) => super::ontology::onto_error(e),
        AppError::Db(e) => Err(e.into()),
    }
}

const INPUT_400: &str = "`field-invalid`: `key` (`required`, `invalid`, `unique`), `name` (`required`, `max_length:120`), `description` (`max_length:2000`), `service_account_id` (`invalid-uuid`, `not-found`, `unique`), `scope.types` / `scope.link_types` (`invalid`, `not-found`), `scope.media_kinds` / `scope.media_max_bytes` (`invalid`), `actions` (`out-of-scope`), `expected_version` (`required`, on a replace)";

#[utoipa::path(
    get,
    path = "/api/v1/apps",
    tag = "apps",
    operation_id = "apps_list",
    summary = "List the applications",
    description = "Applications declared in the org (external programs built on the ontology), sorted by name, paginated. Any org member.",
    params(PageQuery),
    responses((status = 200, body = pnex_core::Paginated<AppView>)),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    match apps::list(&ctx.db, org.org.id).await {
        Ok(rows) => paged("/api/v1/apps", &[], &q, Ok(rows)),
        Err(e) => app_error(e),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/apps/me",
    tag = "apps",
    operation_id = "apps_me",
    summary = "Get the calling application",
    description = "The application whose service account signs this request: its key and its scope. 404 `app-not-found` for a person or a service account no application uses. Any org member, service accounts included.",
    responses(
        (status = 200, body = AppView),
        (status = 404, description = "`app-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn me(State(ctx): State<AppContext>, org: OrgContext) -> Result<Response> {
    let Some(sp) = org.auth.service.as_ref() else {
        return app_error(AppError::NotFound);
    };
    match apps::of_service_account(&ctx.db, org.org.id, sp.id).await {
        Ok(Some(a)) => format::json(a),
        Ok(None) => app_error(AppError::NotFound),
        Err(e) => app_error(e),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/apps/me/object-sets/{name}",
    tag = "apps",
    operation_id = "apps_me_object_sets_run",
    summary = "Run an object set of the calling application",
    description = "Runs a named query of the application whose service account signs the request (D434). Every query parameter other than `limit`, `offset`, `sort` and `desc` fills the set's parameter of that name (`$code` in a filter value or the text); a missing one answers 400 with that field `required`. The set keeps to the application's scope (checked when it is saved). Answers like `POST /ontology/query`. Service accounts of an application.",
    params(
        ("name" = String, Path, description = "Object set name"),
        ("limit" = Option<i64>, Query, description = "Rows per page (default 50, at most 500)"),
        ("offset" = Option<i64>, Query, description = "Rows to skip"),
        ("sort" = Option<String>, Query, description = "Property to sort on, instead of the set's order"),
        ("desc" = Option<bool>, Query, description = "With `sort`: descending")
    ),
    responses(
        (status = 200, body = pnex_core::ontology::api::QueryResult),
        (status = 400, description = "A parameter of the set is missing (`<name>`: `required`), or the query is invalid", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`app-not-found`: no application for the caller, or no set of that name", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn me_object_set(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(name): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response> {
    let Some(sp) = org.auth.service.as_ref() else {
        return app_error(AppError::NotFound);
    };
    let app = match apps::of_service_account(&ctx.db, org.org.id, sp.id).await {
        Ok(Some(a)) => a,
        Ok(None) => return app_error(AppError::NotFound),
        Err(e) => return app_error(e),
    };
    run_set(&ctx, &org, &app, &name, params).await
}

/// Runs a set with the caller's rights: the reserved query parameters page
/// and sort, the others fill the set's parameters. Masked by classification
/// like every ontology read (crm.md D443).
async fn run_set(
    ctx: &AppContext,
    org: &OrgContext,
    app: &AppView,
    name: &str,
    mut params: HashMap<String, String>,
) -> Result<Response> {
    let mut number = |k: &str| params.remove(k).and_then(|v| v.parse::<u64>().ok());
    let (limit, offset) = (number("limit"), number("offset"));
    let sort = params.remove("sort").map(|k| {
        let descending = params.remove("desc").is_some_and(|d| d == "true");
        (k, descending)
    });
    params.remove("desc");
    let hide = super::ontology::redaction(ctx, org)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let client = super::ontology::o2(ctx);
    match apps::run_set(
        &ctx.db,
        client.as_ref(),
        org.org.id,
        app,
        name,
        &params,
        limit,
        offset,
        sort,
        hide,
    )
    .await
    {
        Ok(mut r) => {
            match super::ontology::masking(ctx, org).await {
                Ok(Some(super::ontology::Masking(m, s))) => m.query(&s, &mut r),
                Ok(None) => {}
                Err(e) => return super::ontology::onto_error(e),
            }
            format::json(r)
        }
        Err(e) => app_error(e),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/apps/{id}/object-sets/{name}",
    tag = "apps",
    operation_id = "apps_object_sets_run",
    summary = "Run an object set of an application",
    description = "For a screen of the application (app-builder.md D508): runs a named query with the caller's own rights, masked by classification. An application gives no right, it only narrows: the set keeps to its scope, checked when it is saved. Every query parameter other than `limit`, `offset`, `sort` and `desc` fills the set's parameter of that name, as a value only. Answers like `POST /ontology/query`. Needs the ontology read permission.",
    params(
        ("id" = String, Path, format = "uuid", description = "Application id"),
        ("name" = String, Path, description = "Object set name"),
        ("limit" = Option<i64>, Query, description = "Rows per page (default 50, at most 500)"),
        ("offset" = Option<i64>, Query, description = "Rows to skip"),
        ("sort" = Option<String>, Query, description = "Property to sort on, instead of the set's order"),
        ("desc" = Option<bool>, Query, description = "With `sort`: descending")
    ),
    responses(
        (status = 200, body = pnex_core::ontology::api::QueryResult),
        (status = 400, description = "A parameter of the set is missing (`<name>`: `required`), or the query is invalid", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`app-not-found`: no such application or set", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn object_set(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, name)): Path<(Uuid, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response> {
    match apps::get(&ctx.db, org.org.id, id).await {
        Ok(app) => run_set(&ctx, &org, &app, &name, params).await,
        Err(e) => app_error(e),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/apps/{id}/object-sets/{name}/aggregate/query",
    tag = "apps",
    operation_id = "apps_object_sets_aggregate_query",
    summary = "Aggregate an object set of an application",
    description = "For a widget of a screen: read-only count, sum, average, minimum, maximum and ratios over the objects of a named set, optionally split by property, date or link, with the reader's rights. A money property answers one value per currency. Refused when it names what the reader may not read. Permission: ontology:read.",
    params(
        ("id" = String, Path, format = "uuid", description = "Application id"),
        ("name" = String, Path, description = "Object set name")
    ),
    request_body = pnex_core::ontology::aggregate::SetAggregateQuery,
    responses(
        (status = 200, body = pnex_core::ontology::aggregate::AggregateResult),
        (status = 400, description = "Invalid request, or a parameter of the set is missing (field → token)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Names what the reader may not read (`access-denied`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`app-not-found`: no such application or set", body = crate::openapi::ErrorDetail),
        (status = 503, description = "Ran past its time cap (`ontology-aggregate-timeout`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn object_set_aggregate(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, name)): Path<(Uuid, String)>,
    Json(req): Json<pnex_core::ontology::aggregate::SetAggregateQuery>,
) -> Result<Response> {
    let hide = super::ontology::redaction(&ctx, &org)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let run = async {
        let app = apps::get(&ctx.db, org.org.id, id).await?;
        apps::aggregate_set(&ctx.db, org.org.id, &app, &name, &req, hide).await
    };
    match run.await {
        Ok(r) => format::json(r),
        Err(e) => app_error(e),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/apps/{id}/screens/{key}",
    tag = "apps",
    operation_id = "apps_screens_get",
    summary = "Get a published screen of an application",
    description = "The published version of a screen, as its readers see it (app-builder.md D507, Q-B2). 404 before its first publication. Any org member.",
    params(
        ("id" = String, Path, format = "uuid", description = "Application id"),
        ("key" = String, Path, description = "Screen key")
    ),
    responses(
        (status = 200, body = AppScreenView),
        (status = 404, description = "`app-not-found`: no such application or published screen", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn screen(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, key)): Path<(Uuid, String)>,
) -> Result<Response> {
    let app = match apps::get(&ctx.db, org.org.id, id).await {
        Ok(a) => a,
        Err(e) => return app_error(e),
    };
    match apps::screen(&ctx.db, &app, &key).await {
        Ok(s) => format::json(s),
        Err(e) => app_error(e),
    }
}

#[utoipa::path(
    put,
    path = "/api/v1/apps/{id}/build",
    tag = "apps",
    operation_id = "apps_build",
    summary = "Change the screens, variables and object sets of an application",
    description = "What a builder changes (app-builder.md Q-B1), on the version it was read on: screens in display order (a new key creates its screen, a missing key deletes it with its versions), home screen, variables, object sets kept to the application's scope. The scope and the permitted actions stay with the owner or admin. Needs the build permission.",
    params(("id" = String, Path, format = "uuid", description = "Application id")),
    request_body = AppBuildInput,
    responses(
        (status = 200, body = AppView),
        (status = 400, description = "`field-invalid`: `screens.<i>.key`, `screens.<i>.name`, `home`, `variables.<i>.*`, `object_sets.<name>.*`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`app-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`app-version-conflict`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn build(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<AppBuildInput>,
) -> Result<Response> {
    match apps::build(&ctx, org.org.id, id, &input, &org.auth.user.email).await {
        Ok(a) => format::json(a),
        Err(e) => app_error(e),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/apps/{id}/publish",
    tag = "apps",
    operation_id = "apps_publish",
    summary = "Publish the screens of an application",
    description = "Readers now see the current version of every screen (app-builder.md Q-B2), each checked against the application as it is now. Needs the build permission.",
    params(("id" = String, Path, format = "uuid", description = "Application id")),
    request_body = AppPublishInput,
    responses(
        (status = 200, body = AppView),
        (status = 400, description = "`dashboard-invalid`: a screen names what the application does not have", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`app-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`app-version-conflict`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn publish(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<AppPublishInput>,
) -> Result<Response> {
    match apps::publish(&ctx.db, org.org.id, id, input.expected_version).await {
        Ok(a) => format::json(a),
        Err(e) => app_error(e),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/apps/{id}",
    tag = "apps",
    operation_id = "apps_get",
    summary = "Get an application",
    description = "An application of the org with its scope. Any org member.",
    params(("id" = String, Path, format = "uuid", description = "Application id")),
    responses(
        (status = 200, body = AppView),
        (status = 404, description = "`app-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn one(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match apps::get(&ctx.db, org.org.id, id).await {
        Ok(a) => format::json(a),
        Err(e) => app_error(e),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/apps",
    tag = "apps",
    operation_id = "apps_create",
    summary = "Declare an application",
    description = "Declares an external application of the org: a key, the service account it signs in with (one application per account) and its scope (object types, link types, media kinds and size it copies). The change feed keeps to that scope when the application calls. Owner or admin.",
    request_body = AppInput,
    responses(
        (status = 201, body = AppView),
        (status = 400, description = INPUT_400, body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<AppInput>,
) -> Result<Response> {
    match apps::create(&ctx.db, org.org.id, org.auth.user.id, &input).await {
        Ok(a) => Ok((StatusCode::CREATED, format::json(a)).into_response()),
        Err(e) => app_error(e),
    }
}

#[utoipa::path(
    put,
    path = "/api/v1/apps/{id}",
    tag = "apps",
    operation_id = "apps_update",
    summary = "Replace an application",
    description = "Replaces an application on the version it was read on (`expected_version`, 409 `app-version-conflict` otherwise). Owner or admin.",
    params(("id" = String, Path, format = "uuid", description = "Application id")),
    request_body = AppInput,
    responses(
        (status = 200, body = AppView),
        (status = 400, description = INPUT_400, body = crate::openapi::ErrorDetail),
        (status = 404, description = "`app-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`app-version-conflict`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<AppInput>,
) -> Result<Response> {
    match apps::update(&ctx.db, org.org.id, id, &input).await {
        Ok(a) => format::json(a),
        Err(e) => app_error(e),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/apps/{id}",
    tag = "apps",
    operation_id = "apps_delete",
    summary = "Delete an application",
    description = "Removes the declaration and its screens; its service account and keys stay (delete them on the Access page). Owner or admin.",
    params(("id" = String, Path, format = "uuid", description = "Application id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "`app-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn remove(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match apps::delete(&ctx, org.org.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => app_error(e),
    }
}
