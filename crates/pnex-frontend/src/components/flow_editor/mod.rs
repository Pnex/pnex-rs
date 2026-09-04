//! Éditeur de flows drag & drop (Phase 5 du chantier ETL, D18).
//!
//! Ce module est le squelette monté par la page `/flows` (sous-vue pilotée
//! par le signal `selected`). L'éditeur ne parle qu'à l'API Loco — jamais
//! au runtime (garde-fou PRD, docs/architecture/flow-engine.md).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;

/// Layout 3 colonnes : palette · canevas SVG · inspecteur, surmontés de la
/// toolbar (retour, nom, statut, actions). Squelette : affiche le flow et
/// son statut ; canevas et interactions arrivent avec les tranches suivantes.
#[component]
pub fn FlowEditor(
    flow_id: i64,
    can_write: bool,
    on_back: Callback<()>,
    on_changed: Callback<()>,
) -> Element {
    let reload = use_signal(|| 0u32);

    let detail = use_resource(move || async move {
        let _ = reload();
        api::flows::detail(flow_id).await
    });

    let _ = can_write;

    rsx! {
        div { class: "space-y-4",
            div { class: "flex items-center justify-between gap-4",
                div {
                    button {
                        class: "text-sm text-blue-600 hover:text-blue-700 mb-1",
                        onclick: move |_| on_back.call(()),
                        { "← " }
                        {t!("flows-back-list")}
                    }
                    {match &*detail.value().read() {
                        Some(Ok(flow)) => rsx! {
                            h2 { class: "text-xl font-semibold text-gray-900", {flow.name.clone()} }
                        },
                        _ => rsx! {},
                    }}
                }
            }
            {match &*detail.value().read() {
                Some(Ok(flow)) => rsx! {
                    div { class: "bg-white rounded-lg shadow-sm p-10 text-center text-gray-400",
                        {format!("#{flow_id} · {}", flow.status)}
                    }
                },
                Some(Err(err)) => rsx! {
                    div { class: "bg-red-50 border border-red-200 rounded-lg p-4 text-sm text-red-700", {err.message.clone()} }
                },
                None => rsx! {
                    div { class: "text-center py-12",
                        span { class: "animate-spin inline-block rounded-full h-8 w-8 border-b-2 border-blue-600" }
                    }
                },
            }}
        }
    }
}
