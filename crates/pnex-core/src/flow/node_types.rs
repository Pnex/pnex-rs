//! Node-type allowlist — the security boundary between user flows and the
//! runtime (docs/architecture/security.md R3, R5; SEC-1, SEC-2).
//!
//! The runtime executes the code of several organisations in one process.
//! User flows only carry typed kinds; the projection maps them to the types
//! below, so no builtin with a host effect (`exec`, `template` reading the
//! process env, files, sockets) is ever registered.

/// Builtins the projection emits: `inject` / `debug` for their typed kinds,
/// `comment` for the tab notes, `function` for registry JS functions and
/// JSON merge; `unknown` / `unknown.global` are the engine's inert
/// placeholders for any unregistered type (the engine panics without them),
/// so a dropped builtin in a stale artifact loads as a no-op instead of
/// running.
const RUNTIME_BUILTINS: &[&str] = &[
    "comment",
    "debug",
    "function",
    "inject",
    "unknown",
    "unknown.global",
];

/// Prefix of every PNeX custom node type.
pub const PNEX_TYPE_PREFIX: &str = "pnex-";

/// Whether the flow runtime may register `type_name`: the builtins the
/// projection emits and the PNeX custom nodes.
pub fn runtime_type_allowed(type_name: &str) -> bool {
    RUNTIME_BUILTINS.contains(&type_name) || type_name.starts_with(PNEX_TYPE_PREFIX)
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
            assert!(!runtime_type_allowed(t), "{t} must not be registered");
        }
    }

    #[test]
    fn projected_types_are_allowed() {
        for t in [
            "pnex-device-write",
            "function",
            "inject",
            "debug",
            "unknown",
        ] {
            assert!(runtime_type_allowed(t), "{t}");
        }
    }
}
