//! Invariant "no direct dashboard → device link" (D123, D131,
//! docs/architecture/home-dashboards.md §1 bis): the surface services only
//! declare and write org controls; only a deployed flow acts on a device.
//! Static guard: the surface sources never reference the device bus or the
//! device command path.

use std::path::PathBuf;

/// Backend sources of the surfaces (dashboards, annotations, controls).
const SURFACE_FILES: &[&str] = &[
    "src/services/surface_controls.rs",
    "src/services/dashboards.rs",
    "src/services/annotation_layer.rs",
    "src/controllers/controls.rs",
    "src/controllers/dashboards.rs",
    "src/controllers/annotation_layers.rs",
];

/// Device actuation paths a surface must never reach.
const FORBIDDEN: &[&str] = &[
    "device_bus",
    "ws_device",
    "DeviceCommand",
    "send_command",
    "services::ota",
    "services::builds",
];

#[test]
fn surface_services_never_reach_a_device() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut hits = Vec::new();
    for file in SURFACE_FILES {
        let text = std::fs::read_to_string(root.join(file))
            .unwrap_or_else(|e| panic!("{file}: {e} (update SURFACE_FILES)"));
        for (n, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            for f in FORBIDDEN {
                if code.contains(f) {
                    hits.push(format!("{file}:{} → {f}", n + 1));
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "a surface service references a device actuation path (only a deployed flow \
         acts on a device, home-dashboards.md §1 bis):\n{}",
        hits.join("\n")
    );
}
