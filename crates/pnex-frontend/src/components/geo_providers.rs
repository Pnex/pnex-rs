//! Geo providers of the org (geo-layers.md §8, L16–L28): basemaps,
//! geocoders and routers the org plugs in, list + inline form on the org
//! page (school of `llm_providers`). The optional API key is a
//! [`SecretField`] (typed or picked, never shown), sent as a query
//! parameter like every map provider does.

use std::collections::BTreeMap;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::geo::{
    GeoCapability, GeoProvider, GeoProviderInput, GeoProviderKind, DEFAULT_KEY_PARAM,
};
use uuid::Uuid;

use crate::api;
use crate::components::secret_field::{SecretDraft, SecretField};
use crate::state::toasts;

const BTN: &str = "px-2 py-1 text-xs border border-gray-300 rounded-lg hover:bg-gray-50";
const BTN_PRIMARY: &str =
    "px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors text-sm";
const INPUT: &str = "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white";
const KINDS: [GeoProviderKind; 5] = GeoProviderKind::ALL;
/// Key of the dark style variant of a basemap (server: `STYLE_URL_DARK`).
const STYLE_URL_DARK: &str = "style_url_dark";

#[derive(Clone, Debug, PartialEq)]
enum Editing {
    New,
    Edit(GeoProvider),
}

fn kind_label(k: GeoProviderKind) -> String {
    match k {
        GeoProviderKind::Basemap => t!("geo-kind-basemap").to_string(),
        GeoProviderKind::Nominatim => "Nominatim".to_string(),
        GeoProviderKind::Photon => "Photon".to_string(),
        GeoProviderKind::Valhalla => "Valhalla".to_string(),
        GeoProviderKind::GraphHopper => "GraphHopper".to_string(),
    }
}

fn cap_label(c: GeoCapability) -> String {
    match c {
        GeoCapability::Basemap => t!("geo-cap-basemap"),
        GeoCapability::Geocode => t!("geo-cap-geocode"),
        GeoCapability::Reverse => t!("geo-cap-reverse"),
        GeoCapability::Autocomplete => t!("geo-cap-autocomplete"),
        GeoCapability::Route => t!("geo-cap-route"),
        GeoCapability::Isochrone => t!("geo-cap-isochrone"),
        GeoCapability::Matrix => t!("geo-cap-matrix"),
    }
    .to_string()
}

/// `key=value` lines ↔ map (non-secret query parameters).
fn lines_of(m: &BTreeMap<String, String>) -> String {
    m.iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn map_of(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .filter(|(k, _)| !k.is_empty())
        .collect()
}

#[component]
pub fn GeoProviders(can_manage: bool) -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut editing = use_signal(|| None::<Editing>);
    let mut confirm_delete = use_signal(|| None::<Uuid>);
    let list = use_resource(move || async move {
        let _ = reload();
        api::geo::providers().await
    });

    let rows: Vec<GeoProvider> = match &*list.value().read() {
        Some(Ok(rows)) => rows.clone(),
        _ => Vec::new(),
    };
    let failed = matches!(&*list.value().read(), Some(Err(_)));

    rsx! {
        div { class: "space-y-3",
            div { class: "flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between",
                div {
                    h3 { class: "text-sm font-semibold text-gray-900", {t!("geo-title")} }
                    p { class: "text-xs text-gray-500 mt-1", {t!("geo-help")} }
                }
                if can_manage && editing().is_none() {
                    button {
                        r#type: "button",
                        class: "inline-flex items-center self-start whitespace-nowrap px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors text-sm",
                        onclick: move |_| editing.set(Some(Editing::New)),
                        crate::components::icons::Plus { class: "h-4 w-4 mr-1" }
                        {t!("geo-add")}
                    }
                }
            }
            if failed {
                if let Some(Err(err)) = &*list.value().read() {
                    p { class: "text-sm text-red-700", {api::error_i18n::localize(err)} }
                }
            } else if rows.is_empty() {
                p { class: "text-sm text-gray-500", {t!("geo-empty")} }
            } else {
                div { class: "overflow-x-auto",
                    table { class: "min-w-full text-sm",
                        thead {
                            tr { class: "text-left text-xs uppercase text-gray-500 border-b",
                                th { class: "py-2 pr-4", {t!("geo-col-name")} }
                                th { class: "py-2 pr-4", {t!("geo-col-kind")} }
                                th { class: "py-2 pr-4 hidden md:table-cell", {t!("geo-col-caps")} }
                                th { class: "py-2 sticky right-0 bg-white md:static" }
                            }
                        }
                        tbody {
                            for p in rows {
                                ProviderRow {
                                    key: "{p.id}",
                                    provider: p.clone(),
                                    can_manage,
                                    confirming: confirm_delete() == Some(p.id),
                                    on_edit: move |p: GeoProvider| editing.set(Some(Editing::Edit(p))),
                                    on_ask_delete: move |id: Uuid| confirm_delete.set(Some(id)),
                                    on_deleted: move |_| {
                                        confirm_delete.set(None);
                                        reload += 1;
                                    },
                                }
                            }
                        }
                    }
                }
            }
            match editing() {
                Some(Editing::New) => rsx! {
                    ProviderForm {
                        existing: None,
                        on_done: move |_| {
                            editing.set(None);
                            reload += 1;
                        },
                        on_cancel: move |_| editing.set(None),
                    }
                },
                Some(Editing::Edit(p)) => rsx! {
                    ProviderForm {
                        key: "{p.id}",
                        existing: Some(p.clone()),
                        on_done: move |_| {
                            editing.set(None);
                            reload += 1;
                        },
                        on_cancel: move |_| editing.set(None),
                    }
                },
                None => rsx! {},
            }
        }
    }
}

