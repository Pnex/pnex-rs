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
use pnex_core::mail::{MailMessage, Mailbox, MailboxInput, MailboxTestResult};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination::{self, PageParams};
use crate::services::mail::{self as mail, Access, Caller, MailError};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/mailboxes", get(list).post(create))
        .add("/mailboxes/{id}", axum::routing::put(update).delete(remove))
        .add("/mailboxes/{id}/state", post(set_state))
        .add("/mailboxes/{id}/test", post(test))
        .add("/mailboxes/{id}/messages", get(messages))
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

pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(
        paths(list, create, update, set_state, remove, test, messages),
        components(schemas(StateInput))
    )]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
