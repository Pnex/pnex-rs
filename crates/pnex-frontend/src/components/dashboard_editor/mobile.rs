//! Mobile composer (D124): the canvas slot of a mobile dashboard. A
//! phone-width stack of sections holding cards on 2 columns; cards are
//! reordered by drag and drop (desktop) or by the arrows of the selected
//! card (touch). Palette additions land in the target section (the last
//! one clicked).

use super::*;

use crate::components::dashboard_live_mobile::mobile_card_classes;
use crate::components::dashboard_widget::WidgetBody;
use crate::components::home_icons::{HomeIconPicker, HomeIconView};
use pnex_core::{MobileSection, SectionStyle, Widget};

/// Applies one undoable mutation of the layout.
fn mutate(mut cx: EditorCx, f: impl FnOnce(&mut DashboardLayout)) {
    let current = cx.layout.read().clone();
    cx.history.with_mut(|h| h.push(&current));
    cx.layout.with_mut(f);
}

/// Drops the template dragged from the library into `section`.
fn drop_template(mut cx: EditorCx, section: Option<String>) {
    let Some(tpl) = cx.drag_template.cloned() else {
        return;
    };
    cx.drag_template.set(None);
    cx.counter.with_mut(|c| *c += 1);
    let id = state::next_id("w", cx.counter.cloned());
    let template = tpl.config.clone();
    let card = id.clone();
    mutate(cx, move |l| {
        state::place_widget(l, &template, card.clone(), 0, 0);
        state::place_in_section(l, &card, section.as_deref());
    });
    cx.selected.set(Some(Selection::Widget(id)));
}

#[component]
pub(super) fn MobileComposer(mut cx: EditorCx) -> Element {
    let layout = cx.layout.read().clone();
    // Page shown in the composer (D139); `None` = the first page.
    let mut page: Signal<Option<String>> = use_signal(|| None);
    let current_page = page()
        .filter(|p| layout.pages.iter().any(|x| &x.id == p))
        .or_else(|| layout.pages.first().map(|p| p.id.clone()));
    let on_page: Vec<String> = state::sections_of_page(&layout, current_page.as_deref())
        .iter()
        .map(|s| s.id.clone())
        .collect();
    let count = layout.sections.len();
    let sections: Vec<(usize, MobileSection, Vec<Widget>)> = layout
        .sections
        .iter()
        .enumerate()
        .filter(|(_, sec)| on_page.contains(&sec.id))
        .map(|(i, sec)| {
            let cards = sec
                .items
                .iter()
                .filter_map(|id| layout.widgets.iter().find(|w| &w.id == id).cloned())
                .collect();
            (i, sec.clone(), cards)
        })
        .collect();
    let new_title = t!("db-section-new").to_string();
    let page_for_add = current_page.clone();
    let pages = layout.pages.clone();
    let first_page_title = t!("db-page-home").to_string();
    let new_page_title = t!("db-page-new").to_string();

    rsx! {
        div {
            class: "absolute inset-0 overflow-auto bg-gray-100",
            onpointerup: move |_| drop_template(cx, None),
            onclick: move |_| cx.selected.set(None),
            div { class: "mx-auto w-full max-w-md space-y-5 px-4 pb-24 pt-20",
                PageBar {
                    cx,
                    pages: pages.clone(),
                    current: current_page.clone(),
                    on_pick: move |id: String| {
                        page.set(Some(id));
                        cx.section.set(None);
                    },
                    on_add: move |_| {
                        let (first, title) = (first_page_title.clone(), new_page_title.clone());
                        let mut added = (String::new(), String::new());
                        mutate(cx, |l| added = state::add_page(l, &first, &title));
                        page.set(Some(added.0));
                        cx.section.set(Some(added.1));
                    },
                }
                for (index, section, cards) in sections {
                    SectionBlock {
                        key: "{section.id}",
                        cx,
                        section: section.clone(),
                        cards,
                        index,
                        count,
                        pages: pages.clone(),
                    }
                }
                button {
                    r#type: "button",
                    class: "w-full rounded-xl border-2 border-dashed border-gray-300 py-3 text-sm text-gray-500 hover:border-blue-300 hover:text-blue-600",
                    onclick: move |e| {
                        e.stop_propagation();
                        let title = new_title.clone();
                        let on = page_for_add.clone();
                        let mut added = String::new();
                        mutate(cx, |l| added = state::add_section_on(l, &title, on.as_deref()));
                        cx.section.set(Some(added));
                    },
                    {t!("db-section-add")}
                }
            }
        }
    }
}

