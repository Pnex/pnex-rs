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

/// HTTP status carried by an error, when the upstream answered.
pub fn http_status(e: &NotifyError) -> Option<u16> {
    match e {
        NotifyError::Http { status } => Some(*status),
        _ => None,
    }
}
