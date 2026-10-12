//! Zones d'état standardisées d'une liste — loading / empty / error, le
//! match dupliqué verbatim dans chaque page historique. Volontairement
//! **non générique** : `state` ne porte que le discriminant
//! (`Option<Result<(), ApiError>>`) — les `children` sont construits par
//! la page depuis sa lecture synchrone de la ressource, un payload `T`
//! n'apporterait rien et imposerait `PartialEq` sur chaque type de liste.
//!
//! Réponse de la page (doctrine `use_resource` : lectures dans la partie
//! **synchrone** du corps de rendu) :
//!
//! ```ignore
//! let (state, is_empty, count, rows) = match &*list.value().read() {
//!     None => (None, false, 0, Vec::new()),
//!     Some(Ok(paged)) => (Some(Ok(())), paged.results.is_empty() && paged.count == 0,
//!                         paged.count, paged.results.clone()),
//!     Some(Err(err)) => (Some(Err(err.clone())), false, 0, Vec::new()),
//! };
//! ```

use dioxus::prelude::*;

use crate::api::error::ApiError;

#[component]
pub fn ListStates(
    /// Discriminant de la ressource — `None` = chargement,
    /// `Some(Err)` = erreur (message serveur affiché tel quel),
    /// `Some(Ok)` = données (vide → `empty_message`, sinon `children`).
    state: Option<Result<(), ApiError>>,
    /// Liste vide (consulté uniquement en `Some(Ok)`).
    is_empty: bool,
    /// Message vide — i18n résolu par l'appelant (clé `*-empty` de la page).
    empty_message: String,
    /// Icône facultative au-dessus du message (cercle gris, idiome catalog).
    #[props(default)]
    empty_icon: Option<Element>,
    /// Détail facultatif sous le message vide (hint du catalogue).
    #[props(default)]
    empty_detail: Option<Element>,
    children: Element,
) -> Element {
    match state {
        None => rsx! {
            div { class: "py-12 text-center",
                span { class: "pnex-spinner-delayed inline-block rounded-full h-8 w-8 border-b-2 border-signal" }
            }
        },
        Some(Err(err)) => rsx! {
            div {
                class: "rounded-lg border border-red-200 bg-red-50 p-4 text-sm text-alarm",
                role: "alert",
                {err.message.clone()}
            }
        },
        // The one empty-state form: what is missing, then what it is for.
        Some(Ok(())) if is_empty => rsx! {
            div { class: "flex flex-col items-center gap-2 rounded-lg border border-dashed border-line-strong px-6 py-12 text-center",
                if let Some(icon) = empty_icon {
                    div { class: "mb-2 flex h-12 w-12 items-center justify-center rounded-full bg-gray-100 text-slate",
                        {icon}
                    }
                }
                p { class: "max-w-prose text-base font-medium text-ink", {empty_message} }
                if let Some(detail) = empty_detail {
                    div { class: "max-w-prose text-sm text-slate", {detail} }
                }
            }
        },
        Some(Ok(())) => rsx! {
            {children}
        },
    }
}