#[component]
fn SectionBlock(
    mut cx: EditorCx,
    section: MobileSection,
    cards: Vec<Widget>,
    index: usize,
    count: usize,
    pages: Vec<pnex_core::MobilePage>,
) -> Element {
    let sid = section.id.clone();
    let target = cx.section.cloned().as_deref() == Some(sid.as_str())
        || (cx.section.cloned().is_none() && index + 1 == count);
    let frame = if target {
        "rounded-xl p-2 ring-2 ring-blue-200 bg-blue-50/40"
    } else {
        "rounded-xl p-2"
    };
    let (s_title, s_drop, s_up, s_down, s_del, s_click) = (
        sid.clone(),
        sid.clone(),
        sid.clone(),
        sid.clone(),
        sid.clone(),
        sid.clone(),
    );
    let s_tpl = sid.clone();
    let empty = cards.is_empty();

    rsx! {
        section {
            class: "{frame} space-y-2",
            onclick: move |e| {
                e.stop_propagation();
                cx.section.set(Some(s_click.clone()));
            },
            onpointerup: move |e| {
                if cx.drag_template.peek().is_some() {
                    e.stop_propagation();
                    drop_template(cx, Some(s_tpl.clone()));
                }
            },
            ondragover: move |e| e.prevent_default(),
            ondrop: move |e| {
                e.prevent_default();
                let card = cx.drag_card.cloned();
                cx.drag_card.set(None);
                if let Some(card) = card {
                    let to = s_drop.clone();
                    mutate(cx, move |l| state::move_card(l, &card, &to, usize::MAX));
                }
            },
            div { class: "flex items-center gap-1",
                input {
                    class: "min-w-0 flex-1 rounded bg-transparent px-1 py-0.5 text-xs font-semibold uppercase tracking-wide text-gray-600 hover:bg-white focus:bg-white",
                    value: "{section.title}",
                    placeholder: t!("db-section-title").to_string(),
                    onchange: move |e| {
                        let title = e.value();
                        let id = s_title.clone();
                        mutate(cx, move |l| state::rename_section(l, &id, &title));
                    },
                }
                SectionButton {
                    disabled: index == 0,
                    title: t!("db-section-up").to_string(),
                    on_click: move |_| {
                        let id = s_up.clone();
                        mutate(cx, move |l| state::shift_section(l, &id, -1));
                    },
                    icons::ChevronUp { class: "h-3.5 w-3.5" }
                }
                SectionButton {
                    disabled: index + 1 >= count,
                    title: t!("db-section-down").to_string(),
                    on_click: move |_| {
                        let id = s_down.clone();
                        mutate(cx, move |l| state::shift_section(l, &id, 1));
                    },
                    icons::ChevronDown { class: "h-3.5 w-3.5" }
                }
                SectionButton {
                    disabled: count < 2,
                    title: t!("db-section-delete").to_string(),
                    on_click: move |_| {
                        let id = s_del.clone();
                        mutate(cx, move |l| state::delete_section(l, &id));
                        cx.section.set(None);
                    },
                    icons::Trash { class: "h-3.5 w-3.5" }
                }
            }
            SectionOptions { cx, section: section.clone(), pages }
            if empty {
                p { class: "rounded-lg border border-dashed border-gray-300 px-3 py-6 text-center text-xs text-gray-400",
                    {t!("db-section-empty")}
                }
            }
            div { class: "grid grid-cols-2 gap-3",
                for w in cards {
                    ComposerCard {
                        key: "{w.id}",
                        cx,
                        w: w.clone(),
                        section_id: sid.clone(),
                    }
                }
            }
        }
    }
}

#[component]
fn SectionButton(
    disabled: bool,
    title: String,
    on_click: EventHandler<()>,
    children: Element,
) -> Element {
    rsx! {
        button {
            r#type: "button",
            class: "rounded p-1 text-gray-400 hover:bg-white hover:text-gray-700 disabled:opacity-30",
            disabled,
            title: "{title}",
            onclick: move |e| {
                e.stop_propagation();
                on_click.call(());
            },
            {children}
        }
    }
}

