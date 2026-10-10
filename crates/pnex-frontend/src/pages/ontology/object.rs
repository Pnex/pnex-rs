//! `/ontology/object/:id` — the standard object page (D186): properties,
//! typed temporal links (as of a date, full history), local graph and
//! cascading impact, recomposed series (D181) and provenance (D184).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{
    LinkClose, LinkInput, LinkTypeView, LinkView, ObjectTypeView, ObjectView, OntologyQuery,
};
use pnex_core::ontology::{PropertyKind, MEASURES_METRIC, MEASURES_PROPERTY, REL_MEASURES};
use serde_json::{Map, Value};

use crate::api;
use crate::app::Route;
use crate::components::charts::{ChartSeries, TimeSeriesChart};
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::{ListLayout, DANGER_BTN};
use crate::components::ontology::graph_view::GraphView;
use crate::components::ontology::property_input::{prop_label, PropertyInput, PropertyValue};
use crate::components::ontology::{
    field_errors, icon_of, key_label, link_label, short_time, type_label, TypeIcon,
};
use crate::state::{org, toasts};

const BTN: &str = "inline-flex items-center gap-1 px-3 py-1 text-sm text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50";
const SECTION: &str = "bg-white rounded-lg border border-gray-200 p-4 space-y-3";
const INPUT: &str = "px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white";

#[component]
pub fn OntologyObject(id: String) -> Element {
    let mut reload = use_signal(|| 0u32);
    let navigator = use_navigator();
    let oid = id.clone();
    let object = use_resource(move || {
        let id = oid.clone();
        async move {
            let _ = reload();
            api::ontology::object(&id).await
        }
    });
    let types = use_resource(|| async { api::ontology::types().await.unwrap_or_default() });
    let link_types =
        use_resource(|| async { api::ontology::link_types().await.unwrap_or_default() });
    let mut tab = use_signal(|| "properties".to_string());
    let mut editing = use_signal(|| false);
    let mut archiving = use_signal(|| false);
    let can_write = org::current_can_write();

    let o = match &*object.read() {
        Some(Ok(o)) => o.clone(),
        Some(Err(e)) => {
            let msg = crate::api::error_i18n::localize(e);
            return rsx! {
                p { class: "text-red-600 text-center py-12", "{msg}" }
            };
        }
        None => {
            return rsx! {
                p { class: "text-gray-500 text-center py-12", {t!("common-loading")} }
            }
        }
    };
    let all_types = types.read().clone().unwrap_or_default();
    let all_links = link_types.read().clone().unwrap_or_default();
    let Some(tv) = all_types.iter().find(|t| t.def.key == o.type_key).cloned() else {
        return rsx! {
            p { class: "text-gray-500 text-center py-12", {t!("common-loading")} }
        };
    };
    let archived = o.valid_to.is_some();
    let tabs = [
        ("properties", t!("onto-tab-properties")),
        ("links", t!("onto-tab-links")),
        ("graph", t!("onto-tab-graph")),
        ("time", t!("onto-tab-time")),
        ("provenance", t!("onto-tab-provenance")),
    ];
    let icon = icon_of(&tv.def);
    let kind = type_label(&tv.def);
    let type_key = o.type_key.clone();

    rsx! {
        ListLayout {
            title: o.title.clone(),
            subtitle: Some(kind.clone()),
            can_write,
            on_back: move |_| {
                navigator
                    .push(Route::Ontology {
                        tab: String::new(),
                        type_key: type_key.clone(),
                    });
            },
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            actions: rsx! {
                if can_write && !archived && (!tv.def.system || !tv.def.properties.is_empty()) {
                    button {
                        class: BTN,
                        "data-testid": "onto-edit",
                        onclick: move |_| editing.set(true),
                        {t!("onto-edit")}
                    }
                }
                if can_write && !archived && !tv.def.system {
                    button { class: DANGER_BTN, onclick: move |_| archiving.set(true), {t!("onto-archive")} }
                }
            },
            div { class: "space-y-4",
                div { class: "flex flex-wrap items-center gap-2 text-sm",
                    TypeIcon { icon, class: "h-5 w-5 text-blue-600" }
                    span { class: "rounded bg-blue-50 px-2 py-0.5 text-blue-700", "{kind}" }
                    if tv.def.system {
                        span { class: "rounded bg-gray-100 px-2 py-0.5 text-gray-600",
                            {t!("onto-system")}
                        }
                    }
                    if archived {
                        span { class: "rounded bg-amber-50 px-2 py-0.5 text-amber-700",
                            {t!("onto-archived")}
                        }
                    }
                    span { class: "text-xs text-gray-500", {t!("onto-version", version : o.version)} }
                }
                div { class: "flex flex-wrap gap-2", role: "tablist",
                    for (key, label) in tabs {
                        button {
                            key: "{key}",
                            r#type: "button",
                            role: "tab",
                            "data-tab": "{key}",
                            class: if tab() == key { super::TAB_ACTIVE } else { super::TAB_IDLE },
                            onclick: move |_| tab.set(key.to_string()),
                            "{label}"
                        }
                    }
                }
                if tab() == "links" {
                    LinksPanel {
                        object: o.clone(),
                        types: all_types.clone(),
                        link_types: all_links.clone(),
                        can_write: can_write && !archived,
                    }
                } else if tab() == "graph" {
                    GraphPanel { id: o.id.clone(), link_types: all_links.clone() }
                } else if tab() == "time" {
                    TimePanel {
                        object: o.clone(),
                        type_view: tv.clone(),
                        can_write: can_write && !archived,
                    }
                } else if tab() == "provenance" {
                    ProvenancePanel { id: o.id.clone() }
                } else {
                    PropertiesPanel { object: o.clone(), type_view: tv.clone() }
                }
            }
            if editing() {
                super::ObjectForm {
                    type_view: tv.clone(),
                    initial: Some(o.clone()),
                    on_close: move |_| editing.set(false),
                    on_saved: move |_| {
                        editing.set(false);
                        reload.with_mut(|r| *r += 1);
                    },
                }
            }
            if archiving() {
                ConfirmDialog {
                    title: t!("onto-archive").to_string(),
                    message: t!("onto-archive-confirm", title : o.title.clone()).to_string(),
                    confirm_label: t!("onto-archive").to_string(),
                    on_confirm: move |_| {
                        let id = id.clone();
                        let done = t!("onto-archived-toast").to_string();
                        spawn(async move {
                            match api::ontology::archive_object(&id).await {
                                Ok(_) => {
                                    toasts::success(done);
                                    reload.with_mut(|r| *r += 1);
                                }
                                Err(e) => toasts::error(e),
                            }
                            archiving.set(false);
                        });
                    },
                    on_cancel: move |_| archiving.set(false),
                }
            }
        }
    }
}

