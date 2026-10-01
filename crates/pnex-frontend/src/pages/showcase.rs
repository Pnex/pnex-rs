//! Vitrine du socle CRUD (`/_showcase`) — chaque composant exposé isolément
//! avec des fixtures locales, **zéro appel réseau**. Page de référence dev
//! (non référencée dans la nav : accès URL direct) — libellés en dur
//! assumés ici, hors produit, contrairement à la doctrine i18n des pages.
//! Sert de banc de cohérence visuelle (et plus tard de tests visuels),
//! notamment pour le bouton retour agrandi, non exercé par flows.rs.

use dioxus::prelude::*;
use pnex_core::FlowSummary;

use crate::api::error::ApiError;
use crate::components::crud::filters::{FilterBar, RefreshButton, SearchInput};
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::ListLayout;
use crate::components::crud::pager::ListPager;
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::editor_shell::{
    Chip, EditorShell, EditorStatus, InspectorPanel, PaletteIcon, PaletteItem, PalettePopover,
    StatusTone,
};

/// Fixture flow (FlowSummary : tous les champs pub, Clone + PartialEq).
fn fixture(id: i64, name: &str, status: &str, deployed: Option<i64>, updated: &str) -> FlowSummary {
    FlowSummary {
        id,
        org_id: 1,
        device_id: None,
        name: name.to_string(),
        status: status.to_string(),
        deployed_version_number: deployed,
        latest_version_number: 3,
        created_at: updated.to_string(),
        updated_at: updated.to_string(),
    }
}

