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
        .add("/profiles/{id}/generate", post(generate))
        .add(
            "/test",
            post(test).layer(DefaultBodyLimit::max(CLIP_MAX_BYTES)),
        )
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
        E::Invalid { field, token } => {
            return Ok((
                StatusCode::BAD_REQUEST,
                format::json(serde_json::json!({ field: token })),
            )
                .into_response())
        }
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

async fn list(State(ctx): State<AppContext>, org: OrgContext) -> Result<Response> {
    match profiles::list(&ctx.db, org.org.id).await {
        Ok(v) => format::json(v),
        Err(e) => fail(e),
    }
}

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

#[derive(Deserialize)]
struct ClipQuery {
    name: String,
    #[serde(default)]
    content_type: Option<String>,
    /// Span to enrol, in seconds into the recording (default: its start).
    #[serde(default)]
    start: u32,
    #[serde(default)]
    duration: Option<u32>,
}

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

#[derive(Deserialize)]
struct TestQuery {
    name: String,
    #[serde(default)]
    content_type: Option<String>,
    /// Verification against this person's profile.
    #[serde(default)]
    object_id: Option<Uuid>,
    /// Identification among the ready profiles of this model.
    #[serde(default)]
    model_id: Option<Uuid>,
}

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
