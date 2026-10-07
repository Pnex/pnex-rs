//! Layout racine — porté du `Layout.tsx` React : sidebar grise-900 fixe en
//! desktop, drawer mobile + garde de session (rend `Login` à la place de
//! l'`Outlet` tant que l'utilisateur n'est pas authentifié — parité
//! `AuthWrapper` React, pas de route `/login`).
//!
//! Le pied de sidebar porte le sélecteur d'org (tenant actif), l'identité et
//! la déconnexion — concepts multi-tenant absents de l'UI d'origine.

use crate::app::Route;
use crate::components::assistant::AssistantPanel;
use crate::components::org_switcher::OrgSwitcher;
use crate::components::toasts::ToastContainer;
use crate::state::session::{self, SessionState, SESSION};
use crate::state::ui;
use dioxus::prelude::*;
use dioxus_i18n::t;

#[component]
pub fn Shell() -> Element {
    match SESSION.cloned() {
        SessionState::Booting => rsx! {
            div { class: "min-h-screen flex items-center justify-center bg-gray-50",
                span { class: "animate-spin rounded-full h-10 w-10 border-b-2 border-blue-600" }
            }
        },
        SessionState::LoggedOut => rsx! {
            crate::pages::login::Login {}
        },
        SessionState::Authenticated { .. } => rsx! {
            ShellContent {}
            AssistantPanel {}
            ToastContainer {}
        },
    }
}

#[component]
fn ShellContent() -> Element {
    let mut sidebar_open = use_signal(|| false);

    // Restaure la préférence rail au montage (lecture storage seule —
    // l'effet s'exécute une fois, aucune valeur réactive lue).
    use_effect(move || {
        ui::restore();
    });
    #[cfg(feature = "e2e")]
    use_e2e_navigation();
    // Signal global réactif : le contenu et la sidebar se re-render au toggle.
    let rail = *ui::RAIL.read();

    rsx! {
        div { class: "min-h-screen bg-gray-50",
            // Drawer mobile — toujours déplié (w-64), le rail est un concept
            // desktop uniquement.
            if sidebar_open() {
                div { class: "fixed inset-0 z-50 lg:hidden",
                    div {
                        class: "fixed inset-0 bg-gray-600/75",
                        onclick: move |_| sidebar_open.set(false),
                    }
                    div { class: "fixed inset-y-0 left-0 flex w-64 flex-col bg-gray-900",
                        div { class: "flex h-16 items-center justify-between px-4",
                            SidebarBrand {}
                            button {
                                class: "text-gray-400 hover:text-white",
                                onclick: move |_| sidebar_open.set(false),
                                crate::components::icons::X { class: "h-6 w-6" }
                            }
                        }
                        // Global search (D69) in the drawer too: on a phone
                        // the desktop sidebar never shows.
                        SidebarSearch {
                            rail: false,
                            on_navigate: Some(Callback::new(move |()| sidebar_open.set(false))),
                        }
                        div { class: "flex-1 min-h-0 overflow-y-auto sidebar-scroll px-4 py-6",
                            Nav {
                                on_navigate: Some(Callback::new(move |()| sidebar_open.set(false))),
                                rail: false,
                            }
                        }
                        SidebarFooter { rail: false }
                    }
                }
            }

            // Sidebar desktop — dépliée w-64 ou rail d'icônes w-16 (classes
            // littérales complètes via match, exigence du scan Tailwind).
            div { class: if rail { "hidden lg:fixed lg:inset-y-0 lg:flex lg:w-16 lg:flex-col lg:bg-gray-900" } else { "hidden lg:fixed lg:inset-y-0 lg:flex lg:w-64 lg:flex-col lg:bg-gray-900" },
                div { class: "flex h-16 items-center justify-center px-3",
                    if rail {
                        crate::components::icons::Zap { class: "h-7 w-7 text-white" }
                    } else {
                        SidebarBrand {}
                    }
                }
                // Global search (D69) — right below the logo; rail mode gets
                // an icon button that re-expands the sidebar.
                SidebarSearch { rail }
                // Défilement quand tous les groupes sont dépliés (le menu
                // dépasse l'écran sur web — constat user 2026-09-18).
                div { class: "flex-1 min-h-0 overflow-y-auto sidebar-scroll px-3 py-6",
                    Nav { rail }
                }
                SidebarFooter { rail }
            }

            div { class: if rail { "lg:pl-16" } else { "lg:pl-64" },
                // En-tête mobile
                // Hidden while a full-screen editor is open: its own bar
                // has the back button (saves 64 px of canvas on phones).
                div { class: if *ui::EDITORS_OPEN.read() > 0 { "hidden" } else { "sticky top-0 z-40 lg:hidden" },
                    div { class: "flex h-16 items-center justify-between bg-white px-4 shadow-sm",
                        button {
                            class: "text-gray-500 hover:text-gray-600",
                            onclick: move |_| sidebar_open.set(true),
                            crate::components::icons::Menu { class: "h-6 w-6" }
                        }
                        div { class: "flex items-center",
                            img {
                                src: asset!("/assets/logo.png"),
                                alt: "PNeX",
                                class: "h-8 w-auto",
                            }
                        }
                        div {}
                    }
                }
                main { class: "flex-1", Outlet::<Route> {} }
            }
        }
    }
}

