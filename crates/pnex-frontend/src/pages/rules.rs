//! Page `/rules` (analysis.md D829–D830): conditions on objects or measures that
//! notify when they turn true, and their replay on the past. The server checks
//! every right; the page hides what the caller may not do.

use std::collections::HashMap;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::rules::{Cmp, Rule, RuleCondition, RuleKind};

use crate::api;
use crate::api::error_i18n::localize;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::layout::{ListLayout, DANGER_BTN};
use crate::components::crud::pager::{page_slice, ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::ui::BTN_SECONDARY_SM;
use crate::state::access::can;

mod form;
mod replay;

/// What a rule watches, in words; `names` maps type keys to their labels.
fn watches(c: &RuleCondition, names: &HashMap<String, String>) -> String {
    match c.kind {
        RuleKind::Objects => {
            let name = names.get(&c.type_key).unwrap_or(&c.type_key);
            let n = c.filters.len();
            if n == 0 {
                name.clone()
            } else {
                format!("{name} · {}", t!("rules-filter-count", n: n))
            }
        }
        RuleKind::Series => {
            let cmp = cmp_label(c.cmp);
            format!("{} / {} {cmp} {}", c.device_id, c.metric, c.threshold)
        }
    }
}

pub(crate) fn cmp_label(c: Cmp) -> String {
    match c {
        Cmp::Gt => t!("rules-cmp-gt"),
        Cmp::Gte => t!("rules-cmp-gte"),
        Cmp::Lt => t!("rules-cmp-lt"),
        Cmp::Lte => t!("rules-cmp-lte"),
    }
    .to_string()
}

fn state_label(r: &Rule) -> String {
    if !r.enabled {
        t!("rules-off")
    } else if r.firing {
        t!("rules-firing")
    } else {
        t!("rules-quiet")
    }
    .to_string()
}

#[component]
pub fn Rules() -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut adding = use_signal(|| false);
    let mut editing = use_signal(|| None::<Rule>);
    let mut replaying = use_signal(|| None::<Rule>);
    let mut removing = use_signal(|| None::<Rule>);
    let page = use_signal(|| 0i64);
    let list = use_resource(move || async move {
        let _ = reload();
        api::rules::list().await
    });
    let types = use_resource(|| async { api::ontology::types().await.unwrap_or_default() });
    let names: HashMap<String, String> = types
        .read()
        .iter()
        .flatten()
        .map(|t| {
            (
                t.def.key.clone(),
                crate::components::ontology::type_label(&t.def),
            )
        })
        .collect();
    let (state, all) = match &*list.read() {
        None => (None, Vec::new()),
        Some(Ok(v)) => (Some(Ok(())), v.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), Vec::new()),
    };
    let count = all.len() as i64;
    let rows = page_slice(&all, page().clamp(0, (count - 1).max(0) / PAGE_SIZE));
    let can_write = can("rules:write");
    let can_delete = can("rules:delete");
    let found = all.clone();
    let columns = vec![
        Column::new(t!("pipe-col-name").to_string(), |r: &Rule| {
            rsx! {
                span { class: "font-medium text-ink break-words", "{r.name}" }
            }
        }),
        Column::new(t!("rules-col-watches").to_string(), move |r: &Rule| {
            rsx! {
                span { class: "break-words", "{watches(&r.condition, &names)}" }
            }
        })
        .secondary(),
        Column::new(t!("rules-col-state").to_string(), |r: &Rule| {
            rsx! { "{state_label(r)}" }
        }),
        Column::new(String::new(), move |r: &Rule| {
            let (for_replay, for_remove) = (r.clone(), r.clone());
            rsx! {
                span { class: "inline-flex flex-col items-stretch gap-2 md:flex-row md:items-center",
                    button {
                        class: BTN_SECONDARY_SM,
                        r#type: "button",
                        onclick: move |e: MouseEvent| {
                            e.stop_propagation();
                            replaying.set(Some(for_replay.clone()));
                        },
                        {t!("rules-replay")}
                    }
                    if can_delete {
                        button {
                            class: DANGER_BTN,
                            r#type: "button",
                            onclick: move |e: MouseEvent| {
                                e.stop_propagation();
                                removing.set(Some(for_remove.clone()));
                            },
                            {t!("common-delete")}
                        }
                    }
                }
            }
        })
        .header_sr_only()
        .actions(),
    ];
    let form_open = adding() || editing().is_some();
    rsx! {
        ListLayout {
            title: t!("rules-title").to_string(),
            subtitle: Some(t!("rules-subtitle").to_string()),
            can_write,
            add_label: Some(t!("rules-add").to_string()),
            on_add: move |_| adding.set(true),
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            ListStates {
                state,
                is_empty: all.is_empty(),
                empty_message: t!("rules-empty").to_string(),
                div { class: "space-y-4",
                    DataTable {
                        columns,
                        rows,
                        row_key: RowKey::new(|r: &Rule| r.id.clone()),
                        on_row_click: move |id: String| {
                            if can_write {
                                editing.set(found.iter().find(|r| r.id == id).cloned());
                            }
                        },
                    }
                    ListPager { count, page }
                }
            }
        }
        if form_open {
            form::RuleForm {
                key: "{editing().map(|r| r.id).unwrap_or_default()}",
                initial: editing(),
                on_close: move |_| {
                    adding.set(false);
                    editing.set(None);
                },
                on_saved: move |_| {
                    adding.set(false);
                    editing.set(None);
                    reload.with_mut(|r| *r += 1);
                },
            }
        }
        if let Some(r) = replaying() {
            replay::ReplayDialog { rule: r, on_close: move |_| replaying.set(None) }
        }
        if let Some(r) = removing() {
            ConfirmDialog {
                title: t!("rules-delete-title").to_string(),
                message: t!("rules-delete-message", name : r.name.clone()).to_string(),
                confirm_label: t!("common-delete").to_string(),
                on_cancel: move |_| removing.set(None),
                on_confirm: move |_| {
                    let id = r.id.clone();
                    removing.set(None);
                    spawn(async move {
                        match api::rules::delete(&id).await {
                            Ok(()) => reload.with_mut(|r| *r += 1),
                            Err(e) => crate::state::toasts::error(localize(&e)),
                        }
                    });
                },
            }
        }
    }
}
