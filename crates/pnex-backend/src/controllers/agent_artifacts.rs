//! Artifacts of agent runs (agent-environments.md D729, D744). Org routes take the org from
//! the principal (R1), permissions applied by `OrgContext` (R2); the content is downloaded
//! as an attachment under its own permission, never shown on the origin of the app (R11).
//! The transfer routes take the artifact token of a run, valid only while its job runs.

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::routing::{get, post, put};
use axum::Json;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use pnex_core::agent_runs::{
    artifact_name_is_valid, AgentArtifact, ArtifactForm, ARTIFACT_FORM_HEADER,
};
use pnex_core::err_codes;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination::{self, PageParams};
use crate::services::agent_artifacts::{self, ArtifactError, ArtifactSettings, TransferPrincipal};
use crate::services::artifact_store::{S3Config, S3Store};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        // Raw body streams: no body limit layer applies, the settings hold the bound.
        .add("/agent-artifacts", post(upload))
        .add("/agent-artifacts/{id}", get(one))
        .add("/agent-artifacts/{id}/content", get(content))
        .add("/agent-runs/{id}/artifacts", get(of_run))
        .add("/agent-transfer/inputs/{name}", get(transfer_input))
        .add("/agent-transfer/outputs/{name}", put(transfer_output))
        .add(
            "/agent-transfer/caches/{name}",
            get(transfer_cache_get).put(transfer_cache_put),
        )
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(
        paths(
            upload,
            one,
            content,
            of_run,
            transfer_input,
            transfer_output,
            transfer_cache_get,
            transfer_cache_put
        ),
        components(schemas(
            AgentArtifact,
            pnex_core::agent_runs::ArtifactState,
            pnex_core::agent_runs::ArtifactForm,
            pnex_core::agent_runs::ArtifactInput,
            pnex_core::agent_runs::ArtifactOutput,
            pnex_core::runners::SandboxArtifacts,
            pnex_core::runners::SandboxArtifact,
            pnex_core::agent_runs::RunCache
        ))
    )]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

fn artifact_error(e: ArtifactError) -> Result<Response> {
    let (status, code, message) = match e {
        ArtifactError::NotFound => (
            StatusCode::NOT_FOUND,
            err_codes::AGENT_ARTIFACT_NOT_FOUND,
            "Artifact not found.",
        ),
        ArtifactError::NotDeclared => (
            StatusCode::NOT_FOUND,
            err_codes::AGENT_ARTIFACT_NOT_DECLARED,
            "The run declared no artifact of this name.",
        ),
        ArtifactError::AlreadySent => (
            StatusCode::CONFLICT,
            err_codes::AGENT_ARTIFACT_ALREADY_SENT,
            "This output was already sent.",
        ),
        ArtifactError::TooLarge => (
            StatusCode::PAYLOAD_TOO_LARGE,
            err_codes::AGENT_ARTIFACT_TOO_LARGE,
            "The artifact is over its limit: refused whole.",
        ),
        ArtifactError::Interrupted => (
            StatusCode::BAD_REQUEST,
            err_codes::AGENT_ARTIFACT_INTERRUPTED,
            "The body stopped before its end.",
        ),
        ArtifactError::Storage(e) => {
            tracing::warn!(error = %e, "artifact storage error");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                err_codes::AGENT_ARTIFACT_STORAGE_UNAVAILABLE,
                "The store of artifacts is not available.",
            )
        }
        ArtifactError::Db(e) => {
            tracing::error!(error = %e, "agent artifact database error");
            return Err(Error::InternalServerError);
        }
    };
    Err(super::coded_error(status, code, message, None))
}

fn store(ctx: &AppContext) -> std::result::Result<S3Store, ArtifactError> {
    S3Config::from_config(&ctx.config)
        .store()
        .map_err(ArtifactError::Storage)
}

