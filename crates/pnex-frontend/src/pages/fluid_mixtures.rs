//! Mélanges de fluides personnalisés — page CRUD dédiée (D1).
//! Liste filtrable + formulaire (composants, fractions, base
//! molaire/massique) ; validation structurelle `pnex-core` avant save,
//! validation CoolProp côté serveur (400 avec le message).
//! La page compose le socle CRUD : le formulaire inline historique passe
//! en FormDialog (uniformité des pages liste).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::components::crud::filters::{FilterBar, SearchInput};
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::{ListLayout, DANGER_BTN};
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::icons;
use pnex_core::{FluidMixtureComposition, FluidMixtureInput, MixtureBasis, MixtureComponent};

/// Ligne du formulaire : (fluide, fraction en string d'input).
type DraftRow = (String, String);

/// Form/banner error: structured invalid fraction (i18n-resolved at render
/// via `err-fluid-fraction-invalid`), raw message (pnex-core validation
/// violations, verbatim fallback) or API error (code-resolved, verbatim
/// fallback).
#[derive(Clone, Debug, PartialEq)]
enum FluidError {
    Fraction { fluid: String, value: String },
    Message(String),
    Api(crate::api::error::ApiError),
}

/// Display text of a [`FluidError`] — render-scope only (i18n context).
fn fluid_error_text(err: &FluidError) -> String {
    match err {
        FluidError::Fraction { fluid, value } => crate::api::error_i18n::resolve(
            "err-fluid-fraction-invalid",
            Some(serde_json::json!({ "fluid": fluid, "value": value })).as_ref(),
        ),
        FluidError::Message(text) => text.clone(),
        FluidError::Api(err) => crate::api::error_i18n::localize(err),
    }
}

