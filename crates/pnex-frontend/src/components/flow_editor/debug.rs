//! Drawer de debug — feed des sorties du flow déployé (nœuds `debug`
//! builtin + sonde `pnex-display`), pollé 2 s tant qu'il est monté (le
//! parent ne le monte que si ouvert ; démonté = plus de requêtes).
//!
//! Garde-fou UX : monté seulement si les outils de debug sont actifs (mode
//! dev/debug) — le chip runtime porte `debug_tools`.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;

/// Drawer latéral du feed debug (nœuds `debug` builtin + sonde
/// `pnex-display`), pollé 2 s tant qu'il est monté. Les badges Display du
/// canvas, eux, sont pliés par le parent (cadence chip, 5 s).
#[component]
pub(crate) fn DebugDrawer(flow_id: i64, on_close: Callback<()>) -> Element {
    let mut reload = use_signal(|| 0u32);
    let entries = use_resource(move || async move {
        let _ = reload();
        api::flows::debug(flow_id).await
    });

    // Auto-entretien : re-poll toutes les 2 s tant que monté.
    let mut polling = use_signal(|| false);
    use_effect(move || {
        if polling() {
            return;
        }
        polling.set(true);
        spawn(async move {
            crate::util::sleep(std::time::Duration::from_secs(2)).await;
            polling.set(false);
            reload.with_mut(|r| *r += 1);
        });
    });

    let close = move |_| on_close.call(());

    rsx! {
        div { class: "fixed inset-0 z-40",
            // Clic hors drawer → fermeture.
            div { class: "absolute inset-0", onclick: close }
            aside { class: "absolute inset-y-0 right-0 w-96 max-w-full bg-white shadow-xl border-l border-gray-200 flex flex-col",
                div { class: "flex items-center justify-between px-4 py-3 border-b border-gray-200",
                    h3 { class: "text-sm font-semibold text-gray-900", {t!("flows-debug-title")} }
                    button {
                        class: "text-gray-400 hover:text-gray-600",
                        onclick: close,
                        { "✕" }
                    }
                }
                {match &*entries.value().read() {
                    Some(Ok(feed)) if feed.entries.is_empty() => rsx! {
                        p { class: "p-4 text-sm text-gray-400", {t!("flows-debug-empty")} }
                    },
                    Some(Ok(feed)) => rsx! {
                        ul { class: "flex-1 overflow-y-auto divide-y divide-gray-100",
                            for entry in feed.entries.clone() {
                                li { key: "{entry.seq}", class: "px-4 py-2.5 space-y-1",
                                    div { class: "flex items-center gap-2",
                                        span { class: "text-[11px] font-mono text-gray-400",
                                            {heure_label(&entry.ts)}
                                        }
                                        span { class: "text-xs font-semibold text-gray-700",
                                            {entry.name.clone().unwrap_or_else(|| entry.node_id.clone())}
                                        }
                                        if entry.source == "pnex-display" {
                                            span { class: "inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-medium bg-cyan-100 text-cyan-800",
                                                {t!("flows-debug-display-tag")}
                                            }
                                        }
                                        if let Some(topic) = &entry.topic {
                                            span { class: "text-[11px] text-gray-400 truncate",
                                                {topic.clone()}
                                            }
                                        }
                                    }
                                    pre { class: "text-[11px] font-mono whitespace-pre-wrap break-all text-gray-600 m-0",
                                        {format_msg(&entry.msg)}
                                    }
                                }
                            }
                        }
                    },
                    Some(Err(err)) => rsx! {
                        div { class: "m-4 bg-red-50 border border-red-200 rounded-lg p-3 text-sm text-red-700",
                            {err.message.clone()}
                        }
                    },
                    None => rsx! {
                        div { class: "flex-1 flex items-center justify-center",
                            span { class: "animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600" }
                        }
                    },
                }}
                div { class: "px-4 py-2 border-t border-gray-200 text-[11px] text-gray-400",
                    {t!("flows-debug-hint")}
                }
            }
        }
    }
}

