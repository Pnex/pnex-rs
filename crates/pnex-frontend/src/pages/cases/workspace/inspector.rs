//! Inspector (cases-workspace.md K32), a drawer over the main view, open
//! on a selection or on demand. Without selection, the case at the
//! cursor (what has happened, where the tracked members are, the signals
//! around, each retainable as an event); an event (title, when, pieces with
//! their role, note); an object (its events in the case, follow, page).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{CaseAttachInput, CaseDetachInput, CaseEvent, CaseWorkspace};
use pnex_core::ontology::{EVENT_NOTE, EVENT_OCCURRED, INVOLVES_ROLES};
use serde_json::Value;

use super::{model, Sel, Target, Ws};
use crate::app::Route;
use crate::components::ontology::add_to_case::role_label;
use crate::components::ui::{BTN_DANGER, BTN_GHOST, BTN_SECONDARY_SM, FIELD_LABEL, INPUT};

#[component]
pub(super) fn Inspector() -> Element {
    let ws = use_context::<Ws>();
    let Some(w) = (ws.data)() else {
        return rsx! {};
    };
    let sel = (ws.sel)();
    let local = (ws.local_time)();
    let heading = match &sel {
        Sel::None => t!("cw-case-at", time : model::clock((ws.cursor)(), local)).to_string(),
        Sel::Event(_) => t!("cw-event").to_string(),
        Sel::Object(id) => object_title(&w, id).unwrap_or_default(),
    };
    let event = match &sel {
        Sel::Event(id) => w.events.iter().find(|e| &e.event.id == id).cloned(),
        _ => None,
    };
    let object = match &sel {
        Sel::Object(id) => Some(id.clone()),
        _ => None,
    };
    rsx! {
        aside {
            class: "absolute inset-y-0 right-0 z-30 flex w-full flex-col overflow-auto border-l border-line-strong bg-surface shadow-xl sm:w-96",
            "aria-label": t!("cw-inspector"),
            "data-testid": "case-inspector",
            div { class: "sticky top-0 z-10 flex items-center gap-2 border-b border-line bg-surface px-3 py-2",
                h2 { class: "truncate text-sm font-semibold text-ink", "{heading}" }
                button {
                    class: "{BTN_GHOST} ml-auto",
                    r#type: "button",
                    "data-testid": "case-inspector-close",
                    onclick: move |_| {
                        let (mut s, mut at) = (ws.sel, ws.at_cursor);
                        s.set(Sel::None);
                        at.set(false);
                    },
                    {t!("common-close")}
                }
            }
            div { class: "flex flex-col gap-3 p-3 text-sm",
                if sel == Sel::None {
                    AtCursor { workspace: w.clone() }
                } else if let Some(e) = event {
                    EventPanel {
                        key: "{e.event.id}{e.event.version}",
                        event: e,
                        workspace: w.clone(),
                    }
                } else if let Some(id) = object {
                    ObjectPanel { id, workspace: w.clone() }
                } else {
                    p { class: "text-slate", {t!("cw-gone")} }
                }
            }
        }
    }
}

fn object_title(w: &CaseWorkspace, id: &str) -> Option<String> {
    w.events
        .iter()
        .flat_map(|e| &e.attachments)
        .map(|a| &a.object)
        .chain(&w.undated)
        .find(|o| o.id == id)
        .map(|o| o.title.clone())
}