#[component]
pub fn FluidMixtures() -> Element {
    let mut reload = use_signal(|| 0u32);
    let search = use_signal(String::new);
    // Formulaire : `None` = fermé ; `Some(id)` = édition, `Some(String::new())`
    // vs création distingue via `editing_id.is_empty()`.
    let mut editing_id = use_signal(|| Option::<String>::None);
    let mut name = use_signal(String::new);
    let mut description = use_signal(String::new);
    let mut basis = use_signal(|| MixtureBasis::Mole);
    let mut rows = use_signal(Vec::<DraftRow>::new);
    let mut error = use_signal(|| Option::<FluidError>::None);
    // Suppression en deux clics (pas de modal) — signal de la page.
    let mut confirm_delete = use_signal(|| Option::<String>::None);
    // Pagination client-side (l'API renvoie tout, la recherche est serveur).
    let page = use_signal(|| 0i64);

    // Liste des mélanges de l'org (une seule requête, reload manuel).
    let list = use_resource(move || {
        let value = search().trim().to_string();
        let search = (!value.is_empty()).then_some(value);
        async move {
            let _ = reload();
            api::fluid_mixtures::list_fluid_mixtures(search.as_deref()).await
        }
    });

    // Lecture synchrone de la ressource (doctrine socle CRUD) + pagination
    // client-side clampée (l'API renvoie tout, la recherche reste serveur).
    let (list_state, is_empty, mixtures, count) = match &*list.value().read() {
        None => (None, false, Vec::new(), 0),
        Some(Ok(all)) => {
            let total = all.results.len() as i64;
            let current = page().clamp(0, (total - 1).max(0) / PAGE_SIZE);
            let rows: Vec<pnex_core::FluidMixture> = all
                .results
                .iter()
                .skip((current * PAGE_SIZE) as usize)
                .take(PAGE_SIZE as usize)
                .cloned()
                .collect();
            (Some(Ok(())), all.results.is_empty(), rows, total)
        }
        Some(Err(e)) => (Some(Err(e.clone())), false, Vec::new(), 0),
    };

    // Somme live des fractions (formulaire) + état d'édition, résolus hors
    // rsx (pas de `let` dans le rsx).
    let sum: f64 = rows()
        .iter()
        .filter_map(|(_, f)| f.trim().parse::<f64>().ok())
        .sum();
    let sum_ok = (sum - 1.0).abs() <= pnex_core::FRACTION_SUM_TOL;
    let sum_class = if sum_ok {
        "text-green-600"
    } else {
        "text-red-600"
    };
    let editing = editing_id().is_some();
    let is_edit = editing_id().as_deref().is_some_and(|id| !id.is_empty());

    // Colonnes de la table — nom (+ spec CoolProp sous le nom), description,
    // base, composants condensés, masse molaire, actions (modifier /
    // suppression en deux clics, pas de modal).
    let columns = vec![
        Column::new(
            t!("mixtures-name").to_string(),
            |m: &pnex_core::FluidMixture| {
                let spec = m.spec.clone();
                rsx! {
                    div { class: "text-sm font-medium text-gray-900", {m.name.clone()} }
                    if let Some(spec) = spec {
                        div { class: "text-xs text-gray-400 font-mono break-all", {spec} }
                    }
                }
            },
        ),
        Column::new(
            t!("mixtures-description").to_string(),
            |m: &pnex_core::FluidMixture| {
                rsx! { {m.description.clone().unwrap_or_else(|| "—".into())} }
            },
        )
        .with_td_class("text-gray-600").secondary(),
        Column::new(
            t!("mixtures-basis").to_string(),
            |m: &pnex_core::FluidMixture| {
                rsx! {
                    {match m.composition.basis {
                        MixtureBasis::Mole => t!("mixtures-basis-mole"),
                        MixtureBasis::Mass => t!("mixtures-basis-mass"),
                    }}
                }
            },
        )
        .with_td_class("text-gray-600").secondary(),
        Column::new(
            t!("mixtures-components").to_string(),
            |m: &pnex_core::FluidMixture| {
                let parts: Vec<String> = m
                    .composition
                    .components
                    .iter()
                    .map(|c| format!("{} {:.4}", c.fluid, c.fraction))
                    .collect();
                rsx! {
                    div { class: "text-sm font-mono text-gray-600 break-all", {parts.join(" · ")} }
                }
            },
        ),
        Column::new(
            t!("mixtures-molar-mass").to_string(),
            |m: &pnex_core::FluidMixture| {
                rsx! {
                    {match m.composition.molar_mass {
                        Some(mm) => format!("{mm:.4} kg/mol"),
                        None => "—".to_string(),
                    }}
                }
            },
        )
        .with_td_class("text-gray-500 text-sm").secondary(),
        Column::new(
            t!("common-actions").to_string(),
            move |m: &pnex_core::FluidMixture| {
                rsx! {
                    div { class: "flex gap-1",
                        button {
                            class: "px-3 py-1 text-sm border border-gray-300 rounded-lg hover:bg-gray-50",
                            onclick: {
                                let m_edit = m.clone();
                                move |_| {
                                    open_edit(
                                        &m_edit, &mut editing_id, &mut name, &mut description,
                                        &mut basis, &mut rows, &mut error,
                                    )
                                }
                            },
                            {t!("mixtures-edit")}
                        }
                        if confirm_delete() == Some(m.id.clone()) {
                            button {
                                class: DANGER_BTN,
                                onclick: {
                                    let m_del = m.clone();
                                    move |_| delete_mixture(m_del.id.clone(), confirm_delete, error, reload)
                                },
                                {t!("mixtures-confirm-delete")}
                            }
                        } else {
                            button {
                                class: DANGER_BTN,
                                onclick: {
                                    let m_del = m.clone();
                                    move |_| confirm_delete.set(Some(m_del.id.clone()))
                                },
                                icons::Trash2 { class: "h-3.5 w-3.5 inline mr-0.5" }
                                {t!("mixtures-delete")}
                            }
                        }
                    }
                }
            },
        ).actions(),
    ];

    rsx! {
        ListLayout {
            title: t!("mixtures-title").to_string(),
            subtitle: Some(t!("mixtures-subtitle").to_string()),
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            can_write: true,
            add_label: Some(t!("mixtures-new").to_string()),
            on_add: move |_| {
                open_form(
                    &mut editing_id,
                    &mut name,
                    &mut description,
                    &mut basis,
                    &mut rows,
                    &mut error,
                )
            },
            if editing {
                // Formulaire création/édition — FormDialog du socle (le
                // pied cancel/save est le pied standard ; la validation
                // somme=fractions verrouille le submit via `valid`).
                FormDialog {
                    title: if is_edit { t!("mixtures-edit-title").to_string() } else { t!("mixtures-new-title").to_string() },
                    submit_label: t!("mixtures-save").to_string(),
                    on_close: move |_| editing_id.set(None),
                    on_submit: move |_| { save_mixture(editing_id, name, description, basis, rows, error, reload) },
                    busy: false,
                    valid: sum_ok && !name().trim().is_empty(),
                    div { class: "grid grid-cols-1 md:grid-cols-2 gap-4",
                        div {
                            label {
                                r#for: "fluid-mixtures-field-1",
                                class: "block text-sm font-medium text-gray-700 mb-1",
                                {t!("mixtures-name")}
                            }
                            input {
                                id: "fluid-mixtures-field-1",
                                class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
                                placeholder: t!("mixtures-name-placeholder"),
                                value: "{name}",
                                oninput: move |e| name.set(e.value()),
                            }
                        }
                        div {
                            label {
                                r#for: "fluid-mixtures-field-2",
                                class: "block text-sm font-medium text-gray-700 mb-1",
                                {t!("mixtures-description")}
                            }
                            input {
                                id: "fluid-mixtures-field-2",
                                class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
                                value: "{description}",
                                oninput: move |e| description.set(e.value()),
                            }
                        }
                        div {
                            label {
                                r#for: "fluid-mixtures-field-3",
                                class: "block text-sm font-medium text-gray-700 mb-1",
                                {t!("mixtures-basis")}
                            }
                            select {
                                id: "fluid-mixtures-field-3",
                                class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                                onchange: move |e| {
                                    basis
                                        .set(
                                            match e.value().as_str() {
                                                "mass" => MixtureBasis::Mass,
                                                _ => MixtureBasis::Mole,
                                            },
                                        )
                                },
                                option {
                                    value: "mole",
                                    selected: basis() == MixtureBasis::Mole,
                                    {t!("mixtures-basis-mole")}
                                }
                                option {
                                    value: "mass",
                                    selected: basis() == MixtureBasis::Mass,
                                    {t!("mixtures-basis-mass")}
                                }
                            }
                        }
                        div { class: "flex items-end",
                            div { class: "text-sm {sum_class}", "{t!(\"mixtures-sum\")} : {sum:.4}" }
                        }
                        div { class: "text-sm text-gray-500 md:col-span-2",
                            {t!("mixtures-basis-hint")}
                        }
                    }

                    div {
                        div { class: "flex items-center justify-between mb-2",
                            p {
                                id: "fluid-mixtures-components-label",
                                class: "block text-sm font-medium text-gray-700",
                                {t!("mixtures-components")}
                            }
                            button {
                                class: "px-2 py-1 text-sm text-blue-600 hover:bg-blue-50 rounded",
                                onclick: move |_| rows.push(("".into(), "".into())),
                                {t!("mixtures-add-component")}
                            }
                        }
                        div {
                            class: "space-y-2",
                            role: "group",
                            aria_labelledby: "fluid-mixtures-components-label",
                            {
                                let indexed: Vec<(usize, DraftRow)> =
                                    rows().into_iter().enumerate().collect();
                                rsx! {
                                    for item in indexed {
                                        div { key: "{item.0}", class: "flex gap-2 items-center",
                                            input {
                                                class: "flex-1 px-3 py-2 border border-gray-300 rounded-lg text-sm font-mono",
                                                placeholder: t!("mixtures-fluid-placeholder"),
                                                value: "{item.1.0}",
                                                oninput: move |e| rows.with_mut(|r| r[item.0].0 = e.value()),
                                            }
                                            input {
                                                class: "w-32 px-3 py-2 border border-gray-300 rounded-lg text-sm font-mono",
                                                placeholder: t!("mixtures-fraction-placeholder"),
                                                value: "{item.1.1}",
                                                oninput: move |e| rows.with_mut(|r| r[item.0].1 = e.value()),
                                            }
                                            button {
                                                class: "px-2 py-1 text-sm text-red-600 rounded",
                                                onclick: move |_| {
                                                    let idx = item.0;
                                                    rows.with_mut(|r| {
                                                        if r.len() > 1 {
                                                            r.remove(idx);
                                                        }
                                                    });
                                                },
                                                "×"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if let Some(err) = error() {
                        div { class: "bg-red-50 border border-red-200 rounded-lg p-3 text-sm text-red-700",
                            {fluid_error_text(&err)}
                        }
                    }
                }
            } else {
                // Recherche + refresh — le filtrage est live à la frappe
                // (le signal est lu en partie synchrone de la resource).
                FilterBar {
                    SearchInput {
                        placeholder: t!("mixtures-search-placeholder").to_string(),
                        value: search,
                        on_submit: move |_| reload.with_mut(|r| *r += 1),
                    }
                }

                if let Some(err) = error() {
                    div { class: "bg-red-50 border border-red-200 rounded-lg p-4 text-sm text-red-700 mb-6",
                        {fluid_error_text(&err)}
                    }
                }

                ListStates {
                    state: list_state,
                    is_empty,
                    empty_message: t!("mixtures-empty").to_string(),
                    empty_detail: rsx! {
                        p { class: "text-sm text-gray-400 mt-2", {t!("mixtures-empty-hint")} }
                    },
                    div { class: "space-y-4",
                        DataTable {
                            columns,
                            rows: mixtures,
                            row_key: RowKey::new(|m: &pnex_core::FluidMixture| m.id.clone()),
                        }
                        ListPager { count, page }
                    }
                }
            }
        }
    }
}

/// Réinitialise le formulaire (nouveau mélange).
#[allow(clippy::too_many_arguments)]
fn open_form(
    editing_id: &mut Signal<Option<String>>,
    name: &mut Signal<String>,
    description: &mut Signal<String>,
    basis: &mut Signal<MixtureBasis>,
    rows: &mut Signal<Vec<DraftRow>>,
    error: &mut Signal<Option<FluidError>>,
) {
    editing_id.set(Some(String::new()));
    name.set(String::new());
    description.set(String::new());
    basis.set(MixtureBasis::Mole);
    rows.set(vec![
        ("Propane".into(), "0.5".into()),
        ("Ethane".into(), "0.5".into()),
    ]);
    error.set(None);
}

/// Pré-remplit le formulaire depuis un mélange existant (édition).
#[allow(clippy::too_many_arguments)]
fn open_edit(
    m: &pnex_core::FluidMixture,
    editing_id: &mut Signal<Option<String>>,
    name: &mut Signal<String>,
    description: &mut Signal<String>,
    basis: &mut Signal<MixtureBasis>,
    rows: &mut Signal<Vec<DraftRow>>,
    error: &mut Signal<Option<FluidError>>,
) {
    editing_id.set(Some(m.id.clone()));
    name.set(m.name.clone());
    description.set(m.description.clone().unwrap_or_default());
    basis.set(m.composition.basis);
    rows.set(
        m.composition
            .components
            .iter()
            .map(|c| (c.fluid.clone(), format!("{}", c.fraction)))
            .collect(),
    );
    error.set(None);
}

/// Valid composition from the form, or structured error.
fn draft_composition(
    basis: MixtureBasis,
    rows: &[DraftRow],
) -> Result<FluidMixtureComposition, FluidError> {
    let mut components: Vec<MixtureComponent> = Vec::new();
    for (fluid, fraction) in rows {
        let f: f64 = fraction.trim().parse().map_err(|_| FluidError::Fraction {
            fluid: fluid.clone(),
            value: fraction.clone(),
        })?;
        components.push(MixtureComponent {
            fluid: fluid.trim().to_string(),
            fraction: f,
        });
    }
    let composition = FluidMixtureComposition {
        basis,
        components,
        mole_fractions: vec![],
        molar_mass: None,
    };
    let violations = composition.validate();
    if violations.is_empty() {
        Ok(composition)
    } else {
        Err(FluidError::Message(
            violations
                .into_iter()
                .map(|v| v.message)
                .collect::<Vec<_>>()
                .join(" · "),
        ))
    }
}

/// Sauvegarde (create ou update selon editing_id).
#[allow(clippy::too_many_arguments)]
fn save_mixture(
    mut editing_id: Signal<Option<String>>,
    name: Signal<String>,
    description: Signal<String>,
    basis: Signal<MixtureBasis>,
    rows: Signal<Vec<DraftRow>>,
    mut error: Signal<Option<FluidError>>,
    mut reload: Signal<u32>,
) {
    let composition = match draft_composition(basis(), &rows()) {
        Ok(c) => c,
        Err(err) => {
            error.set(Some(err));
            return;
        }
    };
    let input = FluidMixtureInput {
        name: name().trim().to_string(),
        description: {
            let d = description().trim().to_string();
            (!d.is_empty()).then_some(d)
        },
        composition,
    };
    spawn(async move {
        let id = editing_id();
        let result = match id.as_deref() {
            Some(id) if !id.is_empty() => {
                api::fluid_mixtures::update_fluid_mixture(id, &input).await
            }
            _ => api::fluid_mixtures::create_fluid_mixture(&input).await,
        };
        match result {
            Ok(_) => {
                error.set(None);
                editing_id.set(None);
                reload.with_mut(|r| *r += 1);
            }
            Err(e) => error.set(Some(FluidError::Api(e))),
        }
    });
}

/// Suppression avec confirmation en deux clics (pas de modal).
fn delete_mixture(
    id: String,
    mut confirm: Signal<Option<String>>,
    mut error: Signal<Option<FluidError>>,
    mut reload: Signal<u32>,
) {
    let id2 = id.clone();
    spawn(async move {
        if let Err(e) = api::fluid_mixtures::delete_fluid_mixture(&id2).await {
            error.set(Some(FluidError::Api(e)));
            confirm.set(None);
            return;
        }
        confirm.set(None);
        reload.with_mut(|r| *r += 1);
    });
}
