//! Delivery journal client (D86) — the flow runtime reports each attempt
//! on a non-websocket channel to the backend (`POST /internal/notify/journal`,
//! same service token as the websocket deliver endpoint), which stores it
//! in OpenObserve.

use crate::channels::CLIENT;
use crate::error::NotifyError;

/// Posts one journal entry. Callers treat a failure as a log line: the
/// journal never blocks nor fails a notification.
pub async fn post(
    url: &str,
    token: &str,
    entry: &pnex_core::NotifyDeliveryEntry,
) -> Result<(), NotifyError> {
    let resp = CLIENT
        .post(url)
        .bearer_auth(token)
        .header(
            pnex_core::FLOW_WORKER_HEADER,
            pnex_core::flow_worker_fence().unwrap_or_default(),
        )
        .json(entry)
        .send()
        .await
        .map_err(|e| NotifyError::Network(format!("journal unreachable: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(NotifyError::Http {
            status: status.as_u16(),
        });
    }
    Ok(())
}

/// Asks the server whether this send may go to a recipient taken from the
/// message (D313); the server counts it on "yes". `Err` = the refusal code.
pub async fn check_recipient(
    url: &str,
    token: &str,
    req: &pnex_core::RecipientCheck,
) -> Result<(), String> {
    let resp = CLIENT
        .post(url)
        .bearer_auth(token)
        .header(
            pnex_core::FLOW_WORKER_HEADER,
            pnex_core::flow_worker_fence().unwrap_or_default(),
        )
        .json(req)
        .send()
        .await
        .map_err(|_| "notify-recipient-check-unreachable".to_string())?;
    if resp.status().is_success() {
        return Ok(());
    }
    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    Err(body["code"]
        .as_str()
        .unwrap_or("notify-recipient-refused")
        .to_string())
}

/// HTTP status carried by an error, when the upstream answered.
pub fn http_status(e: &NotifyError) -> Option<u16> {
    match e {
        NotifyError::Http { status } => Some(*status),
        _ => None,
    }
}
