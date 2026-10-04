//! Node-type allowlists — the security boundary between user flows and the
//! runtime (docs/architecture/security.md R3, R5; SEC-1, SEC-2).
//!
//! The runtime executes the code of several organisations in one process:
//! a user-chosen Node-RED type must never reach a builtin with a host effect
//! (`exec`, `template` reading the process env, files, sockets) nor a PNeX
//! node whose tenant fields (`pnex_org_id`, …) would be taken from user
//! config. Both the graph validation (save + deploy) and the runtime
//! registry consult these lists.

/// Node-RED builtins a [`super::FlowNodeKind::Red`] node may target: pure
/// message transforms and flow plumbing, no host, network or env access
/// beyond the runtime's (scrubbed) env store.
pub const RED_ALLOWED_TYPES: &[&str] = &[
    "batch",
    "catch",
    "change",
    "comment",
    "complete",
    "csv",
    "debug",
    "delay",
    "inject",
    "join",
    "json",
    "junction",
    "link call",
    "link in",
    "link out",
    "range",
    "rbe",
    "sort",
    "split",
    "status",
    "switch",
    "trigger",
    "yaml",
];

/// Runtime-only builtins on top of [`RED_ALLOWED_TYPES`]: `function` is
/// emitted by the projection for registry JS functions and JSON merge;
/// `unknown` / `unknown.global` are the engine's inert placeholders for any
/// unregistered type (the engine panics without them), so a dropped builtin
/// in a stale artifact loads as a no-op instead of running.
const RUNTIME_ONLY_BUILTINS: &[&str] = &["function", "unknown", "unknown.global"];

/// Prefix of every PNeX custom node type; reserved to typed kinds.
pub const PNEX_TYPE_PREFIX: &str = "pnex-";

/// Prefix of the tenant / provenance fields the projection stamps on every
/// entry; never accepted from user config.
pub const PNEX_FIELD_PREFIX: &str = "pnex_";

/// Whether a user-supplied Red node may target `type_name`.
pub fn red_type_allowed(type_name: &str) -> bool {
    RED_ALLOWED_TYPES.contains(&type_name)
}

/// Whether the flow runtime may register `type_name`: the Red allowlist,
/// the builtins the projection emits, and the PNeX custom nodes.
pub fn runtime_type_allowed(type_name: &str) -> bool {
    red_type_allowed(type_name)
        || RUNTIME_ONLY_BUILTINS.contains(&type_name)
        || type_name.starts_with(PNEX_TYPE_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_effect_builtins_are_never_allowed() {
        for t in [
            "exec",
            "template",
            "file",
            "file in",
            "watch",
            "http request",
            "http in",
            "tcp in",
            "tcp out",
            "tcp request",
            "udp in",
            "udp out",
            "mqtt in",
            "mqtt out",
            "websocket in",
            "websocket out",
            "websocket-client",
            "websocket-listener",
            "subflow",
            "console-json",
        ] {
            assert!(!red_type_allowed(t), "{t} must not be a Red type");
            assert!(!runtime_type_allowed(t), "{t} must not be registered");
        }
    }

    #[test]
    fn pnex_types_are_runtime_only() {
        assert!(!red_type_allowed("pnex-device-write"));
        assert!(runtime_type_allowed("pnex-device-write"));
        assert!(!red_type_allowed("function"));
        assert!(runtime_type_allowed("function"));
        assert!(!red_type_allowed("unknown"));
        assert!(runtime_type_allowed("unknown"));
    }
}