fn form_of(raw: Option<&str>) -> Option<ArtifactForm> {
    match raw {
        None => Some(ArtifactForm::File),
        Some(f) => ArtifactForm::parse(f),
    }
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct UploadQuery {
    /// `[a-z0-9-]`, the name runs give it as an input.
    name: String,
    /// `file` (default) or `tar` for a folder.
    form: Option<String>,
}

/// `POST /api/v1/agent-artifacts`
#[utoipa::path(
    post,
    path = "/api/v1/agent-artifacts",
    tag = "agent-runs",
    operation_id = "agent_artifacts_upload",
    summary = "Upload a file for runs",
    description = "Streams the body into the store of the org as an artifact, with its SHA-256 and an expiry; runs then take it as an input by id. Over the size limit it is refused whole. Requires `agents:run`.",
    params(UploadQuery),
    request_body(content = String, content_type = "application/octet-stream"),
    responses(
        (status = 201, body = AgentArtifact),
        (status = 400, description = "`field-invalid` (`errors.fields`), `agent-artifact-interrupted`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 413, description = "`agent-artifact-too-large`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`agent-artifact-storage-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn upload(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<UploadQuery>,
    body: Body,
) -> Result<Response> {
    if !artifact_name_is_valid(&q.name) {
        return Ok(super::field_error("name", "invalid"));
    }
    let Some(form) = form_of(q.form.as_deref()) else {
        return Ok(super::field_error("form", "invalid"));
    };
    let store = match store(&ctx) {
        Ok(s) => s,
        Err(e) => return artifact_error(e),
    };
    let settings = ArtifactSettings::from_config(&ctx.config);
    match agent_artifacts::upload(
        &ctx.db,
        &store,
        settings,
        org.org.id,
        org.auth.user.id,
        &q.name,
        form,
        body.into_data_stream(),
    )
    .await
    {
        Ok(a) => Ok((StatusCode::CREATED, Json(a)).into_response()),
        Err(e) => artifact_error(e),
    }
}

/// `GET /api/v1/agent-artifacts/{id}`
#[utoipa::path(
    get,
    path = "/api/v1/agent-artifacts/{id}",
    tag = "agent-runs",
    operation_id = "agent_artifacts_get",
    summary = "Read an artifact",
    description = "Its name, state, checksum, size and expiry; never its content. Requires `agents:read`.",
    params(("id" = String, Path, format = "uuid", description = "Artifact id (UUID)")),
    responses(
        (status = 200, body = AgentArtifact),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`agent-artifact-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn one(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match agent_artifacts::get(&ctx.db, org.org.id, id).await {
        Ok(a) => format::json(a),
        Err(e) => artifact_error(e),
    }
}

/// `GET /api/v1/agent-artifacts/{id}/content`
#[utoipa::path(
    get,
    path = "/api/v1/agent-artifacts/{id}/content",
    tag = "agent-runs",
    operation_id = "agent_artifacts_content",
    summary = "Download an artifact",
    description = "The bytes of a ready artifact, as an attachment: what an agent wrote is never shown by the app. It may hold a secret, hence its own permission `agent-artifacts:read`.",
    params(("id" = String, Path, format = "uuid", description = "Artifact id (UUID)")),
    responses(
        (status = 200, description = "The bytes", content_type = "application/octet-stream"),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`agent-artifact-not-found`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`agent-artifact-storage-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn content(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let artifact = match agent_artifacts::get(&ctx.db, org.org.id, id).await {
        Ok(a) if a.state == pnex_core::agent_runs::ArtifactState::Ready => a,
        Ok(_) => return artifact_error(ArtifactError::NotFound),
        Err(e) => return artifact_error(e),
    };
    let key = agent_artifacts::storage_key(
        org.org.id,
        artifact
            .run_id
            .as_deref()
            .and_then(|r| Uuid::parse_str(r).ok()),
        id,
    );
    let store = match store(&ctx) {
        Ok(s) => s,
        Err(e) => return artifact_error(e),
    };
    let size = artifact.size.unwrap_or(0).max(0) as u64;
    let file = match artifact.form {
        Some(ArtifactForm::Tar) => format!("{}.tar", artifact.name),
        _ => artifact.name.clone(),
    };
    stream_response(store, key, size, Some(&file))
}

fn stream_response(
    store: S3Store,
    key: String,
    size: u64,
    attachment: Option<&str>,
) -> Result<Response> {
    let mut res = Body::from_stream(agent_artifacts::read_stream(store, key, size)).into_response();
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(size));
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if let Some(file) = attachment {
        // The name is `[a-z0-9-]` (and `.tar`): nothing to escape.
        if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{file}\"")) {
            h.insert(header::CONTENT_DISPOSITION, v);
        }
    }
    Ok(res)
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct PageQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// `GET /api/v1/agent-runs/{id}/artifacts`
#[utoipa::path(
    get,
    path = "/api/v1/agent-runs/{id}/artifacts",
    tag = "agent-runs",
    operation_id = "agent_runs_artifacts",
    summary = "List the outputs of a run",
    description = "Each declared output with its state: pending, ready, refused (over a limit) or missing. Requires `agents:read`. Paginated (`limit`/`offset`).",
    params(("id" = String, Path, format = "uuid", description = "Run id (UUID)"), PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<AgentArtifact>),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`agent-run-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn of_run(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    if crate::services::agent_runs::get(&ctx.db, org.org.id, id)
        .await
        .is_err()
    {
        return Err(super::coded_error(
            StatusCode::NOT_FOUND,
            err_codes::AGENT_RUN_NOT_FOUND,
            "Run not found.",
            None,
        ));
    }
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let (count, rows) = agent_artifacts::of_run(&ctx.db, org.org.id, id, page.limit, page.offset)
        .await
        .map_err(Error::DB)?;
    format::json(pagination::envelope(
        &format!("/api/v1/agent-runs/{id}/artifacts"),
        &[],
        page,
        count,
        rows,
    ))
}

/// The run of the artifact token in `Authorization: Bearer`.
async fn transfer_of(ctx: &AppContext, headers: &HeaderMap) -> Result<TransferPrincipal> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let found = match token {
        Some(t) => agent_artifacts::authenticate(&ctx.db, t.trim())
            .await
            .map_err(Error::DB)?,
        None => None,
    };
    found.ok_or_else(|| {
        super::coded_error(
            StatusCode::UNAUTHORIZED,
            err_codes::AGENT_RUN_UNAUTHORIZED,
            "Unknown artifact token, or the run is not running.",
            None,
        )
    })
}

/// `GET /api/v1/agent-transfer/inputs/{name}`
#[utoipa::path(
    get,
    path = "/api/v1/agent-transfer/inputs/{name}",
    tag = "agent-runs",
    operation_id = "agent_transfer_input",
    summary = "Fetch an input of the running run",
    description = "Called by the transfer container of a sandbox with the artifact token of its run, never by the agent. Only the inputs this run declared.",
    params(("name" = String, Path, description = "Declared input name")),
    responses(
        (status = 200, description = "The bytes", content_type = "application/octet-stream"),
        (status = 401, description = "`agent-run-unauthorized`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`agent-artifact-not-declared`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`agent-artifact-storage-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = []))
)]
async fn transfer_input(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Response> {
    let run = transfer_of(&ctx, &headers).await?;
    // The skills of its agent are read from PneX, not from the store (D711).
    if name == pnex_core::agent_skills::SKILLS_INPUT {
        let bundle = crate::services::agent_skills::bundle_of(&ctx.db, run.org_id, run.run_id)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "skills of a run unreadable");
                Error::InternalServerError
            })?;
        if let Some(bytes) = bundle {
            let kind = (header::CONTENT_TYPE, "application/octet-stream");
            return Ok(([kind, (header::X_CONTENT_TYPE_OPTIONS, "nosniff")], bytes).into_response());
        }
    }
    let (key, size) = match agent_artifacts::input_of(&ctx.db, run, &name).await {
        Ok(found) => found,
        Err(e) => return artifact_error(e),
    };
    let store = match store(&ctx) {
        Ok(s) => s,
        Err(e) => return artifact_error(e),
    };
    stream_response(store, key, size, None)
}

/// `PUT /api/v1/agent-transfer/outputs/{name}`
#[utoipa::path(
    put,
    path = "/api/v1/agent-transfer/outputs/{name}",
    tag = "agent-runs",
    operation_id = "agent_transfer_output",
    summary = "Send an output of the running run",
    description = "Called by the collector of a sandbox with the artifact token of its run, once per declared output; `x-pnex-artifact-form` says `file` or `tar`. Over the limits of an artifact or of the run, it is refused whole and marked so on the run.",
    params(("name" = String, Path, description = "Declared output name")),
    request_body(content = String, content_type = "application/octet-stream"),
    responses(
        (status = 200, body = AgentArtifact),
        (status = 400, description = "`field-invalid` on the form, `agent-artifact-interrupted`", body = crate::openapi::ErrorDetail),
        (status = 401, description = "`agent-run-unauthorized`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`agent-artifact-not-declared`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`agent-artifact-already-sent`", body = crate::openapi::ErrorDetail),
        (status = 413, description = "`agent-artifact-too-large`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`agent-artifact-storage-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = []))
)]
async fn transfer_output(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(name): Path<String>,
    body: Body,
) -> Result<Response> {
    let run = transfer_of(&ctx, &headers).await?;
    let raw = headers
        .get(ARTIFACT_FORM_HEADER)
        .and_then(|v| v.to_str().ok());
    let Some(form) = form_of(raw) else {
        return Ok(super::field_error("form", "invalid"));
    };
    let store = match store(&ctx) {
        Ok(s) => s,
        Err(e) => return artifact_error(e),
    };
    let settings = ArtifactSettings::from_config(&ctx.config);
    match agent_artifacts::receive_output(
        &ctx.db,
        &store,
        settings,
        run,
        &name,
        form,
        body.into_data_stream(),
    )
    .await
    {
        Ok(a) => format::json(a),
        Err(e) => artifact_error(e),
    }
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct CacheQuery {
    /// `none`, or 32 hex characters: the fingerprint of the cache's lock files.
    key: String,
}

/// `GET /api/v1/agent-transfer/caches/{name}`
#[utoipa::path(
    get,
    path = "/api/v1/agent-transfer/caches/{name}",
    tag = "agent-runs",
    operation_id = "agent_transfer_cache_get",
    summary = "Fetch a dependency cache of the running run",
    description = "Called by the transfer container of a sandbox with the artifact token of its run, for a cache it declared (D730). The archive of this key, or else the latest of this cache; `x-pnex-cache-exact` says which. 404 before the first save.",
    params(("name" = String, Path, description = "Declared cache name"), CacheQuery),
    responses(
        (status = 200, description = "The archive", content_type = "application/octet-stream"),
        (status = 400, description = "`field-invalid` on the key", body = crate::openapi::ErrorDetail),
        (status = 401, description = "`agent-run-unauthorized`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`agent-artifact-not-declared`, `agent-artifact-not-found` (no archive yet)", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`agent-artifact-storage-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = []))
)]
async fn transfer_cache_get(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Query(q): Query<CacheQuery>,
) -> Result<Response> {
    let run = transfer_of(&ctx, &headers).await?;
    if !agent_artifacts::cache_key_is_valid(&q.key) {
        return Ok(super::field_error("key", "invalid"));
    }
    let environment = match agent_artifacts::cache_of(&ctx.db, run, &name).await {
        Ok(e) => e,
        Err(e) => return artifact_error(e),
    };
    let store = match store(&ctx) {
        Ok(s) => s,
        Err(e) => return artifact_error(e),
    };
    let prefix = agent_artifacts::cache_prefix(run.org_id, environment, &name);
    let found = match agent_artifacts::cache_archive(&store, &prefix, &q.key).await {
        Ok(Some(found)) => found,
        Ok(None) => return artifact_error(ArtifactError::NotFound),
        Err(e) => return artifact_error(e),
    };
    let (key, size, exact) = found;
    let mut res = stream_response(store, key, size, None)?;
    res.headers_mut().insert(
        "x-pnex-cache-exact",
        HeaderValue::from_static(if exact { "true" } else { "false" }),
    );
    Ok(res)
}

/// `PUT /api/v1/agent-transfer/caches/{name}`
#[utoipa::path(
    put,
    path = "/api/v1/agent-transfer/caches/{name}",
    tag = "agent-runs",
    operation_id = "agent_transfer_cache_put",
    summary = "Save a dependency cache of the running run",
    description = "Called by the collector of a sandbox with the artifact token of its run, for a cache it declared: the archive replaces the one of this key. Over the cache limit it is refused whole.",
    params(("name" = String, Path, description = "Declared cache name"), CacheQuery),
    request_body(content = String, content_type = "application/octet-stream"),
    responses(
        (status = 204, description = "Saved"),
        (status = 400, description = "`field-invalid` on the key, `agent-artifact-interrupted`", body = crate::openapi::ErrorDetail),
        (status = 401, description = "`agent-run-unauthorized`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`agent-artifact-not-declared`", body = crate::openapi::ErrorDetail),
        (status = 413, description = "`agent-artifact-too-large`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`agent-artifact-storage-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = []))
)]
async fn transfer_cache_put(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Query(q): Query<CacheQuery>,
    body: Body,
) -> Result<Response> {
    let run = transfer_of(&ctx, &headers).await?;
    if !agent_artifacts::cache_key_is_valid(&q.key) {
        return Ok(super::field_error("key", "invalid"));
    }
    let environment = match agent_artifacts::cache_of(&ctx.db, run, &name).await {
        Ok(e) => e,
        Err(e) => return artifact_error(e),
    };
    let store = match store(&ctx) {
        Ok(s) => s,
        Err(e) => return artifact_error(e),
    };
    let prefix = agent_artifacts::cache_prefix(run.org_id, environment, &name);
    let settings = agent_artifacts::CacheSettings::from_config(&ctx.config);
    match agent_artifacts::save_cache(&store, settings, &prefix, &q.key, body.into_data_stream())
        .await
    {
        Ok(_) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => artifact_error(e),
    }
}
