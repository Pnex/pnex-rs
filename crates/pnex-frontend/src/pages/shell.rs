//! Layout racine — porté du `Layout.tsx` React : sidebar grise-900 fixe en
//! desktop, drawer mobile + garde de session (rend `Login` à la place de
//! l'`Outlet` tant que l'utilisateur n'est pas authentifié — parité
//! `AuthWrapper` React, pas de route `/login`).
//!
//! Le pied de sidebar porte le sélecteur d'org (tenant actif), l'identité et
//! la déconnexion — concepts multi-tenant absents de l'UI d'origine.

use crate::app::Route;
use crate::components::org_switcher::OrgSwitcher;
use crate::components::toasts::ToastContainer;
use crate::state::session::{self, SessionState, SESSION};
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
        SessionState::LoggedOut => rsx! { crate::pages::login::Login {} },
        SessionState::Authenticated { .. } => rsx! {
            ShellContent {}
            ToastContainer {}
        },
    }
}

#[component]
fn ShellContent() -> Element {
    let mut sidebar_open = use_signal(|| false);

    rsx! {
        div { class: "min-h-screen bg-gray-50",
            // Drawer mobile
            if sidebar_open() {
                div { class: "fixed inset-0 z-50 lg:hidden",
                    div {
                        class: "fixed inset-0 bg-gray-600 bg-opacity-75",
                        onclick: move |_| sidebar_open.set(false)
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
                        div { class: "flex-1 px-4 py-6", Nav {} }
                        SidebarFooter {}
                    }
                }
            }

            // Sidebar desktop
            div { class: "hidden lg:fixed lg:inset-y-0 lg:flex lg:w-64 lg:flex-col lg:bg-gray-900",
                div { class: "flex h-16 items-center px-6", SidebarBrand {} }
                div { class: "flex-1 px-6 py-6", Nav {} }
                SidebarFooter {}
            }

            div { class: "lg:pl-64",
                // En-tête mobile
                div { class: "sticky top-0 z-40 lg:hidden",
                    div { class: "flex h-16 items-center justify-between bg-white px-4 shadow-sm",
                        button {
                            class: "text-gray-500 hover:text-gray-600",
                            onclick: move |_| sidebar_open.set(true),
                            crate::components::icons::Menu { class: "h-6 w-6" }
                        }
                        div { class: "flex items-center space-x-3",
                            crate::components::icons::Zap { class: "h-6 w-6 text-blue-600" }
                            span { class: "text-lg font-bold text-gray-900", {t!("app-name")} }
                        }
                        div {}
                    }
                }
                main { class: "flex-1", Outlet::<Route> {} }
            }
        }
    }
}

/// Classes de navigation : littéraux complets pour le scan Tailwind (jamais
/// de classes construites dynamiquement).
fn nav_class(active: bool) -> &'static str {
    if active {
        "flex w-full items-center space-x-3 rounded-lg px-3 py-2 text-left bg-blue-600 text-white transition-colors"
    } else {
        "flex w-full items-center space-x-3 rounded-lg px-3 py-2 text-left text-gray-300 hover:bg-gray-800 hover:text-white transition-colors"
    }
}

/// Navigation latérale — composant dédié pour être rendu dans le drawer ET la
/// sidebar desktop.
#[component]
fn Nav() -> Element {
    let route = use_route::<Route>();
    rsx! {
        nav { class: "flex-1 space-y-2",
            Link { to: Route::Dashboard {}, class: nav_class(route == Route::Dashboard {}),
                crate::components::icons::Home { class: "h-5 w-5" }
                span { {t!("nav-dashboard")} }
            }
            Link { to: Route::Devices {}, class: nav_class(route == Route::Devices {}),
                crate::components::icons::Cpu { class: "h-5 w-5" }
                span { {t!("nav-devices")} }
            }
            Link { to: Route::Catalog {}, class: nav_class(route == Route::Catalog {}),
                crate::components::icons::Package { class: "h-5 w-5" }
                span { {t!("nav-catalog")} }
            }
            Link { to: Route::Orgs {}, class: nav_class(route == Route::Orgs {}),
                crate::components::icons::Building { class: "h-5 w-5" }
                span { {t!("nav-orgs")} }
            }
            Link { to: Route::Profile {}, class: nav_class(route == Route::Profile {}),
                crate::components::icons::User { class: "h-5 w-5" }
                span { {t!("nav-profile")} }
            }
        }
    }
}

#[component]
fn SidebarBrand() -> Element {
    rsx! {
        div { class: "flex items-center space-x-3",
            crate::components::icons::Zap { class: "h-8 w-8 text-blue-400" }
            span { class: "text-xl font-bold text-white", {t!("app-name")} }
        }
    }
}

#[component]
fn SidebarFooter() -> Element {
    let identity = session::user()
        .map(|user| {
            user.full_name
                .or(user.email)
                .unwrap_or(user.username)
        })
        .unwrap_or_default();
    rsx! {
        div { class: "p-4 border-t border-gray-800 space-y-3",
            OrgSwitcher {}
            div { class: "flex items-center justify-between gap-2 px-1",
                span { class: "text-xs text-gray-400 truncate", {identity} }
                button {
                    class: "text-gray-400 hover:text-white transition-colors",
                    title: t!("shell-logout"),
                    onclick: move |_| session::logout(),
                    crate::components::icons::LogOut { class: "h-4 w-4" }
                }
            }
        }
    }
}