/// e2e builds only (feature `e2e`, never shipped): exposes
/// `window.__pnexNavigate(path)` so the native e2e harness can open any route.
/// Native routers keep an in-memory history, a URL navigation does nothing.
#[cfg(feature = "e2e")]
fn use_e2e_navigation() {
    let navigator = use_navigator();
    use_future(move || async move {
        let mut bridge = document::eval(
            "window.__pnexNavigate = (path) => dioxus.send(path); \
             await new Promise(() => {});",
        );
        while let Ok(path) = bridge.recv::<String>().await {
            if let Ok(route) = path.parse::<Route>() {
                navigator.push(route);
            }
        }
    });
}

/// Classes de navigation : littéraux complets pour le scan Tailwind (jamais
/// de classes construites dynamiquement). Variante rail : items centrés,
/// sans libellé — l'infobulle native (`title`) porte le nom.
fn nav_class(active: bool, rail: bool) -> &'static str {
    match (active, rail) {
        (true, false) => {
            "flex w-full items-center space-x-3 rounded-lg px-3 py-2 text-left bg-blue-600 text-white transition-colors"
        }
        (false, false) => {
            "flex w-full items-center space-x-3 rounded-lg px-3 py-2 text-left text-gray-300 hover:bg-gray-800 hover:text-white transition-colors"
        }
        (true, true) => {
            "flex w-full items-center justify-center rounded-lg px-2 py-2 bg-blue-600 text-white transition-colors"
        }
        (false, true) => {
            "flex w-full items-center justify-center rounded-lg px-2 py-2 text-gray-300 hover:bg-gray-800 hover:text-white transition-colors"
        }
    }
}

/// Classes du bouton d'en-tête de groupe — même logique que `nav_class`.
fn nav_group_class(rail: bool) -> &'static str {
    if rail {
        "flex w-full items-center justify-center rounded-lg px-2 py-2 text-gray-300 hover:bg-gray-800 hover:text-white transition-colors"
    } else {
        "flex w-full items-center space-x-3 rounded-lg px-3 py-2 text-left text-gray-300 hover:bg-gray-800 hover:text-white transition-colors"
    }
}

/// Navigation latérale — composant dédié pour être rendu dans le drawer ET la
/// sidebar desktop. `on_navigate` est fourni par le drawer mobile (referme le
/// drawer après un clic sur un item) ; la sidebar desktop n'en a pas besoin.
/// `rail` : mode rail d'icônes (desktop replié) — libellés masqués, groupes
/// qui rouvrent la sidebar.
#[component]
fn Nav(on_navigate: Option<Callback<()>>, rail: bool) -> Element {
    let route = use_route::<Route>();
    let is_platform_admin = session::user().is_some_and(|u| u.platform_admin);
    let close_drawer = move |_| {
        if let Some(cb) = on_navigate {
            cb.call(());
        }
    };
    // Libellé conditionnel — dans le rail, l'item ne garde que son icône.
    rsx! {
        nav { class: "flex-1 space-y-2",
            Link {
                to: Route::Dashboard {},
                class: nav_class(route == Route::Dashboard {}, rail),
                title: t!("nav-dashboard"),
                onclick: close_drawer,
                crate::components::icons::Home { class: "h-5 w-5" }
                if !rail {
                    span { {t!("nav-dashboard")} }
                }
            }
            // Groupe « Visualisation » : Quick charts + Tableaux de bord +
            // Carte (les trois vues de télémétrie).
            NavVizGroup { on_navigate, rail }
            // Groupe « Data » : Library (médias + futurs documents / base de
            // connaissances agent) + Tour studio + futur Plan (placeholder
            // « bientôt », aucune route).
            NavDataGroup { on_navigate, rail }
            // Groupe « Edges » : Devices + Catalogue + futurs collecteurs
            // (doc docs/architecture/edge-model.md — D44-D48).
            NavEdges { on_navigate, rail }
            // Groupe « Automation » : fonctions + flux ETL + notifications +
            // mélanges de fluides.
            NavAutomationGroup { on_navigate, rail }
            Link {
                to: Route::Orgs {},
                class: nav_class(matches!(route, Route::Orgs {} | Route::OrgsCurrent {}), rail),
                title: t!("nav-orgs"),
                onclick: close_drawer,
                crate::components::icons::Building { class: "h-5 w-5" }
                if !rail {
                    span { {t!("nav-orgs")} }
                }
            }
            Link {
                to: Route::Secrets {},
                class: nav_class(route == Route::Secrets {}, rail),
                title: t!("nav-secrets"),
                onclick: close_drawer,
                crate::components::icons::Key { class: "h-5 w-5" }
                if !rail {
                    span { {t!("nav-secrets")} }
                }
            }
            Link {
                to: Route::System {},
                class: nav_class(route == Route::System {}, rail),
                title: t!("nav-system"),
                onclick: close_drawer,
                crate::components::icons::Wrench { class: "h-5 w-5" }
                if !rail {
                    span { {t!("nav-system")} }
                }
            }
            if is_platform_admin {
                Link {
                    to: Route::AdminStatus {},
                    class: nav_class(route == Route::AdminStatus {}, rail),
                    title: t!("nav-admin-status"),
                    onclick: close_drawer,
                    crate::components::icons::Activity { class: "h-5 w-5" }
                    if !rail {
                        span { {t!("nav-admin-status")} }
                    }
                }
            }
            Link {
                to: Route::Profile {},
                class: nav_class(route == Route::Profile {}, rail),
                title: t!("nav-profile"),
                onclick: close_drawer,
                crate::components::icons::User { class: "h-5 w-5" }
                if !rail {
                    span { {t!("nav-profile")} }
                }
            }
        }
    }
}

