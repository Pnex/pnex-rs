//! QR tags (parts-docs.md D962–D963, lot PD1): the page a scan opens
//! (`/q/:token`), the tag of an object with its print, and the printable
//! sheet of labels (outside the shell, so only the labels print).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::OntologyQuery;
use pnex_core::qr::{QrState, QrTag, SHEET_MAX};

use crate::api;
use crate::app::Route;
use crate::components::crud::layout::ListLayout;
use crate::components::ui::{BTN_PRIMARY, BTN_SECONDARY, FIELD_HELP, INPUT};

/// The tokens of a sheet in its address, comma-separated.
fn sheet_route(tags: &[QrTag]) -> Route {
    Route::QrSheet {
        tokens: tags
            .iter()
            .map(|t| t.token.as_str())
            .collect::<Vec<_>>()
            .join(","),
    }
}

/// `/q/:token`: a linked tag opens its object, a blank one is linked here
/// to an object chosen by its title; another org's tag is unknown.
#[component]
pub fn QrScan(token: String) -> Element {
    let nav = use_navigator();
    let tok = use_signal(|| token.clone());
    let tag = use_resource(move || async move { api::qr::get(&tok()).await });
    // A linked tag goes straight to its object.
    use_effect(move || {
        if let Some(Ok(t)) = &*tag.read() {
            if let (QrState::Linked, Some(id)) = (t.state, t.object_id.clone()) {
                nav.replace(Route::OntologyObject { id });
            }
        }
    });
    let body = match &*tag.read() {
        None => rsx! {
            p { class: "text-slate", {t!("qr-resolving")} }
        },
        Some(Err(_)) => rsx! {
            p { class: "text-alarm", "data-testid": "qr-unknown", {t!("qr-unknown")} }
        },
        Some(Ok(t)) if t.state == QrState::Revoked => rsx! {
            p { class: "text-alarm", "data-testid": "qr-revoked", {t!("qr-revoked")} }
        },
        Some(Ok(t)) if t.state == QrState::Blank => rsx! {
            LinkBlank { token: t.token.clone() }
        },
        Some(Ok(_)) => rsx! {
            p { class: "text-slate", {t!("qr-resolving")} }
        },
    };
    rsx! {
        ListLayout { title: t!("qr-scan-title").to_string(), can_write: false, {body} }
    }
}

/// A blank tag scanned on site: find the object by its title, link it.
#[component]
fn LinkBlank(token: String) -> Element {
    let nav = use_navigator();
    let mut text = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let can_write = crate::state::access::can("qr-tags:write");
    let found = use_resource(move || async move {
        let q = text();
        if q.trim().chars().count() < 2 {
            return Ok(Vec::new());
        }
        api::ontology::query(&OntologyQuery {
            text: Some(q),
            limit: Some(10),
            ..Default::default()
        })
        .await
        .map(|r| r.rows)
    });
    let rows = match &*found.read() {
        Some(Ok(rows)) => rows.clone(),
        _ => Vec::new(),
    };
    rsx! {
        div { class: "max-w-xl space-y-3",
            p { class: "text-ink", "data-testid": "qr-blank", {t!("qr-blank")} }
            if can_write {
                input {
                    class: INPUT,
                    r#type: "search",
                    "data-testid": "qr-link-search",
                    placeholder: t!("qr-link-search").to_string(),
                    value: "{text}",
                    oninput: move |e| text.set(e.value()),
                }
                ul { class: "divide-y divide-line",
                    for row in rows {
                        li {
                            key: "{row.object.id}",
                            class: "flex items-center justify-between gap-3 py-2",
                            span { class: "min-w-0 break-words text-ink", "{row.object.title}" }
                            button {
                                r#type: "button",
                                class: BTN_SECONDARY,
                                disabled: busy(),
                                "data-testid": "qr-link-here",
                                onclick: {
                                    let (token, id) = (token.clone(), row.object.id.clone());
                                    move |_| {
                                        let (token, id) = (token.clone(), id.clone());
                                        busy.set(true);
                                        spawn(async move {
                                            match api::qr::link(&token, &id).await {
                                                Ok(_) => {
                                                    nav.replace(Route::OntologyObject { id });
                                                }
                                                Err(e) => crate::state::toasts::error(e),
                                            }
                                            busy.set(false);
                                        });
                                    }
                                },
                                {t!("qr-link-here")}
                            }
                        }
                    }
                }
            } else {
                p { class: FIELD_HELP, {t!("qr-link-no-right")} }
            }
        }
    }
}

