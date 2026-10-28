//! State file `<home>/runtime.json`: the health contract read by the
//! backend supervisor (`GET /api/v1/flows/{n}/runtime`). Atomic write
//! (tmp + rename) — a concurrent reader never sees a truncated file.
//!
//! Process health **only** (pid, alive, reloads): per-flow
//! versions do NOT live here — the artifact is multi-flow and the
//! source of truth "deployed version of a flow" is the DB
//! (`flows.deployed_version_id`), kept up to date by the acknowledged deploy.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuntimeState {
    pub pid: u32,
    pub running: bool,
    /// Epoch seconds of the process start.
    pub started_at: u64,
    /// Fingerprint of the running flows.json (SHA-256 hex, EdgeLinkd).
    pub flow_rev: Option<String>,
    /// Number of successful hot reloads since start.
    pub redeploys: u64,
}

/// Atomic write of the state. Errors are logged, never fatal:
/// the state file is best-effort, it must not kill the runtime.
pub fn write(home: &std::path::Path, state: &RuntimeState) {
    if let Err(e) = std::fs::create_dir_all(home) {
        log::warn!("Cannot create the state directory {}: {e}", home.display());
        return;
    }
    let json = match serde_json::to_string_pretty(state) {
        Ok(j) => j,
        Err(e) => {
            log::warn!("Runtime state not serialisable: {e}");
            return;
        }
    };
    let tmp = home.join("runtime.json.tmp");
    let dst = home.join("runtime.json");
    if std::fs::write(&tmp, json)
        .and_then(|_| std::fs::rename(&tmp, &dst))
        .is_err()
    {
        log::warn!("Cannot write {}", dst.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_roundtrip_and_atomic_write() {
        let dir = std::env::temp_dir().join(format!("pnex-flow-state-test-{}", std::process::id()));
        let st = RuntimeState {
            pid: 42,
            running: true,
            started_at: 1,
            flow_rev: Some("abc".into()),
            redeploys: 3,
        };
        write(&dir, &st);
        let back: RuntimeState =
            serde_json::from_str(&std::fs::read_to_string(dir.join("runtime.json")).unwrap())
                .unwrap();
        assert_eq!(back.redeploys, 3);
        assert_eq!(back.flow_rev, Some("abc".into()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
