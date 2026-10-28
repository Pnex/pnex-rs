//! `/ontology/types/:key` — the type editor (as-UI, D186): name, icon,
//! write role (D188), containment rules (D180) and typed properties
//! (D178). `_new` creates a type. A system type only takes extra org
//! properties (D176). Every save appends a version on top of the one that
//! was read (409 otherwise).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{ObjectTypeInput, ObjectTypeView};
use pnex_core::ontology::{ObjectTypeDef, PropertyDef, PropertyKind, TypeSet, WriteRole};

use crate::api;
use crate::app::Route;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::layout::{ListLayout, DANGER_BTN, PRIMARY_BTN};
use crate::components::ontology::{field_errors, type_label, ICON_NAMES};
use crate::state::toasts;

const INPUT: &str = crate::components::ui::INPUT;
const LABEL: &str = crate::components::ui::FIELD_LABEL;
const SECTION: &str = "bg-white rounded-lg border border-gray-200 p-4 space-y-3";
const KINDS: &[&str] = &[
    "text",
    "number",
    "bool",
    "date",
    "date_time",
    "enum",
    "geo_point",
    "geo_area",
    "url",
    "ref",
    "series",
    "events",
    "fuzzy_time",
    "recurrence",
    "money",
];

fn blank(key: &str) -> ObjectTypeDef {
    ObjectTypeDef {
        names: Default::default(),
        key: key.to_string(),
        name: String::new(),
        icon: "cube".into(),
        system: false,
        properties: Vec::new(),
        write_role: WriteRole::Member,
        may_contain: None,
        may_be_contained_in: Some(TypeSet::Only(vec!["folder".into()])),
        actions: Vec::new(),
        title_classification: Default::default(),
    }
}

fn kind_default(kind: &str) -> PropertyKind {
    match kind {
        "number" => PropertyKind::Number {
            unit: None,
            min: None,
            max: None,
        },
        "bool" => PropertyKind::Bool,
        "date" => PropertyKind::Date,
        "date_time" => PropertyKind::DateTime,
        "enum" => PropertyKind::Enum { values: Vec::new() },
        "geo_point" => PropertyKind::GeoPoint,
        "geo_area" => PropertyKind::GeoArea,
        "url" => PropertyKind::Url,
        "ref" => PropertyKind::Ref {
            to_type: String::new(),
        },
        "series" => PropertyKind::Series {
            metric: None,
            unit: None,
        },
        "events" => PropertyKind::Events {
            stream: String::new(),
        },
        "fuzzy_time" => PropertyKind::FuzzyTime,
        "recurrence" => PropertyKind::Recurrence,
        "money" => PropertyKind::Money,
        _ => PropertyKind::Text { max_len: None },
    }
}

#[component]
pub fn OntologyType(type_key: String) -> Element {
    let key = type_key;
    let navigator = use_navigator();
    let is_new = key == "_new";
    let admin = crate::state::access::can("ontology.schema:write");
    let types = use_resource(|| async { api::ontology::types().await.unwrap_or_default() });
    let all: Vec<ObjectTypeView> = types.read().clone().unwrap_or_default();
    let existing = all.iter().find(|t| t.def.key == key).cloned();
    if types.read().is_none() || (!is_new && existing.is_none()) {
        return rsx! {
            p { class: "text-gray-500 text-center py-12", {t!("common-loading")} }
        };
    }
    rsx! {
        Editor {
            key: "{key}",
            initial: existing.clone(),
            all,
            admin,
            on_back: move |_| {
                navigator
                    .push(Route::Ontology {
                        tab: "schema".into(),
                        type_key: String::new(),
                    });
            },
        }
    }
}

