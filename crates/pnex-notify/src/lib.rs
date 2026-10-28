//! PNEX notify — multi-channel notification abstraction (D49–D54,
//! amended 2026-09-14).
//!
//! **A single send path**: the backend (Test buttons, direct sends) and the
//! `pnex-notify` flow node (snapshot resolved at deploy) both go
//! through [`channel`] + [`Channel::send`] + [`with_retry`] — no duplicated
//! send code.
//!
//! Zero loco dep: linked by the backend, by `pnex-node-notify` (edgelinkd
//! runtime) and testable in isolation. Target scope = a few proven
//! channels, not Apprise coverage:
//!
//! | Kind | Transport | Secrets |
//! |---|---|---|
//! | `websocket` | `POST deliver_url` (internal bus `/internal/notify/deliver`) | token injected by env, never in DB nor flows.json |
//! | `webhook` | `reqwest` POST JSON `{subject, body, meta}` | `secret_value` in optional header |
//! | `ntfy` | `reqwest` JSON publish `{topic, message, title, …}` | token as optional `Bearer` |
//! | `telegram` | Bot API `sendMessage` (JSON `{chat_id, text}`) | bot_token write-only |
//! | `slack` | incoming webhook `{"text": …}` | `webhook_url` write-only (carries the token) |
//! | `discord` | webhook `{"content": …}` | `webhook_url` write-only (carries the token) |
//! | `smtp` | lettre 0.11 (rustls) | `password` write-only |
//!
//! (gotify/… = extensions of ~30 lines each — implement
//! [`Channel`] + an entry in [`registry::CHANNELS`].)

pub mod attachments;
pub mod channels;
pub mod error;
pub mod journal;
pub mod registry;
pub mod render;
pub mod retry;
pub mod secrets;

pub use error::NotifyError;
pub use registry::{channel, kinds, mask_config, merge_config, secrets_set};
pub use render::{render, template_vars};
pub use retry::{with_retry, with_retry_delays};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Rendered notification message — the same object goes to all the
/// channels; `meta` carries the traceability context (flow, node, ts, org).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub subject: Option<String>,
    pub body: String,
    #[serde(default)]
    pub meta: serde_json::Value,
}

/// A notification channel (destination). One implementation per kind —
/// same economy of extension points as the POI attachments
/// registry: 1 variant + 1 impl + 1 `field_spec` + 2 i18n keys.
#[async_trait]
pub trait Channel: Send + Sync {
    /// Kind identifier (`"websocket"`, `"webhook"`) — registry key.
    fn kind(&self) -> &'static str;
    /// Config field specification — source of the UI form
    /// (`notify-kinds.v1` contract, additive-only).
    fn field_spec(&self) -> Vec<pnex_core::FieldSpec>;
    /// Config validation at save time (displayable error
    /// message, never quoting a secret).
    fn validate(&self, config: &serde_json::Value) -> Result<(), String>;
    /// Sends a rendered message. `config` = channel config (snapshot at
    /// deploy for the flow node, DB config for backend sends).
    async fn send(&self, config: &serde_json::Value, msg: &Message) -> Result<(), NotifyError>;

    /// Send with attached files (D209: the PDF of a report). Default: the
    /// channel cannot carry files and sends the message alone; `smtp`
    /// attaches them.
    async fn send_with_attachments(
        &self,
        config: &serde_json::Value,
        msg: &Message,
        attachments: &[Attachment],
    ) -> Result<(), NotifyError> {
        let _ = attachments;
        self.send(config, msg).await
    }

    /// Whether [`Channel::send_with_attachments`] really attaches files.
    fn carries_attachments(&self) -> bool {
        false
    }
}

/// A file attached to a notification (D209).
#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    pub filename: String,
    pub content_type: String,
    pub data: Vec<u8>,
}

/// Hash journaled for a recipient taken from a message (D313): SHA-256
/// hex of the trimmed, lower-case address — never the address itself.
pub fn recipient_hash(address: &str) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(address.trim().to_lowercase().as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Largest attachment a notification carries, bytes.
pub const ATTACHMENT_MAX_BYTES: usize = 10 * 1024 * 1024;
