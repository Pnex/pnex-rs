//! Typed property inputs and read-only values (D178). Inputs edit one key
//! of a shared property document; validation stays the shared
//! `pnex_core::ontology::schema::check_values` (same rules as the server).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::{PropertyDef, PropertyKind};
use serde_json::{json, Map, Value};

const INPUT: &str = crate::components::ui::INPUT;

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
            label { class: "block text-xs font-medium text-gray-500 mb-1", "{label}{unit}{required}" }
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
        PropertyKind::GeoArea => rsx! {
            GeoAreaField { name: key.clone(), doc, current }
        },
        PropertyKind::Series { unit, .. } => {
            let unit = unit.unwrap_or_default();
            rsx! {
                p { class: "text-xs text-gray-500", {t!("onto-series-bound", unit : unit)} }
            }
        }
        PropertyKind::Events { stream } => rsx! {
            p { class: "text-xs text-gray-500", {t!("onto-events-bound", stream : stream)} }
        },
        PropertyKind::FuzzyTime => rsx! {
            FuzzyField { name: key.clone(), doc, current }
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

/// The area value of a circle form, `Null` until it is complete and valid.
fn circle_of(lat: &str, lon: &str, radius: &str) -> Value {
    let n = |s: &str| s.trim().parse::<f64>().ok();
    match (n(lat), n(lon), n(radius)) {
        (Some(lat), Some(lon), Some(r)) => {
            let v = json!({"center": {"lon": lon, "lat": lat}, "radius_m": r});
            if pnex_core::geo_area::GeoArea::parse(&v).is_some() {
                v
            } else {
                Value::Null
            }
        }
        _ => Value::Null,
    }
}

/// Polygon pasted as GeoJSON (a Polygon, or a Feature wrapping one),
/// `None` when it is not a valid area.
fn polygon_of(raw: &str) -> Option<Value> {
    let v: Value = serde_json::from_str(raw.trim()).ok()?;
    let geometry = if v.get("type").and_then(Value::as_str) == Some("Feature") {
        v.get("geometry")?.clone()
    } else {
        v
    };
    let poly = json!({"type": "Polygon", "coordinates": geometry.get("coordinates")?});
    pnex_core::geo_area::GeoArea::parse(&poly).map(|_| poly)
}

/// `geo_area` entry (K29): a circle (centre and radius in metres) or a
/// polygon pasted as GeoJSON.
#[component]
fn GeoAreaField(name: String, doc: Signal<Map<String, Value>>, current: Option<Value>) -> Element {
    let c = current.clone().unwrap_or_default();
    let polygon = c.get("type").is_some();
    let num = |v: Option<&Value>| v.map(|n| n.to_string()).unwrap_or_default();
    let mut mode = use_signal(move || if polygon { "polygon" } else { "circle" }.to_string());
    let mut lat = use_signal(|| num(c.pointer("/center/lat")));
    let mut lon = use_signal(|| num(c.pointer("/center/lon")));
    let mut radius = use_signal(|| num(c.get("radius_m")));
    let pasted = if polygon {
        c.to_string()
    } else {
        String::new()
    };
    let mut raw = use_signal(move || pasted);
    let invalid = mode() == "polygon" && !raw().trim().is_empty() && polygon_of(&raw()).is_none();
    let (n1, n2, n3, n4) = (name.clone(), name.clone(), name.clone(), name.clone());
    rsx! {
        div { class: "flex flex-col gap-2",
            select {
                class: "{INPUT} sm:w-40",
                "data-testid": "geo-area-mode",
                onchange: move |e| mode.set(e.value()),
                option { value: "circle", selected: mode() == "circle", {t!("onto-area-circle")} }
                option { value: "polygon", selected: mode() == "polygon", {t!("onto-area-polygon")} }
            }
            if mode() == "circle" {
                div { class: "flex flex-wrap gap-2",
                    input {
                        r#type: "number",
                        step: "any",
                        class: INPUT,
                        placeholder: t!("onto-lat"),
                        value: lat(),
                        oninput: move |e| {
                            lat.set(e.value());
                            set(doc, &n1, circle_of(&lat(), &lon(), &radius()));
                        },
                    }
                    input {
                        r#type: "number",
                        step: "any",
                        class: INPUT,
                        placeholder: t!("onto-lon"),
                        value: lon(),
                        oninput: move |e| {
                            lon.set(e.value());
                            set(doc, &n2, circle_of(&lat(), &lon(), &radius()));
                        },
                    }
                    input {
                        r#type: "number",
                        step: "any",
                        min: "1",
                        class: INPUT,
                        placeholder: t!("onto-area-radius"),
                        value: radius(),
                        oninput: move |e| {
                            radius.set(e.value());
                            set(doc, &n3, circle_of(&lat(), &lon(), &radius()));
                        },
                    }
                }
            } else {
                textarea {
                    class: "{INPUT} font-mono text-xs",
                    rows: 4,
                    placeholder: t!("onto-area-geojson"),
                    value: raw(),
                    oninput: move |e| {
                        raw.set(e.value());
                        set(doc, &n4, polygon_of(&e.value()).unwrap_or(Value::Null));
                    },
                }
                if invalid {
                    p { class: "text-xs text-alarm", {t!("onto-area-invalid")} }
                }
            }
        }
    }
}

/// `fuzzy_time` entry: a precision, then one start (the bounds cover the
/// precision) or, for a range or an approximation, two bounds. UTC.
#[component]
fn FuzzyField(name: String, doc: Signal<Map<String, Value>>, current: Option<Value>) -> Element {
    let get = |k: &str| {
        current
            .as_ref()
            .and_then(|v| v.get(k))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let init_p = Some(get("precision")).filter(|p| !p.is_empty());
    let mut precision = use_signal(move || init_p.unwrap_or_else(|| "day".to_string()));
    let mut start = use_signal(|| get("from").get(..16).unwrap_or_default().to_string());
    let mut end = use_signal(|| get("to").get(..16).unwrap_or_default().to_string());
    let two = matches!(precision().as_str(), "range" | "approx");
    let apply = move |p: String, a: String, b: String| {
        let bounds = match p.as_str() {
            "range" | "approx" if a.len() == 16 && b.len() == 16 => {
                Some((format!("{a}:00Z"), format!("{b}:00Z")))
            }
            _ => pnex_core::ontology::schema::fuzzy_bounds(&p, &a),
        };
        let v = match bounds {
            Some((from, to)) => json!({"from": from, "to": to, "precision": p}),
            None => Value::Null,
        };
        set(doc, &name, v);
    };
    let (a1, a2, a3) = (apply.clone(), apply.clone(), apply);
    rsx! {
        div { class: "flex flex-wrap gap-2",
            select {
                class: "{INPUT} sm:w-40",
                "data-testid": "fuzzy-precision",
                onchange: move |e| {
                    precision.set(e.value());
                    a1(e.value(), start(), end());
                },
                for p in pnex_core::ontology::schema::FUZZY_PRECISIONS {
                    option {
                        key: "{p}",
                        value: "{p}",
                        selected: p == precision(),
                        {super::precision_label(p)}
                    }
                }
            }
            input {
                r#type: "datetime-local",
                class: "{INPUT} sm:w-56",
                "data-testid": "fuzzy-from",
                value: "{start}",
                oninput: move |e| {
                    start.set(e.value());
                    a2(precision(), e.value(), end());
                },
            }
            if two {
                input {
                    r#type: "datetime-local",
                    class: "{INPUT} sm:w-56",
                    "data-testid": "fuzzy-to",
                    value: "{end}",
                    oninput: move |e| {
                        end.set(e.value());
                        a3(precision(), start(), e.value());
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
            span { class: "text-gray-500", "—" }
        };
    };
    match (&def.kind, &v) {
        (PropertyKind::Url, Value::String(s)) if pnex_core::ontology::schema::valid_http_url(s) => {
            rsx! {
                a {
                    class: "text-signal-600 hover:underline break-all",
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
        (PropertyKind::GeoArea, Value::Object(o)) => {
            let text = match o.get("radius_m") {
                Some(r) => t!("onto-area-circle-value", radius : r.to_string()).to_string(),
                None => t!("onto-area-polygon").to_string(),
            };
            rsx! {
                span { "{text}" }
            }
        }
        (PropertyKind::Number { unit, .. }, Value::Number(n)) => {
            let unit = unit.clone().unwrap_or_default();
            rsx! {
                span { class: "font-mono", "{n} {unit}" }
            }
        }
        (PropertyKind::FuzzyTime, Value::Object(_)) => {
            let text = super::fuzzy_label(&v);
            rsx! {
                span { class: "font-mono", "{text}" }
            }
        }
        (PropertyKind::Ref { .. }, Value::String(id)) => {
            let title = ref_title.unwrap_or_else(|| id.clone());
            let id = id.clone();
            rsx! {
                Link {
                    class: "text-signal-600 hover:underline",
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
