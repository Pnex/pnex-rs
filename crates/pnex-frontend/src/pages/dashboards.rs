//! Page **Tableaux de bord** (studio SCADA, D24/D34/D40/D41) :
//! liste D14 → sous-vues pilotées par signaux locaux (école
//! `pages/flows.rs`) — **live** (rendu de la version courante, un seul
//! polling 15 s via `series-batch`, dégradation par item) et **édition**
//! (`components/dashboard_editor`, polling suspendu).

use std::collections::HashMap;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{TelemetryPoint, VizDashboard, VizDashboardSummary};

use crate::api;
use crate::components::badges::date_label;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::filters::RefreshButton;
use crate::components::crud::layout::{ListLayout, DANGER_BTN};
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::dashboard_editor::DashboardEditor;
use crate::components::dashboard_live::live_canvas;
use crate::components::icons;
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
                        can_write: can_write,
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
            ListView { reload: reload, can_write: can_write, on_open: move |open: (String, bool)| {
                selected.set(Some(open.0));
                editing.set(open.1);
            } }
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
            can_write: can_write,
            // Harmonisation socle : rafraîchissement manuel dans l'en-tête
            // (la page n'a pas de barre de filtres).
            actions: rsx! {
                RefreshButton { on_click: move |_| reload.with_mut(|r| *r += 1) }
            },
            add_label: Some(t!("db-create").to_string()),
            on_add: move |_| {
                // Création immédiate, sans modale : nom daté puis ouverture
                // directe en édition (fluidité — retour user 2026-09-18).
                let params = pnex_core::CreateDashboard {
                    name: t!("db-default-name", date: crate::util::now_label()).to_string(),
                    description: None,
                    layout: None,
                };
                spawn(async move {
                    match api::dashboards::create(params).await {
                        Ok(dashboard) => {
                            toasts::success(t!("db-created").to_string());
                            reload.with_mut(|r| *r += 1);
                            on_open.call((dashboard.id, true));
                        }
                        Err(e) => toasts::error(e),
                    }
                });
            },
            if org::current().is_none() {
                p { class: "text-gray-500 text-center py-12", {t!("orgs-empty")} }
            } else {
                ListStates {
                    state: list_state,
                    is_empty: is_empty,
                    empty_message: t!("db-list-empty").to_string(),
                    div { class: "relative",
                        DataTable {
                            columns: columns,
                            rows: rows,
                            row_key: RowKey::new(|d: &VizDashboardSummary| d.id.clone()),
                        }
                        ListPager { count: count, page: page }
                    }
                }
            }

            // Modal de création supprimée : « + Nouveau » crée directement
            // (nom daté) et ouvre l'éditeur en mode édition.
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

// ─────────────────────────── Vue live ───────────────────────────

#[component]
fn LiveSubView(
    dashboard_id: String,
    can_write: bool,
    on_edit: EventHandler<()>,
    on_back: EventHandler<()>,
) -> Element {
    let reload = use_signal(|| 0u32);

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

            {match detail_loaded {
                Some(d) => rsx! { {live_canvas(&d.layout, &values)} },
                None => rsx! { p { class: "text-gray-500 text-center py-12", "…" } },
            }}
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
                on_back: on_back,
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
                div { class: "bg-red-50 border border-red-200 rounded-lg p-4 text-sm text-red-700", "—" }
            }
        },
        None => rsx! { p { class: "text-gray-500 text-center py-12", "…" } },
    }
}