#[component]
fn SidebarBrand() -> Element {
    // Variante claire (lettres blanches, X rouge) : la variante navy serait
    // illisible directement sur le bg-gray-900 de la sidebar.
    rsx! {
        img {
            src: asset!("/assets/logo-light.png"),
            alt: "PNeX",
            class: "h-10 w-auto",
        }
    }
}

#[component]
fn SidebarFooter(rail: bool) -> Element {
    let identity = session::user()
        .map(|user| user.full_name.or(user.email).unwrap_or(user.username))
        .unwrap_or_default();
    let mut confirm_logout = use_signal(|| false);
    let logout_title = t!("shell-logout");
    let expand_title = t!("shell-sidebar-expand");
    let collapse_title = t!("shell-sidebar-collapse");
    rsx! {
        div { class: "p-4 border-t border-gray-800 space-y-3",
            if rail {
                // Rail : deux icônes empilées (rouvrir la sidebar, déconnexion).
                div { class: "flex flex-col items-center gap-3",
                    button {
                        class: "text-gray-400 hover:text-white transition-colors",
                        title: "{expand_title}",
                        onclick: move |_| ui::set(false),
                        crate::components::icons::PanelLeftOpen { class: "h-5 w-5" }
                    }
                    button {
                        class: "text-gray-400 hover:text-white transition-colors",
                        title: "{logout_title}",
                        onclick: move |_| confirm_logout.set(true),
                        crate::components::icons::LogOut { class: "h-4 w-4" }
                    }
                }
            } else {
                OrgSwitcher {}
                div { class: "flex items-center justify-between gap-2 px-1",
                    button {
                        class: "text-gray-400 hover:text-white transition-colors",
                        title: "{collapse_title}",
                        onclick: move |_| ui::toggle(),
                        crate::components::icons::PanelLeftClose { class: "h-4 w-4" }
                    }
                    span { class: "text-xs text-gray-400 truncate", {identity} }
                    button {
                        class: "text-gray-400 hover:text-white transition-colors",
                        title: "{logout_title}",
                        onclick: move |_| confirm_logout.set(true),
                        crate::components::icons::LogOut { class: "h-4 w-4" }
                    }
                }
            }
            if confirm_logout() {
                crate::components::confirm::ConfirmDialog {
                    title: t!("shell-logout-confirm-title"),
                    message: t!("shell-logout-confirm-message"),
                    confirm_label: t!("shell-logout-confirm-action"),
                    on_confirm: move |_| {
                        confirm_logout.set(false);
                        session::logout();
                    },
                    on_cancel: move |_| confirm_logout.set(false),
                }
            }
        }
    }
}

