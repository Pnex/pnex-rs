//! The "+" panel (cases-workspace.md K22, K34): the case members and a
//! search over the org's objects, dragged onto an event, a lane or the
//! graph; a click selects. The undated pieces are its second tab, still a
//! drop target. Opened on demand, like the editors' palette.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{CaseAttachInput, ObjectInput, ObjectRef, OntologyQuery};
use pnex_core::ontology::KIND_NOTE;

use super::{Drag, Sel, Target, Ws};
use crate::api;
use crate::components::icons;
use crate::components::ontology::type_label_of;
use crate::components::ui::{BTN_GHOST, INPUT};
use crate::state::toasts;

const DEBOUNCE_MS: u64 = 200;

/// One draggable object row.
#[component]
fn ObjectChip(object: ObjectRef, kind: String, #[props(default)] badge: Option<usize>) -> Element {
    let ws = use_context::<Ws>();
    let selected = (ws.sel)() == Sel::Object(object.id.clone());
    let pick = object.clone();
    rsx! {
        div {
            class: "flex cursor-grab select-none touch-none items-center gap-2 rounded-md border bg-surface px-2 py-1.5 text-sm hover:bg-gray-50",
            class: if selected { "border-signal" } else { "border-line" },
            role: "button",
            tabindex: 0,
            "data-testid": "case-object",
            onpointerdown: move |e: PointerEvent| {
                let c = e.client_coordinates();
                let mut drag = ws.drag;
                drag.set(
                    Some(Drag {
                        object: pick.clone(),
                        at: (c.x, c.y),
                        moved: false,
                    }),
                );
            },
            onclick: {
                let id = object.id.clone();
                move |_| {
                    let mut sel = ws.sel;
                    sel.set(Sel::Object(id.clone()));
                }
            },
            div { class: "min-w-0 flex-1",
                div { class: "truncate text-ink", "{object.title}" }
                div { class: "truncate text-xs text-slate", "{kind}" }
            }
            if let Some(n) = badge.filter(|n| *n > 0) {
                span {
                    class: "rounded-md border border-line px-1.5 font-mono text-xs text-slate",
                    title: t!("cw-badge-events"),
                    "{n}"
                }
            }
        }
    }
}

#[component]
fn Palette() -> Element {
    let ws = use_context::<Ws>();
    let mut q = use_signal(String::new);
    let mut debounced = use_signal(String::new);
    let types = use_resource(|| async { api::ontology::types().await.unwrap_or_default() });
    let types = types.read().clone().unwrap_or_default();
    let hits = use_resource(move || {
        let term = debounced();
        async move {
            if term.is_empty() {
                return Vec::new();
            }
            let query = OntologyQuery {
                text: Some(term),
                limit: Some(20),
                ..Default::default()
            };
            api::ontology::query(&query)
                .await
                .map(|r| r.rows.into_iter().map(|r| r.object).collect::<Vec<_>>())
                .unwrap_or_default()
        }
    });
    let Some(w) = (ws.data)() else {
        return rsx! {};
    };
    // Members: what the events involve, then the undated pieces.
    let mut members: Vec<(ObjectRef, usize)> = Vec::new();
    for a in w.events.iter().flat_map(|e| &e.attachments) {
        match members.iter_mut().find(|(o, _)| o.id == a.object.id) {
            Some((_, n)) => *n += 1,
            None => members.push((a.object.clone(), 1)),
        }
    }
    for o in &w.undated {
        if !members.iter().any(|(m, _)| m.id == o.id) {
            members.push((o.clone(), 0));
        }
    }
    let term = q().trim().to_lowercase();
    let shown: Vec<(ObjectRef, usize)> = members
        .into_iter()
        .filter(|(o, _)| term.is_empty() || o.title.to_lowercase().contains(&term))
        .collect();
    let found: Vec<ObjectRef> = hits
        .read()
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter(|o| {
            o.id != w.case.id && o.type_key != "event" && !shown.iter().any(|(m, _)| m.id == o.id)
        })
        .map(|o| ObjectRef {
            id: o.id,
            type_key: o.type_key,
            title: o.title,
            native_id: o.native_id,
        })
        .collect();
    let new_note = move |_| {
        let title = t!("cw-note-untitled").to_string();
        let target = match (ws.sel)() {
            Sel::Event(id) => Some(id),
            _ => None,
        };
        spawn(async move {
            let input = ObjectInput {
                type_key: KIND_NOTE.into(),
                title,
                properties: Default::default(),
                expected_version: None,
            };
            match api::ontology::create_object(&input).await {
                Ok(note) => ws.attach(CaseAttachInput {
                    event_id: target,
                    object_id: note.id,
                    ..Default::default()
                }),
                Err(e) => toasts::error(e),
            }
        });
    };
    rsx! {
        div { class: "flex min-w-0 flex-col", "aria-label": t!("cw-objects"),
            div { class: "flex flex-col gap-2",
                input {
                    class: INPUT,
                    "aria-label": t!("cw-search"),
                    "data-testid": "case-search",
                    placeholder: t!("cw-search"),
                    value: "{q}",
                    oninput: move |e| {
                        let value = e.value();
                        q.set(value.clone());
                        spawn(async move {
                            crate::util::sleep(std::time::Duration::from_millis(DEBOUNCE_MS)).await;
                            if q.cloned() == value {
                                debounced.set(value.trim().to_string());
                            }
                        });
                    },
                }
                if shown.is_empty() && found.is_empty() {
                    p { class: "text-xs text-slate",
                        if term.is_empty() {
                            {t!("cw-objects-empty")}
                        } else {
                            {t!("cw-objects-none")}
                        }
                    }
                }
                if !shown.is_empty() {
                    p { class: "text-xs font-medium text-slate", {t!("cw-in-case")} }
                    for (o, n) in shown {
                        ObjectChip {
                            key: "{o.id}",
                            kind: type_label_of(&types, &o.type_key),
                            badge: Some(n),
                            object: o,
                        }
                    }
                }
                if !found.is_empty() {
                    p { class: "text-xs font-medium text-slate", {t!("cw-in-org")} }
                    for o in found {
                        ObjectChip {
                            key: "{o.id}",
                            kind: type_label_of(&types, &o.type_key),
                            object: o,
                        }
                    }
                }
                if ws.can_write {
                    button {
                        class: BTN_GHOST,
                        r#type: "button",
                        onclick: new_note,
                        {t!("cw-new-note")}
                    }
                }
            }
        }
    }
}

