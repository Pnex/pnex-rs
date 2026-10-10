//! Media streams (media-ingest.md D159): CRUD service shared by the HTTP
//! controller. The capture supervisor reads the rows, it is never driven
//! by a request.

use pnex_core::egress;
use pnex_core::err_codes;
use pnex_core::media_ingest::{
    stream_slug, url_has_credentials, AudioRetention, CaptureOn, CaptureState, MediaStream,
    MediaStreamInput, MediaStreamKind, MediaTracks, FPS_DEFAULT, FPS_MAX, FPS_MIN,
    OVERLAP_SECS_DEFAULT, OVERLAP_SECS_MAX, SEGMENT_SECS_DEFAULT, SEGMENT_SECS_MAX,
    SEGMENT_SECS_MIN, SLUG_MAX_LEN, URL_MAX_LEN,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder, Select, Set, TransactionTrait,
};
use uuid::Uuid;

use crate::models::_entities::{
    asr_profiles, media_segments, media_streams, ml_models, notify_channels,
};
use crate::services::db_lock;
use crate::services::secrets::store::{self, StoreError, Writer};
use crate::services::secrets::{media as media_secret, Keyring};

const NAME_MAX: usize = 200;
const TIMEZONE_MAX: usize = 64;
const TDM_NOTE_MAX: usize = 2000;

#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    #[error("invalid {field}: {token}")]
    Invalid { field: &'static str, token: String },
    #[error("media stream not found")]
    NotFound,
    #[error("media stream quota reached")]
    Limit,
    #[error("stream URL carries a credential")]
    UrlHasCredentials,
    #[error("stream unreachable")]
    Unreachable,
    #[error("capture carrier not supported yet")]
    CaptureUnsupported,
    #[error("ASR model invalid")]
    AsrModelInvalid,
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<DbErr> for StreamError {
    fn from(e: DbErr) -> Self {
        Self::Store(StoreError::Db(e))
    }
}

fn invalid(field: &'static str, token: impl Into<String>) -> StreamError {
    StreamError::Invalid {
        field,
        token: token.into(),
    }
}

/// Who writes.
#[derive(Clone, Copy, Debug)]
pub struct Author {
    pub user_id: Option<i64>,
    /// Owner/admin: may bind a secret, or move a secret-holding stream to
    /// another origin (R9).
    pub can_manage_secrets: bool,
}

/// Read models of `org_id`'s streams (secret = reference, never a value).
pub async fn views<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    rows: &[media_streams::Model],
) -> Result<Vec<MediaStream>, StoreError> {
    let ids: Vec<Uuid> = rows.iter().filter_map(|r| r.secret_id).collect();
    let names = store::names_of(db, Some(org_id), &ids).await?;
    let now = chrono::Utc::now();
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let mut v = view(r);
        v.auth_secret = r.secret_id.and_then(|id| {
            Some(pnex_core::SecretFieldView {
                secret_id: id,
                name: names.get(&id)?.clone(),
            })
        });
        if r.enabled {
            v.health = super::health::sample(db, r, now).await.ok().map(|h| {
                pnex_core::media_ingest::StreamHealth {
                    gap_secs: h.gap_secs,
                    lag_secs: h.lag_secs,
                    coverage: h.coverage,
                }
            });
        }
        out.push(v);
    }
    Ok(out)
}

pub fn view(r: &media_streams::Model) -> MediaStream {
    MediaStream {
        id: r.id.to_string(),
        name: r.name.clone(),
        slug: r.slug.clone(),
        kind: MediaStreamKind::from_wire(&r.kind).unwrap_or_default(),
        url: r.url.clone(),
        has_secret: r.secret_id.is_some(),
        auth_secret: None,
        enabled: r.enabled,
        capture_on: r.capture_on.clone(),
        asr_profile_id: r.asr_profile_id.map(|v| v.to_string()),
        tracks: MediaTracks::from_wire(&r.tracks).unwrap_or_default(),
        fps: r.fps,
        segment_secs: r.segment_secs,
        overlap_secs: r.overlap_secs,
        audio_retention: r.audio_retention.clone(),
        notify_channel_id: r.notify_channel_id.map(|v| v.to_string()),
        timezone: r.timezone.clone(),
        tdm_checked_at: r.tdm_checked_at.map(|t| t.to_rfc3339()),
        tdm_note: r.tdm_note.clone().unwrap_or_default(),
        capture_state: CaptureState::from_wire(&r.capture_state).unwrap_or_default(),
        capture_error: r.capture_error.clone(),
        health: None,
        created_at: r.created_at.to_rfc3339(),
        updated_at: r.updated_at.to_rfc3339(),
    }
}