/// The case at the cursor.
#[component]
fn AtCursor(workspace: CaseWorkspace) -> Element {
    let ws = use_context::<Ws>();
    let cursor = (ws.cursor)();
    let local = (ws.local_time)();
    if workspace.events.is_empty() && workspace.undated.is_empty() {
        return rsx! {
            p { class: "text-ink", {t!("cw-empty-title")} }
            ol { class: "list-decimal space-y-1 pl-5 text-slate",
                li { {t!("cw-empty-step-1")} }
                li { {t!("cw-empty-step-2")} }
                li { {t!("cw-empty-step-3")} }
            }
            if ws.can_write {
                button {
                    class: BTN_SECONDARY_SM,
                    r#type: "button",
                    onclick: move |_| ws.create_event(t!("cw-new-event").to_string(), cursor, cursor, Vec::new()),
                    {t!("cw-new-event-at", time : model::clock(cursor, local))}
                }
            }
        };
    }
    let dated: Vec<(&CaseEvent, (i64, i64))> = workspace
        .events
        .iter()
        .filter_map(|e| Some((e, model::span(&e.event)?)))
        .collect();
    let past = dated.iter().filter(|(_, s)| s.0 <= cursor).count();
    let ongoing: Vec<&CaseEvent> = dated
        .iter()
        .filter(|(_, s)| s.0 <= cursor && s.1 >= cursor && s.1 > s.0)
        .map(|(e, _)| *e)
        .collect();
    let tracks = (ws.tracks)().map(|t| t.tracks).unwrap_or_default();
    let mut signals = (ws.signals)().map(|s| s.items).unwrap_or_default();
    signals.sort_by_key(|s| model::ms(&s.at).map_or(i64::MAX, |t| (t - cursor).abs()));
    signals.truncate(8);
    let mut pending: Vec<_> = (ws.proposals)()
        .into_iter()
        .filter(|p| p.status == "proposed")
        .collect();
    pending.sort_by_key(|p| model::ms(&p.t_start).map_or(i64::MAX, |t| (t - cursor).abs()));
    pending.truncate(8);
    let members: Vec<String> = workspace
        .events
        .iter()
        .flat_map(|e| e.attachments.iter().map(|a| a.object.id.clone()))
        .chain(workspace.undated.iter().map(|o| o.id.clone()))
        .collect();
    rsx! {
        p { class: "text-slate",
            {t!("cw-past-events", past : past, total : workspace.events.len())}
        }
        if !ongoing.is_empty() {
            div { class: "flex flex-wrap gap-1",
                span { class: "text-slate", {t!("cw-ongoing")} }
                for e in ongoing {
                    button {
                        key: "{e.event.id}",
                        class: "text-signal hover:underline",
                        r#type: "button",
                        onclick: {
                            let id = e.event.id.clone();
                            move |_| ws.select_event(id.clone())
                        },
                        "{e.event.title}"
                    }
                }
            }
        }
        if !tracks.is_empty() {
            h3 { class: "text-xs font-medium text-slate", {t!("cw-where")} }
            for (i, track) in tracks.iter().enumerate() {
                button {
                    key: "{track.object.id}",
                    r#type: "button",
                    class: "flex items-center gap-2 border-b border-line py-1 text-left text-xs hover:bg-gray-50",
                    onclick: {
                        let id = track.object.id.clone();
                        move |_| {
                            let mut sel = ws.sel;
                            sel.set(Sel::Object(id.clone()));
                        }
                    },
                    span {
                        class: "h-2.5 w-2.5 rounded-full",
                        style: "background: {super::timeline::actor_color(i)}",
                    }
                    span { class: "text-ink", "{track.object.title}" }
                    if let Some((lon, lat)) = model::position(track, cursor) {
                        span { class: "ml-auto font-mono text-slate", "{lat:.5}, {lon:.5}" }
                    } else {
                        span { class: "ml-auto text-alarm", {t!("cw-no-position")} }
                    }
                }
            }
        }
        h3 { class: "text-xs font-medium text-slate", {t!("cw-detections")} }
        if pending.is_empty() {
            p { class: "text-xs text-slate", {t!("cw-detections-none")} }
        }
        for p in pending {
            div {
                key: "{p.id}",
                class: "flex items-start gap-2 border-b border-line py-1.5",
                "data-testid": "case-proposal-row",
                div { class: "min-w-0 flex-1",
                    div { class: "text-ink", {super::proposal_title(&p)} }
                    div { class: "font-mono text-xs text-slate",
                        {model::ms(&p.t_start).map(|t| model::stamp(t, local)).unwrap_or_default()}
                    }
                }
                if ws.can_write {
                    button {
                        class: BTN_SECONDARY_SM,
                        r#type: "button",
                        onclick: {
                            let (pid, title) = (p.id.clone(), super::proposal_title(&p));
                            move |_| ws.retain(pid.clone(), title.clone())
                        },
                        {t!("cw-retain")}
                    }
                    button {
                        class: BTN_SECONDARY_SM,
                        r#type: "button",
                        onclick: {
                            let pid = p.id.clone();
                            move |_| ws.reject(pid.clone())
                        },
                        {t!("cw-reject")}
                    }
                }
            }
        }
        h3 { class: "text-xs font-medium text-slate", {t!("cw-signals-near")} }
        if signals.is_empty() {
            p { class: "text-xs text-slate", {t!("cw-signals-none")} }
        }
        for (i, s) in signals.into_iter().enumerate() {
            div {
                key: "{i}",
                class: "flex items-start gap-2 border-b border-line py-1.5",
                div { class: "min-w-0 flex-1",
                    div { class: "text-ink", {model::signal_title(&s)} }
                    div { class: "font-mono text-xs text-slate",
                        {model::ms(&s.at).map(|t| model::stamp(t, local)).unwrap_or_default()}
                    }
                }
                if ws.can_write {
                    if let Some(at) = model::ms(&s.at) {
                        button {
                            class: BTN_SECONDARY_SM,
                            r#type: "button",
                            onclick: {
                                let title = model::signal_title(&s);
                                let lane = s.lane.clone();
                                let attach: Vec<CaseAttachInput> = if members.contains(&lane) {
                                    vec![
                                        CaseAttachInput {
                                            object_id: lane,
                                            ..Default::default()
                                        },
                                    ]
                                } else {
                                    Vec::new()
                                };
                                move |_| ws.create_event(title.clone(), at, at, attach.clone())
                            },
                            {t!("cw-retain")}
                        }
                    }
                }
            }
        }
    }
}

