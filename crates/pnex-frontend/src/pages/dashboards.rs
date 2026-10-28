//! **Dashboards** page (SCADA studio, D24/D34/D40/D41):
//! D14 list → subviews driven by local signals (`pages/flows.rs`
//! approach) — **live** (rendering of the current version, a single
//! 15 s polling via `series-batch`, per-item degradation) and **edit**
//! (`components/dashboard_editor`, polling suspended).

use std::collections::HashMap;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{DashboardFormat, TelemetryPoint, VizDashboard, VizDashboardSummary};

use crate::api;
use crate::components::badges::date_label;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::filters::{FilterBar, LabelFilterInput, SearchInput};
use crate::components::crud::layout::ListLayout;
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::dashboard_editor::state::HomeTemplate;
use crate::components::dashboard_editor::DashboardEditor;
use crate::components::dashboard_live::live_layout;
use crate::components::icons;
use crate::components::labels_editor::{use_row_labels, LabelChips};
use crate::components::modal::Modal;
use crate::components::refresh_rate::{use_auto_refresh, RefreshRateControl};
use crate::components::ui::{
    BTN_DANGER, BTN_PRIMARY, BTN_PRIMARY_SM, BTN_SECONDARY, BTN_SECONDARY_SM,
};
use crate::components::ui::{FIELD_LABEL, INPUT};
use crate::state::viz::{OPEN_DASHBOARD, VIZ_EDIT};
use crate::state::{org, session, toasts};

/// Live refresh rate (same choice as dashboard/
/// visualisation: see the data arrive without hammering O2).
const POLL_SECS: u64 = 15;

fn session_absent() -> bool {
    session::user().is_none()
}

#[component]
pub fn Dashboards(id: String, mode: String) -> Element {
    if session_absent() {
        return rsx! {};
    }

    let mut selected: Signal<Option<String>> = use_signal(|| None);
    let mut editing = use_signal(|| false);
    let mut reload = use_signal(|| 0u32);

    // ── `?id&:mode` deep link (dioxus #2784: route props ≠ signals,
    // we drop them into the global signals at mount). A viewer that
    // forces mode=edit falls back to live (D34) — the editor forbids it again
    // anyway.
    //
    // `seeded` guard: the effect consumes the deep link ONLY ONCE. Without
    // it, each run rewrites OPEN_DASHBOARD from the route props
    // (fixed) → re-arms the effect → infinite loop (main thread blocked,
    // "Page Unresponsive" — observed 2026-09-13, deep link from the
    // POI drawer). The mid-run writes to OPEN_DASHBOARD/VIZ_EDIT
    // schedule a 2nd run: it must be a no-op.
    let mut deep_seeded = use_signal(|| false);
    use_effect(move || {
        if deep_seeded() {
            return;
        }
        deep_seeded.set(true);
        let deep_id = if id.is_empty() {
            None
        } else {
            Some(id.clone())
        };
        crate::state::viz::open_dashboard(deep_id, mode == "edit");
        let open_id = OPEN_DASHBOARD.cloned();
        if let Some(open_id) = open_id {
            let can_edit = VIZ_EDIT.cloned() && crate::state::access::can("dashboards:write");
            selected.set(Some(open_id));
            editing.set(can_edit);
            // GlobalSignal: no `&mut` static — mutation via with_mut
            // (intrinsic method, project memory).
            OPEN_DASHBOARD.with_mut(|v| *v = None);
            VIZ_EDIT.with_mut(|v| *v = false);
        }
    });

    let can_write = crate::state::access::can("dashboards:write");

    match selected() {
        Some(dashboard_id) => {
            if editing() && can_write {
                rsx! {
                    EditorSubView {
                        key: "{dashboard_id}",
                        dashboard_id: dashboard_id.clone(),
                        on_back: move |_| {
                            editing.set(false);
                            reload.with_mut(|r| *r += 1);
                        },
                    }
                }
            } else {
                rsx! {
                    LiveSubView {
                        key: "{dashboard_id}",
                        dashboard_id: dashboard_id.clone(),
                        can_write,
                        on_edit: move |_| editing.set(true),
                        on_back: move |_| {
                            selected.set(None);
                            reload.with_mut(|r| *r += 1);
                        },
                    }
                }
            }
        }
        None => rsx! {
            ListView {
                reload,
                can_write,
                on_open: move |open: (String, bool)| {
                    selected.set(Some(open.0));
                    editing.set(open.1);
                },
            }
        },
    }
}

