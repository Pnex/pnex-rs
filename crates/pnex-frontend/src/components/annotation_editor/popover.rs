//! Popover live d'une annotation (D58) — partagée TourViewer / page média :
//! label, kind, couche d'origine, cible résolue ou « cible introuvable »
//! (référence morte tolérée — grisée, jamais 500, école S7), valeur live en
//! **poll 15 s** (D31 : pas de WS navigateur, école `pins_panel`) annulée à
//! la fermeture (tâche scopée au composant). Vit **hors canvas** (panneau
//! dioxus) : conteneur `pointer-events-none`, carte `auto` — jamais
//! d'overlay bloquant (constat device 2026-09-10).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{AnnotationTarget, ResolvedAnnotationItem};

use crate::components::icons;

/// Carte ancrée en bas du conteneur parent (`relative`) — le parent gère
/// l'ouverture (signal d'item sélectionné) et le `key` par item.
#[component]
pub fn AnnotationPopover(item: ResolvedAnnotationItem, on_close: Callback<()>) -> Element {
    // Valeur live : (connected, valeur de pin formatée) — None tant que le
    // premier poll n'est pas revenu.
    let mut live = use_signal(|| None::<(bool, Option<String>)>);
    // Garde du poll (une seule boucle par instance — double invocation des
    // effects de dioxus).
    let mut poll_started = use_signal(|| false);

    // Cible résolue : poll uniquement si le device est vivant (device_pk
    // posé — None si mort, école read model).
    let resolved = item.resolved.clone();
    let device_pk = resolved.as_ref().and_then(|r| r.device_pk);
    let gpio = match &item.target {
        AnnotationTarget::Pin { pin_gpio, .. } => Some(*pin_gpio as i32),
        _ => None,
    };

    use_effect(move || {
        if poll_started() || device_pk.is_none() {
            return;
        }
        poll_started.set(true);
        let pk = device_pk.unwrap();
        spawn(async move {
            loop {
                // Poll annulé à la fermeture : la tâche est scopée au
                // composant (tuée au démontage, école poll TourViewer).
                if let Ok(resp) = crate::api::pins::pins(pk).await {
                    let value = gpio.and_then(|g| {
                        resp.pins
                            .iter()
                            .find(|p| p.gpio == g)
                            .and_then(|p| p.last_value.as_ref().map(format_value))
                    });
                    live.set(Some((resp.connected, value)));
                }
                crate::util::sleep(std::time::Duration::from_secs(15)).await;
            }
        });
    });

    // ─── Précalculs (pas de `let` dans le corps rsx) ───
    let kind_key = match item.kind.as_str() {
        "device" => "annot-kind-device",
        "pin" => "annot-kind-pin",
        "status" => "annot-kind-status",
        _ => "annot-kind-note",
    };
    let is_dead = resolved.as_ref().map(|r| r.dead).unwrap_or(false);
    let target_block = target_text(&item, is_dead);
    let live_now = live.cloned();

    rsx! {
        // Conteneur non bloquant : la vue reste navigable dessous.
        div { class: "absolute inset-x-0 bottom-0 z-20 pointer-events-none p-3",
            div { class: "pointer-events-auto bg-white rounded-xl shadow-xl border border-gray-200 p-4 max-w-md mx-auto",
                div { class: "flex items-start justify-between gap-3",
                    div { class: "min-w-0",
                        div { class: "flex items-center gap-2",
                            span { class: "inline-block h-3 w-3 rounded-full {annot_dot_color(&item.kind)}" }
                            span { class: "text-sm font-semibold text-gray-900 truncate",
                                {
                                    if item.label.is_empty() { t!(kind_key).to_string() } else { item.label.clone() }
                                }
                            }
                            span { class: "text-[11px] uppercase tracking-wide text-gray-400",
                                {t!(kind_key)}
                            }
                        }
                        div { class: "text-xs text-gray-500 mt-0.5 truncate",
                            span { class: "font-medium text-gray-400", {t!("annot-popover-layer")} }
                            {" · "}
                            {item.layer_name.clone()}
                        }
                    }
                    button {
                        class: "text-gray-400 hover:text-gray-600 shrink-0",
                        onclick: move |_| on_close.call(()),
                        aria_label: "Fermer",
                        icons::X { class: "h-4 w-4" }
                    }
                }

                // Cible : résolue ou morte (grisée — référence tolérée, S7).
                div { class: if is_dead { "mt-3 text-xs rounded-lg px-3 py-2 bg-gray-50 text-gray-400 border border-gray-100" } else { "mt-3 text-xs rounded-lg px-3 py-2 bg-blue-50 text-blue-800 border border-blue-100" },
                    {target_block}
                }

                // Valeur live (device vivant ; kind note → pas de section).
                if device_pk.is_some() && item.kind != "note" {
                    div { class: "mt-2 flex items-center gap-2 text-xs text-gray-600 px-3",
                        span { class: "font-medium text-gray-400", {t!("annot-popover-value")} }
                        match live_now {
                            Some((connected, value)) => rsx! {
                                if item.kind == "pin" {
                                    span { class: "font-mono text-gray-900",
                                        {value.unwrap_or_else(|| t!("annot-popover-none").to_string())}
                                    }
                                }
                                span { class: if connected { "ml-auto inline-flex items-center gap-1 text-emerald-600" } else { "ml-auto inline-flex items-center gap-1 text-gray-400" },
                                    span { class: if connected { "h-2 w-2 rounded-full bg-emerald-500" } else { "h-2 w-2 rounded-full bg-gray-300" } }
                                    {
                                        if connected {
                                            t!("annot-popover-connected").to_string()
                                        } else {
                                            t!("annot-popover-offline").to_string()
                                        }
                                    }
                                }
                            },
                            None => rsx! {
                                span { class: "animate-spin inline-block rounded-full h-3 w-3 border-b-2 border-gray-400" }
                            },
                        }
                    }
                }
            }
        }
    }
}

/// Couleur du point de kind (cohérente avec `.pnex-annot-{kind}`) — aussi
/// consommée par les marqueurs plats de la page média.
pub fn annot_dot_color(kind: &str) -> &'static str {
    match kind {
        "device" => "bg-blue-600",
        "pin" => "bg-violet-600",
        "status" => "bg-emerald-600",
        _ => "bg-amber-600",
    }
}

/// Bloc cible : note → texte ; device/pin/status → slug (+ gpio pour une
/// pin) ; morte → « cible introuvable » (grisée).
fn target_text(item: &ResolvedAnnotationItem, is_dead: bool) -> String {
    if is_dead {
        return t!("annot-popover-dead").to_string();
    }
    match &item.target {
        AnnotationTarget::Note { text } => text.clone(),
        AnnotationTarget::Device { device_id } => {
            format!("{} · {device_id}", t!("annot-kind-device"))
        }
        AnnotationTarget::Status { device_id } => {
            format!("{} · {device_id}", t!("annot-kind-status"))
        }
        AnnotationTarget::Pin {
            device_id,
            pin_gpio,
            ..
        } => format!("{} · {device_id} · gpio {pin_gpio}", t!("annot-kind-pin")),
    }
}

/// Valeur de pin live formatée pour l'affichage (JSON → texte plat).
fn format_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
