//! Page `/mt5-terminals/:id`: connecting one MT5 terminal (its generated
//! Expert Advisor), what it sends, a backfill, and its catalogue.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::mt5::{Mt5Symbol, Mt5Terminal, Mt5TerminalInput, BACKFILL_DAYS_MAX};

use crate::api;
use crate::app::Route;
use crate::components::badges::date_label;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::filters::{FilterBar, SearchInput};
use crate::components::crud::layout::{ListLayout, DANGER_BTN};
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::ui::{
    BTN_PRIMARY_SM, BTN_SECONDARY_SM, FIELD_HELP, FIELD_LABEL, INPUT, PANEL,
};
use crate::state::access::can;
use crate::state::toasts;

use super::PresenceDot;

#[component]
pub fn Mt5TerminalDetail(id: String) -> Element {
    let mut reload = use_signal(|| 0u32);
    let key = id.clone();
    let terminal = use_resource(move || {
        let id = key.clone();
        async move {
            let _ = reload();
            api::mt5::get(&id).await
        }
    });
    let nav = use_navigator();
    let mut removing = use_signal(|| false);
    let (state, found) = match &*terminal.value().read() {
        None => (None, None),
        Some(Ok(t)) => (Some(Ok(())), Some(t.clone())),
        Some(Err(e)) => (Some(Err(e.clone())), None),
    };
    let title = found
        .as_ref()
        .map(|t| t.name.clone())
        .unwrap_or_else(|| t!("mt5-title").to_string());
    let can_delete = can("metatrader:delete");
    rsx! {
        ListLayout {
            title,
            subtitle: Some(t!("mt5-detail-subtitle").to_string()),
            can_write: false,
            on_back: move |_| {
                nav.push(Route::Mt5Terminals {});
            },
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            ListStates { state, is_empty: false, empty_message: String::new(),
                if let Some(t) = found.clone() {
                    div { class: "space-y-6",
                        Status { terminal: t.clone() }
                        Connect {
                            terminal: t.clone(),
                            on_done: move |_| reload.with_mut(|r| *r += 1),
                        }
                        Selection {
                            key: "{t.version}",
                            terminal: t.clone(),
                            on_saved: move |_| reload.with_mut(|r| *r += 1),
                        }
                        Backfill {
                            terminal: t.clone(),
                            on_done: move |_| reload.with_mut(|r| *r += 1),
                        }
                        Catalogue { key: "{t.version}", id: t.id.clone() }
                        if can_delete {
                            div { class: "flex justify-end",
                                button {
                                    class: DANGER_BTN,
                                    r#type: "button",
                                    onclick: move |_| removing.set(true),
                                    {t!("mt5-delete")}
                                }
                            }
                        }
                    }
                }
            }
        }
        if removing() {
            if let Some(t) = found.clone() {
                ConfirmDialog {
                    title: t!("mt5-delete").to_string(),
                    message: t!("mt5-delete-message", name : t.name.clone()).to_string(),
                    confirm_label: t!("common-delete").to_string(),
                    on_cancel: move |_| removing.set(false),
                    on_confirm: move |_| {
                        let id = t.id.clone();
                        removing.set(false);
                        spawn(async move {
                            match api::mt5::delete(&id).await {
                                Ok(()) => {
                                    nav.push(Route::Mt5Terminals {});
                                }
                                Err(e) => toasts::error(e),
                            }
                        });
                    },
                }
            }
        }
    }
}

/// Presence, last call, EA version and what the broker announced.
#[component]
fn Status(terminal: Mt5Terminal) -> Element {
    let seen = terminal
        .last_seen_at
        .as_deref()
        .map(date_label)
        .unwrap_or_else(|| t!("mt5-never-seen").to_string());
    let ea = terminal
        .ea_version
        .clone()
        .map(|v| match terminal.terminal_build {
            Some(b) => t!("mt5-ea-version-build", version : v, build : b).to_string(),
            None => t!("mt5-ea-version", version : v).to_string(),
        })
        .unwrap_or_else(|| "—".to_string());
    let broker = [
        terminal.broker.as_str(),
        terminal.server.as_str(),
        terminal.account.as_str(),
    ]
    .iter()
    .filter(|s| !s.is_empty())
    .copied()
    .collect::<Vec<_>>()
    .join(", ");
    rsx! {
        dl { class: "{PANEL} grid grid-cols-1 gap-4 p-4 sm:grid-cols-4",
            div {
                dt { class: "text-xs text-slate", {t!("mt5-col-presence")} }
                dd { class: "mt-1 text-sm text-ink",
                    PresenceDot { presence: terminal.presence }
                }
            }
            div {
                dt { class: "text-xs text-slate", {t!("mt5-last-seen")} }
                dd { class: "mt-1 text-sm text-ink", "{seen}" }
            }
            div {
                dt { class: "text-xs text-slate", {t!("mt5-ea")} }
                dd { class: "mt-1 text-sm text-ink", "{ea}" }
            }
            div {
                dt { class: "text-xs text-slate", {t!("mt5-field-broker")} }
                dd { class: "mt-1 text-sm text-ink break-words",
                    if broker.is_empty() {
                        "—"
                    } else {
                        "{broker}"
                    }
                }
            }
        }
    }
}