// ─────────────────────────── List ───────────────────────────

#[component]
fn ListView(
    mut reload: Signal<u32>,
    can_write: bool,
    on_open: EventHandler<(String, bool)>,
) -> Element {
    let mut page = use_signal(|| 0i64);
    // D218: own label filter ("name" or "name:value") and name search.
    let filter_label = use_signal(String::new);
    let filter_search = use_signal(String::new);
    let mut delete_target = use_signal(|| None::<(String, String)>);
    let mut choosing = use_signal(|| false);

    // Org, reload and page read in the SYNCHRONOUS part of the closure
    // (devices.rs pattern: guaranteed subscriptions). FIX: `page()` was
    // not read and no offset was passed — the Pager's pagination was
    // wired but inert (counter changing without refetch).
    let list = use_resource(move || {
        let offset = page() * PAGE_SIZE;
        let label = {
            let value = filter_label().trim().to_string();
            (!value.is_empty()).then_some(value)
        };
        let search = Some(filter_search()).filter(|v| !v.trim().is_empty());
        let has_org = org::current().is_some();
        async move {
            let _ = reload();
            if !has_org {
                return None;
            }
            Some(
                api::dashboards::list(&api::dashboards::DashboardFilters {
                    search,
                    label,
                    limit: Some(PAGE_SIZE),
                    offset: Some(offset),
                })
                .await,
            )
        }
    });

    // D42: own labels of the page rows, one batch per list (re)load.
    let row_labels = use_row_labels(pnex_core::resources::KIND_DASHBOARD, move || {
        match &*list.value().read() {
            Some(Some(Ok(paged))) => paged.results.iter().map(|d| d.id.clone()).collect(),
            _ => Vec::new(),
        }
    });

    // Synchronous read of the resource (CRUD foundation doctrine). Some(None) =
    // no org (guard rendered in the rsx) → treated as loading.
    let (list_state, is_empty, count, rows) = match &*list.value().read() {
        None | Some(None) => (None, false, 0, Vec::new()),
        Some(Some(Ok(paged))) => (
            Some(Ok(())),
            paged.results.is_empty() && paged.count == 0,
            paged.count,
            paged.results.clone(),
        ),
        Some(Some(Err(err))) => (Some(Err(err.clone())), false, 0, Vec::new()),
    };

    // Table columns — the action closures capture on_open and the
    // delete target; the style of the old raw th/td moves to the
    // foundation tokens (th.th / td.td).
    let columns = vec![
        Column::new(t!("db-name").to_string(), move |d: &VizDashboardSummary| {
            let labels = row_labels.get(&d.id).cloned().unwrap_or_default();
            rsx! {
                div { class: "text-sm font-medium text-gray-900", "{d.name}" }
                LabelChips { labels }
                if let Some(desc) = &d.description {
                    div { class: "text-xs text-gray-500", "{desc}" }
                }
                div { class: "text-xs text-gray-500", "{date_label(&d.updated_at)}" }
            }
        }),
        Column::new(t!("db-format").to_string(), |d: &VizDashboardSummary| {
            let (label, class) = match d.format {
                DashboardFormat::Desktop => (t!("db-format-desktop"), "bg-gray-100 text-gray-700"),
                DashboardFormat::Mobile => (t!("db-format-mobile"), "bg-teal-50 text-teal-700"),
            };
            rsx! {
                span { class: "inline-flex items-center px-2 py-0.5 rounded text-xs font-medium {class}",
                    "{label}"
                }
            }
        }).secondary(),
        Column::new(t!("db-version").to_string(), |d: &VizDashboardSummary| {
            let version_label = t!("db-current-version", version: d.current_version_number).to_string();
            rsx! {
                span { class: "inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-green-50 text-green-700",
                    "{version_label}"
                }
            }
        }),
        Column::new(
            String::new(),
            move |d: &VizDashboardSummary| {
                // One cloned id per button: each `move` closure consumes its own.
                let id_open = d.id.clone();
                let id_edit = d.id.clone();
                let id_delete = d.id.clone();
                let name_delete = d.name.clone();
                rsx! {
                    // One flex row, same height for the three buttons (the
                    // delete button used to sit lower than its neighbours).
                    div { class: "flex items-center justify-end gap-2",
                        button {
                            class: "{BTN_SECONDARY_SM} h-8",
                            title: t!("db-open"),
                            aria_label: t!("db-open"),
                            onclick: move |_| on_open.call((id_open.clone(), false)),
                            icons::Eye { class: "h-4 w-4 mr-1" }
                            {t!("db-open")}
                        }
                        if can_write {
                            button {
                                class: "{BTN_PRIMARY_SM} h-8",
                                title: t!("db-edit"),
                                aria_label: t!("db-edit"),
                                onclick: move |_| on_open.call((id_edit.clone(), true)),
                                icons::Wrench { class: "h-4 w-4 sm:mr-1" }
                                span { class: "hidden sm:inline", {t!("db-edit")} }
                            }
                            button {
                                class: "{BTN_DANGER} h-8",
                                title: t!("viz-delete"),
                                aria_label: t!("viz-delete"),
                                onclick: move |_| delete_target.set(Some((id_delete.clone(), name_delete.clone()))),
                                icons::Trash2 { class: "h-4 w-4 sm:mr-1" }
                                span { class: "hidden sm:inline", {t!("viz-delete")} }
                            }
                        }
                    }
                }
            },
        )
        .with_td_class("text-right whitespace-nowrap").actions(),
    ];

    rsx! {
        ListLayout {
            title: t!("nav-dashboards").to_string(),
            subtitle: Some(t!("db-subtitle").to_string()),
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            can_write,
            // Socle harmonisation: manual refresh lives in the header; the
            // filter bar only holds the D42 label filter.
            add_label: Some(t!("db-create").to_string()),
            on_add: move |_| choosing.set(true),
            // D123: the format is picked once, at creation — the modal
            // explains both formats and how their controls reach the flows.
            if choosing() {
                NewDashboardModal {
                    on_close: move |_| choosing.set(false),
                    on_create: move |pick: (String, DashboardFormat, Option<HomeTemplate>)| {
                        choosing.set(false);
                        create_dashboard(pick.0, pick.1, pick.2, reload, on_open);
                    },
                }
            }
            if org::current().is_none() {
                p { class: "text-gray-500 text-center py-12", {t!("orgs-empty")} }
            } else {
                FilterBar {
                    SearchInput {
                        placeholder: t!("common-search-name").to_string(),
                        value: filter_search,
                        on_submit: move |_| {
                            page.set(0);
                            reload.with_mut(|r| *r += 1);
                        },
                    }
                    LabelFilterInput {
                        placeholder: t!("resources-label-filter-placeholder").to_string(),
                        value: filter_label,
                        on_submit: move |_| {
                            page.set(0);
                            reload.with_mut(|r| *r += 1);
                        },
                    }
                }
                ListStates {
                    state: list_state,
                    is_empty,
                    empty_message: t!("db-list-empty").to_string(),
                    div { class: "relative",
                        DataTable {
                            columns,
                            rows,
                            row_key: RowKey::new(|d: &VizDashboardSummary| d.id.clone()),
                        }
                        ListPager { count, page }
                    }
                }
            }

            if let Some((id, name)) = delete_target() {
                ConfirmDialog {
                    title: t!("db-delete").to_string(),
                    message: format!("{} — {}", name, t!("db-delete-confirm")),
                    confirm_label: t!("common-delete").to_string(),
                    on_confirm: move |_| {
                        let id = id.clone();
                        spawn(async move {
                            match api::dashboards::delete(&id).await {
                                Ok(()) => {
                                    toasts::success(t!("db-deleted").to_string());
                                    reload.with_mut(|r| *r += 1);
                                }
                                Err(e) => toasts::error(e),
                            }
                            delete_target.set(None);
                        });
                    },
                    on_cancel: move |_| delete_target.set(None),
                }
            }
        }
    }
}

