use super::*;

use pnex_core::ui_control::{
    valid_control_key, ControlKind, ControlSourceConfig, ControlSpec, CreateUiControl, UiControl,
    CONTROL_SOURCE_MAX,
};
use uuid::Uuid;

// ─────────────── control-source (D127) ───────────────

/// Applies a new config: rewires the ports by control id, then stores it.
fn commit(cx: &mut EditorCx, mut cfg: Signal<ControlSourceConfig>, next: ControlSourceConfig) {
    let old = cfg.peek().clone();
    cfg.set(next.clone());
    let Some(id) = cx.selected_node.peek().clone() else {
        return;
    };
    cx.update_graph(move |graph| {
        state::rewire_control_source(graph, &id, &old.controls, &next.controls);
        if let Some(node) = graph.nodes.iter_mut().find(|n| n.id == id) {
            if let FlowNodeKind::ControlSource { config } = &mut node.kind {
                *config = next;
            }
        }
    });
}

/// Localized name of a control kind.
pub(crate) fn kind_text(kind: ControlKind) -> String {
    match kind {
        ControlKind::Switch => t!("controls-kind-switch").to_string(),
        ControlKind::Slider => t!("controls-kind-slider").to_string(),
        ControlKind::Button => t!("controls-kind-button").to_string(),
        ControlKind::Number => t!("controls-kind-number").to_string(),
    }
}

/// Control source inspector: the org controls to listen to (one output
/// port each, in pick order), replay at start, inline creation.
#[component]
pub(super) fn ControlSourceForm(
    mut cx: EditorCx,
    initial: ControlSourceConfig,
    can_write: bool,
) -> Element {
    let cfg = use_signal(move || initial.clone());
    let mut reload = use_signal(|| 0u32);
    let controls = use_resource(move || async move {
        let _ = reload();
        api::controls::list().await.unwrap_or_default()
    });
    let all: Vec<UiControl> = controls.read().clone().unwrap_or_default();
    let current = cfg();
    let full = current.controls.len() >= CONTROL_SOURCE_MAX;
    // Listed ids absent from the org (deleted meanwhile): kept visible so
    // the user removes them explicitly (the deploy gate rejects them).
    let missing: Vec<Uuid> = current
        .controls
        .iter()
        .filter(|id| controls.read().is_some() && !all.iter().any(|c| c.id == **id))
        .copied()
        .collect();

    let mut toggle = move |id: Uuid| {
        let mut next = cfg.peek().clone();
        if let Some(pos) = next.controls.iter().position(|c| *c == id) {
            next.controls.remove(pos);
        } else if next.controls.len() < CONTROL_SOURCE_MAX {
            next.controls.push(id);
        }
        commit(&mut cx, cfg, next);
    };

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-control-source-help")} }
            if current.controls.is_empty() {
                p { class: "text-xs text-amber-600", {t!("flows-control-source-empty")} }
            }
            if all.is_empty() && controls.read().is_some() {
                p { class: "text-xs text-gray-500", {t!("flows-control-source-none-in-org")} }
            }
            ul { class: "space-y-1",
                for c in all.clone() {
                    ControlRow {
                        key: "{c.id}",
                        control: c.clone(),
                        checked: current.controls.contains(&c.id),
                        port: current.controls.iter().position(|id| *id == c.id),
                        disabled: !can_write || (full && !current.controls.contains(&c.id)),
                        on_toggle: move |id| toggle(id),
                    }
                }
                for id in missing {
                    li {
                        key: "{id}",
                        class: "flex items-center justify-between gap-2 px-2 py-1 rounded bg-red-50 border border-red-200",
                        span { class: "text-xs text-red-700", {t!("flows-control-source-missing")} }
                        if can_write {
                            button {
                                class: "text-xs text-gray-400 hover:text-red-600",
                                onclick: move |_| toggle(id),
                                "✕"
                            }
                        }
                    }
                }
            }
            label { class: "flex items-center gap-2 text-sm",
                input {
                    r#type: "checkbox",
                    checked: current.emit_on_start,
                    disabled: !can_write,
                    onchange: move |event| {
                        let mut next = cfg.peek().clone();
                        next.emit_on_start = event.checked();
                        commit(&mut cx, cfg, next);
                    },
                }
                {t!("flows-control-source-emit-on-start")}
            }
            span { class: "text-xs text-gray-400 block",
                {t!("flows-control-source-emit-on-start-hint")}
            }
            if can_write && !full {
                QuickCreate {
                    on_created: move |id: Uuid| {
                        reload += 1;
                        toggle(id);
                    },
                }
            }
        }
    }
}

