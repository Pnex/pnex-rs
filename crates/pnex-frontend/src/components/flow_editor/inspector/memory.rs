use super::helpers::*;
use super::*;

use pnex_core::memory::{
    valid_memory_key, MemoryReadConfig, MemoryWriteConfig, MEMORY_KEY_MAX_LEN,
    MEMORY_READ_MAX_KEYS, MEMORY_TTL_MAX_SECS,
};

// ─────────────── shared memory (Valkey): memory-write / memory-read ───────────────

/// Duration units of the lifetime / max-age pickers (seconds per unit).
const DURATION_UNITS: [(&str, u32); 4] = [("s", 1), ("min", 60), ("h", 3600), ("d", 86_400)];

/// Largest unit dividing `secs` exactly → (amount, unit seconds).
fn split_duration(secs: u32) -> (u32, u32) {
    DURATION_UNITS
        .iter()
        .rev()
        .find(|(_, u)| secs > 0 && secs % u == 0)
        .map(|(_, u)| (secs / u, *u))
        .unwrap_or((secs, 1))
}

fn unit_label(unit: u32) -> String {
    match unit {
        60 => t!("flows-memory-unit-min").to_string(),
        3600 => t!("flows-memory-unit-h").to_string(),
        86_400 => t!("flows-memory-unit-d").to_string(),
        _ => t!("flows-memory-unit-s").to_string(),
    }
}

/// Amount + unit picker editing a duration in seconds.
#[component]
fn DurationInput(secs: u32, disabled: bool, onchange: EventHandler<u32>) -> Element {
    let (amount, unit) = split_duration(secs);
    rsx! {
        div { class: "flex gap-2",
            input {
                class: "w-24 px-2 py-1.5 border border-gray-300 rounded-lg text-sm",
                r#type: "number",
                min: "0",
                value: "{amount}",
                disabled,
                onchange: move |event| {
                    if let Ok(n) = event.value().trim().parse::<u32>() {
                        onchange.call(n.saturating_mul(unit));
                    }
                },
            }
            select {
                class: "px-2 py-1 border border-gray-300 rounded-lg text-sm",
                disabled,
                onchange: move |event| {
                    if let Ok(next) = event.value().parse::<u32>() {
                        onchange.call(amount.saturating_mul(next));
                    }
                },
                for (_, u) in DURATION_UNITS {
                    option { key: "{u}", value: "{u}", selected: u == unit, {unit_label(u)} }
                }
            }
        }
    }
}

/// Keys suggested by the pickers: keys written by memory-write nodes of the
/// open graph + live keys of the org (backend listing), sorted, unique.
fn key_suggestions(cx: &EditorCx, live: &[pnex_core::memory::MemoryKeyInfo]) -> Vec<String> {
    let mut keys: std::collections::BTreeSet<String> = cx
        .graph
        .read()
        .nodes
        .iter()
        .filter_map(|n| match &n.kind {
            FlowNodeKind::MemoryWrite { config } if !config.key.is_empty() => {
                Some(config.key.clone())
            }
            _ => None,
        })
        .collect();
    keys.extend(live.iter().map(|k| k.key.clone()));
    keys.into_iter().collect()
}

/// Memory write inspector: key + lifetime.
#[component]
pub(super) fn MemoryWriteForm(
    mut cx: EditorCx,
    initial: MemoryWriteConfig,
    can_write: bool,
) -> Element {
    let mut key = use_signal(move || initial.key.clone());
    let mut ttl = use_signal(move || initial.ttl_secs);
    let key_bad = !valid_memory_key(&key());
    let ttl_bad = ttl() == 0 || ttl() > MEMORY_TTL_MAX_SECS;

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-memory-write-help")} }
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-memory-key")} }
                input {
                    class: if key_bad { "w-full px-2 py-1.5 border border-red-400 bg-red-50 rounded-lg text-sm font-mono" } else { "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm font-mono" },
                    r#type: "text",
                    value: "{key}",
                    placeholder: "cycle.p1",
                    disabled: !can_write,
                    oninput: move |event| {
                        let raw = event.value().trim().to_string();
                        key.set(raw.clone());
                        patch_selected(&mut cx, move |node: &mut FlowNode| {
                            if let FlowNodeKind::MemoryWrite { config } = &mut node.kind {
                                config.key = raw;
                            }
                        });
                    },
                }
                span { class: if key_bad { "text-xs text-red-600 mt-1 block" } else { "text-xs text-gray-400 mt-1 block" },
                    {t!("flows-memory-key-rule", max: MEMORY_KEY_MAX_LEN)}
                }
            }
            div {
                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-memory-ttl")} }
                DurationInput {
                    secs: ttl(),
                    disabled: !can_write,
                    onchange: move |secs: u32| {
                        ttl.set(secs);
                        patch_selected(&mut cx, move |node: &mut FlowNode| {
                            if let FlowNodeKind::MemoryWrite { config } = &mut node.kind {
                                config.ttl_secs = secs;
                            }
                        });
                    },
                }
                span { class: if ttl_bad { "text-xs text-red-600 mt-1 block" } else { "text-xs text-gray-400 mt-1 block" },
                    {t!("flows-memory-ttl-help")}
                }
            }
        }
    }
}

