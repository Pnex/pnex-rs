//! Détection LAN des serveurs PNeX côté serveur (endpoint
//! `GET /api/v1/edge/lan-scan`) — composant réutilisé par le picker host
//! (wizard, modal rebuild) et le formulaire de la page Référentiels.
//!
//! Le serveur sonde les /24 de ses interfaces privées (ou le préfixe saisi)
//! avec la sonde meta du contrat : un hit = un `pnex-server` vérifié, pas
//! un simple port ouvert. Déploiement conteneurisé (bridge 172.x) : saisir
//! le préfixe LAN réel dans le champ dédié (l'outbound traverse le NAT).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::components::icons;
use crate::state::toasts;

#[component]
pub fn LanDetect(on_pick: Callback<String>) -> Element {
    let mut scanning = use_signal(|| false);
    // Préfixe explicite « a.b.c. » — vide = auto-détection côté serveur
    // (interfaces privées de l'hôte pnex-server).
    let mut prefix = use_signal(String::new);
    // Outer None = jamais lancé ; inner None = scan en erreur (toast).
    let mut result = use_signal(|| None::<Option<pnex_core::LanScanResult>>);

    let run = move |_| {
        if scanning() {
            return;
        }
        let p = prefix.read().trim().to_string();
        let explicit = if p.is_empty() { None } else { Some(p) };
        scanning.set(true);
        result.set(None);
        spawn(async move {
            let outcome = api::hosts::lan_scan(explicit.as_deref()).await;
            scanning.set(false);
            match outcome {
                Ok(scan) => result.set(Some(Some(scan))),
                Err(err) => {
                    result.set(Some(None));
                    toasts::error(err);
                }
            }
        });
    };

    rsx! {
        div { class: "rounded-lg border border-gray-200 bg-gray-50 p-3 space-y-2",
            div { class: "flex items-center gap-2",
                input {
                    class: "w-36 px-2 py-1.5 border border-gray-300 rounded-lg text-xs",
                    r#type: "text",
                    placeholder: t!("edgerefs-scan-prefix"),
                    value: "{prefix}",
                    oninput: move |e| prefix.set(e.value()),
                }
                button {
                    class: "px-3 py-1.5 text-xs font-medium text-blue-700 bg-white border border-blue-200 rounded-lg hover:bg-blue-50 transition-colors disabled:opacity-50",
                    r#type: "button",
                    disabled: scanning(),
                    onclick: run,
                    if scanning() {
                        span { class: "animate-spin inline-block rounded-full h-3 w-3 border-b-2 border-blue-600 mr-1" }
                        {t!("edgerefs-detecting")}
                    } else {
                        icons::Search { class: "h-3.5 w-3.5 inline mr-1" }
                        {t!("edgerefs-detect")}
                    }
                }
            }
            if let Some(scan) = result().filter(|r| r.is_some()).map(|r| r.unwrap()) {
                if scan.hits.is_empty() {
                    p { class: "text-xs text-gray-400", {t!("edgerefs-scan-none")} }
                }
                div { class: "space-y-1",
                    for hit in scan.hits {
                        div { class: "flex items-center justify-between gap-2",
                            span { class: "text-xs text-gray-700 truncate",
                                code { class: "font-medium", "ws://{hit.host}" }
                                span { class: "text-gray-400 ml-2", "v{hit.version}" }
                            }
                            button {
                                class: "px-2 py-1 text-xs font-medium text-blue-600 hover:text-blue-800 border border-blue-200 rounded-lg hover:bg-blue-50 transition-colors shrink-0",
                                r#type: "button",
                                onclick: move |_| on_pick.call(hit.host.clone()),
                                {t!("edgerefs-scan-register")}
                            }
                        }
                    }
                }
            }
        }
    }
}