#[component]
fn Editor(
    initial: Option<ObjectTypeView>,
    all: Vec<ObjectTypeView>,
    admin: bool,
    on_back: Callback<()>,
) -> Element {
    let navigator = use_navigator();
    let is_new = initial.is_none();
    let start = initial.clone();
    let mut def = use_signal(move || {
        start
            .as_ref()
            .map(|t| t.def.clone())
            .unwrap_or_else(|| blank(""))
    });
    let mut version = use_signal({
        let v = initial.as_ref().map(|t| t.version).unwrap_or(0);
        move || v
    });
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<(String, String)>);
    let mut deleting = use_signal(|| false);
    let system = def.read().system;
    let title = if is_new {
        t!("onto-new-type").to_string()
    } else {
        type_label(&def.read())
    };
    let choices: Vec<(String, String)> = all
        .iter()
        .map(|t| (t.def.key.clone(), type_label(&t.def)))
        .collect();
    let key_now = def.read().key.clone();
    let versions = use_resource(move || {
        let key = key_now.clone();
        async move {
            if key.is_empty() {
                return Vec::new();
            }
            api::ontology::type_versions(&key).await.unwrap_or_default()
        }
    });

    let save = move |_| {
        let input = ObjectTypeInput {
            def: def.read().clone(),
            expected_version: (!is_new).then(&*version),
        };
        busy.set(true);
        error.set(None);
        spawn(async move {
            let res = if is_new {
                api::ontology::create_type(&input).await
            } else {
                api::ontology::update_type(&input.def.key, &input).await
            };
            busy.set(false);
            match res {
                Ok(v) => {
                    version.set(v.version);
                    toasts::success(t!("onto-type-saved").to_string());
                    if is_new {
                        navigator.replace(Route::OntologyType {
                            type_key: v.def.key,
                        });
                    }
                }
                Err(e) => match field_errors(&e).and_then(|f| f.into_iter().next()) {
                    Some((f, tok)) => {
                        error.set(Some((f, tok.as_str().unwrap_or_default().to_string())))
                    }
                    None => toasts::error(e),
                },
            }
        });
    };
    let err_text = error
        .read()
        .clone()
        .map(|(f, tok)| format!("{f}: {}", crate::api::error_i18n::localize_field(&f, &tok)));
    let props_len = def.read().properties.len();

    rsx! {
        ListLayout {
            title,
            can_write: admin,
            subtitle: Some(
                if system {
                    t!("onto-system-extension").to_string()
                } else {
                    t!("onto-type-editor-subtitle").to_string()
                },
            ),
            on_back: move |_| on_back.call(()),
            actions: rsx! {
                if admin && !is_new && !system {
                    button { class: DANGER_BTN, onclick: move |_| deleting.set(true), {t!("common-delete")} }
                }
                if admin {
                    button {
                        class: PRIMARY_BTN,
                        "data-testid": "onto-save-type",
                        disabled: busy(),
                        onclick: save,
                        {t!("common-save")}
                    }
                }
            },
            div { class: "space-y-4",
                if let Some(e) = err_text {
                    p { class: "rounded bg-red-50 px-3 py-2 text-sm text-red-700",
                        "{e}"
                    }
                }
                if !system {
                    div { class: SECTION,
                        div { class: "grid grid-cols-1 gap-3 sm:grid-cols-2",
                            div {
                                label { class: LABEL, {t!("onto-key")} }
                                input {
                                    class: INPUT,
                                    "data-testid": "onto-type-key",
                                    disabled: !is_new,
                                    value: "{def.read().key}",
                                    oninput: move |e| def.with_mut(|d| d.key = e.value().trim().to_lowercase()),
                                }
                                p { class: "mt-1 text-xs text-gray-500", {t!("onto-key-help")} }
                            }
                            div {
                                label { class: LABEL, {t!("onto-type-name")} }
                                input {
                                    class: INPUT,
                                    "data-testid": "onto-type-name",
                                    value: "{crate::components::ontology::tr(&def.read().name, &def.read().names)}",
                                    // A typed name replaces the pack translations.
                                    oninput: move |e| {
                                        def.with_mut(|d| {
                                            d.name = e.value();
                                            d.names.clear();
                                        })
                                    },
                                }
                            }
                            div {
                                label { class: LABEL, {t!("onto-icon")} }
                                select {
                                    class: INPUT,
                                    onchange: move |e| def.with_mut(|d| d.icon = e.value()),
                                    for i in ICON_NAMES {
                                        option {
                                            key: "{i}",
                                            value: "{i}",
                                            selected: def.read().icon == *i,
                                            "{i}"
                                        }
                                    }
                                }
                            }
                            div {
                                label { class: LABEL, {t!("onto-write-role")} }
                                select {
                                    class: INPUT,
                                    onchange: move |e| {
                                        let role = match e.value().as_str() {
                                            "admin" => WriteRole::Admin,
                                            "owner" => WriteRole::Owner,
                                            _ => WriteRole::Member,
                                        };
                                        def.with_mut(|d| d.write_role = role);
                                    },
                                    option {
                                        value: "member",
                                        selected: def.read().write_role == WriteRole::Member,
                                        {t!("onto-role-member")}
                                    }
                                    option {
                                        value: "admin",
                                        selected: def.read().write_role == WriteRole::Admin,
                                        {t!("onto-role-admin")}
                                    }
                                    option {
                                        value: "owner",
                                        selected: def.read().write_role == WriteRole::Owner,
                                        {t!("onto-role-owner")}
                                    }
                                }
                            }
                        }
                    }
                    div { class: SECTION,
                        h3 { class: "text-sm font-medium text-gray-900", {t!("onto-containment")} }
                        SetEditor {
                            label: t!("onto-may-contain").to_string(),
                            value: def.read().may_contain.clone(),
                            choices: choices.clone(),
                            on_change: move |v| def.with_mut(|d| d.may_contain = v),
                        }
                        SetEditor {
                            label: t!("onto-may-be-contained-in").to_string(),
                            value: def.read().may_be_contained_in.clone(),
                            choices: choices.clone(),
                            on_change: move |v| def.with_mut(|d| d.may_be_contained_in = v),
                        }
                    }
                }
                div { class: SECTION,
                    div { class: "flex items-center",
                        h3 { class: "text-sm font-medium text-gray-900", {t!("onto-properties")} }
                        button {
                            class: "ml-auto px-3 py-1 text-sm text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50",
                            "data-testid": "onto-add-property",
                            onclick: move |_| {
                                def.with_mut(|d| {
                                    d.properties
                                        .push(PropertyDef {
                                            names: Default::default(),
                                            key: String::new(),
                                            name: String::new(),
                                            kind: PropertyKind::Text {
                                                max_len: None,
                                            },
                                            required: false,
                                            indexed: false,
                                            unique: false,
                                            pack: None,
                                            colors: Default::default(),
                                            labels: Default::default(),
                                            classification: Default::default(),
                                        })
                                });
                            },
                            {t!("onto-add-property")}
                        }
                    }
                    if props_len == 0 {
                        p { class: "text-sm text-gray-500", {t!("onto-no-properties")} }
                    }
                    for i in 0..props_len {
                        PropertyRow {
                            key: "{i}",
                            index: i,
                            def,
                            choices: choices.clone(),
                        }
                    }
                }
                VersionsList { versions: versions.read().clone().unwrap_or_default() }
            }
        }
        if deleting() {
            ConfirmDialog {
                title: t!("onto-delete-type").to_string(),
                message: t!("onto-delete-type-confirm", key : def.read().key.clone()).to_string(),
                confirm_label: t!("common-delete").to_string(),
                on_confirm: move |_| {
                    let key = def.read().key.clone();
                    spawn(async move {
                        match api::ontology::delete_type(&key).await {
                            Ok(_) => on_back.call(()),
                            Err(e) => toasts::error(e),
                        }
                        deleting.set(false);
                    });
                },
                on_cancel: move |_| deleting.set(false),
            }
        }
    }
}

