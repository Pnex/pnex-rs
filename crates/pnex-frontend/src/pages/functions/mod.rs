//! Registre « Fonctions » (groupe « Automation ») — fonctions utilisateur
//! versionnées (js | starlark), test en live. Liste + éditeur en sous-vue
//! (signal `selected` — pattern `flows.rs`) ; le scoping org vient du client
//! (`X-Org-Id`), l'écriture est réservée owner/admin (le serveur force,
//! l'UI masque).
//!
//! L'aperçu de signature est calculé **dans le navigateur** via
//! `pnex_core::parse_directives` (pnex-core est wasm32) : la même extraction
//! que le save serveur, affichée en live (erreurs de directive ligne par
//! ligne). Le test en live passe par l'API (`POST /{id}/test`, code ad-hoc =
//! contenu du textarea — testable **avant** save).

use std::collections::BTreeMap;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{
    parse_directives, CreateFunction, FunctionDetail, FunctionLanguage, FunctionSummary,
    FunctionTestAdHoc, FunctionTestRequest, FunctionTestResponse, FunctionType,
};

use crate::api;
use crate::components::badges::date_label;
use crate::components::code_highlight::{AreaHandle, FunctionCodeEditor, LineMark, MarkSeverity};
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::filters::{FilterBar, SearchInput};
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::{ListLayout, DANGER_BTN};
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::function_analysis::PortKind;
use crate::components::function_node_preview::FunctionNodePreview;
use crate::components::function_ports::FunctionPortsPanel;
use crate::components::functions_reference::FunctionsReferenceModal;
use crate::components::icons;
use crate::state::functions::OPEN_FUNCTION;
use crate::state::{org, session, toasts};
use crate::util;

/// Rôle de l'utilisateur dans l'org courante — même helper que `flows.rs`
/// (privé par page, convention projet).
fn current_role() -> Option<String> {
    let user = session::user()?;
    let org_id = org::current()?;
    user.orgs
        .iter()
        .find(|m| m.id == org_id)
        .map(|m| m.role.clone())
}

