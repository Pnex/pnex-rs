//! Registry function picker for the flow inspector — the org's
//! list (`api::functions::list`), on choice: fetch the detail (inputs/
//! outputs of the current version) then parent callback (patch of the
//! node config: pinned snapshot, canvas ports generated from the
//! declarations).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::components::ui::FIELD_LABEL;
use crate::state::toasts;

/// Selection passed to the parent — everything needed to patch the config
/// of the `PnexFunction` node (snapshot of the **current** version at the
/// time of the choice).
#[derive(Clone)]
pub(crate) struct FunctionPick {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) version_number: i64,
    pub(crate) language: pnex_core::FunctionLanguage,
    pub(crate) inputs: Vec<pnex_core::FunctionInput>,
    pub(crate) outputs: Vec<pnex_core::FunctionOutput>,
}

/// `current_id` = the function already pinned on the node (0 = none): the
/// select shows it instead of the placeholder.
#[component]
pub(crate) fn FunctionPicker(
    can_write: bool,
    #[props(default)] current_id: i64,
    on_pick: Callback<FunctionPick>,
) -> Element {
    let generation = use_signal(|| 0u32);
    let mut loading = use_signal(|| false);
    // Selection tracked by id (names may collide or be renamed); seeded
    // from the pinned function so a deployed node does not show the
    // placeholder (phantom select).
    let mut selected_id = use_signal(move || current_id);

    let rows = use_resource(move || {
        let _ = generation();
        async move { api::functions::list().await }
    });

    // i18n labels resolved outside rsx (FnMut trap of rsx closures).
    let placeholder = t!("flows-inspector-function-pick-placeholder");

    let mut pick = move |summary: pnex_core::FunctionSummary| {
        selected_id.set(summary.id);
        loading.set(true);
        spawn(async move {
            match api::functions::detail(summary.id).await {
                Ok(detail) => {
                    on_pick.call(FunctionPick {
                        id: detail.id,
                        name: detail.name,
                        version_number: detail.current_version_number,
                        language: detail.language,
                        inputs: detail.inputs,
                        outputs: detail.outputs,
                    });
                }
                Err(err) => toasts::error(err),
            }
            loading.set(false);
        });
    };

    let rows_value: Vec<pnex_core::FunctionSummary> =
        rows.read().clone().and_then(|r| r.ok()).unwrap_or_default();
    let has_selection = rows_value.iter().any(|s| s.id == selected_id());
    rsx! {
        div { class: "space-y-1",
            label { r#for: "function-picker-select", class: FIELD_LABEL,
                {t!("flows-inspector-function-pick")}
            }
            // Remount the select once the rows arrive (keyed on their
            // count): a select rendered before its options keeps the
            // placeholder even when an option is flagged `selected` later.
            for generation_key in [rows_value.len()] {
                select {
                    key: "{generation_key}",
                    id: "function-picker-select",
                    class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm disabled:bg-gray-50 disabled:text-gray-400",
                    disabled: !can_write || loading(),
                    onchange: move |event| {
                        let Ok(picked_id) = event.value().parse::<i64>() else { return };
                        let picked = match &*rows.read() {
                            Some(Ok(rows)) => rows.iter().find(|s| s.id == picked_id).cloned(),
                            _ => None,
                        };
                        if let Some(summary) = picked {
                            pick(summary);
                        }
                    },
                    option { value: "", selected: !has_selection, disabled: true,
                        {placeholder.clone()}
                    }
                    for summary in rows_value.clone() {
                        option {
                            key: "{summary.id}",
                            value: "{summary.id}",
                            selected: selected_id() == summary.id,
                            {format!("{} (v{})", summary.name, summary.current_version_number)}
                        }
                    }
                }
            }
        }
    }
}
