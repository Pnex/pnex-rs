//! Packs tab (D190): install or upgrade the packs the server ships, and
//! the as-code round trip of the org schema (YAML export / import, D186).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::PackView;

use crate::api;
use crate::components::crud::layout::{ListLayout, PRIMARY_BTN};
use crate::state::{org, toasts};

#[component]
pub fn PacksTab() -> Element {
    let mut reload = use_signal(|| 0u32);
    let admin = org::current_can_administer();
    let packs = use_resource(move || async move {
        let _ = reload();
        api::ontology::packs().await.unwrap_or_default()
    });
    let rows: Vec<PackView> = packs.read().clone().unwrap_or_default();
    let mut yaml = use_signal(String::new);
    let mut busy = use_signal(|| false);
    rsx! {
        ListLayout {
            title: t!("onto-packs").to_string(),
            can_write: admin,
            subtitle: Some(t!("onto-packs-subtitle").to_string()),
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            div { class: "space-y-6",
                ul { class: "grid grid-cols-1 gap-3 md:grid-cols-2",
                    for p in rows {
                        PackCard {
                            key: "{p.key}",
                            pack: p.clone(),
                            admin,
                            on_installed: move |_| reload.with_mut(|r| *r += 1),
                        }
                    }
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

#[component]
fn PackCard(pack: PackView, admin: bool, on_installed: Callback<()>) -> Element {
    let mut busy = use_signal(|| false);
    let installed = pack.installed_version.clone();
    let up_to_date = installed.as_deref() == Some(pack.version.as_str());
    let key = pack.key.clone();
    rsx! {
        li {
            class: "bg-white rounded-lg border border-gray-200 p-4 space-y-2",
            "data-pack": "{pack.key}",
            div { class: "flex items-center gap-2",
                h3 { class: "text-sm font-semibold text-gray-900", "{pack.name}" }
                span { class: "text-xs text-gray-500", "v{pack.version}" }
                if let Some(v) = installed.clone() {
                    span { class: "ml-auto rounded bg-green-50 px-2 py-0.5 text-xs text-green-700",
                        {t!("onto-pack-installed", version : v)}
                    }
                }
            }
            p { class: "text-sm text-gray-600", "{pack.description}" }
            if admin && !up_to_date {
                button {
                    class: "{PRIMARY_BTN} disabled:opacity-40",
                    "data-testid": "onto-install-pack",
                    disabled: busy(),
                    onclick: move |_| {
                        let key = key.clone();
                        busy.set(true);
                        let done = t!("onto-pack-done").to_string();
                        spawn(async move {
                            match api::ontology::install_pack(&key).await {
                                Ok(_) => {
                                    toasts::success(done);
                                    on_installed.call(());
                                }
                                Err(e) => toasts::error(e),
                            }
                            busy.set(false);
                        });
                    },
                    if installed.is_some() {
                        {t!("onto-pack-upgrade")}
                    } else {
                        {t!("onto-pack-install")}
                    }
                }
            }
        }
    }
}