/// Live streams of the org (tombstones excluded), by name.
pub fn select(org_id: i64) -> Select<media_streams::Entity> {
    media_streams::Entity::find()
        .filter(media_streams::Column::OrgId.eq(org_id))
        .filter(media_streams::Column::DeletedAt.is_null())
        .order_by_asc(media_streams::Column::Name)
}

pub async fn find<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    id: Uuid,
) -> Result<media_streams::Model, StreamError> {
    media_streams::Entity::find_by_id(id)
        .filter(media_streams::Column::OrgId.eq(org_id))
        .filter(media_streams::Column::DeletedAt.is_null())
        .one(db)
        .await?
        .ok_or(StreamError::NotFound)
}

/// Stream of the org by slug (deploy checks of `media_source`, transcript
/// search): the slug is resolved, never used as a name as received.
pub async fn find_by_slug<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    slug: &str,
) -> Result<Option<media_streams::Model>, DbErr> {
    media_streams::Entity::find()
        .filter(media_streams::Column::OrgId.eq(org_id))
        .filter(media_streams::Column::Slug.eq(slug))
        .filter(media_streams::Column::DeletedAt.is_null())
        .one(db)
        .await
}

/// A slug ever given in the org, tombstones included: never reused.
async fn slug_taken<C: ConnectionTrait>(db: &C, org_id: i64, slug: &str) -> Result<bool, DbErr> {
    Ok(media_streams::Entity::find()
        .filter(media_streams::Column::OrgId.eq(org_id))
        .filter(media_streams::Column::Slug.eq(slug))
        .one(db)
        .await?
        .is_some())
}

fn clean_name(raw: &str) -> Result<String, StreamError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(invalid("name", err_codes::FIELD_REQUIRED));
    }
    if name.chars().count() > NAME_MAX {
        return Err(invalid(
            "name",
            format!("{}:{NAME_MAX}", err_codes::FIELD_MAX_LENGTH),
        ));
    }
    Ok(name.to_string())
}

/// Parses and checks a stream URL: http(s) or rtsp only (the kind picks
/// one, [`check_kind_url`]), no credential, host
/// allowed by the egress policy. Every refusal of the policy is the same
/// `media-stream-unreachable` (no scan oracle, D160). Address checks
/// happen again at each connection (DNS rebinding).
pub fn clean_url(raw: &str) -> Result<reqwest::Url, StreamError> {
    clean_url_under(raw, egress::policy())
}

fn clean_url_under(raw: &str, policy: egress::EgressPolicy) -> Result<reqwest::Url, StreamError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(invalid("url", err_codes::FIELD_REQUIRED));
    }
    if raw.len() > URL_MAX_LEN {
        return Err(invalid(
            "url",
            format!("{}:{URL_MAX_LEN}", err_codes::FIELD_MAX_LENGTH),
        ));
    }
    let url = reqwest::Url::parse(raw).map_err(|_| invalid("url", err_codes::FIELD_INVALID))?;
    if !matches!(url.scheme(), "http" | "https" | "rtsp") || url.host_str().is_none() {
        return Err(invalid("url", err_codes::FIELD_INVALID));
    }
    if url_has_credentials(&url) {
        return Err(StreamError::UrlHasCredentials);
    }
    if egress::check_host(url.host_str().unwrap_or_default(), policy).is_some() {
        return Err(StreamError::Unreachable);
    }
    Ok(url)
}

