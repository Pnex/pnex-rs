//! Mobile composer (D124): the canvas slot of a mobile dashboard. A
//! phone-width stack of sections holding cards on 2 columns; cards are
//! reordered by drag and drop (desktop) or by the arrows of the selected
//! card (touch). Palette additions land in the target section (the last
//! one clicked).

use super::*;

use crate::components::dashboard_live::mobile_card_classes;
use crate::components::dashboard_widget::WidgetBody;
use pnex_core::{MobileSection, Widget};

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
    let count = layout.sections.len();
    let sections: Vec<(usize, MobileSection, Vec<Widget>)> = layout
        .sections
        .iter()
        .enumerate()
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

    rsx! {
        div {
            class: "absolute inset-0 overflow-auto bg-gray-100",
            onpointerup: move |_| drop_template(cx, None),
            onclick: move |_| cx.selected.set(None),
            div { class: "mx-auto w-full max-w-md space-y-5 px-4 pb-24 pt-20",
                for (index, section, cards) in sections {
                    SectionBlock {
                        key: "{section.id}",
                        cx,
                        section: section.clone(),
                        cards,
                        index,
                        count,
                    }
                }
                button {
                    r#type: "button",
                    class: "w-full rounded-xl border-2 border-dashed border-gray-300 py-3 text-sm text-gray-500 hover:border-blue-300 hover:text-blue-600",
                    onclick: move |e| {
                        e.stop_propagation();
                        let title = new_title.clone();
                        let mut added = String::new();
                        mutate(cx, |l| added = state::add_section(l, &title));
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
