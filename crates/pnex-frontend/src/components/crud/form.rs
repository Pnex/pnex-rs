//! Formulaire modal standardisé — `Modal` + pied cancel/submit. Les champs
//! restent dans `children` avec leurs signaux locaux à la page (idiome des
//! modales de création historiques) ; le pied seul est normalisé, avec les
//! mêmes classes que les pieds existants (consts partagées avec
//! `layout.rs`).

use dioxus::prelude::*;
use dioxus_i18n::t;

use super::layout::{GHOST_BTN, PRIMARY_BTN};
use crate::components::modal::Modal;

#[component]
pub fn FormDialog(
    /// Titre du modal (i18n résolu par l'appelant).
    title: String,
    /// Libellé du bouton de soumission (i18n résolu par l'appelant).
    submit_label: String,
    /// Fermeture (overlay, croix, bouton annuler).
    on_close: Callback<()>,
    /// Soumission — le handler de la page garde le spawn / toasts /
    /// fermeture / refetch.
    on_submit: Callback<()>,
    /// Opération serveur en cours → submit désactivé.
    busy: bool,
    /// Validation préalable → submit désactivé (défaut `true` ; poser
    /// `false` quand la page affiche son erreur inline **après** clic,
    /// le bouton ne doit pas être verrouillé avant).
    #[props(default = true)]
    valid: bool,
    #[props(default = "max-w-md".to_string())] max_width: String,
    children: Element,
) -> Element {
    rsx! {
        Modal { title, max_width, on_close,
            div { class: "space-y-4",
                {children}
                div { class: crate::components::modal::MODAL_FOOTER,
                    button {
                        class: GHOST_BTN,
                        r#type: "button",
                        onclick: move |_| on_close.call(()),
                        {t!("common-cancel")}
                    }
                    button {
                        class: "{PRIMARY_BTN} disabled:opacity-40 disabled:cursor-not-allowed",
                        r#type: "button",
                        disabled: busy || !valid,
                        onclick: move |_| on_submit.call(()),
                        {submit_label}
                    }
                }
            }
        }
    }
}
