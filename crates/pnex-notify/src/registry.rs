//! Channel registry (D51) — source of truth for the kinds: the
//! `GET /api/v1/notify/kinds` API and the front's dynamic form
//! derive from it (`notify-kinds.v1`, additive-only).
//!
//! Also hosts the D54 masking helpers: the `secret` fields of the
//! `field_spec` are write-only (masked GET, PUT null/absent = unchanged).

use crate::channels::{
    discord::DiscordChannel, ntfy::NtfyChannel, slack::SlackChannel, smtp::SmtpChannel,
    telegram::TelegramChannel, webhook::WebhookChannel, websocket::WebSocketChannel,
};
use crate::Channel;
use pnex_core::FieldType;

/// All shipped channel implementations — 1 entry per kind.
/// The order = display order of the front picker.
pub static CHANNELS: &[&dyn Channel] = &[
    &WebSocketChannel,
    &WebhookChannel,
    &NtfyChannel,
    &TelegramChannel,
    &SlackChannel,
    &DiscordChannel,
    &SmtpChannel,
];

/// Resolves a kind (`"websocket"`, `"webhook"`, …).
pub fn channel(kind: &str) -> Option<&'static dyn Channel> {
    CHANNELS.iter().copied().find(|c| c.kind() == kind)
}

/// Catalogue for `GET /api/v1/notify/kinds` — `label` is a displayable
/// name; refined labels/i18n/icons stay local to the front.
pub fn kinds() -> Vec<pnex_core::NotifyKindInfo> {
    CHANNELS
        .iter()
        .map(|c| pnex_core::NotifyKindInfo {
            kind: c.kind().to_string(),
            label: match c.kind() {
                "websocket" => "WebSocket".to_string(),
                "webhook" => "Webhook".to_string(),
                "ntfy" => "ntfy".to_string(),
                "telegram" => "Telegram".to_string(),
                "slack" => "Slack".to_string(),
                "discord" => "Discord".to_string(),
                "smtp" => "SMTP".to_string(),
                other => other.to_string(),
            },
            field_spec: c.field_spec(),
        })
        .collect()
}

/// Ids of the `secret` fields actually set in `config` — returned
/// in the DTO's `secrets_set` (the front shows "set" + "replace").
pub fn secrets_set(kind: &str, config: &serde_json::Value) -> Vec<String> {
    let Some(ch) = channel(kind) else {
        return vec![];
    };
    ch.field_spec()
        .into_iter()
        .filter(|f| f.r#type == FieldType::Secret)
        .filter(|f| {
            config
                .get(&f.id)
                .is_some_and(|v| !v.is_null() && v.as_str().is_none_or(|s| !s.is_empty()))
        })
        .map(|f| f.id)
        .collect()
}

/// Masked config for a GET: the `secret` fields become `null` —
/// never returned in API/UI (D54).
pub fn mask_config(kind: &str, config: &serde_json::Value) -> serde_json::Value {
    let Some(ch) = channel(kind) else {
        return config.clone();
    };
    let mut out = config.clone();
    if let Some(obj) = out.as_object_mut() {
        for f in ch.field_spec() {
            if f.r#type == FieldType::Secret {
                obj.insert(f.id.clone(), serde_json::Value::Null);
            }
        }
    }
    out
}