/// Creates a dashboard of `format` and opens it in the editor (fluidity —
/// user feedback 2026-09-18). An empty name falls back to a dated one.
fn create_dashboard(
    name: String,
    format: DashboardFormat,
    template: Option<HomeTemplate>,
    mut reload: Signal<u32>,
    on_open: EventHandler<(String, bool)>,
) {
    let layout = match template {
        // Template titles resolved now (render-scope i18n, D141).
        Some(t) => crate::components::dashboard_editor::state::template_layout(t, |k| {
            dioxus_i18n::prelude::i18n()
                .try_translate(k)
                .unwrap_or_else(|_| k.to_string())
        }),
        None => crate::components::dashboard_editor::state::initial_layout(
            format,
            &t!("db-section-default"),
        ),
    };
    let params = pnex_core::CreateDashboard {
        name: if name.trim().is_empty() {
            crate::util::plain_text(t!("db-default-name", date : crate ::util::now_label()))
        } else {
            name.trim().to_string()
        },
        description: None,
        layout: Some(layout),
    };
    let created = t!("db-created").to_string();
    spawn(async move {
        match api::dashboards::create(params).await {
            Ok(dashboard) => {
                toasts::success(created);
                reload.with_mut(|r| *r += 1);
                on_open.call((dashboard.id, true));
            }
            Err(e) => toasts::error(e),
        }
    });
}

