//! One-time tickets for browser websockets (`/ws/notify`, `/ws/camera/live`).
//!
//! A browser cannot set an `Authorization` header on `new WebSocket()`, and
//! a JWT in the URL ends up in access logs. The client trades its bearer
//! token for a ticket (`POST /api/v1/ws-ticket`, org from the principal),
//! then opens the socket with `?ticket=`. A ticket is random, bound to the
//! user and the org, valid [`TTL_SECS`], and consumed by its first use
//! (Valkey `GETDEL`, so it works across pods).

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use loco_rs::config::Config;
use rand::RngExt;
use redis::AsyncCommands;

/// Lifetime of an unused ticket.
pub const TTL_SECS: u64 = 60;

/// Who a redeemed ticket was issued to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TicketOwner {
    pub user_id: i64,
    pub org_id: i64,
}

fn key(ticket: &str) -> String {
    format!("pnex:ws-ticket:{ticket}")
}

/// Issues a ticket for `user_id` in `org_id`. `None` when Valkey is not
/// configured or unreachable.
pub async fn issue(config: &Config, user_id: i64, org_id: i64) -> Option<String> {
    let mut conn = crate::services::shared_valkey::conn(config).await?;
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    let ticket = URL_SAFE_NO_PAD.encode(bytes);
    let stored: redis::RedisResult<()> = conn
        .set_ex(key(&ticket), format!("{user_id}:{org_id}"), TTL_SECS)
        .await;
    stored.ok().map(|()| ticket)
}

/// Consumes `ticket`: its owner the first time, `None` afterwards, once
/// expired, or for an unknown ticket.
pub async fn redeem(config: &Config, ticket: &str) -> Option<TicketOwner> {
    let ticket = ticket.trim();
    if ticket.is_empty() || ticket.len() > 64 {
        return None;
    }
    let mut conn = crate::services::shared_valkey::conn(config).await?;
    let value: Option<String> = redis::cmd("GETDEL")
        .arg(key(ticket))
        .query_async(&mut conn)
        .await
        .ok()
        .flatten();
    let (user, org) = value?
        .split_once(':')
        .map(|(u, o)| (u.to_string(), o.to_string()))?;
    Some(TicketOwner {
        user_id: user.parse().ok()?,
        org_id: org.parse().ok()?,
    })
}