/// PUT merge (D54): `secret` fields absent **or** `null` in the
/// request mean "unchanged" (the UI only sends an explicit "replace").
/// Non-secret fields are taken as is (full replacement).
pub fn merge_config(
    kind: &str,
    existing: &serde_json::Value,
    incoming: &serde_json::Value,
) -> serde_json::Value {
    let Some(ch) = channel(kind) else {
        return incoming.clone();
    };
    let mut out = serde_json::Map::new();
    for f in ch.field_spec() {
        let inc = incoming.get(&f.id);
        if f.r#type == FieldType::Secret && inc.is_none_or(|v| v.is_null()) {
            if let Some(prev) = existing.get(&f.id) {
                out.insert(f.id.clone(), prev.clone());
            }
            continue;
        }
        if let Some(v) = inc {
            out.insert(f.id.clone(), v.clone());
        }
    }
    serde_json::Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn seven_kinds_shipped() {
        let ks = kinds();
        let mut names: Vec<&str> = ks.iter().map(|k| k.kind.as_str()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "discord",
                "ntfy",
                "slack",
                "smtp",
                "telegram",
                "webhook",
                "websocket"
            ]
        );
        // websocket: no field (info-block UI); webhook: 4 fields;
        // ntfy : 5 champs ; telegram 2 ; slack 1 ; discord 2 ; smtp 8.
        assert!(channel("websocket").unwrap().field_spec().is_empty());
        assert_eq!(channel("webhook").unwrap().field_spec().len(), 4);
        assert_eq!(channel("ntfy").unwrap().field_spec().len(), 5);
        assert_eq!(channel("telegram").unwrap().field_spec().len(), 2);
        assert_eq!(channel("slack").unwrap().field_spec().len(), 1);
        assert_eq!(channel("discord").unwrap().field_spec().len(), 2);
        assert_eq!(channel("smtp").unwrap().field_spec().len(), 8);
    }

    #[test]
    fn secret_masking_and_merge() {
        let cfg = json!({"url": "https://e.com", "secret_header": "x-sig", "secret_value": "abc"});
        let masked = mask_config("webhook", &cfg);
        assert!(masked["secret_value"].is_null());
        assert_eq!(masked["url"], "https://e.com");
        assert_eq!(secrets_set("webhook", &cfg), ["secret_value"]);
        assert!(secrets_set("webhook", &masked).is_empty());

        // PUT without the secret: unchanged.
        let merged = merge_config(
            "webhook",
            &cfg,
            &json!({"url": "https://n.com", "secret_header": "x-sig"}),
        );
        assert_eq!(merged["secret_value"], "abc");
        assert_eq!(merged["url"], "https://n.com");
        // PUT with null: unchanged too (the front sends null to "keep").
        let merged2 = merge_config(
            "webhook",
            &cfg,
            &json!({"url": "https://n.com", "secret_header": "x-sig", "secret_value": null}),
        );
        assert_eq!(merged2["secret_value"], "abc");
        // PUT with explicit replacement.
        let merged3 = merge_config(
            "webhook",
            &cfg,
            &json!({"url": "https://n.com", "secret_header": "x-sig", "secret_value": "new"}),
        );
        assert_eq!(merged3["secret_value"], "new");
    }

    #[test]
    fn unknown_kind_does_not_panic() {
        assert!(channel("gotify").is_none());
        assert!(secrets_set("gotify", &json!({})).is_empty());
    }

    /// i18n anti-panic guard: every `label_i18n`/`help_i18n` of the
    /// field_spec must exist in both locales — `t!` panics on a
    /// missing key and the front sweep only sees `t!("literal")`
    /// (finding: the 3 keys of the webhook field_spec were missing, opening the
    /// webhook form broke the app).
    #[test]
    fn field_spec_keys_present_in_the_locales() {
        let locales = [
            include_str!("../../pnex-frontend/locales/fr-FR.ftl"),
            include_str!("../../pnex-frontend/locales/en-US.ftl"),
        ];
        let keys_by_locale: Vec<Vec<&str>> = locales
            .iter()
            .map(|ftl| {
                ftl.lines()
                    .filter_map(|l| l.split_once(" = ").map(|(k, _)| k.trim()))
                    .collect()
            })
            .collect();
        for ch in CHANNELS {
            for f in ch.field_spec() {
                for keys in &keys_by_locale {
                    assert!(
                        keys.contains(&f.label_i18n.as_str()),
                        "key {} missing from a locale (field_spec {}.{} — add it to both .ftl files)",
                        f.label_i18n,
                        ch.kind(),
                        f.id
                    );
                    if let Some(help) = &f.help_i18n {
                        for keys in &keys_by_locale {
                            assert!(
                                keys.contains(&help.as_str()),
                                "help key {help} missing from a locale (field_spec {}.{})",
                                ch.kind(),
                                f.id
                            );
                        }
                    }
                }
            }
        }
    }
}