/// Attaches `object` to `event`, with `role` (a new role changes it).
fn attach_to(ws: Ws, event: &str, object: &str, role: Option<String>) {
    ws.attach(CaseAttachInput {
        event_id: Some(event.to_string()),
        object_id: object.to_string(),
        role,
        detail: None,
    });
}

fn detach_from(ws: Ws, event: &str, object: &str) {
    ws.detach(CaseDetachInput {
        event_id: Some(event.to_string()),
        object_id: object.to_string(),
    });
}

/// `YYYY-MM-DDTHH:MM` of Unix ms in UTC, for a datetime-local input.
fn input_value(t: i64) -> String {
    model::rfc3339(t).get(..16).unwrap_or_default().to_string()
}

/// datetime-local value (UTC) → Unix ms.
fn from_input(v: &str) -> Option<i64> {
    model::ms(&format!("{v}:00Z"))
}

#[component]
fn EventPanel(event: CaseEvent, workspace: CaseWorkspace) -> Element {
    let ws = use_context::<Ws>();
    let ev = event.event.clone();
    let span = model::span(&ev);
    let note = ev
        .properties
        .get(EVENT_NOTE)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut title = use_signal(|| ev.title.clone());
    let mut confirm = use_signal(|| false);
    let targeted = (ws.target)() == Some(Target::Event(ev.id.clone()));
    let attached: Vec<String> = event
        .attachments
        .iter()
        .map(|a| a.object.id.clone())
        .collect();
    let candidates: Vec<_> = workspace
        .events
        .iter()
        .flat_map(|e| e.attachments.iter().map(|a| a.object.clone()))
        .chain(workspace.undated.iter().cloned())
        .filter(|o| !attached.contains(&o.id))
        .fold(Vec::new(), |mut acc: Vec<_>, o| {
            if !acc
                .iter()
                .any(|x: &pnex_core::ontology::api::ObjectRef| x.id == o.id)
            {
                acc.push(o);
            }
            acc
        });
    // Save one property (or the title) on the event's version.
    let save = {
        let ev = ev.clone();
        move |key: Option<&'static str>, value: Value, new_title: Option<String>| {
            let mut props = ev.properties.clone();
            if let Some(k) = key {
                props.insert(k.into(), value);
            }
            ws.save_object(
                ev.id.clone(),
                new_title.unwrap_or_else(|| ev.title.clone()),
                props,
                ev.version,
            );
        }
    };
    let (save_title, save_from, save_to, save_note, save_kind) =
        (save.clone(), save.clone(), save.clone(), save.clone(), save);
    let event_id = ev.id.clone();
    let (from0, to0) = span.unwrap_or((0, 0));
    let on_from = move |e: FormEvent| {
        if let Some(f) = from_input(&e.value()) {
            save_from(Some(EVENT_OCCURRED), model::occurred(f, to0.max(f)), None);
        }
    };
    let on_to = move |e: FormEvent| {
        if let Some(t2) = from_input(&e.value()) {
            save_to(
                Some(EVENT_OCCURRED),
                model::occurred(from0, t2.max(from0)),
                None,
            );
        }
    };
    let zone_id = ev.id.clone();
    let on_zone_move = move |_| {
        if (ws.drag)().is_some() {
            let mut target = ws.target;
            target.set(Some(Target::Event(zone_id.clone())));
        }
    };
    rsx! {
        div {
            label { class: FIELD_LABEL, r#for: "cw-ev-title", {t!("cw-event-title")} }
            input {
                id: "cw-ev-title",
                class: INPUT,
                "data-testid": "case-event-title",
                disabled: !ws.can_write,
                value: "{title}",
                oninput: move |e| title.set(e.value()),
                onchange: move |e| save_title(None, Value::Null, Some(e.value())),
            }
        }
        if let Some((from, to)) = span {
            div { class: "flex flex-col gap-2",
                div {
                    label { class: FIELD_LABEL, r#for: "cw-ev-from", {t!("cw-event-from")} }
                    input {
                        id: "cw-ev-from",
                        r#type: "datetime-local",
                        class: INPUT,
                        disabled: !ws.can_write,
                        value: "{input_value(from)}",
                        onchange: on_from,
                    }
                }
                if to > from {
                    div {
                        label { class: FIELD_LABEL, r#for: "cw-ev-to", {t!("cw-event-to")} }
                        input {
                            id: "cw-ev-to",
                            r#type: "datetime-local",
                            class: INPUT,
                            disabled: !ws.can_write,
                            value: "{input_value(to)}",
                            onchange: on_to,
                        }
                    }
                }
            }
            p { class: "text-xs text-slate",
                if to > from {
                    {t!("cw-period")}
                } else {
                    {t!("cw-instant")}
                }
                " · UTC · "
                if ws.can_write {
                    button {
                        class: "text-signal hover:underline",
                        r#type: "button",
                        onclick: move |_| {
                            let window = (ws.window)();
                            let next_to = if to > from { from } else { from + (window.1 - window.0) / 20 };
                            save_kind(Some(EVENT_OCCURRED), model::occurred(from, next_to), None);
                        },
                        if to > from {
                            {t!("cw-to-instant")}
                        } else {
                            {t!("cw-to-period")}
                        }
                    }
                }
            }
        }
        // Where it happened, right under when (only if a piece is located).
        super::map::EventMap { event: event.clone(), workspace: workspace.clone() }
        h3 { class: "text-xs font-medium text-slate",
            {t!("cw-pieces", count : event.attachments.len())}
        }
        if event.attachments.is_empty() {
            p { class: "text-xs text-slate", {t!("cw-pieces-none")} }
        }
        for a in event.attachments.iter().cloned() {
            div {
                key: "{a.link_id}",
                class: "flex items-center gap-2 border-b border-line py-1",
                "data-testid": "case-attachment",
                button {
                    class: "min-w-0 flex-1 truncate text-left text-ink hover:text-signal",
                    r#type: "button",
                    onclick: {
                        let id = a.object.id.clone();
                        move |_| {
                            let mut sel = ws.sel;
                            sel.set(Sel::Object(id.clone()));
                        }
                    },
                    "{a.object.title}"
                    if let Some(d) = &a.detail {
                        span { class: "text-xs text-slate", " · {d}" }
                    }
                }
                select {
                    class: "rounded-md border border-line-strong bg-surface px-1.5 py-0.5 text-xs",
                    "aria-label": t!("case-role"),
                    "data-testid": "case-role",
                    disabled: !ws.can_write,
                    onchange: {
                        let (event_id, object_id) = (event_id.clone(), a.object.id.clone());
                        move |e: FormEvent| {
                            attach_to(ws, &event_id, &object_id, Some(e.value()));
                        }
                    },
                    for r in INVOLVES_ROLES {
                        option { key: "{r}", value: "{r}", selected: a.role == r, {role_label(r)} }
                    }
                }
                if ws.can_write {
                    button {
                        class: "px-1 text-slate hover:text-alarm",
                        r#type: "button",
                        "aria-label": t!("cw-detach", name : a.object.title.clone()),
                        onclick: {
                            let (event_id, object_id) = (event_id.clone(), a.object.id.clone());
                            move |_| {
                                detach_from(ws, &event_id, &object_id);
                            }
                        },
                        "✕"
                    }
                }
            }
        }
        if ws.can_write {
            div {
                class: "rounded-md border border-dashed p-2.5 text-center text-xs",
                class: if targeted { "border-signal bg-signal-soft text-signal" } else { "border-line-strong text-slate" },
                onpointermove: on_zone_move,
                {t!("cw-drop-here")}
            }
            if !candidates.is_empty() {
                select {
                    class: INPUT,
                    "aria-label": t!("cw-attach-keyboard"),
                    "data-testid": "case-attach-select",
                    onchange: {
                        let event_id = event_id.clone();
                        move |e: FormEvent| {
                            if !e.value().is_empty() {
                                attach_to(ws, &event_id, &e.value(), None);
                            }
                        }
                    },
                    option { value: "", {t!("cw-attach-keyboard")} }
                    for o in candidates {
                        option { key: "{o.id}", value: "{o.id}", "{o.title}" }
                    }
                }
            }
        }
        div {
            label { class: FIELD_LABEL, r#for: "cw-ev-note", {t!("cw-note")} }
            textarea {
                id: "cw-ev-note",
                class: "{INPUT} min-h-20",
                disabled: !ws.can_write,
                placeholder: t!("cw-note-placeholder"),
                initial_value: "{note}",
                onchange: move |e| save_note(Some(EVENT_NOTE), Value::String(e.value()), None),
            }
        }
        if ws.can_write {
            if confirm() {
                div { class: "rounded-md border border-alarm p-2 text-xs",
                    p { {t!("cw-remove-event-confirm")} }
                    div { class: "mt-2 flex gap-2",
                        button {
                            class: BTN_DANGER,
                            r#type: "button",
                            onclick: {
                                let id = event_id.clone();
                                move |_| {
                                    let mut sel = ws.sel;
                                    sel.set(Sel::None);
                                    ws.detach(CaseDetachInput {
                                        event_id: None,
                                        object_id: id.clone(),
                                    });
                                }
                            },
                            {t!("cw-remove")}
                        }
                        button {
                            class: BTN_GHOST,
                            r#type: "button",
                            onclick: move |_| confirm.set(false),
                            {t!("common-cancel")}
                        }
                    }
                }
            } else {
                button {
                    class: "{BTN_GHOST} self-end text-alarm",
                    r#type: "button",
                    onclick: move |_| confirm.set(true),
                    {t!("cw-remove-event")}
                }
            }
        }
    }
}

#[component]
fn ObjectPanel(id: String, workspace: CaseWorkspace) -> Element {
    let ws = use_context::<Ws>();
    let involved: Vec<(&CaseEvent, String)> = workspace
        .events
        .iter()
        .filter_map(|e| {
            Some((
                e,
                e.attachments
                    .iter()
                    .find(|a| a.object.id == id)?
                    .role
                    .clone(),
            ))
        })
        .collect();
    let undated = workspace.undated.iter().any(|o| o.id == id);
    let tracked = (ws.tracks)().is_some_and(|t| t.tracks.iter().any(|t| t.object.id == id));
    let following = (ws.follow)().as_ref() == Some(&id);
    let local = (ws.local_time)();
    let in_case = undated || !involved.is_empty();
    rsx! {
        if undated {
            p { class: "text-xs text-warn-text", {t!("cw-undated-status")} }
        }
        if tracked {
            button {
                class: BTN_SECONDARY_SM,
                r#type: "button",
                onclick: {
                    let id = id.clone();
                    move |_| {
                        let mut f = ws.follow;
                        f.set(if following { None } else { Some(id.clone()) });
                    }
                },
                if following {
                    {t!("cw-show-all")}
                } else {
                    {t!("cw-follow")}
                }
            }
        }
        h3 { class: "text-xs font-medium text-slate", {t!("cw-in-events", count : involved.len())} }
        if involved.is_empty() {
            p { class: "text-xs text-slate", {t!("cw-in-events-none")} }
        }
        for (e, role) in involved {
            button {
                key: "{e.event.id}",
                r#type: "button",
                class: "flex items-center gap-2 border-b border-line py-1 text-left hover:bg-gray-50",
                onclick: {
                    let eid = e.event.id.clone();
                    move |_| ws.select_event(eid.clone())
                },
                span { class: "font-mono text-xs text-slate",
                    {model::span(&e.event).map(|s| model::stamp(s.0, local)).unwrap_or_default()}
                }
                span { class: "min-w-0 flex-1 truncate text-ink", "{e.event.title}" }
                span { class: "rounded border border-line-strong px-1.5 text-xs text-slate",
                    {role_label(&role)}
                }
            }
        }
        if in_case {
            super::imports::TrackImports { object_id: id.clone() }
        }
        Link {
            to: Route::OntologyObject {
                id: id.clone(),
            },
            class: "text-signal hover:underline",
            {t!("cw-open-object")}
        }
        if ws.can_write && in_case {
            button {
                class: "{BTN_GHOST} self-end text-alarm",
                r#type: "button",
                onclick: {
                    let id = id.clone();
                    move |_| {
                        let mut sel = ws.sel;
                        sel.set(Sel::None);
                        ws.detach(CaseDetachInput {
                            event_id: None,
                            object_id: id.clone(),
                        });
                    }
                },
                {t!("cw-remove-object")}
            }
        }
    }
}
