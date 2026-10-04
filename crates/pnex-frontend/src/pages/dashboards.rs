//! Page **Tableaux de bord** (studio SCADA, D24/D34/D40/D41) :
//! liste D14 → sous-vues pilotées par signaux locaux (école
//! `pages/flows.rs`) — **live** (rendu de la version courante, un seul
//! polling 15 s via `series-batch`, dégradation par item) et **édition**
//! (`components/dashboard_editor`, polling suspendu).

use std::collections::HashMap;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{DashboardFormat, TelemetryPoint, VizDashboard, VizDashboardSummary};

use crate::api;
use crate::components::badges::date_label;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::filters::RefreshButton;
use crate::components::crud::layout::{ListLayout, DANGER_BTN};
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::dashboard_editor::state::HomeTemplate;
use crate::components::dashboard_editor::DashboardEditor;
use crate::components::dashboard_live::live_layout;
use crate::components::icons;
use crate::components::modal::Modal;
use crate::components::refresh_rate::{use_auto_refresh, RefreshRateControl};
use crate::state::viz::{OPEN_DASHBOARD, VIZ_EDIT};
use crate::state::{org, session, toasts};

/// Cadence de rafraîchissement live (même choix que dashboard/
/// visualisation : voir la donnée arriver sans marteler O2).
const POLL_SECS: u64 = 15;

/// Rôle dans l'org courante (helper privé par page, convention projet).
fn current_role() -> Option<String> {
    let user = session::user()?;
    let org_id = org::current()?;
    user.orgs
        .iter()
        .find(|m| m.id == org_id)
        .map(|m| m.role.clone())
}

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

    // ── Deep-link `?id&:mode` (dioxus #2784 : props de route ≠ signaux,
    // on les dépose dans les signaux globaux au mount). Un viewer qui
    // force mode=edit retombe en live (D34) — l'éditeur le réinterdit de
    // toute façon.
    //
    // Garde `seeded` : l'effet consomme le deep-link UNE SEULE FOIS. Sans
    // elle, chaque exécution ré-écrit OPEN_DASHBOARD depuis les props de
    // route (fixes) → ré-arme l'effet → boucle infinie (main thread bloqué,
    // « Page Unresponsive » — constat 2026-09-13, deep-link depuis le
    // drawer POI). Les écritures mid-run de OPEN_DASHBOARD/VIZ_EDIT
    // programment une 2ᵉ exécution : elle doit être un no-op.
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
            let can_edit = VIZ_EDIT.cloned()
                && current_role().is_some_and(|r| crate::state::org::role_can_write(&r));
            selected.set(Some(open_id));
            editing.set(can_edit);
            // GlobalSignal : pas de `&mut` static — mutation via with_mut
            // (méthode intrinsèque, mémoire projet).
            OPEN_DASHBOARD.with_mut(|v| *v = None);
            VIZ_EDIT.with_mut(|v| *v = false);
        }
    });

    let can_write = current_role().is_some_and(|r| crate::state::org::role_can_write(&r));

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

// ─────────────────────────── Liste ───────────────────────────