#[component]
pub fn Functions() -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut selected = use_signal(|| None::<i64>);
    // Global search deep link (D69): consume the requested function id once
    // (guard prevents the effect from re-arming — see flows.rs comment).
    let mut deep_link_done = use_signal(|| false);
    use_effect(move || {
        if deep_link_done() {
            return;
        }
        if let Some(id) = OPEN_FUNCTION() {
            selected.set(Some(id));
            OPEN_FUNCTION.with_mut(|f| *f = None);
            deep_link_done.set(true);
        }
    });
    let search = use_signal(String::new);
    // Pagination client-side (la recherche est aussi client-side et l'API
    // plafonne le référentiel à 200 entrées) — page remise à 0 par le clamp
    // ci-dessous quand la recherche réduit le résultat.
    let page = use_signal(|| 0i64);
    let mut create_open = use_signal(|| false);
    // Cible de suppression (id, nom) — confirmation à la demande.
    let mut delete_target = use_signal(|| None::<(i64, String)>);

    let can_write = current_role().is_some_and(|role| crate::state::org::role_can_write(&role));

    let list = use_resource(move || {
        let _ = reload();
        async move { api::functions::list().await }
    });

    // Lecture synchrone de la ressource (doctrine socle CRUD) + filtre
    // client-side résolu hors rsx (pas de `let` ni de `continue` dans le
    // rsx). Iso : l'état vide porte sur les lignes brutes — une recherche
    // sans résultat garde la table (en-têtes) rendue. La page est clampée
    // sur la plage utile (la recherche peut réduire le résultat).
    let (list_state, is_empty, rows, filtered_count) = match &*list.value().read() {
        Some(Ok(all)) => {
            let needle = search().trim().to_lowercase();
            let filtered: Vec<FunctionSummary> = all
                .iter()
                .filter(|s| needle.is_empty() || s.name.to_lowercase().contains(&needle))
                .cloned()
                .collect();
            let count = filtered.len() as i64;
            let current = page().clamp(0, (count - 1).max(0) / PAGE_SIZE);
            let rows: Vec<FunctionSummary> = filtered
                .into_iter()
                .skip((current * PAGE_SIZE) as usize)
                .take(PAGE_SIZE as usize)
                .collect();
            (Some(Ok(())), all.is_empty(), rows, count)
        }
        Some(Err(err)) => (Some(Err(err.clone())), false, Vec::new(), 0),
        None => (None, false, Vec::new(), 0),
    };

    // Colonnes de la table — les closures d'action capturent les signaux
    // (Copy) de la page.
    let columns = vec![
        Column::new(
            t!("functions-col-name").to_string(),
            |summary: &FunctionSummary| {
                rsx! { {summary.name.clone()} }
            },
        )
        .with_td_class("font-medium text-gray-900"),
        Column::new(
            t!("functions-col-language").to_string(),
            |summary: &FunctionSummary| {
                rsx! {
                    span { class: "inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium {language_badge(summary.language)} w-fit",
                        {language_label(summary.language)}
                    }
                }
            },
        ),
        Column::new(
            t!("functions-col-version").to_string(),
            |summary: &FunctionSummary| {
                rsx! { {format!("v{}", summary.current_version_number)} }
            },
        )
        .with_td_class("text-sm text-gray-600").secondary(),
        Column::new(
            t!("functions-col-updated").to_string(),
            |summary: &FunctionSummary| {
                rsx! { {date_label(&summary.updated_at)} }
            },
        )
        .with_td_class("text-gray-500 text-sm").secondary(),
        Column::new(
            t!("common-actions").to_string(),
            move |summary: &FunctionSummary| {
                let pk = summary.id;
                // Cloné avant la closure onclick : la référence `summary`
                // ne sort pas du corps de cellule (E0521).
                let name_delete = summary.name.clone();
                rsx! {
                    div { class: "flex items-center gap-1.5",
                        button {
                            class: "px-3 py-1 text-sm bg-blue-100 text-blue-700 rounded-lg hover:bg-blue-200 transition-colors",
                            onclick: move |_| selected.set(Some(pk)),
                            {t!("functions-open")}
                        }
                        if can_write {
                            button {
                                class: DANGER_BTN,
                                onclick: move |_| delete_target.set(Some((pk, name_delete.clone()))),
                                icons::Trash2 { class: "h-3.5 w-3.5 inline mr-0.5" }
                                {t!("functions-delete")}
                            }
                        }
                    }
                }
            },
        ).actions(),
    ];

    rsx! {
        ListLayout {
            title: t!("nav-functions").to_string(),
            subtitle: Some(t!("functions-subtitle").to_string()),
            // Editor subview draws its own header — drop the CRUD title and
            // "New function" button entirely (we are in the editor, not in
            // the list).
            header: selected().is_none(),
            can_write,
            add_label: Some(t!("functions-new").to_string()),
            on_add: move |_| create_open.set(true),
            if org::current().is_none() {
                p { class: "text-gray-500 text-center py-12", {t!("orgs-empty")} }
            } else {
                match selected() {
                    Some(fn_id) => rsx! {
                        FunctionEditor {
                            key: "{fn_id}",
                            fn_id,
                            can_write,
                            on_back: move |_| {
                                selected.set(None);
                                // Retour à la liste : refetch — la vérité
                                // serveur (create, renommage, save faits dans
                                // l'éditeur) s'affiche sans bouton refresh.
                                reload.with_mut(|r| *r += 1);
                            },
                        }
                    },
                    None => rsx! {
                        // Recherche client-side (la liste ramène jusqu'à 200
                        // entrées — un référentiel de fonctions ne pagine pas) :
                        // filtrage live à la frappe, le signal suffit.
                        FilterBar {
                            SearchInput {
                                placeholder: t!("functions-search-placeholder").to_string(),
                                value: search,
                                on_submit: move |_| {},
                                on_refresh: move |_| reload.with_mut(|r| *r += 1),
                            }
                        }

                        ListStates {
                            state: list_state,
                            is_empty,
                            empty_message: t!("functions-empty").to_string(),
                            DataTable {
                                columns,
                                rows,
                                row_key: RowKey::new(|summary: &FunctionSummary| { summary.id.to_string() }),
                            }
                            ListPager { count: filtered_count, page }
                        }

                        // Suppression confirmée d'une ligne.
                        if let Some((fn_id, fn_name)) = delete_target() {
                            ConfirmDialog {
                                title: t!("functions-confirm-delete-title"),
                                message: t!(
                                    "common-quoted-message", name : fn_name.clone(), message :
                                    t!("functions-confirm-delete-message")
                                ),
                                confirm_label: t!("functions-delete"),
                                on_confirm: move |_| {
                                    delete_target.set(None);
                                    let target = fn_id;
                                    spawn(async move {
                                        match api::functions::delete(target).await {
                                            Ok(()) => toasts::success("toast-function-deleted"),
                                            Err(err) => {
                                                match api::functions::referencing_flows(&err) {
                                                    Some(flows) => {
                                                        let list = flows
                                                            .iter()
                                                            .map(|(id, name, v)| format!("#{} {name} v{v}", id))
                                                            .collect::<Vec<_>>()
                                                            .join(", ");
                                                        toasts::error(
                                                            format!("{} {list}", t!("toast-function-in-use")),
                                                        );
                                                    }
                                                    None => toasts::error(err),
                                                }
                                            }
                                        }
                                        reload.with_mut(|r| *r += 1);
                                    });
                                },
                                on_cancel: move |_| delete_target.set(None),
                            }
                        }

                        // Modal de création (montée à la demande).
                        if create_open() {
                            CreateFunctionModal {
                                on_close: move |_| create_open.set(false),
                                on_created: move |fn_id| {
                                    create_open.set(false);
                                    selected.set(Some(fn_id));
                                    reload.with_mut(|r| *r += 1);
                                },
                            }
                        }
                    },
                }
            }
        }
    }
}

/// Couleur du badge de langage — classes littérales complètes (scan Tailwind).
fn language_badge(language: FunctionLanguage) -> &'static str {
    match language {
        FunctionLanguage::Js => "bg-amber-100 text-amber-800",
        FunctionLanguage::Starlark => "bg-sky-100 text-sky-800",
    }
}

fn language_label(language: FunctionLanguage) -> String {
    match language {
        FunctionLanguage::Js => t!("functions-lang-js").to_string(),
        FunctionLanguage::Starlark => t!("functions-lang-starlark").to_string(),
    }
}

/// Libellé i18n d'un type déclaré (aperçu de signature).
fn type_label(ty: FunctionType) -> String {
    match ty {
        FunctionType::Number => t!("functions-type-number").to_string(),
        FunctionType::String => t!("functions-type-string").to_string(),
        FunctionType::Bool => t!("functions-type-bool").to_string(),
        FunctionType::Any => t!("functions-type-any").to_string(),
    }
}

mod create_modal;
mod editor;
mod test_modal;
mod versions_drawer;

use create_modal::CreateFunctionModal;
use editor::FunctionEditor;
use test_modal::FunctionTestModal;
use versions_drawer::FunctionVersionsDrawer;