/// "+ New dashboard" modal: name, then one of the two formats, each with
/// a sketch and its typical uses; the format is final (D123).
#[component]
fn NewDashboardModal(
    on_close: EventHandler<()>,
    on_create: EventHandler<(String, DashboardFormat, Option<HomeTemplate>)>,
) -> Element {
    let mut name = use_signal(String::new);
    let mut format = use_signal(|| None::<DashboardFormat>);
    let mut template = use_signal(|| None::<HomeTemplate>);
    let placeholder =
        crate::util::plain_text(t!("db-default-name", date : crate ::util::now_label()));
    let picked = format();
    rsx! {
        Modal {
            title: t!("db-new-title").to_string(),
            max_width: "max-w-3xl".to_string(),
            on_close: move |_| on_close.call(()),
            div { class: "space-y-5",
                label { class: FIELD_LABEL,
                    {t!("db-new-name")}
                    input {
                        class: "{INPUT} mt-1 w-full",
                        r#type: "text",
                        placeholder: "{placeholder}",
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                    }
                }
                div {
                    p { class: "mb-2 text-sm font-medium text-gray-700", {t!("db-new-pick")} }
                    div { class: "grid gap-3 sm:grid-cols-2",
                        FormatCard {
                            mobile: false,
                            selected: picked == Some(DashboardFormat::Desktop),
                            on_pick: move |_| format.set(Some(DashboardFormat::Desktop)),
                        }
                        FormatCard {
                            mobile: true,
                            selected: picked == Some(DashboardFormat::Mobile),
                            on_pick: move |_| format.set(Some(DashboardFormat::Mobile)),
                        }
                    }
                }
                if picked == Some(DashboardFormat::Mobile) {
                    TemplatePicker {
                        selected: template(),
                        on_pick: move |t: Option<HomeTemplate>| template.set(t),
                    }
                }
                div { class: "flex gap-2 rounded-lg bg-teal-50 px-3 py-2 text-xs text-teal-900",
                    icons::Info { class: "h-4 w-4 shrink-0" }
                    p { {t!("db-new-controls-hint")} }
                }
                p { class: "text-xs text-gray-500", {t!("db-new-final")} }
                div { class: "flex justify-end gap-2",
                    button {
                        class: BTN_SECONDARY,
                        r#type: "button",
                        onclick: move |_| on_close.call(()),
                        {t!("common-cancel")}
                    }
                    button {
                        class: BTN_PRIMARY,
                        r#type: "button",
                        disabled: picked.is_none(),
                        onclick: move |_| {
                            if let Some(f) = format() {
                                let tpl = template().filter(|_| f == DashboardFormat::Mobile);
                                on_create.call((name(), f, tpl));
                            }
                        },
                        {t!("db-new-create")}
                    }
                }
            }
        }
    }
}