/// The three steps that connect the terminal; the EA carries its token.
#[component]
fn Connect(terminal: Mt5Terminal, on_done: Callback<()>) -> Element {
    let mut confirming = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let can_write = can("metatrader:write");
    let id = terminal.id.clone();
    let mut download = move |id: String| {
        busy.set(true);
        spawn(async move {
            match api::mt5::expert_advisor(&id).await {
                Ok(bytes) => {
                    crate::util::save_blob("PnexTerminal.mq5", &bytes);
                    toasts::success("mt5-ea-downloaded");
                    on_done.call(());
                }
                Err(e) => toasts::error(e),
            }
            busy.set(false);
        });
    };
    let has_token = terminal.has_token;
    let for_confirm = id.clone();
    rsx! {
        section { class: "{PANEL} p-4 space-y-3",
            h2 { class: "text-base font-semibold text-ink", {t!("mt5-connect-title")} }
            ol { class: "list-decimal space-y-2 pl-5 text-sm text-ink",
                li {
                    {t!("mt5-connect-step-download")}
                    if can_write {
                        div { class: "mt-2",
                            button {
                                class: BTN_PRIMARY_SM,
                                r#type: "button",
                                disabled: busy(),
                                onclick: move |_| {
                                    if has_token {
                                        confirming.set(true);
                                    } else {
                                        download(id.clone());
                                    }
                                },
                                {t!("mt5-ea-download")}
                            }
                        }
                    }
                }
                li { {t!("mt5-connect-step-allow")} }
                li { {t!("mt5-connect-step-attach")} }
            }
            p { class: FIELD_HELP, {t!("mt5-connect-read-only")} }
        }
        if confirming() {
            ConfirmDialog {
                title: t!("mt5-ea-download").to_string(),
                message: t!("mt5-ea-replace-message").to_string(),
                confirm_label: t!("mt5-ea-download").to_string(),
                on_cancel: move |_| confirming.set(false),
                on_confirm: move |_| {
                    confirming.set(false);
                    download(for_confirm.clone());
                },
            }
        }
    }
}

