//! Voice identification API (voice-id.md, cases.md K19 variant A). Org
//! from the principal (R1); profiles, clips and generation are owner/admin
//! (consent and biometric data), a test needs write access, reads any
//! member (R2). No vector and no audio in any response (R4, R16).

use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::Json;
use loco_rs::controller::{format, ErrorDetail};
use loco_rs::prelude::*;
use pnex_core::err_codes;
use pnex_core::voice_id::{VoiceProfileInput, VoiceProfilePatch, CLIP_MAX_SECS};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::services::voice_id::{profiles, samples, testing, training, VoiceIdError};

/// Upload bound of one clip (a few minutes of compressed audio; only the
/// first `CLIP_MAX_SECS` are decoded).
const CLIP_MAX_BYTES: usize = 50 * 1024 * 1024;

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/voice-id")
        .add("/profiles", get(list).post(create))
        .add("/profiles/{id}", get(show).patch(update).delete(revoke))
        .add("/by-object/{object_id}", get(by_object))
        .add(
            "/profiles/{id}/samples",
            post(add_sample).layer(DefaultBodyLimit::max(CLIP_MAX_BYTES)),
        )
        .add("/profiles/{id}/samples/{sample_id}", delete(remove_sample))
        .add(
            "/profiles/{id}/samples/from-media",
            post(add_samples_from_media),
        )
        .add("/profiles/{id}/generate", post(generate))
        .add(
            "/test",
            post(test).layer(DefaultBodyLimit::max(CLIP_MAX_BYTES)),
        )
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(
        list,
        create,
        show,
        update,
        revoke,
        by_object,
        add_sample,
        add_samples_from_media,
        remove_sample,
        generate,
        test
    ))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

fn detail(status: StatusCode, code: &str, msg: &str) -> Error {
    Error::CustomError(status, ErrorDetail::new(code, msg.to_string()))
}

fn forbidden() -> Error {
    detail(
        StatusCode::FORBIDDEN,
        err_codes::VOICE_ID_WRITE_FORBIDDEN,
        "Voice profiles are written by owners and admins only; tests need write access.",
    )
}

fn require_admin(org: &OrgContext) -> Result<()> {
    if org.can_administer() {
        Ok(())
    } else {
        Err(forbidden())
    }
}

fn fail(e: VoiceIdError) -> Result<Response> {
    use VoiceIdError as E;
    Err(match e {
        E::Invalid { field, token } => return Ok(super::field_error(field, &token)),
        E::NotPerson => detail(
            StatusCode::BAD_REQUEST,
            err_codes::VOICE_ID_NOT_PERSON,
            "A voice profile attaches to a Person object only.",
        ),
        E::ProfileExists => detail(
            StatusCode::CONFLICT,
            err_codes::VOICE_ID_PROFILE_EXISTS,
            "This person already has a voice profile.",
        ),
        E::ProfileNotFound => detail(
            StatusCode::NOT_FOUND,
            err_codes::VOICE_ID_PROFILE_NOT_FOUND,
            "Voice profile not found.",
        ),
        E::SampleNotFound => detail(
            StatusCode::NOT_FOUND,
            err_codes::VOICE_ID_SAMPLE_NOT_FOUND,
            "Voice sample not found.",
        ),
        E::NoModel => detail(
            StatusCode::CONFLICT,
            err_codes::VOICE_ID_NO_MODEL,
            "No valid speaker embedding model in the organization.",
        ),
        E::AudioUnsupported => detail(
            StatusCode::BAD_REQUEST,
            err_codes::VOICE_ID_AUDIO_UNSUPPORTED,
            "The audio file cannot be read.",
        ),
        E::NotEnoughSpeech { found, min } => Error::CustomError(
            StatusCode::CONFLICT,
            ErrorDetail {
                error: Some(err_codes::VOICE_ID_NOT_ENOUGH_SPEECH.into()),
                description: Some("Not enough speech to build or test a profile.".into()),
                errors: Some(serde_json::json!({
                    "args": { "found": found.to_string(), "min_windows": min.to_string() }
                })),
            },
        ),
        E::NotReady => detail(
            StatusCode::CONFLICT,
            err_codes::VOICE_ID_NOT_READY,
            "No generated voice profile to compare with.",
        ),
        E::Failed(reason) => {
            tracing::warn!(%reason, "voice id embedding failed");
            detail(
                StatusCode::BAD_GATEWAY,
                err_codes::VOICE_ID_FAILED,
                "The speaker embedding could not be computed.",
            )
        }
        E::Db(reason) => {
            tracing::error!(%reason, "voice id database error");
            Error::InternalServerError
        }
    })
}

