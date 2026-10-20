//! Site navigation rules API (site-recipes.md D323–D326): web sites of the
//! org, recipes anchored on them, test runs on the server browser and the
//! run journal with its screenshots and PDFs. Org from the principal (R1),
//! `site-recipes:*` checked by the IAM extractor (R2).

use std::collections::BTreeMap;

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::routing::{get, post};
use axum::Json;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use pnex_core::err_codes;
use pnex_core::site_recipe::{
    Recipe, RunInput, SiteRecipe, SiteRecipeDraft, SiteRecipeInput, SiteRecipeRun,
    SiteRecipeVersion, WebSite,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination::{self, PageParams};
use crate::services::site_recipes::endpoints::{self, EndpointError};
use crate::services::site_recipes::{self as recipes, browser, NewRun, RecipeError};
use pnex_core::site_recipe::{BrowserEndpoint, BrowserEndpointInput, BrowserTestResult};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/web-sites", get(sites))
        .add("/site-recipes", get(list).post(create))
        .add("/site-recipes/validate", post(validate))
        .add("/site-recipes/runs", get(runs).post(run_create))
        .add("/site-recipes/runs/{id}", get(run_get))
        .add("/site-recipes/runs/{id}/files/{file}", get(run_file))
        .add(
            "/site-recipes/{id}",
            get(get_one).put(update).delete(remove),
        )
        .add("/site-recipes/{id}/versions", get(versions_list))
        .add(
            "/site-recipes/{id}/versions/{version}/restore",
            post(versions_restore),
        )
        .add(
            "/browser-endpoints",
            get(endpoints_list).post(endpoints_create),
        )
        .add(
            "/browser-endpoints/{id}",
            axum::routing::put(endpoints_update).delete(endpoints_delete),
        )
        .add("/browser-endpoints/{id}/test", post(endpoints_test))
}

fn recipe_error(e: RecipeError) -> Result<Response> {
    match e {
        RecipeError::Invalid(v) => Ok(super::violations_error(err_codes::SITE_RECIPE_INVALID, v)),
        RecipeError::NameTaken => Ok(super::field_error("name", err_codes::FIELD_UNIQUE)),
        RecipeError::NotFound => Err(super::coded_error(
            StatusCode::NOT_FOUND,
            err_codes::SITE_RECIPE_NOT_FOUND,
            "Site recipe not found.",
            None,
        )),
        RecipeError::VersionConflict => Err(super::coded_error(
            StatusCode::CONFLICT,
            err_codes::SITE_RECIPE_VERSION_CONFLICT,
            "The recipe was saved by someone else since it was read.",
            None,
        )),
        RecipeError::Db(e) => db_error(e),
    }
}

fn db_error(e: sea_orm::DbErr) -> Result<Response> {
    tracing::error!(error = %e, "site recipes database error");
    Err(Error::InternalServerError)
}

