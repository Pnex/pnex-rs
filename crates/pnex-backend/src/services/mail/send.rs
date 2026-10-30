//! Sending from a mailbox (mail.md D456–D457, D518–D521). A mail is first
//! a row of the mailbox's outbox; it leaves by SMTP when a reader sends it,
//! or at once when a flow writes from a mailbox set to `send`. The outbox
//! is read with the mailbox's own rules (D454): a draft of a member's
//! mailbox is never shown to anyone who does not read it.
//!
//! No loop (D457): a flow never answers an automatic mail or one PneX
//! sent; caps per hour and per correspondent, a mailbox sending on its own
//! is paused when it hits them; mails that leave without a person carry
//! `Auto-Submitted: auto-generated`, all carry `X-PneX-Origin`.

use std::time::Duration;

use lettre::message::header::{ContentType, HeaderName, HeaderValue};
use lettre::message::Mailbox as Addr;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use pnex_core::err_codes;
use pnex_core::flow::FlowNodeKind;
use pnex_core::mail::{
    check_outgoing, MailAddress, MailSendInput, MailSendNodeReply, MailSendNodeRequest,
    MailboxError, OutboxMail, OutboxSendInput, OutboxState, SendMode, DRAFTS_PER_HOUR,
    ORIGIN_HEADER, REPLIES_PER_DAY, SENDS_PER_HOUR, SEND_KEY_MAX, SEND_PORT_OK, SEND_PORT_REFUSED,
    SMTPS_PORT,
};
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{ConnectionTrait, DatabaseConnection, FromQueryResult};
use serde_json::{json, Value};
use uuid::Uuid;

use super::imap::error;
use super::{find, invalid, stmt, Access, Caller, MailError, Row};
use crate::services::ontology::flow_nodes::{deployed, NodeError};
use crate::services::secrets::Keyring;

const TIMEOUT: Duration = Duration::from_secs(30);

/// Who writes the mail.
#[derive(Clone, Copy, Debug)]
pub enum Sender {
    /// A member (or the assistant on their behalf): they are the one sending.
    User(Caller),
    /// A deployed flow: an automated sender.
    Flow { flow_id: i64, version: i64 },
}

impl Sender {
    fn origin(&self) -> String {
        match self {
            Self::User(c) => format!("user:{}", c.user_id),
            Self::Flow { flow_id, version } => format!("flow:{flow_id}@{version}"),
        }
    }

    fn automated(&self) -> bool {
        matches!(self, Self::Flow { .. })
    }
}

fn refused(status: u16, code: &str, args: Value) -> MailError {
    MailError::Refused {
        status,
        error: error(code, args),
    }
}

#[derive(Debug, Clone, FromQueryResult)]
struct OutRow {
    id: Uuid,
    mailbox_id: Uuid,
    state: String,
    to_addrs: Vec<String>,
    subject: String,
    body: String,
    in_reply_to: Option<String>,
    refs: Vec<String>,
    key: Option<String>,
    origin: String,
    message_id: Option<String>,
    error_code: Option<String>,
    error_args: Option<Value>,
    created_at: DateTimeWithTimeZone,
    sent_at: Option<DateTimeWithTimeZone>,
}

const COLUMNS: &str = "id, mailbox_id, state, to_addrs, subject, body, in_reply_to, refs, key, \
     origin, message_id, error_code, error_args, created_at, sent_at";

impl OutRow {
    fn view(self) -> OutboxMail {
        OutboxMail {
            id: self.id.to_string(),
            mailbox_id: self.mailbox_id.to_string(),
            state: OutboxState::parse(&self.state).unwrap_or(OutboxState::Failed),
            to: self.to_addrs,
            subject: self.subject,
            text: self.body,
            in_reply_to: self.in_reply_to,
            key: self.key,
            origin: self.origin,
            error: self.error_code.map(|code| MailboxError {
                code,
                args: self.error_args.unwrap_or(Value::Null),
            }),
            message_id: self.message_id,
            created_at: self.created_at.to_rfc3339(),
            sent_at: self.sent_at.map(|t| t.to_rfc3339()),
        }
    }
}

async fn out_row<C: ConnectionTrait>(
    db: &C,
    mailbox: Uuid,
    id: Uuid,
) -> Result<Option<OutRow>, MailError> {
    Ok(OutRow::find_by_statement(stmt(
        &format!("SELECT {COLUMNS} FROM mail_outbox WHERE mailbox_id = $1 AND id = $2"),
        vec![mailbox.into(), id.into()],
    ))
    .one(db)
    .await?)
}