/// Mobile starting point (D141): empty, or a ready-made home dashboard.
#[component]
fn TemplatePicker(
    selected: Option<HomeTemplate>,
    on_pick: EventHandler<Option<HomeTemplate>>,
) -> Element {
    let options: Vec<(Option<HomeTemplate>, String, &'static str)> = vec![
        (None, t!("db-tpl-empty").to_string(), "home-check"),
        (
            Some(HomeTemplate::Home),
            t!("db-tpl-home").to_string(),
            "home-house",
        ),
        (
            Some(HomeTemplate::Energy),
            t!("db-tpl-energy").to_string(),
            "home-bolt",
        ),
        (
            Some(HomeTemplate::Security),
            t!("db-tpl-security").to_string(),
            "home-shield",
        ),
        (
            Some(HomeTemplate::Garden),
            t!("db-tpl-garden").to_string(),
            "home-tree",
        ),
    ];
    rsx! {
        div {
            p { class: "mb-2 text-sm font-medium text-gray-700", {t!("db-tpl-pick")} }
            div { class: "flex flex-wrap gap-2",
                for (value, label, icon) in options {
                    button {
                        key: "{label}",
                        r#type: "button",
                        class: if selected == value { "flex items-center gap-1.5 rounded-full border border-signal-500 bg-signal-50 px-3 py-1.5 text-sm text-signal-700" } else { "flex items-center gap-1.5 rounded-full border border-gray-200 px-3 py-1.5 text-sm text-gray-700 hover:border-signal-300" },
                        onclick: move |_| on_pick.call(value),
                        crate::components::home_icons::HomeIconView { id: icon, class: "h-4 w-4" }
                        "{label}"
                    }
                }
            }
            p { class: "mt-1 text-xs text-gray-500", {t!("db-tpl-help")} }
        }
    }
}

/// One selectable format of the creation modal: sketch, name, summary and
/// typical uses.
#[component]
fn FormatCard(mobile: bool, selected: bool, on_pick: EventHandler<()>) -> Element {
    let ring = if selected {
        "border-signal-500 ring-2 ring-signal-200 bg-signal-50/40"
    } else {
        "border-gray-200 hover:border-signal-300"
    };
    let (title, help) = if mobile {
        (t!("db-format-mobile"), t!("db-format-mobile-help"))
    } else {
        (t!("db-format-desktop"), t!("db-format-desktop-help"))
    };
    let uses = if mobile {
        vec![
            t!("db-format-mobile-use-1"),
            t!("db-format-mobile-use-2"),
            t!("db-format-mobile-use-3"),
        ]
    } else {
        vec![
            t!("db-format-desktop-use-1"),
            t!("db-format-desktop-use-2"),
            t!("db-format-desktop-use-3"),
        ]
    };
    rsx! {
        button {
            r#type: "button",
            class: "flex flex-col gap-3 rounded-xl border bg-white p-4 text-left transition {ring}",
            aria_pressed: "{selected}",
            onclick: move |_| on_pick.call(()),
            div { class: "flex h-28 items-center justify-center rounded-lg bg-gray-50",
                if mobile {
                    MobileSketch {}
                } else {
                    DesktopSketch {}
                }
            }
            div { class: "flex items-center gap-2",
                if mobile {
                    icons::Smartphone { class: "h-5 w-5 text-teal-600" }
                } else {
                    icons::Monitor { class: "h-5 w-5 text-signal-600" }
                }
                p { class: "text-sm font-semibold text-gray-900", "{title}" }
                if selected {
                    icons::Check { class: "ml-auto h-4 w-4 text-signal-600" }
                }
            }
            p { class: "text-xs text-gray-600", "{help}" }
            ul { class: "space-y-1 text-xs text-gray-500",
                for u in uses {
                    li { class: "flex gap-1.5",
                        span { class: "text-gray-500", "•" }
                        span { "{u}" }
                    }
                }
            }
        }
    }
}

/// Sketch of a desktop dashboard: a wide canvas, free-placed widgets and a
/// wire.
#[component]
fn DesktopSketch() -> Element {
    rsx! {
        div { class: "relative h-20 w-36 rounded border border-gray-300 bg-white",
            div { class: "absolute left-2 top-2 h-7 w-7 rounded-full border-4 border-signal-300" }
            div { class: "absolute left-12 top-3 h-6 w-16 rounded bg-signal-100" }
            div { class: "absolute left-6 bottom-2 h-5 w-24 rounded bg-gray-100" }
            div { class: "absolute left-9 top-9 h-px w-10 bg-gray-400" }
            div { class: "absolute right-2 top-2 h-4 w-6 rounded bg-emerald-100" }
        }
    }
}

