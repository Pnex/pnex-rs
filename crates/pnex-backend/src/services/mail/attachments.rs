//! An attachment saved as a media (mail.md M5, D458): never at polling,
//! only when a reader of the mailbox asks, since a media is seen by the
//! whole org while the mail is not (D454). The message is fetched again
//! from the server (PneX keeps no attachment bytes), checked to be the
//! stored one, and its bytes go through the media allowlist like an upload.

use loco_rs::prelude::*;
use pnex_core::mail::{MailAttachment, MailboxError};
use pnex_firmware_builder::sanitize_segment;
use sea_orm::FromQueryResult;
use serde_json::Value;
use uuid::Uuid;

use super::{find, imap, parse, stmt, Access, Caller, MailError};
use crate::controllers::media::{write_version, IncomingVersion};
use crate::models::_entities::media_assets;
use crate::services::media::MediaSettings;
use crate::services::media_sniff;
use crate::services::secrets::Keyring;

#[derive(Debug)]
pub enum SaveError {
    Mail(MailError),
    NotFound,
    Gone,
    TooLarge(usize),
    Format,
    Server(MailboxError),
    Failed,
}

impl From<DbErr> for SaveError {
    fn from(e: DbErr) -> Self {
        Self::Mail(e.into())
    }
}

#[derive(FromQueryResult)]
struct Stored {
    folder: String,
    uid: i64,
    message_id: String,
    truncated: bool,
    attachments: Value,
}

/// Saves attachment `index` of message `message` as a media of the org and
/// returns it with its `media_id`; a second call returns the same media
/// while it exists.
pub async fn save_as_media(
    ctx: &AppContext,
    ring: &Keyring,
    org_id: i64,
    caller: Caller,
    mailbox_id: Uuid,
    message: Uuid,
    index: usize,
) -> Result<MailAttachment, SaveError> {
    let access = Access::of(&ctx.db, org_id, caller).await?;
    let row = find(&ctx.db, org_id, mailbox_id)
        .await
        .map_err(SaveError::Mail)?;
    if !access.reads(&row) {
        return Err(SaveError::Mail(MailError::NotFound));
    }
    let stored = Stored::find_by_statement(stmt(
        "SELECT folder, uid, message_id, truncated, attachments FROM mail_messages \
         WHERE id = $1 AND org_id = $2 AND mailbox_id = $3",
        vec![message.into(), org_id.into(), mailbox_id.into()],
    ))
    .one(&ctx.db)
    .await?
    .ok_or(SaveError::NotFound)?;
    let mut listed: MailAttachment =
        serde_json::from_value::<Vec<MailAttachment>>(stored.attachments.clone())
            .unwrap_or_default()
            .into_iter()
            .nth(index)
            .ok_or(SaveError::NotFound)?;
    if let Some(id) = listed
        .media_id
        .as_deref()
        .and_then(|m| Uuid::parse_str(m).ok())
    {
        if media_assets::Entity::find_by_id(id)
            .filter(media_assets::Column::OrgId.eq(org_id))
            .one(&ctx.db)
            .await?
            .is_some()
        {
            return Ok(listed);
        }
    }
    let settings = MediaSettings::from_config(&ctx.config);
    if stored.truncated {
        return Err(SaveError::TooLarge(pnex_core::mail::RAW_MAX_BYTES));
    }
    let raw = fetch_one(ctx, ring, &row, &stored).await?;
    let found = parse::attachment(&raw, index).ok_or(SaveError::Gone)?;
    // Same UID, other message: the folder was renumbered or emptied.
    if found
        .message_id
        .as_deref()
        .is_some_and(|m| m != stored.message_id)
        || found.name != listed.name
    {
        return Err(SaveError::Gone);
    }
    if found.bytes.len() > settings.max_bytes {
        return Err(SaveError::TooLarge(settings.max_bytes));
    }
    let filename = sanitize_segment(&found.name);
    let sniff = media_sniff::detect(&filename, &found.bytes).ok_or(SaveError::Format)?;
    let asset = media_assets::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        kind: Set(sniff.kind.to_string()),
        name: Set(filename.clone()),
        description: Set(None),
        metadata: Set(None),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await?;
    if let Err(e) = write_version(
        ctx,
        org_id,
        &asset,
        IncomingVersion {
            filename,
            content_type: found.content_type,
            note: None,
            bytes: found.bytes.into(),
        },
        &settings,
    )
    .await
    {
        tracing::warn!(error = %e, "mail attachment not stored");
        let _ = asset.delete(&ctx.db).await;
        return Err(SaveError::Failed);
    }
    listed.media_id = Some(asset.id.to_string());
    ctx.db
        .execute_raw(stmt(
            "UPDATE mail_messages SET attachments = \
               jsonb_set(attachments, ARRAY[$3::text, 'media_id'], to_jsonb($4::text)) \
             WHERE id = $1 AND org_id = $2",
            vec![
                message.into(),
                org_id.into(),
                index.to_string().into(),
                asset.id.to_string().into(),
            ],
        ))
        .await?;
    Ok(listed)
}

/// The full message under its stored UID, read-only (`BODY.PEEK`).
async fn fetch_one(
    ctx: &AppContext,
    ring: &Keyring,
    row: &super::Row,
    stored: &Stored,
) -> Result<Vec<u8>, SaveError> {
    let password = super::password(&ctx.db, ring, row)
        .await
        .map_err(SaveError::Server)?;
    let mut s = imap::connect(
        &row.imap_host,
        row.imap_port as u16,
        &row.imap_user,
        &password,
    )
    .await
    .map_err(SaveError::Server)?;
    drop(password);
    let fetched = async {
        imap::examine(&mut s, &stored.folder).await?;
        imap::fetch(&mut s, &[stored.uid as u32]).await
    }
    .await;
    imap::logout(s).await;
    let one = fetched
        .map_err(SaveError::Server)?
        .into_iter()
        .find(|f| f.uid as i64 == stored.uid)
        .ok_or(SaveError::Gone)?;
    if one.truncated {
        return Err(SaveError::TooLarge(pnex_core::mail::RAW_MAX_BYTES));
    }
    Ok(one.raw)
}