/// The URL scheme must match the stream kind (rtsp ⇔ `rtsp`).
fn check_kind_url(am: &media_streams::ActiveModel) -> Result<(), StreamError> {
    let kind = MediaStreamKind::from_wire(am.kind.as_ref()).unwrap_or_default();
    let ok = reqwest::Url::parse(am.url.as_ref()).is_ok_and(|u| kind.scheme_ok(u.scheme()));
    if ok {
        Ok(())
    } else {
        Err(invalid("url", err_codes::FIELD_INVALID))
    }
}

/// `(scheme, host, port)` of a URL: the destination a secret is bound to.
pub fn origin(url: &reqwest::Url) -> (String, String, Option<u16>) {
    (
        url.scheme().to_string(),
        url.host_str().unwrap_or_default().to_ascii_lowercase(),
        url.port_or_known_default(),
    )
}

fn valid_timezone(tz: &str) -> bool {
    !tz.is_empty()
        && tz.len() <= TIMEZONE_MAX
        && tz
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-' | b'+'))
}

/// Resolves an optional uuid field; empty string clears it.
fn opt_uuid(field: &'static str, raw: &str) -> Result<Option<Uuid>, StreamError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    Uuid::parse_str(raw)
        .map(Some)
        .map_err(|_| invalid(field, err_codes::FIELD_INVALID_UUID))
}

/// Applies the non-secret fields of `input` on `am`, checking every
/// reference against the org (R1). The URL is handled by the caller.
async fn apply<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    am: &mut media_streams::ActiveModel,
    input: &MediaStreamInput,
) -> Result<(), StreamError> {
    if let Some(name) = &input.name {
        am.name = Set(clean_name(name)?);
    }
    if let Some(kind) = input.kind {
        am.kind = Set(kind.wire().to_string());
    }
    if let Some(raw) = &input.capture_on {
        match CaptureOn::from_wire(raw.trim()) {
            Some(c @ (CaptureOn::Server | CaptureOn::Worker)) => am.capture_on = Set(c.wire()),
            // Capture boxes arrive with their device upload channel (lot 6).
            Some(CaptureOn::Device(_)) => return Err(StreamError::CaptureUnsupported),
            None => return Err(invalid("capture_on", err_codes::FIELD_INVALID)),
        }
    }
    if let Some(raw) = &input.asr_profile_id {
        let id = opt_uuid("asr_profile_id", raw)?;
        if let Some(id) = id {
            asr_profiles::Entity::find_by_id(id)
                .filter(asr_profiles::Column::OrgId.eq(org_id))
                .one(db)
                .await?
                .ok_or_else(|| invalid("asr_profile_id", err_codes::FIELD_INVALID))?;
        }
        am.asr_profile_id = Set(id);
    }
    if let Some(raw) = &input.notify_channel_id {
        let id = opt_uuid("notify_channel_id", raw)?;
        if let Some(id) = id {
            notify_channels::Entity::find_by_id(id)
                .filter(notify_channels::Column::OrgId.eq(org_id))
                .one(db)
                .await?
                .ok_or_else(|| invalid("notify_channel_id", err_codes::FIELD_INVALID))?;
        }
        am.notify_channel_id = Set(id);
    }
    if let Some(t) = input.tracks {
        am.tracks = Set(t.wire().to_string());
    }
    if let Some(v) = input.fps {
        if !(FPS_MIN..=FPS_MAX).contains(&v) {
            return Err(invalid("fps", err_codes::FIELD_INVALID));
        }
        am.fps = Set(v);
    }
    if let Some(v) = input.segment_secs {
        if !(SEGMENT_SECS_MIN..=SEGMENT_SECS_MAX).contains(&v) {
            return Err(invalid("segment_secs", err_codes::FIELD_INVALID));
        }
        am.segment_secs = Set(v);
    }
    if let Some(v) = input.overlap_secs {
        if !(0..=OVERLAP_SECS_MAX).contains(&v) {
            return Err(invalid("overlap_secs", err_codes::FIELD_INVALID));
        }
        am.overlap_secs = Set(v);
    }
    if let Some(raw) = &input.audio_retention {
        let r = AudioRetention::from_wire(raw.trim())
            .ok_or_else(|| invalid("audio_retention", err_codes::FIELD_INVALID))?;
        am.audio_retention = Set(r.wire());
    }
    if let Some(tz) = &input.timezone {
        let tz = tz.trim();
        if !valid_timezone(tz) {
            return Err(invalid("timezone", err_codes::FIELD_INVALID));
        }
        am.timezone = Set(tz.to_string());
    }
    if let Some(checked) = input.tdm_checked {
        am.tdm_checked_at = Set(checked.then(|| chrono::Utc::now().into()));
    }
    if let Some(note) = &input.tdm_note {
        let note = note.trim();
        if note.chars().count() > TDM_NOTE_MAX {
            return Err(invalid(
                "tdm_note",
                format!("{}:{TDM_NOTE_MAX}", err_codes::FIELD_MAX_LENGTH),
            ));
        }
        am.tdm_note = Set((!note.is_empty()).then(|| note.to_string()));
    }
    if let Some(enabled) = input.enabled {
        am.enabled = Set(enabled);
    }
    Ok(())
}