/// The mailbox, when the sender may write from it: a member who reads it;
/// a flow whose deployer was checked at deploy (D454).
async fn mailbox_for(
    db: &DatabaseConnection,
    org_id: i64,
    mailbox: Uuid,
    sender: Sender,
) -> Result<Row, MailError> {
    let row = find(db, org_id, mailbox).await?;
    if let Sender::User(caller) = sender {
        if !Access::of(db, org_id, caller).await?.reads(&row) {
            return Err(MailError::NotFound);
        }
    }
    if row.smtp_host.is_empty() {
        return Err(refused(409, err_codes::MAIL_SEND_DISABLED, Value::Null));
    }
    // A paused mailbox (by a person, or by a cap) takes nothing from flows.
    if sender.automated() && row.state() != pnex_core::mail::MailboxState::Active {
        return Err(refused(
            409,
            err_codes::MAIL_SEND_DISABLED,
            json!({"state": row.state}),
        ));
    }
    Ok(row)
}

/// The stored mail being answered, and its references for the thread. A
/// flow never answers an automatic mail nor one PneX sent (D457.1, .5).
async fn answered(
    db: &DatabaseConnection,
    row: &Row,
    in_reply_to: &str,
    automated: bool,
) -> Result<Vec<String>, MailError> {
    let found = db
        .query_one_raw(stmt(
            "SELECT refs, automatic, from_pnex FROM mail_messages \
             WHERE mailbox_id = $1 AND message_id = $2",
            vec![row.id.into(), in_reply_to.into()],
        ))
        .await?
        .ok_or_else(|| invalid("in_reply_to", err_codes::FIELD_INVALID))?;
    let automatic: bool = found.try_get("", "automatic").unwrap_or(true);
    let from_pnex: bool = found.try_get("", "from_pnex").unwrap_or(true);
    if automated && automatic {
        return Err(refused(
            409,
            err_codes::MAIL_REPLY_TO_AUTOMATIC,
            Value::Null,
        ));
    }
    if automated && from_pnex {
        return Err(refused(409, err_codes::MAIL_SEND_CHAINED, Value::Null));
    }
    let mut refs: Vec<String> = found.try_get("", "refs").unwrap_or_default();
    refs.push(in_reply_to.to_string());
    Ok(refs)
}

async fn count<C: ConnectionTrait>(db: &C, sql: &str, values: Vec<sea_orm::Value>) -> i64 {
    match db.query_one_raw(stmt(sql, values)).await {
        Ok(Some(r)) => r.try_get("", "n").unwrap_or(i64::MAX),
        // Unknown = over the cap: never a send we could not count.
        _ => i64::MAX,
    }
}

/// Writes a mail from the mailbox: a draft, sent at once when a member asks
/// to or when a flow writes from a mailbox set to `send`. The same `key`
/// answers the mail already written.
pub async fn write(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
    mailbox: Uuid,
    sender: Sender,
    input: &MailSendInput,
) -> Result<OutboxMail, MailError> {
    let row = mailbox_for(db, org_id, mailbox, sender).await?;
    let (to, subject) = check_outgoing(&input.to, &input.subject, &input.text)
        .map_err(|(field, token)| invalid(field, token))?;
    let key = input
        .key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty());
    if key.is_some_and(|k| k.len() > SEND_KEY_MAX || k.chars().any(char::is_control)) {
        return Err(invalid("key", err_codes::FIELD_INVALID));
    }
    if let Some(k) = key {
        if let Some(r) = OutRow::find_by_statement(stmt(
            &format!("SELECT {COLUMNS} FROM mail_outbox WHERE mailbox_id = $1 AND key = $2"),
            vec![row.id.into(), k.into()],
        ))
        .one(db)
        .await?
        {
            return Ok(r.view());
        }
    }
    let automated = sender.automated();
    let refs = match input.in_reply_to.as_deref().map(str::trim) {
        Some(m) if !m.is_empty() => answered(db, &row, m, automated).await?,
        _ => Vec::new(),
    };
    if automated
        && count(
            db,
            "SELECT count(*)::bigint AS n FROM mail_outbox WHERE mailbox_id = $1 AND automated \
             AND created_at > now() - interval '1 hour'",
            vec![row.id.into()],
        )
        .await
            >= DRAFTS_PER_HOUR
    {
        return Err(refused(
            429,
            err_codes::MAIL_SEND_LIMIT,
            json!({"max": DRAFTS_PER_HOUR}),
        ));
    }
    let id = Uuid::new_v4();
    let to: Vec<String> = to.iter().map(|a| a.address.clone()).collect();
    // Two writes racing on one key: the second answers the first's mail.
    let done = db
        .execute_raw(stmt(
            "INSERT INTO mail_outbox (id, org_id, mailbox_id, to_addrs, subject, body, in_reply_to, \
               refs, key, origin, automated) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) \
             ON CONFLICT (mailbox_id, key) DO NOTHING",
            vec![
                id.into(),
                org_id.into(),
                row.id.into(),
                to.into(),
                subject.into(),
                input.text.clone().into(),
                refs.last().cloned().into(),
                refs.into(),
                key.map(str::to_string).into(),
                sender.origin().into(),
                automated.into(),
            ],
        ))
        .await?;
    if done.rows_affected() == 0 {
        return Box::pin(write(db, ring, org_id, mailbox, sender, input)).await;
    }
    let now = match sender {
        Sender::User(_) => input.send_now,
        Sender::Flow { .. } => row.send_mode() == SendMode::Send,
    };
    if now {
        let by = match sender {
            Sender::User(c) => Some(c.user_id),
            Sender::Flow { .. } => None,
        };
        return deliver(db, ring, &row, id, by, None).await;
    }
    out_row(db, row.id, id)
        .await?
        .map(OutRow::view)
        .ok_or(MailError::NotFound)
}