/// Groupe « Edges » de la navigation — converge Devices + Catalogue sous la
/// bannière edge (D44–D48) et matérialise la place des futurs collecteurs
/// (Edge Agent, collecteur navigateur : placeholders désactivés, aucune
/// route). Le groupe s'auto-déploie à l'entrée dans le périmètre (deep-link
/// /devices ou /catalog) mais ne se referme jamais tout seul.
#[component]
fn NavEdges(on_navigate: Option<Callback<()>>, rail: bool) -> Element {
    let route = use_route::<Route>();
    let in_edges = route == Route::Devices {}
        || route == Route::Catalog {}
        || route == Route::EdgeRefs {}
        || route == Route::Firmware {};
    // Ouvert au montage si on est déjà dans le périmètre (deep-link
    // /devices, /catalog). Volontairement SANS effet d'auto-expand à la
    // navigation : un use_effect qui écrit un signal qu'il lit s'auto-réveille
    // (boucle infinie → freeze UI, constat 2026-09-15 sur /catalog) ; le seul
    // chemin UI vers ces pages passe de toute façon par le groupe ouvert.
    let mut open = use_signal(|| in_edges);

    let close_drawer = move |_| {
        if let Some(cb) = on_navigate {
            cb.call(());
        }
    };

    // Entrée désactivée (futur collecteur) — badge « bientôt », pas de route.
    // (Badge inline ×2 : un rsx! n'est pas Copy.)
    rsx! {
        div { class: "space-y-2",
            button {
                class: nav_group_class(rail),
                r#type: "button",
                title: t!("nav-edges"),
                onclick: move |_| {
                    if rail {
                        // Rail : un groupe rouvre la sidebar sur ce groupe.
                        ui::set(false);
                        open.set(true);
                    } else {
                        open.toggle();
                    }
                },
                crate::components::icons::Router { class: "h-5 w-5" }
                if !rail {
                    span { {t!("nav-edges")} }
                    if open() {
                        crate::components::icons::ChevronDown { class: "ml-auto h-4 w-4 text-gray-400" }
                    } else {
                        crate::components::icons::ChevronRight { class: "ml-auto h-4 w-4 text-gray-400" }
                    }
                }
            }
            // En rail, les enfants ne sont jamais rendus (le clic rouvre la
            // sidebar entière).
            if open() && !rail {
                div { class: "space-y-1 pl-6",
                    Link {
                        to: Route::Devices {},
                        class: nav_class(route == Route::Devices {}, false),
                        onclick: close_drawer,
                        crate::components::icons::Cpu { class: "h-5 w-5" }
                        span { {t!("nav-devices")} }
                    }
                    Link {
                        to: Route::Catalog {},
                        class: nav_class(route == Route::Catalog {}, false),
                        onclick: close_drawer,
                        crate::components::icons::Package { class: "h-5 w-5" }
                        span { {t!("nav-catalog")} }
                    }
                    Link {
                        to: Route::EdgeRefs {},
                        class: nav_class(route == Route::EdgeRefs {}, false),
                        onclick: close_drawer,
                        crate::components::icons::Database { class: "h-5 w-5" }
                        span { {t!("nav-edge-refs")} }
                    }
                    Link {
                        to: Route::Firmware {},
                        class: nav_class(route == Route::Firmware {}, false),
                        onclick: close_drawer,
                        crate::components::icons::Braces { class: "h-5 w-5" }
                        span { {t!("nav-firmware")} }
                    }
                    div { class: "flex w-full items-center space-x-3 rounded-lg px-3 py-2 text-left text-gray-500 cursor-not-allowed",
                        crate::components::icons::Globe { class: "h-5 w-5" }
                        span { {t!("nav-browser-collector")} }
                        span { class: "ml-auto rounded bg-gray-700 px-1.5 py-0.5 text-[10px] uppercase text-gray-300",
                            {t!("nav-soon")}
                        }
                    }
                }
            }
        }
    }
}

/// Groupe « Data » de la navigation — converge la Library (médias, demain
/// documents PDF/Word indexés pour la base de connaissances de l'agent) et le
/// Tour studio, et matérialise la place du futur gestionnaire de plans
/// (placeholder désactivé, aucune route). Même contrat que NavEdges : ouvert
/// au montage si la route courante est dans le périmètre (deep-link /media,
/// /studio), jamais refermé automatiquement — pas d'auto-expand (lecture +
/// écriture du même signal dans un use_effect = boucle infinie → freeze UI).
#[component]
fn NavDataGroup(on_navigate: Option<Callback<()>>, rail: bool) -> Element {
    let route = use_route::<Route>();
    let in_media = route == Route::Media {}
        || route == Route::Cameras {}
        || route == Route::Models {}
        || route == Route::Studio {}
        || route == Route::Annotations {}
        || route == Route::Controls {};
    let mut open = use_signal(|| in_media);

    let close_drawer = move |_| {
        if let Some(cb) = on_navigate {
            cb.call(());
        }
    };

    // Entrée désactivée (futur gestionnaire de plans) — badge « bientôt »,
    // pas de route. (Badge inline : un rsx! n'est pas Copy.)
    rsx! {
        div { class: "space-y-2",
            button {
                class: nav_group_class(rail),
                r#type: "button",
                title: t!("nav-data"),
                onclick: move |_| {
                    if rail {
                        ui::set(false);
                        open.set(true);
                    } else {
                        open.toggle();
                    }
                },
                crate::components::icons::Database { class: "h-5 w-5" }
                if !rail {
                    span { {t!("nav-data")} }
                    if open() {
                        crate::components::icons::ChevronDown { class: "ml-auto h-4 w-4 text-gray-400" }
                    } else {
                        crate::components::icons::ChevronRight { class: "ml-auto h-4 w-4 text-gray-400" }
                    }
                }
            }
            // En rail, les enfants ne sont jamais rendus (le clic rouvre la
            // sidebar entière).
            if open() && !rail {
                div { class: "space-y-1 pl-6",
                    Link {
                        to: Route::Media {},
                        class: nav_class(route == Route::Media {}, false),
                        onclick: close_drawer,
                        crate::components::icons::Image { class: "h-5 w-5" }
                        span { {t!("nav-library")} }
                    }
                    Link {
                        to: Route::Cameras {},
                        class: nav_class(route == Route::Cameras {}, false),
                        onclick: close_drawer,
                        crate::components::icons::Video { class: "h-5 w-5" }
                        span { {t!("nav-cameras")} }
                    }
                    Link {
                        to: Route::Models {},
                        class: nav_class(route == Route::Models {}, false),
                        onclick: close_drawer,
                        crate::components::icons::Eye { class: "h-5 w-5" }
                        span { {t!("nav-models")} }
                    }
                    Link {
                        to: Route::Studio {},
                        class: nav_class(route == Route::Studio {}, false),
                        onclick: close_drawer,
                        crate::components::icons::Map { class: "h-5 w-5" }
                        span { {t!("nav-studio")} }
                    }
                    Link {
                        to: Route::Annotations {},
                        class: nav_class(route == Route::Annotations {}, false),
                        onclick: close_drawer,
                        crate::components::icons::MapPin { class: "h-5 w-5" }
                        span { {t!("nav-annotations")} }
                    }
                    Link {
                        to: Route::Controls {},
                        class: nav_class(route == Route::Controls {}, false),
                        onclick: close_drawer,
                        crate::components::icons::ToggleRight { class: "h-5 w-5" }
                        span { {t!("nav-controls")} }
                    }
                    div { class: "flex w-full items-center space-x-3 rounded-lg px-3 py-2 text-left text-gray-500 cursor-not-allowed",
                        crate::components::icons::MapPin { class: "h-5 w-5" }
                        span { {t!("nav-plan")} }
                        span { class: "ml-auto rounded bg-gray-700 px-1.5 py-0.5 text-[10px] uppercase text-gray-300",
                            {t!("nav-soon")}
                        }
                    }
                }
            }
        }
    }
}