/// An enabled stream with an audio track needs a profile whose models
/// (ASR, and the VAD and diarization ones when set) passed their check
/// (D167 guard, school of D101). A video-only stream needs none (D175).
async fn check_runnable<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    am: &media_streams::ActiveModel,
) -> Result<(), StreamError> {
    let tracks = MediaTracks::from_wire(am.tracks.as_ref()).unwrap_or_default();
    if !tracks.has_audio() {
        return Ok(());
    }
    let Some(profile) = am.asr_profile_id.clone().unwrap() else {
        return Err(StreamError::AsrModelInvalid);
    };
    let Some(p) = asr_profiles::Entity::find_by_id(profile)
        .filter(asr_profiles::Column::OrgId.eq(org_id))
        .one(db)
        .await?
    else {
        return Err(StreamError::AsrModelInvalid);
    };
    let ids: Vec<Uuid> = std::iter::once(p.asr_model_id)
        .chain(p.vad_model_id)
        .chain(p.diarization_model_id)
        .chain(p.diarization_embedding_model_id)
        .collect();
    let valid = ml_models::Entity::find()
        .filter(ml_models::Column::Id.is_in(ids.clone()))
        .filter(ml_models::Column::OrgId.eq(org_id))
        .filter(
            ml_models::Column::CheckStatus.eq(pnex_core::vision::ModelCheckStatus::Valid.wire()),
        )
        .all(db)
        .await?
        .len()
        == ids.len();
    if valid {
        Ok(())
    } else {
        Err(StreamError::AsrModelInvalid)
    }
}

/// First free slug for `name` in the org (`_2`, `_3`… on conflict). A
/// slug is frozen at creation.
async fn free_slug<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    name: &str,
) -> Result<String, StreamError> {
    let base = stream_slug(name).unwrap_or_else(|| "stream".to_string());
    for n in 1..1000u32 {
        let candidate = if n == 1 {
            base.clone()
        } else {
            let suffix = format!("_{n}");
            let mut b = base.clone();
            b.truncate(SLUG_MAX_LEN - suffix.len());
            format!("{}{suffix}", b.trim_end_matches('_'))
        };
        if !slug_taken(db, org_id, &candidate).await? {
            return Ok(candidate);
        }
    }
    Err(StreamError::Limit)
}

fn writer(org_id: i64, by: Author) -> Writer {
    Writer {
        org_id: Some(org_id),
        user_id: by.user_id,
    }
}

