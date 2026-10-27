//! A meeting invitation added to the calendar (mail.md D466): like an
//! attachment (D465), only when a reader of the mailbox asks, since the
//! calendar is seen by the whole org. The meeting becomes a range of the
//! org keyed by its iCalendar `UID`: an update moves it, a cancellation
//! marks it cancelled. Each person is matched to the org's people by an
//! e-mail identifier, as the pack flows do; nobody is created.

use pnex_core::mail::{MailAddress, MailInvitation};
use pnex_core::ontology::api::{Direction, FilterOp, Hop, OntologyQuery, PropertyFilter};
use pnex_core::time_range::{RangeOrigin, TimeRange, TimeRangeInput};
use sea_orm::{DatabaseConnection, FromQueryResult};
use serde_json::{json, Value};
use uuid::Uuid;

use super::{find, stmt, Access, Caller, MailError};
use crate::services::ontology::redact::Redaction;
use crate::services::time_ranges::{self, RangeError, Writer};

/// Category of the ranges made from invitations.
pub const CATEGORY: &str = "meeting";

#[derive(Debug)]
pub enum InviteError {
    Mail(MailError),
    /// The message carries no invitation (or is not stored).
    NotFound,
    Range(RangeError),
}

impl From<sea_orm::DbErr> for InviteError {
    fn from(e: sea_orm::DbErr) -> Self {
        Self::Mail(e.into())
    }
}

/// The person objects that use `address` as an identifier, those the
/// caller sees (`hide`); none when the org has no such types.
async fn people(
    db: &DatabaseConnection,
    org_id: i64,
    address: &str,
    hide: &Redaction,
) -> Vec<String> {
    let q = OntologyQuery {
        type_key: Some("identifier".into()),
        filters: vec![PropertyFilter {
            key: "value".into(),
            op: FilterOp::Eq,
            value: json!(address),
        }],
        traverse: vec![Hop {
            link_type: "uses_identifier".into(),
            direction: Direction::In,
        }],
        limit: Some(5),
        ..Default::default()
    };
    match crate::services::ontology::query::run(db, None, org_id, &q, *hide).await {
        Ok(r) => r.rows.into_iter().map(|row| row.object.id).collect(),
        Err(_) => Vec::new(),
    }
}

/// Adds the invitation of message `message` to the org calendar (or
/// updates or cancels the meeting already there) and returns the range.
pub async fn add_to_calendar(
    db: &DatabaseConnection,
    org_id: i64,
    caller: Caller,
    hide: &Redaction,
    mailbox_id: Uuid,
    message: Uuid,
) -> Result<TimeRange, InviteError> {
    let access = Access::of(db, org_id, caller).await?;
    let row = find(db, org_id, mailbox_id)
        .await
        .map_err(InviteError::Mail)?;
    if !access.reads(&row) {
        return Err(InviteError::Mail(MailError::NotFound));
    }
    #[derive(FromQueryResult)]
    struct Stored {
        subject: String,
        invitation: Option<Value>,
    }
    let stored = Stored::find_by_statement(stmt(
        "SELECT subject, invitation FROM mail_messages \
         WHERE id = $1 AND org_id = $2 AND mailbox_id = $3",
        vec![message.into(), org_id.into(), mailbox_id.into()],
    ))
    .one(db)
    .await?
    .ok_or(InviteError::NotFound)?;
    let inv: MailInvitation = stored
        .invitation
        .and_then(|v| serde_json::from_value(v).ok())
        .ok_or(InviteError::NotFound)?;
    let mut who: Vec<Value> = Vec::new();
    for (role, a) in inv
        .organizer
        .iter()
        .map(|a| ("organizer", a))
        .chain(inv.attendees.iter().map(|a| ("attendee", a)))
    {
        // Google lists the organizer among the attendees too.
        if who.iter().any(|w| w["address"] == json!(a.address)) {
            continue;
        }
        let MailAddress { name, address } = a;
        who.push(json!({
            "role": role,
            "name": name,
            "address": address,
            "object_ids": people(db, org_id, address, hide).await,
        }));
    }
    let mut attrs = json!({
        "source": "mail",
        "mail_id": message.to_string(),
        "people": who,
    });
    if let Some(l) = &inv.location {
        attrs["location"] = json!(l);
    }
    if inv.cancelled() {
        attrs["status"] = json!("cancelled");
    }
    let input = TimeRangeInput {
        label: Some(if inv.summary.is_empty() {
            stored.subject.chars().take(200).collect()
        } else {
            inv.summary.clone()
        }),
        external_id: Some(inv.uid.clone()),
        category: Some(CATEGORY.into()),
        planned_start: inv.start.clone(),
        planned_end: inv.end.clone(),
        attrs: Some(attrs),
        ..Default::default()
    };
    let scope = time_ranges::resolve_scope(db, org_id, "org", "")
        .await
        .map_err(InviteError::Range)?;
    let writer = Writer {
        origin: RangeOrigin::Import,
        source_ref: Some(format!("mail:{message}")),
    };
    let (range, _) = time_ranges::upsert(db, org_id, &scope, &input, &writer)
        .await
        .map_err(InviteError::Range)?;
    Ok(time_ranges::view(&range))
}
