//! Mailboxes (mail.md D453–D461): a mailbox is a source of the org, owned
//! by a user (a person or a service account) or shared with teams; its new
//! messages are stored and published to the flows listening to it
//! (`mail_received`). The rules that keep a mail an untrusted datum (text
//! only, bounded, authentication read) and that break loops (automatic
//! replies flagged, our own mails recognized) are pure functions here.
//! serde-only, wasm-safe.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// IMAPS port (TLS from the first byte; STARTTLS is not offered).
pub const IMAPS_PORT: u16 = 993;
pub const NAME_MAX: usize = 120;
pub const ADDRESS_MAX: usize = 320;
pub const HOST_MAX: usize = 253;
pub const FOLDERS_MAX: usize = 10;
pub const FOLDER_MAX: usize = 255;
pub const TEAMS_MAX: usize = 20;
pub const RETENTION_DAYS_DEFAULT: i32 = 90;
pub const RETENTION_DAYS_MIN: i32 = 7;
pub const RETENTION_DAYS_MAX: i32 = 3650;
/// A raw message above this size keeps its headers only (D458).
pub const RAW_MAX_BYTES: usize = 10 * 1024 * 1024;
/// Text kept per message (characters).
pub const TEXT_MAX: usize = 100_000;
/// Recipients kept per field.
pub const RECIPIENTS_MAX: usize = 100;
pub const SUBJECT_MAX: usize = 998;
/// Messages taken per mailbox and per run of the poller (D457.4).
pub const FETCH_BATCH: usize = 200;
/// Text sent to a flow with each message (characters).
pub const EVENT_TEXT_MAX: usize = 20_000;
/// Header naming the org, mailbox and message a mail sent by PneX comes
/// from (D457.2).
pub const ORIGIN_HEADER: &str = "X-PneX-Origin";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum MailboxState {
    #[default]
    Active,
    Paused,
    Error,
}

impl MailboxState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [Self::Active, Self::Paused, Self::Error]
            .into_iter()
            .find(|k| k.as_str() == s)
    }
}

/// Who owns a mailbox (D454): a member or a service account (an agent, an
/// application); `None` on the mailbox = shared with its teams.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MailboxOwner {
    pub user_id: i64,
    pub name: String,
    /// A service account (agent, application), not a person.
    pub service: bool,
}

/// Why the poller stopped, as a machine code and its arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MailboxError {
    pub code: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub args: Value,
}

/// A mailbox as read: never its password, only whether one is stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Mailbox {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub name: String,
    pub address: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<MailboxOwner>,
    /// Teams reading a shared mailbox.
    #[serde(default)]
    pub teams: Vec<String>,
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_user: String,
    /// A password is stored in the vault.
    pub has_password: bool,
    pub folders: Vec<String>,
    pub state: MailboxState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<MailboxError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub last_sync_at: Option<String>,
    pub retention_days: i32,
    /// The caller reads its messages (D454).
    pub readable: bool,
    pub version: i32,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub updated_at: String,
}

/// Create or replace a mailbox. `owner_user_id` absent = the caller's own
/// mailbox; `shared: true` = no owner, read by `teams`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MailboxInput {
    pub name: String,
    pub address: String,
    #[serde(default)]
    pub shared: bool,
    /// Another owner than the caller (a service account): admins only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_user_id: Option<i64>,
    #[serde(default)]
    pub teams: Vec<String>,
    pub imap_host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imap_port: Option<u16>,
    pub imap_user: String,
    /// A vault secret or a typed value; absent on update keeps the current.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<crate::SecretFieldInput>,
    #[serde(default)]
    pub folders: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_days: Option<i32>,
    #[serde(default)]
    pub paused: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i32>,
}