/// Undated pieces: a drop target.
#[component]
fn Tray() -> Element {
    let ws = use_context::<Ws>();
    let types = use_resource(|| async { api::ontology::types().await.unwrap_or_default() });
    let types = types.read().clone().unwrap_or_default();
    let undated = (ws.data)().map(|w| w.undated).unwrap_or_default();
    let targeted = (ws.target)() == Some(Target::Tray);
    rsx! {
        section {
            class: "flex flex-col gap-2 rounded-lg border border-dashed p-2 min-h-24",
            class: if targeted { "border-signal bg-signal-soft" } else { "border-line-strong bg-surface" },
            "aria-label": t!("cw-undated"),
            "data-testid": "case-tray",
            onpointermove: move |_| {
                if (ws.drag)().is_some() {
                    let mut target = ws.target;
                    target.set(Some(Target::Tray));
                }
            },
            onpointerleave: move |_| {
                let mut target = ws.target;
                if target() == Some(Target::Tray) {
                    target.set(None);
                }
            },
            if undated.is_empty() {
                span { class: "text-xs text-slate", {t!("cw-undated-empty")} }
            }
            for o in undated {
                ObjectChip {
                    key: "{o.id}",
                    kind: type_label_of(&types, &o.type_key),
                    object: o,
                }
            }
        }
    }
}

/// The "+" button and its panel: two tabs, the members (and the search),
/// then the undated pieces with their count.
#[component]
pub(super) fn AddMenu() -> Element {
    let ws = use_context::<Ws>();
    let mut open = ws.add_open;
    let mut undated_tab = use_signal(|| false);
    let undated = (ws.data)().map_or(0, |w| w.undated.len());
    let tab = |on: bool| {
        if on {
            "rounded-md px-2.5 py-1 text-sm bg-signal-soft text-signal font-medium"
        } else {
            "rounded-md px-2.5 py-1 text-sm text-slate hover:bg-gray-50"
        }
    };
    rsx! {
        button {
            class: crate::components::ui::BTN_FAB,
            r#type: "button",
            "aria-label": t!("cw-add"),
            "aria-expanded": open(),
            title: t!("cw-add"),
            "data-testid": "case-add",
            onclick: move |_| open.set(!open()),
            if open() {
                icons::X { class: "h-5 w-5" }
            } else {
                icons::Plus { class: "h-5 w-5" }
            }
        }
        if open() {
            div {
                class: "mt-2 flex max-h-[calc(100vh-14rem)] w-80 flex-col gap-2 overflow-auto rounded-lg border border-line-strong bg-surface p-3 shadow-xl",
                role: "dialog",
                "aria-label": t!("cw-add"),
                "data-testid": "case-add-panel",
                div { class: "flex gap-1", role: "tablist",
                    button {
                        r#type: "button",
                        role: "tab",
                        "aria-selected": !undated_tab(),
                        class: tab(!undated_tab()),
                        onclick: move |_| undated_tab.set(false),
                        {t!("cw-objects")}
                    }
                    button {
                        r#type: "button",
                        role: "tab",
                        "aria-selected": undated_tab(),
                        class: tab(undated_tab()),
                        "data-testid": "case-undated-tab",
                        onclick: move |_| undated_tab.set(true),
                        {t!("cw-undated-count", count : undated)}
                    }
                }
                if undated_tab() {
                    Tray {}
                } else {
                    Palette {}
                }
            }
        }
    }
}