#[component]
fn ComposerCard(mut cx: EditorCx, w: Widget, section_id: String) -> Element {
    let id = w.id.clone();
    let selected = matches!(cx.selected.cloned(), Some(Selection::Widget(ref s)) if *s == id);
    let classes = mobile_card_classes(&w);
    let ring = if selected {
        "ring-2 ring-blue-500"
    } else {
        "hover:ring-2 hover:ring-blue-200"
    };
    let points = w
        .source
        .first()
        .and_then(|s| cx.live.read().get(&s.series_key()).cloned())
        .flatten();
    let live = cx.live.read().clone();
    let half = w.options.span == Some(1);
    let (id_drag, id_drop, id_click, id_back, id_fwd, id_span) = (
        id.clone(),
        id.clone(),
        id.clone(),
        id.clone(),
        id.clone(),
        id.clone(),
    );

    rsx! {
        div {
            class: "{classes} relative cursor-pointer rounded-xl border border-gray-200 bg-white shadow-sm {ring}",
            draggable: "true",
            ondragstart: move |_| cx.drag_card.set(Some(id_drag.clone())),
            ondragend: move |_| cx.drag_card.set(None),
            ondragover: move |e| e.prevent_default(),
            ondrop: move |e| {
                e.prevent_default();
                e.stop_propagation();
                let card = cx.drag_card.cloned();
                cx.drag_card.set(None);
                if let Some(card) = card {
                    let before = id_drop.clone();
                    mutate(cx, move |l| state::move_card_before(l, &card, &before));
                }
            },
            onclick: move |e| {
                e.stop_propagation();
                cx.section.set(Some(section_id.clone()));
                cx.selected.set(Some(Selection::Widget(id_click.clone())));
            },
            div { class: "pointer-events-none h-full w-full overflow-hidden rounded-xl",
                WidgetBody { widget: w.clone(), points, values: Some(live) }
            }
            if selected {
                div { class: "absolute -top-3 right-2 flex items-center gap-0.5 rounded-full border border-gray-200 bg-white px-1 shadow",
                    SectionButton {
                        disabled: false,
                        title: t!("db-card-back").to_string(),
                        on_click: move |_| {
                            let id = id_back.clone();
                            mutate(cx, move |l| state::shift_card(l, &id, -1));
                        },
                        icons::ChevronLeft { class: "h-3.5 w-3.5" }
                    }
                    SectionButton {
                        disabled: false,
                        title: t!("db-card-forward").to_string(),
                        on_click: move |_| {
                            let id = id_fwd.clone();
                            mutate(cx, move |l| state::shift_card(l, &id, 1));
                        },
                        icons::ChevronRight { class: "h-3.5 w-3.5" }
                    }
                    SectionButton {
                        disabled: false,
                        title: if half { t!("db-card-full").to_string() } else { t!("db-card-half").to_string() },
                        on_click: move |_| {
                            let id = id_span.clone();
                            let span = if half { 2 } else { 1 };
                            mutate(cx, move |l| state::set_span(l, &id, span));
                        },
                        if half {
                            icons::WidthFull { class: "h-3.5 w-3.5" }
                        } else {
                            icons::WidthHalf { class: "h-3.5 w-3.5" }
                        }
                    }
                }
            }
        }
    }
}

/// Page tabs of the composer (D139): pick, add; the current page can be
/// renamed, given an icon or deleted.
#[component]
fn PageBar(
    cx: EditorCx,
    pages: Vec<pnex_core::MobilePage>,
    current: Option<String>,
    on_pick: EventHandler<String>,
    on_add: EventHandler<()>,
) -> Element {
    let selected = pages
        .iter()
        .find(|p| Some(&p.id) == current.as_ref())
        .cloned();
    let tab_on = "flex shrink-0 items-center gap-1 rounded-full bg-blue-600 px-3 py-1 text-xs font-medium text-white";
    let tab_off = "flex shrink-0 items-center gap-1 rounded-full bg-white px-3 py-1 text-xs font-medium text-gray-600 hover:bg-gray-50";
    rsx! {
        div { class: "space-y-2", onclick: move |e| e.stop_propagation(),
            div { class: "flex items-center gap-1 overflow-x-auto pb-1",
                for p in pages.iter().cloned() {
                    button {
                        key: "{p.id}",
                        r#type: "button",
                        class: if current.as_ref() == Some(&p.id) { tab_on } else { tab_off },
                        onclick: move |_| on_pick.call(p.id.clone()),
                        if let Some(icon) = p.icon.clone() {
                            HomeIconView { id: icon, class: "h-3.5 w-3.5" }
                        }
                        "{p.title}"
                    }
                }
                button {
                    r#type: "button",
                    class: "shrink-0 rounded-full border border-dashed border-gray-300 px-3 py-1 text-xs text-gray-500 hover:border-blue-300 hover:text-blue-600",
                    onclick: move |_| on_add.call(()),
                    {t!("db-page-add")}
                }
            }
            if let Some(p) = selected {
                PageEditor { key: "{p.id}", cx, page: p }
            }
        }
    }
}