/// Result of `POST /mailboxes/{id}/test`: nothing is fetched nor stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MailboxTestResult {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<MailboxError>,
    /// Messages in each tested folder.
    #[serde(default)]
    pub folders: Vec<MailFolderCount>,
    /// The server's spam folder when the mailbox does not poll it: a mail
    /// of a customer filed as spam would never reach PneX.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub junk_folder: Option<String>,
    pub latency_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MailFolderCount {
    pub folder: String,
    pub messages: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MailAddress {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub address: String,
}

/// Verdict of one authentication method of the sender.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum AuthVerdict {
    Pass,
    Fail,
    #[default]
    None,
}

/// What PneX verified of the sender (D458): DKIM aligned on the From
/// domain; SPF stays `none` (a mailbox never knows the connecting IP).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MailAuth {
    pub spf: AuthVerdict,
    pub dkim: AuthVerdict,
    pub dmarc: AuthVerdict,
}

impl MailAuth {
    /// Trusted enough to link the sender automatically: DMARC passed, or
    /// without DMARC, SPF or DKIM passed and nothing failed.
    pub fn trusted(&self) -> bool {
        use AuthVerdict::*;
        match self.dmarc {
            Pass => true,
            Fail => false,
            None => {
                (self.spf == Pass || self.dkim == Pass) && self.spf != Fail && self.dkim != Fail
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MailAttachment {
    pub name: String,
    pub content_type: String,
    pub size: u64,
    /// The media it was saved as by a reader of the mailbox (mail.md M5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_id: Option<String>,
}

/// The original of a forwarded message (D463): its sender, date and
/// subject, read from the attached message or from the forward block of the
/// text. Filed under the original sender only when `trusted`: the mailbox
/// that forwarded it is a member's address and passed authentication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MailForward {
    pub from: MailAddress,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub subject: String,
    pub trusted: bool,
}

/// One stored message (D455). Text only: never HTML (D458).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MailMessage {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub mailbox_id: String,
    pub folder: String,
    pub message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<String>,
    pub from: MailAddress,
    #[serde(default)]
    pub to: Vec<MailAddress>,
    #[serde(default)]
    pub cc: Vec<MailAddress>,
    pub subject: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub date: Option<String>,
    pub text: String,
    /// The text was cut at [`TEXT_MAX`], or the message was too large and
    /// only its headers were kept.
    pub truncated: bool,
    #[serde(default)]
    pub attachments: Vec<MailAttachment>,
    pub auth: MailAuth,
    /// An automatic reply, a list, a bounce (D457.1).
    pub automatic: bool,
    /// Sent by PneX or by a mailbox of the org (D457.2).
    pub from_pnex: bool,
    /// A forward by a member: file it under `forwarded.from` (D463).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forwarded: Option<MailForward>,
    /// The meeting invitation it carries (D466).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invitation: Option<MailInvitation>,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub received_at: String,
}

/// The first event of the `text/calendar` part of a mail (D466): what a
/// reader adds to the calendar. Times are RFC 3339 in UTC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MailInvitation {
    /// iCalendar `UID`: an update or a cancellation names the same one.
    pub uid: String,
    /// `METHOD` of the calendar (`REQUEST`, `CANCEL`, `PUBLISH`, `REPLY`…).
    pub method: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organizer: Option<MailAddress>,
    #[serde(default)]
    pub attendees: Vec<MailAddress>,
}

impl MailInvitation {
    /// The meeting is called off: `METHOD:CANCEL` or `STATUS:CANCELLED`
    /// (kept in `method`).
    pub fn cancelled(&self) -> bool {
        self.method.eq_ignore_ascii_case("CANCEL")
    }
}

impl MailMessage {
    /// Whom the message is about: the original sender of a trusted
    /// forward, else the sender.
    pub fn correspondent(&self) -> &MailAddress {
        match &self.forwarded {
            Some(f) if f.trusted => &f.from,
            _ => &self.from,
        }
    }
}

/// `mail_received` (D456): the mailboxes listened to, and simple filters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailReceivedConfig {
    #[serde(default)]
    pub mailboxes: Vec<String>,
    /// Folder name; empty = every polled folder.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub folder: String,
    /// Case- and accent-insensitive part of the sender's address or name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub from_contains: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub subject_contains: String,
    /// Automatic replies, lists and our own mails are left out (D457).
    #[serde(default = "yes")]
    pub skip_automatic: bool,
}