/// The outbox of a mailbox the caller reads, newest first.
pub async fn list(
    db: &DatabaseConnection,
    org_id: i64,
    caller: Caller,
    mailbox: Uuid,
    limit: u64,
    offset: u64,
) -> Result<(Vec<OutboxMail>, u64), MailError> {
    let row = find(db, org_id, mailbox).await?;
    if !Access::of(db, org_id, caller).await?.reads(&row) {
        return Err(MailError::NotFound);
    }
    let total = count(
        db,
        "SELECT count(*)::bigint AS n FROM mail_outbox WHERE mailbox_id = $1",
        vec![row.id.into()],
    )
    .await;
    let rows = OutRow::find_by_statement(stmt(
        &format!(
            "SELECT {COLUMNS} FROM mail_outbox WHERE mailbox_id = $1 \
             ORDER BY created_at DESC LIMIT $2 OFFSET $3"
        ),
        vec![row.id.into(), (limit as i64).into(), (offset as i64).into()],
    ))
    .all(db)
    .await?;
    Ok((
        rows.into_iter().map(OutRow::view).collect(),
        total.max(0) as u64,
    ))
}

/// A reader sends a draft (or retries a failed mail), with the subject and
/// text as reviewed.
pub async fn send_draft(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
    caller: Caller,
    mailbox: Uuid,
    id: Uuid,
    input: &OutboxSendInput,
) -> Result<OutboxMail, MailError> {
    let row = mailbox_for(db, org_id, mailbox, Sender::User(caller)).await?;
    deliver(db, ring, &row, id, Some(caller.user_id), Some(input)).await
}

/// A reader drops a draft or a failed mail.
pub async fn discard(
    db: &DatabaseConnection,
    org_id: i64,
    caller: Caller,
    mailbox: Uuid,
    id: Uuid,
) -> Result<OutboxMail, MailError> {
    let row = find(db, org_id, mailbox).await?;
    if !Access::of(db, org_id, caller).await?.reads(&row) {
        return Err(MailError::NotFound);
    }
    OutRow::find_by_statement(stmt(
        &format!(
            "UPDATE mail_outbox SET state = 'discarded' \
             WHERE mailbox_id = $1 AND id = $2 AND state IN ('draft', 'failed') RETURNING {COLUMNS}"
        ),
        vec![row.id.into(), id.into()],
    ))
    .one(db)
    .await?
    .map(OutRow::view)
    .ok_or_else(|| refused(404, err_codes::MAIL_OUTBOX_NOT_FOUND, Value::Null))
}