/// The QR of an object on its page: made on first use, printed as a label.
#[component]
pub fn ObjectQr(object_id: String) -> Element {
    let nav = use_navigator();
    let mut tag = use_signal(|| None::<QrTag>);
    let mut busy = use_signal(|| false);
    if !crate::state::access::can("qr-tags:write") {
        return rsx! {};
    }
    rsx! {
        div {
            class: "flex flex-wrap items-center gap-3",
            "data-testid": "object-qr",
            if let Some(t) = tag() {
                if let Some(svg) = api::qr::svg(&api::qr::address(&t.token)) {
                    div {
                        class: "h-24 w-24 rounded border border-line bg-white p-1",
                        "data-testid": "object-qr-code",
                        dangerous_inner_html: "{svg}",
                    }
                }
                button {
                    r#type: "button",
                    class: BTN_SECONDARY,
                    onclick: move |_| {
                        if let Some(t) = tag() {
                            nav.push(sheet_route(&[t]));
                        }
                    },
                    {t!("qr-print")}
                }
            } else {
                button {
                    r#type: "button",
                    class: BTN_SECONDARY,
                    disabled: busy(),
                    "data-testid": "object-qr-show",
                    onclick: {
                        let id = object_id.clone();
                        move |_| {
                            let id = id.clone();
                            busy.set(true);
                            spawn(async move {
                                match api::qr::for_object(&id).await {
                                    Ok(t) => tag.set(Some(t)),
                                    Err(e) => crate::state::toasts::error(e),
                                }
                                busy.set(false);
                            });
                        }
                    },
                    {t!("qr-show")}
                }
            }
        }
    }
}