fn yes() -> bool {
    true
}

impl Default for MailReceivedConfig {
    fn default() -> Self {
        Self {
            mailboxes: Vec::new(),
            folder: String::new(),
            from_contains: String::new(),
            subject_contains: String::new(),
            skip_automatic: true,
        }
    }
}

impl MailReceivedConfig {
    /// `(code, message)` when the node cannot be deployed as configured.
    pub fn check_deployable(&self) -> Option<(&'static str, &'static str)> {
        if self.mailboxes.is_empty() {
            return Some((
                crate::err_codes::MAIL_NO_MAILBOX,
                "mail_received names no mailbox",
            ));
        }
        if self.mailboxes.len() > 20
            || self
                .mailboxes
                .iter()
                .any(|m| uuid::Uuid::parse_str(m).is_err())
        {
            return Some((
                crate::err_codes::MAIL_MAILBOX_UNKNOWN,
                "mail_received names an invalid mailbox id",
            ));
        }
        None
    }

    /// Whether `m` passes the node's filters.
    pub fn accepts(&self, m: &MailMessage) -> bool {
        use crate::fold::fold;
        if !self.mailboxes.iter().any(|id| id == &m.mailbox_id) {
            return false;
        }
        if self.skip_automatic && (m.automatic || m.from_pnex) {
            return false;
        }
        if !self.folder.is_empty() && !self.folder.eq_ignore_ascii_case(&m.folder) {
            return false;
        }
        let sender = fold(&format!("{} {}", m.from.name, m.from.address));
        if !self.from_contains.is_empty() && !sender.contains(&fold(&self.from_contains)) {
            return false;
        }
        if !self.subject_contains.is_empty()
            && !fold(&m.subject).contains(&fold(&self.subject_contains))
        {
            return false;
        }
        true
    }
}

/// Valkey channel of the new messages of an org (the node filters by
/// mailbox; the org boundary is the channel).
pub fn mail_channel(org_id: i64) -> String {
    format!("pnex:mail:v1:{org_id}")
}

/// Published once per new message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailEvent {
    pub message: MailMessage,
}

/// Stored mails replayed to a `mail_received` node when it starts (D464):
/// at most this many…
pub const REPLAY_MAX: usize = 200;
/// …received in the last days only.
pub const REPLAY_DAYS: i64 = 7;

/// `POST /internal/flow/mail/replay`: the mails the node missed since its
/// cursor (none on its first start: the cursor begins then).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailReplayRequest {
    pub node: crate::ontology::flow::FlowNodeRef,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailReplayReply {
    /// Oldest first, already filtered by the node's deployed config.
    pub messages: Vec<MailMessage>,
}

/// `POST /internal/flow/mail/ack`: the node emitted this mail; its cursor
/// moves forward (never back).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MailAckRequest {
    pub node: crate::ontology::flow::FlowNodeRef,
    /// The mail's id (`MailMessage.id`).
    pub id: String,
}

// ───────────────────────────── loops (D457) ─────────────────────────────

/// Whether the headers mark an automatic message: an auto-reply, a list, a
/// bulk send or a delivery report. `headers` = `(name, value)`.
pub fn is_automatic(headers: &[(String, String)], from: &str, content_type: &str) -> bool {
    let get = |n: &'static str| {
        headers
            .iter()
            .filter(move |(k, _)| k.eq_ignore_ascii_case(n))
            .map(|(_, v)| v.trim().to_ascii_lowercase())
    };
    if get("auto-submitted").any(|v| v != "no") {
        return true;
    }
    if get("precedence").any(|v| matches!(v.as_str(), "bulk" | "auto_reply" | "list" | "junk")) {
        return true;
    }
    for h in [
        "x-autoreply",
        "x-autorespond",
        "x-auto-response-suppress",
        "list-id",
        "list-unsubscribe",
        "x-failed-recipients",
    ] {
        if get(h).next().is_some() {
            return true;
        }
    }
    // Robot senders without the headers (a hosting panel, a no-reply).
    let local: String = from
        .split('@')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    if matches!(
        local.as_str(),
        "mailerdaemon" | "postmaster" | "bounce" | "bounces"
            // Hosting panels and cron mail; never `alerts@`: a machine that
            // alerts by mail is a wanted source (D453).
            | "cpanel" | "whm" | "plesk" | "root" | "cron" | "daemon"
    ) || ["noreply", "donotreply", "nepasrepondre"]
        .iter()
        .any(|p| local.starts_with(p))
    {
        return true;
    }
    content_type
        .to_ascii_lowercase()
        .starts_with("multipart/report")
}

