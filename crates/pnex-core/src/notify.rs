//! Notifications (D49–D54, amendées 2026-09-14) — DTO partagés
//! backend ↔ frontend ↔ runtime de flows.
//!
//! Contrat `notify-kinds.v1` : [`FieldSpec`] décrit les formulaires de
//! configuration des canaux, générés par le front depuis
//! `GET /api/v1/notify/kinds` (source de vérité = backend, zéro drift
//! front/back). Toute évolution est **additive-only** (école CONTRACT,
//! reste à 2).
//!
//! Deux canaux v1 : `websocket` (bus interne du serveur — tout client WS
//! authentifié s'abonne via `/ws/notify`) et `webhook` (POST générique,
//! couvre ntfy/Gotify/Telegram/domotique). ntfy/smtp dédiés = extensions
//! (~30 l chacune, registre prévu pour) ; push natif FCM/APNs reporté
//! (creds configurables par instance, produit self-hosté à distribution
//! unique).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Type d'un champ de formulaire de canal ([`FieldSpec`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    Text,
    /// Write-only : jamais rendu par un GET (`secrets_set` liste les
    /// champs définis, « remplacer » à l'édition) — D54.
    Secret,
    Number,
    Select,
    Bool,
}

/// Spécification d'un champ de formulaire de canal — source unique du
/// rendu front (contrat `notify-kinds.v1`, additif-only).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldSpec {
    pub id: String,
    /// Clé i18n front (`notify-field-…`), résolue côté client.
    pub label_i18n: String,
    pub r#type: FieldType,
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help_i18n: Option<String>,
    /// Choix d'un champ `select` (vide dès v1 — posé now pour rester
    /// additif en N4 ; select sans options = rendu en texte).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

/// Un kind de canal exposé par `GET /api/v1/notify/kinds`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotifyKindInfo {
    pub kind: String,
    /// Nom affichable (en) — le front garde son registre local d'icône +
    /// i18n `notify-kind-*` (labels/icons locaux, doctrine §3).
    pub label: String,
    pub field_spec: Vec<FieldSpec>,
}

/// Variable déclarée d'un template (`{name, example}`) — alimente le
/// picker UI et le fallback de l'aperçu.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateVar {
    pub name: String,
    pub example: String,
}

/// Canal de notification (destination) — DTO API. Les champs `secret` de
/// `config` ne sont **jamais** rendus (D54) : `secrets_set` liste leur
/// présence pour l'UI (« défini » / « remplacer »).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotifyChannel {
    pub id: Uuid,
    pub org_id: i64,
    pub kind: String,
    pub name: String,
    pub enabled: bool,
    pub config: serde_json::Value,
    pub secrets_set: Vec<String>,
    /// Vault reference of each set `secret` field (`field → {secret_id,
    /// name}`, secrets.md D113) — what the form shows, never a value.
    #[serde(default)]
    pub secrets: std::collections::BTreeMap<String, crate::SecretFieldView>,
    /// Dernière livraison connue (`sent`/`failed`) — badge carte.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_delivery_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Création / édition de canal. Les champs `secret` absents ou `null`
/// valent « inchangé » (merge au niveau registre, D54).
///
/// A `secret` field otherwise takes a [`crate::SecretFieldInput`]:
/// `{"secret_id": …}` (pick a vault secret) or `{"value": …}` (typed,
/// owner/admin; a bare string is accepted too). Empty string = cleared.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NotifyChannelInput {
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub config: serde_json::Value,
}

/// Template minijinja (sujet optionnel + corps) — DTO API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotifyTemplate {
    pub id: Uuid,
    pub org_id: i64,
    pub name: String,
    pub subject: Option<String>,
    pub body: String,
    #[serde(default)]
    pub vars: Vec<TemplateVar>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NotifyTemplateInput {
    pub name: String,
    #[serde(default)]
    pub subject: Option<String>,
    pub body: String,
    #[serde(default)]
    pub vars: Vec<TemplateVar>,
}

