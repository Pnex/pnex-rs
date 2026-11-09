//! Page `/mt5-terminals` (markets-research.md appendix E, D847–D849): the
//! MetaTrader 5 terminals registered as data sources of the org, their
//! presence and what they send. The server checks every right; the page
//! hides what the caller may not do.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::mt5::{Mt5Terminal, Mt5TerminalInput, Presence};

use crate::api;
use crate::app::Route;
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::ListLayout;
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::ui::{FIELD_HELP, FIELD_LABEL, INPUT};
use crate::state::access::can;

mod detail;
pub use detail::Mt5TerminalDetail;

/// The presence as a coloured dot and its word.
pub(crate) fn presence_label(p: Presence) -> (&'static str, String) {
    match p {
        Presence::Online => ("bg-ok", t!("mt5-presence-online").to_string()),
        Presence::Late => ("bg-warn", t!("mt5-presence-late").to_string()),
        Presence::Offline => ("bg-alarm", t!("mt5-presence-offline").to_string()),
        Presence::Never => ("bg-idle", t!("mt5-presence-never").to_string()),
    }
}

/// What a selection sends, in words: `*` is the whole catalogue.
pub(crate) fn selection_label(patterns: &str) -> String {
    match patterns.trim() {
        "" => t!("mt5-selection-none").to_string(),
        "*" => t!("mt5-selection-all").to_string(),
        p => p.to_string(),
    }
}

#[component]
pub(crate) fn PresenceDot(presence: Presence) -> Element {
    let (dot, label) = presence_label(presence);
    rsx! {
        span { class: "inline-flex items-center gap-1.5 whitespace-nowrap",
            span {
                class: "h-2 w-2 shrink-0 rounded-full {dot}",
                aria_hidden: "true",
            }
            "{label}"
        }
    }
}

#[component]
pub fn Mt5Terminals() -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut adding = use_signal(|| false);
    let page = use_signal(|| 0i64);
    let list = use_resource(move || {
        let offset = page() * PAGE_SIZE;
        async move {
            let _ = reload();
            api::mt5::list(PAGE_SIZE, offset).await
        }
    });
    let (state, count, rows) = match &*list.value().read() {
        None => (None, 0, Vec::new()),
        Some(Ok(p)) => (Some(Ok(())), p.count, p.results.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), 0, Vec::new()),
    };
    let can_write = can("metatrader:write");
    let nav = use_navigator();
    let columns = vec![
        Column::new(t!("mt5-col-name").to_string(), |t: &Mt5Terminal| {
            let detail = [t.broker.as_str(), t.server.as_str()]
                .iter()
                .filter(|s| !s.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(", ");
            rsx! {
                span { class: "block font-medium text-ink break-words", "{t.name}" }
                if !detail.is_empty() {
                    span { class: "block text-xs text-slate break-words", "{detail}" }
                }
            }
        }),
        Column::new(t!("mt5-col-presence").to_string(), |t: &Mt5Terminal| {
            rsx! {
                PresenceDot { presence: t.presence }
            }
        }),
        Column::new(t!("mt5-col-bars").to_string(), |t: &Mt5Terminal| {
            rsx! {
                span { class: "break-words", "{selection_label(&t.bars)}" }
            }
        })
        .secondary(),
        Column::new(t!("mt5-col-ticks").to_string(), |t: &Mt5Terminal| {
            rsx! {
                span { class: "break-words", "{selection_label(&t.ticks)}" }
            }
        })
        .secondary(),
        Column::new(t!("mt5-col-symbols").to_string(), |t: &Mt5Terminal| {
            rsx! { "{t.symbols}" }
        })
        .secondary(),
    ];
    rsx! {
        ListLayout {
            title: t!("mt5-title").to_string(),
            subtitle: Some(t!("mt5-subtitle").to_string()),
            can_write,
            add_label: Some(t!("mt5-add").to_string()),
            on_add: move |_| adding.set(true),
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            ListStates {
                state,
                is_empty: count == 0,
                empty_message: t!("mt5-empty").to_string(),
                div { class: "space-y-4",
                    DataTable {
                        columns,
                        rows,
                        row_key: RowKey::new(|t: &Mt5Terminal| t.id.clone()),
                        on_row_click: move |id: String| {
                            nav.push(Route::Mt5TerminalDetail { id });
                        },
                    }
                    ListPager { count, page }
                }
            }
        }
        if adding() {
            NewTerminal {
                on_close: move |_| adding.set(false),
                on_saved: move |id: String| {
                    adding.set(false);
                    nav.push(Route::Mt5TerminalDetail { id });
                },
            }
        }
    }
}

/// Registers a terminal: a name, and the broker details as typed.
#[component]
fn NewTerminal(on_close: Callback<()>, on_saved: Callback<String>) -> Element {
    let mut name = use_signal(String::new);
    let mut broker = use_signal(String::new);
    let mut account = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let submit = move |_| {
        let input = Mt5TerminalInput {
            name: name(),
            broker: broker(),
            account: account(),
            ..Default::default()
        };
        busy.set(true);
        spawn(async move {
            match api::mt5::save(None, &input).await {
                Ok(t) => on_saved.call(t.id),
                Err(e) => crate::state::toasts::error(e),
            }
            busy.set(false);
        });
    };
    rsx! {
        FormDialog {
            title: t!("mt5-add").to_string(),
            submit_label: t!("mt5-create").to_string(),
            on_close,
            on_submit: submit,
            busy: busy(),
            valid: !name().trim().is_empty(),
            div {
                label { class: FIELD_LABEL, r#for: "mt5-name", {t!("mt5-field-name")} }
                input {
                    id: "mt5-name",
                    class: INPUT,
                    maxlength: "120",
                    value: "{name}",
                    oninput: move |e| name.set(e.value()),
                }
                p { class: FIELD_HELP, {t!("mt5-field-name-help")} }
            }
            div {
                label { class: FIELD_LABEL, r#for: "mt5-broker", {t!("mt5-field-broker")} }
                input {
                    id: "mt5-broker",
                    class: INPUT,
                    maxlength: "120",
                    value: "{broker}",
                    oninput: move |e| broker.set(e.value()),
                }
                p { class: FIELD_HELP, {t!("mt5-field-broker-help")} }
            }
            div {
                label { class: FIELD_LABEL, r#for: "mt5-account", {t!("mt5-field-account")} }
                input {
                    id: "mt5-account",
                    class: INPUT,
                    maxlength: "120",
                    value: "{account}",
                    oninput: move |e| account.set(e.value()),
                }
            }
            p { class: FIELD_HELP, {t!("mt5-new-help")} }
        }
    }
}
