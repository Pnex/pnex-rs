//! Mailboxes of the org (mail.md D453–D463): a classic CRUD of sources,
//! the password in the vault, a Test button, the messages polled. Org from
//! the principal (R1), `mail:*` checked by the IAM extractor (R2); who
//! manages a mailbox and who reads its messages is decided by the service
//! (D454): a mailbox out of the caller's reach answers 404.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use pnex_core::err_codes;
use pnex_core::mail::{
    MailAttachment, MailMessage, MailSendInput, Mailbox, MailboxInput, MailboxTestResult,
    OutboxMail, OutboxSendInput,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination::{self, PageParams};
use crate::services::mail::attachments::SaveError;
use crate::services::mail::invitations::InviteError;
use crate::services::mail::send::Sender;
use crate::services::mail::{self as mail, Access, Caller, MailError};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/mailboxes", get(list).post(create))
        .add("/mailboxes/{id}", axum::routing::put(update).delete(remove))
        .add("/mailboxes/{id}/state", post(set_state))
        .add("/mailboxes/{id}/test", post(test))
        .add("/mailboxes/{id}/messages", get(messages).post(send_message))
        .add("/mailboxes/{id}/outbox", get(outbox))
        .add("/mailboxes/{id}/outbox/{mail}/send", post(outbox_send))
        .add(
            "/mailboxes/{id}/outbox/{mail}/discard",
            post(outbox_discard),
        )
        .add(
            "/mailboxes/{id}/messages/{message}/attachments/{index}/media",
            post(save_attachment),
        )
        .add(
            "/mailboxes/{id}/messages/{message}/invitation/calendar",
            post(add_invitation),
        )
}

fn caller(org: &OrgContext) -> Caller {
    Caller {
        user_id: org.auth.user.id,
        manages_shared: org.can(mail::SHARED_WRITE),
        reads_content: org.can(mail::CONTENT_READ),
        can_write_secrets: org.can("secrets:write"),
    }
}

fn mail_error(e: MailError) -> Result<Response> {
    match e {
        MailError::Invalid { field, token } => Ok(super::field_error(&field, &token)),
        MailError::NotFound => Err(super::coded_error(
            StatusCode::NOT_FOUND,
            err_codes::MAIL_MAILBOX_NOT_FOUND,
            "Mailbox not found.",
            None,
        )),
        MailError::NameTaken => Err(super::coded_error(
            StatusCode::CONFLICT,
            err_codes::MAIL_MAILBOX_NAME_TAKEN,
            "A mailbox with this name already exists.",
            None,
        )),
        MailError::Refused { status, error } => Err(super::coded_error(
            StatusCode::from_u16(status).unwrap_or(StatusCode::CONFLICT),
            &error.code,
            "The mail was not sent.",
            Some(error.args),
        )),
        MailError::Store(e) => crate::controllers::secrets::store_error(e),
    }
}