/// Heure locale affichable `HH:MM:SS` depuis le RFC 3339 (UTC) du backend —
/// conversion vers le fuseau du navigateur : un découpage brutal de la
/// chaîne affichait l'heure UTC (décalée de 2 h en CEST — retour du
/// 05/09 : 21:53:54 affiché 19:53:54). Forme inattendue : rendue telle
/// quelle (jamais de panic).
fn heure_label(ts: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|ts| ts.with_timezone(&chrono::Local).format("%H:%M:%S").to_string())
        .unwrap_or_else(|_| ts.to_string())
}

/// Formate la valeur capturée : chaîne du debug builtin → tentative de
/// re-parse JSON (pretty si objet/tableau) ; objet/tableau → pretty ; scalaire
/// → verbatim.
fn format_msg(msg: &serde_json::Value) -> String {
    match msg {
        serde_json::Value::String(s) => match serde_json::from_str::<serde_json::Value>(s) {
            Ok(v @ (serde_json::Value::Object(_) | serde_json::Value::Array(_))) => {
                serde_json::to_string_pretty(&v).unwrap_or_else(|_| s.clone())
            }
            _ => s.clone(),
        },
        serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
            serde_json::to_string_pretty(msg).unwrap_or_default()
        }
        other => other.to_string(),
    }
}

/// Raccourci d'affichage du badge sous un nœud Display : scalaire verbatim,
/// objet/tableau en JSON compact tronqué (~24 caractères, sûrs pour Unicode).
pub(crate) fn display_value_label(msg: &serde_json::Value) -> String {
    const MAX: usize = 24;
    let raw = match msg {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
            serde_json::to_string(msg).unwrap_or_default()
        }
        other => other.to_string(),
    };
    if raw.chars().count() > MAX {
        let short: String = raw.chars().take(MAX).collect();
        format!("{short}…")
    } else {
        raw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heure_convertie_en_locale() {
        // La valeur attendue dépend du fuseau de la machine : on la compare
        // à la même conversion chrono (source de vérité identique). Un
        // retour au découpage brutal UTC ferait échouer ce test sur toute
        // machine hors UTC.
        let expected = chrono::DateTime::parse_from_rfc3339("2026-09-05T12:34:56Z")
            .unwrap()
            .with_timezone(&chrono::Local)
            .format("%H:%M:%S")
            .to_string();
        assert_eq!(heure_label("2026-09-05T12:34:56Z"), expected);
        // Forme inattendue : rendue telle quelle (jamais de panic).
        assert_eq!(heure_label("bizarre"), "bizarre");
    }

    #[test]
    fn badge_tronque_sans_couper_invalidement() {
        assert_eq!(display_value_label(&serde_json::json!(21.5)), "21.5");
        assert_eq!(display_value_label(&serde_json::json!("bonjour")), "bonjour");
        let long = display_value_label(&serde_json::json!({"cle_beaucoup_plus_longue": 123456789}));
        assert!(long.ends_with('…'), "{long}");
        assert!(long.chars().count() <= 25, "{long}");
    }

    #[test]
    fn msg_stringifie_repare_et_scalaire_verbatim() {
        // Chaîne JSON (debug builtin) → pretty.
        let pretty = format_msg(&serde_json::Value::String("{\"k\":1}".into()));
        assert!(pretty.contains('\n'), "{pretty}");
        // Chaîne non JSON → verbatim.
        assert_eq!(format_msg(&serde_json::Value::String("bonjour".into())), "bonjour");
        // Objet brut (pnex-display) → pretty.
        let pretty = format_msg(&serde_json::json!({"a": [1, 2]}));
        assert!(pretty.contains('\n'), "{pretty}");
        // Scalaire → verbatim.
        assert_eq!(format_msg(&serde_json::json!(21.5)), "21.5");
    }
}
