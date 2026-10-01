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
            div { class: "text-center py-12",
                span { class: "animate-spin inline-block rounded-full h-8 w-8 border-b-2 border-blue-600" }
            }
        },
        Some(Err(err)) => rsx! {
            div { class: "bg-red-50 border border-red-200 rounded-lg p-4 text-sm text-red-700", {err.message.clone()} }
        },
        Some(Ok(())) if is_empty => rsx! {
            div { class: "text-center py-12",
                if let Some(icon) = empty_icon {
                    div { class: "p-4 bg-gray-100 rounded-full w-16 h-16 mx-auto mb-4 flex items-center justify-center",
                        {icon}
                    }
                }
                p { class: "text-gray-500 text-lg", {empty_message} }
                if let Some(detail) = empty_detail {
                    {detail}
                }
            }
        },
        Some(Ok(())) => rsx! { {children} },
    }
}