/// Sketch of a mobile dashboard: a phone with a stack of cards.
#[component]
fn MobileSketch() -> Element {
    rsx! {
        div { class: "flex h-24 w-14 flex-col gap-1 rounded-lg border-2 border-gray-300 bg-white p-1",
            div { class: "h-1 w-6 self-center rounded bg-gray-200" }
            div { class: "flex gap-1",
                div { class: "flex h-4 flex-1 items-center justify-center rounded bg-teal-50",
                    div { class: "h-1.5 w-3 rounded-full bg-teal-400" }
                }
                div { class: "h-4 flex-1 rounded bg-gray-100" }
            }
            div { class: "flex h-4 items-center rounded bg-gray-100 px-1",
                div { class: "h-0.5 w-full rounded bg-teal-300" }
            }
            div { class: "h-4 rounded bg-teal-100" }
        }
    }
}

// ─────────────────────────── Live view ───────────────────────────

#[component]
fn LiveSubView(
    dashboard_id: String,
    can_write: bool,
    on_edit: EventHandler<()>,
    on_back: EventHandler<()>,
) -> Element {
    let reload = use_signal(|| 0u32);
    let via_id = dashboard_id.clone();

    let detail = use_resource(move || {
        let id = dashboard_id.clone();
        async move {
            let _ = reload();
            api::dashboards::detail(&id).await.ok()
        }
    });
    let detail_loaded: Option<VizDashboard> = detail.read().as_ref().cloned().flatten();

    // ── Batch specs — tracked deps read in the synchronous part of
    // the closure (devices.rs pattern): reload (15 s rate) AND the
    // `detail` resource itself. On the first render the detail is absent
    // → empty sources; if only `reload` is subscribed, the batch only restarts
    // on the first tick (15 s of empty screen in view) — reading `detail` here
    // guarantees the re-run as soon as it resolves. A SINGLE 15 s timer for
    // the whole dashboard (D31).
    // Type dashboard (D187): the object shown, and the current sensor of
    // each of its series properties.
    let object_id = use_signal(String::new);
    let bindings = use_resource(move || {
        let id = object_id();
        async move {
            if id.is_empty() {
                return Vec::new();
            }
            api::ontology::bindings(&id).await.unwrap_or_default()
        }
    });
    let batch = use_resource(move || {
        let b = bindings.read().clone().unwrap_or_default();
        let sources: Vec<pnex_core::SourceRef> = detail
            .read()
            .as_ref()
            .cloned()
            .flatten()
            .map(|d| {
                pnex_core::bind_object(&d.layout, &b)
                    .widgets
                    .iter()
                    .flat_map(|w| w.source.iter().cloned())
                    .collect()
            })
            .unwrap_or_default();
        let _ = reload();
        async move {
            if sources.is_empty() {
                return None;
            }
            Some(crate::components::dashboard_live::fetch_live_values(sources).await)
        }
    });
    let bound = bindings.read().clone().unwrap_or_default();
    let object_type = detail_loaded
        .as_ref()
        .and_then(|d| d.layout.object_type.clone());
    let values: HashMap<String, Option<Vec<TelemetryPoint>>> =
        batch.read().as_ref().cloned().flatten().unwrap_or_default();
    // Degraded = derived at render, NEVER a signal set (infinite loop,
    // user feedback 2026-08-19 "all pages frozen").
    let any_degraded = values.values().any(|v| v.is_none());

    // User-selectable auto-refresh (default 15 s) + "refresh now".
    let auto = use_auto_refresh(reload, POLL_SECS);

    // Control cards (D125): definitions and last commanded values follow
    // the same refresh; a viewer sees them disabled.
    crate::components::surface::use_surface_controls(
        move || {
            detail
                .read()
                .as_ref()
                .cloned()
                .flatten()
                .map(|d| crate::components::surface::control_ids(&d.layout))
                .unwrap_or_default()
        },
        reload,
        format!("dashboard:{via_id}"),
        can_write,
    );

    let version_number = detail_loaded
        .as_ref()
        .map(|d| d.current_version_number)
        .unwrap_or(0);
    let version_label = t!("db-current-version", version: version_number).to_string();

    rsx! {
        div { class: "p-4 pb-24 md:p-6",
            // Phone: [back icon] [truncated title] [edit] on one line, then
            // version + refresh rate; one line with labels from sm up.
            div { class: "mb-4 md:mb-6 flex flex-wrap items-center gap-x-3 gap-y-2",
                div { class: "flex min-w-0 flex-1 items-center gap-2",
                    button {
                        class: "inline-flex shrink-0 items-center rounded-lg p-2 text-gray-600 hover:bg-gray-100 sm:border sm:border-gray-300 sm:px-3",
                        title: t!("db-back"),
                        aria_label: t!("db-back"),
                        onclick: move |_| on_back.call(()),
                        icons::ArrowLeft { class: "h-5 w-5 sm:h-4 sm:w-4 sm:mr-2" }
                        span { class: "hidden text-sm sm:inline", {t!("db-back")} }
                    }
                    h1 { class: "min-w-0 truncate text-xl font-bold text-gray-900 sm:text-2xl",
                        "{detail_loaded.as_ref().map(|d| d.name.clone()).unwrap_or_default()}"
                    }
                    // One edit button: icon-only below sm, labelled above (O52).
                    if can_write {
                        button {
                            class: "{BTN_PRIMARY_SM} ml-auto shrink-0",
                            title: t!("db-mode-edit"),
                            aria_label: t!("db-mode-edit"),
                            onclick: move |_| on_edit.call(()),
                            icons::Wrench { class: "h-4 w-4 sm:mr-1" }
                            span { class: "hidden sm:inline", {t!("db-mode-edit")} }
                        }
                    }
                }
                div { class: "flex w-full items-center gap-3 sm:w-auto",
                    span { class: "inline-flex shrink-0 items-center whitespace-nowrap rounded px-2 py-0.5 text-xs font-medium bg-green-50 text-green-700",
                        "{version_label}"
                    }
                    div { class: "ml-auto sm:ml-0",
                        RefreshRateControl { auto }
                    }
                }
            }
            if let Some(t) = object_type {
                div { class: "mb-4",
                    crate::components::ontology::ObjectPicker { type_key: t, selected: object_id }
                }
            }
            if any_degraded {
                p { class: "mb-4 rounded-lg border border-gray-200 bg-gray-50 p-3 text-xs text-gray-500",
                    {t!("vis-unavailable")}
                }
            }

            {
                match detail_loaded {
                    Some(d) => rsx! {
                        {live_layout(&pnex_core::bind_object(&d.layout, &bound), &values)}
                    },
                    None => rsx! {
                        p { class: "text-gray-500 text-center py-12", "…" }
                    },
                }
            }
        }
    }
}