async fn respond(
    ctx: &AppContext,
    org: &OrgContext,
    status: StatusCode,
    row: crate::models::_entities::voice_profiles::Model,
) -> Result<Response> {
    match profiles::view(&ctx.db, org.org.id, row).await {
        Ok(v) => Ok((status, format::json(v)).into_response()),
        Err(e) => fail(e),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/voice-id/profiles",
    tag = "voice-id",
    operation_id = "voice_id_profiles_list",
    summary = "List the voice profiles of the organisation",
    description = "Paginated voice identification profiles (`limit` default 10, max 100; `offset`), biometric data, consent-based, with their consent record, status and clips. Never the voice vector nor audio.",
    params(PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::voice_id::VoiceProfileView>)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    use crate::controllers::pagination;
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    // shortcut: views are built for the whole org then sliced (a handful of
    // consented profiles per org); push LIMIT into the service if it grows.
    match profiles::list(&ctx.db, org.org.id).await {
        Ok(v) => {
            let count = v.len() as i64;
            let (skip, take) = page.slice(v.len());
            let rows: Vec<_> = v.into_iter().skip(skip).take(take).collect();
            format::json(pagination::envelope(
                "/api/v1/voice-id/profiles",
                &[],
                page,
                count,
                rows,
            ))
        }
        Err(e) => fail(e),
    }
}

/// Page parameters of the profiles list.
#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct PageQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/voice-id/profiles/{id}",
    tag = "voice-id",
    operation_id = "voice_id_profiles_get",
    summary = "Get a voice profile",
    description = "One voice profile with its clips; never the voice vector nor audio.",
    params(("id" = String, Path, format = "uuid", description = "Voice profile UUID")),
    responses(
        (status = 200, body = pnex_core::voice_id::VoiceProfileView),
        (status = 404, description = "Unknown voice profile (`voice-id-profile-not-found`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn show(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match profiles::find(&ctx.db, org.org.id, id).await {
        Ok(row) => respond(&ctx, &org, StatusCode::OK, row).await,
        Err(e) => fail(e),
    }
}

/// The profile of a person, `null` when it has none.
#[utoipa::path(
    get,
    path = "/api/v1/voice-id/by-object/{object_id}",
    tag = "voice-id",
    operation_id = "voice_id_profiles_get_by_object",
    summary = "Get the voice profile of a person",
    description = "The voice profile attached to a Person object, or `null` when the person has none.",
    params(("object_id" = String, Path, format = "uuid", description = "Person object UUID")),
    responses(
        (status = 200, body = Option<pnex_core::voice_id::VoiceProfileView>)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn by_object(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(object_id): Path<Uuid>,
) -> Result<Response> {
    match profiles::find_by_object(&ctx.db, org.org.id, object_id).await {
        Ok(Some(row)) => respond(&ctx, &org, StatusCode::OK, row).await,
        Ok(None) => format::json(serde_json::Value::Null),
        Err(e) => fail(e),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/voice-id/profiles",
    tag = "voice-id",
    operation_id = "voice_id_profiles_create",
    summary = "Create a voice profile for a person",
    description = "Creates a `draft` voice profile on a Person object with a speaker embedding model. The person's explicit consent (date, scope, proof) is mandatory and must be confirmed; the caller is recorded as having registered it. Owner/admin only.",
    request_body = pnex_core::voice_id::VoiceProfileInput,
    responses(
        (status = 201, body = pnex_core::voice_id::VoiceProfileView),
        (status = 400, description = "`field-invalid` (consent, model), or object not a Person (`voice-id-not-person`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Not owner/admin (`voice-id-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "The person already has a voice profile (`voice-id-profile-exists`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<VoiceProfileInput>,
) -> Result<Response> {
    require_admin(&org)?;
    match profiles::create(&ctx.db, org.org.id, &org.auth.user.email, &input).await {
        Ok(row) => respond(&ctx, &org, StatusCode::CREATED, row).await,
        Err(e) => fail(e),
    }
}

#[utoipa::path(
    patch,
    path = "/api/v1/voice-id/profiles/{id}",
    tag = "voice-id",
    operation_id = "voice_id_profiles_update",
    summary = "Change the embedding model of a voice profile",
    description = "Sets another speaker embedding model; the profile goes back to `draft` and must be generated again. Owner/admin only.",
    params(("id" = String, Path, format = "uuid", description = "Voice profile UUID")),
    request_body = pnex_core::voice_id::VoiceProfilePatch,
    responses(
        (status = 200, body = pnex_core::voice_id::VoiceProfileView),
        (status = 400, description = "`field-invalid` on `model_id`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Not owner/admin (`voice-id-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown voice profile (`voice-id-profile-not-found`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(patch): Json<VoiceProfilePatch>,
) -> Result<Response> {
    require_admin(&org)?;
    match profiles::set_model(&ctx.db, org.org.id, id, &patch.model_id).await {
        Ok(row) => respond(&ctx, &org, StatusCode::OK, row).await,
        Err(e) => fail(e),
    }
}

/// Consent revoked: profile, clips and their audio deleted.
#[utoipa::path(
    delete,
    path = "/api/v1/voice-id/profiles/{id}",
    tag = "voice-id",
    operation_id = "voice_id_profiles_delete",
    summary = "Delete a voice profile (consent revoked)",
    description = "Permanently deletes the profile, its clips and their stored audio. Owner/admin only.",
    params(("id" = String, Path, format = "uuid", description = "Voice profile UUID")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Not owner/admin (`voice-id-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown voice profile (`voice-id-profile-not-found`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn revoke(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    require_admin(&org)?;
    match profiles::delete(&ctx, org.org.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => fail(e),
    }
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ClipQuery {
    /// File name of the clip (its extension helps pick the decoder).
    name: String,
    /// Content type of the clip.
    #[serde(default)]
    content_type: Option<String>,
    /// Span to enrol, in seconds into the recording (default: its start).
    #[serde(default)]
    start: u32,
    /// Length to enrol, in seconds (clamped to the clip maximum).
    #[serde(default)]
    duration: Option<u32>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
struct FromMediaInput {
    /// Media asset whose confirmed passages of the person are enrolled.
    #[schema(value_type = String, format = "uuid")]
    asset_id: Uuid,
}

#[utoipa::path(
    post,
    path = "/api/v1/voice-id/profiles/{id}/samples/from-media",
    tag = "voice-id",
    operation_id = "voice_id_samples_from_media",
    summary = "Enrol the attributed passages of a media file",
    description = "Adds to the voice profile the passages of a media file attributed to the profile's person and confirmed (media review), the longest first, 180 s in all, decoded from the original file. The profile must exist (consent recorded at its creation). The clips are embedded at the next generation. Owner/admin only.",
    params(("id" = String, Path, format = "uuid", description = "Voice profile UUID")),
    request_body = FromMediaInput,
    responses(
        (status = 201, description = "The updated profile", body = pnex_core::voice_id::VoiceProfileView),
        (status = 400, description = "Unreadable audio (`voice-id-audio-unsupported`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Not owner/admin (`voice-id-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown voice profile or media (`voice-id-profile-not-found`, `voice-id-sample-not-found`)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "No confirmed passage of the person in the media (`voice-id-not-enough-speech`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn add_samples_from_media(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<FromMediaInput>,
) -> Result<Response> {
    require_admin(&org)?;
    match samples::add_from_media(&ctx, org.org.id, id, input.asset_id).await {
        Ok(_) => match profiles::find(&ctx.db, org.org.id, id).await {
            Ok(p) => respond(&ctx, &org, StatusCode::CREATED, p).await,
            Err(e) => fail(e),
        },
        Err(e) => fail(e),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/voice-id/profiles/{id}/samples",
    tag = "voice-id",
    operation_id = "voice_id_samples_create",
    summary = "Add an enrolment clip to a voice profile",
    description = "Raw audio bytes (max 50 MB) of a recording of the person; the span `start`..`start+duration` is enrolled. The clip is stored as an `audio` media asset and its speech measured now; it is embedded at the next generation. Owner/admin only.",
    params(("id" = String, Path, format = "uuid", description = "Voice profile UUID"), ClipQuery),
    request_body(content = Vec<u8>, content_type = "application/octet-stream", description = "Audio file bytes"),
    responses(
        (status = 201, description = "The updated profile", body = pnex_core::voice_id::VoiceProfileView),
        (status = 400, description = "Per-field error, or unreadable audio (`voice-id-audio-unsupported`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Not owner/admin (`voice-id-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown voice profile (`voice-id-profile-not-found`)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Not enough speech in the clip (`voice-id-not-enough-speech`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn add_sample(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<ClipQuery>,
    body: axum::body::Bytes,
) -> Result<Response> {
    require_admin(&org)?;
    let name = pnex_firmware_builder::sanitize_segment(q.name.trim());
    let ct = q.content_type.unwrap_or_default();
    let secs = q.duration.unwrap_or(CLIP_MAX_SECS).clamp(1, CLIP_MAX_SECS);
    match samples::add(&ctx, org.org.id, id, &name, &ct, body, (q.start, secs)).await {
        Ok(row) => match profiles::find(&ctx.db, org.org.id, row.profile_id).await {
            Ok(p) => respond(&ctx, &org, StatusCode::CREATED, p).await,
            Err(e) => fail(e),
        },
        Err(e) => fail(e),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/voice-id/profiles/{id}/samples/{sample_id}",
    tag = "voice-id",
    operation_id = "voice_id_samples_delete",
    summary = "Remove an enrolment clip from a voice profile",
    description = "Deletes the clip and its stored audio. Owner/admin only.",
    params(("id" = String, Path, format = "uuid", description = "Voice profile UUID"), ("sample_id" = String, Path, format = "uuid", description = "Voice sample UUID")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Not owner/admin (`voice-id-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown profile (`voice-id-profile-not-found`) or clip (`voice-id-sample-not-found`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn remove_sample(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, sample_id)): Path<(Uuid, Uuid)>,
) -> Result<Response> {
    require_admin(&org)?;
    match samples::remove(&ctx, org.org.id, id, sample_id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => fail(e),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/voice-id/profiles/{id}/generate",
    tag = "voice-id",
    operation_id = "voice_id_profiles_generate",
    summary = "Generate a voice profile from its clips",
    description = "Computes speaker embeddings of every clip, builds the encrypted voice centroid and its acceptance threshold, and marks each clip used or rejected. Synchronous and compute-heavy. Owner/admin only.",
    params(("id" = String, Path, format = "uuid", description = "Voice profile UUID")),
    responses(
        (status = 200, body = pnex_core::voice_id::VoiceProfileView),
        (status = 403, description = "Not owner/admin (`voice-id-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown voice profile (`voice-id-profile-not-found`)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "No valid speaker embedding model (`voice-id-no-model`) or not enough speech (`voice-id-not-enough-speech`, `errors.args.found`/`min_windows`)", body = crate::openapi::ErrorDetail),
        (status = 502, description = "Embedding computation failed (`voice-id-failed`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn generate(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    require_admin(&org)?;
    match training::generate(&ctx, org.org.id, id).await {
        Ok(row) => respond(&ctx, &org, StatusCode::OK, row).await,
        Err(e) => fail(e),
    }
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct TestQuery {
    /// File name of the clip (its extension helps pick the decoder).
    name: String,
    /// Content type of the clip.
    #[serde(default)]
    content_type: Option<String>,
    /// Verification against this person's profile.
    #[serde(default)]
    #[param(value_type = Option<String>, format = "uuid")]
    object_id: Option<Uuid>,
    /// Identification among the ready profiles of this model.
    #[serde(default)]
    #[param(value_type = Option<String>, format = "uuid")]
    model_id: Option<Uuid>,
}

#[utoipa::path(
    post,
    path = "/api/v1/voice-id/test",
    tag = "voice-id",
    operation_id = "voice_id_test",
    summary = "Test an audio clip against voice profiles",
    description = "Compares the uploaded audio (raw bytes, max 50 MB) with one person's profile (`object_id`, verification) or with every ready profile of a model (`model_id`, identification; default the newest valid model). The result is a proposal for a human, never written anywhere; the test itself is journaled. Writer role required.",
    params(TestQuery),
    request_body(content = Vec<u8>, content_type = "application/octet-stream", description = "Audio file bytes"),
    responses(
        (status = 200, body = pnex_core::voice_id::VoiceTestResult),
        (status = 400, description = "Unreadable audio (`voice-id-audio-unsupported`) or per-field error", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Viewer role (`voice-id-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Person has no voice profile (`voice-id-profile-not-found`)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "No valid model (`voice-id-no-model`), no generated profile to compare (`voice-id-not-ready`) or not enough speech (`voice-id-not-enough-speech`)", body = crate::openapi::ErrorDetail),
        (status = 502, description = "Embedding computation failed (`voice-id-failed`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn test(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<TestQuery>,
    body: axum::body::Bytes,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden());
    }
    let target = match q.object_id {
        Some(id) => testing::Target::Person(id),
        None => testing::Target::Model(q.model_id),
    };
    match testing::run(
        &ctx,
        org.org.id,
        org.auth.user.id,
        target,
        body.to_vec(),
        &q.name,
        q.content_type.as_deref(),
    )
    .await
    {
        Ok(r) => format::json(r),
        Err(e) => fail(e),
    }
}