/// One selectable org control (checkbox, kind, key, output port).
#[component]
fn ControlRow(
    control: UiControl,
    checked: bool,
    port: Option<usize>,
    disabled: bool,
    on_toggle: EventHandler<Uuid>,
) -> Element {
    let id = control.id;
    let kind = kind_text(control.spec.kind);
    rsx! {
        li { class: "flex items-center gap-2 px-2 py-1 rounded bg-gray-50 border border-gray-200",
            input {
                r#type: "checkbox",
                checked,
                disabled,
                onchange: move |_| on_toggle.call(id),
            }
            div { class: "min-w-0 flex-1",
                div { class: "text-sm truncate", "{control.label}" }
                div { class: "text-xs text-gray-500 truncate",
                    code { "{control.key}" }
                    " · {kind}"
                }
            }
            if let Some(p) = port {
                span { class: "text-xs text-blue-700 bg-blue-50 rounded px-1.5",
                    {t!("flows-control-source-port", port : p + 1)}
                }
            }
        }
    }
}

/// Inline creation of an org control (kind + key + label, default spec).
#[component]
fn QuickCreate(on_created: EventHandler<Uuid>) -> Element {
    let mut kind = use_signal(|| ControlKind::Switch);
    let mut key = use_signal(String::new);
    let mut label = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<crate::api::error::ApiError>);
    let ok = valid_control_key(key().trim()) && !label().trim().is_empty() && !busy();

    let submit = move |_| {
        let params = CreateUiControl {
            key: key.peek().trim().to_string(),
            label: label.peek().trim().to_string(),
            spec: ControlSpec::new(*kind.peek()),
        };
        busy.set(true);
        spawn(async move {
            match api::controls::create(params).await {
                Ok(c) => {
                    key.set(String::new());
                    label.set(String::new());
                    error.set(None);
                    on_created.call(c.id);
                }
                Err(e) => error.set(Some(e)),
            }
            busy.set(false);
        });
    };

    rsx! {
        details { class: "rounded-lg border border-gray-200 p-2",
            summary { class: "text-xs font-medium text-gray-600 cursor-pointer",
                {t!("flows-control-source-create")}
            }
            div { class: "mt-2 space-y-2",
                select {
                    class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                    onchange: move |event| {
                        let k = ControlKind::ALL
                            .into_iter()
                            .find(|k| k.as_str() == event.value())
                            .unwrap_or(ControlKind::Switch);
                        kind.set(k);
                    },
                    for k in ControlKind::ALL {
                        option {
                            key: "{k.as_str()}",
                            value: "{k.as_str()}",
                            selected: k == kind(),
                            {kind_text(k)}
                        }
                    }
                }
                input {
                    class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm font-mono",
                    r#type: "text",
                    placeholder: "light.room",
                    value: "{key}",
                    oninput: move |event| key.set(event.value()),
                }
                input {
                    class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm",
                    r#type: "text",
                    placeholder: t!("controls-label-placeholder").to_string(),
                    value: "{label}",
                    oninput: move |event| label.set(event.value()),
                }
                if let Some(e) = error() {
                    p { class: "text-xs text-red-600", {e.to_string()} }
                }
                button {
                    class: "w-full px-3 py-1.5 rounded-lg text-sm bg-gray-900 text-white disabled:opacity-40",
                    disabled: !ok,
                    onclick: submit,
                    {t!("flows-control-source-create-submit")}
                }
            }
        }
    }
}
