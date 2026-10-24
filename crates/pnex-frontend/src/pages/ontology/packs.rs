//! Packs tab (D190): install or upgrade the packs the server ships, and
//! the as-code round trip of the org schema (YAML export / import, D186).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{PackSettingKind, PackSettingsInput, PackTakeInput, PackView};
use serde_json::{Map, Value};

use crate::api;
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::{ListLayout, PRIMARY_BTN};
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::ontology::tr;
use crate::components::ui::{BTN_SECONDARY_SM, FIELD_HELP, FIELD_LABEL, INPUT};
use crate::state::toasts;

#[component]
pub fn PacksTab(tabs: Element) -> Element {
    let mut reload = use_signal(|| 0u32);
    let admin = crate::state::access::can("ontology.schema:write");
    let packs = use_resource(move || async move {
        let _ = reload();
        api::ontology::packs().await.unwrap_or_default()
    });
    let rows: Vec<PackView> = packs.read().clone().unwrap_or_default();
    // Pack being installed (one at a time): disables its button.
    let mut installing = use_signal(|| None::<String>);
    // Pack whose settings are open (D368).
    let mut setting = use_signal(|| None::<PackView>);
    let columns = vec![
        Column::new(t!("onto-packs").to_string(), move |pack: &PackView| {
            let installed = pack.installed_version.clone();
            // Taking the pack's version needs the pack at its shipped one.
            let can_take = admin && installed.as_deref() == Some(pack.version.as_str());
            let key = pack.key.clone();
            rsx! {
                div { "data-pack": "{pack.key}",
                    div { class: "flex flex-wrap items-center gap-2",
                        span { class: "font-medium text-gray-900", {tr(&pack.name, &pack.names)} }
                        span { class: "text-xs text-gray-500", "v{pack.version}" }
                        if let Some(v) = installed {
                            span { class: "rounded bg-green-50 px-2 py-0.5 text-xs text-green-700",
                                {t!("onto-pack-installed", version : v)}
                            }
                        }
                    }
                    p { class: "text-xs text-gray-600", {tr(&pack.description, &pack.descriptions)} }
                    if !pack.kept.is_empty() {
                        div {
                            class: "mt-1 flex flex-wrap items-center gap-2 text-xs text-amber-700",
                            "data-testid": "onto-pack-edited",
                            span { {t!("onto-pack-edited")} }
                            for name in pack.kept.clone() {
                                span { key: "{name}", class: "inline-flex items-center gap-1",
                                    code { title: pack.differs.get(&name).map(|d| d.join(", ")).unwrap_or_default(),
                                        "{name}"
                                    }
                                    if can_take {
                                        button {
                                            class: "underline hover:text-amber-900",
                                            r#type: "button",
                                            "data-testid": "onto-pack-take",
                                            onclick: {
                                                let (key, name) = (key.clone(), name.clone());
                                                let import = pack.differs.contains_key(&name);
                                                move |_| {
                                                    let (key, name) = (key.clone(), name.clone());
                                                    let done = if import {
                                                        t!("onto-pack-taken-import", name : name.clone()).to_string()
                                                    } else {
                                                        t!("onto-pack-taken", name : name.clone()).to_string()
                                                    };
                                                    spawn(async move {
                                                        let input = PackTakeInput { names: vec![name] };
                                                        match api::ontology::take_pack_version(&key, &input).await {
                                                            Ok(_) => {
                                                                toasts::success(done);
                                                                reload.with_mut(|r| *r += 1);
                                                            }
                                                            Err(e) => toasts::error(e),
                                                        }
                                                    });
                                                }
                                            },
                                            {t!("onto-pack-take")}
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }),
        Column::new(t!("common-actions").to_string(), move |pack: &PackView| {
            let installed = pack.installed_version.is_some();
            let up_to_date = pack.installed_version.as_deref() == Some(pack.version.as_str());
            let key = pack.key.clone();
            let busy = installing().is_some();
            let with_settings = (!pack.settings.is_empty()).then(|| pack.clone());
            rsx! {
                if let Some(p) = with_settings.filter(|_| admin) {
                    button {
                        class: BTN_SECONDARY_SM,
                        r#type: "button",
                        "data-testid": "onto-pack-settings",
                        onclick: move |_| setting.set(Some(p.clone())),
                        {t!("onto-pack-settings")}
                    }
                }
                if admin && !up_to_date {
                    button {
                        class: "{PRIMARY_BTN} disabled:opacity-40",
                        "data-testid": "onto-install-pack",
                        disabled: busy,
                        onclick: move |_| {
                            let key = key.clone();
                            installing.set(Some(key.clone()));
                            let done = t!("onto-pack-done").to_string();
                            spawn(async move {
                                match api::ontology::install_pack(&key).await {
                                    Ok(view) => {
                                        toasts::success(done);
                                        if !view.kept.is_empty() {
                                            toasts::info(kept_message(&view.kept));
                                        }
                                        reload.with_mut(|r| *r += 1);
                                    }
                                    Err(e) => toasts::error(e),
                                }
                                installing.set(None);
                            });
                        },
                        if installed {
                            {t!("onto-pack-upgrade")}
                        } else {
                            {t!("onto-pack-install")}
                        }
                    }
                }
            }
        })
        .actions(),
    ];
    let mut yaml = use_signal(String::new);
    let mut busy = use_signal(|| false);
    rsx! {
        ListLayout {
            title: t!("onto-packs").to_string(),
            tabs,
            can_write: admin,
            subtitle: Some(t!("onto-packs-subtitle").to_string()),
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            if let Some(p) = setting() {
                SettingsDialog {
                    pack: p,
                    on_close: move |saved: bool| {
                        setting.set(None);
                        if saved {
                            reload.with_mut(|r| *r += 1);
                        }
                    },
                }
            }
            div { class: "space-y-6",
                DataTable {
                    columns,
                    rows,
                    row_key: RowKey::new(|pack: &PackView| pack.key.clone()),
                }
                div { class: "bg-white rounded-lg border border-gray-200 p-4 space-y-3",
                    h3 { class: "text-sm font-medium text-gray-900", {t!("onto-as-code")} }
                    p { class: "text-xs text-gray-500", {t!("onto-as-code-help")} }
                    button {
                        class: "px-3 py-1 text-sm text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50",
                        "data-testid": "onto-export",
                        onclick: move |_| {
                            spawn(async move {
                                match api::ontology::export_yaml().await {
                                    Ok(bytes) => {
                                        crate::util::trigger_download(
                                            "ontology.yaml",
                                            "application/yaml",
                                            &bytes,
                                        );
                                    }
                                    Err(e) => toasts::error(e),
                                }
                            });
                        },
                        {t!("onto-export")}
                    }
                    if admin {
                        textarea {
                            class: "w-full h-48 px-3 py-2 border border-gray-300 rounded-lg text-xs font-mono",
                            "data-testid": "onto-import-yaml",
                            placeholder: t!("onto-import-placeholder"),
                            value: "{yaml}",
                            oninput: move |e| yaml.set(e.value()),
                        }
                        button {
                            class: "{PRIMARY_BTN} disabled:opacity-40",
                            "data-testid": "onto-import",
                            disabled: busy() || yaml().trim().is_empty(),
                            onclick: move |_| {
                                let body = yaml().into_bytes();
                                busy.set(true);
                                let done = t!("onto-imported").to_string();
                                spawn(async move {
                                    match api::ontology::import_yaml(body).await {
                                        Ok(_) => {
                                            toasts::success(done);
                                            yaml.set(String::new());
                                            reload.with_mut(|r| *r += 1);
                                        }
                                        Err(e) => toasts::error(e),
                                    }
                                    busy.set(false);
                                });
                            },
                            {t!("onto-import")}
                        }
                    }
                }
            }
        }
    }
}

/// The functions and flows the org edited, left as they are (D368).
fn kept_message(kept: &[String]) -> String {
    t!("onto-pack-kept", names : kept.join(", ")).to_string()
}

/// The org's values of a pack's settings (D368); a value equal to the
/// pack's default is sent as `null` so the pack keeps owning it.
#[component]
fn SettingsDialog(pack: PackView, on_close: EventHandler<bool>) -> Element {
    let initial: Map<String, Value> = pack
        .settings
        .iter()
        .map(|s| (s.def.key.clone(), s.value.clone()))
        .collect();
    let mut values = use_signal(move || initial.clone());
    let mut busy = use_signal(|| false);
    let defs = pack.settings.clone();
    let key = pack.key.clone();
    let submit = move |_| {
        let sent: Map<String, Value> = defs
            .iter()
            .map(|s| {
                let v = values().get(&s.def.key).cloned().unwrap_or(Value::Null);
                let v = if v == s.def.default { Value::Null } else { v };
                (s.def.key.clone(), v)
            })
            .collect();
        let key = key.clone();
        busy.set(true);
        spawn(async move {
            match api::ontology::update_pack_settings(&key, &PackSettingsInput { values: sent })
                .await
            {
                Ok(_) => on_close.call(true),
                Err(e) => toasts::error(e),
            }
            busy.set(false);
        });
    };
    rsx! {
        FormDialog {
            title: t!("onto-pack-settings").to_string(),
            submit_label: t!("common-save").to_string(),
            on_close: move |_| on_close.call(false),
            on_submit: submit,
            busy: busy(),
            valid: true,
            div { class: "space-y-4",
                for s in pack.settings.clone() {
                    div { key: "{s.def.key}",
                        label {
                            class: FIELD_LABEL,
                            r#for: "pack-setting-{s.def.key}",
                            {tr(&s.def.name, &s.def.names)}
                        }
                        SettingInput {
                            key_name: s.def.key.clone(),
                            kind: s.def.kind,
                            value: values().get(&s.def.key).cloned().unwrap_or(Value::Null),
                            on_change: move |(k, v): (String, Value)| values.with_mut(|m| set(m, &k, v)),
                        }
                        p { class: FIELD_HELP,
                            {tr(&s.def.description, &s.def.descriptions)}
                            " "
                            {t!("onto-pack-setting-default", value : s.def.default.to_string())}
                        }
                    }
                }
            }
        }
    }
}

fn set(values: &mut Map<String, Value>, key: &str, v: Value) {
    values.insert(key.to_string(), v);
}

#[component]
fn SettingInput(
    key_name: String,
    kind: PackSettingKind,
    value: Value,
    on_change: EventHandler<(String, Value)>,
) -> Element {
    let id = format!("pack-setting-{key_name}");
    let k = key_name.clone();
    let emit = move |v: Value| on_change.call((k.clone(), v));
    match kind {
        PackSettingKind::Bool => rsx! {
            input {
                id,
                r#type: "checkbox",
                checked: value.as_bool().unwrap_or(false),
                onchange: move |e| emit(Value::Bool(e.checked())),
            }
        },
        PackSettingKind::Number => rsx! {
            input {
                id,
                r#type: "number",
                class: INPUT,
                value: "{value}",
                oninput: move |e| emit(number(&e.value())),
            }
        },
        PackSettingKind::Text => rsx! {
            input {
                id,
                r#type: "text",
                class: INPUT,
                value: "{value.as_str().unwrap_or_default()}",
                oninput: move |e| emit(Value::String(e.value())),
            }
        },
    }
}

/// A typed number (a whole one as an integer, so it equals the pack's
/// default), `null` when it is not one.
fn number(raw: &str) -> Value {
    match raw.trim().parse::<f64>() {
        Ok(n) if n.fract() == 0.0 && n.abs() < 1e15 => Value::from(n as i64),
        Ok(n) => serde_json::Number::from_f64(n).map_or(Value::Null, Value::Number),
        Err(_) => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn whole_numbers_equal_the_pack_default() {
        assert_eq!(super::number("80"), serde_json::json!(80));
        assert_eq!(super::number(" 0.5 "), serde_json::json!(0.5));
        assert!(super::number("abc").is_null());
    }
}
