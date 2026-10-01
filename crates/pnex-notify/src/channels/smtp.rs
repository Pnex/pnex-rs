//! Canal `smtp` — e-mail via lettre 0.11 (rustls, jamais openssl).
//!
//! TLS : `tls` = SMTPS implicite (465), `starttls` = upgrade explicite
//! (587), `none` = `builder_dangerous` (25) — le champ vide/absent vaut
//! `starttls` (quirk du Select front qui envoie `""` si jamais touché).
//! Erreurs lettre : transitoire/timeout → [`NotifyError::Network`]
//! (retryable), permanent (auth 535, refus de boîte, TLS) →
//! [`NotifyError::Config`] — un retry ne réparera pas un 5xx permanent.
//! D54 : `password` est un champ secret write-only. D62 : le sujet du
//! Message devient l'objet de l'e-mail ; `meta` jamais projeté.

use async_trait::async_trait;
use lettre::message::{Mailbox, Message as LettreMessage};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{transport::smtp::AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

use crate::channels::{field, str_field};
use crate::error::NotifyError;
use crate::{Channel, Message};

pub struct SmtpChannel;

/// Modes TLS du champ `tls` (select).
const TLS_MODES: &[&str] = &["starttls", "tls", "none"];

/// Port par défaut selon le mode TLS — toujours posé explicitement pour
/// lever l'ambiguïté des ports par défaut lettre.
fn default_port(tls: &str) -> u16 {
    match tls {
        "tls" => 465,
        "none" => 25,
        _ => 587,
    }
}

/// Port lu depuis la config — le front Number sérialise un f64 : lire
/// i64, sinon f64 tronqué.
fn port_de(config: &serde_json::Value) -> Option<u16> {
    let v = config.get("port")?;
    let n = v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))?;
    u16::try_from(n).ok()
}

#[async_trait]
impl Channel for SmtpChannel {
    fn kind(&self) -> &'static str {
        "smtp"
    }

    fn field_spec(&self) -> Vec<pnex_core::FieldSpec> {
        vec![
            field(
                "host",
                "notify-field-host",
                pnex_core::FieldType::Text,
                true,
                Some("smtp.example.com"),
                None,
                &[],
            ),
            field(
                "port",
                "notify-field-port",
                pnex_core::FieldType::Number,
                false,
                None,
                None,
                &[],
            ),
            field(
                "tls",
                "notify-field-tls",
                pnex_core::FieldType::Select,
                false,
                None,
                Some("notify-field-tls-help"),
                TLS_MODES,
            ),
            field(
                "username",
                "notify-field-username",
                pnex_core::FieldType::Text,
                false,
                None,
                None,
                &[],
            ),
            field(
                "password",
                "notify-field-password",
                pnex_core::FieldType::Secret,
                false,
                None,
                None,
                &[],
            ),
            field(
                "from",
                "notify-field-from",
                pnex_core::FieldType::Text,
                true,
                Some("PNeX <no-reply@example.com>"),
                None,
                &[],
            ),
            field(
                "to",
                "notify-field-to",
                pnex_core::FieldType::Text,
                true,
                Some("a@example.com, b@example.com"),
                Some("notify-field-to-help"),
                &[],
            ),
        ]
    }

    fn validate(&self, config: &serde_json::Value) -> Result<(), String> {
        str_field(config, "host").ok_or_else(|| "the SMTP host is required".to_string())?;
        // Port : présent et non null ⇒ doit être numérique valide (1–65535).
        if let Some(v) = config.get("port") {
            if !v.is_null() {
                let port = port_de(config)
                    .filter(|p| *p != 0)
                    .ok_or_else(|| "invalid port (1–65535)".to_string())?;
                let _ = port;
            }
        }
        if let Some(tls) = str_field(config, "tls") {
            if !TLS_MODES.contains(&tls) {
                return Err(format!("invalid TLS mode: {tls} (starttls, tls or none)"));
            }
        }
        // Credentials en paire (école webhook : un secret sans vecteur
        // d'usage est une erreur).
        let username = str_field(config, "username");
        let password = str_field(config, "password");
        if username.is_some() != password.is_some() {
            return Err("username and password come in pairs (or neither)".into());
        }
        let from =
            str_field(config, "from").ok_or_else(|| "the from address is required".to_string())?;
        from.parse::<Mailbox>()
            .map_err(|_| "invalid from address (e.g. PNeX <no-reply@example.com>)".to_string())?;
        let to = str_field(config, "to")
            .ok_or_else(|| "the recipients (to) are required".to_string())?;
        let dests: Vec<&str> = to
            .split(',')
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .collect();
        if dests.is_empty() {
            return Err("at least one recipient is required".into());
        }
        for dest in dests {
            dest.parse::<Mailbox>()
                .map_err(|_| format!("invalid to address: {dest}"))?;
        }
        Ok(())
    }

    async fn send(&self, config: &serde_json::Value, msg: &Message) -> Result<(), NotifyError> {
        let host = str_field(config, "host")
            .ok_or_else(|| NotifyError::Config("missing SMTP host".into()))?;
        // str_field filtre les vides : "" (quirk select) → défaut starttls.
        let tls = str_field(config, "tls").unwrap_or("starttls");
        let port = port_de(config).unwrap_or_else(|| default_port(tls));
        let builder = match tls {
            "tls" => AsyncSmtpTransport::<Tokio1Executor>::relay(host)
                .map_err(|e| NotifyError::Config(e.to_string()))?,
            "none" => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host),
            _ => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host)
                .map_err(|e| NotifyError::Config(e.to_string()))?,
        };
        let mut builder = builder.port(port);
        if let Some((user, pass)) = str_field(config, "username").zip(str_field(config, "password"))
        {
            builder = builder.credentials(Credentials::new(user.to_string(), pass.to_string()));
        }
        let transport = builder.build();
        let email = build_email(config, msg)?;
        transport.send(email).await.map(drop).map_err(|e| {
            if e.is_transient() || e.is_timeout() {
                NotifyError::Network(e.to_string())
            } else {
                // Permanent (auth 535, refus de boîte, TLS) : le retry ne
                // réparera pas.
                NotifyError::Config(e.to_string())
            }
        })
    }
}