/// Paramètres d'aperçu : vars fournies, fallback = `example` déclarées.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PreviewInput {
    #[serde(default)]
    pub vars: std::collections::BTreeMap<String, String>,
    /// Payload d'exemple (contexte `msg` du rendu) — objet quelconque.
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// Résultat d'aperçu (rendu seul, jamais d'envoi).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreviewResult {
    pub subject: Option<String>,
    pub body: String,
}

/// OpenObserve logs stream holding the delivery journal (D86) — one stream
/// per org (the org is the O2 organization), no relational table.
pub const NOTIFY_DELIVERY_STREAM: &str = "notify_deliveries";

/// Delivery outcomes stored in the journal.
pub const DELIVERY_SENT: &str = "sent";
pub const DELIVERY_FAILED: &str = "failed";
/// Held back by the node's anti-spam window (nothing left the server).
pub const DELIVERY_BLOCKED: &str = "blocked";

/// One delivery attempt to journal — written by the backend (websocket
/// bus, Test buttons, OTA outcomes) and by the `pnex-notify` flow node
/// through `POST /internal/notify/journal` (other channel kinds).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NotifyDeliveryEntry {
    pub org_id: i64,
    pub channel_id: Uuid,
    #[serde(default)]
    pub channel_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_id: Option<Uuid>,
    /// `flow` | `test` | `ota` | `direct`.
    pub source: String,
    /// [`DELIVERY_SENT`] | [`DELIVERY_FAILED`] | [`DELIVERY_BLOCKED`].
    pub status: String,
    /// Upstream HTTP status of a failed send, when there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    /// Live websocket sessions reached (websocket channel only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered: Option<i64>,
}

/// Journal entry as read back — DTO API (`GET /api/v1/notify/deliveries`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotifyDelivery {
    /// O2 `_timestamp` (µs) — also the row key in the UI.
    pub ts_us: i64,
    pub channel_id: Uuid,
    #[serde(default)]
    pub channel_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_id: Option<Uuid>,
    pub source: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered: Option<i64>,
    /// RFC 3339 of `ts_us`.
    pub created_at: String,
}

/// Journal page: `available: false` = the org has no O2 organization yet
/// (nothing was ever journaled).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotifyDeliveryPage {
    pub available: bool,
    pub count: i64,
    pub results: Vec<NotifyDelivery>,
}

/// Frame du bus WS `/ws/notify` (texte JSON) — push temps réel d'une
/// livraison websocket vers les clients abonnés de l'org.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotifyItem {
    pub id: i64,
    pub subject: Option<String>,
    pub body: String,
    #[serde(default)]
    pub meta: serde_json::Value,
    pub created_at: String,
}

/// Référence notify périmée détectée au deploy (toast, non bloquant —
/// le nœud dégradé warn + passthrough, jamais de tab cassée).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StaleNotifyNode {
    pub flow_id: i64,
    pub node_id: String,
    /// `channel_missing` | `channel_disabled` | `template_missing` |
    /// `template_changed` (vars du template ≠ stamp du pick).
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Contrat `notify-kinds.v1` : FieldSpec roundtrip — l'ajout d'options
    /// doit rester rétrocompatible (défaut vide).
    #[test]
    fn field_spec_roundtrip_sans_options() {
        let json = r#"{"id":"url","label_i18n":"notify-field-url","type":"text","required":true}"#;
        let spec: FieldSpec = serde_json::from_str(json).expect("désérialisable");
        assert_eq!(spec.r#type, FieldType::Text);
        assert!(spec.options.is_empty());
        let back = serde_json::to_value(&spec).unwrap();
        assert_eq!(back["type"], "text");
    }

    #[test]
    fn secret_field_roundtrip() {
        let json = r#"{"id":"bot_token","label_i18n":"notify-field-bot-token","type":"secret","required":false}"#;
        let spec: FieldSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.r#type, FieldType::Secret);
    }
}