/// Whether a message carries our origin header, or comes from one of the
/// org's own addresses.
pub fn is_from_pnex(headers: &[(String, String)], from: &str, own: &[String]) -> bool {
    headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case(ORIGIN_HEADER))
        || own.iter().any(|a| a.eq_ignore_ascii_case(from.trim()))
}

// ─────────────────────────── forwards (D463) ───────────────────────────

/// `Name <a@b.fr>`, `a@b.fr`, `Name [mailto:a@b.fr]`, `"Name" <a@b.fr>`.
pub fn parse_address(raw: &str) -> Option<MailAddress> {
    let raw = raw.trim();
    let (name, addr) = match (raw.rfind('<'), raw.rfind('>')) {
        (Some(a), Some(b)) if a < b => (&raw[..a], &raw[a + 1..b]),
        _ => match raw.find("[mailto:") {
            Some(a) => (&raw[..a], raw[a + 8..].trim_end_matches(']')),
            None => ("", raw),
        },
    };
    let addr = addr.trim().trim_start_matches("mailto:");
    let (local, domain) = addr.split_once('@')?;
    if local.is_empty()
        || !domain.contains('.')
        || addr.contains(char::is_whitespace)
        || addr.len() > ADDRESS_MAX
    {
        return None;
    }
    Some(MailAddress {
        name: name.trim().trim_matches('"').trim().to_string(),
        address: addr.to_ascii_lowercase(),
    })
}

/// Marker lines that open a forward block (Gmail, Outlook, Apple Mail,
/// Thunderbird; English and French).
const FORWARD_MARKERS: &[&str] = &[
    "forwarded message",
    "message transféré",
    "message transfere",
    "begin forwarded message",
    "début du message réexpédié",
    "debut du message reexpedie",
    "original message",
    "message d'origine",
    "message original",
];

/// Subject prefixes of a forward.
fn forward_subject(subject: &str) -> bool {
    let s = subject.trim_start().to_ascii_lowercase();
    ["fwd:", "fw:", "tr:", "tr :", "fwd :", "fw :", "wg:"]
        .iter()
        .any(|p| s.starts_with(p))
}

/// The header lines of a forward block: `(field, value)` for From, Date,
/// Subject in the forms the mail clients write them.
fn forward_field(line: &str) -> Option<(&'static str, String)> {
    let l = line
        .trim()
        .trim_start_matches(['>', '*', ' '])
        .trim_end_matches('*');
    let (k, v) = l.split_once(':')?;
    let key = k.trim().trim_matches('*').trim().to_lowercase();
    let field = match key.as_str() {
        "from" | "de" | "von" => "from",
        "date" | "sent" | "envoyé" | "envoye" | "gesendet" => "date",
        "subject" | "objet" | "sujet" | "betreff" => "subject",
        _ => return None,
    };
    Some((field, v.trim().to_string()))
}

/// A forwarded date as RFC 3339, or `None`: the block's date is free text
/// and a date property refuses it (shortcut: localized forms such as
/// « lun. 26 oct. 2026 à 13:58 » are dropped, the forward's own date
/// stands; parse them if the original time matters).
fn forward_date(raw: &str) -> Option<String> {
    let raw = raw.trim();
    chrono::DateTime::parse_from_rfc2822(raw)
        .or_else(|_| chrono::DateTime::parse_from_rfc3339(raw))
        .ok()
        .map(|d| d.to_rfc3339())
}

