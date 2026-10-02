//! Firmware choice at the wizard (custom-firmware.md D94): the generic PneX
//! preset (default) or a custom firmware project whose chip family matches
//! the effective board. The device carries the choice (`firmware_project_id`).

use super::*;

#[component]
pub(super) fn FirmwarePickBlock(soc: String, mut firmware_pick: Signal<Option<i64>>) -> Element {
    let projects = use_resource(|| async { api::firmware::list().await });
    let chip = pnex_core::Soc::from_board_soc(&soc)
        .map(|s| s.name().to_string())
        .unwrap_or(soc);
    let compatible: Vec<(i64, String, i64)> = match &*projects.value().read() {
        Some(Ok(all)) => all
            .iter()
            .filter(|p| p.chip_family == chip)
            .map(|p| (p.id, p.name.clone(), p.current_revision_number))
            .collect(),
        _ => Vec::new(),
    };
    let current = firmware_pick().map(|id| id.to_string()).unwrap_or_default();
    rsx! {
        div { class: "border-t border-gray-100 pt-3 mt-3",
            h4 { class: "text-xs font-semibold text-gray-600 uppercase tracking-wider mb-2",
                {t!("wizard-firmware")}
            }
            p { class: "text-xs text-gray-400 mb-2", {t!("wizard-firmware-help")} }
            select {
                class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                aria_label: t!("wizard-firmware"),
                value: "{current}",
                onchange: move |event| firmware_pick.set(event.value().parse::<i64>().ok()),
                option { value: "", selected: current.is_empty(), {t!("wizard-firmware-generic")} }
                for (id, name, rev) in compatible {
                    option { value: "{id}", selected: current == id.to_string(),
                        {format!("{name} (r{rev})")}
                    }
                }
            }
        }
    }
}