/// Groupe « Visualisation » de la navigation — converge les trois vues de
/// télémétrie : Quick charts (vérif rapide que les données arrivent),
/// Tableaux de bord (dashboarding SCADA complet) et la Carte (vue spatiale
/// POI). Même contrat que NavEdges/NavDataGroup/NavFlowGroup : ouvert au
/// montage si la route courante est dans le périmètre (deep-link
/// /visualisation, /dashboards, /map), jamais refermé automatiquement — pas
/// d'auto-expand (lecture + écriture du même signal dans un use_effect =
/// boucle infinie → freeze UI).
#[component]
fn NavVizGroup(on_navigate: Option<Callback<()>>, rail: bool) -> Element {
    let route = use_route::<Route>();
    // matches! pour Dashboards : l'égalité stricte échoue dès que l'URL
    // porte un id/mode — on veut le groupe et l'entrée actifs sur n'importe
    // quel tableau ouvert.
    let in_viz = route == Route::Visualisation {}
        || matches!(route, Route::Dashboards { .. })
        || route == Route::Map {};
    let mut open = use_signal(|| in_viz);

    let close_drawer = move |_| {
        if let Some(cb) = on_navigate {
            cb.call(());
        }
    };

    rsx! {
        div { class: "space-y-2",
            button {
                class: nav_group_class(rail),
                r#type: "button",
                title: t!("nav-visualisation"),
                onclick: move |_| {
                    if rail {
                        ui::set(false);
                        open.set(true);
                    } else {
                        open.toggle();
                    }
                },
                crate::components::icons::LineChart { class: "h-5 w-5" }
                if !rail {
                    span { {t!("nav-visualisation")} }
                    if open() {
                        crate::components::icons::ChevronDown { class: "ml-auto h-4 w-4 text-gray-400" }
                    } else {
                        crate::components::icons::ChevronRight { class: "ml-auto h-4 w-4 text-gray-400" }
                    }
                }
            }
            if open() && !rail {
                div { class: "space-y-1 pl-6",
                    Link {
                        to: Route::Visualisation {},
                        class: nav_class(route == Route::Visualisation {}, false),
                        onclick: close_drawer,
                        crate::components::icons::TrendingUp { class: "h-5 w-5" }
                        span { {t!("nav-quick-charts")} }
                    }
                    Link {
                        to: Route::Dashboards {
                            id: String::new(),
                            mode: String::new(),
                        },
                        class: nav_class(matches!(route, Route::Dashboards { .. }), false),
                        onclick: close_drawer,
                        crate::components::icons::Gauge { class: "h-5 w-5" }
                        span { {t!("nav-dashboards")} }
                    }
                    Link {
                        to: Route::Map {},
                        class: nav_class(route == Route::Map {}, false),
                        onclick: close_drawer,
                        crate::components::icons::Map { class: "h-5 w-5" }
                        span { {t!("nav-map")} }
                    }
                }
            }
        }
    }
}