pub async fn create(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
    max_per_org: u64,
    by: Author,
    input: &MediaStreamInput,
) -> Result<media_streams::Model, StreamError> {
    let name = clean_name(input.name.as_deref().unwrap_or_default())?;
    let Some(kind) = input.kind else {
        return Err(invalid("kind", err_codes::FIELD_REQUIRED));
    };
    let url = clean_url(input.url.as_deref().unwrap_or_default())?;
    let txn = db.begin().await?;
    db_lock::xact_lock(&txn, db_lock::ns::MEDIA_STREAM_QUOTA, org_id).await?;
    let count = media_streams::Entity::find()
        .filter(media_streams::Column::OrgId.eq(org_id))
        .filter(media_streams::Column::DeletedAt.is_null())
        .count(&txn)
        .await?;
    if count >= max_per_org {
        return Err(StreamError::Limit);
    }
    let id = Uuid::new_v4();
    let slug = free_slug(&txn, org_id, &name).await?;
    let now: sea_orm::prelude::DateTimeWithTimeZone = chrono::Utc::now().into();
    let mut am = media_streams::ActiveModel {
        id: Set(id),
        org_id: Set(org_id),
        name: Set(name),
        slug: Set(slug.clone()),
        kind: Set(kind.wire().to_string()),
        url: Set(url.to_string()),
        secret_id: Set(None),
        enabled: Set(false),
        capture_on: Set(CaptureOn::Server.wire()),
        asr_profile_id: Set(None),
        tracks: Set(MediaTracks::Audio.wire().into()),
        fps: Set(FPS_DEFAULT),
        segment_secs: Set(SEGMENT_SECS_DEFAULT),
        overlap_secs: Set(OVERLAP_SECS_DEFAULT),
        audio_retention: Set(AudioRetention::None.wire()),
        notify_channel_id: Set(None),
        timezone: Set("Europe/Paris".into()),
        tdm_checked_at: Set(None),
        tdm_note: Set(None),
        capture_state: Set(CaptureState::Stopped.wire().into()),
        capture_error: Set(None),
        capture_changed_at: Set(None),
        deleted_at: Set(None),
        created_by: Set(by.user_id),
        created_at: Set(now),
        updated_at: Set(now),
    };
    apply(&txn, org_id, &mut am, input).await?;
    check_kind_url(&am)?;
    if input.enabled == Some(true) {
        check_runnable(&txn, org_id, &am).await?;
    }
    if let Some(secret) = input.auth_secret.as_ref() {
        let held = media_secret::save(
            &txn,
            ring,
            writer(org_id, by),
            by.can_manage_secrets,
            id,
            &slug,
            None,
            Some(secret),
        )
        .await?;
        am.secret_id = Set(held);
    }
    let row = am.insert(&txn).await?;
    txn.commit().await?;
    Ok(row)
}

pub async fn update(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
    by: Author,
    id: Uuid,
    input: &MediaStreamInput,
) -> Result<media_streams::Model, StreamError> {
    let txn = db.begin().await?;
    let row = find(&txn, org_id, id).await?;
    let mut am: media_streams::ActiveModel = row.clone().into();
    if let Some(raw) = &input.url {
        let url = clean_url(raw)?;
        let before = reqwest::Url::parse(&row.url).ok();
        let moved = before.as_ref().map(origin) != Some(origin(&url));
        // R9: a stream holding a secret keeps its origin unless an
        // owner/admin moves it.
        if moved && row.secret_id.is_some() && !by.can_manage_secrets {
            return Err(StoreError::DestinationLocked {
                field: "auth".into(),
            }
            .into());
        }
        am.url = Set(url.to_string());
    }
    apply(&txn, org_id, &mut am, input).await?;
    if input.clear_secret {
        if row.secret_id.is_some() {
            media_secret::release(&txn, org_id, id, &row.slug).await?;
        }
        am.secret_id = Set(None);
    } else if let Some(secret) = input.auth_secret.as_ref() {
        let held = media_secret::save(
            &txn,
            ring,
            writer(org_id, by),
            by.can_manage_secrets,
            id,
            &row.slug,
            row.secret_id,
            Some(secret),
        )
        .await?;
        am.secret_id = Set(held);
    }
    check_kind_url(&am)?;
    let enabled = am.enabled.clone().unwrap();
    if enabled {
        check_runnable(&txn, org_id, &am).await?;
    }
    am.updated_at = Set(chrono::Utc::now().into());
    let row = am.update(&txn).await?;
    txn.commit().await?;
    Ok(row)
}

