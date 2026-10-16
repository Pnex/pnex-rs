//! Multi-select picker of media library photos: everything ticked is added
//! in **one** call, i.e. one source (train / validation split unit, D198).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::api::media::{MediaFilters, MediaKind};
use crate::components::modal::Modal;
use crate::components::ui::{BTN_PRIMARY, BTN_SECONDARY, INPUT};

/// Most photos listed at once (search narrows the list).
const PICKER_LIMIT: i64 = 200;

#[component]
pub(super) fn PhotoPicker(
    /// Assets already in the project (shown ticked and disabled).
    present: Vec<String>,
    on_close: EventHandler<()>,
    on_add: EventHandler<Vec<String>>,
) -> Element {
    let mut search = use_signal(String::new);
    let picked = use_signal(Vec::<String>::new);
    let photos = use_resource(move || {
        let q = search();
        async move {
            let filters = MediaFilters {
                kinds: vec![MediaKind::Photo],
                search: (!q.trim().is_empty()).then(|| q.trim().to_string()),
                limit: Some(PICKER_LIMIT),
                ..Default::default()
            };
            api::media::list(&filters).await.map(|p| p.results)
        }
    });
    let rows = photos.read().clone();
    let count = picked.read().len();

    rsx! {
        Modal {
            title: t!("lab-picker-title").to_string(),
            max_width: "max-w-lg".to_string(),
            on_close: move |_| on_close.call(()),
            div { class: "space-y-3",
                input {
                    class: "{INPUT} w-full",
                    r#type: "search",
                    placeholder: t!("lab-picker-search").to_string(),
                    value: "{search}",
                    oninput: move |e| search.set(e.value()),
                }
                p { class: "text-xs text-slate", {t!("lab-picker-help")} }
                div { class: "max-h-80 overflow-y-auto divide-y divide-gray-100 border border-line rounded-lg",
                    match rows {
                        None => rsx! {
                            p { class: "p-3 text-sm text-slate", {t!("common-loading")} }
                        },
                        Some(Err(e)) => rsx! {
                            p { class: "p-3 text-sm text-red-600", "{e}" }
                        },
                        Some(Ok(list)) if list.is_empty() => rsx! {
                            p { class: "p-3 text-sm text-slate", {t!("lab-picker-empty")} }
                        },
                        Some(Ok(list)) => rsx! {
                            for asset in list {
                                PhotoRow {
                                    key: "{asset.id}",
                                    id: asset.id.clone(),
                                    name: asset.name.clone(),
                                    already: present.contains(&asset.id),
                                    picked,
                                }
                            }
                        },
                    }
                }
                div { class: "flex justify-end gap-2 pt-2",
                    button {
                        class: BTN_SECONDARY,
                        r#type: "button",
                        onclick: move |_| on_close.call(()),
                        {t!("common-cancel")}
                    }
                    button {
                        class: BTN_PRIMARY,
                        r#type: "button",
                        disabled: count == 0,
                        onclick: move |_| on_add.call(picked.peek().clone()),
                        {t!("lab-picker-add", count : count)}
                    }
                }
            }
        }
    }
}

#[component]
fn PhotoRow(id: String, name: String, already: bool, mut picked: Signal<Vec<String>>) -> Element {
    let on = already || picked.read().contains(&id);
    rsx! {
        label { class: "flex items-center gap-2 px-3 py-2 text-sm cursor-pointer hover:bg-gray-50",
            input {
                class: "h-4 w-4 accent-signal-600",
                r#type: "checkbox",
                checked: on,
                disabled: already,
                onchange: move |e| {
                    let id = id.clone();
                    let mut list = picked.write();
                    list.retain(|x| x != &id);
                    if e.checked() {
                        list.push(id);
                    }
                },
            }
            span { class: "truncate", "{name}" }
        }
    }
}