#[component]
pub fn Showcase() -> Element {
    // Démos interactives — signaux locaux, aucun appel API.
    let mut dialog_open = use_signal(|| false);
    let search = use_signal(String::new);
    let page = use_signal(|| 0i64);
    // SecretField demo: starts on an existing reference (no API call until
    // "Change" switches to the picker).
    let secret_draft = use_signal(|| {
        crate::components::secret_field::SecretDraft::Keep(pnex_core::SecretFieldView {
            secret_id: uuid::Uuid::nil(),
            name: "notify/astreinte/token".to_string(),
        })
    });

    let rows = vec![
        fixture(
            1,
            "cuve-chaudiere",
            "deployed",
            Some(2),
            "2026-09-12T10:00:00Z",
        ),
        fixture(2, "ingest-capteurs", "draft", None, "2026-09-15T08:30:00Z"),
        fixture(3, "alerte-seuil", "error", Some(1), "2026-09-17T16:45:00Z"),
    ];

    let columns = vec![
        Column::new("Nom".to_string(), |flow: &FlowSummary| {
            rsx! { {flow.name.clone()} }
        })
        .with_td_class("font-medium text-gray-900"),
        Column::new("Statut".to_string(), |flow: &FlowSummary| {
            let (badge, label) = match flow.status.as_str() {
                "deployed" => ("bg-green-100 text-green-800", "déployé"),
                "error" => ("bg-red-100 text-red-800", "erreur"),
                _ => ("bg-gray-100 text-gray-800", "brouillon"),
            };
            rsx! {
                span { class: "inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium {badge} w-fit",
                    "{label}"
                }
            }
        }),
        Column::new("Mise à jour".to_string(), |flow: &FlowSummary| {
            rsx! { {flow.updated_at.clone()} }
        }),
    ];

    rsx! {
        div { class: "p-6 space-y-10",
            div {
                h1 { class: "text-3xl font-bold text-gray-900", "/_showcase — socle CRUD" }
                p { class: "text-gray-600 mt-2",
                    "Référence visuelle des composants (fixtures locales, aucun appel API). "
                    "Doctrine : components/crud/README.md."
                }
            }

            // 1 — ListLayout minimal (titre + corps libre).
            ListLayout {
                title: "ListLayout — titre seul".to_string(),
                can_write: false,
                p { class: "text-gray-500", "Corps libre (children)." }
            }

            // 2 — ListLayout complet : retour agrandi + sous-titre + action.
            ListLayout {
                title: "ListLayout — retour agrandi + sous-titre + action".to_string(),
                subtitle: Some("Le retour est le nouveau canon (icône h-5 w-5, px-4 py-2.5).".to_string()),
                on_back: move |_| {},
                can_write: true,
                add_label: Some("Ajouter un élément".to_string()),
                on_add: move |_| dialog_open.set(true),
                p { class: "text-gray-500", "…" }
            }
            if dialog_open() {
                FormDialog {
                    title: "FormDialog — démo".to_string(),
                    submit_label: "Enregistrer".to_string(),
                    on_close: move |_| dialog_open.set(false),
                    on_submit: move |_| dialog_open.set(false),
                    busy: false,
                    p { class: "text-sm text-gray-600",
                        "Champs de démonstration — le pied cancel/submit est le pied standard du socle."
                    }
                }
            }

            // 3 — FilterBar (search + refresh).
            div {
                h2 { class: "text-lg font-semibold text-gray-900", "FilterBar / SearchInput / RefreshButton" }
                FilterBar {
                    SearchInput {
                        placeholder: "Rechercher…".to_string(),
                        value: search,
                        on_submit: move |_| {},
                    }
                    RefreshButton { on_click: move |_| {} }
                }
            }

            // 4 — ListStates : les 4 zones.
            div { class: "space-y-6",
                h2 { class: "text-lg font-semibold text-gray-900", "ListStates — les 4 états" }
                div {
                    p { class: "text-xs text-gray-400 mb-2", "loading" }
                    ListStates {
                        state: None,
                        is_empty: false,
                        empty_message: "…".to_string(),
                        p { class: "text-gray-400", "children jamais rendus dans cet état." }
                    }
                }
                div {
                    p { class: "text-xs text-gray-400 mb-2", "error" }
                    ListStates {
                        state: Some(Err(ApiError::new("HTTP 502 — backend indisponible"))),
                        is_empty: false,
                        empty_message: "…".to_string(),
                        p { class: "text-gray-400", "children jamais rendus dans cet état." }
                    }
                }
                div {
                    p { class: "text-xs text-gray-400 mb-2", "empty" }
                    ListStates {
                        state: Some(Ok(())),
                        is_empty: true,
                        empty_message: "Aucun élément pour le moment.".to_string(),
                        p { class: "text-gray-400", "children jamais rendus dans cet état." }
                    }
                }
                div {
                    p { class: "text-xs text-gray-400 mb-2", "data (children rendus)" }
                    ListStates {
                        state: Some(Ok(())),
                        is_empty: false,
                        empty_message: "…".to_string(),
                        div { class: "relative",
                            DataTable { columns: columns, rows: rows.clone(), row_key: RowKey::new(|flow: &FlowSummary| flow.id.to_string()) }
                            ListPager { count: 3, page: page }
                        }
                        p { class: "text-gray-400 mt-2", "Pager masqué (3 éléments, 1 page)." }
                    }
                }
            }

            // SecretField (D113): set / type a value / pick an existing secret.
            div { class: "max-w-md space-y-2",
                h2 { class: "text-lg font-semibold text-gray-900", "SecretField — champ secret" }
                crate::components::secret_field::SecretField {
                    label: "Token".to_string(),
                    draft: secret_draft,
                    can_manage: true,
                }
                p { class: "text-xs text-gray-500 font-mono", "{secret_draft():?}" }
            }

            // 5 — EditorShell : la coquille des trois éditeurs (statique,
            // pleine hauteur en fin de page).
            div {
                h2 { class: "text-lg font-semibold text-gray-900", "EditorShell — coquille d'éditeur" }
                p { class: "text-gray-600 mt-1 text-sm",
                    "Barre 3 zones (retour · nom · statut | version | actions), palette à la demande, inspecteur au clic, Échap pour fermer."
                }
            }
            EditorShell {
                on_back: move |_| {},
                title: "demo-flow".to_string(),
                subtitle: Some("#12".to_string()),
                status: EditorStatus::new(StatusTone::Green, "Déployé"),
                version: Some(22),
                extra_chips: rsx! {
                    Chip { tone: StatusTone::Amber, label: "Modifications non enregistrées".to_string() }
                    Chip { tone: StatusTone::Purple, label: "v12".to_string(), on_remove: move |_| {} }
                },
                actions: rsx! {
                    button { class: "px-3 py-1.5 text-sm text-gray-700 bg-gray-50 border border-gray-300 rounded-lg font-medium", "Versions" }
                    button { class: "px-3 py-1.5 text-sm bg-blue-600 text-white rounded-lg font-medium", "Enregistrer" }
                    button { class: "px-3 py-1.5 text-sm bg-emerald-600 text-white rounded-lg font-medium", "Déployer" }
                },
                canvas: rsx! { div { class: "h-full w-full bg-slate-50" } },
                palette: rsx! {
                    PalettePopover {
                        add_title: "Ajouter un nœud".to_string(),
                        title: "Nœuds".to_string(),
                        search_placeholder: "Rechercher…".to_string(),
                        items: vec![
                            PaletteItem::new("inject", "Inject")
                                .with_description("Déclenche : intervalle, cron ou une fois")
                                .with_icon(PaletteIcon::Activity, "bg-emerald-50 text-emerald-600"),
                            PaletteItem::new("device", "Device")
                                .with_description("Lit les dernières valeurs de pins d'appareils")
                                .with_icon(PaletteIcon::Cpu, "bg-amber-50 text-amber-600"),
                            PaletteItem::new("metric", "Metric")
                                .with_description("Écrit le résultat dans OpenObserve")
                                .with_icon(PaletteIcon::LineChart, "bg-pink-50 text-pink-600"),
                        ],
                        on_pick: move |_| {},
                    }
                },
                inspector: rsx! {
                    InspectorPanel {
                        title: "Inject".to_string(),
                        subtitle: Some("#N1".to_string()),
                        icon: PaletteIcon::Activity,
                        on_close: move |_| {},
                        body: rsx! {
                            div {
                                label { class: "block text-sm font-medium text-gray-700 mb-1", "Nom du nœud" }
                                input { class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm", placeholder: "Nom du nœud" }
                            }
                            p { class: "text-sm text-gray-500", "Formulaire de démonstration — le corps appartient à l'éditeur." }
                        },
                    }
                },
            }
        }
    }
}