#[component]
fn ListView(
    mut reload: Signal<u32>,
    can_write: bool,
    on_open: EventHandler<(String, bool)>,
) -> Element {
    let page = use_signal(|| 0i64);
    let mut delete_target = use_signal(|| None::<(String, String)>);
    let mut choosing = use_signal(|| false);

    // Org, reload et page lus dans la partie SYNCHRONE de la closure
    // (pattern devices.rs : abonnements garantis). FIX : `page()` n'était
    // pas lue et aucun offset n'était passé — la pagination du Pager était
    // câblée mais inerte (compteur changeant sans refetch).
    let list = use_resource(move || {
        let offset = page() * PAGE_SIZE;
        let has_org = org::current().is_some();
        async move {
            let _ = reload();
            if !has_org {
                return None;
            }
            Some(
                api::dashboards::list(&api::dashboards::DashboardFilters {
                    search: None,
                    limit: Some(PAGE_SIZE),
                    offset: Some(offset),
                })
                .await,
            )
        }
    });

    // Lecture synchrone de la ressource (doctrine socle CRUD). Some(None) =
    // org absente (garde rendue côté rsx) → traité comme chargement.
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

    // Colonnes de la table — les closures d'action capturent on_open et la
    // cible de suppression ; le style des anciens th/td bruts passe aux
    // tokens du socle (th.th / td.td).
    let columns = vec![
        Column::new(t!("db-name").to_string(), move |d: &VizDashboardSummary| {
            rsx! {
                div { class: "text-sm font-medium text-gray-900", "{d.name}" }
                if let Some(desc) = &d.description {
                    div { class: "text-xs text-gray-500", "{desc}" }
                }
                div { class: "text-xs text-gray-400", "{date_label(&d.updated_at)}" }
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
        }),
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
                // Un id cloné par bouton : chaque closure `move` consomme le sien.
                let id_open = d.id.clone();
                let id_edit = d.id.clone();
                let id_delete = d.id.clone();
                let name_delete = d.name.clone();
                rsx! {
                    button {
                        class: "inline-flex items-center px-3 py-1.5 text-sm text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50 mr-2",
                        onclick: move |_| on_open.call((id_open.clone(), false)),
                        icons::Zap { class: "h-4 w-4 mr-1" }
                        {t!("db-open")}
                    }
                    if can_write {
                        button {
                            class: "inline-flex items-center px-3 py-1.5 text-sm text-white bg-blue-600 rounded-lg hover:bg-blue-700 mr-2",
                            onclick: move |_| on_open.call((id_edit.clone(), true)),
                            icons::Wrench { class: "h-4 w-4 mr-1" }
                            {t!("db-edit")}
                        }
                        button {
                            class: DANGER_BTN,
                            onclick: move |_| delete_target.set(Some((id_delete.clone(), name_delete.clone()))),
                            icons::Trash2 { class: "h-3.5 w-3.5 inline mr-0.5" }
                            {t!("viz-delete")}
                        }
                    }
                }
            },
        )
        .with_td_class("text-right whitespace-nowrap"),
    ];

    rsx! {
        ListLayout {
            title: t!("nav-dashboards").to_string(),
            subtitle: Some(t!("db-subtitle").to_string()),
            can_write,
            // Harmonisation socle : rafraîchissement manuel dans l'en-tête
            // (la page n'a pas de barre de filtres).
            actions: rsx! {
                RefreshButton { on_click: move |_| reload.with_mut(|r| *r += 1) }
            },
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
            t!("db-default-name", date : crate ::util::now_label()).to_string()
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
    let placeholder = t!("db-default-name", date : crate ::util::now_label()).to_string();
    let picked = format();
    rsx! {
        Modal {
            title: t!("db-new-title").to_string(),
            max_width: "max-w-3xl".to_string(),
            on_close: move |_| on_close.call(()),
            div { class: "space-y-5",
                label { class: "block text-sm font-medium text-gray-700",
                    {t!("db-new-name")}
                    input {
                        class: "mt-1 w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
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
                        class: "px-4 py-2 text-sm text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50",
                        r#type: "button",
                        onclick: move |_| on_close.call(()),
                        {t!("common-cancel")}
                    }
                    button {
                        class: "px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-lg hover:bg-blue-700 disabled:opacity-40",
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
                        class: if selected == value { "flex items-center gap-1.5 rounded-full border border-blue-500 bg-blue-50 px-3 py-1.5 text-sm text-blue-700" } else { "flex items-center gap-1.5 rounded-full border border-gray-200 px-3 py-1.5 text-sm text-gray-700 hover:border-blue-300" },
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
        "border-blue-500 ring-2 ring-blue-200 bg-blue-50/40"
    } else {
        "border-gray-200 hover:border-blue-300"
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
                    icons::Monitor { class: "h-5 w-5 text-blue-600" }
                }
                p { class: "text-sm font-semibold text-gray-900", "{title}" }
                if selected {
                    icons::Check { class: "ml-auto h-4 w-4 text-blue-600" }
                }
            }
            p { class: "text-xs text-gray-600", "{help}" }
            ul { class: "space-y-1 text-xs text-gray-500",
                for u in uses {
                    li { class: "flex gap-1.5",
                        span { class: "text-gray-400", "•" }
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
            div { class: "absolute left-2 top-2 h-7 w-7 rounded-full border-4 border-blue-300" }
            div { class: "absolute left-12 top-3 h-6 w-16 rounded bg-blue-100" }
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

// ─────────────────────────── Vue live ───────────────────────────

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

    // ── Specs du batch — déps trackées lues dans la partie synchrone de
    // la closure (pattern devices.rs) : reload (cadence 15 s) ET la
    // resource `detail` elle-même. Au premier rendu le détail est absent
    // → sources vides ; si seule `reload` est abonnée, le batch ne repart
    // qu'au premier tick (15 s d'écran vide en vue) — lire `detail` ici
    // garantit le re-run dès qu'il se résout. Un SEUL timer 15 s pour
    // tout le dashboard (D31).
    let batch = use_resource(move || {
        let sources: Vec<pnex_core::SourceRef> = detail
            .read()
            .as_ref()
            .cloned()
            .flatten()
            .map(|d| {
                d.layout
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
    let values: HashMap<String, Option<Vec<TelemetryPoint>>> =
        batch.read().as_ref().cloned().flatten().unwrap_or_default();
    // Dégradé = dérivé au rendu, JAMAIS un set de signal (boucle infinie,
    // retour user 2026-08-19 « toutes les pages bloquées »).
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
        div { class: "p-6",
            div { class: "mb-6 flex items-center justify-between",
                div { class: "flex items-center gap-3",
                    button {
                        class: "inline-flex items-center px-3 py-2 text-sm text-gray-600 border border-gray-300 rounded-lg hover:bg-gray-50",
                        onclick: move |_| on_back.call(()),
                        icons::ArrowLeft { class: "h-4 w-4 mr-2" }
                        {t!("db-back")}
                    }
                    h1 { class: "text-2xl font-bold text-gray-900",
                        "{detail_loaded.as_ref().map(|d| d.name.clone()).unwrap_or_default()}"
                    }
                    span { class: "inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-green-50 text-green-700",
                        "{version_label}"
                    }
                }
                div { class: "flex items-center gap-3",
                    RefreshRateControl { auto }
                    if can_write {
                        button {
                            class: "inline-flex items-center px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-lg hover:bg-blue-700",
                            onclick: move |_| on_edit.call(()),
                            icons::Wrench { class: "h-4 w-4 mr-2" }
                            {t!("db-mode-edit")}
                        }
                    }
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
                        {live_layout(&d.layout, &values)}
                    },
                    None => rsx! {
                        p { class: "text-gray-500 text-center py-12", "…" }
                    },
                }
            }
        }
    }
}

// ─────────────────────────── Vue édition ───────────────────────────

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

    // Clone du state du resource AVANT le match : les temporaires de
    // lecture ne survivent pas aux bras (école des pièges GenerationalRef).
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
        // Requête en échec (message déjà en toast) : retour possible.
        Some(None) => rsx! {
            div { class: "p-6",
                button {
                    class: "mb-4 inline-flex items-center px-3 py-2 text-sm text-gray-600 border border-gray-300 rounded-lg hover:bg-gray-50",
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