#[component]
fn PropertiesPanel(object: ObjectView, type_view: ObjectTypeView) -> Element {
    let props: Vec<_> = type_view
        .def
        .properties
        .iter()
        .filter(|p| !p.kind.is_temporal())
        .cloned()
        .collect();
    let source = object.source_ref.clone().unwrap_or_default();
    rsx! {
        div { class: SECTION,
            if props.is_empty() {
                p { class: "text-sm text-gray-500", {t!("onto-no-properties")} }
            }
            dl { class: "grid grid-cols-1 gap-3 sm:grid-cols-2",
                for p in props {
                    div { key: "{p.key}", "data-prop": "{p.key}",
                        dt { class: "text-xs font-medium uppercase text-gray-500",
                            {prop_label(&p)}
                        }
                        dd { class: "text-sm text-gray-900",
                            PropertyValue {
                                value: object.properties.get(&p.key).cloned(),
                                def: p.clone(),
                            }
                        }
                    }
                }
            }
            div { class: "border-t border-gray-100 pt-3 text-xs text-gray-500 space-y-1",
                p { {t!("onto-recorded-since", at : short_time(& object.valid_from))} }
                if !source.is_empty() {
                    p { {t!("onto-last-source", source : source)} }
                }
            }
        }
    }
}

/// Links valid as of a date (default now) or the whole history; closing a
/// link keeps it as history (D179).
#[component]
fn LinksPanel(
    object: ObjectView,
    types: Vec<ObjectTypeView>,
    link_types: Vec<LinkTypeView>,
    can_write: bool,
) -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut as_of = use_signal(String::new);
    let mut history = use_signal(|| false);
    let mut adding = use_signal(|| false);
    let oid = object.id.clone();
    let links = use_resource(move || {
        let id = oid.clone();
        let at = as_of();
        let h = history();
        async move {
            let _ = reload();
            let at = (!at.is_empty()).then(|| format!("{at}:00Z"));
            api::ontology::object_links(&id, at.as_deref(), h).await
        }
    });
    let rows: Vec<LinkView> = match &*links.read() {
        Some(Ok(l)) => l.clone(),
        _ => Vec::new(),
    };
    let me = object.id.clone();
    rsx! {
        div { class: SECTION,
            div { class: "flex flex-wrap items-end gap-3",
                div {
                    label { class: "block text-xs font-medium text-gray-500 uppercase mb-1",
                        {t!("onto-as-of")}
                    }
                    input {
                        r#type: "datetime-local",
                        class: INPUT,
                        "data-testid": "onto-as-of",
                        value: "{as_of}",
                        disabled: history(),
                        oninput: move |e| as_of.set(e.value()),
                    }
                }
                label { class: "flex items-center gap-2 text-sm text-gray-700",
                    input {
                        r#type: "checkbox",
                        checked: history(),
                        onchange: move |e| history.set(e.checked()),
                    }
                    {t!("onto-history")}
                }
                if can_write {
                    button {
                        class: "ml-auto {BTN}",
                        "data-testid": "onto-add-link",
                        onclick: move |_| adding.set(true),
                        {t!("onto-add-link")}
                    }
                }
            }
            if rows.is_empty() {
                p { class: "text-sm text-gray-500", {t!("onto-links-empty")} }
            }
            ul { class: "divide-y divide-gray-100",
                for l in rows {
                    LinkRow {
                        key: "{l.id}",
                        link: l.clone(),
                        me: me.clone(),
                        link_types: link_types.clone(),
                        can_write,
                        on_closed: move |_| reload.with_mut(|r| *r += 1),
                    }
                }
            }
        }
        if adding() {
            LinkForm {
                object: object.clone(),
                types,
                link_types: link_types.clone(),
                preset: None,
                on_close: move |_| adding.set(false),
                on_saved: move |_| {
                    adding.set(false);
                    reload.with_mut(|r| *r += 1);
                },
            }
        }
    }
}

