//! Registre des canaux (D51) — source de vérité des kinds : l'API
//! `GET /api/v1/notify/kinds` et le formulaire dynamique du front en
//! dérivent (`notify-kinds.v1`, additif-only).
//!
//! Héberge aussi les helpers de masquage D54 : les champs `secret` du
//! `field_spec` sont write-only (GET masqué, PUT null/absent = inchangé).

use crate::channels::{
    discord::DiscordChannel, ntfy::NtfyChannel, slack::SlackChannel, smtp::SmtpChannel,
    telegram::TelegramChannel, webhook::WebhookChannel, websocket::WebSocketChannel,
};
use crate::Channel;
use pnex_core::FieldType;

/// Toutes les implémentations de canaux livrées — 1 entrée par kind.
/// L'ordre = ordre d'affichage du picker front.
pub static CHANNELS: &[&dyn Channel] = &[
    &WebSocketChannel,
    &WebhookChannel,
    &NtfyChannel,
    &TelegramChannel,
    &SlackChannel,
    &DiscordChannel,
    &SmtpChannel,
];

/// Résout un kind (`"websocket"`, `"webhook"`, …).
pub fn channel(kind: &str) -> Option<&'static dyn Channel> {
    CHANNELS.iter().copied().find(|c| c.kind() == kind)
}

/// Catalogue pour `GET /api/v1/notify/kinds` — `label` est un nom
/// affichable ; les labels/i18n/icônes affinés restent locaux au front.
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

/// Ids des champs `secret` effectivement définis dans `config` — rendu
/// dans `secrets_set` du DTO (le front affiche « défini » + « remplacer »).
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

/// Config masquée pour un GET : les champs `secret` deviennent `null` —
/// jamais rendus en API/UI (D54).
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

/// Merge PUT (D54) : les champs `secret` absents **ou** `null` dans la
/// requête valent « inchangé » (l'UI n'envoie que « remplacer » explicite).
/// Les champs non-secrets sont repris tels quels (remplacement complet).
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
    fn sept_kinds_livres() {
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
        // websocket : aucun champ (info-block UI) ; webhook : 3 champs ;
        // ntfy : 5 champs ; telegram 2 ; slack 1 ; discord 2 ; smtp 7.
        assert!(channel("websocket").unwrap().field_spec().is_empty());
        assert_eq!(channel("webhook").unwrap().field_spec().len(), 3);
        assert_eq!(channel("ntfy").unwrap().field_spec().len(), 5);
        assert_eq!(channel("telegram").unwrap().field_spec().len(), 2);
        assert_eq!(channel("slack").unwrap().field_spec().len(), 1);
        assert_eq!(channel("discord").unwrap().field_spec().len(), 2);
        assert_eq!(channel("smtp").unwrap().field_spec().len(), 7);
    }

    #[test]
    fn masquage_et_merge_secret() {
        let cfg = json!({"url": "https://e.com", "secret_header": "x-sig", "secret_value": "abc"});
        let masked = mask_config("webhook", &cfg);
        assert!(masked["secret_value"].is_null());
        assert_eq!(masked["url"], "https://e.com");
        assert_eq!(secrets_set("webhook", &cfg), ["secret_value"]);
        assert!(secrets_set("webhook", &masked).is_empty());

        // PUT sans le secret : inchangé.
        let merged = merge_config(
            "webhook",
            &cfg,
            &json!({"url": "https://n.com", "secret_header": "x-sig"}),
        );
        assert_eq!(merged["secret_value"], "abc");
        assert_eq!(merged["url"], "https://n.com");
        // PUT avec null : inchangé aussi (le front envoie null pour « garder »).
        let merged2 = merge_config(
            "webhook",
            &cfg,
            &json!({"url": "https://n.com", "secret_header": "x-sig", "secret_value": null}),
        );
        assert_eq!(merged2["secret_value"], "abc");
        // PUT avec remplacement explicite.
        let merged3 = merge_config(
            "webhook",
            &cfg,
            &json!({"url": "https://n.com", "secret_header": "x-sig", "secret_value": "new"}),
        );
        assert_eq!(merged3["secret_value"], "new");
    }

    #[test]
    fn kind_inconnu_ne_panique_pas() {
        assert!(channel("gotify").is_none());
        assert!(secrets_set("gotify", &json!({})).is_empty());
    }

    /// Garde anti-panic i18n : chaque `label_i18n`/`help_i18n` des
    /// field_spec doit exister dans les deux locales — `t!` panique sur
    /// clé absente et le sweep front ne voit que les `t!("littéral")`
    /// (constat : les 3 clés du field_spec webhook manquaient, ouvrir le
    /// formulaire webhook cassait l'app).
    #[test]
    fn cles_field_spec_presentes_dans_les_locales() {
        let locales = [
            include_str!("../../pnex-frontend/locales/fr-FR.ftl"),
            include_str!("../../pnex-frontend/locales/en-US.ftl"),
        ];
        let cles_par_locale: Vec<Vec<&str>> = locales
            .iter()
            .map(|ftl| {
                ftl.lines()
                    .filter_map(|l| l.split_once(" = ").map(|(k, _)| k.trim()))
                    .collect()
            })
            .collect();
        for ch in CHANNELS {
            for f in ch.field_spec() {
                for keys in &cles_par_locale {
                    assert!(
                        keys.contains(&f.label_i18n.as_str()),
                        "key {} missing from a locale (field_spec {}.{} — add it to both .ftl files)",
                        f.label_i18n,
                        ch.kind(),
                        f.id
                    );
                    if let Some(help) = &f.help_i18n {
                        for keys in &cles_par_locale {
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