/// The original sender, date and subject of a forward written in the text
/// of the message (`trusted` is decided by the caller). `None` when the
/// text holds no forward block, or the subject is no forward and no marker
/// line opens one.
pub fn parse_forward_block(subject: &str, text: &str) -> Option<MailForward> {
    parse_forward_chain(subject, text).into_iter().next()
}

/// Hops of a forward chain at most (a forward of a forward…).
pub const FORWARD_HOPS_MAX: usize = 10;

/// Every forward block of the text, the latest first: a mail forwarded
/// then forwarded again holds one block per hop.
pub fn parse_forward_chain(subject: &str, text: &str) -> Vec<MailForward> {
    let lines: Vec<&str> = text.lines().collect();
    let is_marker = |l: &str| {
        let low = crate::fold::fold(l);
        FORWARD_MARKERS
            .iter()
            .any(|m| low.contains(&crate::fold::fold(m)))
            || (l.trim().len() >= 20 && l.trim().chars().all(|c| c == '_' || c == '-'))
    };
    let mut chain = Vec::new();
    let mut from = 0;
    while chain.len() < FORWARD_HOPS_MAX {
        let marker = lines
            .iter()
            .skip(from)
            .position(|l| is_marker(l))
            .map(|i| i + from);
        let start = match marker {
            Some(m) => m + 1,
            // Only the first hop may go without a marker (a « Fwd: » subject).
            None if chain.is_empty() && forward_subject(subject) => 0,
            None => break,
        };
        let Some((fwd, from_at)) = block_at(&lines, start) else {
            break;
        };
        chain.push(fwd);
        from = from_at + 1;
    }
    chain
}

/// The block whose `From` line follows `start`, and that line's index.
fn block_at(lines: &[&str], start: usize) -> Option<(MailForward, usize)> {
    let from_at = lines
        .iter()
        .enumerate()
        .skip(start)
        .take(40)
        .find(|(_, l)| forward_field(l).is_some_and(|(f, _)| f == "from"))
        .map(|(i, _)| i)?;
    let (_, raw_from) = forward_field(lines[from_at])?;
    let from = parse_address(&raw_from)?;
    let mut fwd = MailForward {
        from,
        date: None,
        subject: String::new(),
        trusted: false,
    };
    for l in lines.iter().skip(from_at + 1).take(8) {
        match forward_field(l) {
            Some(("date", v)) if fwd.date.is_none() => fwd.date = forward_date(&v),
            Some(("subject", v)) if fwd.subject.is_empty() => fwd.subject = v,
            _ => {}
        }
    }
    Some((fwd, from_at))
}

/// The original of a forward chain (D463): the first hop written by someone
/// outside the org, the members' own hops skipped (a colleague who passed
/// the mail on); every hop a member's: the latest. `members` in lower case.
pub fn forward_origin(chain: Vec<MailForward>, members: &[String]) -> Option<MailForward> {
    let inside = |f: &MailForward| members.iter().any(|m| m == &f.from.address);
    let first_outside = chain.iter().position(|f| !inside(f)).unwrap_or(0);
    chain.into_iter().nth(first_outside)
}

// ─────────────────────────── untrusted content (D458) ───────────────────────────

