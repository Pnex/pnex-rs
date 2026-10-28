//! Shared "builds" rendering helpers: phase badge (Firmware column of the
//! devices list, registration wizard, Builds page) and compact local
//! date. Full literal Tailwind classes (CSS scan).

use dioxus_i18n::t;

/// Badge (literal Tailwind classes) + i18n label per phase.
pub fn phase_badge(phase: Option<&str>) -> (String, String) {
    let (class, key) = match phase {
        Some("queued") => ("bg-gray-100 text-gray-600", "builds-phase-queued"),
        Some("running") => (
            "bg-signal-100 text-signal-700 pnex-live-dot",
            "builds-phase-running",
        ),
        Some("succeeded") => ("bg-green-100 text-green-700", "builds-phase-succeeded"),
        Some("failed") => ("bg-red-100 text-red-700", "builds-phase-failed"),
        _ => ("bg-gray-100 text-gray-500", "builds-phase-queued"),
    };
    (
        format!("inline-block px-2 py-0.5 rounded-full text-xs font-medium {class}"),
        t!(key),
    )
}

/// Compact local date (last phase change).
pub fn date_label(rfc3339: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .map(|ts| {
            let local = ts.with_timezone(&chrono::Local);
            local.format("%d/%m %H:%M:%S").to_string()
        })
        .unwrap_or_else(|_| rfc3339.to_string())
}