/// Groupe « Automation » de la navigation — converge les fonctions, les flux
/// ETL, les notifications et les mélanges de fluides (tous alimentés par le
/// moteur de flows). Même contrat que NavEdges : ouvert au montage si la
/// route courante est dans le périmètre (deep-link /flows, /notifications,
/// /mixtures), jamais refermé automatiquement — pas d'auto-expand (lecture +
/// écriture du même signal dans un use_effect = boucle infinie → freeze UI).
#[component]
fn NavAutomationGroup(on_navigate: Option<Callback<()>>, rail: bool) -> Element {
    let route = use_route::<Route>();
    let in_flows = route == Route::Functions {}
        || matches!(route, Route::Flows { .. })
        || route == Route::Notifications {}
        || route == Route::Events {}
        || route == Route::FluidMixtures {};
    let mut open = use_signal(|| in_flows);

    let close_drawer = move |_| {
        if let Some(cb) = on_navigate {
            cb.call(());
        }
    };

    rsx! {
        div { class: "space-y-2",
            button {
                class: nav_group_class(rail),
                r#type: "button",
                title: t!("nav-automation"),
                onclick: move |_| {
                    if rail {
                        ui::set(false);
                        open.set(true);
                    } else {
                        open.toggle();
                    }
                },
                crate::components::icons::Workflow { class: "h-5 w-5" }
                if !rail {
                    span { {t!("nav-automation")} }
                    if open() {
                        crate::components::icons::ChevronDown { class: "ml-auto h-4 w-4 text-gray-400" }
                    } else {
                        crate::components::icons::ChevronRight { class: "ml-auto h-4 w-4 text-gray-400" }
                    }
                }
            }
            if open() && !rail {
                div { class: "space-y-1 pl-6",
                    // Les fonctions alimentent les flows : premier item du
                    // groupe « Automation ».
                    Link {
                        to: Route::Functions {},
                        class: nav_class(route == Route::Functions {}, false),
                        onclick: close_drawer,
                        crate::components::icons::Wrench { class: "h-5 w-5" }
                        span { {t!("nav-functions")} }
                    }
                    Link {
                        to: Route::Flows { id: String::new() },
                        class: nav_class(matches!(route, Route::Flows { .. }), false),
                        onclick: close_drawer,
                        crate::components::icons::Activity { class: "h-5 w-5" }
                        span { {t!("nav-flows")} }
                    }
                    Link {
                        to: Route::Notifications {},
                        class: nav_class(route == Route::Notifications {}, false),
                        onclick: close_drawer,
                        crate::components::icons::Bell { class: "h-5 w-5" }
                        span { {t!("nav-notifications")} }
                    }
                    Link {
                        to: Route::Events {},
                        class: nav_class(route == Route::Events {}, false),
                        onclick: close_drawer,
                        crate::components::icons::History { class: "h-5 w-5" }
                        span { {t!("nav-events")} }
                    }
                    Link {
                        to: Route::FluidMixtures {},
                        class: nav_class(route == Route::FluidMixtures {}, false),
                        onclick: close_drawer,
                        crate::components::icons::Layers { class: "h-5 w-5" }
                        span { {t!("nav-mixtures")} }
                    }
                }
            }
        }
    }
}