/// `None` (nothing), `*` (any type) or a closed list.
#[component]
fn SetEditor(
    label: String,
    value: Option<TypeSet>,
    choices: Vec<(String, String)>,
    on_change: Callback<Option<TypeSet>>,
) -> Element {
    let mode = match &value {
        None => "none",
        Some(TypeSet::Any) => "any",
        Some(TypeSet::Only(_)) => "only",
    };
    let picked: Vec<String> = match &value {
        Some(TypeSet::Only(k)) => k.clone(),
        _ => Vec::new(),
    };
    rsx! {
        div {
            label { class: LABEL, "{label}" }
            select {
                class: INPUT,
                onchange: move |e| {
                    let v = match e.value().as_str() {
                        "any" => Some(TypeSet::Any),
                        "only" => Some(TypeSet::Only(Vec::new())),
                        _ => None,
                    };
                    on_change.call(v);
                },
                option { value: "none", selected: mode == "none", {t!("onto-set-none")} }
                option { value: "any", selected: mode == "any", {t!("onto-any-type")} }
                option { value: "only", selected: mode == "only", {t!("onto-set-only")} }
            }
            if mode == "only" {
                div { class: "mt-2 flex flex-wrap gap-3",
                    for (k, l) in choices {
                        SetChoice {
                            key: "{k}",
                            value: k.clone(),
                            label: l,
                            picked: picked.clone(),
                            on_change,
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn SetChoice(
    value: String,
    label: String,
    picked: Vec<String>,
    on_change: Callback<Option<TypeSet>>,
) -> Element {
    let on = picked.contains(&value);
    rsx! {
        label { class: "flex items-center gap-1 text-sm text-gray-700",
            input {
                r#type: "checkbox",
                checked: on,
                onchange: move |e| {
                    let mut keys = picked.clone();
                    keys.retain(|k| k != &value);
                    if e.checked() {
                        keys.push(value.clone());
                    }
                    on_change.call(Some(TypeSet::Only(keys)));
                },
            }
            "{label}"
        }
    }
}

#[component]
fn PropertyRow(
    index: usize,
    def: Signal<ObjectTypeDef>,
    choices: Vec<(String, String)>,
) -> Element {
    let Some(p) = def.read().properties.get(index).cloned() else {
        return rsx! {};
    };
    let mut edit = move |f: Box<dyn FnOnce(&mut PropertyDef)>| {
        def.with_mut(|d| {
            if let Some(p) = d.properties.get_mut(index) {
                f(p);
            }
        });
    };
    let kind = p.kind.wire();
    rsx! {
        div {
            class: "rounded border border-gray-200 p-3 space-y-2",
            "data-property-row": "{index}",
            div { class: "grid grid-cols-1 gap-2 sm:grid-cols-4",
                input {
                    class: INPUT,
                    placeholder: t!("onto-key"),
                    "data-testid": "onto-prop-key",
                    value: "{p.key}",
                    oninput: move |e| {
                        let v = e.value().trim().to_lowercase();
                        edit(Box::new(move |p| p.key = v));
                    },
                }
                input {
                    class: INPUT,
                    placeholder: t!("onto-prop-name"),
                    value: "{crate::components::ontology::tr(&p.name, &p.names)}",
                    oninput: move |e| {
                        let v = e.value();
                        edit(
                            Box::new(move |p| {
                                p.name = v;
                                p.names.clear();
                            }),
                        );
                    },
                }
                select {
                    class: INPUT,
                    "data-testid": "onto-prop-kind",
                    onchange: move |e| {
                        let k = kind_default(&e.value());
                        edit(Box::new(move |p| p.kind = k));
                    },
                    for k in KINDS {
                        option { key: "{k}", value: "{k}", selected: *k == kind, {kind_label(k)} }
                    }
                }
                div { class: "flex items-center gap-3",
                    label { class: "flex items-center gap-1 text-xs text-gray-700",
                        input {
                            r#type: "checkbox",
                            checked: p.required,
                            onchange: move |e| {
                                let v = e.checked();
                                edit(Box::new(move |p| p.required = v));
                            },
                        }
                        {t!("onto-required")}
                    }
                    label { class: "flex items-center gap-1 text-xs text-gray-700",
                        input {
                            r#type: "checkbox",
                            checked: p.indexed,
                            onchange: move |e| {
                                let v = e.checked();
                                edit(Box::new(move |p| p.indexed = v));
                            },
                        }
                        {t!("onto-indexed")}
                    }
                    label {
                        class: "flex items-center gap-1 text-xs text-gray-700",
                        title: t!("onto-unique-help"),
                        input {
                            r#type: "checkbox",
                            checked: p.unique,
                            onchange: move |e| {
                                let v = e.checked();
                                edit(Box::new(move |p| p.unique = v));
                            },
                        }
                        {t!("onto-unique")}
                    }
                    button {
                        class: "ml-auto text-xs text-red-700 hover:underline",
                        onclick: move |_| {
                            def.write().properties.remove(index);
                        },
                        {t!("common-delete")}
                    }
                }
            }
            KindConfig { index, def, choices }
            if let Some(prop) = def.read().properties.get(index).cloned() {
                EnumDisplay {
                    prop,
                    on_change: move |p| {
                        def.with_mut(|d| {
                            if let Some(slot) = d.properties.get_mut(index) {
                                *slot = p;
                            }
                        })
                    },
                }
            }
        }
    }
}

/// How each value of an enum is shown (D370): its text in the UI language
/// and its colour. The value itself stays the stored key. Shared by the
/// type editor and the link types' attributes.
#[component]
pub(super) fn EnumDisplay(prop: PropertyDef, on_change: Callback<PropertyDef>) -> Element {
    let p = prop;
    let PropertyKind::Enum { values } = &p.kind else {
        return rsx! {};
    };
    // Both UI languages, the current one first.
    let langs = if crate::i18n::current_tag().starts_with("fr") {
        ["fr", "en"]
    } else {
        ["en", "fr"]
    };
    let edit = {
        let p = p.clone();
        move |f: Box<dyn FnOnce(&mut PropertyDef)>| {
            let mut p = p.clone();
            f(&mut p);
            on_change.call(p);
        }
    };
    rsx! {
        div { class: "space-y-1", "data-testid": "onto-enum-display",
            p { class: "text-xs text-gray-600", {t!("onto-enum-display")} }
            for v in values.clone() {
                div { key: "{v}", class: "grid grid-cols-4 items-center gap-2",
                    span { class: "font-mono text-xs text-gray-700", "{v}" }
                    for lang in langs {
                        input {
                            key: "{lang}",
                            class: INPUT,
                            title: "{lang}",
                            placeholder: format!("{lang} · {}", crate::components::ontology::enum_label(&v)),
                            value: p.labels.get(&v).and_then(|n| n.get(lang)).cloned().unwrap_or_default(),
                            oninput: {
                                let (v, edit) = (v.clone(), edit.clone());
                                move |e: FormEvent| {
                                    let (v, text) = (v.clone(), e.value());
                                    edit(Box::new(move |p| set_label(p, &v, lang, &text)));
                                }
                            },
                        }
                    }
                    select {
                        class: INPUT,
                        onchange: {
                            let (v, edit) = (v.clone(), edit.clone());
                            move |e: FormEvent| {
                                let (v, c) = (v.clone(), e.value());
                                edit(Box::new(move |p| set_color(p, &v, &c)));
                            }
                        },
                        option { value: "", {t!("onto-color-none")} }
                        for c in pnex_core::ontology::schema::COLORS {
                            option {
                                key: "{c}",
                                value: "{c}",
                                selected: p.colors.get(&v).map(String::as_str) == Some(*c),
                                {color_label(c)}
                            }
                        }
                    }
                }
            }
        }
    }
}

fn color_label(color: &str) -> String {
    dioxus_i18n::prelude::i18n()
        .try_translate(&format!("onto-color-{color}"))
        .unwrap_or_else(|_| color.to_string())
}

/// Sets the text of `value` in `lang`; an empty text removes it.
fn set_label(p: &mut PropertyDef, value: &str, lang: &str, text: &str) {
    let names = p.labels.entry(value.to_string()).or_default();
    if text.trim().is_empty() {
        names.remove(lang);
    } else {
        names.insert(lang.to_string(), text.to_string());
    }
    if names.is_empty() {
        p.labels.remove(value);
    }
}

/// Drops the texts and colours of values the enum no longer has (the
/// server refuses them).
fn keep_known_values(p: &mut PropertyDef) {
    match &p.kind {
        PropertyKind::Enum { values } => {
            p.labels.retain(|v, _| values.contains(v));
            p.colors.retain(|v, _| values.contains(v));
        }
        _ => {
            p.labels.clear();
            p.colors.clear();
        }
    }
}

/// Sets the colour of `value`; an empty one removes it.
fn set_color(p: &mut PropertyDef, value: &str, color: &str) {
    if color.is_empty() {
        p.colors.remove(value);
    } else {
        p.colors.insert(value.to_string(), color.to_string());
    }
}

fn kind_label(kind: &str) -> String {
    dioxus_i18n::prelude::i18n()
        .try_translate(&format!("onto-kind-{}", kind.replace('_', "-")))
        .unwrap_or_else(|_| kind.to_string())
}

/// Kind-specific settings: unit, bounds, enum values, ref target, stream.
#[component]
fn KindConfig(index: usize, def: Signal<ObjectTypeDef>, choices: Vec<(String, String)>) -> Element {
    let Some(p) = def.read().properties.get(index).cloned() else {
        return rsx! {};
    };
    let mut set_kind = move |k: PropertyKind| {
        def.with_mut(|d| {
            if let Some(p) = d.properties.get_mut(index) {
                p.kind = k;
                keep_known_values(p);
            }
        });
    };
    match p.kind {
        PropertyKind::Number { unit, min, max } => {
            let unit_s = unit.clone().unwrap_or_default();
            let min_s = min.map(|v| v.to_string()).unwrap_or_default();
            let max_s = max.map(|v| v.to_string()).unwrap_or_default();
            let (u1, u2) = (unit.clone(), unit.clone());
            rsx! {
                div { class: "grid grid-cols-3 gap-2",
                    input {
                        class: INPUT,
                        placeholder: t!("onto-unit"),
                        value: unit_s,
                        oninput: move |e| set_kind(PropertyKind::Number {
                            unit: Some(e.value()).filter(|u| !u.is_empty()),
                            min,
                            max,
                        }),
                    }
                    input {
                        class: INPUT,
                        r#type: "number",
                        placeholder: t!("onto-min"),
                        value: min_s,
                        oninput: move |e| set_kind(PropertyKind::Number {
                            unit: u1.clone(),
                            min: e.value().parse().ok(),
                            max,
                        }),
                    }
                    input {
                        class: INPUT,
                        r#type: "number",
                        placeholder: t!("onto-max"),
                        value: max_s,
                        oninput: move |e| set_kind(PropertyKind::Number {
                            unit: u2.clone(),
                            min,
                            max: e.value().parse().ok(),
                        }),
                    }
                }
            }
        }
        PropertyKind::Series { metric, unit } => {
            let unit_s = unit.unwrap_or_default();
            rsx! {
                input {
                    class: INPUT,
                    placeholder: t!("onto-unit"),
                    value: unit_s,
                    oninput: move |e| set_kind(PropertyKind::Series {
                        metric: metric.clone(),
                        unit: Some(e.value()).filter(|u| !u.is_empty()),
                    }),
                }
            }
        }
        PropertyKind::Text { max_len } => {
            let v = max_len.map(|n| n.to_string()).unwrap_or_default();
            rsx! {
                input {
                    class: INPUT,
                    r#type: "number",
                    placeholder: t!("onto-max-len"),
                    value: v,
                    oninput: move |e| set_kind(PropertyKind::Text {
                        max_len: e.value().parse().ok(),
                    }),
                }
            }
        }
        PropertyKind::Enum { values } => {
            let v = values.join(", ");
            rsx! {
                input {
                    class: INPUT,
                    placeholder: t!("onto-enum-values"),
                    "data-testid": "onto-enum-values",
                    value: v,
                    oninput: move |e| {
                        let values = e
                            .value()
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                        set_kind(PropertyKind::Enum { values });
                    },
                }
            }
        }
        PropertyKind::Ref { to_type } => rsx! {
            select {
                class: INPUT,
                onchange: move |e| set_kind(PropertyKind::Ref {
                    to_type: e.value(),
                }),
                option { value: "", "—" }
                for (k, l) in choices {
                    option { key: "{k}", value: "{k}", selected: k == to_type, "{l}" }
                }
            }
        },
        PropertyKind::Events { stream } => rsx! {
            input {
                class: INPUT,
                placeholder: t!("onto-stream"),
                value: stream,
                oninput: move |e| set_kind(PropertyKind::Events {
                    stream: e.value(),
                }),
            }
        },
        _ => rsx! {},
    }
}

#[component]
fn VersionsList(versions: Vec<pnex_core::ontology::api::ObjectTypeVersionView>) -> Element {
    if versions.is_empty() {
        return rsx! {};
    }
    rsx! {
        div { class: SECTION,
            h3 { class: "text-sm font-medium text-gray-900", {t!("onto-versions")} }
            ul { class: "text-sm text-gray-700 space-y-1",
                for v in versions {
                    li { key: "{v.version}",
                        {
                            format!(
                                "v{} · {} · {}",
                                v.version,
                                crate::components::ontology::short_time(&v.created_at),
                                t!("onto-n-properties", n : v.def.properties.len()),
                            )
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod display_tests {
    use super::*;

    #[test]
    fn enum_texts_and_colours_follow_the_values() {
        let mut p = PropertyDef {
            key: "status".into(),
            name: String::new(),
            names: Default::default(),
            kind: PropertyKind::Enum {
                values: vec!["customer".into(), "target".into()],
            },
            required: false,
            indexed: false,
            unique: false,
            pack: None,
            colors: Default::default(),
            labels: Default::default(),
            classification: Default::default(),
        };
        set_label(&mut p, "customer", "fr", "Client actif");
        set_color(&mut p, "target", "amber");
        assert_eq!(p.label_of("customer", "fr-FR"), Some("Client actif"));
        p.kind = PropertyKind::Enum {
            values: vec!["customer".into()],
        };
        keep_known_values(&mut p);
        assert!(p.colors.is_empty());
        set_label(&mut p, "customer", "fr", " ");
        assert!(p.labels.is_empty());
    }
}
