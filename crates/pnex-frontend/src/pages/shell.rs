//! Root layout: fixed gray-900 sidebar on desktop, mobile drawer + session
//! guard (renders `Login` instead of the `Outlet` while the user is not
//! authenticated, no `/login` route).
//!
//! The sidebar footer holds the org selector (active tenant), the identity
//! and logout.

use crate::app::Route;
use crate::components::assistant::AssistantPanel;
use crate::components::gerbe::Gerbe;
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
                Gerbe { size: 44 }
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
        div { class: "min-h-screen bg-ground",
            // Drawer mobile — toujours déplié (w-64), le rail est un concept
            // desktop uniquement.
            if sidebar_open() {
                div { class: "fixed inset-0 z-50 lg:hidden",
                    div {
                        class: "pnex-backdrop-in fixed inset-0 bg-ink/55 dark:bg-black/60",
                        onclick: move |_| sidebar_open.set(false),
                    }
                    div { class: "pnex-drawer-in fixed inset-y-0 left-0 flex w-72 max-w-[85vw] flex-col bg-rail",
                        div { class: "flex h-16 items-center justify-between px-4",
                            SidebarBrand {}
                            button {
                                class: "inline-flex h-10 w-10 items-center justify-center rounded-md text-slate hover:bg-surface hover:text-ink",
                                aria_label: t!("common-close"),
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
                        div { class: "flex-1 min-h-0 overflow-y-auto sidebar-scroll px-3 py-4",
                            Nav {
                                on_navigate: Some(Callback::new(move |()| sidebar_open.set(false))),
                                rail: false,
                            }
                        }
                        SidebarFooter {
                            rail: false,
                            on_navigate: Some(Callback::new(move |()| sidebar_open.set(false))),
                        }
                    }
                }
            }

            // Sidebar desktop — dépliée w-64 ou rail d'icônes w-16 (classes
            // littérales complètes via match, exigence du scan Tailwind).
            div { class: if rail { "hidden lg:fixed lg:inset-y-0 lg:flex lg:w-16 lg:flex-col lg:bg-rail lg:border-r lg:border-line" } else { "hidden lg:fixed lg:inset-y-0 lg:flex lg:w-64 lg:flex-col lg:bg-rail lg:border-r lg:border-line" },
                // Rail toggle next to the logo, away from logout.
                if rail {
                    div { class: "flex flex-col items-center gap-1 px-3 pt-4 pb-2",
                        img {
                            src: asset!("/assets/logo-mark.png"),
                            alt: "PNeX",
                            class: "h-7 w-7",
                        }
                        div { class: "h-4", ActivityMark {} }
                        button {
                            class: "inline-flex h-8 w-8 items-center justify-center rounded-md text-slate hover:bg-surface hover:text-ink",
                            r#type: "button",
                            title: t!("shell-sidebar-expand"),
                            aria_label: t!("shell-sidebar-expand"),
                            onclick: move |_| ui::set(false),
                            crate::components::icons::PanelLeftOpen { class: "h-5 w-5" }
                        }
                    }
                } else {
                    div { class: "relative flex h-16 items-center justify-center px-3",
                        div { class: "absolute left-3 top-1/2 -translate-y-1/2", ActivityMark {} }
                        SidebarBrand {}
                        button {
                            class: "absolute right-2 top-2 inline-flex h-8 w-8 items-center justify-center rounded-md text-slate hover:bg-surface hover:text-ink",
                            r#type: "button",
                            title: t!("shell-sidebar-collapse"),
                            aria_label: t!("shell-sidebar-collapse"),
                            onclick: move |_| ui::toggle(),
                            crate::components::icons::PanelLeftClose { class: "h-4 w-4" }
                        }
                    }
                }
                // Global search (D69) — right below the logo; rail mode gets
                // an icon button that re-expands the sidebar.
                SidebarSearch { rail }
                // Défilement quand tous les groupes sont dépliés (le menu
                // dépasse l'écran sur web — constat user 2026-09-18).
                div { class: "flex-1 min-h-0 overflow-y-auto sidebar-scroll px-3 py-4",
                    Nav { rail }
                }
                SidebarFooter { rail, on_navigate: None }
            }

            div { class: if rail { "lg:pl-16" } else { "lg:pl-64" },
                // En-tête mobile
                // Hidden while a full-screen editor is open: its own bar
                // has the back button (saves 64 px of canvas on phones).
                div { class: if *ui::EDITORS_OPEN.read() > 0 { "hidden" } else { "sticky top-0 z-40 lg:hidden" },
                    div { class: "flex h-14 items-center justify-between border-b border-line bg-rail px-2",
                        button {
                            class: "inline-flex h-11 w-11 items-center justify-center rounded-md text-ink hover:bg-surface",
                            aria_label: t!("shell-menu"),
                            onclick: move |_| sidebar_open.set(true),
                            crate::components::icons::Menu { class: "h-6 w-6" }
                        }
                        div { class: "flex items-center", SidebarBrand {} }
                        div { class: "flex w-11 justify-center", ActivityMark {} }
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

/// Navigation entry. Active = surface fill + the logo's vermilion cross in
/// the gutter (design-system.md: the cross marks where you are). `icon`
/// only on top-level entries; group children are indented text.
#[component]
fn NavItem(
    to: Route,
    label: String,
    active: bool,
    #[props(default)] icon: Option<Element>,
    #[props(default)] rail: bool,
    #[props(default)] on_navigate: Option<Callback<()>>,
) -> Element {
    let class = match (active, rail, icon.is_some()) {
        (true, true, _) => "relative flex w-full items-center justify-center rounded-md py-2 bg-surface text-ink shadow-[0_0_0_1px_var(--color-line)]",
        (false, true, _) => "relative flex w-full items-center justify-center rounded-md py-2 text-slate hover:bg-surface hover:text-ink",
        (true, false, true) => "relative flex w-full items-center gap-3 rounded-md py-2 pl-3 pr-3 text-sm font-semibold bg-surface text-ink shadow-[0_0_0_1px_var(--color-line)]",
        (false, false, true) => "relative flex w-full items-center gap-3 rounded-md py-2 pl-3 pr-3 text-sm text-ink hover:bg-surface",
        (true, false, false) => "relative flex w-full items-center rounded-md py-1.5 pl-11 pr-3 text-sm font-semibold bg-surface text-ink shadow-[0_0_0_1px_var(--color-line)]",
        (false, false, false) => "relative flex w-full items-center rounded-md py-1.5 pl-11 pr-3 text-sm text-ink hover:bg-surface",
    };
    rsx! {
        Link {
            to,
            class,
            title: "{label}",
            aria_current: if active { "page" } else { "false" },
            onclick: move |_| {
                if let Some(cb) = on_navigate {
                    cb.call(());
                }
            },
            if active && !rail && icon.is_none() {
                NavMark {}
            }
            if let Some(icon) = icon {
                {icon}
            }
            if !rail {
                span { class: "truncate", "{label}" }
            }
        }
    }
}

/// The logo's cross, the "you are here" mark of the active child entry.
#[component]
fn NavMark() -> Element {
    rsx! {
        svg {
            class: "absolute left-5 top-1/2 h-2.5 w-2.5 -translate-y-1/2",
            view_box: "0 0 10 10",
            "aria-hidden": "true",
            path {
                d: "M1 1l8 8M9 1L1 9",
                stroke: "var(--color-mark)",
                "stroke-width": "2",
                "stroke-linecap": "round",
            }
        }
    }
}

/// Collapsible task group. Opens on mount when the current route is inside
/// it, never closes by itself (an effect writing the signal it reads loops:
/// UI freeze seen 2026-09-15). In rail mode a click re-expands the sidebar
/// on this group.
#[component]
fn NavGroup(label: String, icon: Element, active: bool, rail: bool, children: Element) -> Element {
    let mut open = use_signal(|| active);
    let class = match (rail, active) {
        (true, true) => "flex w-full items-center justify-center rounded-md py-2 text-ink bg-surface/60",
        (true, false) => "flex w-full items-center justify-center rounded-md py-2 text-slate hover:bg-surface hover:text-ink",
        (false, true) => "flex w-full items-center gap-3 rounded-md px-3 py-2 text-left text-sm font-semibold text-ink hover:bg-surface",
        (false, false) => "flex w-full items-center gap-3 rounded-md px-3 py-2 text-left text-sm font-medium text-ink hover:bg-surface",
    };
    rsx! {
        div { class: "space-y-0.5",
            button {
                class,
                r#type: "button",
                title: "{label}",
                aria_expanded: if open() && !rail { "true" } else { "false" },
                onclick: move |_| {
                    if rail {
                        ui::set(false);
                        open.set(true);
                    } else {
                        open.toggle();
                    }
                },
                {icon}
                if !rail {
                    span { class: "truncate", "{label}" }
                    if open() {
                        crate::components::icons::ChevronDown { class: "ml-auto h-4 w-4 text-slate" }
                    } else {
                        crate::components::icons::ChevronRight { class: "ml-auto h-4 w-4 text-slate" }
                    }
                }
            }
            if open() && !rail {
                div { class: "space-y-0.5", {children} }
            }
        }
    }
}

/// Side navigation, grouped by task (design-system.md §navigation): rendered
/// in the mobile drawer and in the desktop sidebar. `on_navigate` closes
/// the drawer after a click.
#[component]
fn Nav(on_navigate: Option<Callback<()>>, rail: bool) -> Element {
    let route = use_route::<Route>();
    let is_platform_admin = session::user().is_some_and(|u| u.platform_admin);
    let at = |r: Route| route == r;

    let monitor = matches!(route, Route::Dashboards { .. })
        || at(Route::Visualisation {})
        || at(Route::Map {})
        || at(Route::Sites {})
        || at(Route::Events {});
    let knowledge = matches!(
        route,
        Route::Docs { .. }
            | Route::Ontology { .. }
            | Route::OntologyObject { .. }
            | Route::OntologyType { .. }
            | Route::Cases {}
            | Route::CaseWorkspace { .. }
            | Route::Calendar {}
    );
    let media = at(Route::Media {})
        || at(Route::Cameras {})
        || at(Route::Streams {})
        || at(Route::VoiceId {})
        || matches!(route, Route::Lab {} | Route::LabProjectPage { .. })
        || at(Route::Studio {})
        || at(Route::Annotations {});
    let devices = at(Route::Devices {})
        || at(Route::Catalog {})
        || at(Route::EdgeRefs {})
        || at(Route::Firmware {});
    let automation = matches!(route, Route::Flows { .. })
        || at(Route::Functions {})
        || at(Route::Controls {})
        || at(Route::Notifications {})
        || at(Route::Models {})
        || at(Route::FluidMixtures {});
    let settings = matches!(route, Route::Orgs {} | Route::OrgsCurrent {})
        || at(Route::Secrets {})
        || at(Route::System {})
        || at(Route::AdminStatus {});
    let icon = "h-5 w-5 shrink-0";

    rsx! {
        nav { class: "flex-1 space-y-1",
            NavItem {
                to: Route::Dashboard {},
                label: t!("nav-dashboard"),
                active: at(Route::Dashboard {}),
                icon: rsx! {
                    crate::components::icons::Home { class: icon }
                },
                rail,
                on_navigate,
            }
            NavGroup {
                label: t!("nav-group-monitor"),
                active: monitor,
                rail,
                icon: rsx! {
                    crate::components::icons::Gauge { class: icon }
                },
                NavItem {
                    to: Route::Dashboards {
                        id: String::new(),
                        mode: String::new(),
                    },
                    label: t!("nav-dashboards"),
                    active: matches!(route, Route::Dashboards { .. }),
                    on_navigate,
                }
                NavItem {
                    to: Route::Visualisation {},
                    label: t!("nav-quick-charts"),
                    active: at(Route::Visualisation {}),
                    on_navigate,
                }
                NavItem {
                    to: Route::Map {},
                    label: t!("nav-map"),
                    active: at(Route::Map {}),
                    on_navigate,
                }
                NavItem {
                    to: Route::Sites {},
                    label: t!("nav-sites"),
                    active: at(Route::Sites {}),
                    on_navigate,
                }
                NavItem {
                    to: Route::Events {},
                    label: t!("nav-events"),
                    active: at(Route::Events {}),
                    on_navigate,
                }
            }
            NavGroup {
                label: t!("nav-group-knowledge"),
                active: knowledge,
                rail,
                icon: rsx! {
                    crate::components::icons::BookOpen { class: icon }
                },
                NavItem {
                    to: Route::Docs { id: String::new() },
                    label: t!("nav-docs"),
                    active: matches!(route, Route::Docs { .. }),
                    on_navigate,
                }
                NavItem {
                    to: Route::Ontology {
                        tab: String::new(),
                        type_key: String::new(),
                    },
                    label: t!("nav-ontology"),
                    active: matches!(
                        route,
                        Route::Ontology { .. }
                        | Route::OntologyObject { .. }
                        | Route::OntologyType { .. }
                    ),
                    on_navigate,
                }
                NavItem {
                    to: Route::Cases {},
                    label: t!("nav-cases"),
                    active: matches!(route, Route::Cases {} | Route::CaseWorkspace { .. }),
                    on_navigate,
                }
                NavItem {
                    to: Route::Calendar {},
                    label: t!("nav-calendar"),
                    active: at(Route::Calendar {}),
                    on_navigate,
                }
            }
            NavGroup {
                label: t!("nav-group-media"),
                active: media,
                rail,
                icon: rsx! {
                    crate::components::icons::Image { class: icon }
                },
                NavItem {
                    to: Route::Media {},
                    label: t!("nav-library"),
                    active: at(Route::Media {}),
                    on_navigate,
                }
                NavItem {
                    to: Route::Cameras {},
                    label: t!("nav-cameras"),
                    active: at(Route::Cameras {}),
                    on_navigate,
                }
                NavItem {
                    to: Route::Streams {},
                    label: t!("nav-streams"),
                    active: at(Route::Streams {}),
                    on_navigate,
                }
                NavItem {
                    to: Route::VoiceId {},
                    label: t!("nav-voice-id"),
                    active: at(Route::VoiceId {}),
                    on_navigate,
                }
                NavItem {
                    to: Route::Lab {},
                    label: t!("nav-lab"),
                    active: matches!(route, Route::Lab {} | Route::LabProjectPage { .. }),
                    on_navigate,
                }
                NavItem {
                    to: Route::Studio {},
                    label: t!("nav-studio"),
                    active: at(Route::Studio {}),
                    on_navigate,
                }
                NavItem {
                    to: Route::Annotations {},
                    label: t!("nav-annotations"),
                    active: at(Route::Annotations {}),
                    on_navigate,
                }
            }
            NavGroup {
                label: t!("nav-group-devices"),
                active: devices,
                rail,
                icon: rsx! {
                    crate::components::icons::Router { class: icon }
                },
                NavItem {
                    to: Route::Devices {},
                    label: t!("nav-devices"),
                    active: at(Route::Devices {}),
                    on_navigate,
                }
                NavItem {
                    to: Route::Catalog {},
                    label: t!("nav-catalog"),
                    active: at(Route::Catalog {}),
                    on_navigate,
                }
                NavItem {
                    to: Route::Firmware {},
                    label: t!("nav-firmware"),
                    active: at(Route::Firmware {}),
                    on_navigate,
                }
                NavItem {
                    to: Route::EdgeRefs {},
                    label: t!("nav-edge-refs"),
                    active: at(Route::EdgeRefs {}),
                    on_navigate,
                }
            }
            NavGroup {
                label: t!("nav-automation"),
                active: automation,
                rail,
                icon: rsx! {
                    crate::components::icons::Workflow { class: icon }
                },
                NavItem {
                    to: Route::Flows { id: String::new() },
                    label: t!("nav-flows"),
                    active: matches!(route, Route::Flows { .. }),
                    on_navigate,
                }
                NavItem {
                    to: Route::Functions {},
                    label: t!("nav-functions"),
                    active: at(Route::Functions {}),
                    on_navigate,
                }
                NavItem {
                    to: Route::Controls {},
                    label: t!("nav-controls"),
                    active: at(Route::Controls {}),
                    on_navigate,
                }
                NavItem {
                    to: Route::Notifications {},
                    label: t!("nav-notifications"),
                    active: at(Route::Notifications {}),
                    on_navigate,
                }
                NavItem {
                    to: Route::Models {},
                    label: t!("nav-models"),
                    active: at(Route::Models {}),
                    on_navigate,
                }
                NavItem {
                    to: Route::FluidMixtures {},
                    label: t!("nav-mixtures"),
                    active: at(Route::FluidMixtures {}),
                    on_navigate,
                }
            }
            div { class: "my-3 border-t border-line" }
            NavGroup {
                label: t!("nav-group-settings"),
                active: settings,
                rail,
                icon: rsx! {
                    crate::components::icons::Settings { class: icon }
                },
                NavItem {
                    to: Route::OrgsCurrent {},
                    label: t!("nav-orgs"),
                    active: matches!(route, Route::Orgs {} | Route::OrgsCurrent {}),
                    on_navigate,
                }
                NavItem {
                    to: Route::Secrets {},
                    label: t!("nav-secrets"),
                    active: at(Route::Secrets {}),
                    on_navigate,
                }
                NavItem {
                    to: Route::System {},
                    label: t!("nav-system"),
                    active: at(Route::System {}),
                    on_navigate,
                }
                if is_platform_admin {
                    NavItem {
                        to: Route::AdminStatus {},
                        label: t!("nav-admin-status"),
                        active: at(Route::AdminStatus {}),
                        on_navigate,
                    }
                }
            }
        }
    }
}

/// The loading mark while API requests are in flight (D224): data arriving,
/// shown only past 250 ms so quick calls never flash.
#[component]
fn ActivityMark() -> Element {
    if *ui::IN_FLIGHT.read() == 0 {
        return rsx! {};
    }
    rsx! {
        Gerbe { size: 24, delayed: Some(true) }
    }
}

#[component]
fn SidebarBrand() -> Element {
    // Navy letters on the light rail; the white variant on the night theme.
    rsx! {
        img {
            src: asset!("/assets/logo.png"),
            alt: "PNeX",
            class: "h-9 w-auto dark:hidden",
        }
        img {
            src: asset!("/assets/logo-light.png"),
            alt: "PNeX",
            class: "hidden h-9 w-auto dark:block",
        }
    }
}

/// Sidebar footer: assistant, org selector, identity (opens the profile)
/// and logout (the rail toggle sits by the logo).
#[component]
fn SidebarFooter(rail: bool, on_navigate: Option<Callback<()>>) -> Element {
    let route = use_route::<Route>();
    let identity = session::user()
        .map(|user| user.full_name.or(user.email).unwrap_or(user.username))
        .unwrap_or_default();
    let mut confirm_logout = use_signal(|| false);
    let logout_title = t!("shell-logout");
    let ai_title = t!("ai-title");
    // Logout in the mark colour of the active menu entry.
    let logout_btn = "inline-flex h-9 w-9 shrink-0 items-center justify-center rounded-md text-mark hover:bg-surface transition-colors";
    rsx! {
        div { class: "space-y-2 border-t border-line p-3",
            if crate::state::ai::visible() {
                button {
                    class: if rail { "flex w-full items-center justify-center rounded-md py-2 text-slate hover:bg-surface hover:text-ink" } else { "flex w-full items-center gap-3 rounded-md px-3 py-2 text-left text-sm text-ink hover:bg-surface" },
                    r#type: "button",
                    title: "{ai_title}",
                    aria_label: "{ai_title}",
                    onclick: move |_| {
                        crate::components::assistant::open();
                        if let Some(cb) = on_navigate {
                            cb.call(());
                        }
                    },
                    crate::components::icons::Sparkles { class: "h-5 w-5 shrink-0" }
                    if !rail {
                        span { "{ai_title}" }
                    }
                }
            }
            if rail {
                div { class: "flex flex-col items-center gap-1",
                    button {
                        class: logout_btn,
                        title: "{logout_title}",
                        onclick: move |_| confirm_logout.set(true),
                        crate::components::icons::LogOut { class: "h-4 w-4" }
                    }
                }
            } else {
                OrgSwitcher {}
                div { class: "flex items-center gap-1",
                    Link {
                        to: Route::Profile {},
                        class: if route == (Route::Profile {}) { "min-w-0 flex-1 truncate rounded-md px-2 py-2 text-sm font-semibold text-ink bg-surface" } else { "min-w-0 flex-1 truncate rounded-md px-2 py-2 text-sm text-slate hover:bg-surface hover:text-ink" },
                        title: t!("nav-profile"),
                        onclick: move |_| {
                            if let Some(cb) = on_navigate {
                                cb.call(());
                            }
                        },
                        {identity}
                    }
                    button {
                        class: logout_btn,
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
                    class: "flex items-center justify-center rounded-md px-2 py-2 text-slate hover:bg-surface hover:text-ink transition-colors",
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
                    class: "w-full rounded-md border border-line bg-surface py-2 pl-9 pr-3 text-sm text-ink placeholder:text-gray-500 focus:border-signal focus:outline-none focus:ring-3 focus:ring-signal-200",
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
        "label" => "search-group-label",
        "media" => "search-group-media",
        "document" => "search-group-document",
        "layer" => "search-group-layer",
        "function" => "search-group-function",
        "flow" => "search-group-flow",
        "dashboard" => "search-group-dashboard",
        "edge_ref" => "search-group-edge_ref",
        "object" => "search-group-object",
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
        // A labelled object of any kind (D218): opened like its own group.
        "label" => {
            use pnex_core::resources as r;
            let kind = hit.kind.clone().unwrap_or_default();
            let same_as = match kind.as_str() {
                k if k == r::KIND_DEVICE => Some("device"),
                k if k == r::KIND_MEDIA_ASSET => Some("media"),
                k if k == r::KIND_TOUR => Some("tour"),
                k if k == r::KIND_MAP_PIN => Some("poi"),
                k if k == r::KIND_ANNOTATION_LAYER => Some("layer"),
                k if k == r::KIND_FUNCTION => Some("function"),
                k if k == r::KIND_FLOW => Some("flow"),
                k if k == r::KIND_DASHBOARD => Some("dashboard"),
                _ => None,
            };
            if let Some(group) = same_as {
                open_hit(navigator, group, hit);
                return;
            }
            match kind.as_str() {
                k if k == r::KIND_DOC => {
                    navigator.push(Route::Docs { id: hit.id.clone() });
                }
                k if k == r::KIND_CONTROL => {
                    navigator.push(Route::Controls {});
                }
                k if k == r::KIND_FIRMWARE_PROJECT => {
                    navigator.push(Route::Firmware {});
                }
                k if k == r::KIND_MEDIA_STREAM => {
                    navigator.push(Route::Streams {});
                }
                k if k == r::KIND_ML_MODEL => {
                    navigator.push(Route::Models {});
                }
                k if k == r::KIND_NOTIFY_CHANNEL || k == r::KIND_NOTIFY_TEMPLATE => {
                    navigator.push(Route::Notifications {});
                }
                k if k == r::KIND_FOLDER => {
                    navigator.push(Route::Sites {});
                }
                "case" => {
                    navigator.push(Route::CaseWorkspace { id: hit.id.clone() });
                }
                _ => {
                    navigator.push(Route::OntologyObject { id: hit.id.clone() });
                }
            }
        }
        "device" => {
            crate::state::devices::OPEN_DEVICE.with_mut(|v| *v = hit.id.parse::<i64>().ok());
            navigator.push(Route::Devices {});
        }
        "poi" => {
            let id = hit.id.clone();
            crate::state::map::OPEN_POI.with_mut(|v| *v = Some(id));
            navigator.push(Route::Sites {});
        }
        "tour" => {
            let id = hit.id.clone();
            crate::state::tours::OPEN_TOUR.with_mut(|v| *v = Some(id));
            navigator.push(Route::Studio {});
        }
        "media" | "document" => {
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
        "object" => {
            navigator.push(Route::OntologyObject { id: hit.id.clone() });
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
            div { class: "px-3 pt-2 pb-1 text-xs font-semibold text-gray-400 bg-gray-50",
                {t!(search_group_key(& entity_type))}
            }
        }
        button {
            class: if active { "flex w-full items-center gap-3 px-3 py-2 text-left bg-signal-600/10" } else { "flex w-full items-center gap-3 px-3 py-2 text-left hover:bg-gray-100" },
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
                "label" => rsx! {
                    crate::components::icons::Tag { class: "h-4 w-4" }
                },
                "document" => rsx! {
                    crate::components::icons::BookOpen { class: "h-4 w-4" }
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
                "object" => rsx! {
                    crate::components::icons::Shapes { class: "h-4 w-4" }
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