/// Applies a new read config: rewires the key ports by key, then stores it.
fn commit_read(cx: &mut EditorCx, mut cfg: Signal<MemoryReadConfig>, next: MemoryReadConfig) {
    let old = cfg.peek().clone();
    cfg.set(next.clone());
    let Some(id) = cx.selected_node.peek().clone() else {
        return;
    };
    cx.update_graph(move |graph| {
        state::rewire_memory_read(graph, &id, &old.keys, &next.keys);
        if let Some(node) = graph.nodes.iter_mut().find(|n| n.id == id) {
            if let FlowNodeKind::MemoryRead { config } = &mut node.kind {
                *config = next;
            }
        }
    });
}

/// Memory read inspector: key list (suggestions from the graph and the
/// org live keys) + max age.
#[component]
pub(super) fn MemoryReadForm(
    mut cx: EditorCx,
    initial: MemoryReadConfig,
    can_write: bool,
) -> Element {
    let cfg = use_signal(move || initial.clone());
    let mut draft = use_signal(String::new);
    let live = use_resource(|| async { crate::api::memory::keys().await.unwrap_or_default() });
    let live_keys = live.read().clone().unwrap_or_default();
    let suggestions = key_suggestions(&cx, &live_keys);
    let current = cfg();
    let draft_value = draft();
    let draft_ok = valid_memory_key(draft_value.trim())
        && !current.keys.iter().any(|k| k == draft_value.trim())
        && current.keys.len() < MEMORY_READ_MAX_KEYS;
    let max_age = current.max_age_secs.max(0.0).round() as u32;

    let mut add_key = move || {
        let k = draft.peek().trim().to_string();
        let mut next = cfg.peek().clone();
        if !valid_memory_key(&k)
            || next.keys.contains(&k)
            || next.keys.len() >= MEMORY_READ_MAX_KEYS
        {
            return;
        }
        next.keys.push(k);
        draft.set(String::new());
        commit_read(&mut cx, cfg, next);
    };

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-memory-read-help")} }
            div {
                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-memory-keys")} }
                if current.keys.is_empty() {
                    p { class: "text-xs text-amber-600 mb-1", {t!("flows-memory-keys-empty")} }
                }
                ul { class: "space-y-1 mb-2",
                    for (i, k) in current.keys.iter().cloned().enumerate() {
                        li { key: "{k}", class: "flex items-center justify-between gap-2 px-2 py-1 rounded bg-gray-50 border border-gray-200",
                            code { class: "text-xs", "{k}" }
                            if can_write {
                                button {
                                    class: "text-xs text-gray-400 hover:text-red-600",
                                    title: t!("flows-memory-key-remove").to_string(),
                                    onclick: move |_| {
                                        let mut next = cfg.peek().clone();
                                        if i < next.keys.len() {
                                            next.keys.remove(i);
                                            commit_read(&mut cx, cfg, next);
                                        }
                                    },
                                    "✕"
                                }
                            }
                        }
                    }
                }
                if can_write {
                    div { class: "flex gap-2",
                        input {
                            class: "flex-1 min-w-0 px-2 py-1.5 border border-gray-300 rounded-lg text-sm font-mono",
                            r#type: "text",
                            list: "pnex-memory-keys",
                            value: "{draft_value}",
                            placeholder: "cycle.p1",
                            oninput: move |event| draft.set(event.value()),
                            onkeydown: move |event| {
                                if event.key() == Key::Enter {
                                    add_key();
                                }
                            },
                        }
                        button {
                            class: "px-3 py-1.5 rounded-lg text-sm bg-gray-900 text-white disabled:opacity-40",
                            disabled: !draft_ok,
                            onclick: move |_| add_key(),
                            {t!("flows-memory-key-add")}
                        }
                    }
                    datalist { id: "pnex-memory-keys",
                        for k in suggestions {
                            option { key: "{k}", value: "{k}" }
                        }
                    }
                    span { class: "text-xs text-gray-400 mt-1 block",
                        {t!("flows-memory-key-rule", max: MEMORY_KEY_MAX_LEN)}
                    }
                }
            }
            div {
                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-memory-max-age")} }
                DurationInput {
                    secs: max_age,
                    disabled: !can_write,
                    onchange: move |secs: u32| {
                        let mut next = cfg.peek().clone();
                        next.max_age_secs = f64::from(secs);
                        commit_read(&mut cx, cfg, next);
                    },
                }
                span { class: "text-xs text-gray-400 mt-1 block", {t!("flows-memory-max-age-help")} }
            }
        }
    }
}
