//! PNEX notify — abstraction multi-canaux des notifications (D49–D54,
//! amendées 2026-09-14).
//!
//! **Un seul chemin d'envoi** : backend (boutons Test, envois directs) et
//! nœud flow `pnex-notify` (snapshot résolu au deploy) passent tous les
//! deux par [`channel`] + [`Channel::send`] + [`with_retry`] — pas de code
//! d'envoi dupliqué.
//!
//! Zéro dep loco : lié par le backend, par `pnex-node-notify` (runtime
//! edgelinkd) et testable isolément. Périmètre cible = quelques canaux
//! éprouvés, pas la couverture Apprise :
//!
//! | Kind | Transport | Secrets |
//! |---|---|---|
//! | `websocket` | `POST deliver_url` (bus interne `/internal/notify/deliver`) | token injecté par env, jamais en DB ni flows.json |
//! | `webhook` | `reqwest` POST JSON `{subject, body, meta}` | `secret_value` en header optionnel |
//! | `ntfy` | `reqwest` publication JSON `{topic, message, title, …}` | token en `Bearer` optionnel |
//! | `telegram` | Bot API `sendMessage` (JSON `{chat_id, text}`) | bot_token write-only |
//! | `slack` | incoming webhook `{"text": …}` | `webhook_url` write-only (porte le token) |
//! | `discord` | webhook `{"content": …}` | `webhook_url` write-only (porte le token) |
//! | `smtp` | lettre 0.11 (rustls) | `password` write-only |
//!
//! (gotify/… = extensions ~30 l chacune — implémenter
//! [`Channel`] + une entrée dans [`registry::CHANNELS`].)

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

/// Message de notification rendu — le même objet transite vers tous les
/// canaux ; `meta` porte le contexte de traçabilité (flow, nœud, ts, org).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub subject: Option<String>,
    pub body: String,
    #[serde(default)]
    pub meta: serde_json::Value,
}

/// Un canal de notification (destination). Une implémentation par kind —
/// même économie de points d'extension que le registre des attachments
/// POI : 1 variante + 1 impl + 1 `field_spec` + 2 clés i18n.
#[async_trait]
pub trait Channel: Send + Sync {
    /// Identifiant du kind (`"websocket"`, `"webhook"`) — clé du registre.
    fn kind(&self) -> &'static str;
    /// Spécification des champs de config — source du formulaire UI
    /// (contrat `notify-kinds.v1`, additif-only).
    fn field_spec(&self) -> Vec<pnex_core::FieldSpec>;
    /// Validation de la config à la sauvegarde (message d'erreur
    /// affichable, jamais de secret cité).
    fn validate(&self, config: &serde_json::Value) -> Result<(), String>;
    /// Envoi d'un message rendu. `config` = config du canal (snapshot au
    /// deploy pour le nœud flow, config DB pour les envois backend).
    async fn send(&self, config: &serde_json::Value, msg: &Message) -> Result<(), NotifyError>;
}