/// Sends one mail of the outbox: claimed first (`sending`), so that two
/// clicks or a click and a flow never send it twice. `by` = the member
/// sending it, `None` = it leaves with no person (automated).
async fn deliver(
    db: &DatabaseConnection,
    ring: &Keyring,
    row: &Row,
    id: Uuid,
    by: Option<i64>,
    edit: Option<&OutboxSendInput>,
) -> Result<OutboxMail, MailError> {
    let mut mail = OutRow::find_by_statement(stmt(
        &format!(
            "UPDATE mail_outbox SET state = 'sending' \
             WHERE mailbox_id = $1 AND id = $2 AND state IN ('draft', 'failed') RETURNING {COLUMNS}"
        ),
        vec![row.id.into(), id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| refused(404, err_codes::MAIL_OUTBOX_NOT_FOUND, Value::Null))?;
    let back = |state: &'static str, e: Option<MailboxError>| {
        let (code, args) = e.map(|e| (Some(e.code), Some(e.args))).unwrap_or_default();
        stmt(
            "UPDATE mail_outbox SET state = $3, error_code = $4, error_args = $5 \
             WHERE mailbox_id = $1 AND id = $2",
            vec![
                row.id.into(),
                id.into(),
                state.into(),
                code.into(),
                args.into(),
            ],
        )
    };
    if let Some(e) = edit {
        let subject = e.subject.clone().unwrap_or_else(|| mail.subject.clone());
        let text = e.text.clone().unwrap_or_else(|| mail.body.clone());
        match check_outgoing(&mail.to_addrs, &subject, &text) {
            Ok((_, subject)) => {
                mail.subject = subject;
                mail.body = text;
            }
            Err((field, token)) => {
                db.execute_raw(back("draft", None)).await?;
                return Err(invalid(field, token));
            }
        }
    }
    let automated = by.is_none();
    if let Some((code, max)) = over_cap(db, row, &mail, automated).await {
        let e = error(code, json!({"max": max}));
        if automated {
            // A mailbox sending on its own stops: never a silent storm (D457.4).
            db.execute_raw(back("failed", Some(e.clone()))).await?;
            db.execute_raw(stmt(
                "UPDATE mailboxes SET state = 'paused', last_error_code = $2, last_error_args = $3 \
                 WHERE id = $1",
                vec![row.id.into(), e.code.clone().into(), e.args.clone().into()],
            ))
            .await?;
        } else {
            db.execute_raw(back("draft", None)).await?;
        }
        return Err(MailError::Refused {
            status: 429,
            error: e,
        });
    }
    let to: Vec<MailAddress> = mail
        .to_addrs
        .iter()
        .filter_map(|a| pnex_core::mail::parse_address(a))
        .collect();
    let message_id = message_id(&row.address, id);
    let built = build(&Outgoing {
        from: &row.address,
        to: &to,
        subject: &mail.subject,
        text: &mail.body,
        message_id: &message_id,
        refs: &mail.refs,
        origin: &format!("{}/{}/{}", row.org_id, row.id, id),
        automated,
    });
    let result = match built {
        Ok(m) => match super::password(db, ring, row).await {
            Ok(pw) => transmit(&row.smtp_host, row.smtp_port as u16, &row.imap_user, &pw, m).await,
            Err(e) => Err(e),
        },
        Err(e) => Err(e),
    };
    match result {
        Ok(()) => {
            let sent = OutRow::find_by_statement(stmt(
                &format!(
                    "UPDATE mail_outbox SET state = 'sent', subject = $3, body = $4, message_id = $5, \
                       sent_at = now(), sent_by = $6, error_code = NULL, error_args = NULL \
                     WHERE mailbox_id = $1 AND id = $2 RETURNING {COLUMNS}"
                ),
                vec![
                    row.id.into(),
                    id.into(),
                    mail.subject.into(),
                    mail.body.into(),
                    message_id.into(),
                    by.into(),
                ],
            ))
            .one(db)
            .await?
            .ok_or(MailError::NotFound)?;
            Ok(sent.view())
        }
        Err(e) => {
            tracing::info!(mailbox = %row.id, code = %e.code, "mail send failed");
            db.execute_raw(back("failed", Some(e.clone()))).await?;
            Err(MailError::Refused {
                status: 502,
                error: e,
            })
        }
    }
}

/// The cap a send would pass, if any: per mailbox and hour; and, for a mail
/// leaving with no person, per correspondent and day (D457.4).
async fn over_cap(
    db: &DatabaseConnection,
    row: &Row,
    mail: &OutRow,
    automated: bool,
) -> Option<(&'static str, i64)> {
    let hour = count(
        db,
        "SELECT count(*)::bigint AS n FROM mail_outbox WHERE mailbox_id = $1 AND state = 'sent' \
         AND sent_at > now() - interval '1 hour'",
        vec![row.id.into()],
    )
    .await;
    if hour >= SENDS_PER_HOUR {
        return Some((err_codes::MAIL_SEND_LIMIT, SENDS_PER_HOUR));
    }
    if automated {
        let day = count(
            db,
            "SELECT count(*)::bigint AS n FROM mail_outbox WHERE mailbox_id = $1 AND state = 'sent' \
             AND sent_by IS NULL AND to_addrs && $2 AND sent_at > now() - interval '1 day'",
            vec![row.id.into(), mail.to_addrs.clone().into()],
        )
        .await;
        if day >= REPLIES_PER_DAY {
            return Some((err_codes::MAIL_SEND_LIMIT, REPLIES_PER_DAY));
        }
    }
    None
}

/// One mail of a deployed `mail_send` node (D456): the mailbox is read
/// from the deployed config, never from the request; a refusal answers on
/// port 1 with its code.
pub async fn flow_node(
    db: &DatabaseConnection,
    ring: &Keyring,
    req: &MailSendNodeRequest,
) -> Result<MailSendNodeReply, NodeError> {
    let FlowNodeKind::MailSend { config } = deployed(db, &req.node).await? else {
        return Err(NodeError::NotDeployed);
    };
    let Ok(mailbox) = Uuid::parse_str(config.mailbox.trim()) else {
        return Err(NodeError::NotDeployed);
    };
    let input = MailSendInput {
        to: req.to.clone(),
        subject: req.subject.clone(),
        text: req.text.clone(),
        in_reply_to: req.in_reply_to.clone(),
        key: req.key.clone(),
        send_now: false,
    };
    let sender = Sender::Flow {
        flow_id: req.node.flow_id,
        version: req.node.version,
    };
    let fail = |code: &str, args: Value| MailSendNodeReply {
        port: SEND_PORT_REFUSED,
        code: Some(code.to_string()),
        args,
        mail: None,
    };
    Ok(
        match write(db, ring, req.node.org_id, mailbox, sender, &input).await {
            Ok(mail) => MailSendNodeReply {
                port: SEND_PORT_OK,
                mail: Some(mail),
                ..Default::default()
            },
            Err(MailError::Invalid { field, token }) => fail(&token, json!({"field": field})),
            Err(MailError::Refused { error, .. }) => fail(&error.code, error.args),
            Err(MailError::NotFound) => fail(err_codes::MAIL_MAILBOX_UNKNOWN, Value::Null),
            Err(MailError::NameTaken) | Err(MailError::Store(_)) => {
                fail(err_codes::MAIL_SEND_REFUSED, Value::Null)
            }
        },
    )
}

fn message_id(from: &str, id: Uuid) -> String {
    let domain = from
        .rsplit_once('@')
        .map(|(_, d)| d)
        .unwrap_or("pnex.invalid");
    format!("<{}@{domain}>", id.simple())
}

pub struct Outgoing<'a> {
    pub from: &'a str,
    pub to: &'a [MailAddress],
    pub subject: &'a str,
    pub text: &'a str,
    pub message_id: &'a str,
    /// The thread, oldest first; the last one is answered.
    pub refs: &'a [String],
    /// `<org>/<mailbox>/<mail>`, for `X-PneX-Origin` (D457.2).
    pub origin: &'a str,
    /// Leaves with no person: `Auto-Submitted: auto-generated`.
    pub automated: bool,
}

