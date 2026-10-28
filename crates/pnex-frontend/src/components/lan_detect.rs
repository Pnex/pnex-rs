//! Server-side LAN detection of PNeX servers (endpoint
//! `GET /api/v1/edge/lan-scan`) — component reused by the host picker
//! (wizard, rebuild modal) and the form of the Registries page.
//!
//! The server probes the /24s of its private interfaces (or the typed prefix)
//! with the contract's meta probe: a hit = a verified `pnex-server`, not
//! just an open port. Containerised deployment (bridge 172.x): type
//! the real LAN prefix in the dedicated field (outbound goes through NAT).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::components::gerbe::Gerbe;
use crate::components::icons;
use crate::state::toasts;

#[component]
pub fn LanDetect(on_pick: Callback<String>) -> Element {
    let mut scanning = use_signal(|| false);
    // Explicit "a.b.c." prefix — empty = server-side auto-detection
    // (private interfaces of the pnex-server host).
    let mut prefix = use_signal(String::new);
    // Outer None = never run; inner None = scan failed (toast).
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

    // The server refuses the scan to viewers (SEC-43): no button for them.
    if !crate::state::access::can("edge:test") {
        return rsx! {};
    }
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
                    class: "px-3 py-1.5 text-xs font-medium text-signal-700 bg-white border border-signal-200 rounded-lg hover:bg-signal-50 transition-colors disabled:opacity-50",
                    r#type: "button",
                    disabled: scanning(),
                    onclick: run,
                    if scanning() {
                        Gerbe { size: 14, class: "mr-1" }
                        {t!("edgerefs-detecting")}
                    } else {
                        icons::Search { class: "h-3.5 w-3.5 inline mr-1" }
                        {t!("edgerefs-detect")}
                    }
                }
            }
            if let Some(scan) = result().flatten() {
                if scan.hits.is_empty() {
                    p { class: "text-xs text-gray-500", {t!("edgerefs-scan-none")} }
                }
                div { class: "space-y-1",
                    for hit in scan.hits {
                        div { class: "flex items-center justify-between gap-2",
                            span { class: "text-xs text-gray-700 truncate",
                                code { class: "font-medium", "ws://{hit.host}" }
                                span { class: "text-gray-500 ml-2", "v{hit.version}" }
                            }
                            button {
                                class: "px-2 py-1 text-xs font-medium text-signal-600 hover:text-signal-800 border border-signal-200 rounded-lg hover:bg-signal-50 transition-colors shrink-0",
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