/// Plain text of an HTML body: tags dropped (scripts and styles with their
/// content), block ends as new lines, common entities decoded. Never a
/// renderer: the result is shown as text.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let lower = html.to_ascii_lowercase();
    let mut i = 0;
    let bytes = html.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'<' {
            let Some(end) = html[i..].find('>') else {
                break;
            };
            let tag = &lower[i + 1..i + end];
            let name: String = tag
                .trim_start_matches('/')
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            if !tag.starts_with('/') && matches!(name.as_str(), "script" | "style" | "head") {
                let close = format!("</{name}");
                match lower[i..].find(&close) {
                    Some(c) => {
                        i += c;
                        i += lower[i..].find('>').map_or(lower.len() - i, |e| e + 1);
                        continue;
                    }
                    None => break,
                }
            }
            if matches!(
                name.as_str(),
                "br" | "p"
                    | "div"
                    | "tr"
                    | "li"
                    | "h1"
                    | "h2"
                    | "h3"
                    | "h4"
                    | "h5"
                    | "h6"
                    | "table"
                    | "blockquote"
            ) {
                out.push('\n');
            }
            i += end + 1;
            continue;
        }
        let next = html[i..].find('<').map_or(html.len(), |n| i + n);
        out.push_str(&decode_entities(&html[i..next]));
        i = next;
    }
    out
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(semi) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..semi];
        let ch = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            e if e.starts_with("#x") || e.starts_with("#X") => u32::from_str_radix(&e[2..], 16)
                .ok()
                .and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Text as stored: control characters dropped (new lines and tabs kept),