/// Deletes a stream: its segments, its dedicated secret and its capture
/// go; the row stays as a tombstone so its slug (the name of its O2
/// streams) is never given to another stream (D159). Audio blobs still in
/// the store are removed by the caller after commit.
pub async fn delete(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
) -> Result<media_streams::Model, StreamError> {
    let txn = db.begin().await?;
    let row = find(&txn, org_id, id).await?;
    media_secret::release(&txn, org_id, id, &row.slug).await?;
    media_segments::Entity::delete_many()
        .filter(media_segments::Column::StreamId.eq(id))
        .exec(&txn)
        .await?;
    let now: sea_orm::prelude::DateTimeWithTimeZone = chrono::Utc::now().into();
    let mut am: media_streams::ActiveModel = row.clone().into();
    am.deleted_at = Set(Some(now));
    am.enabled = Set(false);
    am.secret_id = Set(None);
    am.asr_profile_id = Set(None);
    am.notify_channel_id = Set(None);
    am.capture_state = Set(CaptureState::Stopped.wire().into());
    am.updated_at = Set(now);
    am.update(&txn).await?;
    txn.commit().await?;
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_checks_refuse_credentials_and_odd_schemes() {
        assert!(matches!(
            clean_url("https://u:p@radio.example/live"),
            Err(StreamError::UrlHasCredentials)
        ));
        assert!(matches!(
            clean_url("https://radio.example/live?token=x"),
            Err(StreamError::UrlHasCredentials)
        ));
        for bad in [
            "",
            "ftp://radio.example/x",
            "file:///etc/passwd",
            "not a url",
        ] {
            assert!(
                matches!(
                    clean_url(bad),
                    Err(StreamError::Invalid { field: "url", .. })
                ),
                "{bad}"
            );
        }
        assert!(clean_url("https://icecast.radiofrance.fr/franceinter-midfi.mp3").is_ok());
        assert!(clean_url("rtsp://cam.example/stream1").is_ok());
        assert!(matches!(
            clean_url("rtsp://admin:pw@cam.example/stream1"),
            Err(StreamError::UrlHasCredentials)
        ));
    }

    #[test]
    fn egress_policy_refuses_internal_targets() {
        use egress::EgressPolicy::{Lan, Public};
        for url in [
            "http://127.0.0.1:5150/api",
            "http://[::1]/x",
            "http://169.254.169.254/latest/meta-data",
            "http://localhost/x.mp3",
            "http://postgres:5432/",
        ] {
            assert!(
                matches!(clean_url_under(url, Lan), Err(StreamError::Unreachable)),
                "{url}"
            );
        }
        assert!(clean_url_under("http://192.168.1.20:9981/stream", Lan).is_ok());
        assert!(matches!(
            clean_url_under("http://192.168.1.20:9981/stream", Public),
            Err(StreamError::Unreachable)
        ));
        assert!(clean_url_under("https://radio.example/a.mp3", Public).is_ok());
    }

    #[test]
    fn origin_includes_default_port() {
        let a = reqwest::Url::parse("https://radio.example/a").unwrap();
        let b = reqwest::Url::parse("https://radio.example:443/b?x=1").unwrap();
        let c = reqwest::Url::parse("http://radio.example/a").unwrap();
        assert_eq!(origin(&a), origin(&b));
        assert_ne!(origin(&a), origin(&c));
    }

    #[test]
    fn timezones() {
        for ok in [
            "Europe/Paris",
            "UTC",
            "America/Argentina/Buenos_Aires",
            "Etc/GMT+2",
        ] {
            assert!(valid_timezone(ok), "{ok}");
        }
        for bad in ["", "Europe/Paris; DROP", "../../etc"] {
            assert!(!valid_timezone(bad), "{bad}");
        }
    }
}