/// The symbols whose bars and ticks the terminal sends.
#[component]
fn Selection(terminal: Mt5Terminal, on_saved: Callback<()>) -> Element {
    let mut bars = use_signal(|| terminal.bars.clone());
    let mut ticks = use_signal(|| terminal.ticks.clone());
    let mut busy = use_signal(|| false);
    let can_write = can("metatrader:write");
    let t0 = terminal.clone();
    let save = move |_| {
        let input = Mt5TerminalInput {
            name: t0.name.clone(),
            broker: t0.broker.clone(),
            server: t0.server.clone(),
            account: t0.account.clone(),
            bars: Some(bars()),
            ticks: Some(ticks()),
            expected_version: Some(t0.version),
        };
        let id = t0.id.clone();
        busy.set(true);
        spawn(async move {
            match api::mt5::save(Some(&id), &input).await {
                Ok(_) => {
                    toasts::success("toast-saved");
                    on_saved.call(());
                }
                Err(e) => toasts::error(e),
            }
            busy.set(false);
        });
    };
    rsx! {
        section { class: "{PANEL} p-4 space-y-4",
            h2 { class: "text-base font-semibold text-ink", {t!("mt5-selection-title")} }
            div { class: "grid grid-cols-1 gap-4 md:grid-cols-2",
                div {
                    label { class: FIELD_LABEL, r#for: "mt5-bars", {t!("mt5-col-bars")} }
                    input {
                        id: "mt5-bars",
                        class: INPUT,
                        disabled: !can_write,
                        value: "{bars}",
                        oninput: move |e| bars.set(e.value()),
                    }
                    p { class: FIELD_HELP, {t!("mt5-bars-help")} }
                }
                div {
                    label { class: FIELD_LABEL, r#for: "mt5-ticks", {t!("mt5-col-ticks")} }
                    input {
                        id: "mt5-ticks",
                        class: INPUT,
                        disabled: !can_write,
                        placeholder: "XAUUSD",
                        value: "{ticks}",
                        oninput: move |e| ticks.set(e.value()),
                    }
                    p { class: FIELD_HELP, {t!("mt5-ticks-help")} }
                }
            }
            if can_write {
                div { class: "flex justify-end",
                    button {
                        class: BTN_PRIMARY_SM,
                        r#type: "button",
                        disabled: busy(),
                        onclick: save,
                        {t!("common-save")}
                    }
                }
            }
        }
    }
}

/// Asks the terminal to send past 1-minute bars again.
#[component]
fn Backfill(terminal: Mt5Terminal, on_done: Callback<()>) -> Element {
    let mut days = use_signal(|| "30".to_string());
    let mut busy = use_signal(|| false);
    let can_write = can("metatrader:write");
    let id = terminal.id.clone();
    let last = terminal.backfill_requested_at.as_deref().map(|at| {
        t!("mt5-backfill-last", days : terminal.backfill_days, at : date_label(at)).to_string()
    });
    let ask = move |_| {
        let Ok(n) = days().trim().parse::<u32>() else {
            toasts::error("mt5-backfill-invalid");
            return;
        };
        if !(1..=BACKFILL_DAYS_MAX).contains(&n) {
            toasts::error("mt5-backfill-invalid");
            return;
        }
        let id = id.clone();
        busy.set(true);
        spawn(async move {
            match api::mt5::backfill(&id, n).await {
                Ok(_) => {
                    toasts::success("mt5-backfill-asked");
                    on_done.call(());
                }
                Err(e) => toasts::error(e),
            }
            busy.set(false);
        });
    };
    rsx! {
        section { class: "{PANEL} p-4 space-y-3",
            h2 { class: "text-base font-semibold text-ink", {t!("mt5-backfill-title")} }
            p { class: "text-sm text-slate", {t!("mt5-backfill-help")} }
            if can_write {
                div { class: "flex flex-wrap items-end gap-2",
                    div {
                        label { class: FIELD_LABEL, r#for: "mt5-days", {t!("mt5-backfill-days")} }
                        input {
                            id: "mt5-days",
                            class: "w-28 {INPUT}",
                            r#type: "number",
                            min: "1",
                            max: "3650",
                            value: "{days}",
                            oninput: move |e| days.set(e.value()),
                        }
                    }
                    button {
                        class: BTN_SECONDARY_SM,
                        r#type: "button",
                        disabled: busy(),
                        onclick: ask,
                        {t!("mt5-backfill-ask")}
                    }
                }
            }
            if let Some(l) = last {
                p { class: FIELD_HELP, "{l}" }
            }
        }
    }
}

/// The symbols the terminal announced, and what its selection sends.
#[component]
fn Catalogue(id: String) -> Element {
    let search = use_signal(String::new);
    let mut page = use_signal(|| 0i64);
    let mut reload = use_signal(|| 0u32);
    let key = id.clone();
    let list = use_resource(move || {
        let (id, q, offset) = (key.clone(), search(), page() * PAGE_SIZE);
        async move {
            let _ = reload();
            api::mt5::symbols(&id, &q, PAGE_SIZE, offset).await
        }
    });
    let (state, count, rows) = match &*list.value().read() {
        None => (None, 0, Vec::new()),
        Some(Ok(p)) => (Some(Ok(())), p.count, p.results.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), 0, Vec::new()),
    };
    let yes = t!("mt5-yes").to_string();
    let (yes_bars, yes_ticks) = (yes.clone(), yes);
    let columns = vec![
        Column::new(t!("mt5-col-symbol").to_string(), |s: &Mt5Symbol| {
            rsx! {
                span { class: "block font-medium text-ink break-words", "{s.symbol}" }
                if !s.description.is_empty() {
                    span { class: "block text-xs text-slate break-words", "{s.description}" }
                }
            }
        }),
        Column::new(t!("mt5-col-history").to_string(), |s: &Mt5Symbol| {
            let from = s
                .first_m1
                .as_deref()
                .map(date_label)
                .unwrap_or_else(|| "—".to_string());
            rsx! { "{from}" }
        })
        .secondary(),
        Column::new(t!("mt5-col-bars").to_string(), move |s: &Mt5Symbol| {
            let v = if s.bars {
                yes_bars.clone()
            } else {
                String::new()
            };
            rsx! { "{v}" }
        }),
        Column::new(t!("mt5-col-ticks").to_string(), move |s: &Mt5Symbol| {
            let v = if s.ticks {
                yes_ticks.clone()
            } else {
                String::new()
            };
            rsx! { "{v}" }
        }),
    ];
    rsx! {
        section { class: "{PANEL} p-4 space-y-3",
            h2 { class: "text-base font-semibold text-ink", {t!("mt5-catalogue-title")} }
            FilterBar {
                SearchInput {
                    placeholder: t!("mt5-catalogue-search").to_string(),
                    value: search,
                    on_submit: move |_| {
                        page.set(0);
                        reload.with_mut(|r| *r += 1);
                    },
                }
            }
            ListStates {
                state,
                is_empty: count == 0,
                empty_message: t!("mt5-catalogue-empty").to_string(),
                div { class: "space-y-4",
                    DataTable {
                        columns,
                        rows,
                        row_key: RowKey::new(|s: &Mt5Symbol| s.symbol.clone()),
                    }
                    ListPager { count, page }
                }
            }
        }
    }
}