/// Makes a blank sheet and opens it to print.
#[component]
pub fn BlankSheetButton() -> Element {
    let nav = use_navigator();
    let mut count = use_signal(|| 40u32);
    let mut busy = use_signal(|| false);
    if !crate::state::access::can("qr-tags:write") {
        return rsx! {};
    }
    rsx! {
        div { class: "flex flex-nowrap items-center gap-2",
            label { class: "sr-only", r#for: "qr-sheet-count", {t!("qr-sheet-count")} }
            // The shared input fills its box: the box sets the width.
            div { class: "w-20 shrink-0",
                input {
                    id: "qr-sheet-count",
                    class: INPUT,
                    r#type: "number",
                    min: "1",
                    max: "{SHEET_MAX}",
                    value: "{count}",
                    oninput: move |e| count.set(e.value().parse().unwrap_or(40).clamp(1, SHEET_MAX)),
                }
            }
            button {
                r#type: "button",
                class: BTN_PRIMARY,
                disabled: busy(),
                "data-testid": "qr-sheet-blank",
                onclick: move |_| {
                    busy.set(true);
                    spawn(async move {
                        match api::qr::sheet(count()).await {
                            Ok(tags) => {
                                nav.push(sheet_route(&tags));
                            }
                            Err(e) => crate::state::toasts::error(e),
                        }
                        busy.set(false);
                    });
                },
                {t!("qr-sheet-blank")}
            }
        }
    }
}

/// `/qr-sheet?tokens=…`: labels to print, 4 × 10 per A4 page; only the
/// labels print.
#[component]
pub fn QrSheet(tokens: String) -> Element {
    let wanted: Vec<String> = tokens
        .split(',')
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect();
    let wanted = use_signal(|| wanted);
    let tags = use_resource(move || async move {
        let all = wanted();
        let mut out: Vec<QrTag> = Vec::new();
        for chunk in all.chunks(100) {
            out.extend(api::qr::list(None, None, chunk, 100, 0).await?.results);
        }
        // Printing order, the one of the address.
        out.sort_by_key(|t| all.iter().position(|x| *x == t.token));
        Ok::<_, crate::api::error::ApiError>(out)
    });
    let print = move |_| {
        document::eval("window.print()");
    };
    rsx! {
        div { class: "min-h-screen bg-white p-4 text-ink print:p-0",
            div { class: "mb-4 flex items-center gap-3 print:hidden",
                button {
                    r#type: "button",
                    class: BTN_PRIMARY,
                    onclick: print,
                    "data-testid": "qr-sheet-print",
                    {t!("qr-print")}
                }
                p { class: FIELD_HELP, {t!("qr-sheet-help")} }
            }
            match &*tags.read() {
                None => rsx! {
                    p { class: "text-slate", {t!("qr-resolving")} }
                },
                Some(Err(e)) => rsx! {
                    p { class: "text-alarm", {crate::api::error_i18n::localize(e)} }
                },
                Some(Ok(tags)) => rsx! {
                    div { class: "grid grid-cols-4 gap-2", "data-testid": "qr-sheet",
                        for t in tags.iter() {
                            div {
                                key: "{t.token}",
                                class: "flex break-inside-avoid flex-col items-center gap-1 border border-dashed border-line p-2",
                                "data-testid": "qr-label",
                                if let Some(svg) = api::qr::svg(&api::qr::address(&t.token)) {
                                    div {
                                        class: "aspect-square w-full max-w-28",
                                        dangerous_inner_html: "{svg}",
                                    }
                                }
                                span { class: "w-full truncate text-center text-xs",
                                    {t.object_title.clone().unwrap_or_else(|| t.token[..6].to_uppercase())}
                                }
                            }
                        }
                    }
                },
            }
        }
    }
}

fn state_label(s: QrState) -> String {
    match s {
        QrState::Blank => t!("qr-state-blank").to_string(),
        QrState::Linked => t!("qr-state-linked").to_string(),
        QrState::Revoked => t!("qr-state-revoked").to_string(),
    }
}

/// `/qr-tags`: the tags of the org, the blank sheet, revocation.
#[component]
pub fn QrTags() -> Element {
    use crate::components::crud::pager::{ListPager, PAGE_SIZE};
    use crate::components::crud::states::ListStates;
    use crate::components::crud::table::{Column, DataTable, RowKey};
    let mut reload = use_signal(|| 0u32);
    let page = use_signal(|| 0i64);
    let list = use_resource(move || async move {
        let _ = reload();
        api::qr::list(None, None, &[], PAGE_SIZE, page() * PAGE_SIZE).await
    });
    let (state, count, rows) = match &*list.value().read() {
        None => (None, 0, Vec::new()),
        Some(Ok(p)) => (Some(Ok(())), p.count, p.results.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), 0, Vec::new()),
    };
    let can_write = crate::state::access::can("qr-tags:write");
    let nav = use_navigator();
    let columns = vec![
        Column::new(t!("qr-col-label").to_string(), |q: &QrTag| {
            let short = q.token[..6].to_uppercase();
            rsx! {
                span { class: "font-mono text-sm text-ink", "{short}" }
            }
        }),
        Column::new(t!("qr-col-state").to_string(), |q: &QrTag| {
            let dot = match q.state {
                QrState::Blank => "bg-idle",
                QrState::Linked => "bg-ok",
                QrState::Revoked => "bg-alarm",
            };
            rsx! {
                span { class: "inline-flex items-center gap-1.5 whitespace-nowrap",
                    span { class: "h-2 w-2 shrink-0 rounded-full {dot}", aria_hidden: "true" }
                    {state_label(q.state)}
                }
            }
        }),
        Column::new(t!("qr-col-object").to_string(), |q: &QrTag| {
            let title = q.object_title.clone().unwrap_or_default();
            rsx! {
                span { class: "break-words", "{title}" }
            }
        }),
        Column::new(String::new(), move |q: &QrTag| {
            let token = q.token.clone();
            let live = q.state != QrState::Revoked;
            rsx! {
                if can_write && live {
                    button {
                        r#type: "button",
                        class: "text-sm text-alarm hover:underline",
                        "data-testid": "qr-revoke",
                        onclick: move |e: MouseEvent| {
                            e.stop_propagation();
                            let token = token.clone();
                            spawn(async move {
                                match api::qr::revoke(&token).await {
                                    Ok(_) => reload.with_mut(|r| *r += 1),
                                    Err(e) => crate::state::toasts::error(e),
                                }
                            });
                        },
                        {t!("qr-revoke")}
                    }
                }
            }
        }),
    ];
    rsx! {
        ListLayout {
            title: t!("qr-tags-title").to_string(),
            subtitle: Some(t!("qr-tags-subtitle").to_string()),
            can_write,
            actions: rsx! {
                BlankSheetButton {}
            },
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            ListStates {
                state,
                is_empty: count == 0,
                empty_message: t!("qr-tags-empty").to_string(),
                div { class: "space-y-4",
                    DataTable {
                        columns,
                        rows,
                        row_key: RowKey::new(|q: &QrTag| q.token.clone()),
                        on_row_click: move |token: String| {
                            nav.push(Route::QrScan { token });
                        },
                    }
                    ListPager { count, page }
                }
            }
        }
    }
}