#[component]
fn PageEditor(cx: EditorCx, page: pnex_core::MobilePage) -> Element {
    let (id_title, id_icon, id_del) = (page.id.clone(), page.id.clone(), page.id.clone());
    rsx! {
        div { class: "flex items-center gap-2 rounded-lg bg-white px-2 py-1 shadow-sm",
            HomeIconPicker {
                value: page.icon.clone(),
                disabled: false,
                on_change: move |icon: Option<String>| {
                    let id = id_icon.clone();
                    mutate(cx, move |l| state::set_page_icon(l, &id, icon));
                },
            }
            input {
                class: "min-w-0 flex-1 rounded px-1 py-0.5 text-sm",
                value: "{page.title}",
                placeholder: t!("db-page-title").to_string(),
                maxlength: "40",
                onchange: move |e| {
                    let title = e.value();
                    let id = id_title.clone();
                    mutate(cx, move |l| state::rename_page(l, &id, &title));
                },
            }
            SectionButton {
                disabled: false,
                title: t!("db-page-delete").to_string(),
                on_click: move |_| {
                    let id = id_del.clone();
                    mutate(cx, move |l| state::delete_page(l, &id));
                },
                icons::Trash { class: "h-3.5 w-3.5" }
            }
        }
    }
}

/// Style, icon and page of a section (D139).
#[component]
fn SectionOptions(
    cx: EditorCx,
    section: MobileSection,
    pages: Vec<pnex_core::MobilePage>,
) -> Element {
    let (id_style, id_icon, id_page) = (section.id.clone(), section.id.clone(), section.id.clone());
    let style = match section.style {
        SectionStyle::Cards => "cards",
        SectionStyle::Chips => "chips",
        SectionStyle::Room => "room",
    };
    let current_page = section
        .page
        .clone()
        .or_else(|| pages.first().map(|p| p.id.clone()))
        .unwrap_or_default();
    rsx! {
        div {
            class: "flex flex-wrap items-center gap-1",
            onclick: move |e| e.stop_propagation(),
            HomeIconPicker {
                value: section.icon.clone(),
                disabled: false,
                on_change: move |icon: Option<String>| {
                    let id = id_icon.clone();
                    mutate(cx, move |l| state::set_section_icon(l, &id, icon));
                },
            }
            select {
                class: "rounded border border-gray-300 bg-white px-1 py-0.5 text-xs",
                value: "{style}",
                onchange: move |e| {
                    let next = match e.value().as_str() {
                        "chips" => SectionStyle::Chips,
                        "room" => SectionStyle::Room,
                        _ => SectionStyle::Cards,
                    };
                    let id = id_style.clone();
                    mutate(cx, move |l| state::set_section_style(l, &id, next));
                },
                option { value: "cards", {t!("db-section-style-cards")} }
                option { value: "room", {t!("db-section-style-room")} }
                option { value: "chips", {t!("db-section-style-chips")} }
            }
            if pages.len() > 1 {
                select {
                    class: "rounded border border-gray-300 bg-white px-1 py-0.5 text-xs",
                    value: "{current_page}",
                    title: t!("db-section-page").to_string(),
                    onchange: move |e| {
                        let to = e.value();
                        let id = id_page.clone();
                        mutate(cx, move |l| state::set_section_page(l, &id, Some(&to)));
                    },
                    for p in pages.iter().cloned() {
                        option { key: "{p.id}", value: "{p.id}", "{p.title}" }
                    }
                }
            }
        }
    }
}