/// The message as it leaves: plain text, our marks, the thread.
pub fn build(o: &Outgoing) -> Result<Message, MailboxError> {
    let bad = |_| error(err_codes::MAIL_SEND_REFUSED, Value::Null);
    let mut b = Message::builder()
        .from(o.from.parse::<Addr>().map_err(bad)?)
        .subject(o.subject)
        .message_id(Some(o.message_id.to_string()));
    for a in o.to {
        b = b.to(a.address.parse::<Addr>().map_err(bad)?);
    }
    if let Some(last) = o.refs.last() {
        b = b.in_reply_to(last.clone()).references(o.refs.join(" "));
    }
    b = b.raw_header(HeaderValue::new(
        HeaderName::new_from_ascii_str(ORIGIN_HEADER),
        o.origin.to_string(),
    ));
    if o.automated {
        b = b.raw_header(HeaderValue::new(
            HeaderName::new_from_ascii_str("Auto-Submitted"),
            "auto-generated".into(),
        ));
    }
    b.header(ContentType::TEXT_PLAIN)
        .body(o.text.to_string())
        .map_err(|_| error(err_codes::MAIL_SEND_REFUSED, Value::Null))
}

/// One SMTP session: the host and every resolved address through the
/// egress policy (R8), then TLS checked against the host name — implicit on
/// 465, STARTTLS required on any other port, never in clear (D518).
async fn transmit(
    host: &str,
    port: u16,
    user: &str,
    password: &str,
    message: Message,
) -> Result<(), MailboxError> {
    let unreachable = || error(err_codes::MAIL_UNREACHABLE, json!({"host": host}));
    let policy = pnex_core::egress::policy();
    if pnex_core::egress::check_host(host, policy).is_some() {
        return Err(unreachable());
    }
    let addrs = tokio::time::timeout(TIMEOUT, tokio::net::lookup_host((host, port)))
        .await
        .map_err(|_| unreachable())?
        .map_err(|_| unreachable())?;
    let Some(addr) = pnex_core::egress::filter_addrs(addrs, policy)
        .first()
        .copied()
    else {
        return Err(unreachable());
    };
    let tls = TlsParameters::new(host.to_string())
        .map_err(|_| error(err_codes::MAIL_TLS_FAILED, json!({"host": host})))?;
    let transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(addr.ip().to_string())
        .port(port)
        .tls(if port == SMTPS_PORT {
            Tls::Wrapper(tls)
        } else {
            Tls::Required(tls)
        })
        .credentials(Credentials::new(user.to_string(), password.to_string()))
        .timeout(Some(TIMEOUT))
        .build();
    match transport.send(message).await {
        Ok(_) => Ok(()),
        Err(e) if e.is_tls() => Err(error(err_codes::MAIL_TLS_FAILED, json!({"host": host}))),
        Err(e) if e.status().is_some_and(|c| c.to_string() == "535") => {
            Err(error(err_codes::MAIL_AUTH_FAILED, Value::Null))
        }
        Err(e) if e.is_permanent() => {
            tracing::info!(host, error = %e, "smtp refused the message");
            Err(error(err_codes::MAIL_SEND_REFUSED, json!({"host": host})))
        }
        Err(e) => {
            tracing::info!(host, error = %e, "smtp failed");
            Err(unreachable())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(o: &Outgoing) -> String {
        String::from_utf8(build(o).unwrap().formatted()).unwrap()
    }

    #[test]
    fn a_mail_carries_our_marks_and_its_thread() {
        let to = [MailAddress {
            name: String::new(),
            address: "ann@acme.fr".into(),
        }];
        let refs = ["<a@acme.fr>".to_string(), "<b@acme.fr>".to_string()];
        let o = Outgoing {
            from: "sales@pnex.test",
            to: &to,
            subject: "Re: devis",
            text: "Bonjour Ann,\nvoici.",
            message_id: "<1@pnex.test>",
            refs: &refs,
            origin: "2/box/mail",
            automated: true,
        };
        let t = text(&o);
        for want in [
            "From: sales@pnex.test",
            "To: ann@acme.fr",
            "In-Reply-To: <b@acme.fr>",
            "References: <a@acme.fr> <b@acme.fr>",
            "X-PneX-Origin: 2/box/mail",
            "Auto-Submitted: auto-generated",
            "Message-ID: <1@pnex.test>",
        ] {
            assert!(t.contains(want), "{want} missing in:\n{t}");
        }
        // A mail a person sends is not automatic.
        let by_hand = text(&Outgoing {
            automated: false,
            refs: &[],
            ..o
        });
        assert!(!by_hand.contains("Auto-Submitted") && !by_hand.contains("In-Reply-To"));
        assert!(
            by_hand.contains("X-PneX-Origin"),
            "our mark, so it never triggers us"
        );
    }

    #[test]
    fn our_mails_are_recognised_when_they_come_back() {
        let to = [MailAddress {
            name: String::new(),
            address: "ann@acme.fr".into(),
        }];
        let t = text(&Outgoing {
            from: "sales@pnex.test",
            to: &to,
            subject: "s",
            text: "t",
            message_id: "<1@pnex.test>",
            refs: &[],
            origin: "2/box/mail",
            automated: true,
        });
        let headers: Vec<(String, String)> = t
            .lines()
            .take_while(|l| !l.is_empty())
            .filter_map(|l| l.split_once(": "))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert!(pnex_core::mail::is_from_pnex(
            &headers,
            "sales@pnex.test",
            &[]
        ));
        assert!(pnex_core::mail::is_automatic(
            &headers,
            "sales@pnex.test",
            "text/plain"
        ));
    }

    #[test]
    fn a_message_id_is_on_the_mailbox_domain() {
        assert_eq!(
            message_id("sales@acme.fr", Uuid::nil()),
            "<00000000000000000000000000000000@acme.fr>"
        );
    }
}
