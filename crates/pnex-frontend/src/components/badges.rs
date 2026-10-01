//! Aides de rendu « builds » partagées : badge de phase (colonne Firmware de
//! la liste devices, wizard d'enregistrement, page Builds) et date locale
//! compacte. Classes Tailwind littérales complètes (scan du CSS).

use dioxus_i18n::t;

/// Badge (classes Tailwind littérales) + libellé i18n par phase.
pub fn phase_badge(phase: Option<&str>) -> (String, String) {
    let (class, key) = match phase {
        Some("queued") => ("bg-gray-100 text-gray-600", "builds-phase-queued"),
        Some("running") => (
            "bg-blue-100 text-blue-700 animate-pulse",
            "builds-phase-running",
        ),
        Some("succeeded") => ("bg-green-100 text-green-700", "builds-phase-succeeded"),
        Some("failed") => ("bg-red-100 text-red-700", "builds-phase-failed"),
        _ => ("bg-gray-100 text-gray-400", "builds-phase-queued"),
    };
    (
        format!("inline-block px-2 py-0.5 rounded-full text-xs font-medium {class}"),
        t!(key),
    )
}

/// Date locale compacte (dernier changement de phase).
pub fn date_label(rfc3339: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .map(|ts| {
            let local = ts.with_timezone(&chrono::Local);
            local.format("%d/%m %H:%M:%S").to_string()
        })
        .unwrap_or_else(|_| rfc3339.to_string())
}