/// runs of blank lines collapsed, cut at `max` characters. Returns the text
/// and whether it was cut.
pub fn clean_text(s: &str, max: usize) -> (String, bool) {
    let mut out = String::with_capacity(s.len().min(max * 4));
    let mut blank = 0;
    let mut n = 0;
    for line in s.lines() {
        let line: String = line
            .chars()
            .filter(|c| !c.is_control() || *c == '\t')
            .map(|c| if c == '\u{a0}' { ' ' } else { c })
            .collect();
        let line = line.trim_end();
        if line.is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        for c in line.chars().chain(std::iter::once('\n')) {
            if n == max {
                return (out.trim_end().to_string(), true);
            }
            out.push(c);
            n += 1;
        }
    }
    (out.trim().to_string(), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn automatic_messages_are_recognized() {
        assert!(is_automatic(
            &h(&[("Auto-Submitted", "auto-replied")]),
            "a@b.fr",
            "text/plain"
        ));
        assert!(!is_automatic(
            &h(&[("Auto-Submitted", "no")]),
            "a@b.fr",
            "text/plain"
        ));
        assert!(is_automatic(
            &h(&[("Precedence", "bulk")]),
            "a@b.fr",
            "text/plain"
        ));
        assert!(is_automatic(
            &h(&[("List-Unsubscribe", "<mailto:x>")]),
            "a@b.fr",
            "text/plain"
        ));
        assert!(is_automatic(&[], "MAILER-DAEMON@mx.fr", "text/plain"));
        assert!(is_automatic(&[], "no-reply@acme.fr", "text/plain"));
        assert!(is_automatic(&[], "Ne-Pas-Repondre@impots.fr", "text/plain"));
        assert!(!is_automatic(&[], "noemie@acme.fr", "text/plain"));
        assert!(is_automatic(&[], "cpanel@radis.o2switch.net", "text/plain"));
        assert!(!is_automatic(&[], "alerts@usine.fr", "text/plain"));
        assert!(is_automatic(
            &[],
            "a@b.fr",
            "multipart/report; report-type=delivery-status"
        ));
        assert!(!is_automatic(
            &h(&[("Subject", "Bonjour")]),
            "client@acme.fr",
            "text/plain"
        ));
    }

    #[test]
    fn our_own_mails_are_recognized() {
        let own = vec!["sales@pnex.io".to_string()];
        assert!(is_from_pnex(
            &h(&[("X-PneX-Origin", "1/m/x")]),
            "c@acme.fr",
            &own
        ));
        assert!(is_from_pnex(&[], "SALES@pnex.io", &own));
        assert!(!is_from_pnex(&[], "c@acme.fr", &own));
    }

    #[test]
    fn only_a_pass_without_failure_is_trusted() {
        use AuthVerdict::*;
        let auth = |spf, dkim, dmarc| MailAuth { spf, dkim, dmarc };
        assert!(auth(None, Pass, Pass).trusted());
        assert!(!auth(Pass, Pass, Fail).trusted());
        assert!(
            !MailAuth::default().trusted(),
            "nothing verified: not trusted"
        );
        assert!(!auth(Pass, Fail, None).trusted());
    }

    #[test]
    fn html_becomes_text_without_scripts() {
        let t = html_to_text("<html><head><title>x</title></head><body><p>Bonjour&nbsp;Élodie,</p><script>alert(1)</script><div>Prix&nbsp;: 10&euro; &lt;b&gt;</div><style>p{}</style>Fin&#33;</body></html>");
        assert!(t.contains("Bonjour Élodie,"), "{t}");
        assert!(
            t.contains("<b>") && !t.contains("alert") && !t.contains("p{}") && !t.contains("title"),
            "{t}"
        );
        assert!(t.contains("Fin!"), "{t}");
        assert!(t.contains("&euro;"), "unknown entities stay as text: {t}");
        assert_eq!(html_to_text("a < b"), "a ");
    }

    #[test]
    fn addresses_are_read_in_every_client_form() {
        let a = parse_address("\"Élodie Martin\" <Elodie@Acme.fr>").unwrap();
        assert_eq!(
            (a.name.as_str(), a.address.as_str()),
            ("Élodie Martin", "elodie@acme.fr")
        );
        assert_eq!(
            parse_address("Martin, Élodie [mailto:e.m@acme.fr]")
                .unwrap()
                .address,
            "e.m@acme.fr"
        );
        assert_eq!(parse_address("bob@acme.fr").unwrap().name, "");
        assert!(parse_address("not an address").is_none());
        assert!(parse_address("a@localhost").is_none());
    }

    #[test]
    fn forwards_are_read_from_gmail_outlook_and_apple() {
        let gmail = "Voir ci-dessous\n\n---------- Forwarded message ---------\nFrom: Élodie Martin <elodie@acme.fr>\nDate: Mon, 26 Oct 2026 at 09:12\nSubject: Devis ligne 3\nTo: <bob@example.com>\n\nBonjour";
        let f = parse_forward_block("Fwd: Devis ligne 3", gmail).unwrap();
        assert_eq!(f.from.address, "elodie@acme.fr");
        assert_eq!(f.subject, "Devis ligne 3");
        // A localized date is dropped: it would be refused as a date.
        assert_eq!(f.date, None);
        assert!(!f.trusted, "trust is decided by the server");

        let outlook_fr = "\n________________________________\nDe : Martin Élodie <elodie@acme.fr>\nEnvoyé : lundi 26 octobre 2026 09:12\nÀ : Bob <bob@example.com>\nObjet : Devis ligne 3\n\nBonjour";
        let f = parse_forward_block("TR: Devis ligne 3", outlook_fr).unwrap();
        assert_eq!(
            (f.from.address.as_str(), f.subject.as_str()),
            ("elodie@acme.fr", "Devis ligne 3")
        );
        assert_eq!(f.date, None);
        let rfc = "---------- Forwarded message ---------\nFrom: e@acme.fr\nDate: Mon, 26 Oct 2026 09:12:00 +0100\n";
        assert_eq!(
            parse_forward_block("Fwd: x", rfc).unwrap().date.as_deref(),
            Some("2026-10-26T09:12:00+01:00")
        );

        let outlook_en = "From: Martin, Elodie [mailto:elodie@acme.fr]\nSent: Monday, October 26, 2026 9:12 AM\nTo: Bob\nSubject: Quote";
        assert_eq!(
            parse_forward_block("FW: Quote", outlook_en)
                .unwrap()
                .from
                .address,
            "elodie@acme.fr"
        );

        let apple = "Début du message réexpédié :\n\nDe: Élodie <elodie@acme.fr>\nObjet: Devis\nDate: 26 octobre 2026 à 09:12:00 UTC+1";
        assert_eq!(
            parse_forward_block("Devis", apple).unwrap().subject,
            "Devis"
        );

        // A plain mail quoting a header-like line is no forward.
        assert!(parse_forward_block("Question", "From: the start\nwe agreed").is_none());
    }

    #[test]
    fn a_nested_forward_reaches_the_first_sender_outside_the_org() {
        // A client wrote to a colleague, who forwarded it to Bob, who
        // forwarded it to the mailbox (Gmail in French, as received).
        let text = "---------- Forwarded message ---------\nDe : Alice <alice@example.com>\nDate: lun. 26 oct. 2026 à 15:36\nSubject: Fwd: Devis\n\n\
---------- Forwarded message ---------\nDe : Élodie Martin <elodie@acme.fr>\nDate: Mon, 26 Oct 2026 09:12:00 +0100\nSubject: Devis\n\nBonjour";
        let chain = parse_forward_chain("Fwd: Fwd: Devis", text);
        let hops: Vec<&str> = chain.iter().map(|f| f.from.address.as_str()).collect();
        assert_eq!(hops, ["alice@example.com", "elodie@acme.fr"]);
        let members = ["bob@example.com".to_string(), "alice@example.com".to_string()];
        let origin = forward_origin(chain.clone(), &members).unwrap();
        assert_eq!(origin.from.address, "elodie@acme.fr");
        assert_eq!(origin.date.as_deref(), Some("2026-10-26T09:12:00+01:00"));
        // Alice outside the org: she is the one who sent it to Bob.
        assert_eq!(
            forward_origin(chain.clone(), &members[..1])
                .unwrap()
                .from
                .address,
            "alice@example.com"
        );
        // Only members: the latest hop.
        let all = [members.to_vec(), vec!["elodie@acme.fr".to_string()]].concat();
        assert_eq!(
            forward_origin(chain, &all).unwrap().from.address,
            "alice@example.com"
        );
        assert!(forward_origin(Vec::new(), &all).is_none());
    }

    #[test]
    fn a_trusted_forward_files_the_original_sender() {
        let mut m = sample();
        assert_eq!(m.correspondent().address, "elodie@acme.fr");
        m.forwarded = Some(MailForward {
            from: MailAddress {
                name: String::new(),
                address: "client@beta.fr".into(),
            },
            date: None,
            subject: String::new(),
            trusted: false,
        });
        assert_eq!(
            m.correspondent().address,
            "elodie@acme.fr",
            "untrusted: the sender stays"
        );
        m.forwarded.as_mut().unwrap().trusted = true;
        assert_eq!(m.correspondent().address, "client@beta.fr");
    }

    #[test]
    fn text_is_cleaned_and_bounded() {
        let (t, cut) = clean_text("a\u{0}b\r\n\n\n\nc\t d  \n", 100);
        assert_eq!((t.as_str(), cut), ("ab\n\nc\t d", false));
        let (t, cut) = clean_text(&"x".repeat(50), 10);
        assert_eq!((t.len(), cut), (10, true));
    }

    fn sample() -> MailMessage {
        MailMessage {
            id: "1".into(),
            mailbox_id: "mb".into(),
            folder: "INBOX".into(),
            message_id: "<a@b>".into(),
            in_reply_to: None,
            references: vec![],
            from: MailAddress {
                name: "Élodie Martin".into(),
                address: "elodie@acme.fr".into(),
            },
            to: vec![],
            cc: vec![],
            subject: "Devis été".into(),
            date: None,
            text: String::new(),
            truncated: false,
            attachments: vec![],
            auth: MailAuth::default(),
            automatic: false,
            from_pnex: false,
            forwarded: None,
            invitation: None,
            received_at: String::new(),
        }
    }

    #[test]
    fn the_trigger_filters_by_mailbox_sender_and_loops() {
        let m = sample();
        let mut c = MailReceivedConfig {
            mailboxes: vec!["mb".into()],
            ..Default::default()
        };
        assert!(c.accepts(&m));
        c.from_contains = "ELODIE".into();
        c.subject_contains = "devis ete".into();
        assert!(c.accepts(&m));
        assert!(!c.accepts(&MailMessage {
            automatic: true,
            ..m.clone()
        }));
        assert!(!c.accepts(&MailMessage {
            mailbox_id: "other".into(),
            ..m.clone()
        }));
        c.folder = "Archive".into();
        assert!(!c.accepts(&m));
        assert!(MailReceivedConfig::default().check_deployable().is_some());
    }
}
