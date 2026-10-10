//! Typed property inputs and read-only values (D178). Inputs edit one key
//! of a shared property document; validation stays the shared
//! `pnex_core::ontology::schema::check_values` (same rules as the server).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::{PropertyDef, PropertyKind};
use serde_json::{json, Map, Value};

const INPUT: &str = "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white";

/// Label of a property: its name, else its key.
pub fn prop_label(d: &PropertyDef) -> String {
    if d.name.trim().is_empty() {
        d.key.clone()
    } else {
        d.name.clone()
    }
}

fn set(mut doc: Signal<Map<String, Value>>, key: &str, v: Value) {
    let key = key.to_string();
    doc.with_mut(|m| {
        if v.is_null() {
            m.remove(&key);
        } else {
            m.insert(key, v);
        }
    });
}

fn text_of(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// One editable property. `refs` are the `(id, title)` choices of a
/// `ref` property (loaded by the caller for its target type).
#[component]
pub fn PropertyInput(
    def: PropertyDef,
    doc: Signal<Map<String, Value>>,
    #[props(default)] refs: Vec<(String, String)>,
    #[props(default)] error: Option<String>,
) -> Element {
    let key = def.key.clone();
    let current = doc.read().get(&key).cloned();
    let label = prop_label(&def);
    let unit = match &def.kind {
        PropertyKind::Number { unit: Some(u), .. } => format!(" ({u})"),
        _ => String::new(),
    };
    let required = if def.required { " *" } else { "" };
    rsx! {
        div { "data-prop": "{key}",
            label { class: "block text-xs font-medium text-gray-500 uppercase mb-1",
                "{label}{unit}{required}"
            }
            PropertyField {
                def: def.clone(),
                doc,
                current,
                refs,
            }
            if let Some(e) = error {
                p { class: "mt-1 text-xs text-red-600", "{e}" }
            }
        }
    }
}

#[component]
fn PropertyField(
    def: PropertyDef,
    doc: Signal<Map<String, Value>>,
    current: Option<Value>,
    refs: Vec<(String, String)>,
) -> Element {
    let key = def.key.clone();
    match def.kind.clone() {
        PropertyKind::Bool => {
            let checked = current.as_ref().and_then(Value::as_bool).unwrap_or(false);
            rsx! {
                input {
                    r#type: "checkbox",
                    class: "h-4 w-4",
                    checked,
                    onchange: move |e| set(doc, &key, Value::Bool(e.checked())),
                }
            }
        }
        PropertyKind::Number { .. } => {
            let value = text_of(current.as_ref());
            rsx! {
                input {
                    r#type: "number",
                    step: "any",
                    class: INPUT,
                    value,
                    oninput: move |e| {
                        let v = e
                            .value()
                            .trim()
                            .parse::<f64>()
                            .ok()
                            .map(|n| json!(n))
                            .unwrap_or(Value::Null);
                        set(doc, &key, v);
                    },
                }
            }
        }
        PropertyKind::Enum { values } => {
            let value = text_of(current.as_ref());
            rsx! {
                select {
                    class: INPUT,
                    value: "{value}",
                    onchange: move |e| set(doc, &key, Value::String(e.value())),
                    option { value: "", "—" }
                    for v in values {
                        option { key: "{v}", value: "{v}", selected: v == value, "{v}" }
                    }
                }
            }
        }
        PropertyKind::Ref { .. } => {
            let value = text_of(current.as_ref());
            rsx! {
                select {
                    class: INPUT,
                    onchange: move |e| set(doc, &key, Value::String(e.value())),
                    option { value: "", "—" }
                    for (id, title) in refs {
                        option {
                            key: "{id}",
                            value: "{id}",
                            selected: id == value,
                            "{title}"
                        }
                    }
                }
            }
        }
        PropertyKind::GeoPoint => {
            let lat = current
                .as_ref()
                .and_then(|v| v.get("lat"))
                .map(|v| v.to_string())
                .unwrap_or_default();
            let lon = current
                .as_ref()
                .and_then(|v| v.get("lon"))
                .map(|v| v.to_string())
                .unwrap_or_default();
            let k2 = key.clone();
            rsx! {
                div { class: "flex gap-2",
                    input {
                        r#type: "number",
                        step: "any",
                        class: INPUT,
                        placeholder: t!("onto-lat"),
                        value: lat,
                        oninput: move |e| geo_set(doc, &key, "lat", &e.value()),
                    }
                    input {
                        r#type: "number",
                        step: "any",
                        class: INPUT,
                        placeholder: t!("onto-lon"),
                        value: lon,
                        oninput: move |e| geo_set(doc, &k2, "lon", &e.value()),
                    }
                }
            }
        }
        PropertyKind::Series { unit, .. } => {
            let unit = unit.unwrap_or_default();
            rsx! {
                p { class: "text-xs text-gray-500", {t!("onto-series-bound", unit : unit)} }
            }
        }
        PropertyKind::Events { stream } => rsx! {
            p { class: "text-xs text-gray-500", {t!("onto-events-bound", stream : stream)} }
        },
        kind => {
            let input_type = match kind {
                PropertyKind::Date => "date",
                PropertyKind::DateTime => "datetime-local",
                PropertyKind::Url => "url",
                _ => "text",
            };
            let is_dt = input_type == "datetime-local";
            let mut value = text_of(current.as_ref());
            if is_dt {
                value = value.get(..16).unwrap_or_default().to_string();
            }
            rsx! {
                input {
                    r#type: input_type,
                    class: INPUT,
                    value,
                    oninput: move |e| {
                        let raw = e.value();
                        // datetime-local has no zone: the value is read as UTC.
                        let v = if is_dt && raw.len() == 16 { format!("{raw}:00Z") } else { raw };
                        set(doc, &key, Value::String(v));
                    },
                }
            }
        }
    }
}

fn geo_set(mut doc: Signal<Map<String, Value>>, key: &str, axis: &str, raw: &str) {
    let n = raw.trim().parse::<f64>().ok();
    let key = key.to_string();
    let axis = axis.to_string();
    doc.with_mut(|m| {
        let mut point = m
            .get(&key)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        match n {
            Some(n) => {
                point.insert(axis, json!(n));
            }
            None => {
                point.remove(&axis);
            }
        }
        if point.is_empty() {
            m.remove(&key);
        } else {
            m.insert(key, Value::Object(point));
        }
    });
}

/// Read-only rendering of a value. URLs are links only when http(s) (R11).
#[component]
pub fn PropertyValue(
    def: PropertyDef,
    value: Option<Value>,
    #[props(default)] ref_title: Option<String>,
) -> Element {
    let Some(v) = value else {
        return rsx! {
            span { class: "text-gray-400", "—" }
        };
    };
    match (&def.kind, &v) {
        (PropertyKind::Url, Value::String(s)) if pnex_core::ontology::schema::valid_http_url(s) => {
            rsx! {
                a {
                    class: "text-blue-600 hover:underline break-all",
                    href: "{s}",
                    target: "_blank",
                    rel: "noopener noreferrer",
                    "{s}"
                }
            }
        }
        (PropertyKind::Bool, Value::Bool(b)) => {
            let text = if *b { t!("onto-yes") } else { t!("onto-no") };
            rsx! {
                span { "{text}" }
            }
        }
        (PropertyKind::GeoPoint, Value::Object(o)) => {
            let lat = o.get("lat").map(|v| v.to_string()).unwrap_or_default();
            let lon = o.get("lon").map(|v| v.to_string()).unwrap_or_default();
            rsx! {
                span { class: "font-mono", "{lat}, {lon}" }
            }
        }
        (PropertyKind::Number { unit, .. }, Value::Number(n)) => {
            let unit = unit.clone().unwrap_or_default();
            rsx! {
                span { class: "font-mono", "{n} {unit}" }
            }
        }
        (PropertyKind::Ref { .. }, Value::String(id)) => {
            let title = ref_title.unwrap_or_else(|| id.clone());
            let id = id.clone();
            rsx! {
                Link {
                    class: "text-blue-600 hover:underline",
                    to: crate::app::Route::OntologyObject {
                        id,
                    },
                    "{title}"
                }
            }
        }
        (_, Value::String(s)) => rsx! {
            span { class: "break-words", "{s}" }
        },
        _ => {
            let s = v.to_string();
            rsx! {
                span { "{s}" }
            }
        }
    }
}