/// Construit l'e-mail (from/to/subject/body) — fonction pure, éprouvée en
/// test (l'URL de production ne se prête pas à un récepteur local).
fn build_email(config: &serde_json::Value, msg: &Message) -> Result<LettreMessage, NotifyError> {
    let from =
        str_field(config, "from").ok_or_else(|| NotifyError::Config("missing from".into()))?;
    let from: Mailbox = from
        .parse()
        .map_err(|_| NotifyError::Config("invalid from address".into()))?;
    let to = str_field(config, "to").ok_or_else(|| NotifyError::Config("missing to".into()))?;
    let mut builder = LettreMessage::builder().from(from);
    for dest in to.split(',').map(str::trim).filter(|d| !d.is_empty()) {
        let dest: Mailbox = dest
            .parse()
            .map_err(|_| NotifyError::Config(format!("invalid to address: {dest}")))?;
        builder = builder.to(dest);
    }
    if let Some(subject) = msg.subject.as_deref().filter(|s| !s.is_empty()) {
        builder = builder.subject(subject);
    }
    builder
        .body(msg.body.clone())
        .map_err(|e| NotifyError::Config(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validate_couvre_toutes_les_regles() {
        let base = json!({
            "host": "smtp.example.com",
            "from": "PNeX <no-reply@example.com>",
            "to": "a@example.com, b@example.com",
            "username": "u", "password": "p",
        });
        assert!(SmtpChannel.validate(&base).is_ok());
        assert!(SmtpChannel.validate(&json!({})).is_err());
        assert!(SmtpChannel
            .validate(&json!({"host": "h", "from": "x@y.z", "to": "a@b.c", "port": 0}))
            .is_err());
        assert!(SmtpChannel
            .validate(&json!({"host": "h", "from": "x@y.z", "to": "a@b.c", "port": 70000}))
            .is_err());
        assert!(SmtpChannel
            .validate(&json!({"host": "h", "from": "x@y.z", "to": "a@b.c", "tls": "quic"}))
            .is_err());
        // Credentials en paire.
        assert!(SmtpChannel
            .validate(&json!({"host": "h", "from": "x@y.z", "to": "a@b.c", "password": "p"}))
            .is_err());
        assert!(SmtpChannel
            .validate(&json!({"host": "h", "from": "x@y.z", "to": "a@b.c", "username": "u"}))
            .is_err());
        // Adresses.
        assert!(SmtpChannel
            .validate(&json!({"host": "h", "from": "pas une adresse", "to": "a@b.c"}))
            .is_err());
        assert!(SmtpChannel
            .validate(&json!({"host": "h", "from": "x@y.z", "to": ",,,"}))
            .is_err());
        assert!(SmtpChannel
            .validate(&json!({"host": "h", "from": "x@y.z", "to": "a@b.c"}))
            .is_ok());
    }

    #[test]
    fn default_port_selon_tls() {
        assert_eq!(default_port("starttls"), 587);
        assert_eq!(default_port("tls"), 465);
        assert_eq!(default_port("none"), 25);
        assert_eq!(default_port(""), 587);
    }

    #[tokio::test]
    async fn email_from_to_sujet_via_builder() {
        let cfg = json!({
            "host": "smtp.example.com",
            "from": "PNeX <no-reply@example.com>",
            "to": "a@example.com, b@example.com",
        });
        let msg = Message {
            subject: Some("Alerte seuil".into()),
            body: "v=42".into(),
            meta: json!({}),
        };
        let email = build_email(&cfg, &msg).unwrap();
        let rendu = String::from_utf8_lossy(&email.formatted()).to_string();
        assert!(rendu.contains("no-reply@example.com"), "{rendu}");
        assert!(rendu.contains("a@example.com"), "{rendu}");
        assert!(rendu.contains("b@example.com"), "{rendu}");
        assert!(rendu.contains("Alerte seuil"), "{rendu}");
        assert!(rendu.contains("v=42"), "{rendu}");
        // Sans sujet : pas d'objet, corps intact.
        let email = build_email(
            &cfg,
            &Message {
                subject: None,
                body: "v=42".into(),
                meta: json!({}),
            },
        )
        .unwrap();
        let rendu = String::from_utf8_lossy(&email.formatted()).to_string();
        assert!(!rendu.contains("Subject:"), "{rendu}");
        assert!(rendu.contains("v=42"), "{rendu}");
    }

    #[tokio::test]
    async fn email_invalide_erreur_config() {
        let cfg = json!({"host": "h", "from": "x@y.z", "to": "a@b.c, adresse_ko"});
        let err = build_email(
            &cfg,
            &Message {
                subject: None,
                body: "b".into(),
                meta: json!({}),
            },
        )
        .unwrap_err();
        assert!(matches!(err, NotifyError::Config(_)));
    }
}