#[component]
fn LinkRow(
    link: LinkView,
    me: String,
    link_types: Vec<LinkTypeView>,
    can_write: bool,
    on_closed: Callback<()>,
) -> Element {
    let outgoing = link.source.id == me;
    let other = if outgoing {
        link.target.clone()
    } else {
        link.source.clone()
    };
    let def = link_types
        .iter()
        .find(|t| t.def.key == link.link_type)
        .map(|t| t.def.clone());
    let label = link_label(def.as_ref(), &link.link_type, !outgoing);
    let span = match &link.valid_to {
        Some(to) => format!("{} → {}", short_time(&link.valid_from), short_time(to)),
        None => t!("onto-since", at : short_time(& link.valid_from)).to_string(),
    };
    let attrs: Vec<String> = link
        .attributes
        .iter()
        .map(|(k, v)| {
            format!(
                "{k}: {}",
                v.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| v.to_string())
            )
        })
        .collect();
    let open = link.valid_to.is_none();
    let id = link.id;
    let kind = key_label(&other.type_key);
    rsx! {
        li {
            class: "flex flex-wrap items-center gap-2 py-2",
            "data-link": "{link.link_type}",
            span { class: "text-sm text-gray-500", "{label}" }
            Link {
                class: "text-sm font-medium text-blue-600 hover:underline",
                to: Route::OntologyObject {
                    id: other.id.clone(),
                },
                "{other.title}"
            }
            span { class: "text-xs text-gray-400", "{kind}" }
            if !attrs.is_empty() {
                span { class: "text-xs text-gray-500 font-mono", {attrs.join(" · ")} }
            }
            span { class: "ml-auto text-xs text-gray-500", "{span}" }
            if can_write && open {
                button {
                    class: "text-xs text-red-700 hover:underline",
                    "data-testid": "onto-close-link",
                    onclick: move |_| {
                        let done = t!("onto-link-closed").to_string();
                        spawn(async move {
                            match api::ontology::close_link(id, &LinkClose::default()).await {
                                Ok(_) => {
                                    toasts::success(done);
                                    on_closed.call(());
                                }
                                Err(e) => toasts::error(e),
                            }
                        });
                    },
                    {t!("onto-close-link")}
                }
            }
        }
    }
}