async fn view(
    ctx: &AppContext,
    org: &OrgContext,
    row: &mail::Row,
    status: StatusCode,
) -> Result<Response> {
    let access = Access::of(&ctx.db, org.org.id, caller(org))
        .await
        .map_err(|_| Error::InternalServerError)?;
    match mail::views(&ctx.db, &access, std::slice::from_ref(row)).await {
        Ok(mut v) if !v.is_empty() => Ok((status, format::json(v.remove(0))).into_response()),
        _ => Err(Error::InternalServerError),
    }
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
    path = "/api/v1/mailboxes",
    tag = "mail",
    operation_id = "mailboxes_list",
    summary = "List mailboxes",
    description = "Mailboxes of the organization polled by PneX: personal ones (a member or a service account), shared ones (read by their teams), with their state and last error. Never a password; `readable` says whether the caller reads its messages. Any member.",
    params(PageQuery),
    responses((status = 200, body = pnex_core::Paginated<Mailbox>)),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let (rows, access) = match (
        mail::list(&ctx.db, org.org.id).await,
        Access::of(&ctx.db, org.org.id, caller(&org)).await,
    ) {
        (Ok(r), Ok(a)) => (r, a),
        _ => return Err(Error::InternalServerError),
    };
    let views = mail::views(&ctx.db, &access, &rows)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let count = views.len() as i64;
    let (skip, take) = page.slice(views.len());
    let results: Vec<Mailbox> = views.into_iter().skip(skip).take(take).collect();
    format::json(pagination::envelope(
        "/api/v1/mailboxes",
        &[],
        page,
        count,
        results,
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/mailboxes",
    tag = "mail",
    operation_id = "mailboxes_create",
    summary = "Connect a mailbox",
    description = "Connects an IMAP mailbox (IMAPS, TLS from the first byte, port 993 by default); the server must pass the egress policy. By default the caller's own mailbox; `shared` (read by `teams`) or `owner_user_id` naming a service account need the shared-mailbox permission. The password is typed into the mailbox's own vault secret; picking an existing vault secret is for owners and admins. Nothing is fetched before the next poll. Requires the mail write permission.",
    request_body = MailboxInput,
    responses(
        (status = 201, body = Mailbox),
        (status = 400, description = "`field-invalid`: `errors.fields` maps the field to a token", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`mail-mailbox-name-taken`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<MailboxInput>,
) -> Result<Response> {
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(r) => r,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    match mail::create(&ctx.db, &ring, org.org.id, caller(&org), &input).await {
        Ok(row) => view(&ctx, &org, &row, StatusCode::CREATED).await,
        Err(e) => mail_error(e),
    }
}

#[utoipa::path(
    put,
    path = "/api/v1/mailboxes/{id}",
    tag = "mail",
    operation_id = "mailboxes_update",
    summary = "Replace a mailbox",
    description = "Full replacement by its manager (its owner, or the shared-mailbox permission for a shared or service-account one); an absent `password` keeps the current one, except when the server, port or user name changes for a caller who may not move a vault secret: then the password must be typed again. A changed mailbox is tried again at the next poll. Requires the mail write permission.",
    params(("id" = String, Path, format = "uuid", description = "Mailbox id")),
    request_body = MailboxInput,
    responses(
        (status = 200, body = Mailbox),
        (status = 400, description = "`field-invalid`: `errors.fields` maps the field to a token", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`mail-mailbox-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`mail-mailbox-name-taken`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<MailboxInput>,
) -> Result<Response> {
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(r) => r,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    match mail::update(&ctx.db, &ring, org.org.id, caller(&org), id, &input).await {
        Ok(row) => view(&ctx, &org, &row, StatusCode::OK).await,
        Err(e) => mail_error(e),
    }
}

/// `POST /mailboxes/{id}/state`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
struct StateInput {
    paused: bool,
}

#[utoipa::path(
    post,
    path = "/api/v1/mailboxes/{id}/state",
    tag = "mail",
    operation_id = "mailboxes_state",
    summary = "Pause or resume a mailbox",
    description = "Pauses or resumes the polling (resuming clears the last error). Its manager, or an admin over any mailbox of the org without reading its messages. Requires the mail write permission.",
    params(("id" = String, Path, format = "uuid", description = "Mailbox id")),
    request_body = StateInput,
    responses(
        (status = 200, body = Mailbox),
        (status = 404, description = "`mail-mailbox-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn set_state(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<StateInput>,
) -> Result<Response> {
    match mail::set_paused(&ctx.db, org.org.id, caller(&org), id, input.paused).await {
        Ok(row) => view(&ctx, &org, &row, StatusCode::OK).await,
        Err(e) => mail_error(e),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/mailboxes/{id}",
    tag = "mail",
    operation_id = "mailboxes_delete",
    summary = "Delete a mailbox",
    description = "Deletes the mailbox, its stored messages and its dedicated password; nothing changes on the mail server. What flows wrote from its messages stays. Its manager, or an admin over any mailbox of the org. Requires the mail delete permission.",
    params(("id" = String, Path, format = "uuid", description = "Mailbox id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "`mail-mailbox-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn remove(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match mail::delete(&ctx.db, org.org.id, caller(&org), id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => mail_error(e),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/mailboxes/{id}/test",
    tag = "mail",
    operation_id = "mailboxes_test",
    summary = "Test a mailbox",
    description = "From the server: logs in and counts the messages of each folder, read-only; nothing is fetched, marked read nor stored. Its manager only. Requires the mail write permission.",
    params(("id" = String, Path, format = "uuid", description = "Mailbox id")),
    responses(
        (status = 200, body = MailboxTestResult),
        (status = 404, description = "`mail-mailbox-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn test(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let row = match mail::find(&ctx.db, org.org.id, id).await {
        Ok(r) => r,
        Err(e) => return mail_error(e),
    };
    let access = Access::of(&ctx.db, org.org.id, caller(&org))
        .await
        .map_err(|_| Error::InternalServerError)?;
    if !access.manages(&row) {
        return mail_error(MailError::NotFound);
    }
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(r) => r,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    format::json(mail::test(&ctx.db, &ring, &row).await)
}

#[utoipa::path(
    get,
    path = "/api/v1/mailboxes/{id}/messages",
    tag = "mail",
    operation_id = "mailboxes_messages_list",
    summary = "List the messages of a mailbox",
    description = "Messages polled from the mailbox, newest first: sender, recipients, subject, date, text only (never HTML), attachments listed, the server's authentication verdicts, flags for automatic replies and the org's own mails, and the original sender of a forward (trusted only from an authenticated member). The content is untrusted data. Readable by the owner, the teams of a shared mailbox, and admins for a service-account one — never by an admin for a member's personal mailbox (404). Requires the mail content read permission.",
    params(("id" = String, Path, format = "uuid", description = "Mailbox id"), PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<MailMessage>),
        (status = 404, description = "`mail-mailbox-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn messages(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    match mail::messages(
        &ctx.db,
        org.org.id,
        caller(&org),
        id,
        page.limit as u64,
        page.offset as u64,
    )
    .await
    {
        Ok((count, rows)) => format::json(pagination::envelope(
            &format!("/api/v1/mailboxes/{id}/messages"),
            &[],
            page,
            count,
            rows,
        )),
        Err(e) => mail_error(e),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/mailboxes/{id}/messages/{message}/attachments/{index}/media",
    tag = "mail",
    operation_id = "mailboxes_attachments_save",
    summary = "Save a mail attachment as a media",
    description = "Fetches the message again from the mail server (read-only, nothing marked read) and saves attachment `index` (in the listed order) as a new media of the org, under the media upload rules: allowed formats only, size limit. The media is then visible to whoever reads the org's media, unlike the mail. Answers the attachment with its `media_id`; saving it again returns the same media while it exists. Only a reader of the mailbox (404 for anyone else). Requires the media write permission and the mail content read permission.",
    params(
        ("id" = String, Path, format = "uuid", description = "Mailbox id"),
        ("message" = String, Path, format = "uuid", description = "Stored message id"),
        ("index" = i64, Path, description = "Attachment position, from 0")
    ),
    responses(
        (status = 200, body = MailAttachment),
        (status = 400, description = "`media-format-unsupported`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`mail-mailbox-not-found`, `mail-attachment-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`mail-message-gone`", body = crate::openapi::ErrorDetail),
        (status = 413, description = "`mail-too-large` (`args.max_bytes`)", body = crate::openapi::ErrorDetail),
        (status = 502, description = "The mail server refused or failed: `mail-unreachable`, `mail-tls-failed`, `mail-auth-failed`, `mail-folder-unknown`, `mail-protocol`, `mail-no-password`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn save_attachment(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, message, index)): Path<(Uuid, Uuid, usize)>,
) -> Result<Response> {
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(r) => r,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    match mail::attachments::save_as_media(
        &ctx,
        &ring,
        org.org.id,
        caller(&org),
        id,
        message,
        index,
    )
    .await
    {
        Ok(a) => format::json(a),
        Err(SaveError::Mail(e)) => mail_error(e),
        Err(SaveError::NotFound) => Err(super::coded_error(
            StatusCode::NOT_FOUND,
            err_codes::MAIL_ATTACHMENT_NOT_FOUND,
            "Attachment not found.",
            None,
        )),
        Err(SaveError::Gone) => Err(super::coded_error(
            StatusCode::CONFLICT,
            err_codes::MAIL_MESSAGE_GONE,
            "The mail server no longer holds this message.",
            None,
        )),
        Err(SaveError::TooLarge(max)) => Err(super::coded_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            err_codes::MAIL_TOO_LARGE,
            "Too large to save.",
            Some(serde_json::json!({ "max_bytes": max.to_string() })),
        )),
        Err(SaveError::Format) => Err(crate::controllers::media::format_unsupported()),
        Err(SaveError::Server(e)) => Err(super::coded_error(
            StatusCode::BAD_GATEWAY,
            &e.code,
            "The mail server refused or failed.",
            Some(e.args),
        )),
        Err(SaveError::Failed) => Err(Error::InternalServerError),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/mailboxes/{id}/messages/{message}/invitation/calendar",
    tag = "mail",
    operation_id = "mailboxes_invitations_add",
    summary = "Add a mail's meeting invitation to the calendar",
    description = "Adds the meeting invitation the mail carries to the organization calendar, as a range of category `meeting` keyed by its iCalendar UID: adding it again, or adding a later update, moves the same meeting; a cancellation marks it cancelled. The organizer and attendees are kept with their addresses, each matched to the people of the organization that use it as an e-mail identifier (none created). The meeting is then visible to whoever reads the calendar, unlike the mail. Nothing is answered to the organizer. Only a reader of the mailbox (404 for anyone else). Requires the time range write permission and the mail content read permission.",
    params(
        ("id" = String, Path, format = "uuid", description = "Mailbox id"),
        ("message" = String, Path, format = "uuid", description = "Stored message id")
    ),
    responses(
        (status = 200, body = pnex_core::time_range::TimeRange),
        (status = 404, description = "`mail-mailbox-not-found`, `mail-invitation-not-found`", body = crate::openapi::ErrorDetail),
        (status = 400, description = "`field-invalid` (the invitation has no usable times)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn add_invitation(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, message)): Path<(Uuid, Uuid)>,
) -> Result<Response> {
    let hide = crate::services::ontology::redact::Redaction::of(|p| org.can(p));
    match mail::invitations::add_to_calendar(&ctx.db, org.org.id, caller(&org), &hide, id, message)
        .await
    {
        Ok(r) => format::json(r),
        Err(InviteError::Mail(e)) => mail_error(e),
        Err(InviteError::NotFound) => Err(super::coded_error(
            StatusCode::NOT_FOUND,
            err_codes::MAIL_INVITATION_NOT_FOUND,
            "This mail carries no invitation.",
            None,
        )),
        Err(InviteError::Range(e)) => crate::controllers::time_ranges::range_error(e),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/mailboxes/{id}/messages",
    tag = "mail",
    operation_id = "mailboxes_messages_send",
    summary = "Write a mail from a mailbox",
    description = "Writes a plain-text mail from the mailbox to at most 20 recipients: a draft in its outbox, or sent at once with `send_now` (the caller is the one sending). `in_reply_to` answers a stored mail of this mailbox (threaded with its references). The same `key` answers the mail already written. Caps: 30 sends per mailbox and hour (`mail-send-limit`, 429). Only a reader of the mailbox (404 for anyone else). Requires the send permission and the mail content read permission.",
    params(("id" = String, Path, format = "uuid", description = "Mailbox id")),
    request_body = MailSendInput,
    responses(
        (status = 201, body = OutboxMail),
        (status = 400, description = "Invalid field: `to`, `subject`, `text`, `in_reply_to`, `key`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`mail-mailbox-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`mail-send-disabled` (no SMTP server)", body = crate::openapi::ErrorDetail),
        (status = 429, description = "`mail-send-limit` (`args.max`)", body = crate::openapi::ErrorDetail),
        (status = 502, description = "The SMTP server refused or failed: `mail-send-refused`, `mail-unreachable`, `mail-tls-failed`, `mail-auth-failed`, `mail-no-password`; the mail stays in the outbox as failed", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn send_message(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<MailSendInput>,
) -> Result<Response> {
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(r) => r,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    match mail::send::write(
        &ctx.db,
        &ring,
        org.org.id,
        id,
        Sender::User(caller(&org)),
        &input,
    )
    .await
    {
        Ok(m) => Ok((StatusCode::CREATED, format::json(m)).into_response()),
        Err(e) => mail_error(e),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/mailboxes/{id}/outbox",
    tag = "mail",
    operation_id = "mailboxes_outbox_list",
    summary = "List the outbox of a mailbox",
    description = "Mails written from the mailbox, newest first: drafts waiting for a person (written by flows), sent, failed (with the error) and discarded. Readable like the mailbox's messages: never by an admin for a member's personal mailbox (404). Requires the mail content read permission.",
    params(("id" = String, Path, format = "uuid", description = "Mailbox id"), PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<OutboxMail>),
        (status = 404, description = "`mail-mailbox-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn outbox(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    match mail::send::list(
        &ctx.db,
        org.org.id,
        caller(&org),
        id,
        page.limit as u64,
        page.offset as u64,
    )
    .await
    {
        Ok((rows, count)) => format::json(pagination::envelope(
            &format!("/api/v1/mailboxes/{id}/outbox"),
            &[],
            page,
            count as i64,
            rows,
        )),
        Err(e) => mail_error(e),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/mailboxes/{id}/outbox/{mail}/send",
    tag = "mail",
    operation_id = "mailboxes_outbox_send",
    summary = "Send a draft of the outbox",
    description = "A person sends a draft (or retries a failed mail), optionally with its subject and text as reviewed. Sent once: a second click answers `mail-outbox-not-found`. Sent by a person, the mail carries no Auto-Submitted header. Only a reader of the mailbox. Requires the send permission and the mail content read permission.",
    params(
        ("id" = String, Path, format = "uuid", description = "Mailbox id"),
        ("mail" = String, Path, format = "uuid", description = "Outbox mail id")
    ),
    request_body = OutboxSendInput,
    responses(
        (status = 200, body = OutboxMail),
        (status = 400, description = "Invalid `subject` or `text`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`mail-mailbox-not-found`, `mail-outbox-not-found` (not a draft any more)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`mail-send-disabled`", body = crate::openapi::ErrorDetail),
        (status = 429, description = "`mail-send-limit` (`args.max`): the draft stays", body = crate::openapi::ErrorDetail),
        (status = 502, description = "The SMTP server refused or failed; the mail is failed and may be retried", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn outbox_send(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, mail_id)): Path<(Uuid, Uuid)>,
    Json(input): Json<OutboxSendInput>,
) -> Result<Response> {
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(r) => r,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    match mail::send::send_draft(
        &ctx.db,
        &ring,
        org.org.id,
        caller(&org),
        id,
        mail_id,
        &input,
    )
    .await
    {
        Ok(m) => format::json(m),
        Err(e) => mail_error(e),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/mailboxes/{id}/outbox/{mail}/discard",
    tag = "mail",
    operation_id = "mailboxes_outbox_discard",
    summary = "Discard a draft of the outbox",
    description = "Drops a draft or a failed mail: it stays listed as discarded and is never sent. Only a reader of the mailbox. Requires the send permission and the mail content read permission.",
    params(
        ("id" = String, Path, format = "uuid", description = "Mailbox id"),
        ("mail" = String, Path, format = "uuid", description = "Outbox mail id")
    ),
    responses(
        (status = 200, body = OutboxMail),
        (status = 404, description = "`mail-mailbox-not-found`, `mail-outbox-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn outbox_discard(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, mail_id)): Path<(Uuid, Uuid)>,
) -> Result<Response> {
    match mail::send::discard(&ctx.db, org.org.id, caller(&org), id, mail_id).await {
        Ok(m) => format::json(m),
        Err(e) => mail_error(e),
    }
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(
        paths(
            list,
            create,
            update,
            set_state,
            remove,
            test,
            messages,
            send_message,
            outbox,
            outbox_send,
            outbox_discard,
            save_attachment,
            add_invitation
        ),
        components(schemas(StateInput))
    )]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
