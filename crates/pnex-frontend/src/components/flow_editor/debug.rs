//! Debug drawer — feed of the deployed flow's outputs (builtin `debug`
//! nodes + `pnex-display` probe), polled every 2 s while mounted (the
//! parent mounts it only when open; unmounted = no more requests).
//!
//! UX guard: mounted only if debug tools are enabled (dev/debug
//! mode) — the runtime chip carries `debug_tools`.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::components::gerbe::Gerbe;

/// Side drawer of the debug feed (builtin `debug` nodes + `pnex-display`
/// probe), polled every 2 s while mounted. The canvas Display badges,
/// for their part, are folded by the parent (chip cadence, 5 s).
#[component]
pub(crate) fn DebugDrawer(flow_id: i64, on_close: Callback<()>) -> Element {
    // Expanded entries (JSON object/array) — key = seq, stable within the
    // 5 min window: the 2 s re-poll does not collapse what the user
    // opened.
    let mut expanded = use_signal(std::collections::HashSet::<u64>::new);
    let mut reload = use_signal(|| 0u32);
    let entries = use_resource(move || async move {
        let _ = reload();
        api::flows::debug(flow_id).await
    });

    // Self-sustaining: re-poll every 2 s while mounted.
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
            // Click outside the drawer → close.
            div { class: "absolute inset-0", onclick: close }
            aside { class: "pnex-drawer-in-right absolute inset-y-0 right-0 w-96 max-w-full bg-white shadow-xl border-l border-gray-200 flex flex-col",
                div { class: "flex items-center justify-between px-4 py-3 border-b border-gray-200",
                    h3 { class: "text-sm font-semibold text-gray-900", {t!("flows-debug-title")} }
                    button {
                        class: "text-gray-500 hover:text-gray-600",
                        onclick: close,
                        {"✕"}
                    }
                }
                match &*entries.value().read() {
                    Some(Ok(feed)) if feed.entries.is_empty() => rsx! {
                        p { class: "p-4 text-sm text-gray-500", {t!("flows-debug-empty")} }
                    },
                    Some(Ok(feed)) => rsx! {
                        ul { class: "flex-1 overflow-y-auto divide-y divide-gray-100",
                            for entry in feed.entries.clone() {
                                li { key: "{entry.seq}", class: "px-4 py-2.5 space-y-1",
                                    div { class: "flex items-center gap-2",
                                        span { class: "text-xs font-mono text-gray-500", {heure_label(&entry.ts)} }
                                        span { class: "text-xs font-semibold text-gray-700",
                                            {entry.name.clone().unwrap_or_else(|| entry.node_id.clone())}
                                        }
                                        if entry.source == "pnex-display" {
                                            span { class: "inline-flex items-center px-1.5 py-0.5 rounded text-xs font-medium bg-cyan-100 text-cyan-800",
                                                {t!("flows-debug-display-tag")}
                                            }
                                        }
                                        if entry.source == "pnex-status" {
                                            span { class: "inline-flex items-center px-1.5 py-0.5 rounded text-xs font-medium bg-amber-100 text-amber-800",
                                                {t!("flows-debug-status-tag")}
                                            }
                                        }
                                        if let Some(topic) = &entry.topic {
                                            span { class: "text-xs text-gray-500 truncate", {topic.clone()} }
                                        }
                                    }
                                    if let Some(st) = node_status(&entry.source, &entry.msg) {
                                        pre { class: "text-xs font-mono whitespace-pre-wrap break-all text-gray-700 m-0",
                                            {status_pretty(&st)}
                                        }
                                    } else if let Some(pretty) = display_value_pretty(&entry.msg) {
                                        div { class: "space-y-1",
                                            button {
                                                class: "w-full text-left flex items-center gap-1.5 text-xs font-mono text-gray-500 hover:text-gray-800 transition-colors",
                                                onclick: move |_| {
                                                    let mut set = expanded.write();
                                                    if !set.remove(&entry.seq) {
                                                        set.insert(entry.seq);
                                                    }
                                                },
                                                span { class: "select-none shrink-0",
                                                    {if expanded.read().contains(&entry.seq) { "▾" } else { "▸" }}
                                                }
                                                span { class: "truncate", {display_value_compact(&entry.msg)} }
                                                span { class: "shrink-0 font-sans text-gray-500",
                                                    {t!("flows-debug-json-lines", count : pretty.lines().count())}
                                                }
                                            }
                                            if expanded.read().contains(&entry.seq) {
                                                pre { class: "text-xs font-mono whitespace-pre-wrap break-all text-gray-600 m-0",
                                                    {pretty}
                                                }
                                            }
                                        }
                                    } else {
                                        pre { class: "text-xs font-mono whitespace-pre-wrap break-all text-gray-600 m-0",
                                            {format_msg(&entry.msg)}
                                        }
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
                            Gerbe { size: 36 }
                        }
                    },
                }
                div { class: "px-4 py-2 border-t border-gray-200 text-xs text-gray-500",
                    {t!("flows-debug-hint")}
                }
            }
        }
    }
}

/// Displayable local time `HH:MM:SS` from the backend's RFC 3339 (UTC) —
/// converted to the browser's time zone: a crude slicing of the
/// string showed UTC time (2 h off in CEST — feedback of
/// 05/09: 21:53:54 shown as 19:53:54). Unexpected shape: rendered as
/// is (never a panic).
/// Node status carried by a `pnex-status` debug entry, if any.
fn node_status(source: &str, msg: &serde_json::Value) -> Option<pnex_core::vision::NodeStatus> {
    serde_json::from_value(msg.clone())
        .ok()
        .filter(|_| source == "pnex-status")
}

fn heure_label(ts: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|ts| {
            ts.with_timezone(&chrono::Local)
                .format("%H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|_| ts.to_string())
}

/// Live badge of a Display node: truncated shorthand (canvas pill) +
/// pretty view (modal) if JSON object/array — None = scalar (no
/// expanding).
#[derive(Clone, PartialEq)]
pub(crate) struct DisplayBadge {
    pub(crate) label: String,
    pub(crate) pretty: Option<String>,
    /// Camera/vision node status (D103) — label and colour are derived at
    /// render time from it (localized).
    pub(crate) status: Option<pnex_core::vision::NodeStatus>,
}

/// Short localized badge text of a node status: state + main counter.
pub(crate) fn status_label(st: &pnex_core::vision::NodeStatus) -> String {
    let state = status_text(&st.code);
    let stat = |k: &str| st.stats.get(k).copied().unwrap_or(0);
    // "No frames · 0 frames" said it twice.
    if st.code.ends_with("-no-frames") && stat("received") == 0 && stat("analysed") == 0 {
        return state;
    }
    let counter = if st.code.starts_with("vision-") {
        t!("flow-status-count-analysed", analysed: stat("analysed"), emitted: stat("emitted"))
            .to_string()
    } else if st.code.starts_with("control-") {
        t!("flow-status-count-commands", commands: stat("commands")).to_string()
    } else if st.code.starts_with("calc-") || st.code.starts_with("device-write-") {
        t!("flow-status-count-passed", passed: stat("passed"), rejected: stat("rejected"))
            .to_string()
    } else {
        t!("flow-status-count-frames", received: stat("received")).to_string()
    };
    format!("{state} · {counter}")
}

/// Localized text of a status code (unknown code: verbatim).
pub(crate) fn status_text(code: &str) -> String {
    match code {
        "camera-streaming" => t!("flow-status-camera-streaming").to_string(),
        "camera-no-frames" => t!("flow-status-camera-no-frames").to_string(),
        "camera-bus-unavailable" => t!("flow-status-camera-bus-unavailable").to_string(),
        "vision-model-loading" => t!("flow-status-vision-model-loading").to_string(),
        "vision-model-load-failed" => t!("flow-status-vision-model-load-failed").to_string(),
        "vision-no-frames" => t!("flow-status-vision-no-frames").to_string(),
        "vision-running" => t!("flow-status-vision-running").to_string(),
        "ai-call-failed" => t!("flow-status-ai-call-failed").to_string(),
        "control-listening" => t!("flow-status-control-listening").to_string(),
        "control-bus-unavailable" => t!("flow-status-control-bus-unavailable").to_string(),
        "weather-updated" => t!("flow-status-weather-updated").to_string(),
        "weather-waiting" => t!("flow-status-weather-waiting").to_string(),
        "weather-unavailable" => t!("flow-status-weather-unavailable").to_string(),
        "calc-ok" => t!("flow-status-calc-ok").to_string(),
        "calc-input-rejected" => t!("flow-status-calc-input-rejected").to_string(),
        "device-write-ok" => t!("flow-status-device-write-ok").to_string(),
        "device-write-rejected" => t!("flow-status-device-write-rejected").to_string(),
        "ontology-ok" => t!("flow-status-ontology-ok").to_string(),
        "ontology-refused" => t!("flow-status-ontology-refused").to_string(),
        other => other.to_string(),
    }
}

/// Multi-line localized detail of a status (badge click / drawer): state,
/// runtime diagnostic, every counter.
pub(crate) fn status_pretty(st: &pnex_core::vision::NodeStatus) -> String {
    let mut out = status_text(&st.code);
    if let Some(d) = &st.detail {
        out.push('\n');
        out.push_str(d);
    }
    for (k, v) in &st.stats {
        out.push_str(&format!("\n{}: {v}", status_stat_text(k)));
    }
    out
}

/// Localized counter name (unknown key: verbatim).
fn status_stat_text(key: &str) -> String {
    match key {
        "received" => t!("flow-status-stat-received").to_string(),
        "commands" => t!("flow-status-stat-commands").to_string(),
        "throttled" => t!("flow-status-stat-throttled").to_string(),
        "emitted" => t!("flow-status-stat-emitted").to_string(),
        "not_ready" => t!("flow-status-stat-not-ready").to_string(),
        "stale" => t!("flow-status-stat-stale").to_string(),
        "no_frame" => t!("flow-status-stat-no-frame").to_string(),
        "failed" => t!("flow-status-stat-failed").to_string(),
        "analysed" => t!("flow-status-stat-analysed").to_string(),
        "below_threshold" => t!("flow-status-stat-below-threshold").to_string(),
        "last_infer_ms" => t!("flow-status-stat-last-infer-ms").to_string(),
        "passed" => t!("flow-status-stat-passed").to_string(),
        "rejected" => t!("flow-status-stat-rejected").to_string(),
        other => other.to_string(),
    }
}

/// **Expandable** pretty view of the captured value: JSON object/array —
/// direct (`pnex-display` probe) or by re-parsing a string (builtin
/// debug) — None if scalar (verbatim, no expanding).
pub(crate) fn display_value_pretty(msg: &serde_json::Value) -> Option<String> {
    match msg {
        serde_json::Value::String(s) => match serde_json::from_str::<serde_json::Value>(s) {
            Ok(v @ (serde_json::Value::Object(_) | serde_json::Value::Array(_))) => {
                serde_json::to_string_pretty(&v).ok()
            }
            _ => None,
        },
        serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
            serde_json::to_string_pretty(msg).ok()
        }
        _ => None,
    }
}

/// Single compact line (collapsed drawer entry): verbatim content of
/// strings, compact JSON for object/array, truncated to ~120 characters
/// (Unicode-safe).
pub(crate) fn display_value_compact(msg: &serde_json::Value) -> String {
    const MAX: usize = 120;
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

/// Formats the captured value: builtin debug string → JSON re-parse
/// attempt (pretty if object/array); object/array → pretty; scalar
/// → verbatim. (Non-expandable drawer entries.)
fn format_msg(msg: &serde_json::Value) -> String {
    match display_value_pretty(msg) {
        Some(pretty) => pretty,
        None => match msg {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        },
    }
}

/// Display shorthand of the badge under a Display node: scalar verbatim,
/// object/array as truncated compact JSON (~24 characters, Unicode-safe).
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

/// Badge text of a debug node: its message parsed back when the builtin
/// stringified it, the `summary` field when there is one (vision
/// detections: `person 87%, dog 64%`), else the compact value.
pub(crate) fn debug_value_label(msg: &serde_json::Value) -> String {
    let parsed = match msg {
        serde_json::Value::String(s) => {
            serde_json::from_str::<serde_json::Value>(s).unwrap_or_else(|_| msg.clone())
        }
        other => other.clone(),
    };
    match parsed.get("summary").and_then(|v| v.as_str()) {
        Some(summary) if !summary.is_empty() => {
            display_value_label(&serde_json::Value::String(summary.to_string()))
        }
        _ => display_value_label(&parsed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_label_prefers_summary() {
        let msg = serde_json::Value::String(r#"{"summary":"person 87%","count":1}"#.into());
        assert_eq!(debug_value_label(&msg), "person 87%");
        assert_eq!(debug_value_label(&serde_json::json!(42)), "42");
        assert_eq!(debug_value_label(&serde_json::json!("hello")), "hello");
    }

    #[test]
    fn heure_convertie_en_locale() {
        // The expected value depends on the machine's time zone: we compare
        // it to the same chrono conversion (identical source of truth). A
        // return to the crude UTC slicing would fail this test on any
        // non-UTC machine.
        let expected = chrono::DateTime::parse_from_rfc3339("2026-09-05T12:34:56Z")
            .unwrap()
            .with_timezone(&chrono::Local)
            .format("%H:%M:%S")
            .to_string();
        assert_eq!(heure_label("2026-09-05T12:34:56Z"), expected);
        // Unexpected shape: rendered as is (never a panic).
        assert_eq!(heure_label("bizarre"), "bizarre");
    }

    #[test]
    fn badge_tronque_sans_couper_invalidement() {
        assert_eq!(display_value_label(&serde_json::json!(21.5)), "21.5");
        assert_eq!(
            display_value_label(&serde_json::json!("bonjour")),
            "bonjour"
        );
        let long = display_value_label(&serde_json::json!({"cle_beaucoup_plus_longue": 123456789}));
        assert!(long.ends_with('…'), "{long}");
        assert!(long.chars().count() <= 25, "{long}");
    }

    #[test]
    fn msg_stringifie_repare_et_scalaire_verbatim() {
        // JSON string (builtin debug) → pretty.
        let pretty = format_msg(&serde_json::Value::String("{\"k\":1}".into()));
        assert!(pretty.contains('\n'), "{pretty}");
        // Non-JSON string → verbatim.
        assert_eq!(
            format_msg(&serde_json::Value::String("bonjour".into())),
            "bonjour"
        );
        // Raw object (pnex-display) → pretty.
        let pretty = format_msg(&serde_json::json!({"a": [1, 2]}));
        assert!(pretty.contains('\n'), "{pretty}");
        // Scalaire → verbatim.
        assert_eq!(format_msg(&serde_json::json!(21.5)), "21.5");
    }

    #[test]
    fn pretty_si_objet_tableau_compact_pour_le_replie() {
        // Direct object (pnex-display probe) → expandable pretty view.
        let pretty = display_value_pretty(&serde_json::json!({"a": [1, 2]})).unwrap();
        assert!(pretty.contains('\n'), "{pretty}");
        // String containing JSON (builtin debug) → repaired too.
        assert!(display_value_pretty(&serde_json::Value::String("{\"k\":1}".into())).is_some());
        // Non-JSON string and scalars → no expanding.
        assert!(display_value_pretty(&serde_json::Value::String("bonjour".into())).is_none());
        assert!(display_value_pretty(&serde_json::json!(21.5)).is_none());
        assert!(display_value_pretty(&serde_json::json!(true)).is_none());
        // Collapsed: compact JSON, truncated to ~120 characters.
        assert_eq!(
            display_value_compact(&serde_json::json!({"a": 1})),
            r#"{"a":1}"#
        );
        let long = display_value_compact(&serde_json::json!({
            "bloc": "x".repeat(200)
        }));
        assert!(long.ends_with('…'), "{long}");
        assert!(long.chars().count() <= 121, "{long}");
    }
}