fn run_not_found() -> Error {
    super::coded_error(
        StatusCode::NOT_FOUND,
        err_codes::SITE_RECIPE_RUN_NOT_FOUND,
        "Site recipe run not found.",
        None,
    )
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct PageQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/web-sites",
    tag = "site-recipes",
    operation_id = "web_sites_list",
    summary = "List web sites",
    description = "Web sites of the organization: one per normalised host carrying a navigation recipe, with the recipe count and the last run. Most recently run first. Any member.",
    params(PageQuery),
    responses((status = 200, body = pnex_core::Paginated<WebSite>)),
    security(("bearer" = [], "org" = []))
)]
async fn sites(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    match recipes::sites(&ctx.db, org.org.id, page.limit, page.offset).await {
        Ok((count, rows)) => format::json(pagination::envelope(
            "/api/v1/web-sites",
            &[],
            page,
            count,
            rows,
        )),
        Err(e) => db_error(e),
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ListQuery {
    /// Host or address of a site (any spelling: normalised).
    site: Option<String>,
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/site-recipes",
    tag = "site-recipes",
    operation_id = "site_recipes_list",
    summary = "List site recipes",
    description = "Navigation recipes of the organization, of one site with `site`. Each carries its YAML and the parsed recipe. Any member.",
    params(ListQuery),
    responses((status = 200, body = pnex_core::Paginated<SiteRecipe>)),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let site = q.site.as_deref().filter(|s| !s.trim().is_empty());
    let all = match recipes::list(&ctx.db, org.org.id, site).await {
        Ok(v) => v,
        Err(e) => return db_error(e),
    };
    let count = all.len() as i64;
    let results: Vec<SiteRecipe> = all
        .into_iter()
        .skip(page.offset as usize)
        .take(page.limit as usize)
        .collect();
    let filters: Vec<(String, String)> = site
        .map(|s| ("site".to_string(), s.to_string()))
        .into_iter()
        .collect();
    format::json(pagination::envelope(
        "/api/v1/site-recipes",
        &filters,
        page,
        count,
        results,
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/site-recipes/validate",
    tag = "site-recipes",
    operation_id = "site_recipes_validate",
    summary = "Check a site recipe",
    description = "Parses and checks a recipe draft without saving it; answers the parsed recipe (its step list). Any member.",
    request_body = SiteRecipeDraft,
    responses(
        (status = 200, body = Recipe),
        (status = 400, description = "`site-recipe-invalid`: `errors.violations` lists `{path, token}`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn validate(_org: OrgContext, Json(input): Json<SiteRecipeDraft>) -> Result<Response> {
    match recipes::parse(&input.yaml) {
        Ok(r) => format::json(r),
        Err(v) => recipe_error(RecipeError::Invalid(v)),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/site-recipes",
    tag = "site-recipes",
    operation_id = "site_recipes_create",
    summary = "Create a site recipe",
    description = "Saves a navigation recipe (YAML) on the site it names; version 1. Requires a writer role.",
    request_body = SiteRecipeInput,
    responses(
        (status = 201, body = SiteRecipe),
        (status = 400, description = "`site-recipe-invalid` (violations) or `field-invalid` (`name = unique` on the site)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<SiteRecipeInput>,
) -> Result<Response> {
    match recipes::create(&ctx.db, org.org.id, org.auth.user.id, &input.yaml).await {
        Ok(r) => Ok((StatusCode::CREATED, format::json(r)).into_response()),
        Err(e) => recipe_error(e),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/site-recipes/{id}",
    tag = "site-recipes",
    operation_id = "site_recipes_get",
    summary = "Get a site recipe",
    description = "Any member.",
    params(("id" = String, Path, format = "uuid", description = "Recipe id (UUID)")),
    responses(
        (status = 200, body = SiteRecipe),
        (status = 404, description = "`site-recipe-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn get_one(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match recipes::get(&ctx.db, org.org.id, id).await {
        Ok(r) => format::json(r),
        Err(e) => recipe_error(e),
    }
}

#[utoipa::path(
    put,
    path = "/api/v1/site-recipes/{id}",
    tag = "site-recipes",
    operation_id = "site_recipes_update",
    summary = "Replace a site recipe",
    description = "Replaces the YAML and bumps the version. With `expected_version`, a save made in between answers 409. Requires a writer role.",
    params(("id" = String, Path, format = "uuid", description = "Recipe id (UUID)")),
    request_body = SiteRecipeInput,
    responses(
        (status = 200, body = SiteRecipe),
        (status = 400, description = "`site-recipe-invalid` (violations) or `field-invalid` (`name = unique` on the site)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`site-recipe-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`site-recipe-version-conflict`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<SiteRecipeInput>,
) -> Result<Response> {
    match recipes::update(
        &ctx.db,
        org.org.id,
        org.auth.user.id,
        id,
        &input.yaml,
        input.expected_version,
    )
    .await
    {
        Ok(r) => format::json(r),
        Err(e) => recipe_error(e),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/site-recipes/{id}",
    tag = "site-recipes",
    operation_id = "site_recipes_delete",
    summary = "Delete a site recipe",
    description = "Deletes the recipe; its runs stay in the journal. Requires a writer role.",
    params(("id" = String, Path, format = "uuid", description = "Recipe id (UUID)")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "`site-recipe-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn remove(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match recipes::delete(&ctx.db, org.org.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => recipe_error(e),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/site-recipes/{id}/versions",
    tag = "site-recipes",
    operation_id = "site_recipes_versions_list",
    summary = "List the versions of a site recipe",
    description = "Every saved version of the recipe with its YAML and author, newest first. Any member.",
    params(
        ("id" = String, Path, format = "uuid", description = "Recipe id (UUID)"),
        PageQuery
    ),
    responses(
        (status = 200, body = pnex_core::Paginated<SiteRecipeVersion>),
        (status = 404, description = "`site-recipe-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn versions_list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let all = match recipes::versions(&ctx.db, org.org.id, id).await {
        Ok(v) => v,
        Err(e) => return recipe_error(e),
    };
    let count = all.len() as i64;
    let (skip, take) = page.slice(all.len());
    let results: Vec<SiteRecipeVersion> = all.into_iter().skip(skip).take(take).collect();
    format::json(pagination::envelope(
        &format!("/api/v1/site-recipes/{id}/versions"),
        &[],
        page,
        count,
        results,
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/site-recipes/{id}/versions/{version}/restore",
    tag = "site-recipes",
    operation_id = "site_recipes_versions_restore",
    summary = "Restore a version of a site recipe",
    description = "Makes an earlier version the active one: its YAML is saved again as a new version. Requires a writer role.",
    params(
        ("id" = String, Path, format = "uuid", description = "Recipe id (UUID)"),
        ("version" = i32, Path, description = "Version number to restore")
    ),
    responses(
        (status = 200, body = SiteRecipe),
        (status = 400, description = "`site-recipe-invalid`: the old version no longer passes the rules", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`site-recipe-not-found`: unknown recipe or version", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn versions_restore(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, version)): Path<(Uuid, i32)>,
) -> Result<Response> {
    match recipes::restore(&ctx.db, org.org.id, org.auth.user.id, id, version).await {
        Ok(r) => format::json(r),
        Err(e) => recipe_error(e),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/site-recipes/runs",
    tag = "site-recipes",
    operation_id = "site_recipes_runs_create",
    summary = "Play a site recipe",
    description = "Starts a run on the server browser: a saved recipe (`recipe_id`), or a draft (`yaml`, with `recipe_id` to file it under that recipe). Answers at once with the run, `running`; its steps fill in as they finish (poll the run). Visits the site from the server. Requires a writer role.",
    request_body = RunInput,
    responses(
        (status = 202, body = SiteRecipeRun),
        (status = 400, description = "`site-recipe-invalid` (violations), `site-recipe-param-missing` (`args.param`) or `field-invalid` (`recipe_id = required` with neither a recipe nor a draft)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`site-recipe-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn run_create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<RunInput>,
) -> Result<Response> {
    let recipe_id = match input.recipe_id.as_deref().filter(|s| !s.is_empty()) {
        None => None,
        Some(raw) => match Uuid::parse_str(raw) {
            Ok(id) => Some(id),
            Err(_) => {
                return Ok(super::field_error(
                    "recipe_id",
                    err_codes::FIELD_INVALID_UUID,
                ))
            }
        },
    };
    let saved = match recipe_id {
        Some(id) => match recipes::get(&ctx.db, org.org.id, id).await {
            Ok(r) => Some(r),
            Err(e) => return recipe_error(e),
        },
        None => None,
    };
    let (yaml, version) = match (&input.yaml, &saved) {
        (Some(y), _) => (y.clone(), None),
        (None, Some(r)) => (r.yaml.clone(), Some(r.version)),
        (None, None) => return Ok(super::field_error("recipe_id", err_codes::FIELD_REQUIRED)),
    };
    let recipe = match recipes::parse(&yaml) {
        Ok(r) => r,
        Err(v) => return recipe_error(RecipeError::Invalid(v)),
    };
    let params: BTreeMap<String, String> = input
        .params
        .into_iter()
        .filter(|(k, _)| recipe.params.contains(k))
        .map(|(k, v)| (k, v.chars().take(500).collect()))
        .collect();
    if let Some(missing) = recipe.params.iter().find(|p| !params.contains_key(*p)) {
        return Err(super::coded_error(
            StatusCode::BAD_REQUEST,
            err_codes::SITE_RECIPE_PARAM_MISSING,
            format!("A value is missing for the recipe parameter {missing}."),
            Some(serde_json::json!({ "param": missing })),
        ));
    }
    let new = NewRun {
        recipe_id,
        recipe_version: version,
        recipe: &recipe,
        yaml: &yaml,
        params: &params,
        trigger: "studio",
        user_id: Some(org.auth.user.id),
    };
    let run = match recipes::insert_run(&ctx.db, org.org.id, &new).await {
        Ok(r) => r,
        Err(e) => return db_error(e),
    };
    let run_id = Uuid::parse_str(&run.id).expect("uuid");
    browser::spawn(ctx.clone(), org.org.id, run_id, recipe, params);
    Ok((StatusCode::ACCEPTED, format::json(run)).into_response())
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct RunsQuery {
    /// Host or address of a site.
    site: Option<String>,
    /// Runs of one recipe.
    #[param(format = "uuid")]
    recipe_id: Option<String>,
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/site-recipes/runs",
    tag = "site-recipes",
    operation_id = "site_recipes_runs_list",
    summary = "List site recipe runs",
    description = "Run journal, newest first: of a site (`site`) or of a recipe (`recipe_id`). Any member.",
    params(RunsQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<SiteRecipeRun>),
        (status = 400, description = "`field-invalid`: `recipe_id = invalid-uuid`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn runs(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<RunsQuery>,
) -> Result<Response> {
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let mut filters = Vec::new();
    let recipe_id = match q.recipe_id.as_deref().filter(|s| !s.is_empty()) {
        None => None,
        Some(raw) => match Uuid::parse_str(raw) {
            Ok(id) => {
                filters.push(("recipe_id".to_string(), raw.to_string()));
                Some(id)
            }
            Err(_) => {
                return Ok(super::field_error(
                    "recipe_id",
                    err_codes::FIELD_INVALID_UUID,
                ))
            }
        },
    };
    let site = q.site.as_deref().filter(|s| !s.trim().is_empty());
    if let Some(s) = site {
        filters.push(("site".to_string(), s.to_string()));
    }
    match recipes::runs(
        &ctx.db,
        org.org.id,
        site,
        recipe_id,
        page.limit,
        page.offset,
    )
    .await
    {
        Ok((count, rows)) => format::json(pagination::envelope(
            "/api/v1/site-recipes/runs",
            &filters,
            page,
            count,
            rows,
        )),
        Err(e) => db_error(e),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/site-recipes/runs/{id}",
    tag = "site-recipes",
    operation_id = "site_recipes_runs_get",
    summary = "Get a site recipe run",
    description = "A run with its steps (status, reason, address, screenshot file), its extracts and, per extract, what changed since the previous finished run of the same recipe. Any member.",
    params(("id" = String, Path, format = "uuid", description = "Run id (UUID)")),
    responses(
        (status = 200, body = SiteRecipeRun),
        (status = 404, description = "`site-recipe-run-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn run_get(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match recipes::run(&ctx.db, org.org.id, id).await {
        Ok(r) => format::json(r),
        Err(RecipeError::NotFound) => Err(run_not_found()),
        Err(e) => recipe_error(e),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/site-recipes/runs/{id}/files/{file}",
    tag = "site-recipes",
    operation_id = "site_recipes_runs_file",
    summary = "Download a file of a run",
    description = "A step screenshot (`step-<n>.jpg`, JPEG) or a fetched PDF (`pdf-<n>.pdf`, always as an attachment). Any member.",
    params(
        ("id" = String, Path, format = "uuid", description = "Run id (UUID)"),
        ("file" = String, Path, description = "File name, from the run's steps or extracts")
    ),
    responses(
        (status = 200, content_type = "image/jpeg", body = Vec<u8>),
        (status = 404, description = "`site-recipe-run-not-found`: unknown run or file", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn run_file(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, file)): Path<(Uuid, String)>,
) -> Result<Response> {
    let key = match recipes::file_key(&ctx.db, org.org.id, id, &file).await {
        Ok(Some(k)) => k,
        Ok(None) | Err(RecipeError::NotFound) => return Err(run_not_found()),
        Err(e) => return recipe_error(e),
    };
    let store = crate::services::artifact_store::S3Config::from_config(&ctx.config)
        .store()
        .map_err(|_| Error::InternalServerError)?;
    let Some(bytes) = store
        .get_bytes(&key)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(run_not_found());
    };
    let pdf = file.ends_with(".pdf");
    let mut res = (
        [
            (
                header::CONTENT_TYPE,
                if pdf { "application/pdf" } else { "image/jpeg" },
            ),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::CACHE_CONTROL, "private, max-age=86400"),
        ],
        bytes,
    )
        .into_response();
    if pdf {
        // A site's PDF is never rendered on the app origin (R11–R13).
        res.headers_mut().insert(
            header::CONTENT_DISPOSITION,
            header::HeaderValue::from_str(&format!("attachment; filename=\"{file}\""))
                .map_err(|_| Error::InternalServerError)?,
        );
    }
    Ok(res)
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(
        sites,
        list,
        validate,
        create,
        get_one,
        update,
        remove,
        versions_list,
        versions_restore,
        run_create,
        runs,
        run_get,
        run_file,
        endpoints_list,
        endpoints_create,
        endpoints_update,
        endpoints_delete,
        endpoints_test
    ))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

// ───────────────────────────── browser endpoints ─────────────────────────────

fn endpoint_error(e: EndpointError) -> Result<Response> {
    match e {
        EndpointError::Invalid { field, token } => Ok(super::field_error(field, &token)),
        EndpointError::NotFound => Err(super::coded_error(
            StatusCode::NOT_FOUND,
            err_codes::BROWSER_ENDPOINT_NOT_FOUND,
            "Browser endpoint not found.",
            None,
        )),
        EndpointError::NameTaken => Err(super::coded_error(
            StatusCode::CONFLICT,
            err_codes::BROWSER_ENDPOINT_NAME_TAKEN,
            "A browser endpoint with this name already exists.",
            None,
        )),
        EndpointError::Store(e) => crate::controllers::secrets::store_error(e),
    }
}

fn author(org: &OrgContext) -> endpoints::Author {
    endpoints::Author {
        user_id: Some(org.auth.user.id),
        can_write_secrets: org.can("secrets:write"),
    }
}

async fn endpoint_view(
    ctx: &AppContext,
    org_id: i64,
    row: &endpoints::Row,
    status: StatusCode,
) -> Result<Response> {
    match endpoints::views(&ctx.db, org_id, std::slice::from_ref(row)).await {
        Ok(mut v) if !v.is_empty() => Ok((status, format::json(v.remove(0))).into_response()),
        Ok(_) => Err(Error::InternalServerError),
        Err(e) => crate::controllers::secrets::store_error(e),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/browser-endpoints",
    tag = "site-recipes",
    operation_id = "browser_endpoints_list",
    summary = "List browser endpoints",
    description = "CDP browsers the organization plays its navigation rules on, with the default. The token is a vault reference, never its value. Without any, runs use the platform browser. Any member.",
    params(PageQuery),
    responses((status = 200, body = pnex_core::Paginated<BrowserEndpoint>)),
    security(("bearer" = [], "org" = []))
)]
async fn endpoints_list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let rows = match endpoints::list(&ctx.db, org.org.id).await {
        Ok(r) => r,
        Err(e) => return db_error(e),
    };
    let views = match endpoints::views(&ctx.db, org.org.id, &rows).await {
        Ok(v) => v,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    let count = views.len() as i64;
    let (skip, take) = page.slice(views.len());
    let results: Vec<BrowserEndpoint> = views.into_iter().skip(skip).take(take).collect();
    format::json(pagination::envelope(
        "/api/v1/browser-endpoints",
        &[],
        page,
        count,
        results,
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/browser-endpoints",
    tag = "site-recipes",
    operation_id = "browser_endpoints_create",
    summary = "Create a browser endpoint",
    description = "Registers a CDP browser (`wss://…`, `ws://…` or `http(s)://host:port`). The address must pass the egress policy (no internal host). A typed `token` value is stored in the org vault and sent as `Authorization: Bearer`. The first endpoint, or one with `is_default`, becomes the org default. Owner or admin only.",
    request_body = BrowserEndpointInput,
    responses(
        (status = 201, body = BrowserEndpoint),
        (status = 400, description = "`field-invalid`: `errors.fields` maps the field to a token", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`browser-endpoint-name-taken`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn endpoints_create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<BrowserEndpointInput>,
) -> Result<Response> {
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(r) => r,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    match endpoints::create(&ctx.db, &ring, org.org.id, author(&org), &input).await {
        Ok(row) => endpoint_view(&ctx, org.org.id, &row, StatusCode::CREATED).await,
        Err(e) => endpoint_error(e),
    }
}

#[utoipa::path(
    put,
    path = "/api/v1/browser-endpoints/{id}",
    tag = "site-recipes",
    operation_id = "browser_endpoints_update",
    summary = "Replace a browser endpoint",
    description = "Full replacement; an absent `token` keeps the current one. Owner or admin only.",
    params(("id" = String, Path, format = "uuid", description = "Browser endpoint id")),
    request_body = BrowserEndpointInput,
    responses(
        (status = 200, body = BrowserEndpoint),
        (status = 400, description = "`field-invalid`: `errors.fields` maps the field to a token", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`browser-endpoint-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`browser-endpoint-name-taken`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn endpoints_update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<BrowserEndpointInput>,
) -> Result<Response> {
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(r) => r,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    match endpoints::update(&ctx.db, &ring, org.org.id, author(&org), id, &input).await {
        Ok(row) => endpoint_view(&ctx, org.org.id, &row, StatusCode::OK).await,
        Err(e) => endpoint_error(e),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/browser-endpoints/{id}",
    tag = "site-recipes",
    operation_id = "browser_endpoints_delete",
    summary = "Delete a browser endpoint",
    description = "Deletes the endpoint and its dedicated token. Rules that named it fail until edited. Owner or admin only.",
    params(("id" = String, Path, format = "uuid", description = "Browser endpoint id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "`browser-endpoint-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn endpoints_delete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match endpoints::delete(&ctx.db, org.org.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => endpoint_error(e),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/browser-endpoints/{id}/test",
    tag = "site-recipes",
    operation_id = "browser_endpoints_test",
    summary = "Test a browser endpoint",
    description = "From the server: reads `/json/version` with the token, opens a fresh browser context, loads a public page with a screenshot, and checks that an internal address is refused. Answers each check, the engine and the latency; nothing is stored. Owner or admin only.",
    params(("id" = String, Path, format = "uuid", description = "Browser endpoint id")),
    responses(
        (status = 200, body = BrowserTestResult),
        (status = 404, description = "`browser-endpoint-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn endpoints_test(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let row = match endpoints::find(&ctx.db, org.org.id, id).await {
        Ok(r) => r,
        Err(e) => return endpoint_error(e),
    };
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(r) => r,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    let target = match endpoints::resolve(&ctx.db, Some(&ring), org.org.id, Some(&row.name)).await {
        Ok(t) => t,
        Err(detail) => {
            return format::json(BrowserTestResult {
                ok: false,
                engine: None,
                protocol: None,
                latency_ms: 0,
                checks: vec![pnex_core::site_recipe::BrowserCheck {
                    check: "version".into(),
                    ok: false,
                    detail: Some(detail),
                    ms: 0,
                }],
            })
        }
    };
    format::json(endpoints::test(&target).await)
}