/// Global search typeahead (D69) — input right below the logo, results in
/// a grouped dropdown (group headers via `search-group-*` keys). Typeahead
/// fetches on a 250 ms debounce, 2 chars minimum; keyboard ↑↓/Enter/Escape;
/// a hit navigates by setting its deep-link signal then pushing the route
/// (school `components/assistant.rs`). Rail mode: one icon button that
/// re-expands the sidebar.
#[component]
fn SidebarSearch(
    rail: bool,
    /// Called after a hit opens its object (the mobile drawer closes).
    #[props(default)]
    on_navigate: Option<Callback<()>>,
) -> Element {
    const DEBOUNCE_MS: u64 = 250;
    const MIN_CHARS: usize = 2;

    let mut q = use_signal(String::new);
    let mut debounced = use_signal(String::new);
    let mut active = use_signal(|| 0usize);
    let navigator = use_navigator();

    // Typeahead — signals read in the SYNCHRONOUS part of the closure
    // (dioxus #2784); idle (no org / short term) stays None.
    let res = use_resource(move || {
        let term = debounced.cloned();
        let org = crate::state::org::current();
        async move {
            if term.chars().count() < MIN_CHARS || org.is_none() {
                return None;
            }
            match crate::api::search::global(&crate::api::search::SearchFilters {
                q: term,
                limit: Some(5),
            })
            .await
            {
                Ok(resp) => Some(Ok(resp)),
                Err(_) => Some(Err(())),
            }
        }
    });

    let q_value = q.cloned();
    let trimmed = q_value.trim().to_string();
    // Dropdown opens as soon as the input holds enough characters.
    let open = trimmed.chars().count() >= MIN_CHARS;

    // Loading = a valid query is pending (resource still idle or answering
    // an older term — the response carries its own `q`).
    let resource_state = res.value().read().clone();
    let inner = resource_state.flatten();
    let loading = match &inner {
        None => open,
        Some(Ok(resp)) => resp.q != trimmed,
        Some(Err(())) => false,
    };

    // Flatten results into owned rows BEFORE rsx (no captured lets inside
    // rsx loops). One flat index per row drives keyboard navigation.
    let mut flat: Vec<(String, crate::api::search::SearchHit)> = Vec::new();
    if let Some(Ok(resp)) = &inner {
        if resp.q == trimmed {
            for group in &resp.groups {
                for hit in &group.results {
                    flat.push((group.entity_type.clone(), hit.clone()));
                }
            }
        }
    }
    // Error surfaces while the dropdown is open; a newer in-flight query
    // (loading) replaces it as soon as it resolves.
    let has_error = matches!(&inner, Some(Err(())));

    // ─── Rail mode: icon button that re-expands the sidebar ───
    if rail {
        return rsx! {
            div { class: "flex justify-center pt-3",
                button {
                    class: "flex items-center justify-center rounded-lg px-2 py-2 text-gray-300 hover:bg-gray-800 hover:text-white transition-colors",
                    title: t!("search-title"),
                    onclick: move |_| ui::set(false),
                    crate::components::icons::Search { class: "h-5 w-5" }
                }
            }
        };
    }

    rsx! {
        div { class: "relative px-3 pt-3",
            // Transparent click-outside backdrop, same z-sandwich as the
            // mobile drawer; the dropdown sits above it.
            if open {
                div {
                    class: "fixed inset-0 z-20",
                    onclick: move |_| q.set(String::new()),
                }
            }
            div { class: "relative",
                div { class: "pointer-events-none absolute inset-y-0 left-0 flex items-center pl-3",
                    crate::components::icons::Search { class: "h-4 w-4 text-gray-400" }
                }
                input {
                    class: "w-full rounded-lg bg-gray-800 py-2 pl-9 pr-3 text-sm text-gray-100 placeholder-gray-400 focus:outline-none focus:ring-2 focus:ring-blue-500",
                    placeholder: t!("search-placeholder"),
                    value: "{q_value}",
                    oninput: move |event| {
                        let value = event.value();
                        q.set(value.clone());
                        active.set(0);
                        // Debounce: sleep-and-compare — only the latest
                        // input wins (no debounce utility in the codebase).
                        spawn(async move {
                            crate::util::sleep(std::time::Duration::from_millis(DEBOUNCE_MS)).await;
                            if q.cloned() == value {
                                if value.trim().chars().count() >= MIN_CHARS {
                                    debounced.set(value.trim().to_string());
                                } else {
                                    debounced.set(String::new());
                                }
                            }
                        });
                    },
                    onkeydown: move |event| {
                        match event.key() {
                            Key::ArrowDown => {
                                let len = flat.len();
                                if len > 0 {
                                    let next = (active() + 1).min(len - 1);
                                    active.set(next);
                                }
                            }
                            Key::ArrowUp => {
                                let next = active().saturating_sub(1);
                                active.set(next);
                            }
                            Key::Enter => {
                                let len = flat.len();
                                let idx = active().min(len.saturating_sub(1));
                                if let Some((entity_type, hit)) = flat.get(idx) {
                                    let hit = hit.clone();
                                    open_hit(&navigator, entity_type, &hit);
                                    if let Some(cb) = on_navigate {
                                        cb.call(());
                                    }
                                }
                            }
                            Key::Escape => {
                                q.set(String::new());
                                debounced.set(String::new());
                                active.set(0);
                            }
                            _ => {}
                        }
                    },
                }
            }
            if open {
                div {
                    class: "absolute left-3 right-3 top-full mt-1 z-30 max-h-96 overflow-y-auto rounded-xl border border-gray-200 bg-white shadow-xl",
                    if has_error {
                        p { class: "px-3 py-4 text-sm text-gray-500 text-center",
                            {t!("search-error")}
                        }
                    } else if loading && flat.is_empty() {
                        p { class: "px-3 py-4 text-sm text-gray-500 text-center",
                            {t!("common-loading")}
                        }
                    } else if flat.is_empty() {
                        p { class: "px-3 py-4 text-sm text-gray-500 text-center",
                            {t!("search-no-result")}
                        }
                    } else {
                        // Grouped rows in canonical server order; a group
                        // header renders at each entity-type boundary. No
                        // let-bindings inside the rsx loop (dioxus pitfall):
                        // group start is a plain expression.
                        for (idx, (entity_type, hit)) in flat.iter().enumerate() {
                            SearchHitRow {
                                key: "{entity_type}-{hit.id}",
                                entity_type: entity_type.clone(),
                                hit: hit.clone(),
                                active: idx == active(),
                                is_group_start: is_group_start(&flat, idx),
                                on_navigate,
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Fluent key of the `search-group-*` label for one entity type.
fn search_group_key(entity_type: &str) -> &'static str {
    match entity_type {
        "poi" => "search-group-poi",
        "tour" => "search-group-tour",
        "media" => "search-group-media",
        "layer" => "search-group-layer",
        "function" => "search-group-function",
        "flow" => "search-group-flow",
        "dashboard" => "search-group-dashboard",
        "edge_ref" => "search-group-edge_ref",
        _ => "search-group-device",
    }
}

/// Navigate to a hit: set the deep-link signal first, then push the route
/// (school `components/assistant.rs`). Dashboards go through the URL param
/// (the page's deep_seeded effect seeds `open_dashboard` from it — pushing
/// with an empty id would CLEAR the signal, see pages/dashboards.rs).
fn open_hit(
    navigator: &dioxus::router::Navigator,
    entity_type: &str,
    hit: &crate::api::search::SearchHit,
) {
    match entity_type {
        "device" => {
            crate::state::devices::OPEN_DEVICE.with_mut(|v| *v = hit.id.parse::<i64>().ok());
            navigator.push(Route::Devices {});
        }
        "poi" => {
            let id = hit.id.clone();
            crate::state::map::OPEN_POI.with_mut(|v| *v = Some(id));
            navigator.push(Route::Map {});
        }
        "tour" => {
            let id = hit.id.clone();
            crate::state::tours::OPEN_TOUR.with_mut(|v| *v = Some(id));
            navigator.push(Route::Studio {});
        }
        "media" => {
            let id = hit.id.clone();
            crate::state::media::OPEN_MEDIA.with_mut(|v| *v = Some(id));
            navigator.push(Route::Media {});
        }
        "layer" => {
            let id = hit.id.clone();
            crate::state::annotations::OPEN_LAYER.with_mut(|v| *v = Some(id));
            navigator.push(Route::Annotations {});
        }
        "function" => {
            crate::state::functions::OPEN_FUNCTION.with_mut(|v| *v = hit.id.parse::<i64>().ok());
            navigator.push(Route::Functions {});
        }
        "flow" => {
            navigator.push(Route::Flows { id: hit.id.clone() });
        }
        "dashboard" => {
            navigator.push(Route::Dashboards {
                id: hit.id.clone(),
                mode: String::new(),
            });
        }
        "edge_ref" => {
            navigator.push(Route::EdgeRefs {});
        }
        _ => {}
    }
}

/// One result row (icon + title + subtitle). Own navigator: the click jumps
/// straight to the object. `is_group_start` renders the group header above
/// the row.
#[component]
fn SearchHitRow(
    entity_type: String,
    hit: crate::api::search::SearchHit,
    active: bool,
    is_group_start: bool,
    on_navigate: Option<Callback<()>>,
) -> Element {
    let navigator = use_navigator();
    rsx! {
        if is_group_start {
            div { class: "px-3 pt-2 pb-1 text-[10px] font-semibold uppercase tracking-wider text-gray-400 bg-gray-50",
                {t!(search_group_key(& entity_type))}
            }
        }
        button {
            class: if active { "flex w-full items-center gap-3 px-3 py-2 text-left bg-blue-600/10" } else { "flex w-full items-center gap-3 px-3 py-2 text-left hover:bg-gray-100" },
            onclick: move |_| {
                open_hit(&navigator, &entity_type, &hit);
                if let Some(cb) = on_navigate {
                    cb.call(());
                }
            },
            HitIcon { entity_type: entity_type.clone() }
            span { class: "flex-1 min-w-0",
                div { class: "text-sm font-medium text-gray-900 truncate", {hit.title.clone()} }
                if let Some(sub) = &hit.subtitle {
                    div { class: "text-xs text-gray-500 truncate", {sub.clone()} }
                }
            }
        }
    }
}

/// Entity-type icon of a search hit row.
#[component]
fn HitIcon(entity_type: String) -> Element {
    rsx! {
        span { class: "flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-gray-100 text-gray-500",
            match entity_type.as_str() {
                "poi" => rsx! {
                    crate::components::icons::MapPin { class: "h-4 w-4" }
                },
                "tour" => rsx! {
                    crate::components::icons::Map { class: "h-4 w-4" }
                },
                "media" => rsx! {
                    crate::components::icons::Image { class: "h-4 w-4" }
                },
                "layer" => rsx! {
                    crate::components::icons::Layers { class: "h-4 w-4" }
                },
                "function" => rsx! {
                    crate::components::icons::Wrench { class: "h-4 w-4" }
                },
                "flow" => rsx! {
                    crate::components::icons::Activity { class: "h-4 w-4" }
                },
                "dashboard" => rsx! {
                    crate::components::icons::Gauge { class: "h-4 w-4" }
                },
                "edge_ref" => rsx! {
                    crate::components::icons::Database { class: "h-4 w-4" }
                },
                _ => rsx! {
                    crate::components::icons::Cpu { class: "h-4 w-4" }
                },
            }
        }
    }
}

/// True when row `idx` opens a new entity-type group (first row, or the
/// previous row has another type).
fn is_group_start(flat: &[(String, crate::api::search::SearchHit)], idx: usize) -> bool {
    idx == 0 || flat.get(idx - 1).is_some_and(|r| r.0 != flat[idx].0)
}