/// A preset of the link form: binding a sensor to a series property.
#[derive(Debug, Clone, PartialEq)]
struct Preset {
    property: String,
    metric: String,
}

/// Opens a link from or to this object. The other end is chosen among the
/// live objects of the types the link type admits.
#[component]
fn LinkForm(
    object: ObjectView,
    types: Vec<ObjectTypeView>,
    link_types: Vec<LinkTypeView>,
    preset: Option<Preset>,
    /// Open link closed at the new link's start before it opens (sensor
    /// replacement, D181).
    #[props(default)]
    close_first: Option<i64>,
    on_close: Callback<()>,
    on_saved: Callback<()>,
) -> Element {
    // Link types usable from or to this object's type.
    let usable: Vec<(LinkTypeView, bool)> = link_types
        .iter()
        .flat_map(|l| {
            let from = l.def.from_types.contains(&object.type_key);
            let to = l.def.to_types.contains(&object.type_key);
            [(from, true), (to, false)]
                .into_iter()
                .filter(|(ok, _)| *ok)
                .map(move |(_, out)| (l.clone(), out))
        })
        .collect();
    let first = preset
        .as_ref()
        .map(|_| format!("{REL_MEASURES}:in"))
        .or_else(|| {
            usable
                .first()
                .map(|(l, out)| format!("{}:{}", l.def.key, if *out { "out" } else { "in" }))
        })
        .unwrap_or_default();
    let mut choice = use_signal(move || first.clone());
    let mut other = use_signal(String::new);
    let mut from = use_signal(String::new);
    let pre = preset.clone();
    let attrs = use_signal(move || {
        let mut m = Map::new();
        if let Some(p) = &pre {
            m.insert(MEASURES_PROPERTY.into(), Value::String(p.property.clone()));
            m.insert(MEASURES_METRIC.into(), Value::String(p.metric.clone()));
        }
        m
    });
    let mut busy = use_signal(|| false);
    let mut errors = use_signal(Map::<String, Value>::new);
    let (key, outgoing) = match choice().split_once(':') {
        Some((k, d)) => (k.to_string(), d == "out"),
        None => (String::new(), true),
    };
    let lt = link_types.iter().find(|l| l.def.key == key).cloned();
    // The other end's types are read from `choice()` inside the resource:
    // a value captured at render time is not a tracked dependency.
    let lts = link_types.clone();
    let candidates = use_resource(move || {
        let (k, out) = match choice().split_once(':') {
            Some((k, d)) => (k.to_string(), d == "out"),
            None => (String::new(), true),
        };
        let set = lts.iter().find(|l| l.def.key == k).map(|l| {
            if out {
                l.def.to_types.clone()
            } else {
                l.def.from_types.clone()
            }
        });
        async move {
            let Some(set) = set else { return Vec::new() };
            let q = OntologyQuery {
                type_key: match &set {
                    pnex_core::ontology::TypeSet::Only(keys) if keys.len() == 1 => {
                        Some(keys[0].clone())
                    }
                    _ => None,
                },
                limit: Some(500),
                ..Default::default()
            };
            api::ontology::query(&q)
                .await
                .map(|r| {
                    r.rows
                        .into_iter()
                        .map(|r| r.object)
                        .filter(|o| set.contains(&o.type_key))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        }
    });
    let me = object.id.clone();
    let candidates: Vec<ObjectView> = candidates
        .read()
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter(|o| o.id != me)
        .collect();
    let attr_defs = lt
        .as_ref()
        .map(|l| l.def.attributes.clone())
        .unwrap_or_default();
    let options: Vec<(String, String)> = usable
        .iter()
        .map(|(l, out)| {
            let v = format!("{}:{}", l.def.key, if *out { "out" } else { "in" });
            (v, link_label(Some(&l.def), &l.def.key, !*out))
        })
        .collect();
    let _ = &types;
    let me = object.id.clone();

    rsx! {
        FormDialog {
            title: t!("onto-add-link").to_string(),
            submit_label: t!("onto-open-link").to_string(),
            busy: busy(),
            valid: !other().is_empty() && !key.is_empty(),
            on_close: move |_| on_close.call(()),
            on_submit: move |_| {
                let (src, tgt) = if outgoing {
                    (me.clone(), other())
                } else {
                    (other(), me.clone())
                };
                let input = LinkInput {
                    link_type: key.clone(),
                    source_id: src,
                    target_id: tgt,
                    attributes: attrs.read().clone(),
                    valid_from: (!from().is_empty()).then(|| format!("{}:00Z", from())),
                };
                busy.set(true);
                spawn(async move {
                    if let Some(id) = close_first {
                        let close = LinkClose {
                            valid_to: input.valid_from.clone(),
                        };
                        if let Err(e) = api::ontology::close_link(id, &close).await {
                            busy.set(false);
                            toasts::error(e);
                            return;
                        }
                    }
                    let res = api::ontology::create_link(&input).await;
                    busy.set(false);
                    match res {
                        Ok(_) => on_saved.call(()),
                        Err(e) => {
                            match field_errors(&e) {
                                Some(f) => errors.set(f),
                                None => toasts::error(e),
                            }
                        }
                    }
                });
            },
            div { class: "space-y-3",
                div {
                    label { class: "block text-xs font-medium text-gray-500 uppercase mb-1",
                        {t!("onto-link-type")}
                    }
                    select {
                        class: "w-full {INPUT}",
                        "data-testid": "onto-link-type",
                        onchange: move |e| {
                            choice.set(e.value());
                            other.set(String::new());
                        },
                        for (v, label) in options {
                            option {
                                key: "{v}",
                                value: "{v}",
                                selected: v == choice(),
                                "{label}"
                            }
                        }
                    }
                }
                div {
                    label { class: "block text-xs font-medium text-gray-500 uppercase mb-1",
                        {t!("onto-link-other")}
                    }
                    select {
                        class: "w-full {INPUT}",
                        "data-testid": "onto-link-other",
                        onchange: move |e| other.set(e.value()),
                        option { value: "", "—" }
                        for c in candidates {
                            option {
                                key: "{c.id}",
                                value: "{c.id}",
                                selected: c.id == other(),
                                {format!("{} ({})", c.title, key_label(&c.type_key))}
                            }
                        }
                    }
                }
                for a in attr_defs {
                    PropertyInput {
                        key: "{a.key}",
                        error: errors
                            .read()
                            .get(&format!("attributes.{}", a.key))
                            .and_then(|v| v.as_str())
                            .map(|t| crate::api::error_i18n::localize_field(&a.key, t)),
                        def: a.clone(),
                        doc: attrs,
                    }
                }
                div {
                    label { class: "block text-xs font-medium text-gray-500 uppercase mb-1",
                        {t!("onto-valid-from")}
                    }
                    input {
                        r#type: "datetime-local",
                        class: "w-full {INPUT}",
                        value: "{from}",
                        oninput: move |e| from.set(e.value()),
                    }
                    p { class: "mt-1 text-xs text-gray-500", {t!("onto-valid-from-help")} }
                }
            }
        }
    }
}

#[component]
fn GraphPanel(id: String, link_types: Vec<LinkTypeView>) -> Element {
    let mut depth = use_signal(|| 1u32);
    let gid = id.clone();
    let graph = use_resource(move || {
        let id = gid.clone();
        let d = depth();
        async move { api::ontology::graph(&id, d).await }
    });
    let iid = id.clone();
    let impact = use_resource(move || {
        let id = iid.clone();
        async move { api::ontology::impact(&id, false).await.unwrap_or_default() }
    });
    let impacted = impact.read().clone().unwrap_or_default();
    let body = match &*graph.read() {
        Some(Ok(n)) if n.links.is_empty() => rsx! {
            p { class: "text-sm text-gray-500", {t!("onto-links-empty")} }
        },
        Some(Ok(n)) => rsx! {
            GraphView {
                neighborhood: n.clone(),
                center: id.clone(),
                link_types: link_types.clone(),
            }
        },
        Some(Err(e)) => {
            let msg = crate::api::error_i18n::localize(e);
            rsx! {
                p { class: "text-sm text-red-600", "{msg}" }
            }
        }
        None => rsx! {},
    };
    rsx! {
        div { class: SECTION,
            div { class: "flex items-center gap-3",
                label { class: "text-sm text-gray-700", {t!("onto-depth")} }
                select {
                    class: INPUT,
                    onchange: move |e| depth.set(e.value().parse().unwrap_or(1)),
                    option { value: "1", selected: depth() == 1, "1" }
                    option { value: "2", selected: depth() == 2, "2" }
                }
            }
            {body}
            div {
                h3 { class: "text-sm font-medium text-gray-900", {t!("onto-impact")} }
                p { class: "text-xs text-gray-500", {t!("onto-impact-help")} }
                if impacted.is_empty() {
                    p { class: "text-sm text-gray-500", "—" }
                }
                ul { class: "mt-2 flex flex-wrap gap-2",
                    for r in impacted {
                        li { key: "{r.id}",
                            Link {
                                class: "rounded bg-gray-100 px-2 py-0.5 text-sm text-gray-800 hover:bg-gray-200",
                                to: Route::OntologyObject {
                                    id: r.id.clone(),
                                },
                                "{r.title}"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Series properties of the object, recomposed across their successive
/// sensors (D181), with a shortcut to bind or replace the sensor.
#[component]
fn TimePanel(object: ObjectView, type_view: ObjectTypeView, can_write: bool) -> Element {
    let series: Vec<_> = type_view
        .def
        .properties
        .iter()
        .filter(|p| matches!(p.kind, PropertyKind::Series { .. }))
        .cloned()
        .collect();
    let mut window = use_signal(|| "24h".to_string());
    if series.is_empty() {
        return rsx! {
            div { class: SECTION,
                p { class: "text-sm text-gray-500", {t!("onto-no-series")} }
            }
        };
    }
    rsx! {
        div { class: "space-y-3",
            div { class: "flex items-center gap-3",
                label { class: "text-sm text-gray-700", {t!("onto-window")} }
                select { class: INPUT, onchange: move |e| window.set(e.value()),
                    for (k, _) in pnex_core::VIZ_WINDOW_PRESETS {
                        option {
                            key: "{k}",
                            value: "{k}",
                            selected: *k == window(),
                            "{k}"
                        }
                    }
                }
            }
            for p in series {
                SeriesCard {
                    key: "{p.key}",
                    object: object.clone(),
                    property: p.key.clone(),
                    label: prop_label(&p),
                    window: window(),
                    can_write,
                }
            }
        }
    }
}

#[component]
fn SeriesCard(
    object: ObjectView,
    property: String,
    label: String,
    window: String,
    can_write: bool,
) -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut binding = use_signal(|| false);
    let (oid, prop, win) = (object.id.clone(), property.clone(), window.clone());
    let data = use_resource(use_reactive!(|(oid, prop, win)| async move {
        let _ = reload();
        api::ontology::series(&oid, &prop, &win).await
    }));
    let link_types =
        use_resource(|| async { api::ontology::link_types().await.unwrap_or_default() });
    let view = match &*data.read() {
        Some(Ok(v)) => Some(v.clone()),
        _ => None,
    };
    let open = view
        .as_ref()
        .and_then(|v| v.segments.iter().find(|s| s.to.is_none()).cloned());
    let metric = open
        .as_ref()
        .map(|s| s.metric.clone())
        .unwrap_or_else(|| property.clone());
    let unit = view
        .as_ref()
        .and_then(|v| v.unit.clone())
        .unwrap_or_default();
    let points: Vec<pnex_core::TelemetryPoint> = view
        .as_ref()
        .map(|v| {
            v.points
                .iter()
                .filter_map(|p| {
                    Some(pnex_core::TelemetryPoint {
                        ts: chrono::DateTime::parse_from_rfc3339(&p.t).ok()?.timestamp() as f64,
                        value: p.v,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let has_points = !points.is_empty();
    let segments = view.map(|v| v.segments).unwrap_or_default();
    rsx! {
        div { class: SECTION, "data-series": "{property}",
            div { class: "flex items-center gap-2",
                h3 { class: "text-sm font-medium text-gray-900", "{label}" }
                if !unit.is_empty() {
                    span { class: "text-xs text-gray-500", "({unit})" }
                }
                if can_write {
                    button {
                        class: "ml-auto {BTN}",
                        "data-testid": "onto-bind-sensor",
                        onclick: move |_| binding.set(true),
                        if open.is_some() {
                            {t!("onto-replace-sensor")}
                        } else {
                            {t!("onto-bind-sensor")}
                        }
                    }
                }
            }
            if has_points {
                TimeSeriesChart {
                    series: vec![
                        ChartSeries {
                            color_index: 0,
                            label: label.clone(),
                            points: Some(points),
                        },
                    ],
                }
            } else {
                p { class: "text-sm text-gray-500", {t!("onto-series-empty")} }
            }
            ul { class: "space-y-1",
                for s in segments {
                    li { class: "text-xs text-gray-600",
                        {
                            t!(
                                "onto-segment", device : s.device.title.clone(), metric : s.metric.clone(),
                                from : short_time(& s.from), to : s.to.as_deref().map(short_time)
                                .unwrap_or_else(|| "…".into())
                            )
                        }
                    }
                }
            }
        }
        if binding() {
            BindSensor {
                object: object.clone(),
                property: property.clone(),
                metric,
                link_types: link_types.read().clone().unwrap_or_default(),
                on_close: move |_| binding.set(false),
                on_saved: move |_| {
                    binding.set(false);
                    reload.with_mut(|r| *r += 1);
                },
            }
        }
    }
}

/// Binds a device to a series property; when a sensor is already bound,
/// its link is closed at the new link's start (sensor replacement, D181).
#[component]
fn BindSensor(
    object: ObjectView,
    property: String,
    metric: String,
    link_types: Vec<LinkTypeView>,
    on_close: Callback<()>,
    on_saved: Callback<()>,
) -> Element {
    let oid = object.id.clone();
    let prop = property.clone();
    let current = use_resource(move || {
        let (id, prop) = (oid.clone(), prop.clone());
        async move {
            api::ontology::object_links(&id, None, false)
                .await
                .unwrap_or_default()
                .into_iter()
                .find(|l| {
                    l.link_type == REL_MEASURES
                        && l.attributes.get(MEASURES_PROPERTY).and_then(|v| v.as_str())
                            == Some(prop.as_str())
                })
                .map(|l| l.id)
        }
    });
    // Wait for the lookup: the form must know what it replaces.
    let Some(close_first) = current.read().clone() else {
        return rsx! {};
    };
    rsx! {
        LinkForm {
            object,
            types: Vec::new(),
            link_types,
            preset: Some(Preset { property, metric }),
            close_first,
            on_close,
            on_saved,
        }
    }
}

#[component]
fn ProvenancePanel(id: String) -> Element {
    let changes = use_resource(move || {
        let id = id.clone();
        async move { api::ontology::changes(&id).await }
    });
    let read = changes.read();
    let Some(Ok(c)) = &*read else {
        return rsx! {};
    };
    if !c.available {
        return rsx! {
            div { class: SECTION,
                p { class: "text-sm text-gray-500", {t!("onto-provenance-unavailable")} }
            }
        };
    }
    let rows = c.changes.clone();
    rsx! {
        div { class: SECTION,
            if rows.is_empty() {
                p { class: "text-sm text-gray-500", {t!("onto-provenance-empty")} }
            }
            ul { class: "divide-y divide-gray-100",
                for (i, ch) in rows.into_iter().enumerate() {
                    li { key: "{i}", class: "py-2 text-sm",
                        div { class: "flex flex-wrap gap-2",
                            span { class: "font-mono text-xs text-gray-500", {short_time(&ch.at)} }
                            span { class: "font-medium text-gray-900", {action_label(&ch.action)} }
                            span { class: "text-xs text-gray-500", "{ch.source_ref}" }
                        }
                    }
                }
            }
        }
    }
}

fn action_label(action: &str) -> String {
    dioxus_i18n::prelude::i18n()
        .try_translate(&format!("onto-action-{}", action.replace('.', "-")))
        .unwrap_or_else(|_| action.to_string())
}