// ─────────────────────────── Edit view ───────────────────────────

#[component]
fn EditorSubView(dashboard_id: String, on_back: EventHandler<()>) -> Element {
    let mut reload = use_signal(|| 0u32);
    let detail = use_resource(move || {
        let id = dashboard_id.clone();
        async move {
            let _ = reload();
            api::dashboards::detail(&id).await.ok()
        }
    });

    // Clone of the resource state BEFORE the match: the read temporaries
    // do not outlive the arms (GenerationalRef pitfalls approach).
    let state = detail.read().clone();
    match state {
        Some(Some(d)) => rsx! {
            DashboardEditor {
                key: "{d.id}-{d.current_version_number}",
                detail: d.clone(),
                can_write: true,
                on_back,
                on_changed: move |_| reload.with_mut(|r| *r += 1),
            }
        },
        // Failed request (message already in a toast): going back is possible.
        Some(None) => rsx! {
            div { class: "p-6",
                button {
                    class: "{BTN_SECONDARY_SM} mb-4",
                    onclick: move |_| on_back.call(()),
                    icons::ArrowLeft { class: "h-4 w-4 mr-2" }
                    {t!("db-back")}
                }
                div { class: "bg-red-50 border border-red-200 rounded-lg p-4 text-sm text-red-700",
                    "—"
                }
            }
        },
        None => rsx! {
            p { class: "text-gray-500 text-center py-12", "…" }
        },
    }
}
