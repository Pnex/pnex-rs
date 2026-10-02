//! Organisations — page nouvelle (multi-tenant Phase 3) : liste, création,
//! sélection du tenant actif, détail (membres, rôles, renommage, suppression).
//!
//! Le détail est piloté par un signal local `selected` (pas de route
//! paramétrée — les props de route ne redémarrent pas `use_resource` ;
//! le `key: "{id}"` force le remontage du détail au changement d'org).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::components::crud::filters::RefreshButton;
use crate::components::crud::layout::ListLayout;
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::icons;
use crate::state::{org, toasts};

/// Valeur texte d'un champ de formulaire (`FormValue::Text`).
pub fn field(event: &dioxus::events::FormEvent, name: &str) -> String {
    event
        .values()
        .iter()
        .find(|(key, _)| key == name)
        .and_then(|(_, value)| match value {
            dioxus::events::FormValue::Text(text) => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

#[component]
pub fn Orgs() -> Element {
    rsx! {
        OrgsPage { initial: None }
    }
}

/// `/orgs/current`: opens straight on the current org's detail (members,
/// LLM providers) — deep-link target of the secrets "used by" chips.
#[component]
pub fn OrgsCurrent() -> Element {
    rsx! {
        OrgsPage { initial: org::current() }
    }
}

#[component]
fn OrgsPage(initial: Option<i64>) -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut selected = use_signal(|| initial);
    let mut new_name = use_signal(String::new);

    let list = use_resource(move || async move {
        let _ = reload();
        api::orgs::list().await
    });

    // Lecture synchrone de la ressource (doctrine socle CRUD).
    let (list_state, is_empty, rows) = match &*list.value().read() {
        None => (None, false, Vec::new()),
        Some(Ok(paged)) => (
            Some(Ok(())),
            paged.results.is_empty(),
            paged.results.clone(),
        ),
        Some(Err(err)) => (Some(Err(err.clone())), false, Vec::new()),
    };

    // Colonnes de la table — les closures d'action capturent le signal
    // `selected` de la page ; le badge « current » lit org::current() au
    // rendu (boucle de rendu du parent, donc réactif).
    let columns = vec![
        Column::new(t!("orgs-col-name").to_string(), move |summary: &pnex_core::OrgSummary| {
            let is_current = org::current() == Some(summary.id);
            rsx! {
                span { class: "mr-2", {summary.name.clone()} }
                if is_current {
                    span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium bg-green-100 text-green-800",
                        {t!("orgs-current")}
                    }
                }
            }
        })
        .with_td_class("font-medium text-gray-900"),
        Column::new(t!("orgs-col-role").to_string(), |summary: &pnex_core::OrgSummary| {
            let (role_badge_class, role_label) = role_badge(&summary.role);
            rsx! {
                span { class: "inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium {role_badge_class}",
                    {role_label}
                }
            }
        }),
        Column::new(t!("orgs-col-tier").to_string(), |summary: &pnex_core::OrgSummary| {
            rsx! { {summary.subscription_tier.clone().unwrap_or_else(|| "—".into())} }
        })
        .with_td_class("text-gray-600"),
        Column::new(
            t!("common-actions").to_string(),
            move |summary: &pnex_core::OrgSummary| {
                let org_id = summary.id;
                let is_current = org::current() == Some(org_id);
                rsx! {
                    div { class: "flex items-center gap-2",
                        if !is_current {
                            button {
                                class: "px-3 py-1 text-sm bg-gray-100 text-gray-700 rounded-lg hover:bg-gray-200 transition-colors",
                                onclick: move |_| org::set(org_id),
                                {t!("orgs-select")}
                            }
                        }
                        button {
                            class: "px-3 py-1 text-sm bg-blue-100 text-blue-700 rounded-lg hover:bg-blue-200 transition-colors",
                            onclick: move |_| selected.set(Some(org_id)),
                            {t!("orgs-manage")}
                        }
                    }
                }
            },
        ),
    ];

    rsx! {
        ListLayout {
            title: t!("orgs-title").to_string(),
            subtitle: Some(t!("orgs-subtitle").to_string()),
            can_write: true,
            // Harmonisation socle : rafraîchissement manuel dans l'en-tête
            // (la page n'a pas de barre de filtres).
            actions: rsx! {
                RefreshButton { on_click: move |_| reload.with_mut(|r| *r += 1) }
            },
            // Création
            form {
                class: "mb-6 flex gap-2",
                onsubmit: move |event| {
                    // Bloque la soumission native (rechargement du SPA).
                    event.prevent_default();
                    let name = field(&event, "name");
                    let name = name.trim().to_string();
                    if name.is_empty() {
                        return;
                    }
                    new_name.set(String::new());
                    spawn(async move {
                        match api::orgs::create(&name).await {
                            Ok(created) => {
                                toasts::success("toast-saved");
                                org::set(created.id);
                            }
                            Err(err) => toasts::error(err),
                        }
                        // recharge la liste (la ressource lit `reload`)
                        reload.with_mut(|r| *r += 1);
                    });
                },
                input {
                    class: "flex-1 px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-transparent",
                    r#type: "text",
                    name: "name",
                    placeholder: t!("orgs-new-placeholder"),
                    value: "{new_name}",
                }
                button {
                    class: "inline-flex items-center px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors",
                    r#type: "submit",
                    icons::Plus { class: "h-4 w-4 mr-2" }
                    {t!("orgs-create")}
                }
            }

            match selected() {
                // Détail (remonté à chaque changement d'id via key)
                Some(org_id) => rsx! {
                    OrgDetail {
                        key: "{org_id}",
                        org_id,
                        on_back: move |_| selected.set(None),
                        on_changed: move |_| reload.with_mut(|r| *r += 1),
                    }
                },
                None => rsx! {
                    ListStates {
                        state: list_state,
                        is_empty,
                        empty_message: t!("orgs-empty").to_string(),
                        DataTable {
                            columns,
                            rows,
                            row_key: RowKey::new(|summary: &pnex_core::OrgSummary| summary.id.to_string()),
                        }
                    }
                },
            }
        }
    }
}

/// Badge de rôle — classes littérales complètes (scan Tailwind).
pub fn role_badge(role: &str) -> (&'static str, String) {
    let label = match role {
        "owner" => t!("role-owner"),
        "admin" => t!("role-admin"),
        "member" => t!("role-member"),
        _ => t!("role-viewer"),
    };
    let badge = match role {
        "owner" => "bg-blue-100 text-blue-800",
        "admin" => "bg-amber-100 text-amber-800",
        "member" => "bg-green-100 text-green-800",
        _ => "bg-gray-100 text-gray-800",
    };
    (badge, label)
}

#[component]
fn OrgDetail(org_id: i64, on_back: Callback<()>, on_changed: Callback<()>) -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut rename = use_signal(String::new);
    let mut member_email = use_signal(String::new);
    let mut member_role = use_signal(|| "viewer".to_string());
    let mut confirm_delete = use_signal(|| false);

    let detail = use_resource(move || async move {
        let _ = reload();
        api::orgs::detail(org_id).await
    });

    let refresh = Callback::new(move |_: ()| {
        reload.with_mut(|r| *r += 1);
        on_changed.call(());
    });

    match &*detail.value().read() {
        Some(Ok(detail)) => {
            let can_write = crate::state::org::role_can_administer(&detail.role);
            let is_owner = detail.role == "owner";
            let members = detail.members.clone();
            let name = detail.name.clone();
            rsx! {
                div { class: "bg-white rounded-lg shadow-sm",
                    // En-tête
                    div { class: "p-6 border-b border-gray-200 flex items-center justify-between gap-4",
                        div {
                            button {
                                class: "text-sm text-blue-600 hover:text-blue-700 mb-1",
                                onclick: move |_| on_back.call(()),
                                {"← "}
                                {t!("orgs-back")}
                            }
                            h2 { class: "text-xl font-semibold text-gray-900", {name} }
                        }
                        if is_owner {
                            button {
                                class: "px-3 py-1.5 text-sm font-medium text-red-700 bg-red-50 border border-red-200 rounded-lg hover:bg-red-100 transition-colors",
                                onclick: move |_| confirm_delete.set(true),
                                icons::Trash2 { class: "h-4 w-4 inline mr-1" }
                                {t!("orgs-delete")}
                            }
                        }
                    }

                    // Renommage (owner/admin)
                    if can_write {
                        form {
                            class: "p-6 border-b border-gray-200 flex gap-2",
                            onsubmit: move |event| {
                                // Bloque la soumission native (rechargement du SPA).
                                event.prevent_default();
                                let value = field(&event, "name");
                                let value = value.trim().to_string();
                                if value.is_empty() {
                                    return;
                                }
                                rename.set(String::new());
                                let org_id = org_id;
                                spawn(async move {
                                    match api::orgs::rename(org_id, &value).await {
                                        Ok(()) => toasts::success("toast-saved"),
                                        Err(err) => toasts::error(err),
                                    }
                                    refresh(());
                                });
                            },
                            input {
                                class: "flex-1 px-3 py-2 border border-gray-300 rounded-lg text-sm",
                                r#type: "text",
                                name: "name",
                                placeholder: t!("orgs-rename-placeholder"),
                                value: "{rename}",
                            }
                            button {
                                class: "px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors text-sm",
                                r#type: "submit",
                                {t!("orgs-rename")}
                            }
                        }
                    }

                    // LLM providers of the org (managed by owner/admin)
                    div { class: "p-6 border-b border-gray-200",
                        crate::components::llm_providers::LlmProviders { can_manage: can_write }
                    }

                    // Ajout de membre (owner/admin)
                    if can_write {
                        form {
                            class: "p-6 border-b border-gray-200 flex flex-wrap gap-2",
                            onsubmit: move |event| {
                                // Bloque la soumission native (rechargement du SPA).
                                event.prevent_default();
                                let email = field(&event, "email");
                                let email = email.trim().to_string();
                                if email.is_empty() {
                                    return;
                                }
                                member_email.set(String::new());
                                let role = member_role.cloned();
                                let org_id = org_id;
                                spawn(async move {
                                    match api::orgs::add_member(org_id, &email, &role).await {
                                        Ok(_) => toasts::success("toast-saved"),
                                        Err(err) => toasts::error(err),
                                    }
                                    refresh(());
                                });
                            },
                            input {
                                class: "flex-1 min-w-48 px-3 py-2 border border-gray-300 rounded-lg text-sm",
                                r#type: "email",
                                name: "email",
                                placeholder: t!("orgs-email-placeholder"),
                                value: "{member_email}",
                            }
                            select {
                                class: "px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                                onchange: move |event| member_role.set(event.value()),
                                option {
                                    value: "viewer",
                                    selected: member_role() == "viewer",
                                    {t!("role-viewer")}
                                }
                                option {
                                    value: "member",
                                    selected: member_role() == "member",
                                    {t!("role-member")}
                                }
                                option {
                                    value: "admin",
                                    selected: member_role() == "admin",
                                    {t!("role-admin")}
                                }
                                option {
                                    value: "owner",
                                    selected: member_role() == "owner",
                                    {t!("role-owner")}
                                }
                            }
                            button {
                                class: "px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors text-sm",
                                r#type: "submit",
                                icons::Plus { class: "h-4 w-4 inline mr-1" }
                                {t!("orgs-add-member")}
                            }
                        }
                    }

                    // Membres
                    div { class: "p-6",
                        h3 { class: "text-sm font-semibold text-gray-500 uppercase tracking-wider mb-4",
                            {t!("orgs-members")}
                        }
                        ul { class: "mb-4 space-y-1 text-xs text-gray-500",
                            li {
                                span { class: "font-medium text-gray-700", {t!("role-owner")} }
                                " — "
                                {t!("orgs-role-help-owner")}
                            }
                            li {
                                span { class: "font-medium text-gray-700", {t!("role-admin")} }
                                " — "
                                {t!("orgs-role-help-admin")}
                            }
                            li {
                                span { class: "font-medium text-gray-700", {t!("role-member")} }
                                " — "
                                {t!("orgs-role-help-member")}
                            }
                            li {
                                span { class: "font-medium text-gray-700", {t!("role-viewer")} }
                                " — "
                                {t!("orgs-role-help-viewer")}
                            }
                        }
                        div { class: "space-y-2",
                            for member in members {
                                {member_row(member, org_id, can_write, refresh, reload)}
                            }
                        }
                    }
                }

                if confirm_delete() {
                    crate::components::confirm::ConfirmDialog {
                        title: t!("orgs-confirm-delete-title"),
                        message: t!("orgs-confirm-delete-message"),
                        confirm_label: t!("orgs-delete"),
                        on_confirm: move |_| {
                            confirm_delete.set(false);
                            let org_id = org_id;
                            spawn(async move {
                                match api::orgs::delete(org_id).await {
                                    Ok(()) => toasts::success("toast-saved"),
                                    Err(err) => toasts::error(err),
                                }
                                org::clear();
                                refresh(());
                            });
                            on_back.call(());
                        },
                        on_cancel: move |_| confirm_delete.set(false),
                    }
                }
            }
        }
        Some(Err(err)) => rsx! {
            div { class: "bg-red-50 border border-red-200 rounded-lg p-4 text-sm text-red-700",
                {err.message.clone()}
            }
        },
        None => rsx! {
            div { class: "text-center py-12",
                span { class: "animate-spin inline-block rounded-full h-8 w-8 border-b-2 border-blue-600" }
            }
        },
    }
}

