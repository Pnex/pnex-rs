//! Worker fencing identity (D106) shared by the backend and the flow
//! runtime nodes.
//!
//! The backend injects `PNEX_FLOW_WORKER_FENCE=<worker id>:<boot>` into the
//! runtime child; every node that calls an internal backend route stamps it
//! in the `x-pnex-flow-worker` header. The backend rejects a write whose
//! worker no longer owns the org (moved placement, superseded boot), so a
//! partitioned or zombie worker can never actuate a device twice.

/// Env var carrying the fencing identity into the runtime child.
pub const FLOW_WORKER_ENV: &str = "PNEX_FLOW_WORKER_FENCE";
/// Header stamped by the runtime nodes on their internal backend calls.
pub const FLOW_WORKER_HEADER: &str = "x-pnex-flow-worker";

/// The fencing identity of this runtime process (`None` outside a cluster
/// worker: backend-internal callers and tests send no header).
pub fn flow_worker_fence() -> Option<&'static str> {
    static FENCE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    FENCE
        .get_or_init(|| {
            std::env::var(FLOW_WORKER_ENV)
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        })
        .as_deref()
}

/// Parses `<worker id>:<boot>` (the id itself may contain `:`; the boot is
/// the last segment).
pub fn parse_flow_worker_fence(raw: &str) -> Option<(&str, i64)> {
    let (id, boot) = raw.trim().rsplit_once(':')?;
    let boot = boot.parse().ok()?;
    (!id.is_empty()).then_some((id, boot))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_worker_fence() {
        assert_eq!(parse_flow_worker_fence("pod-a:42"), Some(("pod-a", 42)));
        assert_eq!(
            parse_flow_worker_fence("host:with:colons:7"),
            Some(("host:with:colons", 7))
        );
        assert_eq!(parse_flow_worker_fence("nobooth"), None);
        assert_eq!(parse_flow_worker_fence(":3"), None);
        assert_eq!(parse_flow_worker_fence("a:x"), None);
    }
}