#[component]
fn ProviderRow(
    provider: GeoProvider,
    can_manage: bool,
    confirming: bool,
    on_edit: EventHandler<GeoProvider>,
    on_ask_delete: EventHandler<Uuid>,
    on_deleted: EventHandler<()>,
) -> Element {
    let mut testing = use_signal(|| false);
    let id = provider.id;
    let caps = provider
        .capabilities
        .iter()
        .map(|c| {
            if provider.default_for.contains(c) {
                format!("{} ★", cap_label(*c))
            } else {
                cap_label(*c)
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let for_edit = provider.clone();
    rsx! {
        tr { class: "border-b border-gray-100",
            td { class: "py-2 pr-4 text-gray-900",
                span { class: "font-medium", {provider.name.clone()} }
                p { class: "text-xs text-gray-500 md:hidden", {caps.clone()} }
            }
            td { class: "py-2 pr-4 text-gray-600", {kind_label(provider.kind)} }
            td { class: "py-2 pr-4 hidden text-gray-600 text-xs md:table-cell", {caps} }
            td { class: "py-2 pl-2 text-right space-x-1 sticky right-0 bg-white whitespace-nowrap shadow-[-6px_0_6px_-6px_rgba(0,0,0,0.25)] md:static md:shadow-none",
                if can_manage {
                    button {
                        r#type: "button",
                        class: BTN,
                        disabled: testing(),
                        onclick: move |_| {
                            testing.set(true);
                            spawn(async move {
                                match api::geo::test_provider(id).await {
                                    Ok(ms) => toasts::success(t!("geo-test-ok", ms : ms).to_string()),
                                    Err(err) => toasts::error(err),
                                }
                                testing.set(false);
                            });
                        },
                        {t!("geo-test")}
                    }
                    button {
                        r#type: "button",
                        class: BTN,
                        onclick: move |_| on_edit.call(for_edit.clone()),
                        {t!("geo-edit")}
                    }
                    if confirming {
                        button {
                            r#type: "button",
                            class: "px-2 py-1 text-xs rounded-lg bg-red-600 text-white hover:bg-red-700",
                            onclick: move |_| {
                                spawn(async move {
                                    match api::geo::delete_provider(id).await {
                                        Ok(()) => {
                                            toasts::success(t!("geo-deleted").to_string());
                                            on_deleted.call(());
                                        }
                                        Err(err) => toasts::error(err),
                                    }
                                });
                            },
                            {t!("geo-delete-confirm")}
                        }
                    } else {
                        button {
                            r#type: "button",
                            class: "px-2 py-1 text-xs border border-red-200 text-red-700 rounded-lg hover:bg-red-50",
                            onclick: move |_| on_ask_delete.call(id),
                            {t!("geo-delete")}
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn ProviderForm(
    existing: Option<GeoProvider>,
    on_done: EventHandler<()>,
    on_cancel: EventHandler<()>,
) -> Element {
    let init = existing.clone();
    let mut name = use_signal(|| init.as_ref().map(|p| p.name.clone()).unwrap_or_default());
    let kind = use_signal(|| {
        init.as_ref()
            .map(|p| p.kind)
            .unwrap_or(GeoProviderKind::Nominatim)
    });
    let caps = use_signal(|| {
        init.as_ref()
            .map(|p| p.capabilities.clone())
            .unwrap_or_else(|| GeoProviderKind::Nominatim.supported().to_vec())
    });
    let defaults = use_signal(|| {
        init.as_ref()
            .map(|p| p.default_for.clone())
            .unwrap_or_default()
    });
    let mut base_url = use_signal(|| {
        init.as_ref()
            .map(|p| p.base_url.clone())
            .unwrap_or_default()
    });
    let mut dark_url = use_signal(|| {
        init.as_ref()
            .and_then(|p| p.params.get(STYLE_URL_DARK).cloned())
            .unwrap_or_default()
    });
    let api_key =
        use_signal(|| SecretDraft::from_view(init.as_ref().and_then(|p| p.api_key.clone())));
    let mut key_param = use_signal(|| {
        init.as_ref()
            .map(|p| p.key_param.clone())
            .unwrap_or_else(|| DEFAULT_KEY_PARAM.to_string())
    });
    let mut params = use_signal(|| {
        init.as_ref()
            .map(|p| {
                let mut m = p.params.clone();
                m.remove(STYLE_URL_DARK);
                lines_of(&m)
            })
            .unwrap_or_default()
    });
    let mut rate = use_signal(|| {
        init.as_ref()
            .and_then(|p| p.rate_limit_per_s)
            .map(|r| r.to_string())
            .unwrap_or_default()
    });
    let mut store_allowed = use_signal(|| init.as_ref().is_some_and(|p| p.store_allowed));
    let timeout_ms = init.as_ref().map(|p| p.timeout_ms).unwrap_or(5_000);
    let mut busy = use_signal(|| false);
    let id = existing.as_ref().map(|p| p.id);

    let save = move |_| {
        let mut p = map_of(&params());
        let dark = dark_url().trim().to_string();
        if kind() == GeoProviderKind::Basemap && !dark.is_empty() {
            p.insert(STYLE_URL_DARK.to_string(), dark);
        }
        let input = GeoProviderInput {
            name: name(),
            kind: kind(),
            capabilities: caps(),
            base_url: base_url().trim().to_string(),
            api_key: api_key.read().to_input(),
            key_param: Some(key_param().trim().to_string()).filter(|k| !k.is_empty()),
            params: p,
            rate_limit_per_s: rate().trim().parse().ok(),
            timeout_ms,
            store_allowed: store_allowed(),
            default_for: defaults(),
        };
        busy.set(true);
        spawn(async move {
            let result = match id {
                Some(id) => api::geo::update_provider(id, &input).await,
                None => api::geo::create_provider(&input).await,
            };
            busy.set(false);
            match result {
                Ok(_) => {
                    toasts::success(t!("geo-saved").to_string());
                    on_done.call(());
                }
                Err(err) => toasts::error(err),
            }
        });
    };

    let is_basemap = kind() == GeoProviderKind::Basemap;
    let supported = kind().supported().to_vec();
    rsx! {
        div { class: "mt-2 rounded-lg border border-gray-200 bg-gray-50 p-4 space-y-3",
            div { class: "grid grid-cols-1 md:grid-cols-2 gap-3",
                div {
                    label {
                        r#for: "geo-providers-name",
                        class: "block text-xs text-gray-600 mb-1",
                        {t!("geo-name")}
                    }
                    input {
                        id: "geo-providers-name",
                        class: INPUT,
                        r#type: "text",
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                    }
                }
                div {
                    label {
                        r#for: "geo-providers-kind",
                        class: "block text-xs text-gray-600 mb-1",
                        {t!("geo-kind")}
                    }
                    select {
                        id: "geo-providers-kind",
                        class: INPUT,
                        onchange: move |e| on_kind(e.value(), kind, caps, defaults),
                        for k in KINDS {
                            option { value: k.as_str(), selected: kind() == k, {kind_label(k)} }
                        }
                    }
                }
                div { class: "md:col-span-2",
                    label {
                        r#for: "geo-providers-url",
                        class: "block text-xs text-gray-600 mb-1",
                        if is_basemap {
                            {t!("geo-style-url")}
                        } else {
                            {t!("geo-base-url")}
                        }
                    }
                    input {
                        id: "geo-providers-url",
                        class: INPUT,
                        r#type: "url",
                        value: "{base_url}",
                        oninput: move |e| base_url.set(e.value()),
                    }
                }
                if is_basemap {
                    div { class: "md:col-span-2",
                        label {
                            r#for: "geo-providers-dark",
                            class: "block text-xs text-gray-600 mb-1",
                            {t!("geo-style-url-dark")}
                        }
                        input {
                            id: "geo-providers-dark",
                            class: INPUT,
                            r#type: "url",
                            value: "{dark_url}",
                            oninput: move |e| dark_url.set(e.value()),
                        }
                    }
                }
            }
            fieldset { class: "space-y-1",
                legend { class: "block text-xs text-gray-600 mb-1", {t!("geo-capabilities")} }
                for c in supported {
                    div { class: "flex flex-wrap items-center gap-4 text-sm text-gray-700",
                        label { class: "inline-flex items-center gap-2",
                            input {
                                r#type: "checkbox",
                                checked: caps().contains(&c),
                                onchange: move |e| toggle(caps, c, e.checked()),
                            }
                            {cap_label(c)}
                        }
                        label { class: "inline-flex items-center gap-2 text-xs text-gray-500",
                            input {
                                r#type: "checkbox",
                                disabled: !caps().contains(&c),
                                checked: defaults().contains(&c),
                                onchange: move |e| toggle(defaults, c, e.checked()),
                            }
                            {t!("geo-is-default")}
                        }
                    }
                }
            }
            SecretField {
                label: t!("geo-api-key").to_string(),
                draft: api_key,
                can_manage: true,
            }
            div { class: "grid grid-cols-1 md:grid-cols-2 gap-3",
                div {
                    label {
                        r#for: "geo-providers-key-param",
                        class: "block text-xs text-gray-600 mb-1",
                        {t!("geo-key-param")}
                    }
                    input {
                        id: "geo-providers-key-param",
                        class: INPUT,
                        r#type: "text",
                        value: "{key_param}",
                        oninput: move |e| key_param.set(e.value()),
                    }
                }
            }
            if !is_basemap {
                div { class: "grid grid-cols-1 md:grid-cols-2 gap-3",
                    div {
                        label {
                            r#for: "geo-providers-params",
                            class: "block text-xs text-gray-600 mb-1",
                            {t!("geo-params")}
                        }
                        textarea {
                            id: "geo-providers-params",
                            class: INPUT,
                            rows: 2,
                            placeholder: "accept-language=fr",
                            value: "{params}",
                            oninput: move |e| params.set(e.value()),
                        }
                    }
                    div {
                        label {
                            r#for: "geo-providers-rate",
                            class: "block text-xs text-gray-600 mb-1",
                            {t!("geo-rate-limit")}
                        }
                        input {
                            id: "geo-providers-rate",
                            class: INPUT,
                            r#type: "number",
                            min: "0.1",
                            step: "0.1",
                            placeholder: "1",
                            value: "{rate}",
                            oninput: move |e| rate.set(e.value()),
                        }
                    }
                }
                label { class: "inline-flex items-center gap-2 text-sm text-gray-700",
                    input {
                        r#type: "checkbox",
                        checked: store_allowed(),
                        onchange: move |e| store_allowed.set(e.checked()),
                    }
                    {t!("geo-store-allowed")}
                }
            }
            div { class: "flex gap-2",
                button {
                    r#type: "button",
                    class: BTN_PRIMARY,
                    disabled: busy(),
                    onclick: save,
                    {t!("geo-save")}
                }
                button {
                    r#type: "button",
                    class: "px-4 py-2 border border-gray-300 text-gray-700 rounded-lg hover:bg-gray-50 text-sm",
                    onclick: move |_| on_cancel.call(()),
                    {t!("geo-cancel")}
                }
            }
        }
    }
}

/// Adds or removes `c` from a capability list.
fn toggle(mut list: Signal<Vec<GeoCapability>>, c: GeoCapability, on: bool) {
    let mut next = list();
    next.retain(|x| *x != c);
    if on {
        next.push(c);
    }
    list.set(next);
}

/// A new kind resets the capabilities to what it supports.
fn on_kind(
    value: String,
    mut kind: Signal<GeoProviderKind>,
    mut caps: Signal<Vec<GeoCapability>>,
    mut defaults: Signal<Vec<GeoCapability>>,
) {
    let Some(k) = GeoProviderKind::parse(&value) else {
        return;
    };
    kind.set(k);
    caps.set(k.supported().to_vec());
    defaults.set(Vec::new());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn param_lines_roundtrip() {
        let m = map_of("accept-language=fr\n\nbad line\n =x\nviewbox=a=b");
        assert_eq!(m.get("accept-language").map(String::as_str), Some("fr"));
        assert_eq!(m.get("viewbox").map(String::as_str), Some("a=b"));
        assert_eq!(m.len(), 2);
        assert_eq!(map_of(&lines_of(&m)), m);
    }
}