/// Ligne membre : identité, rôle (select si droit d'écriture), retrait.
fn member_row(
    member: pnex_core::OrgMember,
    org_id: i64,
    can_write: bool,
    on_changed: Callback<()>,
    mut reload: Signal<u32>,
) -> Element {
    let user_id = member.user_id;
    let role = member.role.clone();
    let display = member
        .full_name
        .clone()
        .or_else(|| member.email.clone())
        .unwrap_or_else(|| format!("#{user_id}"));
    let email = member.email.clone().unwrap_or_default();

    rsx! {
        div {
            key: "{user_id}",
            class: "flex items-center justify-between p-3 bg-gray-50 rounded-lg",
            div {
                p { class: "text-sm font-medium text-gray-900", {display} }
                p { class: "text-xs text-gray-500", {email} }
            }
            div { class: "flex items-center gap-2",
                if can_write {
                    select {
                        class: "px-2 py-1 border border-gray-300 rounded-lg text-sm bg-white",
                        onchange: move |event| {
                            let new_role = event.value();
                            spawn(async move {
                                match api::orgs::update_member(org_id, user_id, &new_role).await {
                                    Ok(()) => toasts::success("toast-saved"),
                                    Err(err) => toasts::error(err),
                                }
                                reload.with_mut(|r| *r += 1);
                                on_changed.call(());
                            });
                        },
                        option { value: "viewer", selected: role == "viewer", {t!("role-viewer")} }
                        option { value: "member", selected: role == "member", {t!("role-member")} }
                        option { value: "admin", selected: role == "admin", {t!("role-admin")} }
                        option { value: "owner", selected: role == "owner", {t!("role-owner")} }
                    }
                    button {
                        class: "p-2 text-red-600 hover:bg-red-100 rounded-lg transition-colors",
                        title: t!("orgs-remove-member"),
                        onclick: move |_| {
                            spawn(async move {
                                match api::orgs::remove_member(org_id, user_id).await {
                                    Ok(()) => toasts::success("toast-saved"),
                                    Err(err) => toasts::error(err),
                                }
                                reload.with_mut(|r| *r += 1);
                                on_changed.call(());
                            });
                        },
                        icons::Trash2 { class: "h-4 w-4" }
                    }
                } else {
                    {
                        let (badge, label) = role_badge(&role);
                        rsx! {
                            span { class: "inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium {badge}",
                                {label}
                            }
                        }
                    }
                }
            }
        }
    }
}
