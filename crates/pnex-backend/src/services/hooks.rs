//! Incoming webhooks (connectors.md D795): a call signed with the connector's secret
//! lands in its inbox, in the connector's org (R1); an import reads the inbox later.
//! Nothing of the request names the org, and nothing tells an unknown connector
//! from a bad signature.

use hmac::{Hmac, Mac};
use pnex_core::connectors::{webhook_fresh, WebhookSettings};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use sha2::Sha256;
use uuid::Uuid;

use crate::services::secrets::{store, Keyring};

/// Unread calls one webhook keeps over the retention.
pub const INBOX_MAX: i64 = 100_000;
/// Days a call stays in the inbox, read or not.
pub const INBOX_DAYS: i64 = 7;

#[derive(Debug, PartialEq, Eq)]
pub enum HookRefusal {
    /// Unknown connector, no secret, bad or stale signature: one answer.
    Signature,
    Body,
    Full,
    Unavailable,
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// Checks and keeps one call. `header` reads a request header by name.
pub async fn receive(
    db: &DatabaseConnection,
    ring: &Keyring,
    id: Uuid,
    header: impl Fn(&str) -> Option<String>,
    body: &[u8],
) -> Result<(), HookRefusal> {
    let row = db
        .query_one_raw(stmt(
            "SELECT org_id, settings, secret_id FROM connectors WHERE id = $1 AND kind = 'webhook'",
            vec![id.into()],
        ))
        .await
        .map_err(|_| HookRefusal::Unavailable)?
        .ok_or(HookRefusal::Signature)?;
    let org_id: i64 = row
        .try_get("", "org_id")
        .map_err(|_| HookRefusal::Unavailable)?;
    let secret_id: Option<Uuid> = row
        .try_get("", "secret_id")
        .map_err(|_| HookRefusal::Unavailable)?;
    let settings: WebhookSettings = row
        .try_get::<serde_json::Value>("", "settings")
        .ok()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    let secret_id = secret_id.ok_or(HookRefusal::Signature)?;
    let timestamp = if settings.timestamp_header.is_empty() {
        None
    } else {
        let t = header(&settings.timestamp_header).ok_or(HookRefusal::Signature)?;
        if !webhook_fresh(&t, chrono::Utc::now().timestamp()) {
            return Err(HookRefusal::Signature);
        }
        Some(t)
    };
    let given = header(&settings.signature_header)
        .and_then(|h| settings.signature(&h))
        .ok_or(HookRefusal::Signature)?;
    let secret = store::reveal(db, ring, Some(org_id), secret_id)
        .await
        .map_err(|_| HookRefusal::Unavailable)?;
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(|_| HookRefusal::Unavailable)?;
    mac.update(&settings.signed_payload(timestamp.as_deref(), body));
    // Constant time: the comparison tells nothing of how close a guess was.
    mac.verify_slice(&given)
        .map_err(|_| HookRefusal::Signature)?;

    let json: serde_json::Value = serde_json::from_slice(body).map_err(|_| HookRefusal::Body)?;
    let room = db
        .query_one_raw(stmt(
            "SELECT count(*) AS n FROM (SELECT 1 FROM connector_inbox \
               WHERE connector_id = $1 AND received_at > now() - make_interval(days => $2::int) \
               LIMIT $3) q",
            vec![id.into(), (INBOX_DAYS as i32).into(), INBOX_MAX.into()],
        ))
        .await
        .map_err(|_| HookRefusal::Unavailable)?
        .and_then(|r| r.try_get::<i64>("", "n").ok())
        .unwrap_or(INBOX_MAX);
    if room >= INBOX_MAX {
        return Err(HookRefusal::Full);
    }
    db.execute_raw(stmt(
        "INSERT INTO connector_inbox (org_id, connector_id, body) VALUES ($1, $2, $3::jsonb)",
        vec![org_id.into(), id.into(), json.to_string().into()],
    ))
    .await
    .map_err(|_| HookRefusal::Unavailable)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sender_signature_is_the_hmac_of_the_payload() {
        // RFC 4231 test case 2.
        let mut mac = Hmac::<Sha256>::new_from_slice(b"Jefe").unwrap();
        mac.update(b"what do ya want for nothing?");
        let hex: String = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            hex,
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        let w = WebhookSettings::default();
        let sig = w.signature(&format!("sha256={hex}")).unwrap();
        let mut mac = Hmac::<Sha256>::new_from_slice(b"Jefe").unwrap();
        mac.update(b"what do ya want for nothing?");
        assert!(mac.verify_slice(&sig).is_ok());
    }
}
